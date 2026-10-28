//! Skill abstraction and command execution for Jarvis.

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

/// What Jarvis says after a skill ran.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    pub speech: String,
}

impl Reply {
    pub fn new(speech: impl Into<String>) -> Self {
        Self {
            speech: speech.into(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SkillError {
    #[error("outil inconnu : {0}")]
    UnknownTool(String),
    #[error("arguments invalides : {0}")]
    BadArgs(String),
    #[error("échec : {0}")]
    Failed(String),
}

#[async_trait::async_trait]
pub trait Skill: Send + Sync {
    fn tools(&self) -> Vec<ToolSpec>;
    async fn call(&self, tool: &str, args: serde_json::Value) -> Result<Reply, SkillError>;
}
pub mod media;
