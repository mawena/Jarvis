//! Skill abstraction and command execution for Jarvis.

pub mod agent;
pub mod announce;
pub mod apps;
pub mod browser;
pub mod ccs;
pub mod chantier;
pub mod power;
pub mod runner;
pub mod search;
pub mod text;
pub mod video;

#[cfg(any(test, feature = "test-util"))]
pub use announce::RecordingAnnouncer;
pub use announce::{Announcer, Notice, NullAnnouncer};
#[cfg(any(test, feature = "test-util"))]
pub use runner::FakeRunner;
pub use runner::{CommandOutput, CommandRunner, SystemRunner};

use serde::Serialize;
use std::time::Duration;

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

/// Outil choisi par le raccourci propre d'une compétence.
#[derive(Debug, Clone, PartialEq)]
pub struct Shortcut {
    pub tool: String,
    pub args: serde_json::Value,
}

/// Préparation d'une action risquée, avant la question (voir [`Skill::prepare`]).
#[derive(Debug, Clone, PartialEq)]
pub enum Preparation {
    /// Poser `question` (`None` : question générale du cerveau) ; sur « oui »,
    /// exécuter l'outil avec `args`, qui peuvent préciser la cible vue en préparant.
    Ask {
        question: Option<String>,
        args: serde_json::Value,
    },
    /// Rien à confirmer (cible introuvable, ambiguë…) : réponse dite tout de suite.
    Answer(Reply),
}

#[async_trait::async_trait]
pub trait Skill: Send + Sync {
    /// Tous les outils, appelables par les raccourcis et l'IA.
    fn tools(&self) -> Vec<ToolSpec>;

    /// Outils proposés à l'IA (par défaut : tous). Un outil absent ici n'est
    /// lancé que par un raccourci.
    fn llm_tools(&self) -> Vec<ToolSpec> {
        self.tools()
    }

    /// Raccourci propre à la compétence, essayé après les raccourcis du cerveau
    /// et avant l'IA, pour une voix vérifiée ou du texte tapé seulement. `text` est
    /// la phrase d'origine (casse, accents) : la compétence la normalise elle-même.
    fn shortcut(&self, _text: &str) -> Option<Shortcut> {
        None
    }

    /// Question posée avant un outil `Risky` (« Je mets le PC en veille ? »).
    /// `None` : le cerveau formule une question générale.
    fn confirmation(&self, _tool: &str, _args: &serde_json::Value) -> Option<String> {
        None
    }

    /// Avant la question d'un outil `Risky` : par défaut, [`Self::confirmation`] et
    /// les mêmes arguments. Une compétence peut y interroger un service (simulation
    /// sans effet) pour lire la question qu'il formule et figer la cible exacte.
    async fn prepare(
        &self,
        tool: &str,
        args: serde_json::Value,
    ) -> Result<Preparation, SkillError> {
        Ok(Preparation::Ask {
            question: self.confirmation(tool, &args),
            args,
        })
    }

    /// Délai propre à un outil. `None` : le délai commun du cerveau (5 s).
    fn timeout(&self, _tool: &str) -> Option<Duration> {
        None
    }

    async fn call(&self, tool: &str, args: serde_json::Value) -> Result<Reply, SkillError>;
}
pub mod media;
