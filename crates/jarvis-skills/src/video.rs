//! « Lance la vidéo de… » : trouve une vidéo et la lance dans le navigateur.

use crate::media::MediaPlayers;
use crate::search::{SearchSettings, SearxClient};
use crate::{CommandRunner, Reply, Risk, Skill, SkillError, ToolSpec};
use std::sync::Arc;

/// Outil de la vidéo, proposé à l'IA et aux raccourcis.
pub fn tool_specs() -> Vec<ToolSpec> {
    vec![ToolSpec {
        name: "play_video".into(),
        description: "Trouve une vidéo (YouTube) et la lance dans le navigateur.".into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "La vidéo cherchée (ex. Daft Punk, bande-annonce de Dune)."
                }
            },
            "required": ["query"]
        }),
        risk: Risk::Direct,
    }]
}

/// Compétence vidéo : `play_video`.
#[allow(dead_code)] // Champs lus par `call` (tâche 3).
pub struct VideoSkill {
    client: Arc<SearxClient>,
    players: Arc<dyn MediaPlayers>,
    runner: Arc<dyn CommandRunner>,
    settings: SearchSettings,
}

impl VideoSkill {
    pub fn new(
        client: Arc<SearxClient>,
        players: Arc<dyn MediaPlayers>,
        runner: Arc<dyn CommandRunner>,
        settings: SearchSettings,
    ) -> Self {
        Self {
            client,
            players,
            runner,
            settings,
        }
    }
}

#[async_trait::async_trait]
impl Skill for VideoSkill {
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
    fn outil_video_direct() {
        let specs = tool_specs();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].name, "play_video");
        assert_eq!(specs[0].risk, Risk::Direct);
        assert_eq!(
            specs[0].parameters["required"],
            serde_json::json!(["query"])
        );
    }
}
