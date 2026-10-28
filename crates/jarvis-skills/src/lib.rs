//! Skill abstraction and command execution for Jarvis.

pub mod apps;
pub mod power;
pub mod runner;

#[cfg(any(test, feature = "test-util"))]
pub use runner::FakeRunner;
pub use runner::{CommandOutput, CommandRunner, SystemRunner};

use serde::Serialize;

/// Whether a tool can run immediately or needs confirmation (phase 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Risk {
    Direct,
    Risky,
}

/// Description of a tool a skill exposes.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    /// JSON schema (object) of the arguments.
    pub parameters: serde_json::Value,
    pub risk: Risk,
}

/// Issue d'une réponse : décide du bip joué par l'oreille.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Outcome {
    /// Action faite (bip de réussite).
    Done,
    /// Échec (bip d'échec).
    Failed,
    /// Simple information, sans bip.
    #[default]
    Info,
}

/// What Jarvis says after a skill ran.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    pub speech: String,
    pub outcome: Outcome,
}

impl Reply {
    /// Information, sans bip.
    pub fn new(speech: impl Into<String>) -> Self {
        Self {
            speech: speech.into(),
            outcome: Outcome::Info,
        }
    }

    /// Action réussie.
    pub fn done(speech: impl Into<String>) -> Self {
        Self {
            speech: speech.into(),
            outcome: Outcome::Done,
        }
    }

    /// Échec.
    pub fn failed(speech: impl Into<String>) -> Self {
        Self {
            speech: speech.into(),
            outcome: Outcome::Failed,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SkillError {
    #[error("outil inconnu : {0}")]
    UnknownTool(String),
    #[error("arguments invalides : {0}")]
    BadArgs(String),
    /// Cible introuvable ; le message est prêt à être dit.
    #[error("{0}")]
    NotFound(String),
    #[error("échec : {0}")]
    Failed(String),
}

#[async_trait::async_trait]
pub trait Skill: Send + Sync {
    fn tools(&self) -> Vec<ToolSpec>;

    /// Question posée avant un outil `Risky` (« Je mets le PC en veille ? »).
    /// `None` : le cerveau formule une question générale.
    fn confirmation(&self, _tool: &str, _args: &serde_json::Value) -> Option<String> {
        None
    }

    async fn call(&self, tool: &str, args: serde_json::Value) -> Result<Reply, SkillError>;
}
pub mod media;
