//! Compétence système : changement de mode et statut.

use crate::mode_store::ModeStore;
use jarvis_protocol::Mode;
use jarvis_skills::{Reply, Risk, Skill, SkillError, ToolSpec};
use std::sync::Arc;

pub struct SystemSkill {
    modes: Arc<ModeStore>,
}

impl SystemSkill {
    pub fn new(modes: Arc<ModeStore>) -> Self {
        Self { modes }
    }
}

#[async_trait::async_trait]
impl Skill for SystemSkill {
    fn tools(&self) -> Vec<ToolSpec> {
        vec![
            ToolSpec {
                name: "set_mode".into(),
                description:
                    "Change le mode de compréhension : local (Ollama) ou en ligne (Claude).".into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {"mode": {"type": "string", "enum": ["local", "online"]}},
                    "required": ["mode"]
                }),
                risk: Risk::Direct,
            },
            ToolSpec {
                name: "status".into(),
                description: "Indique le mode actuel et l'état de Jarvis.".into(),
                parameters: serde_json::json!({"type": "object", "properties": {}}),
                risk: Risk::Direct,
            },
        ]
    }

    async fn call(&self, tool: &str, args: serde_json::Value) -> Result<Reply, SkillError> {
        match tool {
            "set_mode" => {
                let mode: Mode = args
                    .get("mode")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| SkillError::BadArgs("mode manquant".into()))?
                    .parse()
                    .map_err(|e| SkillError::BadArgs(format!("{e}")))?;
                self.modes
                    .set(mode)
                    .map_err(|e| SkillError::Failed(e.to_string()))?;
                Ok(Reply::done(format!("Mode {mode} activé.")))
            }
            "status" => Ok(Reply::new(format!(
                "Je suis en mode {}. Tout va bien.",
                self.modes.get()
            ))),
            other => Err(SkillError::UnknownTool(other.into())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skill() -> (SystemSkill, Arc<ModeStore>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let modes = Arc::new(ModeStore::new(dir.path().join("mode"), Mode::Local));
        (SystemSkill::new(modes.clone()), modes, dir)
    }

    #[tokio::test]
    async fn set_mode_invalide_est_bad_args() {
        let (s, modes, _d) = skill();
        let r = s.call("set_mode", serde_json::json!({"mode": "bof"})).await;
        assert!(matches!(r, Err(SkillError::BadArgs(_))));
        let r = s.call("set_mode", serde_json::json!({})).await;
        assert!(matches!(r, Err(SkillError::BadArgs(_))));
        assert_eq!(modes.get(), Mode::Local);
    }

    #[tokio::test]
    async fn status_dit_le_mode() {
        let (s, modes, _d) = skill();
        modes.set(Mode::Online).unwrap();
        let r = s.call("status", serde_json::json!({})).await.unwrap();
        assert_eq!(r.speech, "Je suis en mode en ligne. Tout va bien.");
    }
}
