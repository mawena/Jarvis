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
use std::time::Duration;
use tracing::{debug, info, warn};

pub struct Brain {
    skills: Vec<Arc<dyn Skill>>,
    /// Compétence système : appelable par les raccourcis, jamais proposée à l'IA.
    system: Arc<SystemSkill>,
    local: Arc<dyn LlmBackend>,
    online: Arc<dyn LlmBackend>,
    modes: Arc<ModeStore>,
    skill_timeout: Duration,
}

/// Résultat d'un appel de compétence.
enum Outcome {
    Reply(Reply),
    /// Cible introuvable : message prêt à être dit.
    NotFound(String),
}

impl Brain {
    pub fn new(
        skills: Vec<Arc<dyn Skill>>,
        local: Arc<dyn LlmBackend>,
        online: Arc<dyn LlmBackend>,
        modes: Arc<ModeStore>,
    ) -> Brain {
        Brain {
            skills,
            system: Arc::new(SystemSkill::new(modes.clone())),
            local,
            online,
            modes,
            skill_timeout: SKILL_TIMEOUT,
        }
    }

    /// Change le délai maximal d'un appel de compétence (tests).
    pub fn with_skill_timeout(mut self, timeout: Duration) -> Brain {
        self.skill_timeout = timeout;
        self
    }

    /// Les outils proposés à l'IA : ceux des compétences, sans les outils système
    /// (`set_mode`, `status`), que les raccourcis couvrent.
    pub fn tools(&self) -> Vec<ToolSpec> {
        self.skills.iter().flat_map(|s| s.tools()).collect()
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
            match self.run_intent(intent).await {
                Outcome::Reply(r) => return r,
                Outcome::NotFound(m) => {
                    debug!(message = %m, "raccourci sans résultat, repli sur l'IA");
                }
            }
        }
        let mode = self.modes.get();
        let (backend, route) = match mode {
            Mode::Local => (&self.local, "local"),
            Mode::Online => (&self.online, "online"),
        };
        debug!(route, "compréhension par IA");
        let tools = self.tools();
        match backend.understand(text, &tools).await {
            Ok(Understanding::Tool(intent)) => {
                if mode == Mode::Local
                    && intent.tool == "set_mode"
                    && intent.args.get("mode").and_then(|m| m.as_str()) == Some("online")
                {
                    return Reply::new(CONFIRM_ONLINE);
                }
                match self.run_intent(intent).await {
                    Outcome::Reply(r) => r,
                    Outcome::NotFound(m) => Reply::new(m),
                }
            }
            Ok(Understanding::Answer(a)) if a.trim().is_empty() => Reply::new(BAD_RESPONSE),
            Ok(Understanding::Answer(a)) => Reply::new(a),
            Err(e) => Reply::new(llm_error_phrase(&e, mode)),
        }
    }

    async fn run_intent(&self, intent: Intent) -> Outcome {
        let system: Arc<dyn Skill> = self.system.clone();
        let skill = self
            .skills
            .iter()
            .chain(std::iter::once(&system))
            .find(|s| s.tools().iter().any(|t| t.name == intent.tool));
        let Some(skill) = skill else {
            return Outcome::Reply(Reply::new(UNKNOWN));
        };
        let call = skill.call(&intent.tool, intent.args);
        let result = match tokio::time::timeout(self.skill_timeout, call).await {
            Ok(r) => r,
            Err(_) => {
                warn!(tool = %intent.tool, "délai de la compétence dépassé");
                return Outcome::Reply(Reply::new(TOO_SLOW));
            }
        };
        match result {
            Ok(reply) => Outcome::Reply(reply),
            Err(SkillError::NotFound(m)) => Outcome::NotFound(m),
            Err(SkillError::UnknownTool(_) | SkillError::BadArgs(_)) => {
                Outcome::Reply(Reply::new(UNKNOWN))
            }
            Err(SkillError::Failed(detail)) => {
                Outcome::Reply(Reply::new(format!("Ça n'a pas marché : {detail}.")))
            }
        }
    }
}

const SKILL_TIMEOUT: Duration = Duration::from_secs(5);
const UNKNOWN: &str = "Je ne sais pas faire ça.";
const BAD_RESPONSE: &str = "Je n'ai pas compris la réponse de l'IA.";
const TOO_SLOW: &str = "Ça n'a pas marché : la commande a pris trop de temps.";
const CONFIRM_ONLINE: &str = "Dites « passe en mode en ligne » pour confirmer.";

fn llm_error_phrase(e: &LlmError, mode: Mode) -> String {
    match (e, mode) {
        (LlmError::Unreachable(_), Mode::Local) => {
            "L'IA locale ne répond pas. Ollama est-il lancé ?".into()
        }
        (LlmError::Unreachable(_), Mode::Online) => {
            "Je n'arrive pas à joindre l'IA en ligne. Dites « passe en mode local » pour continuer sans elle."
                .into()
        }
        (LlmError::Rejected(_), Mode::Local) => {
            "L'IA locale a refusé la demande (modèle installé ?).".into()
        }
        (LlmError::Rejected(_), Mode::Online) => "La clé de l'IA en ligne est refusée.".into(),
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
        assert_eq!(s.online.calls(), 0);
    }

    #[tokio::test]
    async fn refus_traduit_en_phrase() {
        let s = setup(
            FakeLlm::new(Err(LlmError::Rejected("404".into()))),
            FakeLlm::new(Err(LlmError::Rejected("401".into()))),
        );
        let r = s.brain.handle_text("quel temps fait-il").await;
        assert_eq!(
            r.speech,
            "L'IA locale a refusé la demande (modèle installé ?)."
        );
        s.brain.set_mode(Mode::Online).unwrap();
        let r = s.brain.handle_text("quel temps fait-il").await;
        assert_eq!(r.speech, "La clé de l'IA en ligne est refusée.");
    }

    fn not_found_skill() -> Arc<FakeSkill> {
        Arc::new(
            FakeSkill::new(&["app_launch"])
                .with_result(Err(SkillError::NotFound("Je ne trouve pas x.".into()))),
        )
    }

    fn brain_with(skill: Arc<FakeSkill>, local: Arc<FakeLlm>) -> (Brain, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let brain = Brain::new(
            vec![skill],
            local,
            Arc::new(answer("y")),
            Arc::new(ModeStore::new(dir.path().join("mode"), Mode::Local)),
        );
        (brain, dir)
    }

    #[tokio::test]
    async fn raccourci_introuvable_repli_ia() {
        let skill = not_found_skill();
        let local = Arc::new(answer("Voici la réponse de l'IA."));
        let (brain, _dir) = brain_with(skill.clone(), local.clone());
        let r = brain.handle_text("ouvre le truc").await;
        assert_eq!(r.speech, "Voici la réponse de l'IA.");
        assert_eq!(local.calls(), 1);
        assert_eq!(skill.calls().len(), 1);
    }

    #[tokio::test]
    async fn ia_introuvable_dit_le_message() {
        let local = Arc::new(FakeLlm::new(Ok(tool_understanding("app_launch"))));
        let (brain, _dir) = brain_with(not_found_skill(), local);
        let r = brain.handle_text("fais apparaître le truc").await;
        assert_eq!(r.speech, "Je ne trouve pas x.");
    }

    #[tokio::test]
    async fn competence_trop_lente() {
        let skill =
            Arc::new(FakeSkill::new(&["media_next"]).with_delay(Duration::from_millis(500)));
        let (brain, _dir) = brain_with(skill, Arc::new(answer("x")));
        let brain = brain.with_skill_timeout(Duration::from_millis(30));
        let r = brain.handle_text("suivant").await;
        assert_eq!(
            r.speech,
            "Ça n'a pas marché : la commande a pris trop de temps."
        );
    }

    #[test]
    fn outils_de_l_ia_sans_outils_systeme() {
        let s = setup(answer("x"), answer("y"));
        let names: Vec<String> = s.brain.tools().into_iter().map(|t| t.name).collect();
        assert_eq!(names, vec!["media_next".to_string()]);
    }

    #[tokio::test]
    async fn les_outils_envoyes_a_l_ia_excluent_le_systeme() {
        let s = setup(answer("x"), answer("y"));
        s.brain.handle_text("quel temps fait-il").await;
        let sent = s.local.last_tools();
        assert_eq!(sent, vec!["media_next".to_string()]);
    }

    #[tokio::test]
    async fn ia_ne_bascule_pas_en_ligne() {
        let set_online = Understanding::Tool(Intent::new(
            "set_mode",
            serde_json::json!({"mode": "online"}),
        ));
        let s = setup(FakeLlm::new(Ok(set_online)), answer("y"));
        let r = s.brain.handle_text("passe sur l'IA en ligne").await;
        assert_eq!(r.speech, "Dites « passe en mode en ligne » pour confirmer.");
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
