//! Orchestrateur : raccourci, sinon IA du mode courant, puis compétence.
//! Une action risquée attend d'abord un « oui » (voir [`Brain::answer_follow_up`]).

use crate::fastpath::{BrainCommand, match_brain_command, match_confirmation, match_fast};
use crate::intent::Intent;
use crate::llm::{LlmBackend, LlmError, Understanding};
use crate::mode_store::ModeStore;
use crate::system_skill::SystemSkill;
use jarvis_protocol::Mode;
use jarvis_skills::{Reply, Risk, Skill, SkillError, ToolSpec};
use std::io;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

/// Source de l'heure, remplaçable dans les tests.
pub trait Clock: Send + Sync {
    fn now(&self) -> Instant;
}

/// Horloge réelle.
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// Réponse du cerveau. `awaits_reply` : une question est posée, l'oreille doit
/// écouter la réponse (oui / non) sans mot d'éveil.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    pub reply: Reply,
    pub awaits_reply: bool,
}

impl From<Reply> for Answer {
    fn from(reply: Reply) -> Self {
        Answer {
            reply,
            awaits_reply: false,
        }
    }
}

/// Action risquée en attente d'un « oui ».
struct Pending {
    intent: Intent,
    question: String,
    asked_at: Instant,
}

pub struct Brain {
    skills: Vec<Arc<dyn Skill>>,
    /// Compétence système : appelable par les raccourcis, jamais proposée à l'IA.
    system: Arc<SystemSkill>,
    local: Arc<dyn LlmBackend>,
    online: Arc<dyn LlmBackend>,
    modes: Arc<ModeStore>,
    skill_timeout: Duration,
    local_model: String,
    clock: Arc<dyn Clock>,
    /// Une seule action en attente à la fois.
    pending: Mutex<Option<Pending>>,
    /// Dernière phrase dite (non vide), pour « répète ».
    last_speech: Mutex<Option<String>>,
}

/// Résultat d'une intention.
enum Step {
    Answer(Answer),
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
            local_model: String::new(),
            clock: Arc::new(SystemClock),
            pending: Mutex::new(None),
            last_speech: Mutex::new(None),
        }
    }

    /// Remplace l'horloge (tests du délai de confirmation).
    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Brain {
        self.clock = clock;
        self
    }

    pub fn now(&self) -> Instant {
        self.clock.now()
    }

    /// Efface l'action en attente. Vrai s'il y en avait une.
    pub fn clear_pending(&self) -> bool {
        self.pending.lock().unwrap().take().is_some()
    }

    pub fn has_pending(&self) -> bool {
        self.pending.lock().unwrap().is_some()
    }

    /// Nom du modèle local, affiché par `Status`.
    pub fn with_local_model(mut self, model: impl Into<String>) -> Brain {
        self.local_model = model.into();
        self
    }

    pub fn local_model(&self) -> &str {
        &self.local_model
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

    /// Ne renvoie jamais d'erreur : tout échec devient une phrase. Sans l'attente de
    /// réponse : voir [`Self::answer_text`].
    pub async fn handle_text(&self, text: &str) -> Reply {
        self.answer_text(text).await.reply
    }

    /// Phrase complète : commande du cerveau, raccourci, sinon IA. Toute nouvelle
    /// commande efface l'action en attente (sauf « annule » et « répète »).
    pub async fn answer_text(&self, text: &str) -> Answer {
        info!(phrase = %text, "phrase reçue");
        let answer = match match_brain_command(text) {
            Some(BrainCommand::Repeat) => return self.repeat(),
            Some(BrainCommand::Cancel) => self.cancel(),
            None => {
                self.clear_pending();
                self.understand(text).await
            }
        };
        self.remember(&answer);
        answer
    }

    async fn understand(&self, text: &str) -> Answer {
        if let Some(intent) = match_fast(text) {
            debug!(route = "fast", tool = %intent.tool);
            match self.run_or_ask(intent).await {
                Step::Answer(a) => return a,
                Step::NotFound(m) => {
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
                    return Reply::new(CONFIRM_ONLINE).into();
                }
                match self.run_or_ask(intent).await {
                    Step::Answer(a) => a,
                    Step::NotFound(m) => Reply::failed(m).into(),
                }
            }
            Ok(Understanding::Answer(a)) if a.trim().is_empty() => {
                Reply::failed(BAD_RESPONSE).into()
            }
            Ok(Understanding::Answer(a)) => Reply::new(a).into(),
            Err(e) => Reply::failed(llm_error_phrase(&e, mode)).into(),
        }
    }

    /// Comme [`Self::answer_fast`], sans l'attente de réponse.
    pub async fn handle_fast(&self, text: &str) -> Option<Reply> {
        self.answer_fast(text).await.map(|a| a.reply)
    }

    /// Raccourcis seuls, sans jamais consulter l'IA (voix non vérifiée). `None` : aucun
    /// raccourci reconnu. Une action risquée n'est jamais proposée : `None` aussi.
    pub async fn answer_fast(&self, text: &str) -> Option<Answer> {
        let answer = match match_brain_command(text) {
            Some(BrainCommand::Repeat) => return Some(self.repeat()),
            Some(BrainCommand::Cancel) => self.cancel(),
            None => {
                self.clear_pending();
                let intent = match_fast(text)?;
                if self
                    .find(&intent.tool)
                    .is_some_and(|(_, t)| t.risk == Risk::Risky)
                {
                    info!(tool = %intent.tool, "action risquée refusée : voix non vérifiée");
                    return None;
                }
                info!(phrase = %text, "phrase reçue (raccourci seul)");
                match self.run_or_ask(intent).await {
                    Step::Answer(a) => a,
                    Step::NotFound(m) => Reply::failed(m).into(),
                }
            }
        };
        self.remember(&answer);
        Some(answer)
    }

    /// Réponse à une question (`Listen { follow_up: true }`). Seul un « oui » commencé
    /// dans le délai exécute l'action en attente ; dans tous les cas, l'attente est
    /// effacée. `spoken_at` : début de la réponse.
    pub async fn answer_follow_up(&self, text: &str, spoken_at: Instant) -> Answer {
        let pending = self.pending.lock().unwrap().take();
        let reply = match (pending, match_confirmation(text)) {
            (None, Some(true)) => Reply::new(NOTHING_TO_CONFIRM),
            (None, _) => Reply::new(""),
            (Some(p), _) if spoken_at.saturating_duration_since(p.asked_at) > REPLY_WINDOW => {
                info!(tool = %p.intent.tool, "réponse trop tardive : rien n'est fait");
                Reply::new(TOO_LATE)
            }
            (Some(p), Some(true)) => {
                info!(tool = %p.intent.tool, "action confirmée");
                match self.find(&p.intent.tool) {
                    Some((skill, _)) => match self.execute(skill.as_ref(), p.intent).await {
                        Step::Answer(a) => a.reply,
                        Step::NotFound(m) => Reply::failed(m),
                    },
                    None => Reply::failed(UNKNOWN),
                }
            }
            (Some(p), Some(false)) => {
                info!(tool = %p.intent.tool, "action annulée");
                Reply::new(CANCELLED)
            }
            (Some(p), None) => {
                info!(tool = %p.intent.tool, "ni oui ni non : rien n'est fait");
                Reply::new(NOT_DONE)
            }
        };
        let answer = Answer::from(reply);
        self.remember(&answer);
        answer
    }

    fn cancel(&self) -> Answer {
        if self.clear_pending() {
            info!("action en attente annulée");
            Reply::new(CANCELLED).into()
        } else {
            Reply::new(NOTHING_TO_CANCEL).into()
        }
    }

    /// Redit la question en attente (le délai repart), sinon la dernière phrase.
    /// Rien n'est réexécuté.
    fn repeat(&self) -> Answer {
        let now = self.clock.now();
        {
            let mut pending = self.pending.lock().unwrap();
            match pending.as_mut() {
                Some(p) if now.saturating_duration_since(p.asked_at) <= REPLY_WINDOW => {
                    p.asked_at = now;
                    return Answer {
                        reply: Reply::new(p.question.clone()),
                        awaits_reply: true,
                    };
                }
                Some(_) => *pending = None,
                None => {}
            }
        }
        match self.last_speech.lock().unwrap().clone() {
            Some(s) => Reply::new(s).into(),
            None => Reply::new(NOTHING_SAID).into(),
        }
    }

    fn remember(&self, answer: &Answer) {
        let speech = answer.reply.speech.trim();
        if !speech.is_empty() {
            *self.last_speech.lock().unwrap() = Some(speech.to_string());
        }
    }

    /// Compétence et description de l'outil `tool`.
    fn find(&self, tool: &str) -> Option<(Arc<dyn Skill>, ToolSpec)> {
        let system: Arc<dyn Skill> = self.system.clone();
        self.skills
            .iter()
            .cloned()
            .chain(std::iter::once(system))
            .find_map(|s| {
                let spec = s.tools().into_iter().find(|t| t.name == tool)?;
                Some((s, spec))
            })
    }

    /// Exécute l'intention, ou la met en attente d'un « oui » si l'outil est risqué.
    async fn run_or_ask(&self, intent: Intent) -> Step {
        let Some((skill, spec)) = self.find(&intent.tool) else {
            return Step::Answer(Reply::failed(UNKNOWN).into());
        };
        if spec.risk == Risk::Risky {
            let question = skill
                .confirmation(&intent.tool, &intent.args)
                .unwrap_or_else(|| format!("{} Je lance ?", spec.description.trim()));
            info!(tool = %intent.tool, "action risquée : confirmation demandée");
            *self.pending.lock().unwrap() = Some(Pending {
                intent,
                question: question.clone(),
                asked_at: self.clock.now(),
            });
            return Step::Answer(Answer {
                reply: Reply::new(question),
                awaits_reply: true,
            });
        }
        self.execute(skill.as_ref(), intent).await
    }

    async fn execute(&self, skill: &dyn Skill, intent: Intent) -> Step {
        let call = skill.call(&intent.tool, intent.args);
        let result = match tokio::time::timeout(self.skill_timeout, call).await {
            Ok(r) => r,
            Err(_) => {
                warn!(tool = %intent.tool, "délai de la compétence dépassé");
                return Step::Answer(Reply::failed(TOO_SLOW).into());
            }
        };
        let reply = match result {
            Ok(reply) => reply,
            Err(SkillError::NotFound(m)) => return Step::NotFound(m),
            Err(SkillError::UnknownTool(_) | SkillError::BadArgs(_)) => Reply::failed(UNKNOWN),
            Err(SkillError::Failed(detail)) => {
                Reply::failed(format!("Ça n'a pas marché : {detail}."))
            }
        };
        Step::Answer(reply.into())
    }
}

const SKILL_TIMEOUT: Duration = Duration::from_secs(5);
/// Délai pour commencer à répondre à une question : les 8 s de l'oreille après la
/// question, plus le temps de la synthétiser et de la dire.
pub const REPLY_WINDOW: Duration = Duration::from_secs(8 + 4);
const UNKNOWN: &str = "Je ne sais pas faire ça.";
const BAD_RESPONSE: &str = "Je n'ai pas compris la réponse de l'IA.";
const TOO_SLOW: &str = "Ça n'a pas marché : la commande a pris trop de temps.";
const CONFIRM_ONLINE: &str = "Dites « passe en mode en ligne » pour confirmer.";
const CANCELLED: &str = "D'accord, j'annule.";
const NOTHING_TO_CANCEL: &str = "Il n'y a rien à annuler.";
const NOTHING_TO_CONFIRM: &str = "Il n'y a rien à confirmer.";
const NOTHING_SAID: &str = "Je n'ai encore rien dit.";
const TOO_LATE: &str = "Trop tard : je ne fais rien.";
const NOT_DONE: &str = "Bien, je ne fais rien.";

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
    async fn raccourci_seul_n_appelle_jamais_l_ia() {
        let s = setup(answer("x"), answer("y"));
        assert_eq!(s.brain.handle_fast("suivant").await.unwrap().speech, "ok");
        assert!(s.brain.handle_fast("quel temps fait-il").await.is_none());
        // Cible introuvable : le message est dit, sans repli sur l'IA.
        let skill = FakeSkill::new(&["media_next"])
            .with_result(Err(SkillError::NotFound("Aucun lecteur.".into())));
        let brain = Brain::new(
            vec![Arc::new(skill)],
            s.local.clone(),
            s.online.clone(),
            Arc::new(ModeStore::new(s.path.clone(), Mode::Local)),
        );
        assert_eq!(
            brain.handle_fast("suivant").await.unwrap().speech,
            "Aucun lecteur."
        );
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

    // --- Confirmations, « annule », « répète » ---

    use crate::testing::ManualClock;
    use jarvis_skills::power::PowerSkill;
    use jarvis_skills::{FakeRunner, Outcome};

    struct Power {
        brain: Brain,
        runner: Arc<FakeRunner>,
        clock: Arc<ManualClock>,
        skill: Arc<FakeSkill>,
        _dir: tempfile::TempDir,
    }

    fn power_with(llm: FakeLlm) -> Power {
        let dir = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::new());
        let clock = Arc::new(ManualClock::new());
        let skill = Arc::new(FakeSkill::new(&["media_next"]).with_result(Ok(Reply::done("Bien."))));
        let llm = Arc::new(llm);
        let brain = Brain::new(
            vec![skill.clone(), Arc::new(PowerSkill::new(runner.clone()))],
            llm.clone(),
            llm,
            Arc::new(ModeStore::new(dir.path().join("mode"), Mode::Local)),
        )
        .with_clock(clock.clone());
        Power {
            brain,
            runner,
            clock,
            skill,
            _dir: dir,
        }
    }

    fn power() -> Power {
        power_with(answer("x"))
    }

    fn suspend_call() -> Vec<(String, Vec<String>)> {
        vec![("systemctl".to_string(), vec!["suspend".to_string()])]
    }

    impl Power {
        async fn ask(&self) {
            let a = self.brain.answer_text("mets le PC en veille").await;
            assert_eq!(a.reply.speech, "Je mets le PC en veille ?");
            assert!(a.awaits_reply);
        }

        async fn reply(&self, text: &str) -> Answer {
            self.brain.answer_follow_up(text, self.brain.now()).await
        }
    }

    #[tokio::test]
    async fn action_risquee_attend_confirmation() {
        let p = power();
        p.ask().await;
        assert!(p.runner.calls().is_empty());
        assert!(p.brain.has_pending());
        // Par l'IA aussi.
        let p = power_with(FakeLlm::new(Ok(tool_understanding("shutdown"))));
        let a = p.brain.answer_text("coupe tout, j'ai fini").await;
        assert_eq!(a.reply.speech, "J'éteins le PC ?");
        assert!(a.awaits_reply);
        assert!(p.runner.calls().is_empty());
    }

    #[tokio::test]
    async fn oui_execute() {
        let p = power();
        p.ask().await;
        p.clock.advance(Duration::from_secs(5));
        let a = p.reply("Oui, vas-y.").await;
        assert_eq!(a.reply.outcome, Outcome::Done);
        assert!(!a.awaits_reply);
        assert_eq!(p.runner.calls(), suspend_call());
        assert!(!p.brain.has_pending());
        // Une seule fois : un second « oui » ne refait rien.
        let a = p.reply("oui").await;
        assert_eq!(a.reply.speech, "Il n'y a rien à confirmer.");
        assert_eq!(p.runner.calls().len(), 1);
    }

    #[tokio::test]
    async fn non_annule() {
        let p = power();
        p.ask().await;
        let a = p.reply("non").await;
        assert_eq!(a.reply, Reply::new("D'accord, j'annule."));
        assert!(p.runner.calls().is_empty());
        assert!(!p.brain.has_pending());
    }

    #[tokio::test]
    async fn delai_depasse_rien() {
        let p = power();
        p.ask().await;
        p.clock.advance(REPLY_WINDOW + Duration::from_millis(1));
        let a = p.reply("oui").await;
        assert_eq!(a.reply, Reply::new("Trop tard : je ne fais rien."));
        assert!(p.runner.calls().is_empty());
        assert!(!p.brain.has_pending());
    }

    #[tokio::test]
    async fn debut_de_reponse_dans_le_delai_accepte() {
        // Réponse arrivée après le délai, mais commencée avant : acceptée.
        let p = power();
        p.ask().await;
        p.clock.advance(REPLY_WINDOW + Duration::from_secs(1));
        let started = p.brain.now() - Duration::from_secs(2);
        p.brain.answer_follow_up("oui", started).await;
        assert_eq!(p.runner.calls(), suspend_call());
    }

    #[tokio::test]
    async fn autre_phrase_annule_l_attente() {
        let p = power();
        p.ask().await;
        let a = p.reply("mets pause").await;
        assert_eq!(a.reply, Reply::new("Bien, je ne fais rien."));
        assert!(!p.brain.has_pending());
        p.reply("oui").await;
        assert!(p.runner.calls().is_empty());
        assert!(p.skill.calls().is_empty());
    }

    #[tokio::test]
    async fn nouvelle_commande_efface_l_attente() {
        let p = power();
        p.ask().await;
        p.brain.answer_text("suivant").await;
        assert!(!p.brain.has_pending());
        p.reply("oui").await;
        assert!(p.runner.calls().is_empty());
        // Une question libre (IA) aussi.
        p.ask().await;
        p.brain.answer_text("quel temps fait-il").await;
        assert!(!p.brain.has_pending());
        // Et une nouvelle action risquée remplace la première.
        p.ask().await;
        p.brain.answer_text("redémarre le PC").await;
        p.reply("oui").await;
        assert_eq!(
            p.runner.calls(),
            vec![("systemctl".to_string(), vec!["reboot".to_string()])]
        );
    }

    #[tokio::test]
    async fn annule_sans_attente() {
        let p = power();
        let a = p.brain.answer_text("annule").await;
        assert_eq!(a.reply, Reply::new("Il n'y a rien à annuler."));
        p.ask().await;
        let a = p.brain.answer_text("Jarvis, annule.").await;
        assert_eq!(a.reply, Reply::new("D'accord, j'annule."));
        assert!(!p.brain.has_pending());
        p.reply("oui").await;
        assert!(p.runner.calls().is_empty());
    }

    #[tokio::test]
    async fn repete_la_derniere_reponse() {
        let p = power();
        let a = p.brain.answer_text("répète").await;
        assert_eq!(a.reply, Reply::new("Je n'ai encore rien dit."));
        p.brain.answer_text("suivant").await;
        let a = p.brain.answer_text("tu peux répéter").await;
        // Redit sans réexécuter, et sans bip de réussite.
        assert_eq!(a.reply, Reply::new("Bien."));
        assert_eq!(p.skill.calls().len(), 1);
        // Question en attente : elle est reposée, le délai repart.
        p.ask().await;
        p.clock.advance(Duration::from_secs(10));
        let a = p.brain.answer_text("répète").await;
        assert_eq!(a.reply.speech, "Je mets le PC en veille ?");
        assert!(a.awaits_reply);
        p.clock.advance(Duration::from_secs(10));
        p.reply("oui").await;
        assert_eq!(p.runner.calls(), suspend_call());
    }

    #[tokio::test]
    async fn voix_non_verifiee_sans_action_risquee() {
        let p = power();
        assert!(p.brain.answer_fast("mets le PC en veille").await.is_none());
        assert!(!p.brain.has_pending());
        // « annule » et « répète » restent permis.
        assert!(p.brain.answer_fast("annule").await.is_some());
        assert!(p.brain.answer_fast("répète").await.is_some());
    }

    #[tokio::test]
    async fn echecs_du_cerveau_avec_bip_d_echec() {
        let failed = |speech: &str| (speech.to_string(), Outcome::Failed);
        let got = |r: Reply| (r.speech, r.outcome);

        let s = setup(
            FakeLlm::new(Ok(tool_understanding("lance_fusee"))),
            answer("y"),
        );
        assert_eq!(
            got(s.brain.handle_text("lance une fusée").await),
            failed("Je ne sais pas faire ça.")
        );
        let s = setup(
            FakeLlm::new(Err(LlmError::Unreachable("x".into()))),
            answer("y"),
        );
        assert_eq!(
            got(s.brain.handle_text("quel temps").await),
            failed("L'IA locale ne répond pas. Ollama est-il lancé ?")
        );
        let s = setup(
            FakeLlm::new(Err(LlmError::Rejected("x".into()))),
            answer("y"),
        );
        assert_eq!(
            s.brain.handle_text("quel temps").await.outcome,
            Outcome::Failed
        );
        let skill =
            Arc::new(FakeSkill::new(&["media_next"]).with_delay(Duration::from_millis(500)));
        let (brain, _dir) = brain_with(skill, Arc::new(answer("x")));
        let brain = brain.with_skill_timeout(Duration::from_millis(30));
        assert_eq!(brain.handle_text("suivant").await.outcome, Outcome::Failed);
        let local = Arc::new(FakeLlm::new(Ok(tool_understanding("app_launch"))));
        let (brain, _dir) = brain_with(not_found_skill(), local);
        assert_eq!(
            got(brain.handle_text("fais apparaître le truc").await),
            failed("Je ne trouve pas x.")
        );
        // Une réponse de l'IA reste une information.
        let s = setup(answer("Il fait beau."), answer("y"));
        assert_eq!(
            s.brain.handle_text("quel temps").await.outcome,
            Outcome::Info
        );
    }
}
