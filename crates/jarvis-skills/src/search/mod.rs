//! Recherche sur internet par le SearXNG local, résumée à voix haute.

pub mod searxng;
pub mod spoken;
pub mod summary;

pub use searxng::{SearchHit, SearchPage, SearxClient, SearxError};
#[cfg(any(test, feature = "test-util"))]
pub use summary::FakeSummarizer;
pub use summary::{NoSummarizer, Source, Summarizer};

use crate::{CommandRunner, Reply, Risk, Skill, SkillError, ToolSpec};
use serde::Deserialize;
use std::sync::Arc;

/// Réglages de la section `[search]`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct SearchSettings {
    /// Adresse du SearXNG local.
    pub url: String,
    pub language: String,
    /// 0 : aucun filtre, 1 : modéré, 2 : strict.
    pub safesearch: u8,
    /// Résultats gardés en mémoire (« ouvre le deuxième résultat »).
    pub max_results: usize,
    /// Résultats donnés à l'IA pour le résumé.
    pub summary_sources: usize,
    /// Longueur maximale d'un extrait donné à l'IA.
    pub snippet_chars: usize,
    /// Délai de SearXNG.
    pub timeout_ms: u64,
    /// Résumé par l'IA ; sinon l'extrait seul.
    pub summarize: bool,
    pub local_summary_timeout_ms: u64,
    pub online_summary_timeout_ms: u64,
    pub summary_max_tokens: u32,
    /// Modèle Ollama du résumé ; vide : le modèle de compréhension.
    pub local_summary_model: String,
    /// Moteurs de SearXNG pour les vidéos.
    pub video_engines: Vec<String>,
    /// `yt-dlp` en secours si SearXNG ne trouve pas de vidéo.
    pub ytdlp: bool,
    pub ytdlp_timeout_s: u64,
}

impl Default for SearchSettings {
    fn default() -> Self {
        Self {
            url: "http://127.0.0.1:8888".into(),
            language: "fr-FR".into(),
            safesearch: 1,
            max_results: 5,
            summary_sources: 3,
            snippet_chars: 200,
            timeout_ms: 4000,
            summarize: true,
            local_summary_timeout_ms: 10_000,
            online_summary_timeout_ms: 5000,
            summary_max_tokens: 80,
            local_summary_model: String::new(),
            video_engines: vec!["youtube".into()],
            ytdlp: true,
            ytdlp_timeout_s: 8,
        }
    }
}

/// Outils de la recherche, proposés à l'IA et aux raccourcis.
pub fn tool_specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "web_search".into(),
            description: "Cherche sur internet et résume la réponse à voix haute \
                          (actualité, météo, horaires, faits récents)."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Ce qu'il faut chercher (ex. météo de demain à Lomé)."
                    }
                },
                "required": ["query"]
            }),
            risk: Risk::Direct,
        },
        ToolSpec {
            name: "open_results".into(),
            description: "Ouvre dans le navigateur les résultats de la dernière recherche, \
                          ou le résultat numéro index."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "index": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Numéro du résultat (1 pour le premier)."
                    }
                }
            }),
            risk: Risk::Direct,
        },
    ]
}

/// Compétence de recherche : `web_search` et `open_results`.
#[allow(dead_code)] // Champs lus par `call` (tâche 2).
pub struct SearchSkill {
    client: Arc<SearxClient>,
    summarizer: Arc<dyn Summarizer>,
    runner: Arc<dyn CommandRunner>,
    settings: SearchSettings,
}

impl SearchSkill {
    pub fn new(
        client: Arc<SearxClient>,
        summarizer: Arc<dyn Summarizer>,
        runner: Arc<dyn CommandRunner>,
        settings: SearchSettings,
    ) -> Self {
        Self {
            client,
            summarizer,
            runner,
            settings,
        }
    }
}

#[async_trait::async_trait]
impl Skill for SearchSkill {
    fn tools(&self) -> Vec<ToolSpec> {
        tool_specs()
    }

    async fn call(&self, _tool: &str, _args: serde_json::Value) -> Result<Reply, SkillError> {
        Err(SkillError::Failed("à venir".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outils_de_recherche_directs() {
        let specs = tool_specs();
        let names: Vec<_> = specs.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["web_search", "open_results"]);
        assert!(specs.iter().all(|t| t.risk == Risk::Direct));
        assert_eq!(
            specs[0].parameters["required"],
            serde_json::json!(["query"])
        );
        assert_eq!(specs[1].parameters["properties"]["index"]["minimum"], 1);
    }

    #[test]
    fn reglages_par_defaut() {
        let s: SearchSettings = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(s, SearchSettings::default());
        assert_eq!(s.url, "http://127.0.0.1:8888");
        let s: SearchSettings =
            serde_json::from_value(serde_json::json!({"summarize": false})).unwrap();
        assert!(!s.summarize);
        assert_eq!(s.max_results, 5);
    }
}
