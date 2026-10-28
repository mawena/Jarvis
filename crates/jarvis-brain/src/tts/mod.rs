//! Synthèse vocale : la voix de Jarvis.

pub mod piper;

use jarvis_protocol::{ControlResponse, SpeechAudio, encode_pcm};
use std::sync::Arc;
use tracing::{debug, warn};

/// Son produit par la synthèse : i16 mono à `sample_rate` Hz.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Speech {
    pub samples: Vec<i16>,
    pub sample_rate: u32,
}

/// Transforme une phrase en voix. Bloquant (dizaines à centaines de ms) : à appeler
/// dans `spawn_blocking`.
pub trait Synthesizer: Send + Sync {
    fn synthesize(&self, text: &str) -> anyhow::Result<Speech>;
}

/// La voix de Jarvis, si elle est installée.
#[derive(Clone, Default)]
pub struct Voice {
    synth: Option<Arc<dyn Synthesizer>>,
}

impl Voice {
    pub fn new(synth: Arc<dyn Synthesizer>) -> Self {
        Self { synth: Some(synth) }
    }

    /// Pas de voix : les réponses restent en texte seul.
    pub fn none() -> Self {
        Self::default()
    }

    /// Ajoute le son de `speech` à une réponse `Heard`. Voix absente ou en panne :
    /// réponse inchangée (texte seul).
    pub async fn speak(&self, response: ControlResponse) -> ControlResponse {
        let ControlResponse::Heard {
            text,
            speech,
            ear_action,
            audio: None,
            sound,
        } = response
        else {
            return response;
        };
        let audio = match &self.synth {
            Some(synth) if !speech.trim().is_empty() => synthesize(synth.clone(), &speech).await,
            _ => None,
        };
        ControlResponse::Heard {
            text,
            speech,
            ear_action,
            audio,
            sound,
        }
    }
}

/// Synthèse hors du runtime. Échec : `None` et un avertissement (sans le texte).
async fn synthesize(synth: Arc<dyn Synthesizer>, speech: &str) -> Option<SpeechAudio> {
    debug!(phrase = %speech, "synthèse");
    let text = speech.to_string();
    match tokio::task::spawn_blocking(move || synth.synthesize(&text)).await {
        Ok(Ok(s)) if !s.samples.is_empty() => Some(SpeechAudio {
            pcm: encode_pcm(&s.samples),
            sample_rate: s.sample_rate,
        }),
        Ok(Ok(_)) => {
            warn!("synthèse vocale vide, réponse en texte seul");
            None
        }
        Ok(Err(e)) => {
            warn!(erreur = %e, "synthèse vocale impossible, réponse en texte seul");
            None
        }
        Err(e) => {
            warn!(erreur = %e, "synthèse vocale interrompue, réponse en texte seul");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FakeSynthesizer;

    fn heard(speech: &str) -> ControlResponse {
        ControlResponse::Heard {
            text: "suivant".into(),
            speech: speech.into(),
            ear_action: None,
            audio: None,
            sound: None,
        }
    }

    fn audio_of(r: &ControlResponse) -> Option<SpeechAudio> {
        match r {
            ControlResponse::Heard { audio, .. } => audio.clone(),
            _ => panic!("pas une réponse Heard : {r:?}"),
        }
    }

    #[tokio::test]
    async fn synthese_remplit_l_audio() {
        let fake = Arc::new(FakeSynthesizer::new(Speech {
            samples: vec![1, -2, 3],
            sample_rate: 22_050,
        }));
        let r = Voice::new(fake.clone())
            .speak(heard("Bien, Monsieur."))
            .await;
        assert_eq!(
            audio_of(&r),
            Some(SpeechAudio {
                pcm: encode_pcm(&[1, -2, 3]),
                sample_rate: 22_050,
            })
        );
        assert_eq!(fake.texts(), ["Bien, Monsieur."]);
        // Le reste de la réponse ne change pas.
        let ControlResponse::Heard { text, speech, .. } = r else {
            unreachable!()
        };
        assert_eq!(
            (text.as_str(), speech.as_str()),
            ("suivant", "Bien, Monsieur.")
        );
    }

    #[tokio::test]
    async fn synthese_en_echec_reponse_sans_audio() {
        let fake = Arc::new(FakeSynthesizer::failing());
        let r = Voice::new(fake.clone())
            .speak(heard("Bien, Monsieur."))
            .await;
        assert_eq!(r, heard("Bien, Monsieur."));
        assert_eq!(fake.texts().len(), 1);
    }

    #[tokio::test]
    async fn phrase_vide_sans_synthese() {
        let fake = Arc::new(FakeSynthesizer::new(Speech {
            samples: vec![1],
            sample_rate: 22_050,
        }));
        let voice = Voice::new(fake.clone());
        assert_eq!(voice.speak(heard("")).await, heard(""));
        assert_eq!(voice.speak(heard("  ")).await, heard("  "));
        assert!(fake.texts().is_empty());
    }

    #[tokio::test]
    async fn sans_voix_texte_seul() {
        assert_eq!(Voice::none().speak(heard("Bien.")).await, heard("Bien."));
        let error = ControlResponse::Error {
            message: "x".into(),
        };
        assert_eq!(Voice::none().speak(error.clone()).await, error);
    }
}
