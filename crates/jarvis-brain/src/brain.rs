//! Orchestrateur : raccourci, sinon IA du mode courant, puis compétence.

use crate::fastpath::match_fast;
use crate::intent::Intent;
use crate::llm::{LlmBackend, LlmError, Understanding};
use crate::mode_store::ModeStore;
use crate::system_skill::SystemSkill;
use jarvis_protocol::Mode;
use jarvis_skills::{Reply, Skill, SkillError, ToolSpec};
use std::io;
use std::sync::Arc;
use tracing::{debug, info};

pub struct Brain {
    skills: Vec<Arc<dyn Skill>>,
    local: Arc<dyn LlmBackend>,
    online: Arc<dyn LlmBackend>,
    modes: Arc<ModeStore>,
}

impl Brain {
    pub fn new(
        mut skills: Vec<Arc<dyn Skill>>,
        local: Arc<dyn LlmBackend>,
        online: Arc<dyn LlmBackend>,
        modes: Arc<ModeStore>,
    ) -> Brain {
        skills.push(Arc::new(SystemSkill::new(modes.clone())));
        Brain {
            skills,
            local,
            online,
            modes,
        }
    }

    pub fn mode(&self) -> Mode {
        self.modes.get()
    }

    pub fn set_mode(&self, mode: Mode) -> io::Result<()> {
        self.modes.set(mode)
    }

    /// Ne renvoie jamais d'erreur : tout échec devient une phrase.
    pub async fn handle_text(&self, text: &str) -> Reply {
        info!(phrase = %text, "phrase reçue");
        if let Some(intent) = match_fast(text) {
            debug!(route = "fast", tool = %intent.tool);
            return self.run_intent(intent).await;
        }
        let mode = self.modes.get();
        let (backend, route) = match mode {
            Mode::Local => (&self.local, "local"),
            Mode::Online => (&self.online, "online"),
        };
        debug!(route, "compréhension par IA");
        let tools: Vec<ToolSpec> = self.skills.iter().flat_map(|s| s.tools()).collect();
        match backend.understand(text, &tools).await {
            Ok(Understanding::Tool(intent)) => self.run_intent(intent).await,
            Ok(Understanding::Answer(a)) if a.trim().is_empty() => Reply::new(BAD_RESPONSE),
            Ok(Understanding::Answer(a)) => Reply::new(a),
            Err(e) => Reply::new(llm_error_phrase(&e, mode)),
        }
    }

    async fn run_intent(&self, intent: Intent) -> Reply {
        let skill = self
            .skills
            .iter()
            .find(|s| s.tools().iter().any(|t| t.name == intent.tool));
        let Some(skill) = skill else {
            return Reply::new(UNKNOWN);
        };
        match skill.call(&intent.tool, intent.args).await {
            Ok(reply) => reply,
            Err(SkillError::UnknownTool(_) | SkillError::BadArgs(_)) => Reply::new(UNKNOWN),
            Err(SkillError::Failed(detail)) => Reply::new(format!("Ça n'a pas marché : {detail}.")),
        }
    }
}

const UNKNOWN: &str = "Je ne sais pas faire ça.";
const BAD_RESPONSE: &str = "Je n'ai pas compris la réponse de l'IA.";

fn llm_error_phrase(e: &LlmError, mode: Mode) -> String {
    match (e, mode) {
        (LlmError::Unreachable(_), Mode::Local) => {
            "L'IA locale ne répond pas. Ollama est-il lancé ?".into()
        }
        (LlmError::Unreachable(_), Mode::Online) => {
            "Je n'arrive pas à joindre l'IA en ligne. Dites « passe en mode local » pour continuer sans elle."
                .into()
        }
        (LlmError::MissingApiKey(var), _) => {
            format!("La clé de l'IA en ligne est absente : définissez {var}.")
        }
        (LlmError::BadResponse(_), _) => BAD_RESPONSE.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{FakeLlm, FakeSkill, tool_understanding};

    struct Setup {
        brain: Brain,
        local: Arc<FakeLlm>,
        online: Arc<FakeLlm>,
        skill: Arc<FakeSkill>,
        path: std::path::PathBuf,
        _dir: tempfile::TempDir,
    }

    fn setup(local: FakeLlm, online: FakeLlm) -> Setup {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mode");
        let local = Arc::new(local);
        let online = Arc::new(online);
        let skill = Arc::new(FakeSkill::new(&["media_next"]));
        let brain = Brain::new(
            vec![skill.clone()],
            local.clone(),
            online.clone(),
            Arc::new(ModeStore::new(path.clone(), Mode::Local)),
        );
        Setup {
            brain,
            local,
            online,
            skill,
            path,
            _dir: dir,
        }
    }

    fn answer(s: &str) -> FakeLlm {
        FakeLlm::new(Ok(Understanding::Answer(s.into())))
    }

    #[tokio::test]
    async fn raccourci_sans_ia() {
        let s = setup(answer("x"), answer("y"));
        let r = s.brain.handle_text("suivant").await;
        assert_eq!(r.speech, "ok");
        assert_eq!(s.skill.calls()[0].0, "media_next");
        assert_eq!(s.local.calls() + s.online.calls(), 0);
    }

    #[tokio::test]
    async fn mode_local_n_appelle_que_le_local() {
        let s = setup(answer("Il fait beau."), answer("y"));
        let r = s.brain.handle_text("quel temps fait-il").await;
        assert_eq!(r.speech, "Il fait beau.");
        assert_eq!((s.local.calls(), s.online.calls()), (1, 0));
    }

    #[tokio::test]
    async fn mode_en_ligne() {
        let s = setup(answer("x"), answer("Soleil."));
        s.brain.set_mode(Mode::Online).unwrap();
        let r = s.brain.handle_text("quel temps fait-il").await;
        assert_eq!(r.speech, "Soleil.");
        assert_eq!((s.local.calls(), s.online.calls()), (0, 1));
    }

    #[tokio::test]
    async fn switch_vocal_persiste() {
        let s = setup(answer("x"), answer("y"));
        let r = s.brain.handle_text("passe en mode en ligne").await;
        assert_eq!(r.speech, "Mode en ligne activé.");
        assert_eq!(
            ModeStore::new(s.path.clone(), Mode::Local).get(),
            Mode::Online
        );
    }

    #[tokio::test]
    async fn outil_inconnu_reponse_polie() {
        let s = setup(
            FakeLlm::new(Ok(tool_understanding("lance_fusee"))),
            answer("y"),
        );
        let r = s.brain.handle_text("lance une fusée").await;
        assert_eq!(r.speech, "Je ne sais pas faire ça.");
    }

    #[tokio::test]
    async fn ollama_injoignable() {
        let s = setup(
            FakeLlm::new(Err(LlmError::Unreachable("x".into()))),
            answer("y"),
        );
        let r = s.brain.handle_text("quel temps fait-il").await;
        assert_eq!(r.speech, "L'IA locale ne répond pas. Ollama est-il lancé ?");
        assert_eq!(s.brain.mode(), Mode::Local);
    }

    #[tokio::test]
    async fn en_ligne_injoignable_ne_bascule_pas() {
        let s = setup(
            answer("x"),
            FakeLlm::new(Err(LlmError::Unreachable("x".into()))),
        );
        s.brain.set_mode(Mode::Online).unwrap();
        let r = s.brain.handle_text("quel temps fait-il").await;
        assert_eq!(
            r.speech,
            "Je n'arrive pas à joindre l'IA en ligne. Dites « passe en mode local » pour continuer sans elle."
        );
        assert_eq!(s.brain.mode(), Mode::Online);
        assert_eq!(s.local.calls(), 0);
    }

    #[tokio::test]
    async fn cle_absente_reponse_vide_et_echec_competence() {
        let s = setup(
            FakeLlm::new(Err(LlmError::MissingApiKey("ANTHROPIC_API_KEY".into()))),
            answer("y"),
        );
        let r = s.brain.handle_text("bonjour").await;
        assert_eq!(
            r.speech,
            "La clé de l'IA en ligne est absente : définissez ANTHROPIC_API_KEY."
        );

        let s = setup(answer("  "), answer("y"));
        let r = s.brain.handle_text("bonjour").await;
        assert_eq!(r.speech, "Je n'ai pas compris la réponse de l'IA.");

        let dir = tempfile::tempdir().unwrap();
        let skill = Arc::new(
            FakeSkill::new(&["media_next"]).with_result(Err(SkillError::Failed("boum".into()))),
        );
        let brain = Brain::new(
            vec![skill],
            Arc::new(answer("x")),
            Arc::new(answer("y")),
            Arc::new(ModeStore::new(dir.path().join("mode"), Mode::Local)),
        );
        assert_eq!(
            brain.handle_text("suivant").await.speech,
            "Ça n'a pas marché : boum."
        );
    }
}
