//! Écoute côté cerveau : mot d'éveil (baisse du son) et phrases captées.

use crate::brain::{Answer, Brain};
use crate::ducking::Ducker;
use crate::fastpath::{is_shortcut, match_ear_command};
use crate::speaker::{SpeakerGate, SpeakerState, Verdict};
use crate::stt::Transcriber;
use jarvis_protocol::{ControlResponse, EarAction, Sound};
use jarvis_skills::Outcome;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{info, warn};

/// Dit quand une empreinte existe mais que le modèle de vérification manque.
pub const VERIFICATION_UNAVAILABLE: &str = "La vérification de votre voix est indisponible.";

/// La reconnaissance vocale n'est pas disponible : le message est dit tel quel.
#[derive(Debug, thiserror::Error)]
#[error("La reconnaissance vocale n'est pas installée.")]
pub struct SttUnavailable;

/// Transcripteur provisoire, tant qu'aucun modèle n'est branché.
pub struct NoTranscriber;

#[async_trait::async_trait]
impl Transcriber for NoTranscriber {
    async fn transcribe(&self, _audio: &[f32]) -> anyhow::Result<String> {
        Err(SttUnavailable.into())
    }
}

pub struct ListenService {
    brain: Arc<Brain>,
    stt: Arc<dyn Transcriber>,
    ducker: Arc<dyn Ducker>,
    speaker: SpeakerState,
}

fn heard(text: &str, speech: &str, ear_action: Option<EarAction>) -> ControlResponse {
    ControlResponse::Heard {
        text: text.into(),
        speech: speech.into(),
        ear_action,
        audio: None,
        sound: None,
    }
}

/// Réponse du cerveau : bip selon l'issue, écoute de la réponse si une question est posée.
fn answered(text: &str, answer: Answer) -> ControlResponse {
    ControlResponse::Heard {
        text: text.into(),
        speech: answer.reply.speech,
        ear_action: answer.awaits_reply.then_some(EarAction::AwaitReply),
        audio: None,
        sound: match answer.reply.outcome {
            Outcome::Done => Some(Sound::Success),
            Outcome::Failed => Some(Sound::Failure),
            Outcome::Info => None,
        },
    }
}

fn awaits_reply(r: &ControlResponse) -> bool {
    matches!(
        r,
        ControlResponse::Heard {
            ear_action: Some(EarAction::AwaitReply),
            ..
        }
    )
}

/// Durée d'un audio à 16 kHz.
fn duration(audio: &[i16]) -> Duration {
    Duration::from_secs_f64(audio.len() as f64 / 16_000.0)
}

impl ListenService {
    pub fn new(brain: Arc<Brain>, stt: Arc<dyn Transcriber>, ducker: Arc<dyn Ducker>) -> Self {
        Self {
            brain,
            stt,
            ducker,
            speaker: SpeakerState::Disabled,
        }
    }

    /// Active la vérification du locuteur avant toute transcription.
    pub fn with_speaker(self, speaker: Option<Arc<SpeakerGate>>) -> Self {
        self.with_speaker_state(speaker.map_or(SpeakerState::Disabled, SpeakerState::Ready))
    }

    /// Comme [`Self::with_speaker`], en tenant compte d'un modèle indisponible.
    pub fn with_speaker_state(mut self, speaker: SpeakerState) -> Self {
        self.speaker = speaker;
        self
    }

    /// « Jarvis » entendu : baisse le son.
    pub async fn wake(&self) {
        self.ducker.duck().await;
    }

    /// Phrase captée (i16 16 kHz mono). Rétablit toujours le son d'abord.
    /// `follow_up` : réponse à une question du cerveau (`EarAction::AwaitReply`).
    pub async fn listen(&self, audio: &[i16], muted: bool, follow_up: bool) -> ControlResponse {
        let arrived = self.brain.now();
        let response = self.hear(audio, muted, follow_up, arrived).await;
        // L'action en attente ne vit que jusqu'à la phrase suivante, quelle qu'elle soit
        // (silence, voix inconnue, autre commande…), sauf si cette phrase repose une question.
        if !awaits_reply(&response) {
            self.brain.clear_pending();
        }
        response
    }

    async fn hear(
        &self,
        audio: &[i16],
        muted: bool,
        follow_up: bool,
        arrived: Instant,
    ) -> ControlResponse {
        self.ducker.restore().await;
        if audio.is_empty() {
            return heard("", "", None);
        }
        let samples: Vec<f32> = audio.iter().map(|&s| f32::from(s) / 32768.0).collect();
        if let SpeakerState::Unavailable { enrolled: true } = self.speaker {
            warn!("vérification du locuteur indisponible (modèle absent) : phrase refusée");
            let speech = if muted { "" } else { VERIFICATION_UNAVAILABLE };
            return heard("", speech, None);
        }
        // `Unavailable { enrolled: false }` : jamais enrôlé, tout est accepté (voir
        // `SpeakerState::unavailable`).
        // Phrase courte acceptée par la session, sans vraie vérification : raccourcis seuls.
        let mut unverified = false;
        if let SpeakerState::Ready(gate) = &self.speaker {
            let (g, audio) = (gate.clone(), samples.clone());
            // L'empreinte prend des dizaines de ms : hors du runtime.
            let verdict = tokio::task::spawn_blocking(move || g.check(&audio))
                .await
                .unwrap_or(Verdict::NoMatch(0.0));
            match verdict {
                Verdict::Match(_) | Verdict::NotEnrolled => {}
                Verdict::SessionShort => unverified = true,
                Verdict::NoMatch(score) => {
                    info!("voix non reconnue (score {score:.2})");
                    return heard("", "", None);
                }
                Verdict::TooShort => {
                    info!("voix non reconnue (phrase trop courte)");
                    return heard("", "", None);
                }
            }
        }
        let text = match self.stt.transcribe(&samples).await {
            Ok(t) => t.trim().to_string(),
            Err(e) => {
                warn!(erreur = %e, "transcription impossible");
                let speech = match e.downcast_ref::<SttUnavailable>() {
                    Some(u) => u.to_string(),
                    None => "Je n'ai pas compris.".into(),
                };
                return heard("", &speech, None);
            }
        };
        if text.is_empty() {
            return heard("", "", None);
        }
        let action = match_ear_command(&text);
        if muted {
            // Écoute coupée : seule la réactivation est retenue, le reste n'est pas journalisé.
            return match action {
                Some(EarAction::Unmute) => {
                    info!(phrase = %text, "écoute réactivée");
                    heard(&text, "Écoute réactivée.", Some(EarAction::Unmute))
                }
                _ => heard("", "", None),
            };
        }
        if follow_up && action.is_none() {
            // Réponse à une question : seulement oui / non, jamais une nouvelle commande.
            // Une phrase courte de session (« oui » dure moins d'une seconde) est
            // acceptée ici : la question vient d'une voix reconnue juste avant.
            info!(phrase = %text, "réponse entendue");
            let spoken_at = arrived.checked_sub(duration(audio)).unwrap_or(arrived);
            return answered(&text, self.brain.answer_follow_up(&text, spoken_at).await);
        }
        if unverified && action.is_none() && !is_shortcut(&text) {
            // Jamais l'IA pour une voix non vérifiée : ignorée comme une voix inconnue.
            info!("phrase courte non vérifiée, ignorée");
            return heard("", "", None);
        }
        info!(phrase = %text, "texte entendu");
        match action {
            Some(EarAction::Mute) => heard(&text, "Écoute coupée.", Some(EarAction::Mute)),
            Some(EarAction::Unmute) => heard(&text, "L'écoute est déjà active.", None),
            // `AwaitReply` ne vient jamais d'une commande d'écoute.
            Some(EarAction::AwaitReply) | None if unverified => {
                match self.brain.answer_fast(&text).await {
                    Some(a) => answered(&text, a),
                    // Action risquée : jamais pour une voix non vérifiée.
                    None => heard("", "", None),
                }
            }
            Some(EarAction::AwaitReply) | None => {
                answered(&text, self.brain.answer_text(&text).await)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::Understanding;
    use crate::mode_store::ModeStore;
    use crate::testing::{FakeDucker, FakeLlm, FakeSkill, FakeTranscriber};
    use jarvis_protocol::Mode;

    struct Setup {
        service: ListenService,
        skill: Arc<FakeSkill>,
        ducker: Arc<FakeDucker>,
        stt: Arc<FakeTranscriber>,
        /// IA locale et en ligne (la même doublure).
        llm: Arc<FakeLlm>,
        _dir: tempfile::TempDir,
    }

    fn llm() -> Arc<FakeLlm> {
        Arc::new(FakeLlm::new(Ok(Understanding::Answer("x".into()))))
    }

    fn brain_with(dir: &tempfile::TempDir, skill: Arc<FakeSkill>, llm: Arc<FakeLlm>) -> Arc<Brain> {
        Arc::new(Brain::new(
            vec![skill],
            llm.clone(),
            llm,
            Arc::new(ModeStore::new(dir.path().join("mode"), Mode::Local)),
        ))
    }

    fn brain(dir: &tempfile::TempDir, skill: Arc<FakeSkill>) -> Arc<Brain> {
        brain_with(dir, skill, llm())
    }

    fn setup(stt: FakeTranscriber) -> Setup {
        let dir = tempfile::tempdir().unwrap();
        let skill = Arc::new(FakeSkill::new(&["media_next"]));
        let ducker = Arc::new(FakeDucker::new());
        let stt = Arc::new(stt);
        let llm = llm();
        Setup {
            service: ListenService::new(
                brain_with(&dir, skill.clone(), llm.clone()),
                stt.clone(),
                ducker.clone(),
            ),
            skill,
            ducker,
            stt,
            llm,
            _dir: dir,
        }
    }

    const AUDIO: &[i16] = &[1, 2, 3];

    /// Voix de Charles : premier échantillon bas ; voix inconnue : premier échantillon haut.
    struct ByFirstSample;

    impl crate::speaker::SpeakerVerifier for ByFirstSample {
        fn embed(&self, audio: &[f32]) -> anyhow::Result<Vec<f32>> {
            Ok(if audio[0] < 0.5 {
                vec![1.0, 0.0]
            } else {
                vec![0.0, 1.0]
            })
        }
    }

    fn voice(first: i16) -> Vec<i16> {
        let mut a = vec![0i16; 3 * 16_000];
        a[0] = first;
        a
    }

    fn with_gate(enrolled: bool) -> Setup {
        with_gate_text(enrolled, "suivant")
    }

    fn with_gate_text(enrolled: bool, text: &str) -> Setup {
        let mut s = setup(FakeTranscriber::new(text));
        let path = s._dir.path().join("voiceprint/charles.json");
        let gate = SpeakerGate::new(Arc::new(ByFirstSample), path, 0.55, 0.65);
        if enrolled {
            gate.enroll(&[vec![0.0; 16_000]]).unwrap();
        }
        s.service = s.service.with_speaker(Some(Arc::new(gate)));
        s
    }

    #[tokio::test]
    async fn voix_inconnue_ignoree_sans_transcription() {
        let s = with_gate(true);
        let r = s.service.listen(&voice(20_000), false, false).await;
        assert_eq!(r, heard("", "", None));
        assert_eq!(s.stt.calls(), 0);
        assert!(s.skill.calls().is_empty());
        // Écoute coupée : même vérification.
        let r = s.service.listen(&voice(20_000), true, false).await;
        assert_eq!(r, heard("", "", None));
        assert_eq!(s.stt.calls(), 0);
    }

    #[tokio::test]
    async fn voix_reconnue_executee() {
        let s = with_gate(true);
        let r = s.service.listen(&voice(0), false, false).await;
        assert_eq!(r, heard("suivant", "ok", None));
        assert_eq!(s.stt.calls(), 1);
    }

    /// Phrase de moins d'une seconde, voix inconnue : acceptée seulement par la session.
    fn short_voice() -> Vec<i16> {
        let mut a = vec![0i16; 8_000];
        a[0] = 20_000;
        a
    }

    #[tokio::test]
    async fn phrase_courte_de_session_suivant_executee() {
        let s = with_gate(true);
        s.service.listen(&voice(0), false, false).await;
        assert_eq!(s.skill.calls().len(), 1);
        let r = s.service.listen(&short_voice(), false, false).await;
        assert_eq!(r, heard("suivant", "ok", None));
        assert_eq!(s.skill.calls().len(), 2);
        assert_eq!(s.llm.calls(), 0);
    }

    #[tokio::test]
    async fn phrase_courte_de_session_question_libre_ignoree() {
        let s = with_gate_text(true, "quel temps fait-il");
        s.service.listen(&voice(0), false, false).await;
        assert_eq!(s.llm.calls(), 1);
        let r = s.service.listen(&short_voice(), false, false).await;
        assert_eq!(r, heard("", "", None));
        assert_eq!(s.llm.calls(), 1);
    }

    #[tokio::test]
    async fn phrase_courte_de_session_coupe_l_ecoute() {
        let s = with_gate_text(true, "Jarvis, coupe l'écoute.");
        s.service.listen(&voice(0), false, false).await;
        let r = s.service.listen(&short_voice(), false, false).await;
        assert_eq!(
            r,
            heard(
                "Jarvis, coupe l'écoute.",
                "Écoute coupée.",
                Some(EarAction::Mute)
            )
        );
    }

    #[tokio::test]
    async fn sans_empreinte_executee() {
        let s = with_gate(false);
        let r = s.service.listen(&voice(20_000), false, false).await;
        assert_eq!(r, heard("suivant", "ok", None));
    }

    #[tokio::test]
    async fn modele_indisponible_avec_empreinte_refuse() {
        let mut s = setup(FakeTranscriber::new("suivant"));
        s.service = s
            .service
            .with_speaker_state(SpeakerState::Unavailable { enrolled: true });
        let r = s.service.listen(&voice(0), false, false).await;
        assert_eq!(r, heard("", VERIFICATION_UNAVAILABLE, None));
        assert_eq!(s.stt.calls(), 0);
        assert!(s.skill.calls().is_empty());
        // Écoute coupée : refusée sans rien dire.
        let r = s.service.listen(&voice(0), true, false).await;
        assert_eq!(r, heard("", "", None));
        assert_eq!(s.stt.calls(), 0);
    }

    #[tokio::test]
    async fn modele_indisponible_sans_empreinte_executee() {
        let mut s = setup(FakeTranscriber::new("suivant"));
        s.service = s
            .service
            .with_speaker_state(SpeakerState::Unavailable { enrolled: false });
        let r = s.service.listen(&voice(20_000), false, false).await;
        assert_eq!(r, heard("suivant", "ok", None));
    }

    #[tokio::test]
    async fn voix_inconnue_restaure_la_musique() {
        let s = with_gate(true);
        s.service.listen(&voice(20_000), false, false).await;
        assert_eq!(s.ducker.events(), ["restore"]);
    }

    #[tokio::test]
    async fn listen_transcrit_puis_agit() {
        let s = setup(FakeTranscriber::new("suivant"));
        let r = s.service.listen(AUDIO, false, false).await;
        assert_eq!(r, heard("suivant", "ok", None));
        assert_eq!(s.skill.calls()[0].0, "media_next");
        assert_eq!(s.ducker.events(), ["restore"]);
    }

    #[tokio::test]
    async fn listen_vide_retablit_volume() {
        let s = setup(FakeTranscriber::new("suivant"));
        let r = s.service.listen(&[], false, false).await;
        assert_eq!(r, heard("", "", None));
        assert_eq!(s.ducker.events(), ["restore"]);
        assert_eq!(s.stt.calls(), 0);
    }

    #[tokio::test]
    async fn transcription_vide_ou_en_echec() {
        let s = setup(FakeTranscriber::new("  "));
        assert_eq!(
            s.service.listen(AUDIO, false, false).await,
            heard("", "", None)
        );
        let s = setup(FakeTranscriber::failing());
        assert_eq!(
            s.service.listen(AUDIO, false, false).await,
            heard("", "Je n'ai pas compris.", None)
        );
        let dir = tempfile::tempdir().unwrap();
        let svc = ListenService::new(
            brain(&dir, Arc::new(FakeSkill::new(&[]))),
            Arc::new(NoTranscriber),
            Arc::new(FakeDucker::new()),
        );
        assert_eq!(
            svc.listen(AUDIO, false, false).await,
            heard("", "La reconnaissance vocale n'est pas installée.", None)
        );
    }

    #[tokio::test]
    async fn wake_baisse_le_son() {
        let s = setup(FakeTranscriber::new("x"));
        s.service.wake().await;
        assert_eq!(s.ducker.events(), ["duck"]);
    }

    #[tokio::test]
    async fn ecoute_coupee_ignore_le_reste() {
        let s = setup(FakeTranscriber::new("suivant"));
        let r = s.service.listen(AUDIO, true, false).await;
        assert_eq!(r, heard("", "", None));
        assert!(s.skill.calls().is_empty());
        assert_eq!(s.ducker.events(), ["restore"]);
    }

    #[tokio::test]
    async fn ecoute_coupee_ignore_aussi_coupe_l_ecoute() {
        let s = setup(FakeTranscriber::new("Jarvis, coupe l'écoute."));
        let r = s.service.listen(AUDIO, true, false).await;
        assert_eq!(r, heard("", "", None));
        assert!(s.skill.calls().is_empty());
    }

    #[tokio::test]
    async fn ecoute_coupee_accepte_reactivation() {
        let s = setup(FakeTranscriber::new("Jarvis, réactive l'écoute."));
        let r = s.service.listen(AUDIO, true, false).await;
        assert_eq!(
            r,
            heard(
                "Jarvis, réactive l'écoute.",
                "Écoute réactivée.",
                Some(EarAction::Unmute)
            )
        );
    }

    #[tokio::test]
    async fn coupe_l_ecoute_renvoie_mute() {
        let s = setup(FakeTranscriber::new("Jarvis, coupe l'écoute."));
        let r = s.service.listen(AUDIO, false, false).await;
        assert_eq!(
            r,
            heard(
                "Jarvis, coupe l'écoute.",
                "Écoute coupée.",
                Some(EarAction::Mute)
            )
        );
        assert!(s.skill.calls().is_empty());
    }

    // --- Confirmations et bips ---

    use jarvis_skills::power::PowerSkill;
    use jarvis_skills::{FakeRunner, Reply, SkillError};

    /// Transcripteur qui renvoie les phrases données, dans l'ordre.
    struct Script(std::sync::Mutex<std::collections::VecDeque<String>>);

    #[async_trait::async_trait]
    impl Transcriber for Script {
        async fn transcribe(&self, _audio: &[f32]) -> anyhow::Result<String> {
            Ok(self.0.lock().unwrap().pop_front().unwrap_or_default())
        }
    }

    struct Confirm {
        service: ListenService,
        brain: Arc<Brain>,
        runner: Arc<FakeRunner>,
        _dir: tempfile::TempDir,
    }

    /// Compétence d'alimentation, empreinte de Charles enregistrée.
    fn confirm(lines: &[&str]) -> Confirm {
        let dir = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::new());
        let brain = Arc::new(Brain::new(
            vec![
                Arc::new(PowerSkill::new(runner.clone())),
                Arc::new(FakeSkill::new(&["media_next"])),
            ],
            llm(),
            llm(),
            Arc::new(ModeStore::new(dir.path().join("mode"), Mode::Local)),
        ));
        let script = Script(std::sync::Mutex::new(
            lines.iter().map(|l| l.to_string()).collect(),
        ));
        let gate = SpeakerGate::new(
            Arc::new(ByFirstSample),
            dir.path().join("voiceprint/charles.json"),
            0.55,
            0.65,
        );
        gate.enroll(&[vec![0.0; 16_000]]).unwrap();
        let service =
            ListenService::new(brain.clone(), Arc::new(script), Arc::new(FakeDucker::new()))
                .with_speaker(Some(Arc::new(gate)));
        Confirm {
            service,
            brain,
            runner,
            _dir: dir,
        }
    }

    fn suspend_call() -> Vec<(String, Vec<String>)> {
        vec![("systemctl".to_string(), vec!["suspend".to_string()])]
    }

    fn question() -> ControlResponse {
        heard(
            "mets le PC en veille",
            "Je mets le PC en veille ?",
            Some(EarAction::AwaitReply),
        )
    }

    #[tokio::test]
    async fn question_puis_oui_avec_bip_de_reussite() {
        let c = confirm(&["mets le PC en veille", "oui"]);
        assert_eq!(c.service.listen(&voice(0), false, false).await, question());
        assert!(c.runner.calls().is_empty());
        let r = c.service.listen(&voice(0), false, true).await;
        assert_eq!(
            r,
            ControlResponse::Heard {
                text: "oui".into(),
                speech: "Je mets le PC en veille.".into(),
                ear_action: None,
                audio: None,
                sound: Some(Sound::Success),
            }
        );
        assert_eq!(c.runner.calls(), suspend_call());
    }

    #[tokio::test]
    async fn voix_inconnue_n_execute_pas_la_confirmation() {
        let c = confirm(&["mets le PC en veille", "oui"]);
        c.service.listen(&voice(0), false, false).await;
        let r = c.service.listen(&voice(20_000), false, true).await;
        assert_eq!(r, heard("", "", None));
        assert!(c.runner.calls().is_empty());
        // L'attente est effacée : un « oui » de Charles ensuite ne fait rien.
        assert!(!c.brain.has_pending());
        let r = c.service.listen(&voice(0), false, true).await;
        assert_eq!(r, heard("oui", "Il n'y a rien à confirmer.", None));
        assert!(c.runner.calls().is_empty());
    }

    #[tokio::test]
    async fn oui_court_accepte_en_session() {
        let c = confirm(&["mets le PC en veille", "oui"]);
        c.service.listen(&voice(0), false, false).await;
        // Moins d'une seconde, non vérifiable : accepté comme réponse, grâce à la session.
        let r = c.service.listen(&short_voice(), false, true).await;
        assert_eq!(c.runner.calls(), suspend_call());
        assert!(matches!(
            r,
            ControlResponse::Heard {
                sound: Some(Sound::Success),
                ..
            }
        ));
    }

    #[tokio::test]
    async fn oui_court_hors_reponse_n_execute_rien() {
        // Un « oui » court qui n'est pas la réponse attendue (après « Jarvis ») : ignoré.
        let c = confirm(&["mets le PC en veille", "oui", "oui"]);
        c.service.listen(&voice(0), false, false).await;
        let r = c.service.listen(&short_voice(), false, false).await;
        assert_eq!(r, heard("", "", None));
        assert!(!c.brain.has_pending());
        c.service.listen(&short_voice(), false, true).await;
        assert!(c.runner.calls().is_empty());
    }

    #[tokio::test]
    async fn phrase_courte_ne_demande_pas_d_action_risquee() {
        let c = confirm(&["suivant", "mets le PC en veille", "oui"]);
        c.service.listen(&voice(0), false, false).await;
        let r = c.service.listen(&short_voice(), false, false).await;
        assert_eq!(r, heard("", "", None));
        assert!(!c.brain.has_pending());
        c.service.listen(&voice(0), false, true).await;
        assert!(c.runner.calls().is_empty());
    }

    #[tokio::test]
    async fn suivi_vide_ou_ecoute_coupee_n_execute_rien() {
        let c = confirm(&["mets le PC en veille", "mets le PC en veille", "oui"]);
        c.service.listen(&voice(0), false, false).await;
        assert_eq!(
            c.service.listen(&[], false, true).await,
            heard("", "", None)
        );
        assert!(!c.brain.has_pending());
        c.service.listen(&voice(0), false, false).await;
        assert_eq!(
            c.service.listen(&voice(0), true, true).await,
            heard("", "", None)
        );
        assert!(!c.brain.has_pending());
        assert!(c.runner.calls().is_empty());
    }

    #[tokio::test]
    async fn repete_redemande_et_garde_l_attente() {
        let c = confirm(&["mets le PC en veille", "répète", "oui"]);
        c.service.listen(&voice(0), false, false).await;
        let r = c.service.listen(&voice(0), false, false).await;
        assert_eq!(
            r,
            heard(
                "répète",
                "Je mets le PC en veille ?",
                Some(EarAction::AwaitReply)
            )
        );
        c.service.listen(&voice(0), false, true).await;
        assert_eq!(c.runner.calls(), suspend_call());
    }

    fn with_result(result: Result<Reply, SkillError>) -> (ListenService, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let skill = Arc::new(FakeSkill::new(&["media_next"]).with_result(result));
        let svc = ListenService::new(
            brain(&dir, skill),
            Arc::new(FakeTranscriber::new("suivant")),
            Arc::new(FakeDucker::new()),
        );
        (svc, dir)
    }

    #[tokio::test]
    async fn echec_donne_le_bip_d_echec() {
        let (svc, _dir) = with_result(Err(SkillError::Failed("boum".into())));
        let r = svc.listen(AUDIO, false, false).await;
        assert_eq!(
            r,
            ControlResponse::Heard {
                text: "suivant".into(),
                speech: "Ça n'a pas marché : boum.".into(),
                ear_action: None,
                audio: None,
                sound: Some(Sound::Failure),
            }
        );
    }

    #[tokio::test]
    async fn reussite_donne_le_bip_de_reussite() {
        let (svc, _dir) = with_result(Ok(Reply::done("")));
        let r = svc.listen(AUDIO, false, false).await;
        assert_eq!(
            r,
            ControlResponse::Heard {
                text: "suivant".into(),
                speech: String::new(),
                ear_action: None,
                audio: None,
                sound: Some(Sound::Success),
            }
        );
    }
}
