//! Écoute côté cerveau : mot d'éveil (baisse du son) et phrases captées.

use crate::brain::Brain;
use crate::ducking::Ducker;
use crate::fastpath::match_ear_command;
use crate::speaker::{SpeakerGate, SpeakerState, Verdict};
use crate::stt::Transcriber;
use jarvis_protocol::{ControlResponse, EarAction};
use std::sync::Arc;
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
    }
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
    pub async fn listen(&self, audio: &[i16], muted: bool) -> ControlResponse {
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
        if let SpeakerState::Ready(gate) = &self.speaker {
            let (g, audio) = (gate.clone(), samples.clone());
            // L'empreinte prend des dizaines de ms : hors du runtime.
            let verdict = tokio::task::spawn_blocking(move || g.check(&audio))
                .await
                .unwrap_or(Verdict::NoMatch(0.0));
            match verdict {
                // `Match(1.0)` est la valeur sentinelle d'une phrase courte acceptée par la
                // session : ce n'est pas un vrai score, on ne le journalise pas.
                Verdict::Match(_) | Verdict::NotEnrolled => {}
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
        info!(phrase = %text, "texte entendu");
        match action {
            Some(EarAction::Mute) => heard(&text, "Écoute coupée.", Some(EarAction::Mute)),
            Some(EarAction::Unmute) => heard(&text, "L'écoute est déjà active.", None),
            None => {
                let speech = self.brain.handle_text(&text).await.speech;
                heard(&text, &speech, None)
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
        _dir: tempfile::TempDir,
    }

    fn brain(dir: &tempfile::TempDir, skill: Arc<FakeSkill>) -> Arc<Brain> {
        let llm = || Arc::new(FakeLlm::new(Ok(Understanding::Answer("x".into()))));
        Arc::new(Brain::new(
            vec![skill],
            llm(),
            llm(),
            Arc::new(ModeStore::new(dir.path().join("mode"), Mode::Local)),
        ))
    }

    fn setup(stt: FakeTranscriber) -> Setup {
        let dir = tempfile::tempdir().unwrap();
        let skill = Arc::new(FakeSkill::new(&["media_next"]));
        let ducker = Arc::new(FakeDucker::new());
        let stt = Arc::new(stt);
        Setup {
            service: ListenService::new(brain(&dir, skill.clone()), stt.clone(), ducker.clone()),
            skill,
            ducker,
            stt,
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
        let mut s = setup(FakeTranscriber::new("suivant"));
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
        let r = s.service.listen(&voice(20_000), false).await;
        assert_eq!(r, heard("", "", None));
        assert_eq!(s.stt.calls(), 0);
        assert!(s.skill.calls().is_empty());
        // Écoute coupée : même vérification.
        let r = s.service.listen(&voice(20_000), true).await;
        assert_eq!(r, heard("", "", None));
        assert_eq!(s.stt.calls(), 0);
    }

    #[tokio::test]
    async fn voix_reconnue_executee() {
        let s = with_gate(true);
        let r = s.service.listen(&voice(0), false).await;
        assert_eq!(r, heard("suivant", "ok", None));
        assert_eq!(s.stt.calls(), 1);
    }

    #[tokio::test]
    async fn sans_empreinte_executee() {
        let s = with_gate(false);
        let r = s.service.listen(&voice(20_000), false).await;
        assert_eq!(r, heard("suivant", "ok", None));
    }

    #[tokio::test]
    async fn modele_indisponible_avec_empreinte_refuse() {
        let mut s = setup(FakeTranscriber::new("suivant"));
        s.service = s
            .service
            .with_speaker_state(SpeakerState::Unavailable { enrolled: true });
        let r = s.service.listen(&voice(0), false).await;
        assert_eq!(r, heard("", VERIFICATION_UNAVAILABLE, None));
        assert_eq!(s.stt.calls(), 0);
        assert!(s.skill.calls().is_empty());
        // Écoute coupée : refusée sans rien dire.
        let r = s.service.listen(&voice(0), true).await;
        assert_eq!(r, heard("", "", None));
        assert_eq!(s.stt.calls(), 0);
    }

    #[tokio::test]
    async fn modele_indisponible_sans_empreinte_executee() {
        let mut s = setup(FakeTranscriber::new("suivant"));
        s.service = s
            .service
            .with_speaker_state(SpeakerState::Unavailable { enrolled: false });
        let r = s.service.listen(&voice(20_000), false).await;
        assert_eq!(r, heard("suivant", "ok", None));
    }

    #[tokio::test]
    async fn voix_inconnue_restaure_la_musique() {
        let s = with_gate(true);
        s.service.listen(&voice(20_000), false).await;
        assert_eq!(s.ducker.events(), ["restore"]);
    }

    #[tokio::test]
    async fn listen_transcrit_puis_agit() {
        let s = setup(FakeTranscriber::new("suivant"));
        let r = s.service.listen(AUDIO, false).await;
        assert_eq!(r, heard("suivant", "ok", None));
        assert_eq!(s.skill.calls()[0].0, "media_next");
        assert_eq!(s.ducker.events(), ["restore"]);
    }

    #[tokio::test]
    async fn listen_vide_retablit_volume() {
        let s = setup(FakeTranscriber::new("suivant"));
        let r = s.service.listen(&[], false).await;
        assert_eq!(r, heard("", "", None));
        assert_eq!(s.ducker.events(), ["restore"]);
        assert_eq!(s.stt.calls(), 0);
    }

    #[tokio::test]
    async fn transcription_vide_ou_en_echec() {
        let s = setup(FakeTranscriber::new("  "));
        assert_eq!(s.service.listen(AUDIO, false).await, heard("", "", None));
        let s = setup(FakeTranscriber::failing());
        assert_eq!(
            s.service.listen(AUDIO, false).await,
            heard("", "Je n'ai pas compris.", None)
        );
        let dir = tempfile::tempdir().unwrap();
        let svc = ListenService::new(
            brain(&dir, Arc::new(FakeSkill::new(&[]))),
            Arc::new(NoTranscriber),
            Arc::new(FakeDucker::new()),
        );
        assert_eq!(
            svc.listen(AUDIO, false).await,
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
        let r = s.service.listen(AUDIO, true).await;
        assert_eq!(r, heard("", "", None));
        assert!(s.skill.calls().is_empty());
        assert_eq!(s.ducker.events(), ["restore"]);
    }

    #[tokio::test]
    async fn ecoute_coupee_ignore_aussi_coupe_l_ecoute() {
        let s = setup(FakeTranscriber::new("Jarvis, coupe l'écoute."));
        let r = s.service.listen(AUDIO, true).await;
        assert_eq!(r, heard("", "", None));
        assert!(s.skill.calls().is_empty());
    }

    #[tokio::test]
    async fn ecoute_coupee_accepte_reactivation() {
        let s = setup(FakeTranscriber::new("Jarvis, réactive l'écoute."));
        let r = s.service.listen(AUDIO, true).await;
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
        let r = s.service.listen(AUDIO, false).await;
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
}
