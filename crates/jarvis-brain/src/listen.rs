//! Écoute côté cerveau : mot d'éveil (baisse du son) et phrases captées.

use crate::brain::Brain;
use crate::ducking::Ducker;
use crate::fastpath::match_ear_command;
use crate::stt::Transcriber;
use jarvis_protocol::{ControlResponse, EarAction};
use std::sync::Arc;
use tracing::{info, warn};

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
        Self { brain, stt, ducker }
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
