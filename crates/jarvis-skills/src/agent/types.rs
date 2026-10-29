//! Types partagés de l'agent de code.

use crate::text::fold;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::PathBuf;

/// Identifiant d'une tâche (aussi l'id de session de Claude Code).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TaskId(pub String);

impl TaskId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TaskId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// État d'une tâche.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Running,
    Succeeded,
    Failed,
    /// Arrêtée à la demande de Charles.
    Stopped,
    /// Durée maximale dépassée.
    TimedOut,
    /// Plus aucune sortie depuis trop longtemps : arrêtée.
    Stalled,
    /// Le cerveau a redémarré pendant la tâche.
    Interrupted,
}

/// Enregistrement d'une tâche, gardé sur disque.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskRecord {
    pub id: TaskId,
    /// Nom du projet.
    pub project: String,
    /// Consigne de Charles.
    pub instruction: String,
    pub branch: String,
    pub worktree: PathBuf,
    /// Branche de départ (develop).
    pub base: String,
    /// SHA de la base au départ.
    pub base_sha: String,
    /// Heure Unix (secondes).
    pub started_at: u64,
    #[serde(default)]
    pub ended_at: Option<u64>,
    pub state: TaskState,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub files_changed: Option<usize>,
    /// Garde-fous enfreints (phrases).
    #[serde(default)]
    pub violations: Vec<String>,
    #[serde(default)]
    pub cost_usd: Option<f64>,
    /// Journal brut de la tâche.
    pub log: PathBuf,
}

/// Suivi tiré du flux de Claude Code.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Progress {
    /// Dernière action, prête à être dite (« lance « cargo test » »).
    pub last_action: Option<String>,
    pub last_text: Option<String>,
    pub result: Option<RunResult>,
    /// Lignes lues.
    pub lines: usize,
}

/// Mot de réveil : jamais dit dans une phrase de Jarvis, il couperait l'annonce.
pub const WAKE_WORD: &str = "jarvis";

/// Vrai si `text` contient le mot de réveil.
pub fn has_wake_word(text: &str) -> bool {
    fold(text).split(' ').any(|w| w == WAKE_WORD)
}

/// Nom d'un projet tel qu'il est dit à voix haute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpokenName {
    /// Son nom (ou un alias sans article) : « le projet Mawena ».
    Name(String),
    /// Un alias avec article : « l'assistant ».
    Article(String),
    /// Ni nom ni alias dicible : « votre projet ».
    Generic,
}

impl SpokenName {
    /// Nom dit : le nom, sauf s'il contient le mot de réveil ; alors le premier
    /// alias qui ne le contient pas ; sinon « votre projet ».
    pub fn of(name: &str, aliases: &[String]) -> SpokenName {
        if !has_wake_word(name) {
            return SpokenName::Name(name.to_string());
        }
        match aliases
            .iter()
            .map(|a| a.trim())
            .find(|a| !a.is_empty() && !has_wake_word(a))
        {
            Some(a) if starts_with_article(a) => SpokenName::Article(a.to_string()),
            Some(a) => SpokenName::Name(a.to_string()),
            None => SpokenName::Generic,
        }
    }

    /// Après « sur » : « Mawena », « l'assistant », « votre projet ».
    pub fn bare(&self) -> String {
        match self {
            SpokenName::Name(n) | SpokenName::Article(n) => n.clone(),
            SpokenName::Generic => "votre projet".into(),
        }
    }

    /// « le projet Mawena », « l'assistant », « votre projet ».
    pub fn the_project(&self) -> String {
        match self {
            SpokenName::Name(n) => format!("le projet {n}"),
            other => other.bare(),
        }
    }

    /// En tête de phrase : « Le projet Mawena », « L'assistant ».
    pub fn the_project_capitalized(&self) -> String {
        let text = self.the_project();
        let mut chars = text.chars();
        match chars.next() {
            Some(c) => c.to_uppercase().chain(chars).collect(),
            None => text,
        }
    }

    /// « du projet Mawena », « de l'assistant », « du cerveau », « de votre projet ».
    pub fn of_project(&self) -> String {
        match self {
            SpokenName::Name(n) => format!("du projet {n}"),
            SpokenName::Article(a) => {
                let lower = a.to_lowercase();
                if lower.starts_with("le ") {
                    format!("du {}", &a[3..])
                } else if lower.starts_with("les ") {
                    format!("des {}", &a[4..])
                } else {
                    format!("de {a}")
                }
            }
            SpokenName::Generic => "de votre projet".into(),
        }
    }
}

/// « le », « la », « les », « l' » en tête.
fn starts_with_article(text: &str) -> bool {
    let lower = text.to_lowercase();
    ["le ", "la ", "les ", "l'", "l’"]
        .iter()
        .any(|a| lower.starts_with(a))
}

/// Ligne `result` du flux.
#[derive(Debug, Clone, PartialEq)]
pub struct RunResult {
    pub is_error: bool,
    pub subtype: String,
    pub text: String,
    pub cost_usd: Option<f64>,
    pub num_turns: Option<u32>,
}

/// Pourquoi le processus s'est arrêté.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndReason {
    /// Sorti de lui-même.
    Exited,
    /// Arrêt demandé.
    Stopped,
    /// Durée maximale dépassée.
    TimedOut,
    /// Aucune sortie depuis trop longtemps.
    Idle,
}

/// Commande comprise dans une phrase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentCommand {
    Start {
        project: String,
        instruction: String,
    },
    Status {
        project: Option<String>,
    },
    Stop {
        project: Option<String>,
    },
    /// Verbe de code et « projet X » inconnu.
    UnknownProject {
        heard: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enregistrement_aller_retour_json() {
        let r = TaskRecord {
            id: TaskId("abc".into()),
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
            cost_usd: Some(0.5),
            log: PathBuf::from("/tmp/abc.log"),
        };
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains(r#""id":"abc""#), "{json}");
        assert!(json.contains(r#""state":"running""#), "{json}");
        assert_eq!(serde_json::from_str::<TaskRecord>(&json).unwrap(), r);
        assert_eq!(
            serde_json::to_string(&TaskState::Stalled).unwrap(),
            r#""stalled""#
        );
    }

    #[test]
    fn nom_dit_du_projet_jarvis() {
        let alias = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        // Autre projet : son nom.
        let m = SpokenName::of("Mawena", &alias(&["mawena"]));
        assert_eq!(m, SpokenName::Name("Mawena".into()));
        assert_eq!(m.bare(), "Mawena");
        assert_eq!(m.of_project(), "du projet Mawena");
        assert_eq!(m.the_project_capitalized(), "Le projet Mawena");
        // Projet Jarvis : son premier alias dicible.
        let j = SpokenName::of("Jarvis", &alias(&["Jarvis 2", "l'assistant"]));
        assert_eq!(j, SpokenName::Article("l'assistant".into()));
        assert_eq!(j.bare(), "l'assistant");
        assert_eq!(j.of_project(), "de l'assistant");
        assert_eq!(j.the_project(), "l'assistant");
        assert_eq!(j.the_project_capitalized(), "L'assistant");
        assert_eq!(
            SpokenName::of("jarvis-essai", &alias(&["le cerveau"])).of_project(),
            "du cerveau"
        );
        assert_eq!(
            SpokenName::of("JARVIS", &alias(&["maison"])).the_project(),
            "le projet maison"
        );
        // Sans alias : « votre projet ».
        let g = SpokenName::of("Jarvis", &[]);
        assert_eq!(g, SpokenName::Generic);
        assert_eq!(g.bare(), "votre projet");
        assert_eq!(g.of_project(), "de votre projet");
        assert_eq!(g.the_project_capitalized(), "Votre projet");
        assert!(has_wake_word("feature/jarvis-finis-tests"));
        assert!(!has_wake_word("jarvisette"));
    }
}
