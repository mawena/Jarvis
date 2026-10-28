//! Alimentation du PC : veille, redémarrage, arrêt. Toujours après un « oui ».

use crate::{CommandRunner, Reply, Risk, Skill, SkillError, ToolSpec};
use std::sync::Arc;
use std::time::Duration;

/// Délai entre la réponse et la commande : le temps de synthétiser la confirmation
/// et de la dire dans l'oreille.
pub const POWER_DELAY: Duration = Duration::from_secs(4);

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

    /// Répond tout de suite ; la commande part [`POWER_DELAY`] plus tard, pour que
    /// la confirmation soit dite avant la veille ou l'arrêt. Un échec de `systemctl`
    /// n'est donc que journalisé.
    async fn call(&self, tool: &str, _args: serde_json::Value) -> Result<Reply, SkillError> {
        let Some(&Tool { verb, said, .. }) = find(tool) else {
            return Err(SkillError::UnknownTool(tool.into()));
        };
        let runner = self.runner.clone();
        tracing::info!(
            "systemctl {verb} programmé dans {} s",
            POWER_DELAY.as_secs()
        );
        tokio::spawn(async move {
            tokio::time::sleep(POWER_DELAY).await;
            // Appel bref : systemctl ne fait que transmettre la demande à logind.
            match runner.run("systemctl", &[verb.to_string()]) {
                Ok(out) if out.success => {}
                Ok(_) => tracing::warn!("systemctl {verb} a échoué"),
                Err(e) => tracing::warn!("systemctl {verb} impossible : {e}"),
            }
        });
        Ok(Reply::done(said))
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

    const PEU: Duration = Duration::from_millis(100);

    #[tokio::test(start_paused = true)]
    async fn power_appelle_systemctl_apres_la_reponse() {
        for (tool, verb, said) in [
            ("suspend", "suspend", "Je mets le PC en veille."),
            ("reboot", "reboot", "Je redémarre le PC."),
            ("shutdown", "poweroff", "J'éteins le PC."),
        ] {
            let (s, runner) = skill();
            // La réponse revient tout de suite, avant la commande.
            let r = s.call(tool, serde_json::json!({})).await.unwrap();
            assert_eq!(r, Reply::done(said), "{tool}");
            assert!(runner.calls().is_empty(), "{tool}");
            // Rien avant le délai : Jarvis a le temps de dire sa phrase.
            tokio::time::sleep(POWER_DELAY - PEU).await;
            assert!(runner.calls().is_empty(), "{tool}");
            tokio::time::sleep(2 * PEU).await;
            assert_eq!(
                runner.calls(),
                vec![("systemctl".to_string(), vec![verb.to_string()])],
                "{tool}"
            );
        }
    }

    #[test]
    fn delai_d_environ_quatre_secondes() {
        assert_eq!(POWER_DELAY, Duration::from_secs(4));
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

    #[tokio::test(start_paused = true)]
    async fn echec_de_systemctl_journalise_sans_paniquer() {
        let (s, runner) = skill();
        runner.respond(
            "systemctl",
            CommandOutput {
                success: false,
                stdout: String::new(),
            },
        );
        // L'échec arrive après la réponse : il n'est que journalisé.
        let r = s.call("reboot", serde_json::json!({})).await.unwrap();
        assert_eq!(r.outcome, Outcome::Done);
        tokio::time::sleep(POWER_DELAY + PEU).await;
        assert_eq!(runner.calls().len(), 1);
    }

    #[tokio::test]
    async fn outil_inconnu() {
        let (s, runner) = skill();
        let r = s.call("hibernate", serde_json::json!({})).await;
        assert!(matches!(r, Err(SkillError::UnknownTool(_))));
        assert!(runner.calls().is_empty());
    }
}
