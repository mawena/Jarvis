//! Synthèse vocale : la voix de Jarvis.

pub mod piper;

use jarvis_protocol::{ControlResponse, SpeechAudio, encode_pcm};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;
use tracing::{debug, warn};

/// Au-delà, seul le début de la réponse est dit (le texte reste entier).
pub const MAX_SPOKEN_CHARS: usize = 400;

/// Attente maximale (place libre + synthèse) : l'oreille abandonne à 30 s, et le LLM
/// et la transcription ont déjà pris leur part.
const SYNTH_DEADLINE: Duration = Duration::from_secs(8);

/// Filet de sécurité : un son plus long n'est pas envoyé.
const MAX_AUDIO: Duration = Duration::from_secs(30);

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

/// La voix de Jarvis, si elle est installée. Une seule synthèse à la fois.
#[derive(Clone)]
pub struct Voice {
    synth: Option<Arc<dyn Synthesizer>>,
    lock: Arc<Semaphore>,
    deadline: Duration,
}

impl Default for Voice {
    fn default() -> Self {
        Self {
            synth: None,
            lock: Arc::new(Semaphore::new(1)),
            deadline: SYNTH_DEADLINE,
        }
    }
}

impl Voice {
    pub fn new(synth: Arc<dyn Synthesizer>) -> Self {
        Self {
            synth: Some(synth),
            ..Self::default()
        }
    }

    /// Pas de voix : les réponses restent en texte seul.
    pub fn none() -> Self {
        Self::default()
    }

    /// Change l'attente maximale de la synthèse (tests).
    pub fn with_deadline(mut self, deadline: Duration) -> Self {
        self.deadline = deadline;
        self
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
            Some(synth) if !speech.trim().is_empty() => {
                self.synthesize(synth.clone(), &speech).await
            }
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

    /// Synthèse hors du runtime, une à la fois, bornée par `deadline`. Échec, retard
    /// ou son trop long : `None` et un avertissement (sans le texte).
    async fn synthesize(&self, synth: Arc<dyn Synthesizer>, speech: &str) -> Option<SpeechAudio> {
        let spoken = spoken_part(speech, MAX_SPOKEN_CHARS);
        if spoken.len() < speech.len() {
            debug!(
                longueur = speech.chars().count(),
                dite = spoken.chars().count(),
                "réponse abrégée pour la voix"
            );
        }
        debug!(phrase = %spoken, "synthèse");
        let text = spoken.to_string();
        let lock = self.lock.clone();
        // Le jeton part avec la tâche : un fil bloquant ne s'annule pas, donc une
        // synthèse abandonnée finit seule (résultat jeté) et garde la place jusque-là.
        let job = async move {
            let permit = lock.acquire_owned().await.ok()?;
            Some(
                tokio::task::spawn_blocking(move || {
                    let _permit = permit;
                    synth.synthesize(&text)
                })
                .await,
            )
        };
        let joined = match tokio::time::timeout(self.deadline, job).await {
            Ok(Some(joined)) => joined,
            Ok(None) => {
                warn!("file de synthèse fermée, réponse en texte seul");
                return None;
            }
            Err(_) => {
                warn!(
                    delai_ms = self.deadline.as_millis() as u64,
                    "synthèse vocale trop lente ou occupée, réponse en texte seul"
                );
                return None;
            }
        };
        match joined {
            Ok(Ok(s)) if s.samples.is_empty() => {
                warn!("synthèse vocale vide, réponse en texte seul");
                None
            }
            Ok(Ok(s)) if too_long(&s) => {
                warn!(
                    echantillons = s.samples.len(),
                    frequence = s.sample_rate,
                    "son de synthèse trop long, réponse en texte seul"
                );
                None
            }
            Ok(Ok(s)) => Some(SpeechAudio {
                pcm: encode_pcm(&s.samples),
                sample_rate: s.sample_rate,
            }),
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
}

/// Plus de `MAX_AUDIO` de son.
fn too_long(s: &Speech) -> bool {
    s.samples.len() as u128 * 1000 > MAX_AUDIO.as_millis() * u128::from(s.sample_rate)
}

/// Début de `text` d'au plus `max` caractères : coupé après la dernière fin de phrase
/// (`.`, `!`, `?`, `…` suivi d'un blanc), sinon au dernier mot entier.
fn spoken_part(text: &str, max: usize) -> &str {
    let Some((end, _)) = text.char_indices().nth(max) else {
        return text;
    };
    let head = &text[..end];
    let sentence = head.char_indices().rfind(|&(i, c)| {
        let after = i + c.len_utf8();
        matches!(c, '.' | '!' | '?' | '…')
            && text[after..].chars().next().is_none_or(char::is_whitespace)
    });
    if let Some((i, c)) = sentence {
        return &text[..i + c.len_utf8()];
    }
    let words = if text[end..].starts_with(char::is_whitespace) {
        head
    } else {
        match head.rfind(char::is_whitespace) {
            Some(i) => &head[..i],
            None => return head,
        }
    };
    words.trim_end_matches(|c: char| c.is_whitespace() || matches!(c, ',' | ';' | ':'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FakeSynthesizer;
    use std::time::Duration;

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

    fn voice_of(samples: usize, sample_rate: u32) -> FakeSynthesizer {
        FakeSynthesizer::new(Speech {
            samples: vec![1; samples],
            sample_rate,
        })
    }

    #[tokio::test]
    async fn texte_long_coupe_a_la_phrase() {
        let mut long = String::from("Voici la réponse, Monsieur. ");
        while long.chars().count() <= MAX_SPOKEN_CHARS {
            long.push_str("Une phrase de plus, assez longue, avec des accents éàù ! ");
        }
        let fake = Arc::new(voice_of(3, 22_050));
        let r = Voice::new(fake.clone()).speak(heard(&long)).await;
        let spoken = &fake.texts()[0];
        assert!(spoken.chars().count() <= MAX_SPOKEN_CHARS, "{spoken}");
        assert!(spoken.ends_with('!'), "{spoken}");
        assert!(long.starts_with(spoken.as_str()));
        // Le texte garde la réponse entière, avec le son.
        let ControlResponse::Heard { speech, audio, .. } = r else {
            unreachable!()
        };
        assert_eq!(speech, long);
        assert!(audio.is_some());
    }

    #[test]
    fn texte_court_intact_texte_sans_phrase_coupe_au_mot() {
        assert_eq!(spoken_part("Bien, Monsieur.", 400), "Bien, Monsieur.");
        assert_eq!(spoken_part("un deux trois quatre", 12), "un deux");
        assert_eq!(spoken_part("Fin. un deux trois", 12), "Fin.");
        // Un point sans espace derrière (3.5) n'est pas une fin de phrase.
        assert_eq!(spoken_part("vaut 3.5 euros hier", 12), "vaut 3.5");
        assert_eq!(spoken_part("Oui… et puis non", 8), "Oui…");
        assert_eq!(spoken_part("anticonstitutionnel", 5), "antic");
    }

    #[tokio::test]
    async fn synthese_trop_lente_texte_seul() {
        let fake = Arc::new(voice_of(3, 22_050).with_delay(Duration::from_millis(300)));
        let voice = Voice::new(fake.clone()).with_deadline(Duration::from_millis(50));
        let start = std::time::Instant::now();
        let r = voice.speak(heard("Bien, Monsieur.")).await;
        assert!(start.elapsed() < Duration::from_millis(250));
        assert_eq!(r, heard("Bien, Monsieur."));
        assert_eq!(fake.texts().len(), 1);
    }

    #[tokio::test]
    async fn syntheses_une_a_la_fois() {
        let fake = Arc::new(voice_of(3, 22_050).with_delay(Duration::from_millis(50)));
        let voice = Voice::new(fake.clone()).with_deadline(Duration::from_secs(2));
        let (a, b) = tokio::join!(voice.speak(heard("Un.")), voice.speak(heard("Deux.")));
        assert!(audio_of(&a).is_some() && audio_of(&b).is_some());
        assert_eq!(fake.max_concurrent(), 1);

        // Une synthèse abandonnée garde la place : la suivante, sans place dans le
        // délai, part en texte seul sans lancer de synthèse.
        let slow = Arc::new(voice_of(3, 22_050).with_delay(Duration::from_millis(400)));
        let voice = Voice::new(slow.clone()).with_deadline(Duration::from_millis(50));
        assert_eq!(voice.speak(heard("Un.")).await, heard("Un."));
        assert_eq!(voice.speak(heard("Deux.")).await, heard("Deux."));
        assert_eq!(slow.texts(), ["Un."]);
        assert_eq!(slow.max_concurrent(), 1);
    }

    #[tokio::test]
    async fn audio_trop_long_ignore() {
        // 3 001 échantillons à 100 Hz : un peu plus de 30 s.
        let fake = Arc::new(voice_of(3_001, 100));
        let r = Voice::new(fake.clone()).speak(heard("Bien.")).await;
        assert_eq!(r, heard("Bien."));
        // Pile 30 s : gardé.
        let fake = Arc::new(voice_of(3_000, 100));
        let r = Voice::new(fake.clone()).speak(heard("Bien.")).await;
        assert!(audio_of(&r).is_some());
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
