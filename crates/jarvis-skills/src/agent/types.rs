//! Types partagés de l'agent de code.

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
    /// Durée maximale dépassée, ou silence trop long.
    TimedOut,
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
    }
}
