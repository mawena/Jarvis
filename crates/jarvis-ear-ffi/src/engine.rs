//! Moteur de l'oreille Android : la session commune de `jarvis-ear-core`, branchée sur
//! le micro et la sortie audio de Kotlin (`EarHost`).
//!
//! Fils : `start` lance un fil dédié qui possède un runtime tokio à 2 fils et exécute
//! la session ; les appels de Kotlin (`push_audio`, `mute`…) ne font que poser un
//! message dans un canal ou lire un état, sans jamais attendre la session.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use jarvis_ear_core::earshot::EarshotVad;
use jarvis_ear_core::listener::Listener;
use jarvis_ear_core::net::discovery::MdnsDiscovery;
use jarvis_ear_core::net::tls::{TlsConnector, pinned_client_config};
use jarvis_ear_core::net::{Discovery, EarLink, Link, LinkConfig, LinkEvent};
use jarvis_ear_core::resample::f32_to_i16;
use jarvis_ear_core::rustpotter::RustpotterDetector;
use jarvis_ear_core::session::{
    BrainLink, Capture, CaptureEvent, EarCommand, EarSession, EarStatus, OfflinePhrase, Player,
};
use jarvis_ear_core::{MicState, WakeDetector};
use tokio::sync::{mpsc, watch};

use crate::enroll::{model_path, pcm_to_f32, samples_dir};
use crate::pairing::{pairing_dir, store_of};
use crate::view::{Notifier, view_of};
use crate::{EarError, EarHost, EarSettings, EarView};

/// Attente maximale de la fin de la session dans `stop` (elle ferme le lien en 4 s).
const STOP_TIMEOUT: Duration = Duration::from_secs(5);

/// Zéros exacts consécutifs, en secondes de son, avant de déclarer le micro bloqué.
const BLOCKED_AFTER_SECONDS: usize = 3;

/// Taille de la file du micro vers la session (voir `EarSession::run`).
type CaptureTx = mpsc::Sender<CaptureEvent<Vec<f32>>>;

/// Crée le lien (dans le runtime de la session : `EarLink::spawn` y lance une tâche).
type LinkFactory = Box<dyn FnOnce() -> (Arc<dyn Link>, mpsc::Receiver<LinkEvent>) + Send>;

/// Appelle Kotlin sans laisser une exception étrangère faire tomber le fil Rust.
fn guard(what: &str, f: impl FnOnce()) {
    if catch_unwind(AssertUnwindSafe(f)).is_err() {
        tracing::error!("{what} : échec côté Kotlin");
    }
}

/// État partagé entre l'engin, la session (capture, lecteur) et les appels de Kotlin.
struct Shared {
    host: Arc<dyn EarHost>,
    output_rate: u32,
    capture_tx: Mutex<Option<CaptureTx>>,
    rate: AtomicU32,
    channels: AtomicU32,
    /// Échantillons exactement nuls consécutifs (tous canaux).
    zero_run: AtomicUsize,
    blocked: AtomicBool,
    overflow_warned: AtomicBool,
    /// Numéro du dernier son confié à Kotlin.
    generation: Mutex<u64>,
    playing: watch::Sender<bool>,
    status: watch::Receiver<EarStatus>,
    notifier: Notifier,
    /// La session tourne : les commandes ont un destinataire.
    running: AtomicBool,
}

impl Shared {
    fn view(&self) -> EarView {
        view_of(&self.status.borrow(), self.blocked.load(Ordering::SeqCst))
    }

    /// Envoie la vue à Kotlin si elle a changé.
    fn refresh(&self) {
        let view = self.view();
        self.notifier.notify(&view, |view| {
            guard("status_changed", || self.host.status_changed(view.clone()));
        });
    }

    fn send_event(&self, event: CaptureEvent<Vec<f32>>) {
        let tx = self
            .capture_tx
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if let Some(tx) = tx
            && tx.try_send(event).is_err()
            && !self.overflow_warned.swap(true, Ordering::Relaxed)
        {
            tracing::warn!("le son du micro s'accumule : paquets perdus");
        }
    }

    fn reset_zeros(&self) {
        self.zero_run.store(0, Ordering::SeqCst);
        if self.blocked.swap(false, Ordering::SeqCst) {
            self.refresh();
        }
    }

    fn push_audio(&self, pcm_le: &[u8]) {
        let has_tx = self
            .capture_tx
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some();
        if !has_tx {
            return;
        }
        let samples = pcm_to_f32(pcm_le);
        if samples.is_empty() {
            return;
        }
        self.track_zeros(&samples);
        self.send_event(CaptureEvent::Samples(samples));
    }

    /// Android livre des zéros exacts quand il bloque le micro : au bout de 3 s, la
    /// vue passe en « bloqué » ; un échantillon non nul la lève.
    fn track_zeros(&self, samples: &[f32]) {
        let trailing = samples.iter().rev().take_while(|s| **s == 0.0).count();
        if trailing == samples.len() {
            self.zero_run.fetch_add(trailing, Ordering::SeqCst);
        } else {
            self.zero_run.store(trailing, Ordering::SeqCst);
            if self.blocked.swap(false, Ordering::SeqCst) {
                self.refresh();
            }
            return;
        }
        let rate = self.rate.load(Ordering::SeqCst) as usize;
        let channels = self.channels.load(Ordering::SeqCst) as usize;
        let limit = BLOCKED_AFTER_SECONDS * rate * channels;
        if self.zero_run.load(Ordering::SeqCst) >= limit
            && !self.blocked.swap(true, Ordering::SeqCst)
        {
            tracing::warn!("le micro livre des zéros exacts depuis 3 s : bloqué par Android ?");
            self.refresh();
        }
    }
}

/// Micro de Kotlin vu par la session.
struct FfiCapture(Arc<Shared>);

impl Capture for FfiCapture {
    type Samples = Vec<f32>;

    fn start(&mut self, tx: CaptureTx) -> anyhow::Result<()> {
        // Avant d'appeler Kotlin : `audio_format` peut arriver aussitôt.
        *self.0.capture_tx.lock().unwrap_or_else(|e| e.into_inner()) = Some(tx);
        self.0.reset_zeros();
        guard("start_capture", || self.0.host.start_capture());
        Ok(())
    }

    fn stop(&mut self) {
        *self.0.capture_tx.lock().unwrap_or_else(|e| e.into_inner()) = None;
        self.0.reset_zeros();
        guard("stop_capture", || self.0.host.stop_capture());
    }
}

/// Sortie sonore de Kotlin vue par la session.
struct FfiPlayer(Arc<Shared>);

impl Player for FfiPlayer {
    fn sample_rate(&self) -> u32 {
        self.0.output_rate
    }

    fn play(&mut self, samples: Vec<f32>, sample_rate: u32) {
        let pcm: Vec<u8> = samples
            .iter()
            .flat_map(|s| f32_to_i16(*s).to_le_bytes())
            .collect();
        let generation = {
            let mut generation = self.0.generation.lock().unwrap_or_else(|e| e.into_inner());
            *generation += 1;
            self.0.playing.send_replace(true);
            *generation
        };
        guard("play_pcm", || {
            self.0.host.play_pcm(pcm, sample_rate, generation)
        });
    }

    fn stop(&mut self) {
        self.0.playing.send_replace(false);
        guard("stop_playback", || self.0.host.stop_playback());
    }

    fn playing(&self) -> watch::Receiver<bool> {
        self.0.playing.subscribe()
    }
}

/// Détecteur de mot d'éveil rangé dans une boîte.
struct BoxedWake(Box<dyn WakeDetector>);

impl WakeDetector for BoxedWake {
    fn frame_len(&self) -> usize {
        self.0.frame_len()
    }
    fn process(&mut self, frame: &[f32]) -> Option<f32> {
        self.0.process(frame)
    }
}

/// Ce qui attend le démarrage.
struct Parts {
    wake: Box<dyn WakeDetector>,
    link: LinkFactory,
    commands_rx: mpsc::UnboundedReceiver<EarCommand>,
    status_tx: watch::Sender<EarStatus>,
}

struct Running {
    done: std_mpsc::Receiver<()>,
}

enum Lifecycle {
    Ready(Box<Parts>),
    Running(Running),
    Stopped,
}

/// Moteur de l'oreille : un seul par service.
#[derive(uniffi::Object)]
pub struct EarEngine {
    settings: EarSettings,
    data_dir: PathBuf,
    shared: Arc<Shared>,
    commands: mpsc::UnboundedSender<EarCommand>,
    life: Mutex<Lifecycle>,
}

impl EarEngine {
    fn build(
        settings: EarSettings,
        host: Arc<dyn EarHost>,
        wake: Box<dyn WakeDetector>,
        link: LinkFactory,
    ) -> Arc<Self> {
        let data_dir = PathBuf::from(&settings.data_dir);
        // L'écoute coupée est retenue d'un démarrage à l'autre : la vue le dit déjà.
        let muted = data_dir.join("muted").exists();
        let (status_tx, status) = watch::channel(EarStatus {
            mic: if muted {
                MicState::Muted
            } else {
                MicState::Open
            },
            capturing: false,
            brain: BrainLink::Connecting,
            last_heard: None,
        });
        let (commands, commands_rx) = mpsc::unbounded_channel();
        let shared = Arc::new(Shared {
            host,
            output_rate: settings.output_rate,
            capture_tx: Mutex::new(None),
            rate: AtomicU32::new(jarvis_ear_core::SAMPLE_RATE),
            channels: AtomicU32::new(1),
            zero_run: AtomicUsize::new(0),
            blocked: AtomicBool::new(false),
            overflow_warned: AtomicBool::new(false),
            generation: Mutex::new(0),
            playing: watch::channel(false).0,
            status,
            notifier: Notifier::default(),
            running: AtomicBool::new(false),
        });
        Arc::new(Self {
            settings,
            data_dir,
            shared,
            commands,
            life: Mutex::new(Lifecycle::Ready(Box::new(Parts {
                wake,
                link,
                commands_rx,
                status_tx,
            }))),
        })
    }

    /// Moteur sur un lien et un détecteur donnés (tests sans réseau ni modèle).
    #[cfg(test)]
    pub(crate) fn with_parts(
        settings: EarSettings,
        host: Arc<dyn EarHost>,
        link: Arc<dyn Link>,
        events: mpsc::Receiver<LinkEvent>,
        wake: impl WakeDetector + 'static,
    ) -> Arc<Self> {
        Self::build(
            settings,
            host,
            Box::new(wake),
            Box::new(move || (link, events)),
        )
    }

    fn command(&self, command: EarCommand) {
        if self.shared.running.load(Ordering::SeqCst) {
            let _ = self.commands.send(command);
        }
    }
}

/// Corps du fil du moteur : runtime, lien, session, jusqu'à `Quit`.
fn engine_main(
    parts: Parts,
    shared: Arc<Shared>,
    settings: EarSettings,
    data_dir: PathBuf,
    ready: std_mpsc::Sender<Result<(), String>>,
    done: std_mpsc::Sender<()>,
) {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("jarvis-tokio")
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => {
            let _ = ready.send(Err(format!("runtime tokio : {e}")));
            return;
        }
    };
    shared.running.store(true, Ordering::SeqCst);
    let _ = ready.send(Ok(()));
    tracing::info!("oreille « {} » démarrée", settings.name);
    let Parts {
        wake,
        link,
        commands_rx,
        status_tx,
    } = parts;
    let result = runtime.block_on(async {
        let (link, events) = link();
        let listener = Listener::new(BoxedWake(wake), EarshotVad::new());
        let store = store_of(&data_dir);
        let offline: OfflinePhrase = Arc::new(move || store.offline_phrase().map(|(_, a)| a));
        let threshold = settings.wake_threshold;
        let session = EarSession::new(
            listener,
            FfiCapture(shared.clone()),
            link,
            events,
            commands_rx,
            status_tx,
        )
        .with_player(Box::new(FfiPlayer(shared.clone())))
        .with_wake_threshold(threshold)
        .with_offline_phrase(offline)
        .with_mute_file(data_dir.join("muted"));
        shared.refresh();
        let mut status = shared.status.clone();
        let forward = shared.clone();
        tokio::spawn(async move {
            while status.changed().await.is_ok() {
                forward.refresh();
            }
        });
        session.run().await
    });
    if let Err(e) = result {
        tracing::error!("session de l'oreille : {e:#}");
    }
    shared.running.store(false, Ordering::SeqCst);
    let _ = done.send(());
    runtime.shutdown_background();
}

#[uniffi::export]
impl EarEngine {
    /// Prépare le moteur (appairage et mot d'éveil lus), sans rien lancer.
    #[uniffi::constructor]
    pub fn new(settings: EarSettings, host: Arc<dyn EarHost>) -> Result<Arc<Self>, EarError> {
        let data_dir = PathBuf::from(&settings.data_dir);
        for dir in [pairing_dir(&data_dir), samples_dir(&data_dir)] {
            jarvis_ear_core::enroll::create_private_dir(&dir).map_err(|e| EarError::Io {
                message: format!("dossier de l'oreille : {e}"),
            })?;
        }
        let store = store_of(&data_dir);
        let pairing = store
            .load()
            .map_err(|e| EarError::Io {
                message: format!("appairage illisible : {e}"),
            })?
            .ok_or(EarError::NotPaired)?;
        let model = model_path(&data_dir);
        if !model.exists() {
            return Err(EarError::NoWakeword);
        }
        let threshold = settings.wake_threshold.clamp(0.30, 0.80);
        let wake = RustpotterDetector::load(&model, threshold).map_err(|e| EarError::Io {
            message: format!("mot d'éveil illisible : {e:#}"),
        })?;
        let settings = EarSettings {
            wake_threshold: threshold,
            ..settings
        };
        let link: LinkFactory = Box::new(move || {
            let discovery = match MdnsDiscovery::new() {
                Ok(mdns) => Some(Arc::new(mdns) as Arc<dyn Discovery>),
                Err(e) => {
                    tracing::warn!("découverte mDNS indisponible : {e:#}");
                    None
                }
            };
            let connector = Arc::new(TlsConnector::new(pinned_client_config(
                &pairing.fingerprint,
            )));
            let (link, events) =
                EarLink::spawn(LinkConfig::new(pairing, store, connector, discovery));
            (Arc::new(link) as Arc<dyn Link>, events)
        });
        Ok(Self::build(settings, host, Box::new(wake), link))
    }

    /// Lance l'écoute et le lien avec le cerveau ; un second appel ne fait rien.
    /// Bref : le travail se fait sur un fil du moteur. Après `stop`, il faut un
    /// nouveau moteur.
    pub fn start(&self) -> Result<(), EarError> {
        let mut life = self.life.lock().unwrap_or_else(|e| e.into_inner());
        let parts = match std::mem::replace(&mut *life, Lifecycle::Stopped) {
            Lifecycle::Ready(parts) => parts,
            Lifecycle::Running(running) => {
                *life = Lifecycle::Running(running);
                return Ok(());
            }
            Lifecycle::Stopped => {
                return Err(EarError::Internal {
                    message: "moteur arrêté : créez-en un nouveau".into(),
                });
            }
        };
        let (ready_tx, ready_rx) = std_mpsc::channel();
        let (done_tx, done_rx) = std_mpsc::channel();
        let shared = self.shared.clone();
        let settings = self.settings.clone();
        let data_dir = self.data_dir.clone();
        std::thread::Builder::new()
            .name("jarvis-oreille".into())
            .spawn(move || engine_main(*parts, shared, settings, data_dir, ready_tx, done_tx))
            .map_err(|e| EarError::Internal {
                message: format!("fil du moteur : {e}"),
            })?;
        match ready_rx.recv_timeout(Duration::from_secs(10)) {
            Ok(Ok(())) => {
                *life = Lifecycle::Running(Running { done: done_rx });
                Ok(())
            }
            Ok(Err(message)) => Err(EarError::Internal { message }),
            Err(_) => Err(EarError::Internal {
                message: "le moteur ne démarre pas".into(),
            }),
        }
    }

    /// Arrête tout (idempotent, borné à 5 s).
    pub fn stop(&self) {
        let running = {
            let mut life = self.life.lock().unwrap_or_else(|e| e.into_inner());
            match std::mem::replace(&mut *life, Lifecycle::Stopped) {
                Lifecycle::Running(running) => Some(running),
                _ => None,
            }
        };
        let Some(running) = running else { return };
        let _ = self.commands.send(EarCommand::Quit);
        if let Err(std_mpsc::RecvTimeoutError::Timeout) = running.done.recv_timeout(STOP_TIMEOUT) {
            tracing::warn!("le moteur ne s'arrête pas en 5 s");
        }
    }

    /// Format du micro, annoncé avant le premier `push_audio` (et à chaque
    /// `start_capture`).
    pub fn audio_format(&self, rate: u32, channels: u16) {
        if rate == 0 || channels == 0 {
            return;
        }
        self.shared.rate.store(rate, Ordering::SeqCst);
        self.shared
            .channels
            .store(u32::from(channels), Ordering::SeqCst);
        self.shared.zero_run.store(0, Ordering::SeqCst);
        self.shared
            .send_event(CaptureEvent::Format { rate, channels });
    }

    /// Son du micro : PCM 16 bits little-endian (blocs de 20 ms).
    pub fn push_audio(&self, pcm_le: Vec<u8>) {
        self.shared.push_audio(&pcm_le);
    }

    /// Le micro n'a pas pu s'ouvrir.
    pub fn capture_unavailable(&self) {
        self.shared.send_event(CaptureEvent::Unavailable);
    }

    /// Fin de lecture du son de cette génération ; une fin périmée est ignorée.
    pub fn playback_idle(&self, generation: u64) {
        let latest = self
            .shared
            .generation
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if *latest == generation {
            self.shared.playing.send_replace(false);
        }
    }

    /// Couper l'écoute (le micro reste ouvert, « Jarvis » est ignoré).
    pub fn mute(&self) {
        self.command(EarCommand::Mute);
    }

    /// Réactiver l'écoute.
    pub fn unmute(&self) {
        self.command(EarCommand::Unmute);
    }

    /// Coupure totale : micro fermé.
    pub fn hard_mute(&self) {
        self.command(EarCommand::HardMute);
    }

    /// Rouvrir le micro après une coupure totale.
    pub fn reopen(&self) {
        self.command(EarCommand::Reopen);
    }

    /// État courant.
    pub fn view(&self) -> EarView {
        self.shared.view()
    }
}

impl Drop for EarEngine {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests;
