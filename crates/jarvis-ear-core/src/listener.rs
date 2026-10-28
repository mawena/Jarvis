//! Machine d'états d'écoute : attente du mot d'éveil, puis capture de la phrase.

use std::collections::VecDeque;

use crate::{SAMPLE_RATE, Vad, WakeDetector};

const RATE: usize = SAMPLE_RATE as usize;
/// Dernière seconde conservée (contient « Jarvis »).
const RING_LEN: usize = RATE;
/// Pré-enregistrement avant le début de parole.
const PRE_ROLL: usize = RATE * 3 / 10;
/// Silence qui clôt la phrase.
const END_SILENCE: usize = RATE * 8 / 10;
/// Délai sans parole après le réveil avant abandon.
const NO_SPEECH: usize = RATE * 4;
/// Durée maximale de la phrase, mesurée depuis le réveil.
const MAX_UTTERANCE: usize = RATE * 10;

#[derive(Debug, Clone, PartialEq)]
pub enum ListenEvent {
    /// Mot d'éveil détecté.
    Wake { score: f32 },
    /// Phrase complète (commence par la seconde précédant le réveil).
    Utterance(Vec<f32>),
    /// Aucune parole après le réveil.
    NoSpeech,
}

enum State {
    Idle,
    Capturing {
        /// Audio après le réveil (trames VAD entières).
        post: Vec<f32>,
        /// Début de la parole dans `post`.
        speech_start: Option<usize>,
        /// Silence consécutif depuis la dernière parole (échantillons).
        silence: usize,
    },
}

pub struct Listener<W: WakeDetector, V: Vad> {
    wake: W,
    vad: V,
    state: State,
    /// Dernière seconde entendue (alimentée en attente).
    ring: VecDeque<f32>,
    /// Seconde figée au moment du réveil.
    head: Vec<f32>,
    wake_buf: Vec<f32>,
    vad_buf: Vec<f32>,
}

impl<W: WakeDetector, V: Vad> Listener<W, V> {
    pub fn new(wake: W, vad: V) -> Self {
        Self {
            wake,
            vad,
            state: State::Idle,
            ring: VecDeque::with_capacity(RING_LEN + 1),
            head: Vec::new(),
            wake_buf: Vec::new(),
            vad_buf: Vec::new(),
        }
    }

    /// Consomme du mono 16 kHz (taille quelconque) et renvoie les événements produits.
    pub fn push(&mut self, mono16k: &[f32]) -> Vec<ListenEvent> {
        let mut events = Vec::new();
        let mut input = mono16k;
        while !input.is_empty() {
            if matches!(self.state, State::Idle) {
                let len = self.wake.frame_len().max(1);
                let take = (len - self.wake_buf.len()).min(input.len());
                self.wake_buf.extend_from_slice(&input[..take]);
                input = &input[take..];
                if self.wake_buf.len() < len {
                    continue;
                }
                let frame = std::mem::take(&mut self.wake_buf);
                self.ring.extend(frame.iter().copied());
                let excess = self.ring.len().saturating_sub(RING_LEN);
                self.ring.drain(..excess);
                if let Some(score) = self.wake.process(&frame) {
                    self.head = self.ring.iter().copied().collect();
                    self.ring.clear();
                    self.vad_buf.clear();
                    self.state = State::Capturing {
                        post: Vec::new(),
                        speech_start: None,
                        silence: 0,
                    };
                    events.push(ListenEvent::Wake { score });
                } else {
                    self.wake_buf = frame;
                    self.wake_buf.clear();
                }
            } else {
                let len = self.vad.frame_len().max(1);
                let take = (len - self.vad_buf.len()).min(input.len());
                self.vad_buf.extend_from_slice(&input[..take]);
                input = &input[take..];
                if self.vad_buf.len() == len {
                    self.vad_frame(&mut events);
                }
            }
        }
        events
    }

    fn vad_frame(&mut self, events: &mut Vec<ListenEvent>) {
        let speech = self.vad.is_speech(&self.vad_buf);
        let State::Capturing {
            post,
            speech_start,
            silence,
        } = &mut self.state
        else {
            return;
        };
        if speech {
            speech_start.get_or_insert(post.len());
            *silence = 0;
        } else if speech_start.is_some() {
            *silence += self.vad_buf.len();
        }
        post.extend_from_slice(&self.vad_buf);
        self.vad_buf.clear();

        match *speech_start {
            None if post.len() >= NO_SPEECH => {
                events.push(ListenEvent::NoSpeech);
                self.reset();
            }
            Some(start) if *silence >= END_SILENCE || post.len() >= MAX_UTTERANCE => {
                let from = start.saturating_sub(PRE_ROLL);
                let mut utterance = std::mem::take(&mut self.head);
                utterance.extend_from_slice(&post[from..]);
                events.push(ListenEvent::Utterance(utterance));
                self.reset();
            }
            _ => {}
        }
    }

    /// Abandonne la capture en cours et vide les tampons (coupure totale, perte du micro).
    pub fn reset(&mut self) {
        self.state = State::Idle;
        self.head.clear();
        self.vad_buf.clear();
        self.wake_buf.clear();
        self.ring.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct FakeWake {
        at_frames: Vec<usize>,
        frame: usize,
        calls: Arc<AtomicUsize>,
    }
    impl FakeWake {
        fn new(at_frame: usize) -> (Self, Arc<AtomicUsize>) {
            Self::multi(vec![at_frame])
        }
        /// Se déclenche à chacun des numéros d'appel `process` indiqués.
        fn multi(at_frames: Vec<usize>) -> (Self, Arc<AtomicUsize>) {
            let calls = Arc::new(AtomicUsize::new(0));
            let w = Self {
                at_frames,
                frame: 0,
                calls: calls.clone(),
            };
            (w, calls)
        }
    }
    impl WakeDetector for FakeWake {
        fn frame_len(&self) -> usize {
            160
        }
        fn process(&mut self, _: &[f32]) -> Option<f32> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.frame += 1;
            self.at_frames.contains(&self.frame).then_some(0.9)
        }
    }

    /// VAD piloté par une table : parole pour les trames dans `speech`.
    struct FakeVad {
        speech: std::ops::Range<usize>,
        speech2: std::ops::Range<usize>,
        frame: usize,
    }
    impl FakeVad {
        fn new(speech: std::ops::Range<usize>) -> Self {
            Self {
                speech,
                speech2: 0..0,
                frame: 0,
            }
        }
    }
    impl Vad for FakeVad {
        fn frame_len(&self) -> usize {
            256
        }
        fn is_speech(&mut self, _: &[f32]) -> bool {
            let r = self.speech.contains(&self.frame) || self.speech2.contains(&self.frame);
            self.frame += 1;
            r
        }
    }

    fn ramp(n: usize) -> Vec<f32> {
        (0..n).map(|i| i as f32 / 1.0e6).collect()
    }

    fn run(l: &mut Listener<FakeWake, FakeVad>, signal: &[f32]) -> Vec<ListenEvent> {
        let mut ev = Vec::new();
        for chunk in signal.chunks(97) {
            ev.extend(l.push(chunk));
        }
        ev
    }

    #[test]
    fn reveil_puis_phrase() {
        // Réveil à la fin de la trame 150 (échantillon 24 000).
        let (w, _) = FakeWake::new(150);
        let mut l = Listener::new(w, FakeVad::new(32..95));
        let sig = ramp(24_000 + 40_000);
        let ev = run(&mut l, &sig);
        assert_eq!(ev[0], ListenEvent::Wake { score: 0.9 });
        let ListenEvent::Utterance(u) = &ev[1] else {
            panic!("{ev:?}")
        };
        // 1 s avant le réveil en tête.
        assert_eq!(u[0], sig[8_000]);
        assert_eq!(u[15_999], sig[23_999]);
        // Pré-enregistrement : parole à 32*256 = 8192, moins 4800.
        assert_eq!(u[16_000], sig[24_000 + 8_192 - 4_800]);
        // Parole (63 trames) + 50 trames de silence, pré-enregistrement inclus.
        assert_eq!(u.len(), 16_000 + (32 + 63 + 50) * 256 - (8_192 - 4_800));
        assert_eq!(ev.len(), 2);
    }

    #[test]
    fn reveil_sans_parole_abandon() {
        let (w, _) = FakeWake::new(10);
        let mut l = Listener::new(w, FakeVad::new(0..0));
        let ev = run(&mut l, &ramp(1_600 + 70_000));
        assert_eq!(
            ev,
            vec![ListenEvent::Wake { score: 0.9 }, ListenEvent::NoSpeech]
        );
    }

    #[test]
    fn phrase_coupee_a_dix_secondes() {
        let (w, _) = FakeWake::new(10);
        let mut l = Listener::new(w, FakeVad::new(0..10_000));
        let ev = run(&mut l, &ramp(1_600 + 170_000));
        assert_eq!(ev[0], ListenEvent::Wake { score: 0.9 });
        let ListenEvent::Utterance(u) = &ev[1] else {
            panic!("{ev:?}")
        };
        assert_eq!(u.len(), 1_600 + 160_000);
        assert_eq!(ev.len(), 2);
    }

    #[test]
    fn second_reveil() {
        // Deuxième réveil 10 trames après la fin de la première capture.
        let (w, _) = FakeWake::multi(vec![10, 20]);
        let mut vad = FakeVad::new(32..95);
        vad.speech2 = 160..190;
        let mut l = Listener::new(w, vad);
        let sig = ramp(1_600 + 37_120 + 1_600 + 70_000);
        let ev = run(&mut l, &sig);
        assert_eq!(ev.len(), 4, "{ev:?}");
        assert_eq!(ev[0], ListenEvent::Wake { score: 0.9 });
        assert!(matches!(ev[1], ListenEvent::Utterance(_)));
        assert_eq!(ev[2], ListenEvent::Wake { score: 0.9 });
        let ListenEvent::Utterance(u) = &ev[3] else {
            panic!("{ev:?}")
        };
        // L'anneau a été vidé : seules les 10 trames d'attente précèdent le réveil.
        assert_eq!(u[0], sig[38_720]);
        assert_eq!(u.len(), 1_600 + (15 + 30 + 50) * 256);
    }

    #[test]
    fn independant_du_decoupage() {
        let sig = ramp(24_000 + 40_000);
        let make = || {
            let (w, _) = FakeWake::new(150);
            Listener::new(w, FakeVad::new(32..95))
        };
        let by_chunks = run(&mut make(), &sig);
        let whole = make().push(&sig);
        assert_eq!(by_chunks, whole);
        assert_eq!(whole.len(), 2);
    }

    #[test]
    fn pas_de_reveil_pendant_la_phrase() {
        let (w, calls) = FakeWake::new(10);
        let mut l = Listener::new(w, FakeVad::new(0..10_000));
        run(&mut l, &ramp(1_600));
        let after_wake = calls.load(Ordering::SeqCst);
        run(&mut l, &ramp(50_000));
        assert_eq!(calls.load(Ordering::SeqCst), after_wake);
    }

    #[test]
    fn reinitialisation_abandonne_la_capture() {
        let (w, _) = FakeWake::new(10);
        let mut l = Listener::new(w, FakeVad::new(0..0));
        let ev = l.push(&ramp(1_600));
        assert_eq!(ev, vec![ListenEvent::Wake { score: 0.9 }]);
        l.reset();
        // Sans la réinitialisation, 4 s sans parole produiraient un `NoSpeech`.
        assert!(run(&mut l, &ramp(70_000)).is_empty());
    }
}
