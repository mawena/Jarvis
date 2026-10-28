//! Lecture des sons (voix de Jarvis, bips) sur la sortie audio par défaut.
//!
//! Un thread dédié possède le `cpal::Stream` (qui n'est pas `Send`). Il rééchantillonne
//! chaque son vers la fréquence du périphérique, puis le passe au callback par un canal
//! borné. Le callback ne bloque ni n'alloue jamais : il lit le son en cours, le copie sur
//! tous les canaux, et rend chaque son fini au thread, qui le libère.
//!
//! `stop()` incrémente un compteur (génération) lu par le callback : les sons plus
//! anciens sont abandonnés dès le tampon suivant (~10-20 ms), et le son demandé juste
//! après (le bip de réveil) n'est pas coupé par erreur.
//!
//! La sortie s'ouvre au premier son et se ferme après `IDLE_CLOSE` de silence : pas de
//! flux ouvert en permanence (la sortie peut se mettre en veille), et un changement de
//! sortie par défaut (casque Bluetooth branché) est pris au son suivant. Sortie perdue ou
//! bloquée : le son en cours est abandonné (journalisé), la sortie est rouverte au suivant.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc as std_mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Context, anyhow};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use jarvis_ear_core::resample::resample;
use tokio::sync::watch;

/// Fréquence des bips si la sortie est inconnue.
const FALLBACK_RATE: u32 = 48_000;
/// Sons en file au plus (au-delà, le son est abandonné).
const QUEUE: usize = 8;
/// Fermeture de la sortie après ce délai sans son.
const IDLE_CLOSE: Duration = Duration::from_secs(30);
/// La lecture est dite finie un peu après le dernier échantillon envoyé : le temps que
/// le son sorte vraiment (Bluetooth compris), pour ne pas capter la fin de la phrase.
const TAIL: Duration = Duration::from_millis(300);
/// Marge avant de déclarer la sortie bloquée.
const STALL_MARGIN: Duration = Duration::from_secs(2);
/// Période de surveillance pendant la lecture.
const POLL: Duration = Duration::from_millis(20);

/// Sortie sonore (remplaçable dans les tests).
pub trait Player: Send {
    /// Fréquence conseillée pour générer les bips.
    fn sample_rate(&self) -> u32;
    /// Ajoute un son mono à la file ; il sera joué après ceux déjà en file.
    fn play(&mut self, samples: Vec<f32>, sample_rate: u32);
    /// Coupe tout de suite le son en cours et vide la file.
    fn stop(&mut self);
    /// `true` tant qu'un son est en file ou en cours de lecture.
    fn playing(&self) -> watch::Receiver<bool>;

    fn is_playing(&self) -> bool {
        *self.playing().borrow()
    }
}

/// Aucune sortie : les sons sont ignorés.
pub struct SilentPlayer {
    playing: watch::Sender<bool>,
}

impl SilentPlayer {
    pub fn new() -> Self {
        Self {
            playing: watch::channel(false).0,
        }
    }
}

impl Player for SilentPlayer {
    fn sample_rate(&self) -> u32 {
        FALLBACK_RATE
    }
    fn play(&mut self, _: Vec<f32>, _: u32) {}
    fn stop(&mut self) {}
    fn playing(&self) -> watch::Receiver<bool> {
        self.playing.subscribe()
    }
}

/// Un son prêt pour le callback : mono, à la fréquence de la sortie.
struct Chunk {
    /// Génération au moment de la demande ; plus ancienne que l'actuelle = coupé.
    generation: u64,
    samples: Vec<f32>,
}

/// État partagé entre l'application, le thread et le callback.
struct Shared {
    /// Incrémentée par `stop()` : le callback abandonne les sons plus anciens.
    generation: Arc<AtomicU64>,
    /// Sons demandés et pas encore finis.
    pending: AtomicUsize,
    playing: watch::Sender<bool>,
}

impl Shared {
    /// Recalcule `playing` sous le verrou du `watch` : deux appels concurrents ne peuvent
    /// pas laisser une valeur périmée.
    fn refresh(&self) {
        self.playing.send_if_modified(|p| {
            let now = self.pending.load(Ordering::SeqCst) > 0;
            let changed = *p != now;
            *p = now;
            changed
        });
    }

    fn finish(&self, count: usize) {
        if count > 0 {
            self.pending.fetch_sub(count, Ordering::SeqCst);
            self.refresh();
        }
    }
}

/// Côté callback : le son en cours et la file.
struct Feed {
    chunks: std_mpsc::Receiver<Chunk>,
    retired: std_mpsc::SyncSender<Chunk>,
    current: Option<Chunk>,
    pos: usize,
    generation: Arc<AtomicU64>,
}

impl Feed {
    /// Remplit `out` (entrelacé sur `channels`) ; silence quand la file est vide.
    /// Ni allocation ni verrou bloquant.
    fn fill<T>(&mut self, out: &mut [T], channels: usize)
    where
        T: cpal::Sample + cpal::FromSample<f32>,
    {
        let generation = self.generation.load(Ordering::Acquire);
        for frame in out.chunks_mut(channels.max(1)) {
            let v = T::from_sample(self.next(generation));
            frame.fill(v);
        }
    }

    fn next(&mut self, generation: u64) -> f32 {
        loop {
            let Some(chunk) = &self.current else {
                match self.chunks.try_recv() {
                    Ok(c) => {
                        self.current = Some(c);
                        self.pos = 0;
                        continue;
                    }
                    Err(_) => return 0.0,
                }
            };
            if chunk.generation < generation || self.pos >= chunk.samples.len() {
                self.retire();
                continue;
            }
            self.pos += 1;
            return chunk.samples[self.pos - 1];
        }
    }

    /// Rend le son fini au thread (qui le libère hors du callback).
    fn retire(&mut self) {
        if let Some(chunk) = self.current.take() {
            // Le canal est dimensionné pour toute la file : l'envoi ne bloque pas.
            let _ = self.retired.try_send(chunk);
        }
    }
}

/// Sortie ouverte, côté thread.
struct Output {
    _stream: cpal::Stream,
    rate: u32,
    chunks: std_mpsc::SyncSender<Chunk>,
    retired: std_mpsc::Receiver<Chunk>,
    failed: Arc<AtomicBool>,
    /// Sons passés au callback et pas encore rendus.
    in_flight: usize,
    /// Fin prévue de la lecture (au-delà, avec une marge : sortie bloquée).
    expected_end: Instant,
    last_activity: Instant,
}

enum Command {
    Play {
        samples: Vec<f32>,
        rate: u32,
        generation: u64,
    },
    Quit,
}

/// Lecture réelle sur `cpal`.
pub struct CpalPlayer {
    commands: std_mpsc::Sender<Command>,
    shared: Arc<Shared>,
    rate: u32,
    thread: Option<JoinHandle<()>>,
}

impl CpalPlayer {
    /// Démarre le thread de lecture. La sortie n'est ouverte qu'au premier son ; sa
    /// fréquence est seulement lue ici (pour les bips), sans rien jouer.
    pub fn new() -> anyhow::Result<Self> {
        let rate = match output_rate() {
            Ok(rate) => rate,
            Err(e) => {
                tracing::warn!("sortie audio indisponible pour l'instant : {e:#}");
                FALLBACK_RATE
            }
        };
        let shared = Arc::new(Shared {
            generation: Arc::new(AtomicU64::new(0)),
            pending: AtomicUsize::new(0),
            playing: watch::channel(false).0,
        });
        let (commands, rx) = std_mpsc::channel();
        let thread = {
            let shared = shared.clone();
            std::thread::Builder::new()
                .name("jarvis-playback".into())
                .spawn(move || run(rx, shared))
                .context("démarrage du thread de lecture")?
        };
        Ok(Self {
            commands,
            shared,
            rate,
            thread: Some(thread),
        })
    }
}

impl Player for CpalPlayer {
    fn sample_rate(&self) -> u32 {
        self.rate
    }

    fn play(&mut self, samples: Vec<f32>, rate: u32) {
        if samples.is_empty() || rate == 0 {
            return;
        }
        self.shared.pending.fetch_add(1, Ordering::SeqCst);
        self.shared.refresh();
        let generation = self.shared.generation.load(Ordering::SeqCst);
        let sent = self.commands.send(Command::Play {
            samples,
            rate,
            generation,
        });
        if sent.is_err() {
            tracing::warn!("thread de lecture arrêté : son ignoré");
            self.shared.finish(1);
        }
    }

    fn stop(&mut self) {
        self.shared.generation.fetch_add(1, Ordering::SeqCst);
    }

    fn playing(&self) -> watch::Receiver<bool> {
        self.shared.playing.subscribe()
    }
}

impl Drop for CpalPlayer {
    fn drop(&mut self) {
        self.stop();
        let _ = self.commands.send(Command::Quit);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Boucle du thread de lecture.
fn run(commands: std_mpsc::Receiver<Command>, shared: Arc<Shared>) {
    let mut out: Option<Output> = None;
    // Fins de lecture différées de `TAIL`.
    let mut releases: VecDeque<Instant> = VecDeque::new();
    loop {
        let command = if out.is_none() && releases.is_empty() {
            commands
                .recv()
                .map_err(|_| std_mpsc::RecvTimeoutError::Disconnected)
        } else {
            commands.recv_timeout(POLL)
        };
        match command {
            Ok(Command::Play {
                samples,
                rate,
                generation,
            }) => {
                if !enqueue(&mut out, &shared, samples, rate, generation) {
                    shared.finish(1);
                }
            }
            Ok(Command::Quit) | Err(std_mpsc::RecvTimeoutError::Disconnected) => break,
            Err(std_mpsc::RecvTimeoutError::Timeout) => {}
        }
        let now = Instant::now();
        if let Some(o) = out.as_mut() {
            // Les sons finis sont libérés ici, hors du callback.
            while o.retired.try_recv().is_ok() {
                o.in_flight -= 1;
                o.last_activity = now;
                releases.push_back(now + TAIL);
            }
            let failed = o.failed.load(Ordering::Relaxed);
            let stalled = o.in_flight > 0 && now > o.expected_end + STALL_MARGIN;
            if failed || stalled {
                if stalled {
                    tracing::warn!("sortie audio bloquée : son abandonné");
                }
                let lost = o.in_flight;
                out = None;
                shared.finish(lost);
            } else if o.in_flight == 0 && now.duration_since(o.last_activity) > IDLE_CLOSE {
                tracing::debug!("sortie audio fermée (inactive)");
                out = None;
            }
        }
        while releases.front().is_some_and(|t| *t <= now) {
            releases.pop_front();
            shared.finish(1);
        }
    }
}

/// Passe un son au callback (sortie ouverte au besoin). `false` : son abandonné.
fn enqueue(
    out: &mut Option<Output>,
    shared: &Shared,
    samples: Vec<f32>,
    rate: u32,
    generation: u64,
) -> bool {
    if generation < shared.generation.load(Ordering::SeqCst) {
        return false;
    }
    if out.is_none() {
        match open(&shared.generation) {
            Ok(o) => *out = Some(o),
            Err(e) => {
                tracing::warn!("sortie audio indisponible, son ignoré : {e:#}");
                return false;
            }
        }
    }
    let Some(o) = out.as_mut() else { return false };
    if o.in_flight >= QUEUE {
        tracing::warn!("trop de sons en file : son ignoré");
        return false;
    }
    let samples = resample(&samples, rate, o.rate);
    let duration = Duration::from_secs_f64(samples.len() as f64 / o.rate as f64);
    let now = Instant::now();
    if o.in_flight == 0 {
        o.expected_end = now;
    }
    if o.chunks
        .try_send(Chunk {
            generation,
            samples,
        })
        .is_err()
    {
        return false;
    }
    o.in_flight += 1;
    o.expected_end = o.expected_end.max(now) + duration;
    o.last_activity = now;
    true
}

fn output_rate() -> anyhow::Result<u32> {
    let device = cpal::default_host()
        .default_output_device()
        .ok_or_else(|| anyhow!("aucune sortie audio"))?;
    Ok(device.default_output_config()?.sample_rate())
}

fn open(generation: &Arc<AtomicU64>) -> anyhow::Result<Output> {
    let device = cpal::default_host()
        .default_output_device()
        .ok_or_else(|| anyhow!("aucune sortie audio"))?;
    let supported = device.default_output_config()?;
    let channels = supported.channels() as usize;
    let rate = supported.sample_rate();
    let (chunks_tx, chunks_rx) = std_mpsc::sync_channel(QUEUE);
    // Une place de plus que la file : le callback n'attend jamais pour rendre un son.
    let (retired_tx, retired_rx) = std_mpsc::sync_channel(QUEUE + 1);
    let failed = Arc::new(AtomicBool::new(false));
    let mut feed = Feed {
        chunks: chunks_rx,
        retired: retired_tx,
        current: None,
        pos: 0,
        generation: generation.clone(),
    };
    macro_rules! stream {
        ($t:ty) => {{
            let failed = failed.clone();
            device.build_output_stream(
                supported.config(),
                move |data: &mut [$t], _: &cpal::OutputCallbackInfo| feed.fill(data, channels),
                move |e| {
                    tracing::error!("erreur de la sortie audio : {e}");
                    failed.store(true, Ordering::Relaxed);
                },
                None,
            )?
        }};
    }
    let stream = match supported.sample_format() {
        cpal::SampleFormat::F32 => stream!(f32),
        cpal::SampleFormat::I16 => stream!(i16),
        cpal::SampleFormat::I32 => stream!(i32),
        cpal::SampleFormat::U16 => stream!(u16),
        other => return Err(anyhow!("format de sortie non géré : {other:?}")),
    };
    stream.play()?;
    tracing::debug!("sortie audio ouverte ({rate} Hz, {channels} canaux)");
    let now = Instant::now();
    Ok(Output {
        _stream: stream,
        rate,
        chunks: chunks_tx,
        retired: retired_rx,
        failed,
        in_flight: 0,
        expected_end: now,
        last_activity: now,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(
        generation: &Arc<AtomicU64>,
    ) -> (Feed, std_mpsc::SyncSender<Chunk>, std_mpsc::Receiver<Chunk>) {
        let (tx, rx) = std_mpsc::sync_channel(QUEUE);
        let (rtx, rrx) = std_mpsc::sync_channel(QUEUE + 1);
        let f = Feed {
            chunks: rx,
            retired: rtx,
            current: None,
            pos: 0,
            generation: generation.clone(),
        };
        (f, tx, rrx)
    }

    #[test]
    fn copie_le_mono_sur_chaque_canal_puis_silence() {
        let generation = Arc::new(AtomicU64::new(0));
        let (mut f, tx, retired) = feed(&generation);
        tx.send(Chunk {
            generation: 0,
            samples: vec![0.5, -0.25],
        })
        .unwrap();
        let mut out = [9.0f32; 6];
        f.fill(&mut out, 2);
        assert_eq!(out, [0.5, 0.5, -0.25, -0.25, 0.0, 0.0]);
        assert_eq!(retired.try_recv().unwrap().samples, vec![0.5, -0.25]);
    }

    #[test]
    fn enchaine_les_sons_en_file() {
        let generation = Arc::new(AtomicU64::new(0));
        let (mut f, tx, retired) = feed(&generation);
        for s in [vec![0.1], vec![0.2, 0.3]] {
            tx.send(Chunk {
                generation: 0,
                samples: s,
            })
            .unwrap();
        }
        let mut out = [0.0f32; 4];
        f.fill(&mut out, 1);
        assert_eq!(out, [0.1, 0.2, 0.3, 0.0]);
        assert_eq!(retired.try_iter().count(), 2);
    }

    #[test]
    fn arret_coupe_et_vide_la_file() {
        let generation = Arc::new(AtomicU64::new(0));
        let (mut f, tx, retired) = feed(&generation);
        for _ in 0..2 {
            tx.send(Chunk {
                generation: 0,
                samples: vec![0.5; 100],
            })
            .unwrap();
        }
        let mut out = [0.0f32; 4];
        f.fill(&mut out, 1);
        assert_eq!(out, [0.5; 4]);
        // `stop()`, puis un nouveau son (le bip de réveil) : seul lui est joué.
        generation.fetch_add(1, Ordering::SeqCst);
        tx.send(Chunk {
            generation: 1,
            samples: vec![0.25; 2],
        })
        .unwrap();
        let mut out = [9.0f32; 4];
        f.fill(&mut out, 1);
        assert_eq!(out, [0.25, 0.25, 0.0, 0.0]);
        assert_eq!(retired.try_iter().count(), 3);
    }

    #[test]
    fn convertit_vers_le_format_de_la_sortie() {
        let generation = Arc::new(AtomicU64::new(0));
        let (mut f, tx, _retired) = feed(&generation);
        tx.send(Chunk {
            generation: 0,
            samples: vec![0.5, -1.0],
        })
        .unwrap();
        let mut out = [0i16; 2];
        f.fill(&mut out, 1);
        assert_eq!(out, [16_384, -32_768]);
    }
}
