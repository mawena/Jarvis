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
pub use types::{AgentCommand, EndReason, Progress, RunResult, TaskId, TaskRecord, TaskState};

use crate::{
    Announcer, CommandRunner, Notice, Outcome, Reply, Risk, Shortcut, Skill, SkillError, ToolSpec,
};
use git::{FlowNames, GitEnv, Prepared, RefSnapshot, Refusal, Violation};
use projects::ProjectList;
use registry::Registry;
use serde_json::{Value, json};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use supervisor::{Limits, RunOutcome, Running, Stopper};
use tokio::sync::watch;
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

/// Phrase de départ.
fn started_phrase(project: &str) -> String {
    format!(
        "Bien. Je confie cela à Claude, sur une branche à part du projet {project}. \
         Je vous préviens à la fin."
    )
}

const CLAUDE_MISSING: &str = "Claude Code est introuvable : je ne lance rien.";
const NO_TASK: &str = "Aucune tâche en cours.";

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
    active: Mutex<Vec<Active>>,
}

/// Tâche en cours (ou en préparation).
struct Active {
    id: TaskId,
    project: String,
    started_at: u64,
    /// Suivi et arrêt, une fois le processus lancé.
    run: Option<(watch::Receiver<Progress>, Stopper)>,
    /// Arrêt demandé pendant la préparation.
    stop_requested: bool,
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
                let task = tokio::spawn(inner.clone().start(project, instruction));
                task.await
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

impl Inner {
    /// Projet connu, sinon une phrase qui cite les projets connus.
    fn project(&self, spoken: &str) -> Result<&Project, SkillError> {
        self.projects.resolve(spoken).ok_or_else(|| {
            let names = self.projects.names();
            let known = if names.is_empty() {
                "Aucun projet n'est configuré.".to_string()
            } else {
                format!("Projets connus : {}.", names.join(", "))
            };
            SkillError::NotFound(format!("Je ne connais pas le projet « {spoken} ». {known}"))
        })
    }

    /// Départ d'une tâche : limites, garde-fous git, lancement, surveillance.
    async fn start(self: Arc<Self>, project: Project, instruction: String) -> Reply {
        let id = (self.ids)();
        let started_at = (self.now)();
        if let Err(refused) = self.reserve(&id, &project.name, started_at) {
            return refused;
        }
        match self.launch(&id, &project, instruction, started_at).await {
            Ok(()) => Reply::done(started_phrase(&project.name)),
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
                "Une tâche tourne déjà sur le projet {project}."
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
        });
        Ok(())
    }

    fn release(&self, id: &TaskId) {
        self.active.lock().unwrap().retain(|a| &a.id != id);
    }

    async fn launch(
        self: &Arc<Self>,
        id: &TaskId,
        project: &Project,
        instruction: String,
        started_at: u64,
    ) -> Result<(), Reply> {
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
            let before = git::snapshot(r, &repo, &names.protected);
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
                    return Err(Reply::failed(refusal.phrase(&project.name)));
                }
                Err(e) => {
                    warn!(erreur = %e, "préparation git interrompue");
                    return Err(Reply::failed(
                        Refusal::Git(String::new()).phrase(&project.name),
                    ));
                }
            };

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
                    "Je n'ai pas pu lancer Claude sur le projet {}. La branche {} reste en place.",
                    record.project, record.branch
                )));
            }
        };
        self.attach(id, &running);
        tokio::spawn(
            self.clone()
                .follow(record, project.path.clone(), before, running),
        );
        Ok(())
    }

    /// Note le suivi et l'arrêt d'une tâche lancée.
    fn attach(&self, id: &TaskId, running: &Running) {
        let mut active = self.active.lock().unwrap();
        if let Some(a) = active.iter_mut().find(|a| &a.id == id) {
            a.run = Some((running.progress_watch(), running.stopper()));
            if a.stop_requested {
                running.stopper().stop();
            }
        }
    }

    fn save(&self, record: &TaskRecord) {
        if let Err(e) = self.registry.save(record) {
            warn!(tache = %record.id, erreur = %e, "tâche non enregistrée");
        }
    }

    /// Fin d'une tâche : vérification, commit, registre, annonce.
    async fn follow(
        self: Arc<Self>,
        mut record: TaskRecord,
        repo: PathBuf,
        before: RefSnapshot,
        running: Running,
    ) {
        let outcome = running.wait().await;
        let result = outcome.progress.result.clone();
        let exited_ok = outcome.reason == EndReason::Exited
            && outcome.exit_code == Some(0)
            && result.as_ref().is_some_and(|r| !r.is_error);

        let runner = self.runner.clone();
        let commit_at_end = self.settings.commit_at_end;
        let message = commit_message(&record.instruction);
        let (worktree, branch, base_sha) = (
            record.worktree.clone(),
            record.branch.clone(),
            record.base_sha.clone(),
        );
        let checked = tokio::task::spawn_blocking(move || {
            let r = runner.as_ref();
            let violations = git::verify(r, &repo, &worktree, &before, &branch, &base_sha);
            let files = git::changed_files(r, &worktree, &base_sha);
            let commit = (exited_ok && violations.is_empty() && !files.is_empty() && commit_at_end)
                .then(|| git::commit_all(r, &worktree, &message));
            (violations, files, commit)
        })
        .await;
        let (violations, files, commit) = checked.unwrap_or_else(|e| {
            warn!(erreur = %e, "vérification après coup interrompue");
            (Vec::new(), Vec::new(), None)
        });
        if let Some(Err(e)) = &commit {
            warn!(tache = %record.id, erreur = %e, "commit de fin impossible");
        }

        record.ended_at = Some((self.now)());
        record.files_changed = Some(files.len());
        record.violations = violations.iter().map(Violation::phrase).collect();
        record.cost_usd = result.as_ref().and_then(|r| r.cost_usd);
        record.summary = result.as_ref().and_then(|r| claude::summary(&r.text));
        record.state = match outcome.reason {
            EndReason::Stopped => TaskState::Stopped,
            EndReason::TimedOut | EndReason::Idle => TaskState::TimedOut,
            EndReason::Exited if exited_ok && violations.is_empty() => TaskState::Succeeded,
            EndReason::Exited => TaskState::Failed,
        };
        info!(
            tache = %record.id,
            projet = %record.project,
            etat = ?record.state,
            fichiers = files.len(),
            "tâche d'agent terminée"
        );
        let notice = self.notice(&record, &outcome, files.len(), commit.as_ref());
        self.save(&record);
        self.release(&record.id);
        self.announcer.announce(notice);
    }

    /// Annonce de fin.
    fn notice(
        &self,
        record: &TaskRecord,
        outcome: &RunOutcome,
        files: usize,
        commit: Option<&Result<(), String>>,
    ) -> Notice {
        let p = &record.project;
        let (title, speech, result_outcome, urgent, speak) = if !record.violations.is_empty() {
            (
                format!("Attention : tâche sur {p}"),
                format!(
                    "Attention : {}. Vérifiez le projet {p}.",
                    join_and(&record.violations)
                ),
                Outcome::Failed,
                true,
                true,
            )
        } else {
            match outcome.reason {
                EndReason::Stopped => (
                    format!("Tâche arrêtée : {p}"),
                    format!("La tâche sur {p} a été arrêtée. Le travail reste sur sa branche."),
                    Outcome::Info,
                    false,
                    false,
                ),
                EndReason::TimedOut => (
                    format!("Tâche trop longue : {p}"),
                    format!(
                        "La tâche sur {p} a dépassé {} : je l'ai arrêtée.",
                        minutes(self.settings.max_minutes)
                    ),
                    Outcome::Failed,
                    false,
                    true,
                ),
                EndReason::Idle => (
                    format!("Tâche bloquée : {p}"),
                    format!(
                        "La tâche sur {p} ne donnait plus signe de vie depuis {} : je l'ai arrêtée.",
                        minutes(self.settings.idle_minutes)
                    ),
                    Outcome::Failed,
                    false,
                    true,
                ),
                EndReason::Exited if record.state == TaskState::Succeeded => {
                    let mut speech = match &record.summary {
                        Some(s) => format!("La tâche sur {p} est terminée : {}.", sentence(s)),
                        None => format!("La tâche sur {p} est terminée."),
                    };
                    speech.push(' ');
                    speech.push_str(&files_phrase(files));
                    if let Some(Err(_)) = commit {
                        speech.push_str(
                            " Je n'ai pas pu enregistrer le travail : il reste dans le worktree.",
                        );
                    }
                    (
                        format!("Tâche terminée : {p}"),
                        speech,
                        Outcome::Done,
                        false,
                        true,
                    )
                }
                EndReason::Exited => (
                    format!("Échec de la tâche : {p}"),
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
            body: details(&speech, record),
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
                    let since = minutes_since(now.saturating_sub(a.started_at));
                    let action = a
                        .run
                        .as_ref()
                        .and_then(|(progress, _)| progress.borrow().last_action.clone());
                    match action {
                        Some(action) => format!(
                            "La tâche sur {} tourne depuis {since} ; dernière action : {action}.",
                            a.project
                        ),
                        None => format!("La tâche sur {} tourne depuis {since}.", a.project),
                    }
                })
                .collect()
        };
        if !running.is_empty() {
            return Reply::new(running.join(" "));
        }
        let mut speech = match project {
            Some(p) => format!("Aucune tâche en cours sur {p}."),
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
            speech.push_str(&last_phrase(&r));
        }
        Reply::new(speech)
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
                Some(p) => format!("Aucune tâche en cours sur {p}."),
                None => NO_TASK.to_string(),
            }),
            [a] => {
                a.stop_requested = true;
                if let Some((_, stopper)) = &a.run {
                    stopper.stop();
                }
                info!(tache = %a.id, projet = %a.project, "arrêt de la tâche demandé");
                Reply::done(format!(
                    "J'arrête la tâche sur {}. Le travail reste sur sa branche.",
                    a.project
                ))
            }
            many => {
                let names: Vec<String> = many.iter().map(|a| a.project.clone()).collect();
                Reply::new(format!(
                    "{} tâches tournent, sur {} : dites « arrête la tâche {} », par exemple.",
                    count_word(names.len()),
                    join_and(&names),
                    names[0]
                ))
            }
        }
    }
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

/// Phrase sur la dernière tâche terminée.
fn last_phrase(r: &TaskRecord) -> String {
    let p = &r.project;
    match r.state {
        TaskState::Succeeded => match &r.summary {
            Some(s) => format!("La dernière, sur {p}, est terminée : {}.", sentence(s)),
            None => format!("La dernière, sur {p}, est terminée."),
        },
        TaskState::Failed => format!("La dernière, sur {p}, a échoué."),
        TaskState::Stopped => format!("La dernière, sur {p}, a été arrêtée."),
        TaskState::TimedOut => format!("La dernière, sur {p}, a dépassé le temps permis."),
        TaskState::Interrupted => {
            format!("La dernière, sur {p}, a été interrompue par un redémarrage.")
        }
        TaskState::Running => format!("La dernière, sur {p}, tourne encore."),
    }
}

/// Corps de la notification : la phrase, puis où trouver le travail.
fn details(speech: &str, r: &TaskRecord) -> String {
    format!(
        "{speech}\nBranche : {}\nDossier : {}\nReprendre : claude --resume {}\nJournal : {}",
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
        let (dir_j, jarvis) = temp_repo();
        let (dir_m, mawena) = temp_repo();
        let work = tempfile::tempdir().unwrap();
        let claude_dir = work.path().join("claude");
        std::fs::create_dir(&claude_dir).unwrap();
        let claude_path = fake_claude(&claude_dir, body);
        let project = |name: &str, path: &Path| Project {
            name: name.into(),
            path: path.to_path_buf(),
            aliases: Vec::new(),
            base: None,
            extra_allowed_tools: Vec::new(),
        };
        let mut settings = AgentSettings {
            claude_path,
            worktree_dir: work.path().join("worktrees"),
            cargo_target_dir: work.path().join("target"),
            extra_path: Vec::new(),
            projects: vec![project("Jarvis", &jarvis), project("Mawena", &mawena)],
            ..AgentSettings::default()
        };
        tweak(&mut settings);
        let registry = Registry::open(work.path().join("tasks")).unwrap();
        let announcer = Arc::new(RecordingAnnouncer::new());
        let clock = Arc::new(AtomicU64::new(T0));
        let counter = Arc::new(AtomicU64::new(0));
        let now = clock.clone();
        let skill = AgentSkill::new(
            settings,
            Arc::new(SystemRunner),
            announcer.clone(),
            registry,
        )
        .with_ids(Arc::new(move || {
            let n = counter.fetch_add(1, Ordering::SeqCst) + 1;
            TaskId(format!("{n:08x}-0000-4000-8000-000000000000"))
        }))
        .with_now(Arc::new(move || now.load(Ordering::SeqCst)))
        .with_limits(limits);
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

    const STARTED: &str = "Bien. Je confie cela à Claude, sur une branche à part du projet \
                           Jarvis. Je vous préviens à la fin.";

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
            Reply::failed(
                "Le projet Jarvis a des modifications non enregistrées : je ne lance rien."
            )
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
                "Je ne connais pas le projet « Zorglub ». Projets connus : Jarvis, Mawena."
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
            Reply::failed("Une tâche tourne déjà sur le projet Jarvis.")
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
            "La tâche sur Jarvis est terminée : J'ai fini les tests. 2 fichiers modifiés."
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
            "L'agent a échoué sur Jarvis : Les tests ne compilent pas."
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
            "L'agent a échoué sur Jarvis : il s'est arrêté avec le code 3."
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
            "La tâche sur Jarvis a dépassé 45 minutes : je l'ai arrêtée."
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
            Reply::done("J'arrête la tâche sur Jarvis. Le travail reste sur sa branche.")
        );
        let n = b.notice(1).await;
        assert!(!n.speak);
        assert!(!n.urgent);
        assert_eq!(b.records()[0].state, TaskState::Stopped);
        assert_eq!(
            b.status().await,
            "Aucune tâche en cours. La dernière, sur Jarvis, a été arrêtée."
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
                "Deux tâches tournent, sur Jarvis et Mawena : \
                 dites « arrête la tâche Jarvis », par exemple."
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
            "La tâche sur Jarvis tourne depuis moins d'une minute ; \
             dernière action : lance « cargo test --workspace »."
        );
        b.clock.store(T0 + 12 * 60 + 5, Ordering::SeqCst);
        assert_eq!(
            b.status().await,
            "La tâche sur Jarvis tourne depuis 12 minutes ; \
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
            "Aucune tâche en cours. La dernière, sur Jarvis, est terminée : J'ai fini les tests."
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
            "Attention : la branche develop a été modifiée. Vérifiez le projet Jarvis."
        );
        assert!(n.urgent && n.speak);
        assert_eq!(n.outcome, Outcome::Failed);
        let r = &b.records()[0];
        assert_eq!(r.state, TaskState::Failed);
        assert_eq!(r.violations, ["la branche develop a été modifiée"]);
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
