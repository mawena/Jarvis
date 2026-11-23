//! Pilotage du réseau CCS à la voix : état, sessions, sous-agents, arrêts, lancement.
//!
//! Jarvis appelle les méthodes `jarvis.*` du pont CCS du serveur
//! (`docs/protocole-ccs-jarvis.md`) par le même chemin que les grands chantiers :
//! la commande `[ccs] command` (par défaut `[chantier] command`, en pratique `ssh`
//! avec la clé limitée au pont `scripts/jarvis-pont-ccs`), suivie de l'ordre
//! `jarvis <méthode> <paramètres JSON en hexadécimal>`. Le pont répond la réponse
//! CCS telle quelle : `{"result": …}` ou `{"error": {"code", "message"}}`.
//!
//! Les arrêts sont des outils risqués : avant la question, [`Skill::prepare`] appelle
//! CCS **sans** `confirme` (rien n'est fait), lit la `question` rendue et fige la
//! cible exacte (`chat_id`, machine, `tool_id`). Sur le « oui » de Charles, le cerveau
//! exécute l'outil avec ces arguments et `confirme: true` ; toute autre réponse annule.

use crate::chantier::{expand, to_hex};
use crate::text::fold;
use crate::{CommandRunner, Preparation, Reply, Risk, Shortcut, Skill, SkillError, ToolSpec};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};

pub const TOOL_STATE: &str = "ccs_etat";
pub const TOOL_SESSIONS: &str = "ccs_sessions";
pub const TOOL_SUBAGENTS: &str = "ccs_sous_agents";
pub const TOOL_STOP_SUBAGENT: &str = "ccs_arreter_sous_agent";
pub const TOOL_STOP_SESSION: &str = "ccs_arreter_session";
pub const TOOL_START: &str = "ccs_lancer_session";

/// Délai d'un appel (SSH, puis jusqu'à 20 s par machine, toutes en même temps).
const CCS_TIMEOUT: Duration = Duration::from_secs(40);

/// Sessions citées au plus dans une liste dite.
const LISTED_MAX: usize = 6;

/// Longueur d'un premier message cité dans une question.
const QUOTED_MAX: usize = 100;

const UNREACHABLE: &str = "Je n'arrive pas à joindre CCS.";
const NEEDS_YES: &str = "Rien n'est fait sans votre confirmation.";
const WHICH_PROJECT: &str = "Sur quel projet ?";

/// Réglages de la section `[ccs]`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct CcsSettings {
    /// Faux tant que le pont n'a pas l'ordre `jarvis` (et CCS les méthodes `jarvis.*`).
    pub enabled: bool,
    /// Commande qui joint le pont ; vide : celle de `[chantier]`.
    pub command: Vec<String>,
}

/// Échec d'un appel.
#[derive(Debug, Clone, PartialEq, Eq)]
enum CallError {
    /// Erreur rendue par CCS : `code` et phrase lisible telle quelle.
    Ccs { code: String, message: String },
    /// Pont injoignable, réponse illisible… (détail journalisé, pas dit).
    Bridge(String),
}

/// Compétence de pilotage du réseau CCS.
pub struct CcsSkill {
    program: String,
    base_args: Vec<String>,
    runner: Arc<dyn CommandRunner>,
}

impl CcsSkill {
    /// `command` : programme et arguments qui joignent le pont (`~/` développé).
    pub fn new(command: &[String], runner: Arc<dyn CommandRunner>) -> Self {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let mut command = command.iter().map(|a| expand(a, home.as_deref()));
        Self {
            program: command.next().unwrap_or_default(),
            base_args: command.collect(),
            runner,
        }
    }

    /// Appelle `method` avec `params` ; renvoie le `result` de CCS.
    async fn call_ccs(&self, method: &str, params: Value) -> Result<Value, CallError> {
        if self.program.is_empty() {
            return Err(CallError::Bridge("commande du pont non réglée".into()));
        }
        let mut args = self.base_args.clone();
        args.extend([
            "jarvis".to_string(),
            method.to_string(),
            to_hex(&params.to_string()),
        ]);
        let (runner, program) = (self.runner.clone(), self.program.clone());
        let job = tokio::task::spawn_blocking(move || runner.run(&program, &args));
        let out = tokio::time::timeout(CCS_TIMEOUT, job)
            .await
            .map_err(|_| CallError::Bridge("délai dépassé".into()))?
            .map_err(|e| CallError::Bridge(e.to_string()))?
            .map_err(|e| CallError::Bridge(format!("lancement de {} : {e}", self.program)))?;
        let line = out
            .stdout
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("");
        let v: Value = serde_json::from_str(line).map_err(|_| {
            CallError::Bridge(format!("réponse illisible (succès : {})", out.success))
        })?;
        if let Some(e) = v.get("erreur").and_then(Value::as_str) {
            return Err(CallError::Bridge(e.to_string()));
        }
        if let Some(e) = v.get("error") {
            return Err(CallError::Ccs {
                code: str_of(e, "code").to_string(),
                message: str_of(e, "message").to_string(),
            });
        }
        v.get("result")
            .cloned()
            .ok_or_else(|| CallError::Bridge("réponse sans résultat".into()))
    }

    async fn state(&self) -> Result<Reply, SkillError> {
        let r = match self.call_ccs("jarvis.state", json!({})).await {
            Ok(r) => r,
            Err(e) => return error_reply(e),
        };
        let mut speech = str_of(&r, "summary").trim().to_string();
        if speech.is_empty() {
            speech = "CCS n'a rien résumé.".into();
        }
        let silent: Vec<&str> = list(&r, "errors")
            .iter()
            .map(|e| str_of(e, "machine"))
            .filter(|m| !m.is_empty())
            .collect();
        if !silent.is_empty() {
            speech.push_str(&format!(" Sans réponse : {}.", silent.join(", ")));
        }
        Ok(Reply::new(speech))
    }

    async fn sessions(&self, args: &Value) -> Result<Reply, SkillError> {
        let mut params = Map::new();
        copy(args, "machine", &mut params, "machine");
        let r = match self
            .call_ccs("jarvis.sessions", Value::Object(params))
            .await
        {
            Ok(r) => r,
            Err(e) => return error_reply(e),
        };
        let live: Vec<&Value> = list(&r, "sessions")
            .iter()
            .filter(|s| str_of(s, "state") != "stopped")
            .collect();
        if live.is_empty() {
            return Ok(Reply::new("Aucune session ne tourne."));
        }
        let described: Vec<String> = live.iter().take(LISTED_MAX).map(|s| describe(s)).collect();
        let mut speech = format!(
            "{} : {}",
            plural(live.len(), "session", "sessions"),
            described.join(" ; ")
        );
        if live.len() > LISTED_MAX {
            speech.push_str(&format!(", et {} autres", live.len() - LISTED_MAX));
        }
        speech.push('.');
        Ok(Reply::new(speech))
    }

    async fn subagents(&self, args: &Value) -> Result<Reply, SkillError> {
        let params = target_params(args)?;
        let r = match self
            .call_ccs("jarvis.subagents", Value::Object(params))
            .await
        {
            Ok(r) => r,
            Err(e) => return error_reply(e),
        };
        let name = session_name(r.get("session").unwrap_or(&Value::Null));
        let all = list(&r, "subagents");
        let running: Vec<&Value> = all
            .iter()
            .filter(|a| str_of(a, "state") == "running")
            .collect();
        let finished = all.len() - running.len();
        let mut speech = if running.is_empty() {
            format!("Aucun sous-agent ne tourne dans la session {name}.")
        } else {
            let items: Vec<String> = running
                .iter()
                .enumerate()
                .map(|(i, a)| {
                    let rank = a
                        .get("active_index")
                        .and_then(Value::as_i64)
                        .unwrap_or(i as i64 + 1);
                    format!("{rank}, « {} »", str_of(a, "description").trim())
                })
                .collect();
            format!(
                "Session {name} : {} en cours : {}.",
                plural(running.len(), "sous-agent", "sous-agents"),
                items.join(" ; ")
            )
        };
        if finished > 0 {
            speech.push_str(&format!(
                " {} {}.",
                plural(finished, "autre", "autres"),
                if finished > 1 { "finis" } else { "fini" }
            ));
        }
        Ok(Reply::new(speech))
    }

    /// Arrêt confirmé : `args` vient de [`Self::prepare_stop`] (`cible` exacte).
    async fn stop(&self, method: &str, args: &Value) -> Result<Reply, SkillError> {
        let confirmed = args.get("confirme").and_then(Value::as_bool) == Some(true);
        let Some(Value::Object(target)) = args.get("cible").filter(|_| confirmed) else {
            info!(methode = %method, "arrêt sans confirmation : rien n'est fait");
            return Ok(Reply::failed(NEEDS_YES));
        };
        let mut params = target.clone();
        params.insert("confirme".into(), Value::Bool(true));
        match self.call_ccs(method, Value::Object(params)).await {
            Ok(r) if r.get("done").and_then(Value::as_bool) == Some(true) => {
                let message = str_of(&r, "message").trim();
                Ok(Reply::done(if message.is_empty() {
                    "Arrêt demandé."
                } else {
                    message
                }))
            }
            Ok(_) => Err(SkillError::Failed("arrêt non fait malgré confirme".into())),
            Err(e) => error_reply(e),
        }
    }

    /// Simulation sans effet : la question de CCS, et la cible exacte pour le « oui ».
    async fn prepare_stop(
        &self,
        tool: &str,
        method: &str,
        args: &Value,
    ) -> Result<Preparation, SkillError> {
        let mut params = target_params(args)?;
        if tool == TOOL_STOP_SUBAGENT {
            copy(args, "rang", &mut params, "index");
            copy(args, "tool_id", &mut params, "tool_id");
        }
        let r = match self.call_ccs(method, Value::Object(params.clone())).await {
            Ok(r) => r,
            Err(e) => return error_reply(e).map(Preparation::Answer),
        };
        if r.get("done").and_then(Value::as_bool) == Some(true) {
            return Ok(Preparation::Answer(Reply::done(str_of(&r, "message"))));
        }
        let question = str_of(&r, "question").trim();
        if r.get("confirm_needed").and_then(Value::as_bool) != Some(true) || question.is_empty() {
            return Err(SkillError::Failed("simulation sans question".into()));
        }
        let target = r.get("target").unwrap_or(&Value::Null);
        // Cible exacte : l'ordre des sessions ou des sous-agents peut changer avant le « oui ».
        let mut exact = Map::new();
        let chat_id = str_of(target, "chat_id");
        exact.insert(
            "session".into(),
            if chat_id.is_empty() {
                params.get("session").cloned().unwrap_or(Value::Null)
            } else {
                Value::String(chat_id.into())
            },
        );
        let machine = str_of(target, "machine");
        if !machine.is_empty() {
            exact.insert("machine".into(), Value::String(machine.into()));
        } else if let Some(m) = params.get("machine") {
            exact.insert("machine".into(), m.clone());
        }
        if tool == TOOL_STOP_SUBAGENT {
            let tool_id = target
                .get("subagent")
                .map(|s| str_of(s, "tool_id"))
                .unwrap_or("");
            if tool_id.is_empty() {
                return Err(SkillError::Failed("sous-agent sans tool_id".into()));
            }
            exact.insert("tool_id".into(), Value::String(tool_id.into()));
        }
        Ok(Preparation::Ask {
            question: Some(question.to_string()),
            args: json!({"cible": exact, "confirme": true}),
        })
    }

    async fn start(&self, args: &Value) -> Result<Reply, SkillError> {
        let project = str_of(args, "projet").trim();
        if project.is_empty() {
            return Ok(Reply::new(WHICH_PROJECT));
        }
        let mut params = Map::new();
        params.insert("project".into(), Value::String(project.into()));
        copy(args, "machine", &mut params, "machine");
        copy(args, "message", &mut params, "message");
        match self
            .call_ccs("jarvis.start_session", Value::Object(params))
            .await
        {
            Ok(r) => {
                let name = non_empty(str_of(&r, "project"), project);
                let machine = non_empty(str_of(&r, "machine_name"), str_of(&r, "machine"));
                Ok(Reply::done(if machine.is_empty() {
                    format!("Session lancée sur {name}.")
                } else {
                    format!("Session lancée sur {name}, sur {machine}.")
                }))
            }
            Err(e) => error_reply(e),
        }
    }
}

/// Réponse dite pour un échec : la phrase de CCS quand elle est lisible.
fn error_reply(e: CallError) -> Result<Reply, SkillError> {
    match e {
        CallError::Ccs { code, message } if !message.trim().is_empty() => match code.as_str() {
            // Une question (« Lequel ? », « Sur quelle machine ? ») ou une redemande.
            "conflit" | "invalide" => Ok(Reply::new(message)),
            "introuvable" | "refuse" => Ok(Reply::failed(message)),
            _ => Err(SkillError::Failed(format!("{code} : {message}"))),
        },
        CallError::Ccs { code, .. } => Err(SkillError::Failed(format!("erreur CCS {code}"))),
        CallError::Bridge(detail) => {
            warn!(detail = %detail, "pont CCS injoignable");
            Ok(Reply::failed(UNREACHABLE))
        }
    }
}

/// Paramètres `session` (exigé) et `machine` pour CCS.
fn target_params(args: &Value) -> Result<Map<String, Value>, SkillError> {
    let mut params = Map::new();
    copy(args, "session", &mut params, "session");
    if !params.contains_key("session") {
        return Err(SkillError::BadArgs("« session » manquant".into()));
    }
    copy(args, "machine", &mut params, "machine");
    Ok(params)
}

/// Copie `args[from]` (texte non vide ou nombre) dans `params[to]`.
fn copy(args: &Value, from: &str, params: &mut Map<String, Value>, to: &str) {
    match args.get(from) {
        Some(Value::String(s)) if !s.trim().is_empty() => {
            params.insert(to.into(), Value::String(s.trim().into()));
        }
        Some(n @ Value::Number(_)) => {
            params.insert(to.into(), n.clone());
        }
        _ => {}
    }
}

fn str_of<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

fn list<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v.get(key)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

fn non_empty<'a>(a: &'a str, b: &'a str) -> &'a str {
    if a.trim().is_empty() { b } else { a }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n > 1 { many } else { one })
}

fn session_name(s: &Value) -> &str {
    non_empty(str_of(s, "name"), non_empty(str_of(s, "project"), "?"))
}

/// « GliGli sur lp-work, au travail, 2 sous-agents en cours ».
fn describe(s: &Value) -> String {
    let mut out = session_name(s).to_string();
    let machine = non_empty(str_of(s, "machine_name"), str_of(s, "machine"));
    if !machine.is_empty() {
        out.push_str(&format!(" sur {machine}"));
    }
    let state = str_of(s, "state_label");
    if !state.is_empty() {
        out.push_str(&format!(", {state}"));
    }
    let running = s
        .get("subagents_running")
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;
    if running > 0 {
        out.push_str(&format!(
            ", {} en cours",
            plural(running, "sous-agent", "sous-agents")
        ));
    }
    out
}

fn shorten(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        let cut: String = text.chars().take(max).collect();
        format!("{}…", cut.trim_end())
    }
}

// --- Raccourcis (phrases reconnues sans IA) ---

const LEADING: &[&str] = &[
    "jarvis", "stp", "svp", "bon", "alors", "allez", "dis", "moi",
];
const STOP_VERBS: &[&str] = &[
    "tue",
    "arrete",
    "stoppe",
    "termine",
    "coupe",
    "kill",
    "ferme",
    "interromps",
];
const START_VERBS: &[&str] = &["lance", "ouvre", "demarre", "cree", "commence"];
const ASK_WORDS: &[&str] = &[
    "quelles",
    "quelle",
    "quels",
    "quel",
    "liste",
    "combien",
    "tournent",
    "tourne",
    "ouvertes",
    "cours",
    "montre",
    "donne",
    "lesquelles",
    "actives",
];
const ARTICLES: &[&str] = &[
    "le", "la", "les", "l", "de", "du", "des", "d", "dans", "sur", "pour",
];

/// Rang dit : « premier », « deuxième », « 2 », « dernier »…
fn rank(word: &str, after: bool) -> Option<i64> {
    let n = match word {
        "premier" | "premiere" | "1er" => 1,
        "deuxieme" | "second" | "seconde" | "2e" => 2,
        "troisieme" | "3e" => 3,
        "quatrieme" | "4e" => 4,
        "cinquieme" | "5e" => 5,
        "dernier" | "derniere" => -1,
        // Après « sous-agent » seulement : « un » est sinon un article.
        "un" if after => 1,
        "deux" if after => 2,
        "trois" if after => 3,
        "quatre" if after => 4,
        "cinq" if after => 5,
        w => return w.parse().ok().filter(|n: &i64| *n > 0),
    };
    Some(n)
}

/// Mots qui suivent `from`, articles retirés en tête ; vide : `None`.
fn name_after(words: &[&str], from: usize) -> Option<String> {
    let rest: Vec<&str> = words[from..]
        .iter()
        .copied()
        .skip_while(|w| ARTICLES.contains(w) || matches!(*w, "session" | "projet" | "ccs"))
        .collect();
    (!rest.is_empty()).then(|| rest.join(" "))
}

/// Phrase reconnue : outil et arguments.
pub fn parse(text: &str) -> Option<(&'static str, Value)> {
    let folded = fold(text);
    let words: Vec<&str> = folded
        .split(' ')
        .skip_while(|w| LEADING.contains(w))
        .filter(|w| !w.is_empty())
        .collect();
    let first = *words.first()?;
    let has = |w: &str| words.contains(&w);
    let sub_at = words
        .windows(2)
        .position(|p| p[0] == "sous" && matches!(p[1], "agent" | "agents"));
    let session_at = words.iter().rposition(|w| *w == "session");

    // « Tue le premier sous-agent de la session GliGli. »
    if STOP_VERBS.contains(&first)
        && let Some(i) = sub_at
    {
        let before = i.checked_sub(1).and_then(|j| rank(words[j], false));
        let after_at = if words.get(i + 2) == Some(&"numero") {
            i + 3
        } else {
            i + 2
        };
        let after = words.get(after_at).and_then(|w| rank(w, true));
        let session = match session_at {
            Some(s) if s > i => name_after(&words, s + 1),
            _ => name_after(&words, after_at + usize::from(after.is_some())),
        }?;
        let mut args = json!({ "session": session });
        if let Some(r) = before.or(after) {
            args["rang"] = json!(r);
        }
        return Some((TOOL_STOP_SUBAGENT, args));
    }
    // « Quels sous-agents tournent dans la session GliGli ? »
    if let Some(i) = sub_at
        && words[..i]
            .iter()
            .chain(&words[i + 2..])
            .any(|w| ASK_WORDS.contains(w))
    {
        let s = session_at.filter(|s| *s > i)?;
        return Some((
            TOOL_SUBAGENTS,
            json!({ "session": name_after(&words, s + 1)? }),
        ));
    }
    if sub_at.is_some() {
        return None;
    }
    // « Arrête la session GliGli. »
    if STOP_VERBS.contains(&first)
        && let Some(s) = session_at
    {
        return Some((
            TOOL_STOP_SESSION,
            json!({ "session": name_after(&words, s + 1)? }),
        ));
    }
    // « Lance une session sur le projet Jarvis (sur la machine lp-work). »
    if START_VERBS.contains(&first)
        && let Some(s) = session_at
        && words[1..s].iter().any(|w| matches!(*w, "une" | "nouvelle"))
    {
        let mut rest = &words[s + 1..];
        while let Some((w, tail)) = rest.split_first()
            && matches!(*w, "claude" | "ccs")
        {
            rest = tail;
        }
        let (w, rest) = rest.split_first()?;
        if !matches!(*w, "sur" | "dans" | "pour") {
            return None;
        }
        let machine_at = rest.windows(3).position(|p| {
            p[0] == "sur" && matches!((p[1], p[2]), ("la", "machine") | ("machine", _))
        });
        let (project, machine) = match machine_at {
            Some(m) => {
                let skip = if rest[m + 1] == "la" { 3 } else { 2 };
                (&rest[..m], name_after(rest, m + skip))
            }
            None => (rest, None),
        };
        let project = name_after(project, 0)?;
        let mut args = json!({ "projet": project });
        if let Some(m) = machine {
            args["machine"] = json!(m);
        }
        return Some((TOOL_START, args));
    }
    // « Quelles sessions tournent ? »
    if has("sessions") && words.iter().any(|w| ASK_WORDS.contains(w)) {
        return Some((TOOL_SESSIONS, json!({})));
    }
    // « État du réseau », « état de CCS ».
    if has("etat") && (has("reseau") || has("ccs")) && session_at.is_none() {
        return Some((TOOL_STATE, json!({})));
    }
    None
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
impl Skill for CcsSkill {
    fn tools(&self) -> Vec<ToolSpec> {
        let target = json!({"type": "object", "properties": {
            "session": {"type": "string", "description": "nom approché de la session, ou rang (1, \"dernier\")"},
            "machine": {"type": "string", "description": "machine, si le nom est ambigu"}},
            "required": ["session"]});
        vec![
            spec(
                TOOL_STATE,
                "État du réseau CCS : machines en ligne, sessions Claude, sous-agents, travaux distants.",
                json!({"type": "object", "properties": {}}),
                Risk::Direct,
            ),
            spec(
                TOOL_SESSIONS,
                "Liste les sessions Claude qui tournent sur le réseau CCS.",
                json!({"type": "object", "properties": {
                    "machine": {"type": "string", "description": "une seule machine"}}}),
                Risk::Direct,
            ),
            spec(
                TOOL_SUBAGENTS,
                "Liste les sous-agents d'une session Claude de CCS.",
                target.clone(),
                Risk::Direct,
            ),
            spec(
                TOOL_STOP_SUBAGENT,
                "Arrête un sous-agent d'une session Claude de CCS (le reste de la session continue).",
                json!({"type": "object", "properties": {
                    "session": {"type": "string", "description": "nom approché de la session"},
                    "machine": {"type": "string"},
                    "rang": {"type": "integer", "description": "rang parmi les sous-agents en cours : 1 le premier, -1 le dernier"}},
                    "required": ["session"]}),
                Risk::Risky,
            ),
            spec(
                TOOL_STOP_SESSION,
                "Arrête une session Claude de CCS entière.",
                target,
                Risk::Risky,
            ),
            spec(
                TOOL_START,
                "Lance une nouvelle session Claude de CCS sur un projet.",
                json!({"type": "object", "properties": {
                    "projet": {"type": "string", "description": "nom approché du projet"},
                    "machine": {"type": "string"},
                    "message": {"type": "string", "description": "premier message à Claude"}},
                    "required": ["projet"]}),
                Risk::Risky,
            ),
        ]
    }

    fn shortcut(&self, text: &str) -> Option<Shortcut> {
        let (tool, args) = parse(text)?;
        Some(Shortcut {
            tool: tool.into(),
            args,
        })
    }

    fn confirmation(&self, tool: &str, args: &Value) -> Option<String> {
        if tool != TOOL_START {
            return None;
        }
        let mut q = format!(
            "Je lance une session Claude sur le projet {}",
            str_of(args, "projet").trim()
        );
        let machine = str_of(args, "machine").trim();
        if !machine.is_empty() {
            q.push_str(&format!(" sur {machine}"));
        }
        let message = str_of(args, "message").trim();
        if !message.is_empty() {
            q.push_str(&format!(", avec « {} »", shorten(message, QUOTED_MAX)));
        }
        q.push_str(" ?");
        Some(q)
    }

    async fn prepare(&self, tool: &str, args: Value) -> Result<Preparation, SkillError> {
        match tool {
            TOOL_STOP_SUBAGENT => self.prepare_stop(tool, "jarvis.stop_subagent", &args).await,
            TOOL_STOP_SESSION => self.prepare_stop(tool, "jarvis.stop_session", &args).await,
            TOOL_START if str_of(&args, "projet").trim().is_empty() => {
                Ok(Preparation::Answer(Reply::new(WHICH_PROJECT)))
            }
            _ => Ok(Preparation::Ask {
                question: self.confirmation(tool, &args),
                args,
            }),
        }
    }

    fn timeout(&self, _tool: &str) -> Option<Duration> {
        Some(CCS_TIMEOUT + Duration::from_secs(5))
    }

    async fn call(&self, tool: &str, args: Value) -> Result<Reply, SkillError> {
        match tool {
            TOOL_STATE => self.state().await,
            TOOL_SESSIONS => self.sessions(&args).await,
            TOOL_SUBAGENTS => self.subagents(&args).await,
            TOOL_STOP_SUBAGENT => self.stop("jarvis.stop_subagent", &args).await,
            TOOL_STOP_SESSION => self.stop("jarvis.stop_session", &args).await,
            TOOL_START => self.start(&args).await,
            other => Err(SkillError::UnknownTool(other.into())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CommandOutput, Outcome};
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// Faux CCS derrière le pont : décode l'ordre `jarvis <méthode> <hex>`, note
    /// l'appel, et répond la ligne programmée pour la méthode (ou pour la méthode
    /// confirmée, clé `<méthode>+confirme`).
    #[derive(Default)]
    struct FakeCcs {
        calls: Mutex<Vec<(String, Value)>>,
        replies: Mutex<HashMap<String, String>>,
    }

    impl FakeCcs {
        fn reply(&self, key: &str, line: Value) {
            self.replies
                .lock()
                .unwrap()
                .insert(key.into(), line.to_string());
        }
        fn calls(&self) -> Vec<(String, Value)> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl CommandRunner for FakeCcs {
        fn run(&self, program: &str, args: &[String]) -> std::io::Result<CommandOutput> {
            assert_eq!(program, "ssh");
            assert_eq!(&args[..2], ["vps", "jarvis"]);
            let method = args[2].clone();
            let bytes: Vec<u8> = (0..args[3].len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&args[3][i..i + 2], 16).unwrap())
                .collect();
            let params: Value = serde_json::from_slice(&bytes).unwrap();
            let key = if params.get("confirme") == Some(&json!(true)) {
                format!("{method}+confirme")
            } else {
                method.clone()
            };
            self.calls.lock().unwrap().push((method, params));
            let stdout = self.replies.lock().unwrap().get(&key).cloned();
            Ok(CommandOutput {
                success: stdout.is_some(),
                stdout: stdout.map(|s| s + "\n").unwrap_or_default(),
            })
        }
        fn spawn_detached(&self, _: &str, _: &[String]) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn banc() -> (CcsSkill, Arc<FakeCcs>) {
        let fake = Arc::new(FakeCcs::default());
        let skill = CcsSkill::new(&["ssh".into(), "vps".into()], fake.clone());
        (skill, fake)
    }

    fn session(name: &str, machine: &str, state: &str, label: &str, running: u64) -> Value {
        json!({"name": name, "machine": machine, "machine_name": machine, "state": state,
               "state_label": label, "subagents_running": running, "chat_id": format!("c-{name}")})
    }

    #[test]
    fn raccourcis_reconnus() {
        assert_eq!(
            parse("Jarvis, état du réseau"),
            Some((TOOL_STATE, json!({})))
        );
        assert_eq!(parse("état de CCS ?"), Some((TOOL_STATE, json!({}))));
        assert_eq!(
            parse("Quelles sessions tournent ?"),
            Some((TOOL_SESSIONS, json!({})))
        );
        assert_eq!(
            parse("liste les sessions"),
            Some((TOOL_SESSIONS, json!({})))
        );
        assert_eq!(
            parse("Tue le premier sous-agent de la session GliGli."),
            Some((TOOL_STOP_SUBAGENT, json!({"session": "gligli", "rang": 1})))
        );
        assert_eq!(
            parse("arrête le dernier sous-agent de GliGli"),
            Some((TOOL_STOP_SUBAGENT, json!({"session": "gligli", "rang": -1})))
        );
        assert_eq!(
            parse("stoppe le sous-agent 2 de la session Jarvis"),
            Some((TOOL_STOP_SUBAGENT, json!({"session": "jarvis", "rang": 2})))
        );
        assert_eq!(
            parse("arrête le sous-agent de la session GliGli"),
            Some((TOOL_STOP_SUBAGENT, json!({"session": "gligli"})))
        );
        assert_eq!(
            parse("quels sous-agents tournent dans la session GliGli ?"),
            Some((TOOL_SUBAGENTS, json!({"session": "gligli"})))
        );
        assert_eq!(
            parse("Ferme la session GliGli"),
            Some((TOOL_STOP_SESSION, json!({"session": "gligli"})))
        );
        assert_eq!(
            parse("Lance une session sur le projet Jarvis"),
            Some((TOOL_START, json!({"projet": "jarvis"})))
        );
        assert_eq!(
            parse("ouvre une nouvelle session Claude dans GliGli sur la machine lp work"),
            Some((
                TOOL_START,
                json!({"projet": "gligli", "machine": "lp work"})
            ))
        );
    }

    #[test]
    fn phrases_etrangeres_ignorees() {
        for p in [
            "arrête la musique",
            "arrête",
            "lance un grand chantier sur CCS : ajoute X",
            "lance Firefox",
            "ouvre la session",
            "arrête la session",
            "tue le sous-agent",
            "quel temps fait-il ?",
            "lance une session",
        ] {
            assert_eq!(parse(p), None, "{p}");
        }
    }

    #[tokio::test]
    async fn etat_lit_le_resume_et_les_machines_muettes() {
        let (skill, fake) = banc();
        fake.reply(
            "jarvis.state",
            json!({"result": {"summary": "2 machines en ligne sur 3.", "errors": [{"machine": "pc"}]}}),
        );
        let r = skill.call(TOOL_STATE, json!({})).await.unwrap();
        assert_eq!(r.speech, "2 machines en ligne sur 3. Sans réponse : pc.");
        assert_eq!(fake.calls(), vec![("jarvis.state".into(), json!({}))]);
    }

    #[tokio::test]
    async fn sessions_listees_sans_les_arretees() {
        let (skill, fake) = banc();
        fake.reply(
            "jarvis.sessions",
            json!({"result": {"sessions": [
                session("GliGli", "lp-work", "working", "au travail", 2),
                session("Vieille", "vps", "stopped", "arrêtée", 0),
                session("Jarvis", "vps", "waiting", "en attente de réponse", 0)]}}),
        );
        let r = skill.call(TOOL_SESSIONS, json!({})).await.unwrap();
        assert_eq!(
            r.speech,
            "2 sessions : GliGli sur lp-work, au travail, 2 sous-agents en cours ; \
             Jarvis sur vps, en attente de réponse."
        );
        fake.reply("jarvis.sessions", json!({"result": {"sessions": []}}));
        let r = skill.call(TOOL_SESSIONS, json!({})).await.unwrap();
        assert_eq!(r.speech, "Aucune session ne tourne.");
    }

    #[tokio::test]
    async fn sous_agents_d_une_session() {
        let (skill, fake) = banc();
        fake.reply(
            "jarvis.subagents",
            json!({"result": {"session": {"name": "GliGli"}, "subagents": [
                {"description": "Lire le code", "state": "done", "active_index": null},
                {"description": "Tests", "state": "running", "active_index": 1}]}}),
        );
        let r = skill
            .call(TOOL_SUBAGENTS, json!({"session": "gligli"}))
            .await
            .unwrap();
        assert_eq!(
            r.speech,
            "Session GliGli : 1 sous-agent en cours : 1, « Tests ». 1 autre fini."
        );
        assert_eq!(fake.calls()[0].1, json!({"session": "gligli"}));
    }

    fn stop_subagent_question() -> Value {
        json!({"result": {"done": false, "confirm_needed": true, "action": "stop_subagent",
            "target": {"machine": "pc", "machine_name": "lp-work", "session": "GliGli",
                       "chat_id": "c1-4", "subagent": {"active_index": 1, "tool_id": "toolu_9"}},
            "question": "Arrêter le sous-agent 1 « Lire le code » de la session GliGli sur lp-work ?"}})
    }

    #[tokio::test]
    async fn arret_de_sous_agent_simule_puis_confirme_sur_la_cible_exacte() {
        let (skill, fake) = banc();
        fake.reply("jarvis.stop_subagent", stop_subagent_question());
        fake.reply(
            "jarvis.stop_subagent+confirme",
            json!({"result": {"done": true, "message": "Arrêt demandé pour le sous-agent « Lire le code »."}}),
        );
        // L'IA ne peut pas sauter la simulation en inventant « confirme ».
        let prep = skill
            .prepare(
                TOOL_STOP_SUBAGENT,
                json!({"session": "GliGli", "rang": 1, "confirme": true}),
            )
            .await
            .unwrap();
        let Preparation::Ask { question, args } = prep else {
            panic!("question attendue : {prep:?}")
        };
        assert_eq!(
            question.as_deref(),
            Some("Arrêter le sous-agent 1 « Lire le code » de la session GliGli sur lp-work ?")
        );
        assert_eq!(
            fake.calls(),
            vec![(
                "jarvis.stop_subagent".into(),
                json!({"session": "GliGli", "index": 1})
            )]
        );
        let r = skill.call(TOOL_STOP_SUBAGENT, args).await.unwrap();
        assert_eq!(r.outcome, Outcome::Done);
        assert_eq!(
            r.speech,
            "Arrêt demandé pour le sous-agent « Lire le code »."
        );
        assert_eq!(
            fake.calls()[1],
            (
                "jarvis.stop_subagent".into(),
                json!({"session": "c1-4", "machine": "pc", "tool_id": "toolu_9", "confirme": true})
            )
        );
    }

    #[tokio::test]
    async fn arret_de_session_confirme_par_chat_id() {
        let (skill, fake) = banc();
        fake.reply(
            "jarvis.stop_session",
            json!({"result": {"done": false, "confirm_needed": true,
                "target": {"machine": "vps", "chat_id": "c9", "kind": "chat"},
                "question": "Arrêter la session GliGli sur Serveur CCS ?"}}),
        );
        fake.reply(
            "jarvis.stop_session+confirme",
            json!({"result": {"done": true, "message": "Session GliGli arrêtée."}}),
        );
        let Preparation::Ask { args, .. } = skill
            .prepare(TOOL_STOP_SESSION, json!({"session": "gligli"}))
            .await
            .unwrap()
        else {
            panic!()
        };
        let r = skill.call(TOOL_STOP_SESSION, args).await.unwrap();
        assert_eq!(r.speech, "Session GliGli arrêtée.");
        assert_eq!(
            fake.calls()[1].1,
            json!({"session": "c9", "machine": "vps", "confirme": true})
        );
    }

    #[tokio::test]
    async fn sans_confirmation_rien_n_est_envoye() {
        let (skill, fake) = banc();
        let r = skill
            .call(
                TOOL_STOP_SESSION,
                json!({"session": "gligli", "confirme": true}),
            )
            .await
            .unwrap();
        assert_eq!(r.speech, NEEDS_YES);
        assert!(fake.calls().is_empty());
    }

    #[tokio::test]
    async fn erreurs_de_ccs_dites_sans_question() {
        let (skill, fake) = banc();
        fake.reply(
            "jarvis.stop_subagent",
            json!({"error": {"code": "conflit", "message": "2 sous-agents tournent. Lequel ?"}}),
        );
        let prep = skill
            .prepare(TOOL_STOP_SUBAGENT, json!({"session": "gligli"}))
            .await
            .unwrap();
        assert_eq!(
            prep,
            Preparation::Answer(Reply::new("2 sous-agents tournent. Lequel ?"))
        );
        fake.reply(
            "jarvis.stop_session",
            json!({"error": {"code": "introuvable", "message": "Aucune session « Zut »."}}),
        );
        let prep = skill
            .prepare(TOOL_STOP_SESSION, json!({"session": "zut"}))
            .await
            .unwrap();
        assert_eq!(
            prep,
            Preparation::Answer(Reply::failed("Aucune session « Zut »."))
        );
    }

    #[tokio::test]
    async fn pont_injoignable() {
        let (skill, fake) = banc();
        // Rien de programmé : sortie vide, échec.
        let r = skill.call(TOOL_STATE, json!({})).await.unwrap();
        assert_eq!(r, Reply::failed(UNREACHABLE));
        fake.reply(
            "jarvis.state",
            json!({"erreur": "pont CCS injoignable : FileNotFoundError"}),
        );
        let r = skill.call(TOOL_STATE, json!({})).await.unwrap();
        assert_eq!(r, Reply::failed(UNREACHABLE));
    }

    #[tokio::test]
    async fn lancement_question_locale_puis_appel() {
        let (skill, fake) = banc();
        let args = json!({"projet": "Jarvis", "machine": "lp-work", "message": "lance les tests"});
        let Preparation::Ask { question, args } = skill.prepare(TOOL_START, args).await.unwrap()
        else {
            panic!()
        };
        assert_eq!(
            question.as_deref(),
            Some(
                "Je lance une session Claude sur le projet Jarvis sur lp-work, avec « lance les tests » ?"
            )
        );
        assert!(fake.calls().is_empty(), "aucun appel avant le « oui »");
        fake.reply(
            "jarvis.start_session",
            json!({"result": {"done": true, "project": "Jarvis", "machine_name": "lp-work"}}),
        );
        let r = skill.call(TOOL_START, args).await.unwrap();
        assert_eq!(r, Reply::done("Session lancée sur Jarvis, sur lp-work."));
        assert_eq!(
            fake.calls()[0].1,
            json!({"project": "Jarvis", "machine": "lp-work", "message": "lance les tests"})
        );
        let prep = skill.prepare(TOOL_START, json!({})).await.unwrap();
        assert_eq!(prep, Preparation::Answer(Reply::new(WHICH_PROJECT)));
    }

    #[test]
    fn outils_proposes_a_l_ia_et_arrets_risques() {
        let (skill, _) = banc();
        let names: Vec<_> = skill
            .llm_tools()
            .into_iter()
            .map(|t| (t.name, t.risk))
            .collect();
        assert_eq!(names.len(), 6);
        for (name, risk) in names {
            let destructive = name.starts_with("ccs_arreter") || name == TOOL_START;
            assert_eq!(risk == Risk::Risky, destructive, "{name}");
        }
    }
}
