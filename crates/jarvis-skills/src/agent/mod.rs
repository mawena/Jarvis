//! Agent de code : Claude Code lancé en arrière-plan sur une branche à part d'un
//! projet connu, suivi, arrêté, puis annoncé à la fin.

pub mod claude;
pub mod git;
pub mod phrases;
pub mod projects;
pub mod registry;
pub mod settings;
pub mod supervisor;
#[cfg(any(test, feature = "test-util"))]
pub mod testing;
pub mod types;

pub use settings::{AgentSettings, DEFAULT_ALLOWED_TOOLS, DENIED_TOOLS, Project};
pub use types::{
    AgentCommand, EndReason, Progress, RunResult, SpokenName, TaskId, TaskRecord, TaskState,
};

use crate::{
    Announcer, CommandRunner, Notice, Outcome, Reply, Risk, Shortcut, Skill, SkillError, ToolSpec,
};
use git::{FlowNames, GitEnv, Prepared, RefSnapshot, Refusal, TaskTree, Violation};
use projects::ProjectList;
use registry::Registry;
use serde_json::{Value, json};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use supervisor::{Limits, RunOutcome, Running, Stopper};
use tokio::sync::{oneshot, watch};
use tracing::{info, warn};

/// Source des identifiants de tâche.
pub type IdSource = Arc<dyn Fn() -> TaskId + Send + Sync>;
/// Heure Unix en secondes.
pub type NowSource = Arc<dyn Fn() -> u64 + Send + Sync>;

/// Délai de `agent_start` : préparer un worktree d'un gros dépôt prend parfois
/// plus que le délai commun des compétences.
const START_TIMEOUT: Duration = Duration::from_secs(20);

/// Longueur de la consigne dans le message de commit.
const COMMIT_INSTRUCTION_MAX: usize = 60;

/// Tâches citées au plus par le suivi.
const STATUS_MAX: usize = 2;

/// Références changées citées une à une au plus dans une annonce.
const REFS_SPOKEN_MAX: usize = 3;

/// Délais de la fin d'une tâche.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EndLimits {
    /// Commit de fin, signature comprise (les autres git : `git::GIT_TIMEOUT`).
    pub commit: Duration,
    /// Filet de chaque étape (vérification, commit) : passé ce délai, la place
    /// est rendue et l'échec dit, même si git ne rend pas la main.
    pub step: Duration,
}

impl Default for EndLimits {
    fn default() -> Self {
        EndLimits {
            commit: git::COMMIT_TIMEOUT,
            step: Duration::from_secs(180),
        }
    }
}

/// Phrase de départ.
fn started_phrase(project: &SpokenName) -> String {
    format!(
        "Bien. Je confie cela à Claude, sur une branche à part {}. \
         Je vous préviens à la fin.",
        project.of_project()
    )
}

const CLAUDE_MISSING: &str = "Claude Code est introuvable : je ne lance rien.";
const NO_TASK: &str = "Aucune tâche en cours.";
const NOT_SAVED: &str = "Je n'ai pas pu enregistrer le travail : il reste dans le worktree.";

/// Compétence `agent` : départ, suivi et arrêt des tâches de code.
pub struct AgentSkill {
    inner: Arc<Inner>,
}

struct Inner {
    settings: AgentSettings,
    projects: ProjectList,
    runner: Arc<dyn CommandRunner>,
    announcer: Arc<dyn Announcer>,
    registry: Registry,
    ids: IdSource,
    now: NowSource,
    limits: Limits,
    end: EndLimits,
    active: Mutex<Vec<Active>>,
}

/// Tâche en cours (ou en préparation).
struct Active {
    id: TaskId,
    project: String,
    started_at: u64,
    /// Suivi et arrêt, une fois le processus lancé.
    run: Option<(watch::Receiver<Progress>, Stopper)>,
    /// Arrêt demandé (pendant la préparation, ou en cours).
    stop_requested: bool,
    /// Processus fini : vérification et commit en cours.
    finishing: bool,
}

impl AgentSkill {
    /// `settings` est développé (`~/`) ici.
    pub fn new(
        settings: AgentSettings,
        runner: Arc<dyn CommandRunner>,
        announcer: Arc<dyn Announcer>,
        registry: Registry,
    ) -> Self {
        let settings = settings.resolved();
        let limits = Limits::from_settings(&settings);
        Self {
            inner: Arc::new(Inner {
                projects: ProjectList::new(settings.projects.clone()),
                settings,
                runner,
                announcer,
                registry,
                ids: Arc::new(new_task_id),
                now: Arc::new(unix_now),
                limits,
                end: EndLimits::default(),
                active: Mutex::new(Vec::new()),
            }),
        }
    }

    fn inner_mut(&mut self) -> &mut Inner {
        Arc::get_mut(&mut self.inner).expect("réglage avant la première tâche")
    }

    /// Remplace la source des identifiants (tests).
    pub fn with_ids(mut self, ids: IdSource) -> Self {
        self.inner_mut().ids = ids;
        self
    }

    /// Remplace l'horloge (heure Unix, tests).
    pub fn with_now(mut self, now: NowSource) -> Self {
        self.inner_mut().now = now;
        self
    }

    /// Remplace les limites de durée (tests).
    pub fn with_limits(mut self, limits: Limits) -> Self {
        self.inner_mut().limits = limits;
        self
    }

    /// Remplace les délais de fin de tâche (tests).
    pub fn with_end_limits(mut self, end: EndLimits) -> Self {
        self.inner_mut().end = end;
        self
    }
}

/// Au démarrage du cerveau : les tâches restées `Running` deviennent
/// `Interrupted`, chacune notifiée sans voix. Renvoie leur nombre.
pub fn recover_interrupted(registry: &Registry, announcer: &dyn Announcer, now: u64) -> usize {
    let recovered = registry.recover(now);
    for r in &recovered {
        let speech = format!(
            "La tâche sur {} a été interrompue par un redémarrage de Jarvis.",
            r.project
        );
        announcer.announce(Notice {
            title: format!("Tâche interrompue : {}", r.project),
            body: details(&speech, r),
            speech,
            outcome: Outcome::Failed,
            urgent: false,
            speak: false,
        });
    }
    recovered.len()
}

#[async_trait::async_trait]
impl Skill for AgentSkill {
    fn tools(&self) -> Vec<ToolSpec> {
        let project = json!({"type": "string", "description": "Nom du projet."});
        vec![
            ToolSpec {
                name: "agent_start".into(),
                description: "Confie une tâche de code à Claude Code, sur une branche à part."
                    .into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "project": project,
                        "instruction": {"type": "string", "description": "Consigne de Charles."},
                    },
                    "required": ["project", "instruction"],
                }),
                risk: Risk::Direct,
            },
            ToolSpec {
                name: "agent_status".into(),
                description: "Dit où en sont les tâches de code.".into(),
                parameters: json!({"type": "object", "properties": {"project": project}}),
                risk: Risk::Direct,
            },
            ToolSpec {
                name: "agent_stop".into(),
                description: "Arrête une tâche de code.".into(),
                parameters: json!({"type": "object", "properties": {"project": project}}),
                risk: Risk::Direct,
            },
        ]
    }

    /// Jamais proposés à l'IA : seul le raccourci lance l'agent.
    fn llm_tools(&self) -> Vec<ToolSpec> {
        Vec::new()
    }

    fn shortcut(&self, text: &str) -> Option<Shortcut> {
        let (tool, args) = match phrases::parse(text, &self.inner.projects)? {
            AgentCommand::Start {
                project,
                instruction,
            } => (
                "agent_start",
                json!({"project": project, "instruction": instruction}),
            ),
            AgentCommand::UnknownProject { heard } => (
                "agent_start",
                json!({"project": heard, "instruction": text.trim()}),
            ),
            AgentCommand::Status { project } => ("agent_status", json!({"project": project})),
            AgentCommand::Stop { project } => ("agent_stop", json!({"project": project})),
        };
        Some(Shortcut {
            tool: tool.into(),
            args,
        })
    }

    fn timeout(&self, tool: &str) -> Option<Duration> {
        (tool == "agent_start").then_some(START_TIMEOUT)
    }

    async fn call(&self, tool: &str, args: Value) -> Result<Reply, SkillError> {
        let inner = &self.inner;
        match tool {
            "agent_start" => {
                let project = inner.project(required(&args, "project")?)?.clone();
                let instruction = required(&args, "instruction")?.to_string();
                // Tâche à part : un appel abandonné ne laisse rien à moitié fait.
                // Si le cerveau n'attend plus (délai), la réponse est annoncée.
                let (tx, rx) = oneshot::channel();
                let task = inner.clone();
                tokio::spawn(async move {
                    let title = format!("Tâche de code : {}", project.name);
                    let reply = task.clone().start(project, instruction).await;
                    if let Err(reply) = tx.send(reply) {
                        info!("réponse du départ annoncée après le délai");
                        task.announcer.announce(Notice {
                            title,
                            body: reply.speech.clone(),
                            speech: reply.speech,
                            outcome: reply.outcome,
                            urgent: false,
                            speak: true,
                        });
                    }
                });
                rx.await
                    .map_err(|e| SkillError::Failed(format!("départ interrompu : {e}")))
            }
            "agent_status" => {
                let project = optional(&args, "project")
                    .map(|p| inner.project(p).cloned())
                    .transpose()?;
                Ok(inner.status(project.as_ref().map(|p| p.name.as_str())))
            }
            "agent_stop" => {
                let project = optional(&args, "project")
                    .map(|p| inner.project(p).cloned())
                    .transpose()?;
                Ok(inner.stop(project.as_ref().map(|p| p.name.as_str())))
            }
            other => Err(SkillError::UnknownTool(other.into())),
        }
    }
}

/// Argument texte obligatoire, non vide.
fn required<'a>(args: &'a Value, key: &str) -> Result<&'a str, SkillError> {
    optional(args, key).ok_or_else(|| SkillError::BadArgs(format!("« {key} » manquant")))
}

/// Argument texte facultatif (vide : absent).
fn optional<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// Résultat de l'étape de commit.
enum CommitStep {
    /// Commit fait ; problèmes vus à la relecture.
    Done(Vec<Violation>),
    /// Commit impossible : travail laissé dans le worktree.
    Failed(String),
}

impl Inner {
    /// Projet connu, sinon une phrase qui cite les projets connus.
    fn project(&self, spoken: &str) -> Result<&Project, SkillError> {
        self.projects.resolve(spoken).ok_or_else(|| {
            let names: Vec<String> = self
                .projects
                .names()
                .into_iter()
                .map(|n| self.spoken(n).bare())
                .collect();
            let known = if names.is_empty() {
                "Aucun projet n'est configuré.".to_string()
            } else {
                format!("Projets connus : {}.", names.join(", "))
            };
            SkillError::NotFound(format!("Je ne connais pas le projet « {spoken} ». {known}"))
        })
    }

    /// Nom dit d'un projet (jamais le mot de réveil).
    fn spoken(&self, name: &str) -> SpokenName {
        match self.projects.resolve(name).filter(|p| p.name == name) {
            Some(p) => SpokenName::of(&p.name, &p.aliases),
            None => SpokenName::of(name, &[]),
        }
    }

    /// Départ d'une tâche : limites, garde-fous git, lancement, surveillance.
    async fn start(self: Arc<Self>, project: Project, instruction: String) -> Reply {
        let id = (self.ids)();
        let started_at = (self.now)();
        if let Err(refused) = self.reserve(&id, &project.name, started_at) {
            return refused;
        }
        let spoken = self.spoken(&project.name);
        match self.launch(&id, &project, instruction, started_at).await {
            Ok(Launched::Running) => Reply::done(started_phrase(&spoken)),
            Ok(Launched::StoppedAtOnce) => {
                Reply::new("Tâche annulée dès son départ. Sa branche reste en place.")
            }
            Err(refused) => {
                self.release(&id);
                refused
            }
        }
    }

    /// Réserve une place : une tâche par projet, `max_tasks` en tout.
    fn reserve(&self, id: &TaskId, project: &str, started_at: u64) -> Result<(), Reply> {
        let mut active = self.active.lock().unwrap();
        if active.iter().any(|a| a.project == project) {
            return Err(Reply::failed(format!(
                "Une tâche tourne déjà sur {}.",
                self.spoken(project).the_project()
            )));
        }
        let max = self.settings.max_tasks.max(1);
        if active.len() >= max {
            let tasks = if max == 1 {
                "Une tâche tourne déjà".to_string()
            } else {
                format!("{} tâches tournent déjà", count_word(max))
            };
            return Err(Reply::failed(format!(
                "{tasks} : attendez qu'une se termine."
            )));
        }
        active.push(Active {
            id: id.clone(),
            project: project.to_string(),
            started_at,
            run: None,
            stop_requested: false,
            finishing: false,
        });
        Ok(())
    }

    fn release(&self, id: &TaskId) {
        self.active.lock().unwrap().retain(|a| &a.id != id);
    }

    /// Arrêt demandé pour cette tâche ?
    fn stop_requested(&self, id: &TaskId) -> bool {
        self.active
            .lock()
            .unwrap()
            .iter()
            .any(|a| &a.id == id && a.stop_requested)
    }

    async fn launch(
        self: &Arc<Self>,
        id: &TaskId,
        project: &Project,
        instruction: String,
        started_at: u64,
    ) -> Result<Launched, Reply> {
        let spoken = self.spoken(&project.name);
        if !is_executable(&self.settings.claude_path) {
            warn!(chemin = %self.settings.claude_path.display(), "claude introuvable");
            return Err(Reply::failed(CLAUDE_MISSING));
        }

        let runner = self.runner.clone();
        let repo = project.path.clone();
        let base = project.base.clone();
        let worktree_dir = self.settings.worktree_dir.clone();
        let slug = git::slug(&instruction, &project.name);
        let task_id = id.clone();
        let prepared = tokio::task::spawn_blocking(move || {
            let r = runner.as_ref();
            let names = git::flow_names(r, &repo, base.as_deref());
            git::check_clean(r, &repo)?;
            let before = git::snapshot(r, &repo)
                .ok_or_else(|| Refusal::Git("relevé des références impossible".into()))?;
            let prepared = git::prepare(r, &repo, &names, &slug, &worktree_dir, &task_id)?;
            let env = git::no_push_env(r, &repo);
            Ok::<_, Refusal>((names, before, prepared, env))
        })
        .await;
        let (names, before, prepared, env): (FlowNames, RefSnapshot, Prepared, GitEnv) =
            match prepared {
                Ok(Ok(p)) => p,
                Ok(Err(refusal)) => {
                    warn!(projet = %project.name, refus = ?refusal, "tâche refusée");
                    return Err(Reply::failed(refusal.phrase(&spoken)));
                }
                Err(e) => {
                    warn!(erreur = %e, "préparation git interrompue");
                    return Err(Reply::failed(Refusal::Git(String::new()).phrase(&spoken)));
                }
            };

        // Arrêt demandé pendant la préparation : rien n'est lancé.
        if self.stop_requested(id) {
            info!(tache = %id, projet = %project.name, "tâche annulée avant son départ");
            let runner = self.runner.clone();
            let repo = project.path.clone();
            let p = prepared.clone();
            let removed =
                tokio::task::spawn_blocking(move || git::discard(runner.as_ref(), &repo, &p)).await;
            return Err(Reply::new(match removed {
                Ok(Ok(())) => "Tâche annulée avant son départ : sa branche est retirée.",
                other => {
                    warn!(resultat = ?other.map(|r| r.err()), "worktree annulé non retiré");
                    "Tâche annulée avant son départ : sa branche reste en place."
                }
            }));
        }

        let tree = prepared.tree();
        let mut record = TaskRecord {
            id: id.clone(),
            project: project.name.clone(),
            instruction,
            branch: prepared.branch,
            worktree: prepared.worktree,
            base: names.base,
            base_sha: prepared.base_sha,
            started_at,
            ended_at: None,
            state: TaskState::Running,
            summary: None,
            files_changed: None,
            violations: Vec::new(),
            cost_usd: None,
            log: self.registry.log_path(id),
        };
        self.save(&record);

        let inv = claude::build(&self.settings, project, &record, env.set, env.remove);
        info!(
            projet = %record.project,
            branche = %record.branch,
            consigne = %record.instruction,
            "consigne envoyée à Claude Code (en ligne)"
        );
        let running = match supervisor::spawn(inv, &record.log, self.limits) {
            Ok(running) => running,
            Err(e) => {
                warn!(erreur = %e, "lancement de Claude Code impossible");
                record.state = TaskState::Failed;
                record.ended_at = Some((self.now)());
                record.summary = Some(format!("lancement impossible : {e}"));
                self.save(&record);
                return Err(Reply::failed(format!(
                    "Je n'ai pas pu lancer Claude sur {}. Sa branche reste en place.",
                    spoken.the_project()
                )));
            }
        };
        let stopped = self.attach(id, &running);
        tokio::spawn(
            self.clone()
                .follow(record, project.path.clone(), tree, before, running),
        );
        Ok(if stopped {
            Launched::StoppedAtOnce
        } else {
            Launched::Running
        })
    }

    /// Note le suivi et l'arrêt d'une tâche lancée. Vrai si l'arrêt avait été
    /// demandé entre-temps (le processus est alors arrêté aussitôt).
    fn attach(&self, id: &TaskId, running: &Running) -> bool {
        let mut active = self.active.lock().unwrap();
        let Some(a) = active.iter_mut().find(|a| &a.id == id) else {
            return false;
        };
        a.run = Some((running.progress_watch(), running.stopper()));
        if a.stop_requested {
            running.stopper().stop();
        }
        a.stop_requested
    }

    fn save(&self, record: &TaskRecord) {
        if let Err(e) = self.registry.save(record) {
            warn!(tache = %record.id, erreur = %e, "tâche non enregistrée");
        }
    }

    /// Travail git bloquant, borné par le filet `end.step` : la tâche tokio
    /// reprend la main même si git ne la rend pas.
    async fn blocking<T: Send + 'static>(
        &self,
        work: impl FnOnce() -> Result<T, String> + Send + 'static,
    ) -> Result<T, String> {
        match tokio::time::timeout(self.end.step, tokio::task::spawn_blocking(work)).await {
            Ok(Ok(result)) => result,
            Ok(Err(e)) => Err(format!("travail git interrompu : {e}")),
            Err(_) => Err("délai dépassé".into()),
        }
    }

    /// Fin d'une tâche : vérification, commit, relecture, registre, annonce.
    async fn follow(
        self: Arc<Self>,
        mut record: TaskRecord,
        repo: PathBuf,
        tree: TaskTree,
        before: RefSnapshot,
        running: Running,
    ) {
        let outcome = running.wait().await;
        if let Some(a) = self
            .active
            .lock()
            .unwrap()
            .iter_mut()
            .find(|a| a.id == record.id)
        {
            a.finishing = true;
        }
        let result = outcome.progress.result.clone();
        let exited_ok = outcome.reason == EndReason::Exited
            && outcome.exit_code == Some(0)
            && result.as_ref().is_some_and(|r| !r.is_error);
        record.ended_at = Some((self.now)());
        record.cost_usd = result.as_ref().and_then(|r| r.cost_usd);
        record.summary = result.as_ref().and_then(|r| claude::summary(&r.text));
        let before = Arc::new(before);

        // Vérification après coup.
        let checked = {
            let (runner, repo, tree, before) = (
                self.runner.clone(),
                repo.clone(),
                tree.clone(),
                before.clone(),
            );
            let (branch, base_sha) = (record.branch.clone(), record.base_sha.clone());
            self.blocking(move || {
                let r = runner.as_ref();
                let violations = git::verify(r, &repo, &tree, &before, &branch, &base_sha)?;
                let files = git::changed_files(r, &tree, &base_sha)?;
                Ok((violations, files))
            })
            .await
        };
        let (mut violations, files) = match checked {
            Ok(checked) => checked,
            Err(e) => return self.unverified(record, e),
        };

        // Commit de fin, puis relecture : parent, branche, références.
        let mut commit = None;
        if exited_ok && violations.is_empty() && !files.is_empty() && self.settings.commit_at_end {
            let runner = self.runner.clone();
            let message = commit_message(&record.instruction);
            let (branch, base_sha) = (record.branch.clone(), record.base_sha.clone());
            let limit = self.end.commit;
            let tree = tree.clone();
            let step = self
                .blocking(move || {
                    let r = runner.as_ref();
                    let parent = match git::commit_all(r, &tree, &message, limit) {
                        Ok(parent) => parent,
                        Err(e) => return Ok(CommitStep::Failed(e)),
                    };
                    let mut problems = git::check_commit(r, &tree, &branch, &parent);
                    for v in git::verify(r, &repo, &tree, &before, &branch, &base_sha)? {
                        if !problems.contains(&v) {
                            problems.push(v);
                        }
                    }
                    Ok(CommitStep::Done(problems))
                })
                .await;
            match step {
                Ok(CommitStep::Done(problems)) => {
                    violations = problems;
                    commit = Some(Ok(()));
                }
                Ok(CommitStep::Failed(e)) => {
                    warn!(tache = %record.id, erreur = %e, "commit de fin impossible");
                    commit = Some(Err(e));
                }
                Err(e) => return self.unverified(record, e),
            }
        }

        record.files_changed = Some(files.len());
        record.violations = violations
            .iter()
            .map(|v| v.phrase(&record.branch))
            .collect();
        record.state = match outcome.reason {
            _ if !violations.is_empty() => TaskState::Failed,
            EndReason::Stopped => TaskState::Stopped,
            EndReason::TimedOut => TaskState::TimedOut,
            EndReason::Idle => TaskState::Stalled,
            EndReason::Exited if exited_ok => TaskState::Succeeded,
            EndReason::Exited => TaskState::Failed,
        };
        info!(
            tache = %record.id,
            projet = %record.project,
            etat = ?record.state,
            fichiers = files.len(),
            "tâche d'agent terminée"
        );
        let stop_requested = self.stop_requested(&record.id);
        let notice = self.notice(
            &record,
            &outcome,
            &violations,
            commit.as_ref(),
            stop_requested,
        );
        self.save(&record);
        self.release(&record.id);
        self.announcer.announce(notice);
    }

    /// Vérification ou commit impossible (délai, git interrompu) : échec urgent.
    fn unverified(&self, mut record: TaskRecord, error: String) {
        warn!(tache = %record.id, erreur = %error, "vérification après coup impossible");
        record.state = TaskState::Failed;
        record.violations = vec!["vérification impossible".into()];
        let speech = format!(
            "Je n'ai pas pu vérifier la tâche sur {} : vérifiez le projet.",
            self.spoken(&record.project).bare()
        );
        let notice = Notice {
            title: format!("Attention : tâche sur {}", record.project),
            body: details(&speech, &record),
            speech,
            outcome: Outcome::Failed,
            urgent: true,
            speak: true,
        };
        self.save(&record);
        self.release(&record.id);
        self.announcer.announce(notice);
    }

    /// Annonce de fin.
    fn notice(
        &self,
        record: &TaskRecord,
        outcome: &RunOutcome,
        violations: &[Violation],
        commit: Option<&Result<(), String>>,
        stop_requested: bool,
    ) -> Notice {
        let real = &record.project;
        let spoken = self.spoken(real);
        let p = spoken.bare();
        let files = record.files_changed.unwrap_or(0);
        let mut body_extra = String::new();
        let (title, speech, result_outcome, urgent, speak) = if !violations.is_empty() {
            let details: Vec<String> = violations.iter().filter_map(Violation::detail).collect();
            if !details.is_empty() {
                body_extra = format!("\nChangements : {}", details.join(", "));
            }
            (
                format!("Attention : tâche sur {real}"),
                format!(
                    "Attention : {}. Si ce n'est pas vous, vérifiez {}.",
                    violations_speech(violations, &record.branch),
                    spoken.the_project()
                ),
                Outcome::Failed,
                true,
                true,
            )
        } else {
            match outcome.reason {
                EndReason::Stopped => (
                    format!("Tâche arrêtée : {real}"),
                    format!("La tâche sur {p} a été arrêtée. Le travail reste sur sa branche."),
                    Outcome::Info,
                    false,
                    false,
                ),
                EndReason::TimedOut => (
                    format!("Tâche trop longue : {real}"),
                    format!(
                        "La tâche sur {p} a dépassé {} : je l'ai arrêtée.",
                        minutes(self.settings.max_minutes)
                    ),
                    Outcome::Failed,
                    false,
                    true,
                ),
                EndReason::Idle => (
                    format!("Tâche bloquée : {real}"),
                    format!(
                        "La tâche sur {p} ne donnait plus signe de vie depuis {} : je l'ai arrêtée.",
                        minutes(self.settings.idle_minutes)
                    ),
                    Outcome::Failed,
                    false,
                    true,
                ),
                EndReason::Exited if record.state == TaskState::Succeeded => {
                    // Arrêt demandé trop tard : la fin est dite comme telle.
                    let ended = if stop_requested {
                        "s'est terminée avant l'arrêt"
                    } else {
                        "est terminée"
                    };
                    let mut speech = match &record.summary {
                        Some(s) => format!("La tâche sur {p} {ended} : {}.", sentence(s)),
                        None => format!("La tâche sur {p} {ended}."),
                    };
                    speech.push(' ');
                    speech.push_str(&files_phrase(files));
                    if let Some(Err(_)) = commit {
                        speech.push(' ');
                        speech.push_str(NOT_SAVED);
                    }
                    (
                        format!("Tâche terminée : {real}"),
                        speech,
                        Outcome::Done,
                        false,
                        true,
                    )
                }
                EndReason::Exited => (
                    format!("Échec de la tâche : {real}"),
                    format!(
                        "L'agent a échoué sur {p} : {}.",
                        failure_reason(outcome, record)
                    ),
                    Outcome::Failed,
                    false,
                    true,
                ),
            }
        };
        Notice {
            title,
            body: details(&speech, record) + &body_extra,
            speech,
            outcome: result_outcome,
            urgent,
            speak,
        }
    }

    /// « Où en est la tâche ? »
    fn status(&self, project: Option<&str>) -> Reply {
        let now = (self.now)();
        let running: Vec<String> = {
            let active = self.active.lock().unwrap();
            active
                .iter()
                .filter(|a| project.is_none_or(|p| a.project == p))
                .take(STATUS_MAX)
                .map(|a| {
                    let p = self.spoken(&a.project).bare();
                    let since = minutes_since(now.saturating_sub(a.started_at));
                    let action = a
                        .run
                        .as_ref()
                        .and_then(|(progress, _)| progress.borrow().last_action.clone());
                    match action {
                        Some(action) => format!(
                            "La tâche sur {p} tourne depuis {since} ; dernière action : {action}."
                        ),
                        None => format!("La tâche sur {p} tourne depuis {since}."),
                    }
                })
                .collect()
        };
        if !running.is_empty() {
            return Reply::new(running.join(" "));
        }
        let mut speech = match project {
            Some(p) => format!("Aucune tâche en cours sur {}.", self.spoken(p).bare()),
            None => NO_TASK.to_string(),
        };
        let last = self
            .registry
            .load_all()
            .into_iter()
            .filter(|r| r.state != TaskState::Running)
            .filter(|r| project.is_none_or(|p| r.project == p))
            .max_by_key(|r| (r.ended_at.unwrap_or(r.started_at), r.started_at));
        if let Some(r) = last {
            speech.push(' ');
            speech.push_str(&self.last_phrase(&r));
        }
        Reply::new(speech)
    }

    /// Phrase sur la dernière tâche terminée.
    fn last_phrase(&self, r: &TaskRecord) -> String {
        let p = self.spoken(&r.project).bare();
        match r.state {
            TaskState::Succeeded => match &r.summary {
                Some(s) => format!("La dernière, sur {p}, est terminée : {}.", sentence(s)),
                None => format!("La dernière, sur {p}, est terminée."),
            },
            TaskState::Failed => format!("La dernière, sur {p}, a échoué."),
            TaskState::Stopped => format!("La dernière, sur {p}, a été arrêtée."),
            TaskState::TimedOut => format!("La dernière, sur {p}, a dépassé le temps permis."),
            TaskState::Stalled => format!(
                "La dernière, sur {p}, a été arrêtée : plus de signe de vie depuis {}.",
                minutes(self.settings.idle_minutes)
            ),
            TaskState::Interrupted => {
                format!("La dernière, sur {p}, a été interrompue par un redémarrage.")
            }
            TaskState::Running => format!("La dernière, sur {p}, tourne encore."),
        }
    }

    /// « Arrête la tâche. »
    fn stop(&self, project: Option<&str>) -> Reply {
        let mut active = self.active.lock().unwrap();
        let mut matching: Vec<&mut Active> = active
            .iter_mut()
            .filter(|a| project.is_none_or(|p| a.project == p))
            .collect();
        match matching.as_mut_slice() {
            [] => Reply::new(match project {
                Some(p) => format!("Aucune tâche en cours sur {}.", self.spoken(p).bare()),
                None => NO_TASK.to_string(),
            }),
            [a] => {
                let p = self.spoken(&a.project).bare();
                if a.finishing {
                    return Reply::new(format!(
                        "La tâche sur {p} vient de se terminer : je vous annonce le résultat \
                         dans un instant."
                    ));
                }
                a.stop_requested = true;
                info!(tache = %a.id, projet = %a.project, "arrêt de la tâche demandé");
                match &a.run {
                    Some((_, stopper)) => {
                        stopper.stop();
                        Reply::done(format!(
                            "J'arrête la tâche sur {p}. Le travail reste sur sa branche."
                        ))
                    }
                    None => Reply::done(format!("J'annule la tâche sur {p} avant son départ.")),
                }
            }
            many => {
                let names: Vec<SpokenName> = many.iter().map(|a| self.spoken(&a.project)).collect();
                let bare: Vec<String> = names.iter().map(SpokenName::bare).collect();
                // Exemple dicible : un projet appelé par son nom, sinon un alias.
                let example = names
                    .iter()
                    .find(|n| matches!(n, SpokenName::Name(_)))
                    .unwrap_or(&names[0])
                    .bare();
                Reply::new(format!(
                    "{} tâches tournent, sur {} : dites « arrête la tâche {example} », par exemple.",
                    count_word(names.len()),
                    join_and(&bare),
                ))
            }
        }
    }
}

/// Issue du lancement.
enum Launched {
    Running,
    /// Arrêt demandé juste avant que le processus ne parte : arrêté aussitôt.
    StoppedAtOnce,
}

/// Violations dites : les références changées groupées, puis le reste.
fn violations_speech(violations: &[Violation], branch: &str) -> String {
    let subjects: Vec<String> = violations
        .iter()
        .filter_map(|v| v.ref_subject(branch))
        .collect();
    let mut parts = Vec::new();
    match subjects.len() {
        0 => {}
        1 => parts.push(format!("{} a changé pendant la tâche", subjects[0])),
        n if n <= REFS_SPOKEN_MAX => parts.push(format!(
            "{} ont changé pendant la tâche",
            join_and(&subjects)
        )),
        n => parts.push(format!(
            "{}, {} et {} autres références ont changé pendant la tâche",
            subjects[0],
            subjects[1],
            n - 2
        )),
    }
    parts.extend(
        violations
            .iter()
            .filter(|v| v.ref_subject(branch).is_none())
            .map(|v| v.phrase(branch)),
    );
    join_and(&parts)
}

/// Raison courte d'un échec.
fn failure_reason(outcome: &RunOutcome, record: &TaskRecord) -> String {
    match (&outcome.progress.result, outcome.exit_code) {
        (Some(r), _) if r.is_error => match &record.summary {
            Some(s) => sentence(s).to_string(),
            None => match r.subtype.as_str() {
                "error_max_turns" => "trop d'étapes".into(),
                s if s.contains("budget") => "budget dépassé".into(),
                _ => "Claude Code a signalé une erreur".into(),
            },
        },
        (_, Some(code)) if code != 0 => format!("il s'est arrêté avec le code {code}"),
        (_, Some(_)) => "il s'est arrêté sans résultat".into(),
        (_, None) => "il a été tué par un signal".into(),
    }
}

/// Corps de la notification : la phrase, puis où trouver le travail.
fn details(speech: &str, r: &TaskRecord) -> String {
    format!(
        "{speech}\nProjet : {}\nBranche : {}\nDossier : {}\nReprendre : claude --resume {}\nJournal : {}",
        r.project,
        r.branch,
        r.worktree.display(),
        r.id,
        r.log.display()
    )
}

/// `wip(jarvis): <consigne, 60 caractères>`.
fn commit_message(instruction: &str) -> String {
    let one_line = instruction.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut short: String = one_line.chars().take(COMMIT_INSTRUCTION_MAX).collect();
    if one_line.chars().count() > COMMIT_INSTRUCTION_MAX {
        short = short.trim_end().to_string();
        short.push('…');
    }
    format!("wip(jarvis): {short}")
}

/// Phrase sans ponctuation finale (un point est ajouté après).
fn sentence(text: &str) -> &str {
    text.trim()
        .trim_end_matches(|c: char| c.is_whitespace() || ".!;:,…".contains(c))
}

fn files_phrase(n: usize) -> String {
    match n {
        0 => "Aucun fichier modifié.".into(),
        1 => "Un fichier modifié.".into(),
        n => format!("{n} fichiers modifiés."),
    }
}

fn minutes(n: u64) -> String {
    match n {
        1 => "une minute".into(),
        n => format!("{n} minutes"),
    }
}

/// Durée écoulée, dite en minutes.
fn minutes_since(secs: u64) -> String {
    match secs / 60 {
        0 => "moins d'une minute".into(),
        n => minutes(n),
    }
}

/// « Deux », « Trois »… (au féminin, en tête de phrase).
fn count_word(n: usize) -> String {
    const WORDS: &[&str] = &[
        "Zéro", "Une", "Deux", "Trois", "Quatre", "Cinq", "Six", "Sept", "Huit", "Neuf", "Dix",
    ];
    WORDS
        .get(n)
        .map_or_else(|| n.to_string(), |w| w.to_string())
}

/// « a, b et c ».
fn join_and(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} et {last}", rest.join(", ")),
    }
}

fn is_executable(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Identifiant de tâche : un UUID du noyau (aussi l'id de session de Claude Code).
fn new_task_id() -> TaskId {
    let uuid = std::fs::read_to_string("/proc/sys/kernel/random/uuid")
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    if uuid.len() == 36 {
        return TaskId(uuid);
    }
    // Repli : forme d'UUID tirée de l'heure et du pid.
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let n = nanos ^ (u128::from(std::process::id()) << 64);
    let hex = format!("{n:032x}");
    TaskId(format!(
        "{}-{}-4{}-8{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[13..16],
        &hex[17..20],
        &hex[20..32]
    ))
}

#[cfg(test)]
mod tests {
    use super::testing::{fake_claude, git, result_line, shell_quote, temp_repo, tool_use_line};
    use super::*;
    use crate::{RecordingAnnouncer, SystemRunner};
    use std::sync::atomic::{AtomicU64, Ordering};
    use tempfile::TempDir;

    /// Heure de départ des tests.
    const T0: u64 = 1_000_000;

    /// Deux projets (Jarvis et Mawena), un faux `claude`, un registre.
    struct Bench {
        _repos: (TempDir, TempDir),
        jarvis: PathBuf,
        mawena: PathBuf,
        work: TempDir,
        skill: AgentSkill,
        announcer: Arc<RecordingAnnouncer>,
        clock: Arc<AtomicU64>,
    }

    fn bench(body: &str) -> Bench {
        bench_with(body, |_| {}, short_limits())
    }

    fn short_limits() -> Limits {
        Limits {
            max_run: Duration::from_secs(60),
            idle: Duration::from_secs(60),
            grace: Duration::from_secs(1),
        }
    }

    fn bench_with(body: &str, tweak: impl FnOnce(&mut AgentSettings), limits: Limits) -> Bench {
        bench_full(
            body,
            tweak,
            limits,
            Arc::new(SystemRunner),
            EndLimits::default(),
        )
    }

    /// Banc avec un exécutant de commandes au choix (git ralenti, en panne…).
    fn bench_runner(body: &str, runner: Arc<dyn CommandRunner>, end: EndLimits) -> Bench {
        bench_full(body, |_| {}, short_limits(), runner, end)
    }

    fn bench_full(
        body: &str,
        tweak: impl FnOnce(&mut AgentSettings),
        limits: Limits,
        runner: Arc<dyn CommandRunner>,
        end: EndLimits,
    ) -> Bench {
        let (dir_j, jarvis) = temp_repo();
        let (dir_m, mawena) = temp_repo();
        let work = tempfile::tempdir().unwrap();
        let claude_dir = work.path().join("claude");
        std::fs::create_dir(&claude_dir).unwrap();
        let claude_path = fake_claude(&claude_dir, body);
        let project = |name: &str, path: &Path, aliases: &[&str]| Project {
            name: name.into(),
            path: path.to_path_buf(),
            aliases: aliases.iter().map(|a| a.to_string()).collect(),
            base: None,
            extra_allowed_tools: Vec::new(),
        };
        let mut settings = AgentSettings {
            claude_path,
            worktree_dir: work.path().join("worktrees"),
            cargo_target_dir: work.path().join("target"),
            extra_path: Vec::new(),
            // Comme la config par défaut : le projet Jarvis se dit « l'assistant ».
            projects: vec![
                project("Jarvis", &jarvis, &["l'assistant"]),
                project("Mawena", &mawena, &[]),
            ],
            ..AgentSettings::default()
        };
        tweak(&mut settings);
        let registry = Registry::open(work.path().join("tasks")).unwrap();
        let announcer = Arc::new(RecordingAnnouncer::new());
        let clock = Arc::new(AtomicU64::new(T0));
        let counter = Arc::new(AtomicU64::new(0));
        let now = clock.clone();
        let skill = AgentSkill::new(settings, runner, announcer.clone(), registry)
            .with_ids(Arc::new(move || {
                let n = counter.fetch_add(1, Ordering::SeqCst) + 1;
                TaskId(format!("{n:08x}-0000-4000-8000-000000000000"))
            }))
            .with_now(Arc::new(move || now.load(Ordering::SeqCst)))
            .with_limits(limits)
            .with_end_limits(end);
        Bench {
            _repos: (dir_j, dir_m),
            jarvis,
            mawena,
            work,
            skill,
            announcer,
            clock,
        }
    }

    impl Bench {
        async fn start(&self, project: &str) -> Reply {
            let instruction = format!("finis les tests du projet {project}");
            self.skill
                .call(
                    "agent_start",
                    json!({"project": project, "instruction": instruction}),
                )
                .await
                .unwrap()
        }

        async fn status(&self) -> String {
            self.skill
                .call("agent_status", json!({}))
                .await
                .unwrap()
                .speech
        }

        async fn stop(&self, project: Option<&str>) -> Reply {
            self.skill
                .call("agent_stop", json!({"project": project}))
                .await
                .unwrap()
        }

        /// Attend la `n`-ième annonce.
        async fn notice(&self, n: usize) -> Notice {
            for _ in 0..1500 {
                if let Some(notice) = self.announcer.notices().get(n - 1) {
                    return notice.clone();
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            panic!("aucune annonce n°{n} : {:?}", self.announcer.notices());
        }

        /// Attend que le suivi contienne `text`.
        async fn status_contains(&self, text: &str) -> String {
            for _ in 0..1500 {
                let s = self.status().await;
                if s.contains(text) {
                    return s;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            panic!("suivi sans « {text} » : {}", self.status().await);
        }

        fn records(&self) -> Vec<TaskRecord> {
            Registry::open(self.work.path().join("tasks"))
                .unwrap()
                .load_all()
        }

        fn worktrees(&self) -> Vec<PathBuf> {
            match std::fs::read_dir(self.work.path().join("worktrees")) {
                Ok(d) => d.map(|e| e.unwrap().path()).collect(),
                Err(_) => Vec::new(),
            }
        }

        fn claude_ran(&self) -> bool {
            self.work.path().join("claude/args").exists()
        }
    }

    fn branches(repo: &Path) -> Vec<String> {
        git(repo, &["branch", "--format=%(refname:short)"])
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// Agent qui travaille, réussit et laisse un résumé.
    fn successful_agent() -> String {
        let action = tool_use_line("Bash", json!({"command": "cargo test --workspace"}));
        let result = result_line(true, "Tout est fait.\nRÉSUMÉ : J'ai fini les tests.");
        format!(
            "printf '%s\\n' {}\n\
             echo 'pub fn deux() -> u32 {{ 2 }}' >> lib.rs\n\
             echo 'nouveau' > nouveau.rs\n\
             printf '%s\\n' {}",
            shell_quote(&action),
            shell_quote(&result)
        )
    }

    /// Agent qui annonce une action puis attend longtemps.
    fn long_agent() -> String {
        let action = tool_use_line("Bash", json!({"command": "cargo test --workspace"}));
        format!("printf '%s\\n' {}\nsleep 30", shell_quote(&action))
    }

    /// Agent qui casse un fichier et échoue.
    fn failing_agent() -> String {
        let result = result_line(false, "RÉSUMÉ : Les tests ne compilent pas.");
        format!(
            "echo 'cassé' >> lib.rs\nprintf '%s\\n' {}\nexit 1",
            shell_quote(&result)
        )
    }

    const STARTED: &str = "Bien. Je confie cela à Claude, sur une branche à part de l'assistant. \
                           Je vous préviens à la fin.";

    #[tokio::test]
    async fn demarrage_repond_tout_de_suite() {
        let b = bench(&long_agent());
        let begin = std::time::Instant::now();
        let r = b.start("Jarvis").await;
        assert!(begin.elapsed() < Duration::from_secs(5));
        assert_eq!(r, Reply::done(STARTED));
        // Branche feature dans un worktree ; la copie de Charles reste sur develop.
        let wt = b.worktrees();
        assert_eq!(wt.len(), 1, "{wt:?}");
        assert_eq!(
            git(&wt[0], &["branch", "--show-current"]),
            "feature/jarvis-finis-tests"
        );
        assert_eq!(git(&b.jarvis, &["branch", "--show-current"]), "develop");
        let records = b.records();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].state, TaskState::Running);
        assert_eq!(records[0].branch, "feature/jarvis-finis-tests");
        b.status_contains("dernière action").await;
        b.stop(None).await;
        b.notice(1).await;
        // La consigne passe par l'entrée standard.
        let stdin = std::fs::read_to_string(b.work.path().join("claude/stdin")).unwrap();
        assert_eq!(stdin, "finis les tests du projet Jarvis");
    }

    #[tokio::test]
    async fn projet_modifie_refuse_sans_rien_creer() {
        let b = bench(&successful_agent());
        std::fs::write(b.jarvis.join("lib.rs"), "modifié\n").unwrap();
        let r = b.start("Jarvis").await;
        assert_eq!(
            r,
            Reply::failed("L'assistant a des modifications non enregistrées : je ne lance rien.")
        );
        assert!(b.worktrees().is_empty());
        assert_eq!(branches(&b.jarvis), ["develop", "main"]);
        assert!(!b.claude_ran());
        assert!(b.records().is_empty());
        assert!(b.announcer.notices().is_empty());
        // La place est libérée : une fois l'arbre propre, la tâche part.
        git(&b.jarvis, &["checkout", "--", "lib.rs"]);
        assert_eq!(b.start("Jarvis").await.outcome, Outcome::Done);
        b.notice(1).await;
    }

    #[tokio::test]
    async fn projet_inconnu_dit_les_projets_connus() {
        let b = bench(&successful_agent());
        let s = b
            .skill
            .shortcut("Jarvis, corrige le bug du projet Zorglub")
            .unwrap();
        assert_eq!(s.tool, "agent_start");
        match b.skill.call(&s.tool, s.args).await {
            Err(SkillError::NotFound(m)) => assert_eq!(
                m,
                "Je ne connais pas le projet « Zorglub ». Projets connus : l'assistant, Mawena."
            ),
            other => panic!("{other:?}"),
        }
        assert!(b.worktrees().is_empty());
        assert!(!b.claude_ran());
    }

    #[tokio::test]
    async fn raccourcis_de_la_competence() {
        let b = bench(&successful_agent());
        let s = b
            .skill
            .shortcut("Jarvis, finis les tests du projet Jarvis.")
            .unwrap();
        assert_eq!(s.tool, "agent_start");
        assert_eq!(
            s.args,
            json!({"project": "Jarvis", "instruction": "finis les tests du projet Jarvis"})
        );
        let tool = |t: &str| b.skill.shortcut(t).map(|s| s.tool);
        assert_eq!(
            tool("où en est la tâche ?").as_deref(),
            Some("agent_status")
        );
        assert_eq!(tool("arrête la tâche").as_deref(), Some("agent_stop"));
        assert_eq!(tool("lance Jarvis"), None);
        assert_eq!(tool("arrête la musique"), None);
        assert!(b.skill.llm_tools().is_empty());
        assert!(b.skill.tools().iter().all(|t| t.risk == Risk::Direct));
    }

    #[tokio::test]
    async fn une_tache_par_projet() {
        let b = bench(&long_agent());
        assert_eq!(b.start("Jarvis").await.outcome, Outcome::Done);
        assert_eq!(
            b.start("Jarvis").await,
            Reply::failed("Une tâche tourne déjà sur l'assistant.")
        );
        assert_eq!(b.worktrees().len(), 1);
        // Un autre projet peut partir.
        assert_eq!(b.start("Mawena").await.outcome, Outcome::Done);
        b.stop(Some("Jarvis")).await;
        b.stop(Some("Mawena")).await;
        b.notice(2).await;
    }

    #[tokio::test]
    async fn nombre_maximal_de_taches() {
        let b = bench_with(&long_agent(), |s| s.max_tasks = 1, short_limits());
        assert_eq!(b.start("Jarvis").await.outcome, Outcome::Done);
        assert_eq!(
            b.start("Mawena").await,
            Reply::failed("Une tâche tourne déjà : attendez qu'une se termine.")
        );
        assert_eq!(branches(&b.mawena), ["develop", "main"]);
        b.stop(None).await;
        b.notice(1).await;
    }

    #[tokio::test]
    async fn claude_absent_refuse() {
        let b = bench_with(
            &successful_agent(),
            |s| s.claude_path = PathBuf::from("/nulle/part/claude"),
            short_limits(),
        );
        assert_eq!(b.start("Jarvis").await, Reply::failed(CLAUDE_MISSING));
        assert!(b.worktrees().is_empty());
        assert_eq!(branches(&b.jarvis), ["develop", "main"]);
        assert!(b.records().is_empty());
    }

    #[tokio::test]
    async fn tache_reussie_annoncee_avec_resume_et_fichiers() {
        let b = bench(&successful_agent());
        let develop = git(&b.jarvis, &["rev-parse", "develop"]);
        b.start("Jarvis").await;
        let n = b.notice(1).await;
        assert_eq!(
            n.speech,
            "La tâche sur l'assistant est terminée : J'ai fini les tests. 2 fichiers modifiés."
        );
        assert_eq!(n.outcome, Outcome::Done);
        assert!(n.speak && !n.urgent);
        assert!(n.body.contains("feature/jarvis-finis-tests"), "{}", n.body);
        assert!(
            n.body
                .contains("claude --resume 00000001-0000-4000-8000-000000000000"),
            "{}",
            n.body
        );
        // Un commit sur la branche, develop inchangée, worktree propre.
        let wt = &b.worktrees()[0];
        let branch = "feature/jarvis-finis-tests";
        assert_eq!(
            git(&b.jarvis, &["log", "-1", "--format=%s", branch]),
            "wip(jarvis): finis les tests du projet Jarvis"
        );
        assert_eq!(git(&b.jarvis, &["rev-parse", "develop"]), develop);
        assert_eq!(git(wt, &["status", "--porcelain"]), "");
        let message = git(&b.jarvis, &["log", "-1", "--format=%B", branch]);
        assert!(!message.contains("Co-Authored"), "{message}");
        let r = &b.records()[0];
        assert_eq!(r.state, TaskState::Succeeded);
        assert_eq!(r.files_changed, Some(2));
        assert_eq!(r.summary.as_deref(), Some("J'ai fini les tests."));
        assert_eq!(r.cost_usd, Some(0.12));
        // Journal complet.
        let log = std::fs::read_to_string(&r.log).unwrap();
        assert!(log.contains("RÉSUMÉ"), "{log}");
    }

    #[tokio::test]
    async fn tache_echouee_annoncee() {
        let b = bench(&failing_agent());
        b.start("Jarvis").await;
        let n = b.notice(1).await;
        assert_eq!(
            n.speech,
            "L'agent a échoué sur l'assistant : Les tests ne compilent pas."
        );
        assert_eq!(n.outcome, Outcome::Failed);
        assert!(n.speak && !n.urgent);
        assert_eq!(b.records()[0].state, TaskState::Failed);
    }

    #[tokio::test]
    async fn agent_sorti_sans_resultat_echoue() {
        let b = bench("exit 3");
        b.start("Jarvis").await;
        let n = b.notice(1).await;
        assert_eq!(
            n.speech,
            "L'agent a échoué sur l'assistant : il s'est arrêté avec le code 3."
        );
    }

    #[tokio::test]
    async fn pas_de_commit_si_echec() {
        let b = bench(&failing_agent());
        b.start("Jarvis").await;
        b.notice(1).await;
        let r = &b.records()[0];
        assert_eq!(
            git(&b.jarvis, &["rev-parse", &r.branch]),
            git(&b.jarvis, &["rev-parse", "develop"])
        );
        // Le travail reste dans le worktree, non enregistré.
        assert_eq!(git(&r.worktree, &["status", "--porcelain"]), " M lib.rs");
        assert_eq!(r.files_changed, Some(1));
    }

    #[tokio::test]
    async fn delai_depasse_annonce() {
        let limits = Limits {
            max_run: Duration::from_millis(300),
            ..short_limits()
        };
        let b = bench_with(&long_agent(), |_| {}, limits);
        b.start("Jarvis").await;
        let n = b.notice(1).await;
        assert_eq!(
            n.speech,
            "La tâche sur l'assistant a dépassé 45 minutes : je l'ai arrêtée."
        );
        assert_eq!(n.outcome, Outcome::Failed);
        assert!(n.speak);
        assert_eq!(b.records()[0].state, TaskState::TimedOut);
    }

    #[tokio::test]
    async fn arret_sans_annonce_vocale() {
        let b = bench(&long_agent());
        b.start("Jarvis").await;
        b.status_contains("dernière action").await;
        assert_eq!(
            b.stop(None).await,
            Reply::done("J'arrête la tâche sur l'assistant. Le travail reste sur sa branche.")
        );
        let n = b.notice(1).await;
        assert!(!n.speak);
        assert!(!n.urgent);
        assert_eq!(b.records()[0].state, TaskState::Stopped);
        assert_eq!(
            b.status().await,
            "Aucune tâche en cours. La dernière, sur l'assistant, a été arrêtée."
        );
    }

    #[tokio::test]
    async fn arret_sans_tache() {
        let b = bench(&long_agent());
        assert_eq!(b.stop(None).await, Reply::new("Aucune tâche en cours."));
        assert_eq!(
            b.stop(Some("Mawena")).await,
            Reply::new("Aucune tâche en cours sur Mawena.")
        );
    }

    #[tokio::test]
    async fn arret_ambigu_demande_laquelle() {
        let b = bench(&long_agent());
        b.start("Jarvis").await;
        b.start("Mawena").await;
        assert_eq!(
            b.stop(None).await,
            Reply::new(
                "Deux tâches tournent, sur l'assistant et Mawena : \
                 dites « arrête la tâche Mawena », par exemple."
            )
        );
        assert!(b.announcer.notices().is_empty());
        assert_eq!(
            b.stop(Some("Mawena")).await,
            Reply::done("J'arrête la tâche sur Mawena. Le travail reste sur sa branche.")
        );
        b.notice(1).await;
        assert_eq!(b.stop(None).await.outcome, Outcome::Done);
        b.notice(2).await;
    }

    #[tokio::test]
    async fn statut_sans_tache() {
        let b = bench(&long_agent());
        assert_eq!(b.status().await, "Aucune tâche en cours.");
    }

    #[tokio::test]
    async fn statut_tache_en_cours() {
        let b = bench(&long_agent());
        b.start("Jarvis").await;
        assert_eq!(
            b.status_contains("dernière action").await,
            "La tâche sur l'assistant tourne depuis moins d'une minute ; \
             dernière action : lance « cargo test --workspace »."
        );
        b.clock.store(T0 + 12 * 60 + 5, Ordering::SeqCst);
        assert_eq!(
            b.status().await,
            "La tâche sur l'assistant tourne depuis 12 minutes ; \
             dernière action : lance « cargo test --workspace »."
        );
        b.stop(None).await;
        b.notice(1).await;
    }

    #[tokio::test]
    async fn statut_apres_fin_dit_la_derniere() {
        let b = bench(&successful_agent());
        b.start("Jarvis").await;
        b.notice(1).await;
        assert_eq!(
            b.status().await,
            "Aucune tâche en cours. La dernière, sur l'assistant, est terminée : J'ai fini les tests."
        );
    }

    #[tokio::test]
    async fn fin_avec_violation_annonce_urgente() {
        let result = result_line(true, "RÉSUMÉ : Fini.");
        // Le faux agent enregistre lui-même puis déplace develop.
        let body = format!(
            "echo 'x' >> lib.rs\n\
             git commit -q -a -m agent\n\
             git update-ref refs/heads/develop HEAD\n\
             printf '%s\\n' {}",
            shell_quote(&result)
        );
        let b = bench(&body);
        b.start("Jarvis").await;
        let n = b.notice(1).await;
        assert_eq!(
            n.speech,
            "Attention : la branche develop a changé pendant la tâche. Si ce n'est pas vous, vérifiez l'assistant."
        );
        assert!(n.urgent && n.speak);
        assert_eq!(n.outcome, Outcome::Failed);
        let r = &b.records()[0];
        assert_eq!(r.state, TaskState::Failed);
        assert_eq!(
            r.violations,
            ["la branche develop a changé pendant la tâche"]
        );
        // Aucun commit de Jarvis par-dessus.
        assert_eq!(
            git(&b.jarvis, &["log", "-1", "--format=%s", &r.branch]),
            "agent"
        );
    }

    #[tokio::test]
    async fn taches_interrompues_notifiees_au_demarrage() {
        let tmp = tempfile::tempdir().unwrap();
        let reg = Registry::open(tmp.path()).unwrap();
        let id = TaskId("abc".into());
        reg.save(&TaskRecord {
            log: reg.log_path(&id),
            id,
            project: "Jarvis".into(),
            instruction: "finis les tests".into(),
            branch: "feature/jarvis-finis-tests".into(),
            worktree: PathBuf::from("/tmp/w"),
            base: "develop".into(),
            base_sha: "0123".into(),
            started_at: 10,
            ended_at: None,
            state: TaskState::Running,
            summary: None,
            files_changed: None,
            violations: vec![],
            cost_usd: None,
        })
        .unwrap();
        let a = RecordingAnnouncer::new();
        assert_eq!(recover_interrupted(&reg, &a, 99), 1);
        let notices = a.notices();
        assert_eq!(notices.len(), 1);
        assert_eq!(
            notices[0].speech,
            "La tâche sur Jarvis a été interrompue par un redémarrage de Jarvis."
        );
        assert!(!notices[0].speak);
        assert!(notices[0].body.contains("claude --resume abc"));
        assert_eq!(reg.load_all()[0].state, TaskState::Interrupted);
        assert_eq!(recover_interrupted(&reg, &a, 100), 0);
    }

    /// Exécutant qui montre chaque commande à `hook` avant de la lancer pour de vrai.
    struct HookRunner<F>(F);

    impl<F: Fn(&[String]) + Send + Sync> CommandRunner for HookRunner<F> {
        fn run(&self, program: &str, args: &[String]) -> std::io::Result<crate::CommandOutput> {
            (self.0)(args);
            SystemRunner.run(program, args)
        }

        fn spawn_detached(&self, program: &str, args: &[String]) -> std::io::Result<()> {
            SystemRunner.spawn_detached(program, args)
        }
    }

    fn hooked(hook: impl Fn(&[String]) + Send + Sync + 'static) -> Arc<dyn CommandRunner> {
        Arc::new(HookRunner(hook))
    }

    /// Sous-commande git (`commit`, `for-each-ref`…) et dossier `-C`.
    fn sub(args: &[String]) -> Option<(&str, &str)> {
        let i = args.iter().position(|a| a == "-C")?;
        Some((args.get(i + 2)?.as_str(), args.get(i + 1)?.as_str()))
    }

    fn is_sub(args: &[String], name: &str) -> bool {
        sub(args).is_some_and(|(s, _)| s == name)
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[tokio::test]
    async fn hooks_du_projet_jamais_lances() {
        let hooks = tempfile::tempdir().unwrap();
        let marker = hooks.path().join("lance");
        let hook = hooks.path().join("hook.sh");
        std::fs::write(
            &hook,
            format!(
                "#!/bin/sh\necho \"$0\" >> {}\n",
                shell_quote(&marker.display().to_string())
            ),
        )
        .unwrap();
        // L'agent ajoute des hooks suivis (comme .husky) et réussit.
        let body = format!(
            "mkdir -p .githooks\n\
             for h in pre-commit prepare-commit-msg commit-msg post-commit; do\n\
               install -m 755 {} .githooks/$h\n\
             done\n{}",
            shell_quote(&hook.display().to_string()),
            successful_agent()
        );
        let b = bench(&body);
        git(&b.jarvis, &["config", "core.hooksPath", ".githooks"]);
        b.start("Jarvis").await;
        let n = b.notice(1).await;
        assert_eq!(n.outcome, Outcome::Done, "{}", n.speech);
        assert!(
            !marker.exists(),
            "{}",
            std::fs::read_to_string(&marker).unwrap()
        );
        let r = &b.records()[0];
        assert_eq!(
            git(&b.jarvis, &["log", "-1", "--format=%s", &r.branch]),
            "wip(jarvis): finis les tests du projet Jarvis"
        );
    }

    #[tokio::test]
    async fn commit_relu_voit_une_branche_deplacee() {
        // Pendant le commit, quelque chose déplace develop.
        let moved = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = moved.clone();
        let runner = hooked(move |args| {
            if let Some(("rev-list", dir)) = sub(args)
                && !flag.swap(true, Ordering::SeqCst)
            {
                git(
                    Path::new(dir),
                    &["update-ref", "refs/heads/develop", "HEAD"],
                );
            }
        });
        let b = bench_runner(&successful_agent(), runner, EndLimits::default());
        b.start("Jarvis").await;
        let n = b.notice(1).await;
        assert_eq!(
            n.speech,
            "Attention : la branche develop a changé pendant la tâche. \
             Si ce n'est pas vous, vérifiez l'assistant."
        );
        assert!(n.urgent && n.speak);
        assert!(
            n.body.contains("Changements : refs/heads/develop"),
            "{}",
            n.body
        );
        let r = &b.records()[0];
        assert_eq!(r.state, TaskState::Failed);
        assert_eq!(
            git(&b.jarvis, &["log", "-1", "--format=%s", &r.branch]),
            "wip(jarvis): finis les tests du projet Jarvis"
        );
    }

    #[tokio::test]
    async fn verification_interrompue_annoncee_en_urgence() {
        // La vérification après coup plante (JoinError).
        let calls = Arc::new(AtomicU64::new(0));
        let count = calls.clone();
        let runner = hooked(move |args| {
            if is_sub(args, "for-each-ref") && count.fetch_add(1, Ordering::SeqCst) == 1 {
                panic!("git en panne (test)");
            }
        });
        let b = bench_runner(&successful_agent(), runner, EndLimits::default());
        b.start("Jarvis").await;
        let n = b.notice(1).await;
        assert_eq!(
            n.speech,
            "Je n'ai pas pu vérifier la tâche sur l'assistant : vérifiez le projet."
        );
        assert!(n.urgent && n.speak);
        assert_eq!(n.outcome, Outcome::Failed);
        assert_eq!(b.records()[0].state, TaskState::Failed);
        assert_eq!(
            b.status().await,
            "Aucune tâche en cours. La dernière, sur l'assistant, a échoué."
        );
        // Aucun commit sans vérification.
        let r = &b.records()[0];
        assert_eq!(git(&b.jarvis, &["rev-parse", &r.branch]), r.base_sha);
    }

    /// Chaque pid (un par ligne) disparaît (le temps d'être récolté).
    fn assert_dead(pids: &str) {
        for pid in pids.lines() {
            let alive = || {
                std::process::Command::new("kill")
                    .args(["-0", pid])
                    .stderr(std::process::Stdio::null())
                    .status()
                    .unwrap()
                    .success()
            };
            let mut tries = 0;
            while alive() && tries < 300 {
                std::thread::sleep(ms(10));
                tries += 1;
            }
            assert!(!alive(), "processus {pid} encore là");
        }
    }

    #[tokio::test]
    async fn commit_trop_long_libere_la_place() {
        let end = EndLimits {
            commit: ms(500),
            ..EndLimits::default()
        };
        let b = bench_runner(&successful_agent(), Arc::new(SystemRunner), end);
        // Signature qui ne rend jamais la main.
        let pids = b.work.path().join("pids");
        let signer = b.work.path().join("signe.sh");
        std::fs::write(
            &signer,
            format!(
                "#!/bin/sh\necho $$ >> '{0}'\nsleep 1000 &\necho $! >> '{0}'\nwait\n",
                pids.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&signer, std::fs::Permissions::from_mode(0o755)).unwrap();
        for (k, v) in [
            ("commit.gpgsign", "true"),
            ("gpg.format", "ssh"),
            ("user.signingkey", "/dev/null"),
            ("gpg.ssh.program", signer.to_str().unwrap()),
        ] {
            git(&b.jarvis, &["config", k, v]);
        }
        b.start("Jarvis").await;
        let n = b.notice(1).await;
        assert_eq!(
            n.speech,
            format!(
                "La tâche sur l'assistant est terminée : J'ai fini les tests. 2 fichiers modifiés. {NOT_SAVED}"
            )
        );
        assert_dead(&std::fs::read_to_string(&pids).unwrap());
        // La place est rendue.
        assert_eq!(b.start("Jarvis").await.outcome, Outcome::Done);
        b.notice(2).await;
    }

    #[tokio::test]
    async fn filet_rend_la_place_si_git_ne_repond_plus() {
        let runner = hooked(|args| {
            if is_sub(args, "commit") {
                std::thread::sleep(ms(1500));
            }
        });
        let end = EndLimits {
            step: ms(300),
            ..EndLimits::default()
        };
        let b = bench_runner(&successful_agent(), runner, end);
        b.start("Jarvis").await;
        let n = b.notice(1).await;
        assert_eq!(
            n.speech,
            "Je n'ai pas pu vérifier la tâche sur l'assistant : vérifiez le projet."
        );
        assert!(n.urgent);
        assert!(b.status().await.starts_with("Aucune tâche en cours."));
    }

    #[tokio::test]
    async fn depart_tardif_annonce() {
        let runner = hooked(|args| {
            if is_sub(args, "worktree") {
                std::thread::sleep(ms(300));
            }
        });
        let b = bench_runner(&long_agent(), runner, EndLimits::default());
        let call = b.skill.call(
            "agent_start",
            json!({"project": "Jarvis", "instruction": "finis les tests du projet Jarvis"}),
        );
        // Le cerveau abandonne l'appel (délai des compétences).
        assert!(tokio::time::timeout(ms(50), call).await.is_err());
        let n = b.notice(1).await;
        assert_eq!(n.speech, STARTED);
        assert!(n.speak && !n.urgent);
        assert_eq!(n.outcome, Outcome::Done);
        b.stop(None).await;
        b.notice(2).await;
    }

    #[tokio::test]
    async fn refus_tardif_annonce() {
        let runner = hooked(|args| {
            if is_sub(args, "status") {
                std::thread::sleep(ms(300));
            }
        });
        let b = bench_runner(&successful_agent(), runner, EndLimits::default());
        std::fs::write(b.jarvis.join("lib.rs"), "modifié\n").unwrap();
        let call = b.skill.call(
            "agent_start",
            json!({"project": "Jarvis", "instruction": "finis les tests"}),
        );
        assert!(tokio::time::timeout(ms(50), call).await.is_err());
        let n = b.notice(1).await;
        assert_eq!(
            n.speech,
            "L'assistant a des modifications non enregistrées : je ne lance rien."
        );
        assert_eq!(n.outcome, Outcome::Failed);
        assert!(n.speak);
    }

    #[tokio::test]
    async fn arret_pendant_la_preparation() {
        let runner = hooked(|args| {
            if is_sub(args, "worktree") && args.iter().any(|a| a == "add") {
                std::thread::sleep(ms(400));
            }
        });
        let b = bench_runner(&long_agent(), runner, EndLimits::default());
        let (started, stopped) = tokio::join!(b.start("Jarvis"), async {
            tokio::time::sleep(ms(150)).await;
            b.stop(None).await
        });
        assert_eq!(
            stopped,
            Reply::done("J'annule la tâche sur l'assistant avant son départ.")
        );
        assert_eq!(
            started,
            Reply::new("Tâche annulée avant son départ : sa branche est retirée.")
        );
        assert!(!b.claude_ran());
        assert!(b.worktrees().is_empty(), "{:?}", b.worktrees());
        assert_eq!(branches(&b.jarvis), ["develop", "main"]);
        assert!(b.records().is_empty());
        assert!(b.announcer.notices().is_empty());
        // Place rendue.
        assert_eq!(b.start("Jarvis").await, Reply::done(STARTED));
        b.stop(None).await;
        b.notice(1).await;
    }

    #[tokio::test]
    async fn arret_apres_la_fin_du_processus() {
        // Le processus a fini ; la vérification traîne.
        let entered = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let calls = Arc::new(AtomicU64::new(0));
        let (flag, count) = (entered.clone(), calls.clone());
        let runner = hooked(move |args| {
            if is_sub(args, "for-each-ref") && count.fetch_add(1, Ordering::SeqCst) == 1 {
                flag.store(true, Ordering::SeqCst);
                std::thread::sleep(ms(500));
            }
        });
        let b = bench_runner(&successful_agent(), runner, EndLimits::default());
        b.start("Jarvis").await;
        for _ in 0..500 {
            if entered.load(Ordering::SeqCst) {
                break;
            }
            tokio::time::sleep(ms(10)).await;
        }
        assert!(entered.load(Ordering::SeqCst));
        assert_eq!(
            b.stop(None).await,
            Reply::new(
                "La tâche sur l'assistant vient de se terminer : \
                 je vous annonce le résultat dans un instant."
            )
        );
        let n = b.notice(1).await;
        assert_eq!(
            n.speech,
            "La tâche sur l'assistant est terminée : J'ai fini les tests. 2 fichiers modifiés."
        );
    }

    #[tokio::test]
    async fn silence_trop_long_dit_dans_le_suivi() {
        let limits = Limits {
            idle: ms(300),
            ..short_limits()
        };
        let b = bench_with(&long_agent(), |s| s.idle_minutes = 30, limits);
        b.start("Jarvis").await;
        let n = b.notice(1).await;
        assert_eq!(
            n.speech,
            "La tâche sur l'assistant ne donnait plus signe de vie depuis 30 minutes : \
             je l'ai arrêtée."
        );
        assert_eq!(b.records()[0].state, TaskState::Stalled);
        assert_eq!(
            b.status().await,
            "Aucune tâche en cours. La dernière, sur l'assistant, a été arrêtée : \
             plus de signe de vie depuis 30 minutes."
        );
    }

    #[tokio::test]
    async fn nom_dit_jamais_le_mot_de_reveil() {
        let b = bench(&successful_agent());
        let n = {
            b.start("Jarvis").await;
            b.notice(1).await
        };
        assert!(!types::has_wake_word(&n.speech), "{}", n.speech);
        assert!(!types::has_wake_word(&b.status().await));
        // Le titre de la notification garde le vrai nom.
        assert_eq!(n.title, "Tâche terminée : Jarvis");
        assert!(n.body.contains("Projet : Jarvis"), "{}", n.body);
        // Les autres projets gardent leur nom.
        assert_eq!(
            b.start("Mawena").await,
            Reply::done(
                "Bien. Je confie cela à Claude, sur une branche à part du projet Mawena. \
                 Je vous préviens à la fin."
            )
        );
        b.notice(2).await;
    }

    #[test]
    fn violations_groupees_a_voix_haute() {
        let v = |r: &str| Violation::RefChanged(r.into());
        let b = "feature/jarvis-x";
        assert_eq!(
            violations_speech(
                &[
                    v("refs/heads/develop"),
                    v("refs/tags/v1"),
                    Violation::BaseLost
                ],
                b
            ),
            "la branche develop et le tag v1 ont changé pendant la tâche et \
             la branche de la tâche ne part plus de sa base"
        );
        let many: Vec<Violation> = (0..5).map(|i| v(&format!("refs/heads/b{i}"))).collect();
        assert_eq!(
            violations_speech(&many, b),
            "la branche b0, la branche b1 et 3 autres références ont changé pendant la tâche"
        );
        assert_eq!(
            violations_speech(&[v("refs/remotes/origin/feature/jarvis-x")], b),
            "la branche de la tâche a été poussée sur origin"
        );
    }

    #[test]
    fn message_de_commit_borne() {
        assert_eq!(
            commit_message("finis  les\ntests"),
            "wip(jarvis): finis les tests"
        );
        let m = commit_message(&"a ".repeat(100));
        assert!(m.chars().count() <= "wip(jarvis): ".len() + 61, "{m}");
        assert!(m.ends_with('…'));
    }

    #[test]
    fn identifiant_de_tache_en_uuid() {
        let id = new_task_id();
        assert_eq!(id.as_str().len(), 36, "{id}");
        assert_ne!(new_task_id(), id);
    }
}
