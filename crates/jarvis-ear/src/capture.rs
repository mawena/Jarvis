//! Capture du micro. Un thread dédié possède le `cpal::Stream` (qui n'est pas `Send`),
//! l'ouvre, le surveille et le rouvre toutes les 5 s si le périphérique disparaît.
//!
//! Le callback audio ne bloque ni n'alloue jamais : il prend un tampon dans un pool
//! préalloué, le remplit, puis `try_send` vers un canal borné. Le tampon revient au pool
//! quand l'application a fini de le lire. Pool vide ou canal plein : la trame est perdue,
//! comptée et journalisée par le thread de surveillance.

use std::ops::Deref;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, mpsc as std_mpsc};
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::{Context, anyhow};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use tokio::sync::mpsc;

/// Délai avant un nouvel essai d'ouverture du micro.
const RETRY_DELAY: Duration = Duration::from_secs(5);
/// Période de surveillance du flux.
const WATCH_PERIOD: Duration = Duration::from_millis(500);

/// Nombre de tampons du pool.
const POOL_BUFFERS: usize = 64;
/// Capacité (en échantillons) de chaque tampon du pool.
const POOL_CAPACITY: usize = 8192;

/// Échantillons entrelacés ; le tampon retourne au pool quand l'objet est détruit.
#[derive(Debug)]
pub struct Samples {
    data: Vec<f32>,
    home: Option<std_mpsc::SyncSender<Vec<f32>>>,
}

impl Samples {
    /// Échantillons hors pool (tests).
    #[cfg(test)]
    pub fn detached(data: Vec<f32>) -> Self {
        Self { data, home: None }
    }
}

impl Deref for Samples {
    type Target = [f32];
    fn deref(&self) -> &[f32] {
        &self.data
    }
}

impl PartialEq for Samples {
    fn eq(&self, other: &Self) -> bool {
        self.data == other.data
    }
}

impl Drop for Samples {
    fn drop(&mut self) {
        if let Some(home) = self.home.take() {
            // Le pool est dimensionné pour tous les tampons : l'envoi ne bloque pas.
            let _ = home.try_send(std::mem::take(&mut self.data));
        }
    }
}

/// Pool de tampons recyclés, côté callback audio.
struct Pool {
    capacity: usize,
    free: std_mpsc::Receiver<Vec<f32>>,
    home: std_mpsc::SyncSender<Vec<f32>>,
}

impl Pool {
    fn new(buffers: usize, capacity: usize) -> Self {
        let (home, free) = std_mpsc::sync_channel(buffers);
        for _ in 0..buffers {
            let _ = home.try_send(Vec::with_capacity(capacity));
        }
        Self {
            capacity,
            free,
            home,
        }
    }

    /// Copie `data` dans des tampons du pool et les envoie ; aucune allocation.
    /// Un tampon manquant ou un canal plein compte une trame perdue.
    fn forward<T>(
        &self,
        data: &[T],
        channels: usize,
        tx: &mpsc::Sender<CaptureEvent>,
        dropped: &AtomicU64,
    ) where
        T: cpal::Sample + Copy,
        f32: cpal::FromSample<T>,
    {
        // Les morceaux gardent des trames entières (pas de canal coupé en deux).
        let step = (self.capacity / channels.max(1)).max(1) * channels.max(1);
        for part in data.chunks(step) {
            let Ok(mut buf) = self.free.try_recv() else {
                dropped.fetch_add(1, Ordering::Relaxed);
                continue;
            };
            buf.clear();
            if part.len() > buf.capacity() {
                // Jamais de réallocation dans le callback.
                dropped.fetch_add(1, Ordering::Relaxed);
                let _ = self.home.try_send(buf);
                continue;
            }
            buf.extend(part.iter().map(|s| cpal::Sample::to_sample::<f32>(*s)));
            let samples = Samples {
                data: buf,
                home: Some(self.home.clone()),
            };
            if tx.try_send(CaptureEvent::Samples(samples)).is_err() {
                // L'événement renvoyé par `try_send` est détruit : le tampon revient au pool.
                dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

/// Ce que la capture transmet à l'application.
#[derive(Debug, PartialEq)]
pub enum CaptureEvent {
    /// Le flux est ouvert avec ce format ; les `Samples` suivants sont entrelacés ainsi.
    Format { rate: u32, channels: u16 },
    /// Échantillons entrelacés au format du périphérique.
    Samples(Samples),
    /// Le micro est absent ou perdu (nouvel essai dans 5 s).
    Unavailable,
}

/// Source audio de l'application (remplaçable dans les tests).
pub trait Capture: Send {
    /// Ouvre le flux et envoie les événements dans `tx`.
    fn start(&mut self, tx: mpsc::Sender<CaptureEvent>) -> anyhow::Result<()>;
    /// Ferme le flux (coupure totale).
    fn stop(&mut self);
}

/// Poignée du thread de capture.
pub struct CaptureHandle {
    stop: std_mpsc::Sender<()>,
    thread: Option<JoinHandle<()>>,
}

impl CaptureHandle {
    /// Ferme le flux `cpal` et attend la fin du thread.
    pub fn stop(mut self) {
        let _ = self.stop.send(());
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Démarre la capture ; `device` désigne un périphérique par son nom (défaut : celui du système).
pub fn start(
    tx: mpsc::Sender<CaptureEvent>,
    device: Option<String>,
) -> anyhow::Result<CaptureHandle> {
    let (stop, stop_rx) = std_mpsc::channel();
    let thread = std::thread::Builder::new()
        .name("jarvis-capture".into())
        .spawn(move || supervise(tx, device, stop_rx))
        .context("démarrage du thread de capture")?;
    Ok(CaptureHandle {
        stop,
        thread: Some(thread),
    })
}

/// Capture réelle sur `cpal`.
pub struct CpalCapture {
    device: Option<String>,
    handle: Option<CaptureHandle>,
}

impl CpalCapture {
    pub fn new(device: Option<String>) -> Self {
        Self {
            device,
            handle: None,
        }
    }
}

impl Capture for CpalCapture {
    fn start(&mut self, tx: mpsc::Sender<CaptureEvent>) -> anyhow::Result<()> {
        self.stop();
        self.handle = Some(start(tx, self.device.clone())?);
        Ok(())
    }

    fn stop(&mut self) {
        if let Some(h) = self.handle.take() {
            h.stop();
        }
    }
}

impl Drop for CpalCapture {
    fn drop(&mut self) {
        self.stop();
    }
}

fn supervise(tx: mpsc::Sender<CaptureEvent>, device: Option<String>, stop: std_mpsc::Receiver<()>) {
    let dropped = Arc::new(AtomicU64::new(0));
    let mut reported = 0;
    loop {
        let failed = Arc::new(AtomicBool::new(false));
        match open(device.as_deref(), &tx, &failed, &dropped) {
            Ok(stream) => {
                loop {
                    match stop.recv_timeout(WATCH_PERIOD) {
                        Err(std_mpsc::RecvTimeoutError::Timeout) => {}
                        // Arrêt demandé (ou poignée abandonnée) : le flux est fermé en sortant.
                        _ => return,
                    }
                    let total = dropped.load(Ordering::Relaxed);
                    if total != reported {
                        tracing::warn!("{} trames audio perdues (canal plein)", total - reported);
                        reported = total;
                    }
                    if failed.load(Ordering::Relaxed) {
                        break;
                    }
                }
                drop(stream);
                tracing::warn!("flux micro interrompu");
            }
            Err(e) => tracing::warn!("micro indisponible : {e:#}"),
        }
        if !notify_unavailable(&tx, &stop) {
            return;
        }
        match stop.recv_timeout(RETRY_DELAY) {
            Err(std_mpsc::RecvTimeoutError::Timeout) => {}
            _ => return,
        }
    }
}

/// Annonce `Unavailable` sans jamais bloquer : en cas de canal plein, nouvel essai
/// toutes les 100 ms (au plus 2 s) en surveillant la demande d'arrêt, pour ne pas
/// se bloquer mutuellement avec `CaptureHandle::stop`. Renvoie `false` pour sortir.
fn notify_unavailable(tx: &mpsc::Sender<CaptureEvent>, stop: &std_mpsc::Receiver<()>) -> bool {
    for _ in 0..20 {
        match tx.try_send(CaptureEvent::Unavailable) {
            Ok(()) => return true,
            Err(mpsc::error::TrySendError::Closed(_)) => return false,
            Err(mpsc::error::TrySendError::Full(_)) => {
                match stop.recv_timeout(Duration::from_millis(100)) {
                    Err(std_mpsc::RecvTimeoutError::Timeout) => {}
                    _ => return false,
                }
            }
        }
    }
    tracing::warn!("canal plein : micro indisponible non annoncé");
    true
}

fn pick_device(name: Option<&str>) -> anyhow::Result<cpal::Device> {
    let host = cpal::default_host();
    match name {
        None => host
            .default_input_device()
            .ok_or_else(|| anyhow!("aucun micro disponible")),
        Some(wanted) => host
            .input_devices()?
            .find(|d| {
                d.description()
                    .map(|desc| desc.name() == wanted || desc.to_string() == wanted)
                    .unwrap_or(false)
            })
            .ok_or_else(|| anyhow!("périphérique « {wanted} » introuvable")),
    }
}

fn open(
    device: Option<&str>,
    tx: &mpsc::Sender<CaptureEvent>,
    failed: &Arc<AtomicBool>,
    dropped: &Arc<AtomicU64>,
) -> anyhow::Result<cpal::Stream> {
    let device = pick_device(device)?;
    let supported = device.default_input_config()?;
    let config = supported.config();

    let channels = supported.channels() as usize;
    macro_rules! stream {
        ($t:ty) => {{
            let tx = tx.clone();
            let dropped = dropped.clone();
            let failed = failed.clone();
            let pool = Pool::new(POOL_BUFFERS, POOL_CAPACITY);
            device.build_input_stream(
                config,
                move |data: &[$t], _| pool.forward(data, channels, &tx, &dropped),
                move |e| {
                    tracing::error!("erreur du flux micro : {e}");
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
        other => return Err(anyhow!("format de micro non géré : {other:?}")),
    };
    // Le format part avant le premier échantillon.
    // Sans attente : un canal plein ferait échouer l'ouverture (nouvel essai dans 5 s)
    // plutôt que de bloquer le thread, que `stop()` attend.
    tx.try_send(CaptureEvent::Format {
        rate: supported.sample_rate(),
        channels: supported.channels(),
    })
    .map_err(|e| anyhow!("format du micro non transmis : {e}"))?;
    stream.play()?;
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_of(event: CaptureEvent) -> Samples {
        match event {
            CaptureEvent::Samples(s) => s,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn les_tampons_reviennent_dans_le_pool() {
        let pool = Pool::new(2, 8);
        let (tx, mut rx) = mpsc::channel(8);
        let dropped = AtomicU64::new(0);
        for round in 0..5 {
            pool.forward(&[0.5f32, -0.5, 0.25, 0.0], 2, &tx, &dropped);
            let s = sample_of(rx.try_recv().unwrap());
            assert_eq!(&*s, &[0.5, -0.5, 0.25, 0.0], "tour {round}");
            assert_eq!(s.data.capacity(), 8, "pas de réallocation");
            drop(s);
        }
        assert_eq!(dropped.load(Ordering::Relaxed), 0);
        // Les deux tampons sont revenus : deux envois consécutifs passent.
        pool.forward(&[1.0f32; 4], 1, &tx, &dropped);
        pool.forward(&[1.0f32; 4], 1, &tx, &dropped);
        assert_eq!(dropped.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn pool_vide_compte_une_perte_sans_allouer() {
        let pool = Pool::new(1, 8);
        let (tx, mut rx) = mpsc::channel(8);
        let dropped = AtomicU64::new(0);
        pool.forward(&[1.0f32; 4], 1, &tx, &dropped);
        let held = sample_of(rx.try_recv().unwrap());
        // Le seul tampon est détenu : la trame est perdue, rien n'est envoyé.
        pool.forward(&[1.0f32; 4], 1, &tx, &dropped);
        assert_eq!(dropped.load(Ordering::Relaxed), 1);
        assert!(rx.try_recv().is_err());
        drop(held);
        pool.forward(&[1.0f32; 4], 1, &tx, &dropped);
        assert_eq!(dropped.load(Ordering::Relaxed), 1);
        assert!(rx.try_recv().is_ok());
    }

    #[test]
    fn donnees_trop_grandes_decoupees_sans_reallocation() {
        let pool = Pool::new(4, 8);
        let (tx, mut rx) = mpsc::channel(8);
        let dropped = AtomicU64::new(0);
        // 20 échantillons stéréo : morceaux de 8 (4 trames entières).
        pool.forward(&[0.1f32; 20], 2, &tx, &dropped);
        let sizes: Vec<usize> = std::iter::from_fn(|| rx.try_recv().ok())
            .map(|e| {
                let s = sample_of(e);
                assert_eq!(s.data.capacity(), 8);
                s.len()
            })
            .collect();
        assert_eq!(sizes, vec![8, 8, 4]);
        assert_eq!(dropped.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn canal_plein_compte_une_perte_et_rend_le_tampon() {
        let pool = Pool::new(2, 8);
        let (tx, mut rx) = mpsc::channel(1);
        let dropped = AtomicU64::new(0);
        pool.forward(&[1.0f32; 4], 1, &tx, &dropped);
        pool.forward(&[1.0f32; 4], 1, &tx, &dropped);
        assert_eq!(dropped.load(Ordering::Relaxed), 1);
        drop(rx.try_recv().unwrap());
        // Le tampon de la trame perdue est revenu au pool.
        pool.forward(&[1.0f32; 4], 1, &tx, &dropped);
        pool.forward(&[1.0f32; 4], 1, &tx, &dropped);
        assert_eq!(dropped.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn unavailable_ne_bloque_pas_sur_canal_plein() {
        let (tx, _rx) = mpsc::channel(1);
        tx.try_send(CaptureEvent::Unavailable).unwrap();
        let (stop, stop_rx) = std_mpsc::channel();
        stop.send(()).unwrap();
        // Canal plein et arrêt demandé : sortie immédiate.
        assert!(!notify_unavailable(&tx, &stop_rx));
    }
}
