//! Alimentation du PC : veille, redémarrage, arrêt. Toujours après un « oui ».

use crate::{CommandRunner, Reply, Risk, Skill, SkillError, ToolSpec};
use std::sync::Arc;

pub struct PowerSkill {
    runner: Arc<dyn CommandRunner>,
}

impl PowerSkill {
    pub fn new(runner: Arc<dyn CommandRunner>) -> Self {
        Self { runner }
    }
}

struct Tool {
    name: &'static str,
    description: &'static str,
    /// Verbe de `systemctl`.
    verb: &'static str,
    question: &'static str,
    said: &'static str,
}

const TOOLS: &[Tool] = &[
    Tool {
        name: "suspend",
        description: "Met le PC en veille.",
        verb: "suspend",
        question: "Je mets le PC en veille ?",
        said: "Je mets le PC en veille.",
    },
    Tool {
        name: "reboot",
        description: "Redémarre le PC.",
        verb: "reboot",
        question: "Je redémarre le PC ?",
        said: "Je redémarre le PC.",
    },
    Tool {
        name: "shutdown",
        description: "Éteint le PC.",
        verb: "poweroff",
        question: "J'éteins le PC ?",
        said: "J'éteins le PC.",
    },
];

fn find(tool: &str) -> Option<&'static Tool> {
    TOOLS.iter().find(|t| t.name == tool)
}

#[async_trait::async_trait]
impl Skill for PowerSkill {
    fn tools(&self) -> Vec<ToolSpec> {
        TOOLS
            .iter()
            .map(|t| ToolSpec {
                name: t.name.into(),
                description: t.description.into(),
                parameters: serde_json::json!({"type": "object", "properties": {}}),
                risk: Risk::Risky,
            })
            .collect()
    }

    fn confirmation(&self, tool: &str, _args: &serde_json::Value) -> Option<String> {
        find(tool).map(|t| t.question.to_string())
    }

    async fn call(&self, tool: &str, _args: serde_json::Value) -> Result<Reply, SkillError> {
        let Some(&Tool { verb, said, .. }) = find(tool) else {
            return Err(SkillError::UnknownTool(tool.into()));
        };
        let out = self
            .runner
            .run("systemctl", &[verb.to_string()])
            .map_err(|e| SkillError::Failed(format!("systemctl : {e}")))?;
        if out.success {
            Ok(Reply::done(said))
        } else {
            Err(SkillError::Failed(format!("systemctl {verb} a échoué")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CommandOutput, FakeRunner, Outcome};

    fn skill() -> (PowerSkill, Arc<FakeRunner>) {
        let runner = Arc::new(FakeRunner::new());
        (PowerSkill::new(runner.clone()), runner)
    }

    #[tokio::test]
    async fn power_appelle_systemctl() {
        for (tool, verb) in [
            ("suspend", "suspend"),
            ("reboot", "reboot"),
            ("shutdown", "poweroff"),
        ] {
            let (s, runner) = skill();
            let r = s.call(tool, serde_json::json!({})).await.unwrap();
            assert_eq!(r.outcome, Outcome::Done, "{tool}");
            assert_eq!(
                runner.calls(),
                vec![("systemctl".to_string(), vec![verb.to_string()])],
                "{tool}"
            );
        }
    }

    #[test]
    fn tous_les_outils_sont_risques_et_ont_une_question() {
        let (s, _) = skill();
        let tools = s.tools();
        assert_eq!(tools.len(), 3);
        for t in tools {
            assert_eq!(t.risk, Risk::Risky, "{}", t.name);
            assert!(
                s.confirmation(&t.name, &serde_json::json!({}))
                    .is_some_and(|q| q.ends_with('?')),
                "{}",
                t.name
            );
        }
        assert_eq!(
            s.confirmation("suspend", &serde_json::json!({})).as_deref(),
            Some("Je mets le PC en veille ?")
        );
    }

    #[tokio::test]
    async fn echec_de_systemctl() {
        let (s, runner) = skill();
        runner.respond(
            "systemctl",
            CommandOutput {
                success: false,
                stdout: String::new(),
            },
        );
        let r = s.call("reboot", serde_json::json!({})).await;
        assert!(matches!(r, Err(SkillError::Failed(_))));
    }

    #[tokio::test]
    async fn outil_inconnu() {
        let (s, runner) = skill();
        let r = s.call("hibernate", serde_json::json!({})).await;
        assert!(matches!(r, Err(SkillError::UnknownTool(_))));
        assert!(runner.calls().is_empty());
    }
}
