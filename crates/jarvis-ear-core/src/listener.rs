//! Machine d'états d'écoute : attente du mot d'éveil, puis capture de la phrase,
//! rendue d'un bloc (`Utterance`) ou en flux (`Chunk` puis `Ended`).

use std::collections::VecDeque;
use std::time::Duration;

use crate::{SAMPLE_RATE, Vad, WakeDetector};
use jarvis_protocol::frame::MAX_PHRASE_SAMPLES;

const RATE: usize = SAMPLE_RATE as usize;
/// Dernière seconde conservée (contient « Jarvis »).
const RING_LEN: usize = RATE;
/// Pré-enregistrement avant le début de parole.
const PRE_ROLL: usize = RATE * 3 / 10;
/// Silence qui clôt la phrase.
const END_SILENCE: usize = RATE * 8 / 10;
/// Délai sans parole après le réveil avant abandon.
const NO_SPEECH: usize = RATE * 4;
/// Durée maximale de la phrase, mesurée depuis le réveil (seconde de tête en plus ;
/// le tout borné à `MAX_PHRASE_SAMPLES`, ce que le cerveau accepte).
const MAX_UTTERANCE: usize = RATE * 10;
/// Temps de parole garanti même si elle commence à la fin du délai.
const MIN_SPEECH: usize = MAX_UTTERANCE - NO_SPEECH;
/// Fenêtre du niveau sonore du réveil (0,5 s avant).
const LEVEL_WINDOW: usize = RATE / 2;
/// Plancher du niveau sonore (dBFS).
const LEVEL_FLOOR_DB: f32 = -90.0;
/// Après le réveil : le bip de réveil (~120 ms) ne compte pas comme parole.
pub const WAKE_GUARD: Duration = Duration::from_millis(200);
/// Début de l'écoute directe : la fin de la voix de Jarvis (latence de la sortie) non plus.
pub const LISTEN_GUARD: Duration = Duration::from_millis(300);

fn samples(d: Duration) -> usize {
    (d.as_secs_f64() * RATE as f64) as usize
}

#[derive(Debug, Clone, PartialEq)]
pub enum ListenEvent {
    /// Mot d'éveil détecté ; `level_db` : niveau RMS (dBFS) des 0,5 s précédentes.
    Wake { score: f32, level_db: f32 },
    /// Phrase complète (commence par la seconde précédant le réveil). Sans flux.
    Utterance(Vec<f32>),
    /// En flux : son de la capture (d'abord la seconde de tête, puis chaque trame).
    Chunk(Vec<f32>),
    /// En flux : fin de la phrase. `cut` : plage d'échantillons du flux à retirer
    /// (silence entre la tête et le pré-enregistrement) pour obtenir la phrase.
    Ended { cut: Option<(usize, usize)> },
    /// Aucune parole après le réveil.
    NoSpeech,
}

enum State {
    Idle,
    Capturing {
        /// Audio après le réveil (trames VAD entières ; vide en flux).
        post: Vec<f32>,
        /// Longueur de l'audio après le réveil (échantillons).
        len: usize,
        /// Longueur de la tête (déjà rendue en flux).
        head_len: usize,
        /// Début de la parole dans `post`.
        speech_start: Option<usize>,
        /// Silence consécutif depuis la dernière parole (échantillons).
        silence: usize,
        /// Abandon si aucune parole avant cette longueur de `post`.
        no_speech: usize,
        /// Longueur maximale de `post`.
        max_len: usize,
        /// Pas de début de parole tant que `post` est plus court (l'audio est gardé).
        guard: usize,
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
    wake_guard: usize,
    listen_guard: usize,
    /// Score de réveil minimal (relevé pendant que Jarvis parle).
    min_wake_score: f32,
    /// Capture rendue en flux.
    streaming: bool,
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
            wake_guard: samples(WAKE_GUARD),
            listen_guard: samples(LISTEN_GUARD),
            min_wake_score: 0.0,
            streaming: false,
        }
    }

    /// Mode flux : `Chunk` pendant la capture puis `Ended`, au lieu de `Utterance`.
    pub fn with_streaming(mut self, streaming: bool) -> Self {
        self.streaming = streaming;
        self
    }

    /// Durées pendant lesquelles la parole est ignorée après le réveil et au début de
    /// l'écoute directe (par défaut [`WAKE_GUARD`] et [`LISTEN_GUARD`]).
    pub fn with_guards(mut self, wake: Duration, listen: Duration) -> Self {
        self.wake_guard = samples(wake);
        self.listen_guard = samples(listen);
        self
    }

    /// Réveils de score inférieur ignorés (0 : tous acceptés).
    pub fn set_min_wake_score(&mut self, score: f32) {
        self.min_wake_score = score;
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
                if let Some(score) = self.wake.process(&frame)
                    && score >= self.min_wake_score
                {
                    let level_db = level_db(&self.ring);
                    self.head = self.ring.iter().copied().collect();
                    self.ring.clear();
                    self.vad_buf.clear();
                    events.push(ListenEvent::Wake { score, level_db });
                    let head_len = self.head.len();
                    if self.streaming && head_len > 0 {
                        events.push(ListenEvent::Chunk(std::mem::take(&mut self.head)));
                    }
                    self.state = State::Capturing {
                        post: Vec::new(),
                        len: 0,
                        head_len,
                        speech_start: None,
                        silence: 0,
                        no_speech: NO_SPEECH,
                        max_len: MAX_UTTERANCE.min(MAX_PHRASE_SAMPLES - head_len),
                        guard: self.wake_guard,
                    };
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
            len,
            head_len,
            speech_start,
            silence,
            no_speech,
            max_len,
            guard,
        } = &mut self.state
        else {
            return;
        };
        // Garde : décision ignorée, mais l'audio et le délai d'abandon courent.
        let speech = speech && *len >= *guard;
        if speech {
            speech_start.get_or_insert(*len);
            *silence = 0;
        } else if speech_start.is_some() {
            *silence += self.vad_buf.len();
        }
        *len += self.vad_buf.len();
        if self.streaming {
            events.push(ListenEvent::Chunk(self.vad_buf.clone()));
        } else {
            post.extend_from_slice(&self.vad_buf);
        }
        self.vad_buf.clear();

        match *speech_start {
            None if *len >= *no_speech => {
                events.push(ListenEvent::NoSpeech);
                self.reset();
            }
            Some(start) if *silence >= END_SILENCE || *len >= *max_len => {
                let from = start.saturating_sub(PRE_ROLL);
                if self.streaming {
                    let cut = (from > 0).then_some((*head_len, *head_len + from));
                    events.push(ListenEvent::Ended { cut });
                } else {
                    let mut utterance = std::mem::take(&mut self.head);
                    utterance.extend_from_slice(&post[from..]);
                    events.push(ListenEvent::Utterance(utterance));
                }
                self.reset();
            }
            _ => {}
        }
    }

    /// Capture tout de suite, sans mot d'éveil (réponse à une question de Jarvis) :
    /// abandon si personne ne parle avant `wait`. La phrase n'a pas de seconde de tête ;
    /// elle dure au plus `MAX_PHRASE_SAMPLES` en tout (attente comprise).
    pub fn listen_now(&mut self, wait: Duration) {
        self.reset();
        let max_len = (samples(wait) + MIN_SPEECH).min(MAX_PHRASE_SAMPLES);
        let no_speech = samples(wait).min(max_len);
        self.state = State::Capturing {
            post: Vec::new(),
            len: 0,
            head_len: 0,
            speech_start: None,
            silence: 0,
            no_speech,
            max_len,
            guard: self.listen_guard,
        };
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

/// Niveau RMS (dBFS) de la fin de `ring` ([`LEVEL_WINDOW`]), plancher [`LEVEL_FLOOR_DB`].
fn level_db(ring: &VecDeque<f32>) -> f32 {
    let n = ring.len().min(LEVEL_WINDOW);
    let sum: f64 = ring
        .iter()
        .skip(ring.len() - n)
        .map(|s| f64::from(*s) * f64::from(*s))
        .sum();
    let rms = if n == 0 { 0.0 } else { (sum / n as f64).sqrt() };
    if rms <= 0.0 {
        return LEVEL_FLOOR_DB;
    }
    ((20.0 * rms.log10()) as f32).max(LEVEL_FLOOR_DB)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

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
        run_with(l, signal)
    }

    /// Réveil du faux détecteur (score 0,9), quel que soit le niveau.
    fn is_wake(e: &ListenEvent) -> bool {
        matches!(e, ListenEvent::Wake { score, .. } if *score == 0.9)
    }

    /// Flux reconstitué (`Chunk` mis bout à bout) et événements hors `Chunk`.
    fn split_stream(ev: Vec<ListenEvent>) -> (Vec<f32>, Vec<ListenEvent>) {
        let mut stream = Vec::new();
        let mut rest = Vec::new();
        for e in ev {
            match e {
                ListenEvent::Chunk(c) => stream.extend(c),
                other => rest.push(other),
            }
        }
        (stream, rest)
    }

    /// Retire `cut` du flux.
    fn apply_cut(stream: &[f32], cut: Option<(usize, usize)>) -> Vec<f32> {
        match cut {
            Some((a, b)) => [&stream[..a], &stream[b..]].concat(),
            None => stream.to_vec(),
        }
    }

    #[test]
    fn flux_moins_coupe_egale_la_phrase() {
        let sig = ramp(24_000 + 40_000);
        let make = |streaming| {
            let (w, _) = FakeWake::new(150);
            Listener::new(w, FakeVad::new(32..95)).with_streaming(streaming)
        };
        let ev = run(&mut make(false), &sig);
        let ListenEvent::Utterance(utterance) = &ev[1] else {
            panic!("{ev:?}")
        };
        let ev = run(&mut make(true), &sig);
        // La tête vient tout de suite après le réveil.
        assert!(is_wake(&ev[0]), "{:?}", &ev[..2]);
        assert!(matches!(&ev[1], ListenEvent::Chunk(c) if c.len() == RATE));
        let (stream, rest) = split_stream(ev);
        let [w, ListenEvent::Ended { cut }] = rest.as_slice() else {
            panic!("{rest:?}")
        };
        assert!(is_wake(w));
        // Coupe entre la tête et le pré-enregistrement.
        assert_eq!(*cut, Some((RATE, RATE + 8_192 - PRE_ROLL)));
        assert_eq!(&apply_cut(&stream, *cut), utterance);

        // Écoute directe : pas de tête, coupe depuis le début.
        let make = |streaming| {
            let (w, _) = FakeWake::new(usize::MAX);
            let mut l = Listener::new(w, FakeVad::new(30..50)).with_streaming(streaming);
            l.listen_now(Duration::from_secs(8));
            l
        };
        let sig = ramp((30 + 20 + 50) * 256);
        let ev = run(&mut make(false), &sig);
        let [ListenEvent::Utterance(utterance)] = ev.as_slice() else {
            panic!("{ev:?}")
        };
        let (stream, rest) = split_stream(run(&mut make(true), &sig));
        let [ListenEvent::Ended { cut }] = rest.as_slice() else {
            panic!("{rest:?}")
        };
        assert_eq!(*cut, Some((0, 30 * 256 - PRE_ROLL)));
        assert_eq!(&apply_cut(&stream, *cut), utterance);

        // Parole dès le réveil : rien à couper.
        let (w, _) = FakeWake::new(150);
        let mut l = Listener::new(w, FakeVad::new(0..40))
            .with_streaming(true)
            .with_guards(Duration::ZERO, Duration::ZERO);
        let (_, rest) = split_stream(run(&mut l, &ramp(24_000 + 30_000)));
        assert!(
            matches!(rest[..], [_, ListenEvent::Ended { cut: None }]),
            "{rest:?}"
        );
    }

    #[test]
    fn niveau_sonore_du_reveil() {
        let level = |sig: &[f32]| {
            let (w, _) = FakeWake::new(sig.len() / 160);
            let mut l = Listener::new(w, FakeVad::new(0..0));
            let ev = l.push(sig);
            let [ListenEvent::Wake { level_db, .. }] = ev.as_slice() else {
                panic!("{ev:?}")
            };
            *level_db
        };
        // Sinusoïde d'amplitude 0,5 : RMS 0,354, soit −9,03 dBFS.
        let loud: Vec<f32> = (0..16_000)
            .map(|i| 0.5 * (i as f32 * 2.0 * std::f32::consts::PI / 40.0).sin())
            .collect();
        assert!((level(&loud) + 9.03).abs() < 0.1, "{}", level(&loud));
        // Silence : plancher.
        assert_eq!(level(&[0.0; 16_000]), -90.0);
        // Seules les 0,5 dernières secondes comptent.
        let mut late_silence = loud[..8_000].to_vec();
        late_silence.extend([0.0; 8_000]);
        assert_eq!(level(&late_silence), -90.0);
        // Réveil sans flux : le niveau y est aussi.
        let mut quiet = vec![0.0; 8_000];
        quiet.extend(loud[..8_000].iter().map(|s| s / 10.0));
        assert!((level(&quiet) + 29.03).abs() < 0.1, "{}", level(&quiet));
    }

    #[test]
    fn fin_sans_parole_signalee() {
        let (w, _) = FakeWake::new(10);
        let mut l = Listener::new(w, FakeVad::new(0..0)).with_streaming(true);
        let ev = run(&mut l, &ramp(1_600 + 70_000));
        let (stream, rest) = split_stream(ev);
        let [w, ListenEvent::NoSpeech] = rest.as_slice() else {
            panic!("{rest:?}")
        };
        assert!(is_wake(w));
        // Tête puis 4 s de flux, et plus rien ensuite.
        assert_eq!(stream.len(), 1_600 + NO_SPEECH);
        assert!(run(&mut l, &ramp(10_000)).is_empty());
    }

    fn run_with<V: Vad>(l: &mut Listener<FakeWake, V>, signal: &[f32]) -> Vec<ListenEvent> {
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
        assert!(is_wake(&ev[0]), "{ev:?}");
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
        let [w, ListenEvent::NoSpeech] = ev.as_slice() else {
            panic!("{ev:?}")
        };
        assert!(is_wake(w));
    }

    #[test]
    fn phrase_coupee_a_dix_secondes() {
        let (w, _) = FakeWake::new(10);
        let mut l = Listener::new(w, FakeVad::new(0..10_000));
        let ev = run(&mut l, &ramp(1_600 + 170_000));
        assert!(is_wake(&ev[0]), "{ev:?}");
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
        assert!(is_wake(&ev[0]), "{ev:?}");
        assert!(matches!(ev[1], ListenEvent::Utterance(_)));
        assert!(is_wake(&ev[2]), "{ev:?}");
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
    fn ecoute_directe_sans_mot_d_eveil() {
        let (w, calls) = FakeWake::new(1);
        let mut l = Listener::new(w, FakeVad::new(30..50));
        l.listen_now(Duration::from_secs(8));
        // Exactement jusqu'à la fin de la phrase (20 trames de parole, 50 de silence).
        let sig = ramp((30 + 20 + 50) * 256);
        let ev = run(&mut l, &sig);
        assert_eq!(calls.load(Ordering::SeqCst), 0, "mot d'éveil non consulté");
        let [ListenEvent::Utterance(u)] = ev.as_slice() else {
            panic!("{ev:?}")
        };
        // Pas de seconde de tête : la phrase commence au pré-enregistrement.
        assert_eq!(u[0], sig[30 * 256 - PRE_ROLL]);
        assert_eq!(u.len(), PRE_ROLL + (20 + 50) * 256);
    }

    #[test]
    fn ecoute_directe_sans_parole_abandon_apres_le_delai() {
        let (w, _) = FakeWake::new(usize::MAX);
        let mut l = Listener::new(w, FakeVad::new(0..0));
        l.listen_now(Duration::from_secs(8));
        // 4 s ne suffisent plus : le délai demandé est de 8 s.
        assert!(run(&mut l, &ramp(5 * RATE)).is_empty());
        assert_eq!(
            run(&mut l, &ramp(3 * RATE + 512)),
            vec![ListenEvent::NoSpeech]
        );
    }

    #[test]
    fn ecoute_directe_parole_tardive_pas_coupee_trop_tot() {
        let (w, _) = FakeWake::new(usize::MAX);
        // Parole à partir de 7,5 s, pendant 4 s.
        let start = 7 * RATE / 256 + 31;
        let mut l = Listener::new(w, FakeVad::new(start..start + 250));
        l.listen_now(Duration::from_secs(8));
        let ev = run(&mut l, &ramp(16 * RATE));
        let [ListenEvent::Utterance(u)] = ev.as_slice() else {
            panic!("{ev:?}")
        };
        assert_eq!(u.len(), PRE_ROLL + (250 + 50) * 256);
    }

    /// Longueur du flux d'une capture (tous les `Chunk`) et ses événements de fin.
    fn streamed(l: &mut Listener<FakeWake, FakeVad>, signal: &[f32]) -> (usize, Vec<ListenEvent>) {
        let (stream, rest) = split_stream(run(l, signal));
        (stream.len(), rest)
    }

    #[test]
    fn flux_borne_a_ce_que_le_cerveau_accepte() {
        // Parole sans fin dès le réveil : seconde de tête + 10 s, sous la borne commune.
        let (w, _) = FakeWake::new(100);
        let mut l = Listener::new(w, FakeVad::new(0..100_000)).with_streaming(true);
        let (len, rest) = streamed(&mut l, &ramp(RATE + 20 * RATE));
        assert!(
            matches!(rest.last(), Some(ListenEvent::Ended { .. })),
            "{rest:?}"
        );
        assert!(len <= MAX_PHRASE_SAMPLES, "{len}");

        // Écoute directe à attente longue : bornée elle aussi, attente comprise.
        let (w, _) = FakeWake::new(usize::MAX);
        let mut l = Listener::new(w, FakeVad::new(0..100_000)).with_streaming(true);
        l.listen_now(Duration::from_secs(30));
        let (len, rest) = streamed(&mut l, &ramp(40 * RATE));
        assert!(
            matches!(rest.as_slice(), [ListenEvent::Ended { .. }]),
            "{rest:?}"
        );
        assert!(len <= MAX_PHRASE_SAMPLES, "{len}");
        assert!(len + 256 > MAX_PHRASE_SAMPLES, "{len}");

        // Sans parole : abandon au plus tard à la borne.
        let (w, _) = FakeWake::new(usize::MAX);
        let mut l = Listener::new(w, FakeVad::new(0..0)).with_streaming(true);
        l.listen_now(Duration::from_secs(30));
        let (len, rest) = streamed(&mut l, &ramp(40 * RATE));
        assert_eq!(rest, [ListenEvent::NoSpeech]);
        assert!(len <= MAX_PHRASE_SAMPLES, "{len}");
    }

    #[test]
    fn reponse_a_une_question_tient_dans_la_borne() {
        // Attente de 8 s (`REPLY_WAIT`) + 6 s de parole garantie = la borne commune.
        assert_eq!(
            samples(Duration::from_secs(8)) + MIN_SPEECH,
            MAX_PHRASE_SAMPLES
        );
        assert!(RATE + MAX_UTTERANCE <= MAX_PHRASE_SAMPLES);
    }

    /// VAD réel en miniature : parole dès qu'une valeur dépasse 0,5.
    struct LoudVad;
    impl Vad for LoudVad {
        fn frame_len(&self) -> usize {
            256
        }
        fn is_speech(&mut self, frame: &[f32]) -> bool {
            frame.iter().any(|v| v.abs() > 0.5)
        }
    }

    fn tone(n: usize) -> Vec<f32> {
        (0..n).map(|i| 0.8 * (i as f32 * 0.35).sin()).collect()
    }

    #[test]
    fn bip_de_reveil_pas_pris_pour_de_la_parole() {
        let (w, _) = FakeWake::new(10);
        let mut l = Listener::new(w, LoudVad);
        // Réveil, puis bip de 120 ms, puis silence.
        let mut sig = vec![0.0; 1_600];
        sig.extend(tone(1_920));
        sig.extend(vec![0.0; NO_SPEECH - 1_920 - 256]);
        let ev = l.push(&sig);
        assert!(
            matches!(ev.as_slice(), [w] if is_wake(w)),
            "pas de phrase : {ev:?}"
        );
        // L'abandon tombe toujours 4 s après le réveil.
        assert_eq!(l.push(&[0.0; 256]), vec![ListenEvent::NoSpeech]);
    }

    #[test]
    fn parole_apres_la_garde_du_reveil_capturee() {
        let (w, _) = FakeWake::new(10);
        let mut l = Listener::new(w, LoudVad);
        let mut sig = vec![0.0; 1_600];
        sig.extend(tone(1_920));
        sig.extend(vec![0.0; 2_176]);
        // Parole à 256 ms du réveil (trame 16).
        sig.extend(tone(256 * 20));
        sig.extend(vec![0.0; 256 * 60]);
        let ev = run_with(&mut l, &sig);
        let [ListenEvent::Wake { .. }, ListenEvent::Utterance(u)] = ev.as_slice() else {
            panic!("{ev:?}")
        };
        // Le bip reste dans la phrase (il tombe dans le pré-enregistrement).
        assert_eq!(u[1_600..1_600 + 1_920], sig[1_600..1_600 + 1_920]);
        assert_eq!(u.len(), 1_600 + (16 + 20 + 50) * 256);
    }

    #[test]
    fn garde_du_reveil_reglable() {
        let (w, _) = FakeWake::new(10);
        let mut l = Listener::new(w, LoudVad).with_guards(Duration::ZERO, Duration::ZERO);
        let mut sig = vec![0.0; 1_600];
        sig.extend(tone(1_920));
        sig.extend(vec![0.0; 256 * 60]);
        let ev = l.push(&sig);
        assert!(matches!(
            ev[..],
            [ListenEvent::Wake { .. }, ListenEvent::Utterance(_)]
        ));
    }

    #[test]
    fn ecoute_directe_ignore_la_parole_pendant_la_garde() {
        let (w, _) = FakeWake::new(usize::MAX);
        let mut l = Listener::new(w, LoudVad);
        l.listen_now(Duration::from_secs(8));
        // Fin de la voix de Jarvis captée dans les 300 premières ms, puis silence.
        let mut sig = tone(4_608);
        sig.extend(vec![0.0; 256 * 62]);
        assert!(run_with(&mut l, &sig).is_empty(), "pas de phrase");
        // Vraie réponse ensuite.
        let mut reply = tone(256 * 20);
        reply.extend(vec![0.0; 256 * 60]);
        let ev = run_with(&mut l, &reply);
        let [ListenEvent::Utterance(u)] = ev.as_slice() else {
            panic!("{ev:?}")
        };
        assert_eq!(u.len(), PRE_ROLL + (20 + 50) * 256);
    }

    #[test]
    fn reveil_sous_le_score_minimal_ignore() {
        let (w, _) = FakeWake::multi(vec![10, 20]);
        let mut l = Listener::new(w, FakeVad::new(0..0));
        l.set_min_wake_score(0.95);
        assert!(l.push(&ramp(1_600)).is_empty());
        l.set_min_wake_score(0.0);
        assert!(matches!(l.push(&ramp(1_600)).as_slice(), [w] if is_wake(w)));
    }

    #[test]
    fn reinitialisation_abandonne_la_capture() {
        let (w, _) = FakeWake::new(10);
        let mut l = Listener::new(w, FakeVad::new(0..0));
        let ev = l.push(&ramp(1_600));
        assert!(matches!(ev.as_slice(), [w] if is_wake(w)), "{ev:?}");
        l.reset();
        // Sans la réinitialisation, 4 s sans parole produiraient un `NoSpeech`.
        assert!(run(&mut l, &ramp(70_000)).is_empty());
    }
}
