//! Compréhension d'une phrase par un modèle de langage (local ou en ligne).

pub mod claude;
pub mod ollama;

use crate::intent::Intent;
use jarvis_skills::ToolSpec;

pub use claude::ClaudeBackend;
pub use ollama::OllamaBackend;

/// Ce que le modèle a compris : un outil à appeler ou une réponse directe.
#[derive(Debug, Clone, PartialEq)]
pub enum Understanding {
    Tool(Intent),
    Answer(String),
}

#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("modèle injoignable : {0}")]
    Unreachable(String),
    #[error("réponse inattendue du modèle : {0}")]
    BadResponse(String),
    #[error("requête refusée par le modèle : {0}")]
    Rejected(String),
    #[error("clé d'API absente (variable d'environnement {0})")]
    MissingApiKey(String),
}

#[async_trait::async_trait]
pub trait LlmBackend: Send + Sync {
    async fn understand(&self, text: &str, tools: &[ToolSpec]) -> Result<Understanding, LlmError>;
}

pub fn system_prompt(address: &str) -> String {
    format!(
        "Tu es Jarvis, l'assistant vocal de {address}. Tu réponds en français, en une ou deux \
         phrases, en vouvoyant. Si la demande correspond à un outil, appelle cet outil. \
         Sinon, réponds directement."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_contient_le_nom() {
        let p = system_prompt("Charles");
        assert!(p.starts_with("Tu es Jarvis, l'assistant vocal de Charles."));
        assert!(p.ends_with("Sinon, réponds directement."));
    }
}
