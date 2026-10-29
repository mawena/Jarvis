//! Compréhension d'une phrase par un modèle de langage (local ou en ligne).

pub mod claude;
pub mod ollama;
pub mod router;

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

    /// Complétion simple, **sans outils** (résumé d'une recherche).
    async fn complete(
        &self,
        _system: &str,
        _user: &str,
        _max_tokens: u32,
    ) -> Result<String, LlmError> {
        Err(LlmError::BadResponse(
            "complétion non prise en charge".into(),
        ))
    }
}

/// Ton commun aux deux consignes. Court : les petits modèles locaux (llama3.2:3b,
/// qwen2.5:3b) doivent le suivre.
fn persona(address: &str) -> String {
    format!(
        "Tu es Jarvis, le majordome de {address}, discret et poli. Réponds en français, \
         en une seule phrase, deux au plus. Vouvoie-le. Dis « {address} » rarement, \
         par exemple « Bien, {address}. » ou « Certainement, {address}. ». Jamais \
         ni liste, ni markdown, ni émoji : ta réponse est lue à voix haute, écris donc \
         les unités en toutes lettres (« 20 degrés », pas « 20 °C »)."
    )
}

/// Consigne de la conversation libre (mode local, sans outils).
pub fn chat_prompt(address: &str) -> String {
    format!(
        "{} Ne dis jamais « mon ami ». Si tu ne sais pas, dis-le simplement.",
        persona(address)
    )
}

/// Consigne avec outils (compréhension d'une commande).
pub fn system_prompt(address: &str) -> String {
    format!(
        "{} Si la demande correspond à un outil, appelle cet outil. Sinon, réponds directement.",
        persona(address)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Backend qui ne sait que comprendre.
    struct SansCompletion;

    #[async_trait::async_trait]
    impl LlmBackend for SansCompletion {
        async fn understand(&self, _: &str, _: &[ToolSpec]) -> Result<Understanding, LlmError> {
            Ok(Understanding::Answer("ok".into()))
        }
    }

    #[tokio::test]
    async fn complete_par_defaut_non_pris_en_charge() {
        let r = SansCompletion.complete("consigne", "question", 80).await;
        assert!(
            matches!(&r, Err(LlmError::BadResponse(m)) if m == "complétion non prise en charge"),
            "{r:?}"
        );
    }

    #[test]
    fn prompt_contient_le_nom() {
        let p = system_prompt("Charles");
        assert!(
            p.starts_with("Tu es Jarvis, le majordome de Charles,"),
            "{p}"
        );
        assert!(p.ends_with("Sinon, réponds directement."), "{p}");
    }

    #[test]
    fn prompt_ton_majordome() {
        for p in [system_prompt("Monsieur"), chat_prompt("Monsieur")] {
            assert!(p.contains("majordome"), "{p}");
            assert!(p.contains("Vouvoie"), "{p}");
            assert!(p.contains("une seule phrase"), "{p}");
            assert!(p.contains("« Bien, Monsieur. »"), "{p}");
            assert!(p.contains("ni liste, ni markdown, ni émoji"), "{p}");
            assert!(p.contains("lue à voix haute"), "{p}");
            // Court : les petits modèles locaux doivent la suivre.
            assert!(p.chars().count() < 600, "{} caractères", p.chars().count());
        }
        assert!(chat_prompt("Monsieur").contains("mon ami"));
    }
}
