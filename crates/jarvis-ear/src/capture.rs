//! Capture du micro. Un thread dédié possède le `cpal::Stream` (qui n'est pas `Send`),
//! l'ouvre, le surveille et le rouvre toutes les 5 s si le périphérique disparaît.
//!
//! Le callback audio ne bloque jamais : `try_send` vers un canal borné, les trames
//! perdues sont comptées et journalisées par le thread de surveillance.

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

/// Ce que la capture transmet à l'application.
#[derive(Debug, Clone, PartialEq)]
pub enum CaptureEvent {
    /// Le flux est ouvert avec ce format ; les `Samples` suivants sont entrelacés ainsi.
    Format { rate: u32, channels: u16 },
    /// Échantillons entrelacés au format du périphérique.
    Samples(Vec<f32>),
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
        if tx.try_send(CaptureEvent::Unavailable).is_err() && tx.is_closed() {
            return;
        }
        match stop.recv_timeout(RETRY_DELAY) {
            Err(std_mpsc::RecvTimeoutError::Timeout) => {}
            _ => return,
        }
    }
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

    macro_rules! stream {
        ($t:ty) => {{
            let tx = tx.clone();
            let dropped = dropped.clone();
            let failed = failed.clone();
            device.build_input_stream(
                config,
                move |data: &[$t], _| {
                    let samples = data
                        .iter()
                        .map(|s| cpal::Sample::to_sample::<f32>(*s))
                        .collect();
                    if tx.try_send(CaptureEvent::Samples(samples)).is_err() {
                        dropped.fetch_add(1, Ordering::Relaxed);
                    }
                },
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
    tx.blocking_send(CaptureEvent::Format {
        rate: supported.sample_rate(),
        channels: supported.channels(),
    })
    .map_err(|_| anyhow!("application arrêtée"))?;
    stream.play()?;
    Ok(stream)
}
