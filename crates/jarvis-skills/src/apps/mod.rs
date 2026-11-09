//! Applications skill: bring a running app to the front or launch it.

mod index;

pub use index::{AppEntry, AppIndex};

use crate::{CommandRunner, Reply, Risk, Skill, SkillError, ToolSpec};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;

pub struct AppsSkill {
    pub index: AppIndex,
    pub aliases: HashMap<String, String>,
    pub runner: Arc<dyn CommandRunner>,
}

#[derive(Deserialize)]
struct LaunchArgs {
    name: String,
}

impl AppsSkill {
    pub fn new(
        index: AppIndex,
        aliases: HashMap<String, String>,
        runner: Arc<dyn CommandRunner>,
    ) -> Self {
        Self {
            index,
            aliases,
            runner,
        }
    }
}

#[async_trait::async_trait]
impl Skill for AppsSkill {
    fn tools(&self) -> Vec<ToolSpec> {
        vec![ToolSpec {
            name: "app_launch".into(),
            description:
                "Ouvre une application, ou la met au premier plan si elle est déjà ouverte.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Nom de l'application à ouvrir (ex. Spotify, VS Code)."
                    }
                },
                "required": ["name"]
            }),
            risk: Risk::Direct,
        }]
    }

    async fn call(&self, tool: &str, args: serde_json::Value) -> Result<Reply, SkillError> {
        if tool != "app_launch" {
            return Err(SkillError::UnknownTool(tool.to_string()));
        }
        let LaunchArgs { name } =
            serde_json::from_value(args).map_err(|e| SkillError::BadArgs(e.to_string()))?;
        let Some(app) = self.index.find(&name, &self.aliases) else {
            return Err(SkillError::NotFound(format!(
                "Je ne trouve pas l'application {name}."
            )));
        };
        if let Some(class) = &app.wm_class {
            let focused = self
                .runner
                .run("wmctrl", &["-x".into(), "-a".into(), class.clone()])
                .map(|o| o.success)
                .unwrap_or(false);
            if focused {
                return Ok(Reply::done(format!("Voici {}.", app.name)));
            }
        }
        self.runner
            .spawn_detached(
                "gio",
                &["launch".into(), app.path.to_string_lossy().into_owned()],
            )
            .map_err(|e| SkillError::Failed(e.to_string()))?;
        Ok(Reply::done(format!("Je lance {}.", app.name)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CommandOutput, FakeRunner};
    use std::path::PathBuf;

    fn fixtures() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/applications")
    }

    fn index() -> AppIndex {
        AppIndex::load(&[fixtures()])
    }

    fn skill(runner: Arc<FakeRunner>) -> AppsSkill {
        AppsSkill::new(index(), HashMap::new(), runner)
    }

    fn outcome(success: bool) -> CommandOutput {
        CommandOutput {
            success,
            stdout: String::new(),
        }
    }

    #[test]
    fn index_ignore_les_cachees() {
        let idx = index();
        assert_eq!(idx.entries.len(), 3);
        assert!(idx.entries.iter().all(|e| e.id != "hidden"));
    }

    #[test]
    fn premier_dossier_gagne() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("code.desktop"),
            "[Desktop Entry]\nType=Application\nName=Autre\nExec=x\n",
        )
        .unwrap();
        let idx = AppIndex::load(&[tmp.path().to_path_buf(), fixtures(), "/absent".into()]);
        assert_eq!(idx.entries.len(), 3);
        let code = idx.entries.iter().find(|e| e.id == "code").unwrap();
        assert_eq!(code.name, "Autre");
    }

    #[test]
    fn alias_vs_code() {
        let aliases = HashMap::from([("vs code".to_string(), "code".to_string())]);
        assert_eq!(index().find("vs code", &aliases).unwrap().id, "code");
    }

    #[test]
    fn recherche_floue() {
        let idx = index();
        let none = HashMap::new();
        assert_eq!(
            idx.find("spotify s il te plait", &none).unwrap().id,
            "spotify_spotify"
        );
        assert_eq!(
            idx.find("nautilus", &none).unwrap().id,
            "org.gnome.Nautilus"
        );
        assert!(idx.find("zorglub", &none).is_none());
    }

    #[tokio::test]
    async fn appli_introuvable() {
        let runner = Arc::new(FakeRunner::new());
        let err = skill(runner.clone())
            .call("app_launch", serde_json::json!({"name": "zorglub"}))
            .await
            .unwrap_err();
        assert!(
            matches!(&err, SkillError::NotFound(m) if m == "Je ne trouve pas l'application zorglub."),
            "{err:?}"
        );
        assert!(runner.calls().is_empty());
    }

    #[tokio::test]
    async fn deja_ouverte_mise_au_premier_plan() {
        let runner = Arc::new(FakeRunner::new());
        runner.respond("wmctrl", outcome(true));
        let r = skill(runner.clone())
            .call("app_launch", serde_json::json!({"name": "spotify"}))
            .await
            .unwrap();
        assert_eq!(r.speech, "Voici Spotify.");
        assert_eq!(r.outcome, crate::Outcome::Done);
        assert_eq!(
            runner.calls(),
            vec![(
                "wmctrl".to_string(),
                vec!["-x".to_string(), "-a".to_string(), "spotify".to_string()]
            )]
        );
    }

    #[tokio::test]
    async fn pas_ouverte_lancee() {
        let runner = Arc::new(FakeRunner::new());
        runner.respond("wmctrl", outcome(false));
        let r = skill(runner.clone())
            .call("app_launch", serde_json::json!({"name": "spotify"}))
            .await
            .unwrap();
        assert_eq!(r.speech, "Je lance Spotify.");
        assert_eq!(r.outcome, crate::Outcome::Done);
        let expected = fixtures()
            .join("spotify_spotify.desktop")
            .to_string_lossy()
            .into_owned();
        assert_eq!(
            runner.calls().last().unwrap(),
            &("gio".to_string(), vec!["launch".to_string(), expected])
        );
    }

    #[tokio::test]
    async fn args_invalides() {
        let runner = Arc::new(FakeRunner::new());
        let err = skill(runner)
            .call("app_launch", serde_json::json!({"nom": "x"}))
            .await
            .unwrap_err();
        assert!(matches!(err, SkillError::BadArgs(_)));
    }
}
