//! Boucle principale de l'oreille : trames du micro, mot d'éveil, phrases envoyées au
//! cerveau, réponses jouées, commandes de l'icône et statut périodique.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use jarvis_ear_core::listener::{ListenEvent, Listener};
use jarvis_ear_core::resample::{Downmixer, f32_to_i16, i16_to_f32};
use jarvis_ear_core::rustpotter::RustpotterDetector;
use jarvis_ear_core::{MicState, Vad, WakeDetector};
use jarvis_protocol::{
    ControlRequest, ControlResponse, EarAction, Sound, SpeechAudio, decode_pcm, encode_pcm,
};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use crate::capture::{Capture, CaptureEvent};
use crate::client::{self, ClientError};
use crate::playback::{Player, SilentPlayer};
use crate::sounds;
use crate::tray::{BrainLink, TrayCommand, TrayState};

/// Période d'interrogation du statut du cerveau.
pub const STATUS_PERIOD: Duration = Duration::from_secs(5);

/// Fichier `~/.local/state/jarvis/ear-muted` : présent tant que l'écoute est coupée.
pub fn default_mute_file() -> Option<PathBuf> {
    let base = directories::BaseDirs::new()?;
    let dir = base.state_dir().unwrap_or_else(|| base.data_local_dir());
    Some(dir.join("jarvis").join("ear-muted"))
}

/// Délai pour commencer à répondre à une question de Jarvis (`EarAction::AwaitReply`).
pub const REPLY_WAIT: Duration = Duration::from_secs(8);

/// Délai maximal de chaque requête envoyée en quittant.
const QUIT_TIMEOUT: Duration = Duration::from_secs(2);

/// Pendant que Jarvis parle, le réveil exige ce score en plus du seuil normal : la voix
/// de synthèse qui dit « Jarvis » ressemble moins au modèle (enregistré par
/// l'utilisateur) que l'utilisateur lui-même, qui peut donc toujours couper la parole.
pub const PLAYBACK_WAKE_MARGIN: f32 = 0.1;

/// Liaison avec le cerveau (remplaçable dans les tests).
#[async_trait::async_trait]
pub trait Brain: Send + Sync + 'static {
    async fn request(&self, req: ControlRequest) -> Result<ControlResponse, ClientError>;
}

/// Cerveau joint par son socket de contrôle.
pub struct SocketBrain {
    pub socket: PathBuf,
}

#[async_trait::async_trait]
impl Brain for SocketBrain {
    async fn request(&self, req: ControlRequest) -> Result<ControlResponse, ClientError> {
        client::request(&self.socket, &req).await
    }
}

/// Résultats des requêtes lancées en tâche de fond.
enum Internal {
    Heard {
        result: Result<ControlResponse, ClientError>,
        /// La phrase a été envoyée pendant l'écoute coupée.
        muted: bool,
    },
    Status(Result<ControlResponse, ClientError>),
}

pub struct App<W: WakeDetector, V: Vad, C: Capture> {
    listener: Listener<W, V>,
    capture: C,
    brain: Arc<dyn Brain>,
    commands: mpsc::UnboundedReceiver<TrayCommand>,
    tray: watch::Sender<TrayState>,
    status_period: Duration,
    downmixer: Option<Downmixer>,
    muted: bool,
    /// Présence = écoute coupée : survit à un redémarrage de l'oreille.
    mute_file: Option<PathBuf>,
    /// Coupure totale volontairement non retenue : au redémarrage, le micro se rouvre
    /// (un micro fermé pour de bon sans que l'on s'en souvienne serait pire).
    hard_muted: bool,
    unavailable: bool,
    capturing: bool,
    /// La capture en cours répond à une question (sans mot d'éveil).
    follow_up: bool,
    /// Une question attend la fin de la lecture pour écouter la réponse.
    awaiting_reply: bool,
    player: Box<dyn Player>,
    playing: watch::Receiver<bool>,
    link: BrainLink,
    last_heard: Option<String>,
    /// Requête `Wake` en cours : la phrase attend qu'elle soit partie.
    pending_wake: Option<JoinHandle<()>>,
    status_in_flight: bool,
    /// Score de réveil minimal pendant la lecture.
    playback_wake_score: f32,
    internal_tx: mpsc::UnboundedSender<Internal>,
    internal_rx: mpsc::UnboundedReceiver<Internal>,
}

impl<W: WakeDetector, V: Vad, C: Capture> App<W, V, C> {
    pub fn new(
        listener: Listener<W, V>,
        capture: C,
        brain: Arc<dyn Brain>,
        commands: mpsc::UnboundedReceiver<TrayCommand>,
        tray: watch::Sender<TrayState>,
    ) -> Self {
        let (internal_tx, internal_rx) = mpsc::unbounded_channel();
        let player = SilentPlayer::new();
        let playing = player.playing();
        Self {
            listener,
            capture,
            brain,
            commands,
            tray,
            status_period: STATUS_PERIOD,
            downmixer: None,
            muted: false,
            mute_file: None,
            hard_muted: false,
            unavailable: false,
            capturing: false,
            follow_up: false,
            awaiting_reply: false,
            player: Box::new(player),
            playing,
            link: BrainLink::Unreachable,
            last_heard: None,
            pending_wake: None,
            status_in_flight: false,
            playback_wake_score: RustpotterDetector::DEFAULT_THRESHOLD + PLAYBACK_WAKE_MARGIN,
            internal_tx,
            internal_rx,
        }
    }

    /// Sortie des sons (par défaut : aucune).
    pub fn with_player(mut self, player: Box<dyn Player>) -> Self {
        self.playing = player.playing();
        self.player = player;
        self
    }

    /// Seuil normal du mot d'éveil : celui de la lecture s'en déduit.
    pub fn with_wake_threshold(mut self, threshold: f32) -> Self {
        self.playback_wake_score = threshold + PLAYBACK_WAKE_MARGIN;
        self
    }

    /// Retient l'écoute coupée dans `path` ; un fichier présent coupe l'écoute dès le départ.
    pub fn with_mute_file(mut self, path: PathBuf) -> Self {
        self.muted = path.exists();
        self.mute_file = Some(path);
        self
    }

    fn set_muted(&mut self, muted: bool) {
        if self.muted == muted {
            return;
        }
        self.muted = muted;
        let Some(path) = &self.mute_file else { return };
        let result = if muted {
            path.parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|()| std::fs::write(path, ""))
        } else {
            std::fs::remove_file(path).or_else(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    Ok(())
                } else {
                    Err(e)
                }
            })
        };
        if let Err(e) = result {
            tracing::warn!("état de l'écoute non retenu ({}) : {e}", path.display());
        }
    }

    fn mic(&self) -> MicState {
        if self.hard_muted {
            MicState::HardMuted
        } else if self.unavailable {
            MicState::Unavailable
        } else if self.muted {
            MicState::Muted
        } else {
            MicState::Open
        }
    }

    fn publish(&self) {
        let state = TrayState {
            mic: self.mic(),
            capturing: self.capturing,
            brain: self.link.clone(),
            last_heard: self.last_heard.clone(),
        };
        self.tray.send_if_modified(|current| {
            let changed = *current != state;
            *current = state;
            changed
        });
    }

    /// Tourne jusqu'à la commande `Quit` (ou la fermeture du canal des commandes).
    pub async fn run(mut self) -> anyhow::Result<()> {
        let (cap_tx, mut cap_rx) = mpsc::channel(256);
        if let Err(e) = self.capture.start(cap_tx.clone()) {
            tracing::warn!("micro indisponible : {e:#}");
            self.unavailable = true;
        }
        self.publish();
        let mut tick = tokio::time::interval(self.status_period);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                Some(event) = cap_rx.recv() => self.on_capture(event),
                command = self.commands.recv() => match command {
                    Some(TrayCommand::Quit) | None => break,
                    Some(command) => self.on_command(command, &cap_tx),
                },
                Some(internal) = self.internal_rx.recv() => self.on_internal(internal),
                Ok(()) = self.playing.changed() => self.on_playback(),
                _ = tick.tick() => self.poll_status(),
            }
            self.publish();
        }
        self.capture.stop();
        self.restore_on_quit().await;
        Ok(())
    }

    /// Quitter pendant une capture : la musique avait été baissée au réveil. On attend le
    /// `Wake`, puis on envoie un `Listen` vide et on attend la réponse (une tâche
    /// détachée mourrait avec le runtime).
    async fn restore_on_quit(&mut self) {
        if !self.capturing {
            return;
        }
        self.capturing = false;
        if let Some(wake) = self.pending_wake.take()
            && tokio::time::timeout(QUIT_TIMEOUT, wake).await.is_err()
        {
            tracing::warn!("réveil toujours en cours en quittant");
        }
        let listen = ControlRequest::Listen {
            audio: String::new(),
            muted: self.muted,
            follow_up: self.follow_up,
        };
        match tokio::time::timeout(QUIT_TIMEOUT, self.brain.request(listen)).await {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => tracing::warn!("musique non rétablie en quittant : {e}"),
            Err(_) => tracing::warn!("le cerveau ne répond pas en quittant"),
        }
    }

    fn on_capture(&mut self, event: CaptureEvent) {
        if self.hard_muted {
            return;
        }
        match event {
            CaptureEvent::Format { rate, channels } => {
                self.downmixer = Some(Downmixer::new(rate, channels));
                self.unavailable = false;
            }
            CaptureEvent::Unavailable => {
                self.downmixer = None;
                self.unavailable = true;
                if self.capturing {
                    // La musique avait été baissée au réveil : on la rétablit.
                    self.send_listen(&[]);
                }
                self.listener.reset();
            }
            CaptureEvent::Samples(data) => {
                let Some(downmixer) = self.downmixer.as_mut() else {
                    return;
                };
                let mono = downmixer.push(&data);
                // Le tampon retourne au pool de la capture.
                drop(data);
                let min_score = if self.player.is_playing() {
                    self.playback_wake_score
                } else {
                    0.0
                };
                self.listener.set_min_wake_score(min_score);
                for event in self.listener.push(&mono) {
                    self.on_listen(event);
                }
            }
        }
    }

    fn on_listen(&mut self, event: ListenEvent) {
        match event {
            ListenEvent::Wake { score } => {
                tracing::info!("mot d'éveil détecté (score {score:.2})");
                // « Jarvis » coupe la parole de Jarvis, et une question en attente.
                if self.player.is_playing() {
                    self.player.stop();
                }
                self.awaiting_reply = false;
                // Écoute coupée : pas de bip (le mot d'éveil n'est là que pour réactiver).
                if !self.muted {
                    let rate = self.player.sample_rate();
                    self.player.play(sounds::wake(rate), rate);
                }
                self.capturing = true;
                let brain = self.brain.clone();
                self.pending_wake = Some(tokio::spawn(async move {
                    if let Err(e) = brain.request(ControlRequest::Wake).await {
                        tracing::warn!("réveil non transmis : {e}");
                    }
                }));
            }
            ListenEvent::Utterance(samples) => self.send_listen(&samples),
            ListenEvent::NoSpeech => self.send_listen(&[]),
        }
    }

    /// Envoie la phrase (vide = rien entendu) ; la réponse revient par `Internal::Heard`.
    fn send_listen(&mut self, samples: &[f32]) {
        self.capturing = false;
        let follow_up = std::mem::take(&mut self.follow_up);
        let audio = if samples.is_empty() {
            String::new()
        } else {
            let pcm: Vec<i16> = samples.iter().map(|s| f32_to_i16(*s)).collect();
            encode_pcm(&pcm)
        };
        let muted = self.muted;
        let wake = self.pending_wake.take();
        let brain = self.brain.clone();
        let tx = self.internal_tx.clone();
        tokio::spawn(async move {
            // Le réveil part toujours avant la phrase.
            if let Some(wake) = wake {
                let _ = wake.await;
            }
            let result = brain
                .request(ControlRequest::Listen {
                    audio,
                    muted,
                    follow_up,
                })
                .await;
            let _ = tx.send(Internal::Heard { result, muted });
        });
    }

    fn on_command(&mut self, command: TrayCommand, cap_tx: &mpsc::Sender<CaptureEvent>) {
        match command {
            TrayCommand::Mute if !self.hard_muted => self.set_muted(true),
            TrayCommand::Unmute if !self.hard_muted => self.set_muted(false),
            TrayCommand::HardMute if !self.hard_muted => {
                self.capture.stop();
                self.hard_muted = true;
                self.downmixer = None;
                if self.capturing {
                    // La musique avait été baissée au réveil : on la rétablit.
                    self.send_listen(&[]);
                }
                // Plus de capture en cours : pas de `Listen` orphelin à la réouverture.
                self.listener.reset();
            }
            TrayCommand::Reopen if self.hard_muted => {
                self.hard_muted = false;
                self.unavailable = false;
                if let Err(e) = self.capture.start(cap_tx.clone()) {
                    tracing::warn!("micro indisponible : {e:#}");
                    self.unavailable = true;
                }
            }
            TrayCommand::SetMode(mode) => {
                let brain = self.brain.clone();
                let tx = self.internal_tx.clone();
                self.status_in_flight = true;
                tokio::spawn(async move {
                    if let Err(e) = brain.request(ControlRequest::SetMode { mode }).await {
                        tracing::warn!("changement de mode refusé : {e}");
                    }
                    let _ = tx.send(Internal::Status(
                        brain.request(ControlRequest::Status).await,
                    ));
                });
            }
            _ => {}
        }
    }

    /// Joue le bip puis la voix de la réponse. Renvoie `false` si une nouvelle phrase est
    /// déjà en cours de capture : la réponse précédente n'est alors pas jouée (le micro
    /// capterait la voix de Jarvis).
    fn play_reply(&mut self, sound: Option<Sound>, audio: Option<SpeechAudio>) -> bool {
        if self.capturing {
            tracing::debug!("réponse non jouée : nouvelle phrase en cours");
            return false;
        }
        let rate = self.player.sample_rate();
        match sound {
            Some(Sound::Success) => self.player.play(sounds::success(rate), rate),
            Some(Sound::Failure) => self.player.play(sounds::failure(rate), rate),
            None => {}
        }
        if let Some(audio) = audio {
            match decode_pcm(&audio.pcm) {
                Ok(pcm) if audio.sample_rate > 0 => {
                    let samples = pcm.into_iter().map(i16_to_f32).collect();
                    self.player.play(samples, audio.sample_rate);
                }
                Ok(_) => tracing::warn!("voix de Jarvis sans fréquence : ignorée"),
                Err(e) => tracing::warn!("voix de Jarvis illisible : {e}"),
            }
        }
        true
    }

    /// Lecture finie et question en attente : écoute la réponse sans mot d'éveil.
    fn on_playback(&mut self) {
        if !self.awaiting_reply || *self.playing.borrow_and_update() {
            return;
        }
        self.awaiting_reply = false;
        if self.capturing || self.hard_muted || self.unavailable || self.downmixer.is_none() {
            tracing::debug!("réponse non écoutée : micro occupé ou fermé");
            return;
        }
        tracing::info!("écoute de la réponse");
        self.listener.listen_now(REPLY_WAIT);
        self.capturing = true;
        self.follow_up = true;
    }

    fn poll_status(&mut self) {
        if self.status_in_flight {
            return;
        }
        self.status_in_flight = true;
        let brain = self.brain.clone();
        let tx = self.internal_tx.clone();
        tokio::spawn(async move {
            let _ = tx.send(Internal::Status(
                brain.request(ControlRequest::Status).await,
            ));
        });
    }

    fn on_internal(&mut self, internal: Internal) {
        match internal {
            Internal::Status(result) => {
                self.status_in_flight = false;
                self.link = match result {
                    Ok(ControlResponse::Status {
                        mode, local_model, ..
                    }) => BrainLink::Connected { mode, local_model },
                    Ok(_) => return,
                    Err(_) => BrainLink::Unreachable,
                };
            }
            Internal::Heard { result, muted } => match result {
                Ok(ControlResponse::Heard {
                    text,
                    speech,
                    ear_action,
                    audio,
                    sound,
                }) => {
                    if !speech.is_empty() {
                        tracing::debug!("Jarvis : {speech}");
                    }
                    if !muted {
                        if !text.is_empty() {
                            tracing::info!("entendu : {text}");
                            self.last_heard = Some(text);
                        }
                    } else if ear_action == Some(EarAction::Unmute) {
                        // Seule la réactivation est journalisée pendant l'écoute coupée.
                        tracing::info!("entendu : {text}");
                    }
                    let played = self.play_reply(sound, audio);
                    match ear_action {
                        Some(EarAction::Mute) => self.set_muted(true),
                        Some(EarAction::Unmute) => self.set_muted(false),
                        Some(EarAction::AwaitReply) if played => {
                            self.awaiting_reply = true;
                            self.on_playback();
                        }
                        Some(EarAction::AwaitReply) | None => {}
                    }
                }
                Ok(ControlResponse::Error { message }) => {
                    tracing::warn!("le cerveau signale une erreur : {message}");
                }
                Ok(_) => {}
                Err(ClientError::BrainUnreachable) => self.link = BrainLink::Unreachable,
                Err(e) => tracing::warn!("phrase non transmise : {e}"),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::Samples;
    use jarvis_protocol::Mode;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Détecteur de mot d'éveil qui se déclenche toutes les `every` trames de 160 échantillons.
    struct FakeWake {
        every: usize,
        frame: usize,
        /// Nombre maximal de déclenchements.
        left: usize,
        score: f32,
    }
    impl WakeDetector for FakeWake {
        fn frame_len(&self) -> usize {
            160
        }
        fn process(&mut self, _: &[f32]) -> Option<f32> {
            self.frame += 1;
            if self.left > 0 && self.frame.is_multiple_of(self.every) {
                self.left -= 1;
                return Some(self.score);
            }
            None
        }
    }

    /// Détecteur de parole : parole pendant les `speech_frames` premières trames
    /// analysées, et pour toute trame plus forte que 0,5.
    struct FakeVad {
        speech_frames: usize,
        frame: usize,
    }
    impl Vad for FakeVad {
        fn frame_len(&self) -> usize {
            256
        }
        fn is_speech(&mut self, frame: &[f32]) -> bool {
            self.frame += 1;
            self.frame <= self.speech_frames || frame.iter().any(|v| v.abs() > 0.5)
        }
    }

    /// Ce que l'application a demandé au lecteur.
    #[derive(Debug, Clone, PartialEq)]
    enum Played {
        Sound { len: usize, rate: u32 },
        Stop,
    }

    /// Fréquence de sortie du faux lecteur.
    const OUT_RATE: u32 = 48_000;

    struct PlayerShared {
        calls: Mutex<Vec<Played>>,
        playing: watch::Sender<bool>,
    }
    impl PlayerShared {
        /// Fin de la lecture (le faux lecteur ne finit jamais seul).
        fn finish(&self) {
            self.playing.send_replace(false);
        }
    }

    struct FakePlayer(Arc<PlayerShared>);
    impl Player for FakePlayer {
        fn sample_rate(&self) -> u32 {
            OUT_RATE
        }
        fn play(&mut self, samples: Vec<f32>, rate: u32) {
            self.0.calls.lock().unwrap().push(Played::Sound {
                len: samples.len(),
                rate,
            });
            self.0.playing.send_replace(true);
        }
        fn stop(&mut self) {
            self.0.calls.lock().unwrap().push(Played::Stop);
            self.0.playing.send_replace(false);
        }
        fn playing(&self) -> watch::Receiver<bool> {
            self.0.playing.subscribe()
        }
    }

    fn wake_beep() -> Played {
        Played::Sound {
            len: sounds::wake(OUT_RATE).len(),
            rate: OUT_RATE,
        }
    }

    #[derive(Default)]
    struct CaptureShared {
        tx: Mutex<Option<mpsc::Sender<CaptureEvent>>>,
        starts: AtomicUsize,
        stops: AtomicUsize,
    }

    struct FakeCapture {
        shared: Arc<CaptureShared>,
        fail: bool,
    }
    impl Capture for FakeCapture {
        fn start(&mut self, tx: mpsc::Sender<CaptureEvent>) -> anyhow::Result<()> {
            self.shared.starts.fetch_add(1, Ordering::SeqCst);
            if self.fail {
                return Err(anyhow::anyhow!("aucun micro"));
            }
            *self.shared.tx.lock().unwrap() = Some(tx);
            Ok(())
        }
        fn stop(&mut self) {
            self.shared.stops.fetch_add(1, Ordering::SeqCst);
        }
    }

    type Responder = dyn Fn(&ControlRequest) -> Result<ControlResponse, ClientError> + Send + Sync;

    struct FakeBrain {
        requests: Mutex<Vec<ControlRequest>>,
        responder: Box<Responder>,
    }
    #[async_trait::async_trait]
    impl Brain for FakeBrain {
        async fn request(&self, req: ControlRequest) -> Result<ControlResponse, ClientError> {
            let answer = (self.responder)(&req);
            self.requests.lock().unwrap().push(req);
            answer
        }
    }

    struct Harness {
        capture: Arc<CaptureShared>,
        brain: Arc<FakeBrain>,
        player: Arc<PlayerShared>,
        commands: mpsc::UnboundedSender<TrayCommand>,
        tray: watch::Receiver<TrayState>,
        task: JoinHandle<anyhow::Result<()>>,
    }

    fn status_ok() -> Result<ControlResponse, ClientError> {
        Ok(ControlResponse::Status {
            mode: Mode::Local,
            version: "test".into(),
            local_model: "llama3.2:3b".into(),
        })
    }

    fn harness_speech(speech_frames: usize) -> Harness {
        build_harness(|_| status_ok(), false, speech_frames, usize::MAX, None)
    }

    /// Un seul réveil possible : une capture « orpheline » serait visible.
    fn harness_one_wake() -> Harness {
        build_harness(|_| status_ok(), false, 0, 1, None)
    }

    fn harness(
        responder: impl Fn(&ControlRequest) -> Result<ControlResponse, ClientError>
        + Send
        + Sync
        + 'static,
        capture_fails: bool,
    ) -> Harness {
        build_harness(responder, capture_fails, 0, usize::MAX, None)
    }

    fn build_harness(
        responder: impl Fn(&ControlRequest) -> Result<ControlResponse, ClientError>
        + Send
        + Sync
        + 'static,
        capture_fails: bool,
        speech_frames: usize,
        wakes: usize,
        mute_file: Option<PathBuf>,
    ) -> Harness {
        build_harness_scored(
            responder,
            capture_fails,
            speech_frames,
            wakes,
            mute_file,
            0.9,
        )
    }

    fn build_harness_scored(
        responder: impl Fn(&ControlRequest) -> Result<ControlResponse, ClientError>
        + Send
        + Sync
        + 'static,
        capture_fails: bool,
        speech_frames: usize,
        wakes: usize,
        mute_file: Option<PathBuf>,
        wake_score: f32,
    ) -> Harness {
        let shared = Arc::new(CaptureShared::default());
        let brain = Arc::new(FakeBrain {
            requests: Mutex::new(Vec::new()),
            responder: Box::new(responder),
        });
        let (commands, commands_rx) = mpsc::unbounded_channel();
        let (tray_tx, tray) = watch::channel(TrayState {
            mic: MicState::Open,
            capturing: false,
            brain: BrainLink::Unreachable,
            last_heard: None,
        });
        let listener = Listener::new(
            FakeWake {
                every: 10,
                frame: 0,
                left: wakes,
                score: wake_score,
            },
            FakeVad {
                speech_frames,
                frame: 0,
            },
        );
        let capture = FakeCapture {
            shared: shared.clone(),
            fail: capture_fails,
        };
        let player = Arc::new(PlayerShared {
            calls: Mutex::new(Vec::new()),
            playing: watch::channel(false).0,
        });
        let mut app = App::new(listener, capture, brain.clone(), commands_rx, tray_tx)
            .with_player(Box::new(FakePlayer(player.clone())));
        if let Some(path) = mute_file {
            app = app.with_mute_file(path);
        }
        Harness {
            capture: shared,
            brain,
            player,
            commands,
            tray,
            task: tokio::spawn(app.run()),
        }
    }

    impl Harness {
        async fn wait_for(&self, what: &str, cond: impl Fn(&Harness) -> bool) {
            for _ in 0..300 {
                if cond(self) {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            panic!("délai dépassé : {what}");
        }

        async fn feed(&self, samples: Vec<f32>) {
            let tx = self
                .capture
                .tx
                .lock()
                .unwrap()
                .clone()
                .expect("capture démarrée");
            tx.send(CaptureEvent::Format {
                rate: 16_000,
                channels: 1,
            })
            .await
            .unwrap();
            for chunk in samples.chunks(1_600) {
                tx.send(CaptureEvent::Samples(Samples::detached(chunk.to_vec())))
                    .await
                    .unwrap();
            }
        }

        fn requests(&self) -> Vec<ControlRequest> {
            self.brain.requests.lock().unwrap().clone()
        }

        fn played(&self) -> Vec<Played> {
            self.player.calls.lock().unwrap().clone()
        }

        /// Les `Listen` envoyés : (audio, follow_up).
        fn follow_ups(&self) -> Vec<(String, bool)> {
            self.requests()
                .into_iter()
                .filter_map(|r| match r {
                    ControlRequest::Listen {
                        audio, follow_up, ..
                    } => Some((audio, follow_up)),
                    _ => None,
                })
                .collect()
        }

        fn wakes(&self) -> usize {
            self.requests()
                .iter()
                .filter(|r| **r == ControlRequest::Wake)
                .count()
        }

        fn listens(&self) -> Vec<bool> {
            self.requests()
                .into_iter()
                .filter_map(|r| match r {
                    ControlRequest::Listen { muted, .. } => Some(muted),
                    _ => None,
                })
                .collect()
        }
    }

    /// Réveil à la trame 10 (1 600 échantillons), puis 4 s sans parole (marge comprise).
    fn wake_then_silence() -> Vec<f32> {
        vec![0.0; 65_900]
    }

    #[tokio::test]
    async fn cerveau_absent_icone_injoignable() {
        let h = harness(|_| Err(ClientError::BrainUnreachable), false);
        h.wait_for("statut interrogé", |h| {
            h.requests().contains(&ControlRequest::Status)
        })
        .await;
        h.wait_for("icône injoignable", |h| {
            h.tray.borrow().brain == BrainLink::Unreachable
        })
        .await;
        assert_eq!(h.tray.borrow().mic, MicState::Open);
    }

    #[tokio::test]
    async fn cerveau_joignable_affiche_le_mode() {
        let h = harness(|_| status_ok(), false);
        h.wait_for("lien établi", |h| {
            matches!(h.tray.borrow().brain, BrainLink::Connected { .. })
        })
        .await;
    }

    #[tokio::test]
    async fn capture_absente_etat_indisponible() {
        let h = harness(|_| status_ok(), true);
        h.wait_for("micro indisponible", |h| {
            h.tray.borrow().mic == MicState::Unavailable
        })
        .await;
    }

    #[tokio::test]
    async fn perte_du_micro_puis_retour() {
        let h = harness(|_| status_ok(), false);
        h.wait_for("capture", |h| h.capture.tx.lock().unwrap().is_some())
            .await;
        let tx = h.capture.tx.lock().unwrap().clone().unwrap();
        tx.send(CaptureEvent::Unavailable).await.unwrap();
        h.wait_for("indisponible", |h| {
            h.tray.borrow().mic == MicState::Unavailable
        })
        .await;
        tx.send(CaptureEvent::Format {
            rate: 16_000,
            channels: 1,
        })
        .await
        .unwrap();
        h.wait_for("rétabli", |h| h.tray.borrow().mic == MicState::Open)
            .await;
    }

    #[tokio::test]
    async fn reveil_envoie_wake_puis_listen() {
        let h = harness(
            |r| match r {
                ControlRequest::Listen { .. } => Ok(ControlResponse::Heard {
                    text: "quelle heure est-il".into(),
                    speech: "Il est midi.".into(),
                    ear_action: None,
                    audio: None,
                    sound: None,
                }),
                _ => status_ok(),
            },
            false,
        );
        h.wait_for("capture", |h| h.capture.tx.lock().unwrap().is_some())
            .await;
        h.feed(wake_then_silence()).await;
        h.wait_for("phrase entendue", |h| {
            h.tray.borrow().last_heard.as_deref() == Some("quelle heure est-il")
        })
        .await;
        let sent: Vec<ControlRequest> = h
            .requests()
            .into_iter()
            .filter(|r| !matches!(r, ControlRequest::Status))
            .collect();
        assert_eq!(sent.len(), 2, "{sent:?}");
        assert_eq!(sent[0], ControlRequest::Wake);
        assert!(matches!(
            &sent[1],
            ControlRequest::Listen { muted: false, audio, .. } if audio.is_empty()
        ));
    }

    #[tokio::test]
    async fn action_mute_appliquee() {
        let h = harness(
            |r| match r {
                ControlRequest::Listen { .. } => Ok(ControlResponse::Heard {
                    text: "coupe l'écoute".into(),
                    speech: "D'accord.".into(),
                    ear_action: Some(EarAction::Mute),
                    audio: None,
                    sound: None,
                }),
                _ => status_ok(),
            },
            false,
        );
        h.wait_for("capture", |h| h.capture.tx.lock().unwrap().is_some())
            .await;
        h.feed(wake_then_silence()).await;
        h.wait_for("écoute coupée", |h| {
            h.tray.borrow().mic == MicState::Muted
        })
        .await;
        // La phrase suivante part marquée « muted » et n'alimente pas « dernière phrase ».
        h.feed(wake_then_silence()).await;
        h.wait_for("deuxième phrase", |h| h.listens().len() == 2)
            .await;
        assert_eq!(h.listens(), vec![false, true]);
        assert_eq!(
            h.tray.borrow().last_heard.as_deref(),
            Some("coupe l'écoute")
        );
    }

    #[tokio::test]
    async fn pas_de_derniere_phrase_pendant_l_ecoute_coupee() {
        let h = harness(
            |r| match r {
                ControlRequest::Listen { .. } => Ok(ControlResponse::Heard {
                    text: "secret".into(),
                    speech: String::new(),
                    ear_action: None,
                    audio: None,
                    sound: None,
                }),
                _ => status_ok(),
            },
            false,
        );
        h.commands.send(TrayCommand::Mute).unwrap();
        h.wait_for("coupée", |h| h.tray.borrow().mic == MicState::Muted)
            .await;
        h.wait_for("capture", |h| h.capture.tx.lock().unwrap().is_some())
            .await;
        h.feed(wake_then_silence()).await;
        h.wait_for("phrase envoyée", |h| h.listens() == vec![true])
            .await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(h.tray.borrow().last_heard, None);
    }

    #[tokio::test]
    async fn coupure_totale_arrete_la_capture() {
        let h = harness(|_| status_ok(), false);
        h.wait_for("capture démarrée", |h| {
            h.capture.starts.load(Ordering::SeqCst) == 1
        })
        .await;
        h.commands.send(TrayCommand::HardMute).unwrap();
        h.wait_for("flux fermé", |h| {
            h.capture.stops.load(Ordering::SeqCst) == 1
        })
        .await;
        h.wait_for("icône", |h| h.tray.borrow().mic == MicState::HardMuted)
            .await;
        h.commands.send(TrayCommand::Reopen).unwrap();
        h.wait_for("flux rouvert", |h| {
            h.capture.starts.load(Ordering::SeqCst) == 2
        })
        .await;
        h.wait_for("icône ouverte", |h| h.tray.borrow().mic == MicState::Open)
            .await;
        h.commands.send(TrayCommand::Quit).unwrap();
        h.task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn coupure_totale_pendant_la_capture_puis_reouverture_sans_listen_orphelin() {
        let h = harness_one_wake();
        h.wait_for("capture", |h| h.capture.tx.lock().unwrap().is_some())
            .await;
        // Réveil seul : la capture est en cours (aucune parole, pas encore d'abandon).
        h.feed(vec![0.0; 3_200]).await;
        h.wait_for("réveil", |h| h.requests().contains(&ControlRequest::Wake))
            .await;
        h.commands.send(TrayCommand::HardMute).unwrap();
        // Un `Listen` vide rétablit la musique.
        h.wait_for("listen vide", |h| h.listens() == vec![false])
            .await;
        h.commands.send(TrayCommand::Reopen).unwrap();
        h.wait_for("flux rouvert", |h| {
            h.capture.starts.load(Ordering::SeqCst) == 2
        })
        .await;
        // Plus de 4 s de silence : sans réinitialisation, l'ancienne capture enverrait un
        // second `Listen` (abandon faute de parole).
        h.feed(vec![0.0; 70_000]).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(h.listens(), vec![false]);
    }

    #[tokio::test]
    async fn perte_du_micro_pendant_la_capture_retablit_la_musique() {
        let h = harness_one_wake();
        h.wait_for("capture", |h| h.capture.tx.lock().unwrap().is_some())
            .await;
        h.feed(vec![0.0; 3_200]).await;
        h.wait_for("réveil", |h| h.requests().contains(&ControlRequest::Wake))
            .await;
        let tx = h.capture.tx.lock().unwrap().clone().unwrap();
        tx.send(CaptureEvent::Unavailable).await.unwrap();
        h.wait_for("listen vide", |h| h.listens() == vec![false])
            .await;
        h.wait_for("indisponible", |h| {
            h.tray.borrow().mic == MicState::Unavailable
        })
        .await;
        // Micro de retour : pas de second `Listen` issu de l'ancienne capture.
        h.feed(vec![0.0; 70_000]).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(h.listens(), vec![false]);
        assert!(!h.tray.borrow().capturing);
    }

    #[tokio::test]
    async fn quitter_pendant_la_capture_retablit_la_musique() {
        let h = harness_one_wake();
        h.wait_for("capture", |h| h.capture.tx.lock().unwrap().is_some())
            .await;
        // Réveil seul : la capture est en cours.
        h.feed(vec![0.0; 3_200]).await;
        h.wait_for("capture en cours", |h| h.tray.borrow().capturing)
            .await;
        h.commands.send(TrayCommand::Quit).unwrap();
        let brain = h.brain.clone();
        h.task.await.unwrap().unwrap();
        // Le `Listen` vide est parti avant la fin de `run`, pas dans une tâche détachée.
        let sent: Vec<ControlRequest> = brain
            .requests
            .lock()
            .unwrap()
            .clone()
            .into_iter()
            .filter(|r| !matches!(r, ControlRequest::Status))
            .collect();
        assert_eq!(sent.len(), 2, "{sent:?}");
        assert_eq!(sent[0], ControlRequest::Wake);
        assert!(matches!(
            &sent[1],
            ControlRequest::Listen { muted: false, audio, .. } if audio.is_empty()
        ));
    }

    #[tokio::test]
    async fn quitter_hors_capture_n_envoie_pas_de_listen() {
        let h = harness_one_wake();
        h.wait_for("capture", |h| h.capture.tx.lock().unwrap().is_some())
            .await;
        h.commands.send(TrayCommand::Quit).unwrap();
        let brain = h.brain.clone();
        h.task.await.unwrap().unwrap();
        let requests = brain.requests.lock().unwrap().clone();
        assert!(
            !requests
                .iter()
                .any(|r| matches!(r, ControlRequest::Listen { .. })),
            "{requests:?}"
        );
    }

    fn harness_mute_file(path: &std::path::Path) -> Harness {
        build_harness(
            |_| status_ok(),
            false,
            0,
            usize::MAX,
            Some(path.to_path_buf()),
        )
    }

    #[tokio::test]
    async fn ecoute_coupee_retrouvee_au_redemarrage() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jarvis/ear-muted");
        let h = harness_mute_file(&path);
        h.wait_for("ouverte", |h| h.tray.borrow().mic == MicState::Open)
            .await;
        h.commands.send(TrayCommand::Mute).unwrap();
        h.wait_for("coupée", |h| h.tray.borrow().mic == MicState::Muted)
            .await;
        assert!(path.exists());
        h.commands.send(TrayCommand::Quit).unwrap();
        h.task.await.unwrap().unwrap();

        // Redémarrage : toujours coupée.
        let h = harness_mute_file(&path);
        h.wait_for("coupée au démarrage", |h| {
            h.tray.borrow().mic == MicState::Muted
        })
        .await;
        h.commands.send(TrayCommand::Unmute).unwrap();
        h.wait_for("rouverte", |h| h.tray.borrow().mic == MicState::Open)
            .await;
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn coupure_par_la_voix_aussi_retenue() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ear-muted");
        let h = build_harness(
            |r| match r {
                ControlRequest::Listen { .. } => Ok(ControlResponse::Heard {
                    text: "coupe l'écoute".into(),
                    speech: "D'accord.".into(),
                    ear_action: Some(EarAction::Mute),
                    audio: None,
                    sound: None,
                }),
                _ => status_ok(),
            },
            false,
            0,
            usize::MAX,
            Some(path.clone()),
        );
        h.wait_for("capture", |h| h.capture.tx.lock().unwrap().is_some())
            .await;
        h.feed(wake_then_silence()).await;
        h.wait_for("coupée", |h| h.tray.borrow().mic == MicState::Muted)
            .await;
        assert!(path.exists());
    }

    #[tokio::test]
    async fn coupure_totale_non_retenue() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ear-muted");
        let h = harness_mute_file(&path);
        h.commands.send(TrayCommand::HardMute).unwrap();
        h.wait_for("coupure totale", |h| {
            h.tray.borrow().mic == MicState::HardMuted
        })
        .await;
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn listen_muet_porte_l_audio_de_la_phrase() {
        // Parole pendant 20 trames de VAD après le réveil, puis silence.
        let h = harness_speech(20);
        h.commands.send(TrayCommand::Mute).unwrap();
        h.wait_for("coupée", |h| h.tray.borrow().mic == MicState::Muted)
            .await;
        h.wait_for("capture", |h| h.capture.tx.lock().unwrap().is_some())
            .await;
        h.feed(vec![0.1; 1_600 + 30_000]).await;
        h.wait_for("phrase envoyée", |h| !h.listens().is_empty())
            .await;
        let audio = h
            .requests()
            .into_iter()
            .find_map(|r| match r {
                ControlRequest::Listen {
                    audio, muted: true, ..
                } => Some(audio),
                _ => None,
            })
            .expect("Listen muted");
        assert!(!audio.is_empty());
    }

    /// Voix de Jarvis : 2 205 échantillons à 22,05 kHz (0,1 s).
    fn voice() -> Option<SpeechAudio> {
        Some(SpeechAudio {
            pcm: encode_pcm(&[1_000; 2_205]),
            sample_rate: 22_050,
        })
    }

    fn voice_played() -> Played {
        Played::Sound {
            len: 2_205,
            rate: 22_050,
        }
    }

    fn heard(
        text: &str,
        ear_action: Option<EarAction>,
        sound: Option<Sound>,
    ) -> Result<ControlResponse, ClientError> {
        Ok(ControlResponse::Heard {
            text: text.into(),
            speech: "Bien.".into(),
            ear_action,
            audio: voice(),
            sound,
        })
    }

    #[tokio::test]
    async fn reveil_joue_le_bip() {
        let h = harness_one_wake();
        h.wait_for("capture", |h| h.capture.tx.lock().unwrap().is_some())
            .await;
        h.feed(vec![0.0; 3_200]).await;
        h.wait_for("bip", |h| h.played() == vec![wake_beep()]).await;
        assert!(h.tray.borrow().capturing);
    }

    #[tokio::test]
    async fn ecoute_coupee_sans_bip_de_reveil() {
        let h = harness_one_wake();
        h.commands.send(TrayCommand::Mute).unwrap();
        h.wait_for("coupée", |h| h.tray.borrow().mic == MicState::Muted)
            .await;
        h.wait_for("capture", |h| h.capture.tx.lock().unwrap().is_some())
            .await;
        h.feed(vec![0.0; 3_200]).await;
        h.wait_for("réveil", |h| h.tray.borrow().capturing).await;
        assert!(h.played().is_empty(), "{:?}", h.played());
    }

    #[tokio::test]
    async fn reponse_jouee_apres_le_bip() {
        let h = harness(
            |r| match r {
                ControlRequest::Listen { .. } => heard("suivant", None, Some(Sound::Success)),
                _ => status_ok(),
            },
            false,
        );
        h.wait_for("capture", |h| h.capture.tx.lock().unwrap().is_some())
            .await;
        h.feed(wake_then_silence()).await;
        let success = Played::Sound {
            len: sounds::success(OUT_RATE).len(),
            rate: OUT_RATE,
        };
        h.wait_for("réponse jouée", |h| h.played().len() == 3).await;
        assert_eq!(h.played(), vec![wake_beep(), success, voice_played()]);
    }

    #[tokio::test]
    async fn reveil_coupe_la_lecture() {
        let h = build_harness(
            |r| match r {
                ControlRequest::Listen { .. } => heard("quelle heure est-il", None, None),
                _ => status_ok(),
            },
            false,
            0,
            2,
            None,
        );
        h.wait_for("capture", |h| h.capture.tx.lock().unwrap().is_some())
            .await;
        h.feed(wake_then_silence()).await;
        h.wait_for("voix jouée", |h| {
            h.played() == vec![wake_beep(), voice_played()]
        })
        .await;
        // « Jarvis » pendant que Jarvis parle : coupure, bip, puis capture normale.
        h.feed(wake_then_silence()).await;
        h.wait_for("seconde phrase", |h| h.listens().len() == 2)
            .await;
        assert_eq!(
            h.played()[..4],
            [wake_beep(), voice_played(), Played::Stop, wake_beep()]
        );
        assert_eq!(h.wakes(), 2);
    }

    /// Première phrase : le cerveau pose une question ; réponse « oui » ensuite.
    fn harness_question() -> Harness {
        build_harness(
            |r| match r {
                ControlRequest::Listen {
                    follow_up: false, ..
                } => heard("mets le PC en veille", Some(EarAction::AwaitReply), None),
                ControlRequest::Listen { .. } => Ok(ControlResponse::Heard {
                    text: "oui".into(),
                    speech: String::new(),
                    ear_action: None,
                    audio: None,
                    sound: None,
                }),
                _ => status_ok(),
            },
            false,
            0,
            1,
            None,
        )
    }

    /// Question posée et lue ; la lecture n'est pas encore finie.
    async fn ask(h: &Harness) {
        h.wait_for("capture", |h| h.capture.tx.lock().unwrap().is_some())
            .await;
        h.feed(wake_then_silence()).await;
        h.wait_for("question lue", |h| {
            h.played() == vec![wake_beep(), voice_played()]
        })
        .await;
        // Pas d'écoute tant que Jarvis parle.
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!h.tray.borrow().capturing);
        h.player.finish();
        h.wait_for("écoute de la réponse", |h| h.tray.borrow().capturing)
            .await;
    }

    #[tokio::test]
    async fn attente_de_reponse_capture_sans_mot_d_eveil() {
        let h = harness_question();
        ask(&h).await;
        // Garde du début d'écoute, « oui » (20 trames de parole), puis le silence qui
        // clôt la phrase.
        let mut reply = vec![0.0; 256 * 20];
        reply.extend(vec![0.9; 256 * 20]);
        reply.extend(vec![0.0; 256 * 60]);
        h.feed(reply).await;
        h.wait_for("réponse entendue", |h| {
            h.tray.borrow().last_heard.as_deref() == Some("oui")
        })
        .await;
        let listens = h.follow_ups();
        assert_eq!(listens.len(), 2, "{listens:?}");
        assert!(!listens[0].1);
        assert!(listens[1].1, "réponse marquée follow_up");
        assert!(!listens[1].0.is_empty());
        // Aucun réveil (donc aucun son baissé) pour la réponse.
        assert_eq!(h.wakes(), 1);
        assert!(!h.tray.borrow().capturing);
    }

    #[tokio::test]
    async fn attente_sans_parole_abandonnee() {
        let h = harness_question();
        ask(&h).await;
        // 5 s de silence : on attend toujours (le délai est de 8 s, pas 4).
        h.feed(vec![0.0; 5 * 16_000]).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(h.follow_ups().len(), 1);
        assert!(h.tray.borrow().capturing);
        // Au-delà de 8 s : abandon, signalé par une réponse vide.
        h.feed(vec![0.0; 3 * 16_000 + 1_000]).await;
        h.wait_for("abandon", |h| h.follow_ups().len() == 2).await;
        assert_eq!(h.follow_ups()[1], (String::new(), true));
        h.wait_for("fin de capture", |h| !h.tray.borrow().capturing)
            .await;
        assert_eq!(h.wakes(), 1);
    }

    #[tokio::test]
    async fn reveil_faible_ignore_pendant_que_jarvis_parle() {
        // Score au-dessus du seuil normal (0,5), sous le seuil de lecture.
        let h = build_harness_scored(|_| status_ok(), false, 0, usize::MAX, None, 0.55);
        h.wait_for("capture", |h| h.capture.tx.lock().unwrap().is_some())
            .await;
        // Jarvis parle : sa propre voix dit « Jarvis ».
        h.player.playing.send_replace(true);
        h.feed(vec![0.0; 3_200]).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(h.wakes(), 0);
        assert!(
            h.played().is_empty(),
            "lecture non coupée : {:?}",
            h.played()
        );
        assert!(!h.tray.borrow().capturing);
        // Lecture finie : le même score réveille.
        h.player.finish();
        h.feed(vec![0.0; 3_200]).await;
        h.wait_for("réveil", |h| h.wakes() == 1).await;
    }

    #[tokio::test]
    async fn reveil_franc_coupe_toujours_la_lecture() {
        let h = build_harness_scored(|_| status_ok(), false, 0, 1, None, 0.9);
        h.wait_for("capture", |h| h.capture.tx.lock().unwrap().is_some())
            .await;
        h.player.playing.send_replace(true);
        h.feed(vec![0.0; 3_200]).await;
        h.wait_for("réveil", |h| h.wakes() == 1).await;
        assert_eq!(h.played(), vec![Played::Stop, wake_beep()]);
    }

    #[tokio::test]
    async fn question_sans_lecture_ecoute_tout_de_suite() {
        let h = build_harness(
            |r| match r {
                ControlRequest::Listen {
                    follow_up: false, ..
                } => Ok(ControlResponse::Heard {
                    text: "mets le PC en veille".into(),
                    speech: "Je mets le PC en veille ?".into(),
                    ear_action: Some(EarAction::AwaitReply),
                    audio: None,
                    sound: None,
                }),
                _ => status_ok(),
            },
            false,
            0,
            1,
            None,
        );
        h.wait_for("capture", |h| h.capture.tx.lock().unwrap().is_some())
            .await;
        h.feed(wake_then_silence()).await;
        h.wait_for("bip lu", |h| h.played() == vec![wake_beep()])
            .await;
        h.player.finish();
        // Voix absente : l'écoute de la réponse commence dès la fin du bip.
        h.wait_for("écoute de la réponse", |h| h.tray.borrow().capturing)
            .await;
    }
}
