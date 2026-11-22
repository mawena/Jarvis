//! Grands chantiers : un long travail de code confié à Claude Code par la file
//! de tâches de CCS, sur le serveur, et non sur la machine de Jarvis.
//!
//! Jarvis joint le serveur par une commande (`[chantier] command`, en pratique
//! `ssh` avec une clé limitée au pont `scripts/jarvis-pont-ccs`) et lui passe un
//! ordre : `ajouter <projet> <consigne en hexadécimal>`, `voir <id>`,
//! `annuler <id>`. Le pont répond une ligne JSON. Les chantiers suivis sont
//! gardés dans un fichier d'état ; une veille interroge le pont et annonce la fin.
//!
//! Comme l'agent de code, un chantier n'est jamais lancé par l'IA (raccourci
//! seul), exige une voix reconnue (outils `agent_*`) et un « oui » (outil risqué).

use crate::agent::Project;
use crate::agent::phrases::original_words;
use crate::agent::projects::ProjectList;
use crate::text::fold;
use crate::{
    Announcer, CommandRunner, Notice, Outcome, Reply, Risk, Shortcut, Skill, SkillError, ToolSpec,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::{info, warn};

pub const TOOL_START: &str = "agent_chantier_lancer";
pub const TOOL_STATUS: &str = "agent_chantier_suivi";
pub const TOOL_CANCEL: &str = "agent_chantier_annuler";
pub const TOOL_HELP: &str = "agent_chantier_aide";

/// Longueur maximale d'une consigne (caractères).
const MAX_INSTRUCTION: usize = 2000;

/// Longueur de la consigne citée dans la question de confirmation.
const QUOTED_MAX: usize = 120;

/// Longueur du résumé de fin repris dans la notification.
const SUMMARY_MAX: usize = 400;

/// Chantiers terminés gardés dans le fichier d'état.
const KEEP_FINISHED: usize = 20;

/// Délai d'un appel au pont (connexion SSH comprise).
const BRIDGE_TIMEOUT: Duration = Duration::from_secs(30);

const UNREACHABLE: &str = "Je n'arrive pas à joindre le serveur des chantiers.";

/// Réglages de la section `[chantier]`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct ChantierSettings {
    /// Faux tant que le pont n'est pas installé.
    pub enabled: bool,
    /// Commande qui joint le pont ; l'ordre est ajouté à la fin. Un `~/` en tête
    /// d'argument est remplacé par le dossier personnel.
    pub command: Vec<String>,
    /// Intervalle de la veille des chantiers actifs (minutes).
    pub poll_minutes: u64,
    /// Projets qu'on peut nommer ; le pont retrouve leur dossier sur le serveur.
    pub projects: Vec<ChantierProject>,
}

impl Default for ChantierSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            command: Vec::new(),
            poll_minutes: 3,
            projects: Vec::new(),
        }
    }
}

/// Projet d'un chantier : nom (celui du dossier sur le serveur) et alias dits.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ChantierProject {
    pub name: String,
    #[serde(default)]
    pub aliases: Vec<String>,
}

/// Un chantier suivi.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chantier {
    /// Identifiant de la tâche CCS.
    pub id: String,
    pub project: String,
    pub instruction: String,
    /// Heure de lancement (secondes Unix).
    pub started_at: u64,
    /// État CCS : `queued`, `running`, `waiting`, `done`, `failed`, `cancelled`.
    pub state: String,
}

impl Chantier {
    pub fn is_active(&self) -> bool {
        is_active(&self.state)
    }
}

fn is_active(state: &str) -> bool {
    matches!(state, "queued" | "running" | "waiting")
}

/// Ordre compris dans une phrase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Start {
        project: String,
        instruction: String,
    },
    Status,
    Cancel {
        project: Option<String>,
    },
    /// Phrase de chantier incomplète : la réponse à dire.
    Help(String),
}

/// Mots retirés en tête.
const LEADING: &[&str] = &["jarvis", "stp", "svp", "bon", "alors", "allez"];

/// Mots permis devant « chantier » pour un départ.
const OPENING: &[&str] = &[
    "lance",
    "lancer",
    "lances",
    "ouvre",
    "ouvrir",
    "demarre",
    "demarrer",
    "commence",
    "commencer",
    "attaque",
    "attaquer",
    "nouveau",
    "grand",
    "gros",
    "un",
    "le",
    "mon",
    "tu",
    "peux",
    "on",
];

/// Au moins un de ces mots doit précéder « chantier » pour un départ.
const TRIGGERS: &[&str] = &[
    "lance",
    "lancer",
    "lances",
    "ouvre",
    "ouvrir",
    "demarre",
    "demarrer",
    "commence",
    "commencer",
    "attaque",
    "attaquer",
    "nouveau",
    "grand",
    "gros",
];

/// Liens entre « chantier » et le nom du projet.
const LINKS: &[&str] = &[
    "sur", "pour", "du", "de", "dans", "au", "le", "la", "l", "projet",
];

/// Liens entre le nom du projet et la consigne.
const INSTRUCTION_LINKS: &[&str] = &["pour", "et", "afin", "d", "de", "qui", "consiste", "a"];

/// Verbes d'arrêt, en tête.
const STOP: &[&str] = &[
    "annule",
    "annuler",
    "annulez",
    "arrete",
    "arreter",
    "arretez",
    "stoppe",
    "stop",
    "abandonne",
    "abandonner",
    "interromps",
];

/// Formules de suivi.
const STATUS: &[&[&str]] = &[
    &["ou", "en", "est"],
    &["ou", "en", "sont"],
    &["ca", "avance"],
    &["en", "est", "ou"],
    &["etat", "du"],
    &["etat", "des"],
    &["est", "fini"],
    &["est", "termine"],
    &["sont", "finis"],
    &["quels", "chantiers"],
];

/// Politesses retirées en fin de consigne.
const TRAILING: &[&[&str]] = &[
    &["s", "il", "te", "plait"],
    &["s", "il", "vous", "plait"],
    &["merci", "beaucoup"],
    &["merci"],
    &["stp"],
    &["svp"],
    &["jarvis"],
];

/// Ordre de chantier compris dans `text` (phrase d'origine), ou `None`.
pub fn parse(text: &str, projects: &ProjectList) -> Option<Command> {
    let places = original_words(text);
    let folded: Vec<String> = places.iter().map(|p| fold(&text[p.clone()])).collect();
    let all: Vec<&str> = folded.iter().map(String::as_str).collect();
    let skip = all.iter().take_while(|w| LEADING.contains(w)).count();
    let words = &all[skip..];
    let pos = words
        .iter()
        .position(|w| *w == "chantier" || *w == "chantiers")?;

    if STOP.contains(words.first()?) {
        let project = projects
            .find_in(&words[pos + 1..])
            .map(|(p, _, _)| p.name.clone());
        return Some(Command::Cancel { project });
    }
    if STATUS
        .iter()
        .any(|s| words.windows(s.len()).any(|w| w == *s))
    {
        return Some(Command::Status);
    }

    let before = &words[..pos];
    if before.len() > 5
        || !before.iter().all(|w| OPENING.contains(w))
        || !before.iter().any(|w| TRIGGERS.contains(w))
    {
        return None;
    }
    let after = &words[pos + 1..];
    let links = after.iter().take_while(|w| LINKS.contains(w)).count();
    // Le nom suit les liens (« …du projet Jarvis » : le mot d'éveil n'est un nom
    // que derrière « projet », d'où la recherche dans toute la suite).
    let (project, end) = match projects.find_in(after) {
        Some((p, start, end)) if start == links => (p, end),
        _ => return Some(Command::Help(which_project(projects))),
    };
    // Indices dans `all` de la consigne : après le nom, liens et politesses retirés.
    let mut first = skip + pos + 1 + end;
    while first < all.len() && INSTRUCTION_LINKS.contains(&all[first]) {
        first += 1;
    }
    let mut last = all.len();
    while let Some(t) = TRAILING
        .iter()
        .find(|t| last >= first + t.len() && all[first..last].ends_with(t))
    {
        last -= t.len();
    }
    if first >= last {
        return Some(Command::Help(format!(
            "Que dois-je construire sur {} ? Dites par exemple : grand chantier sur {} : \
             ajoute la synchronisation des réglages.",
            project.name, project.name
        )));
    }
    let instruction = text[places[first].start..places[last - 1].end].trim();
    let instruction: String = instruction.chars().take(MAX_INSTRUCTION).collect();
    Some(Command::Start {
        project: project.name.clone(),
        instruction,
    })
}

fn which_project(projects: &ProjectList) -> String {
    let names = projects.names();
    if names.is_empty() {
        "Aucun projet n'est déclaré pour les chantiers.".into()
    } else {
        format!("Sur quel projet ? Je connais : {}.", names.join(", "))
    }
}

/// Texte en hexadécimal (passe sans échappement dans une commande SSH).
pub fn to_hex(text: &str) -> String {
    text.bytes().map(|b| format!("{b:02x}")).collect()
}

/// `text` coupé à `max` caractères, avec « … ».
fn shorten(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        let cut: String = text.chars().take(max).collect();
        format!("{}…", cut.trim_end())
    }
}

/// Durée dite : « 5 minutes », « 2 heures 10 ».
fn spoken_duration(secs: u64) -> String {
    let minutes = secs / 60;
    match minutes {
        0 | 1 => "une minute".into(),
        m if m < 60 => format!("{m} minutes"),
        m if m % 60 == 0 && m / 60 == 1 => "une heure".into(),
        m if m % 60 == 0 => format!("{} heures", m / 60),
        m if m / 60 == 1 => format!("une heure {}", m % 60),
        m => format!("{} heures {}", m / 60, m % 60),
    }
}

fn state_phrase(state: &str) -> &'static str {
    match state {
        "queued" => "en attente d'un compte",
        "running" => "en cours",
        "waiting" => "en pause, limite de quota atteinte",
        "done" => "fini",
        "failed" => "échoué",
        "cancelled" => "annulé",
        _ => "dans un état inconnu",
    }
}

/// Heure Unix en secondes.
pub type Clock = Arc<dyn Fn() -> u64 + Send + Sync>;

fn system_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Compétence des grands chantiers.
#[derive(Clone)]
pub struct ChantierSkill {
    inner: Arc<Inner>,
}

struct Inner {
    poll: Duration,
    program: String,
    base_args: Vec<String>,
    projects: ProjectList,
    runner: Arc<dyn CommandRunner>,
    announcer: Arc<dyn Announcer>,
    state_path: Option<PathBuf>,
    now: Clock,
    list: Mutex<Vec<Chantier>>,
}

impl ChantierSkill {
    /// `state_path` : fichier des chantiers suivis (relu ici). `None` en test.
    pub fn new(
        settings: &ChantierSettings,
        runner: Arc<dyn CommandRunner>,
        announcer: Arc<dyn Announcer>,
        state_path: Option<PathBuf>,
    ) -> Self {
        Self::with_clock(
            settings,
            runner,
            announcer,
            state_path,
            Arc::new(system_now),
        )
    }

    pub fn with_clock(
        settings: &ChantierSettings,
        runner: Arc<dyn CommandRunner>,
        announcer: Arc<dyn Announcer>,
        state_path: Option<PathBuf>,
        now: Clock,
    ) -> Self {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let mut command = settings.command.iter().map(|a| expand(a, home.as_deref()));
        let program = command.next().unwrap_or_default();
        let base_args = command.collect();
        let projects = ProjectList::new(
            settings
                .projects
                .iter()
                .map(|p| Project {
                    name: p.name.clone(),
                    path: PathBuf::new(),
                    aliases: p.aliases.clone(),
                    base: None,
                    extra_allowed_tools: Vec::new(),
                })
                .collect(),
        );
        let list = state_path.as_deref().map(load).unwrap_or_default();
        Self {
            inner: Arc::new(Inner {
                poll: Duration::from_secs(settings.poll_minutes.max(1) * 60),
                program,
                base_args,
                projects,
                runner,
                announcer,
                state_path,
                now,
                list: Mutex::new(list),
            }),
        }
    }

    /// Chantiers suivis (actifs et derniers terminés).
    pub fn chantiers(&self) -> Vec<Chantier> {
        self.inner.list.lock().unwrap().clone()
    }

    /// Lance la veille : toutes les `poll_minutes`, l'état des chantiers actifs.
    pub fn spawn_watcher(&self) -> tokio::task::JoinHandle<()> {
        let skill = self.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(skill.inner.poll).await;
                skill.check_now().await;
            }
        })
    }

    /// Interroge le pont pour chaque chantier actif ; annonce les fins.
    pub async fn check_now(&self) {
        let active: Vec<String> = self
            .chantiers()
            .into_iter()
            .filter(Chantier::is_active)
            .map(|c| c.id)
            .collect();
        for id in active {
            match self.bridge(vec!["voir".into(), id.clone()]).await {
                Ok(v) => self.apply(&id, &v, true),
                Err(e) => warn!(chantier = %id, erreur = %e, "suivi du chantier impossible"),
            }
        }
    }

    /// Note l'état reçu du pont ; annonce une fin si `announce`.
    fn apply(&self, id: &str, v: &Value, announce: bool) {
        let Some(state) = v.get("etat").and_then(Value::as_str) else {
            return;
        };
        let finished = {
            let mut list = self.inner.list.lock().unwrap();
            let Some(c) = list.iter_mut().find(|c| c.id == id) else {
                return;
            };
            if c.state == state {
                return;
            }
            let was_active = c.is_active();
            c.state = state.to_string();
            (was_active && !c.is_active()).then(|| c.clone())
        };
        self.save();
        if let (Some(c), true) = (finished, announce) {
            self.announce_end(&c, v);
        }
    }

    fn announce_end(&self, c: &Chantier, v: &Value) {
        let summary = v
            .get("resume")
            .and_then(Value::as_str)
            .map(|s| shorten(s.trim(), SUMMARY_MAX))
            .unwrap_or_default();
        let (title, speech, outcome, speak) = match c.state.as_str() {
            "done" => (
                format!("Chantier {} fini", c.project),
                format!(
                    "Le grand chantier sur {} est fini. Le résumé est dans CCS.",
                    c.project
                ),
                Outcome::Done,
                true,
            ),
            "failed" => (
                format!("Chantier {} échoué", c.project),
                format!(
                    "Le grand chantier sur {} a échoué. Le détail est dans CCS.",
                    c.project
                ),
                Outcome::Failed,
                true,
            ),
            _ => (
                format!("Chantier {} annulé", c.project),
                String::new(),
                Outcome::Info,
                false,
            ),
        };
        let mut body = format!("Tâche CCS {} : {}", c.id, shorten(&c.instruction, 100));
        if !summary.is_empty() {
            body.push_str("\n\n");
            body.push_str(&summary);
        }
        info!(chantier = %c.id, projet = %c.project, etat = %c.state, "fin de chantier");
        self.inner.announcer.announce(Notice {
            title,
            speech,
            body,
            outcome,
            urgent: false,
            speak,
        });
    }

    /// Appelle le pont avec `order` ; renvoie sa réponse JSON.
    async fn bridge(&self, order: Vec<String>) -> Result<Value, String> {
        let inner = self.inner.clone();
        if inner.program.is_empty() {
            return Err("commande du pont non réglée ([chantier] command)".into());
        }
        let job = tokio::task::spawn_blocking(move || {
            let mut args = inner.base_args.clone();
            args.extend(order);
            inner.runner.run(&inner.program, &args)
        });
        let out = tokio::time::timeout(BRIDGE_TIMEOUT, job)
            .await
            .map_err(|_| "délai dépassé".to_string())?
            .map_err(|e| e.to_string())?
            .map_err(|e| format!("lancement de {} : {e}", self.inner.program))?;
        let line = out
            .stdout
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("");
        let v: Value = serde_json::from_str(line).map_err(|_| {
            if out.success {
                format!("réponse illisible : {}", shorten(line, 80))
            } else {
                "pont injoignable".to_string()
            }
        })?;
        if let Some(e) = v.get("erreur").and_then(Value::as_str) {
            return Err(e.to_string());
        }
        Ok(v)
    }

    fn save(&self) {
        let Some(path) = &self.inner.state_path else {
            return;
        };
        let mut list = self.inner.list.lock().unwrap();
        // Les actifs, puis les derniers terminés.
        let finished = list.iter().filter(|c| !c.is_active()).count();
        if finished > KEEP_FINISHED {
            let mut drop = finished - KEEP_FINISHED;
            list.retain(|c| {
                if drop > 0 && !c.is_active() {
                    drop -= 1;
                    false
                } else {
                    true
                }
            });
        }
        if let Err(e) = store(path, &list) {
            warn!(erreur = %e, fichier = %path.display(), "état des chantiers non enregistré");
        }
    }

    async fn start(&self, args: &Value) -> Result<Reply, SkillError> {
        let project = arg(args, "projet")?;
        let instruction = arg(args, "consigne")?;
        let Some(project) = self
            .inner
            .projects
            .resolve(&project)
            .map(|p| p.name.clone())
        else {
            return Ok(Reply::failed(which_project(&self.inner.projects)));
        };
        let instruction: String = instruction.trim().chars().take(MAX_INSTRUCTION).collect();
        if instruction.is_empty() {
            return Err(SkillError::BadArgs("consigne vide".into()));
        }
        let order = vec!["ajouter".into(), project.clone(), to_hex(&instruction)];
        let v = match self.bridge(order).await {
            Ok(v) => v,
            Err(e) => {
                warn!(erreur = %e, projet = %project, "chantier non lancé");
                return Ok(Reply::failed(if e.contains("projet") {
                    format!("Le serveur ne trouve pas le projet {project}.")
                } else {
                    UNREACHABLE.to_string()
                }));
            }
        };
        let Some(id) = v.get("id").and_then(|i| match i {
            Value::String(s) => Some(s.clone()),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        }) else {
            return Ok(Reply::failed(UNREACHABLE));
        };
        info!(chantier = %id, projet = %project, "grand chantier lancé");
        self.inner.list.lock().unwrap().push(Chantier {
            id,
            project: project.clone(),
            instruction,
            started_at: (self.inner.now)(),
            state: "queued".into(),
        });
        self.save();
        Ok(Reply::done(format!(
            "C'est lancé. Claude prend le chantier sur {project}, sur le serveur, dès qu'un \
             compte a du quota. Je vous préviens à la fin."
        )))
    }

    async fn status(&self) -> Reply {
        let active: Vec<String> = self
            .chantiers()
            .into_iter()
            .filter(Chantier::is_active)
            .map(|c| c.id)
            .collect();
        for id in &active {
            if let Ok(v) = self.bridge(vec!["voir".into(), id.clone()]).await {
                self.apply(id, &v, true);
            }
        }
        let now = (self.inner.now)();
        let list = self.chantiers();
        let mut phrases: Vec<String> = list
            .iter()
            .filter(|c| active.contains(&c.id))
            .map(|c| {
                format!(
                    "Le chantier sur {} est {}, lancé il y a {}.",
                    c.project,
                    state_phrase(&c.state),
                    spoken_duration(now.saturating_sub(c.started_at))
                )
            })
            .collect();
        if phrases.is_empty() {
            return match list.last() {
                Some(c) => Reply::new(format!(
                    "Aucun chantier en cours. Le dernier, sur {}, est {}.",
                    c.project,
                    state_phrase(&c.state)
                )),
                None => Reply::new("Aucun chantier en cours."),
            };
        }
        phrases.truncate(3);
        Reply::new(phrases.join(" "))
    }

    async fn cancel(&self, args: &Value) -> Reply {
        let project = args.get("projet").and_then(Value::as_str);
        let target = self
            .chantiers()
            .into_iter()
            .rev()
            .find(|c| c.is_active() && project.is_none_or(|p| c.project == p));
        let Some(c) = target else {
            return Reply::new("Aucun chantier en cours à annuler.");
        };
        match self.bridge(vec!["annuler".into(), c.id.clone()]).await {
            Ok(v) => {
                let state =
                    json!({"etat": v.get("etat").and_then(Value::as_str).unwrap_or("cancelled")});
                self.apply(&c.id, &state, false);
                Reply::done(format!("J'ai annulé le chantier sur {}.", c.project))
            }
            Err(e) => {
                warn!(chantier = %c.id, erreur = %e, "annulation impossible");
                Reply::failed(UNREACHABLE)
            }
        }
    }
}

fn arg(args: &Value, key: &str) -> Result<String, SkillError> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| SkillError::BadArgs(format!("« {key} » manquant")))
}

fn expand(arg: &str, home: Option<&Path>) -> String {
    match (arg.strip_prefix("~/"), home) {
        (Some(rest), Some(home)) => home.join(rest).to_string_lossy().into_owned(),
        _ => arg.to_string(),
    }
}

fn load(path: &Path) -> Vec<Chantier> {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
            warn!(erreur = %e, fichier = %path.display(), "état des chantiers illisible");
            Vec::new()
        }),
        Err(_) => Vec::new(),
    }
}

fn store(path: &Path, list: &[Chantier]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(list)?)?;
    std::fs::rename(tmp, path)
}

fn spec(name: &str, description: &str, parameters: Value, risk: Risk) -> ToolSpec {
    ToolSpec {
        name: name.into(),
        description: description.into(),
        parameters,
        risk,
    }
}

#[async_trait::async_trait]
impl Skill for ChantierSkill {
    fn tools(&self) -> Vec<ToolSpec> {
        vec![
            spec(
                TOOL_START,
                "Confie un grand chantier de code à Claude Code, par la file de tâches de CCS sur le serveur.",
                json!({"type": "object", "properties": {
                    "projet": {"type": "string"}, "consigne": {"type": "string"}},
                    "required": ["projet", "consigne"]}),
                Risk::Risky,
            ),
            spec(
                TOOL_STATUS,
                "Dit où en sont les grands chantiers.",
                json!({"type": "object", "properties": {}}),
                Risk::Direct,
            ),
            spec(
                TOOL_CANCEL,
                "Annule le dernier grand chantier en cours.",
                json!({"type": "object", "properties": {"projet": {"type": "string"}}}),
                Risk::Direct,
            ),
            spec(
                TOOL_HELP,
                "Explique comment lancer un grand chantier.",
                json!({"type": "object", "properties": {"message": {"type": "string"}}}),
                Risk::Direct,
            ),
        ]
    }

    /// Jamais proposé à l'IA : un chantier coûte des heures de quota.
    fn llm_tools(&self) -> Vec<ToolSpec> {
        Vec::new()
    }

    fn shortcut(&self, text: &str) -> Option<Shortcut> {
        let (tool, args) = match parse(text, &self.inner.projects)? {
            Command::Start {
                project,
                instruction,
            } => (
                TOOL_START,
                json!({"projet": project, "consigne": instruction}),
            ),
            Command::Status => (TOOL_STATUS, json!({})),
            Command::Cancel { project } => (TOOL_CANCEL, json!({"projet": project})),
            Command::Help(message) => (TOOL_HELP, json!({"message": message})),
        };
        Some(Shortcut {
            tool: tool.into(),
            args,
        })
    }

    fn confirmation(&self, tool: &str, args: &Value) -> Option<String> {
        if tool != TOOL_START {
            return None;
        }
        let project = args.get("projet").and_then(Value::as_str).unwrap_or("?");
        let instruction = args.get("consigne").and_then(Value::as_str).unwrap_or("");
        Some(format!(
            "Grand chantier sur {project} : « {} ». Je le confie à Claude sur le serveur ?",
            shorten(instruction, QUOTED_MAX)
        ))
    }

    fn timeout(&self, _tool: &str) -> Option<Duration> {
        // Le suivi interroge le pont une fois par chantier actif.
        Some(BRIDGE_TIMEOUT * 2)
    }

    async fn call(&self, tool: &str, args: Value) -> Result<Reply, SkillError> {
        match tool {
            TOOL_START => self.start(&args).await,
            TOOL_STATUS => Ok(self.status().await),
            TOOL_CANCEL => Ok(self.cancel(&args).await),
            TOOL_HELP => Ok(Reply::new(
                args.get("message").and_then(Value::as_str).unwrap_or(
                    "Dites par exemple : grand chantier sur Jarvis : ajoute un minuteur.",
                ),
            )),
            other => Err(SkillError::UnknownTool(other.into())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CommandOutput, FakeRunner, RecordingAnnouncer};

    fn settings() -> ChantierSettings {
        ChantierSettings {
            enabled: true,
            command: vec!["ssh".into(), "-i".into(), "~/.ssh/cle".into(), "vps".into()],
            poll_minutes: 3,
            projects: vec![
                ChantierProject {
                    name: "CCS".into(),
                    aliases: vec!["c c s".into()],
                },
                ChantierProject {
                    name: "Jarvis".into(),
                    aliases: vec!["l'assistant".into()],
                },
            ],
        }
    }

    fn liste() -> ProjectList {
        ChantierSkill::new(
            &settings(),
            Arc::new(FakeRunner::new()),
            Arc::new(RecordingAnnouncer::new()),
            None,
        )
        .inner
        .projects
        .clone()
    }

    fn start(project: &str, instruction: &str) -> Option<Command> {
        Some(Command::Start {
            project: project.into(),
            instruction: instruction.into(),
        })
    }

    #[test]
    fn depart_avec_consigne() {
        let l = liste();
        assert_eq!(
            parse(
                "Jarvis, lance un grand chantier sur CCS : ajoute la synchronisation des réglages.",
                &l
            ),
            start("CCS", "ajoute la synchronisation des réglages")
        );
        assert_eq!(
            parse(
                "Nouveau chantier pour le projet Jarvis pour ajouter un minuteur, merci",
                &l
            ),
            start("Jarvis", "ajouter un minuteur")
        );
        assert_eq!(
            parse("grand chantier CCS, refais l'écran des tâches", &l),
            start("CCS", "refais l'écran des tâches")
        );
    }

    #[test]
    fn depart_incomplet_explique() {
        let l = liste();
        assert!(matches!(
            parse("lance un grand chantier sur CCS", &l),
            Some(Command::Help(m)) if m.contains("Que dois-je construire")
        ));
        assert!(matches!(
            parse("lance un grand chantier sur la cuisine", &l),
            Some(Command::Help(m)) if m.contains("CCS, Jarvis")
        ));
    }

    #[test]
    fn phrases_sans_chantier_ignorees() {
        let l = liste();
        assert_eq!(parse("lance CCS", &l), None);
        assert_eq!(parse("finis les tests du projet Jarvis", &l), None);
        // « chantier » sans verbe de départ.
        assert_eq!(parse("le chantier de la maison avance bien", &l), None);
        assert_eq!(parse("chantier", &l), None);
    }

    #[test]
    fn suivi_et_annulation() {
        let l = liste();
        assert_eq!(parse("Où en est le chantier ?", &l), Some(Command::Status));
        assert_eq!(
            parse("Jarvis, le chantier est fini ?", &l),
            Some(Command::Status)
        );
        assert_eq!(
            parse("annule le chantier sur CCS", &l),
            Some(Command::Cancel {
                project: Some("CCS".into())
            })
        );
        assert_eq!(
            parse("arrête le chantier", &l),
            Some(Command::Cancel { project: None })
        );
    }

    #[test]
    fn hexadecimal() {
        assert_eq!(to_hex("é a"), "c3a92061");
    }

    #[test]
    fn durees_dites() {
        assert_eq!(spoken_duration(30), "une minute");
        assert_eq!(spoken_duration(600), "10 minutes");
        assert_eq!(spoken_duration(3600), "une heure");
        assert_eq!(spoken_duration(3600 + 600), "une heure 10");
        assert_eq!(spoken_duration(7200), "2 heures");
    }

    struct Banc {
        skill: ChantierSkill,
        runner: Arc<FakeRunner>,
        announcer: Arc<RecordingAnnouncer>,
    }

    fn banc(state: Option<PathBuf>) -> Banc {
        let runner = Arc::new(FakeRunner::new());
        let announcer = Arc::new(RecordingAnnouncer::new());
        let skill = ChantierSkill::with_clock(
            &settings(),
            runner.clone(),
            announcer.clone(),
            state,
            Arc::new(|| 1_000),
        );
        Banc {
            skill,
            runner,
            announcer,
        }
    }

    fn pont_repond(runner: &FakeRunner, line: &str) {
        runner.respond(
            "ssh",
            CommandOutput {
                success: true,
                stdout: format!("{line}\n"),
            },
        );
    }

    #[tokio::test]
    async fn lancement_puis_fin_annoncee() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("chantiers.json");
        let b = banc(Some(state.clone()));
        pont_repond(&b.runner, r#"{"id": "12", "projet": "CCS"}"#);
        let reply = b
            .skill
            .call(
                TOOL_START,
                json!({"projet": "ccs", "consigne": "ajoute un mode sombre"}),
            )
            .await
            .unwrap();
        assert_eq!(reply.outcome, Outcome::Done, "{}", reply.speech);
        let (program, args) = b.runner.calls().pop().unwrap();
        assert_eq!(program, "ssh");
        let home = std::env::var("HOME").unwrap();
        assert_eq!(args[1], format!("{home}/.ssh/cle"));
        assert_eq!(&args[3..5], ["ajouter", "CCS"]);
        assert_eq!(args[5], to_hex("ajoute un mode sombre"));

        // Relu au redémarrage.
        let relu = banc(Some(state.clone()));
        assert_eq!(relu.skill.chantiers().len(), 1);

        pont_repond(&b.runner, r#"{"id": "12", "etat": "running"}"#);
        b.skill.check_now().await;
        assert!(b.announcer.notices().is_empty());
        pont_repond(
            &b.runner,
            r#"{"id": "12", "etat": "done", "resume": "Mode sombre ajouté sur feature/mode-sombre."}"#,
        );
        b.skill.check_now().await;
        let notices = b.announcer.notices();
        assert_eq!(notices.len(), 1);
        assert!(notices[0].speak);
        assert!(notices[0].speech.contains("CCS est fini"));
        assert!(notices[0].body.contains("feature/mode-sombre"));
        // Plus rien d'actif : plus d'appel au pont.
        let calls = b.runner.calls().len();
        b.skill.check_now().await;
        assert_eq!(b.runner.calls().len(), calls);
    }

    #[tokio::test]
    async fn pont_injoignable() {
        let b = banc(None);
        b.runner.respond(
            "ssh",
            CommandOutput {
                success: false,
                stdout: String::new(),
            },
        );
        let reply = b
            .skill
            .call(TOOL_START, json!({"projet": "CCS", "consigne": "x"}))
            .await
            .unwrap();
        assert_eq!(reply.outcome, Outcome::Failed);
        assert_eq!(reply.speech, UNREACHABLE);
        assert!(b.skill.chantiers().is_empty());
    }

    #[tokio::test]
    async fn projet_inconnu_du_serveur() {
        let b = banc(None);
        pont_repond(&b.runner, r#"{"erreur": "projet inconnu : CCS"}"#);
        let reply = b
            .skill
            .call(TOOL_START, json!({"projet": "CCS", "consigne": "x"}))
            .await
            .unwrap();
        assert!(reply.speech.contains("ne trouve pas le projet CCS"));
    }

    #[tokio::test]
    async fn suivi_et_annulation_du_dernier() {
        let b = banc(None);
        assert_eq!(
            b.skill.call(TOOL_STATUS, json!({})).await.unwrap().speech,
            "Aucun chantier en cours."
        );
        pont_repond(&b.runner, r#"{"id": 7}"#);
        b.skill
            .call(TOOL_START, json!({"projet": "Jarvis", "consigne": "x"}))
            .await
            .unwrap();
        pont_repond(&b.runner, r#"{"id": "7", "etat": "running"}"#);
        let s = b.skill.call(TOOL_STATUS, json!({})).await.unwrap().speech;
        assert!(s.contains("Jarvis est en cours"), "{s}");
        pont_repond(&b.runner, r#"{"id": "7", "etat": "cancelled"}"#);
        let r = b
            .skill
            .call(TOOL_CANCEL, json!({"projet": null}))
            .await
            .unwrap();
        assert_eq!(r.outcome, Outcome::Done);
        assert_eq!(b.runner.calls().last().unwrap().1[3..], ["annuler", "7"]);
        assert!(b.announcer.notices().is_empty());
        assert!(!b.skill.chantiers()[0].is_active());
    }

    #[test]
    fn jamais_propose_a_l_ia_et_confirme() {
        let b = banc(None);
        assert!(b.skill.llm_tools().is_empty());
        assert!(b.skill.tools().iter().all(|t| t.name.starts_with("agent_")));
        let q = b
            .skill
            .confirmation(
                TOOL_START,
                &json!({"projet": "CCS", "consigne": "ajoute X"}),
            )
            .unwrap();
        assert!(q.contains("CCS") && q.contains("ajoute X"));
        assert_eq!(
            b.skill
                .tools()
                .iter()
                .find(|t| t.name == TOOL_START)
                .unwrap()
                .risk,
            Risk::Risky
        );
    }
}
