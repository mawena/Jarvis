//! Lien avec le cerveau : connexion, `hello`, reconnexion avec délai croissant,
//! battement de cœur. Hors connexion, rien n'est envoyé ni gardé.

use super::backoff::Backoff;
use super::store::{Pairing, PairingStore, remember};
use super::{AsyncStream, Connector, Discovery};
use futures_util::stream::SplitSink;
use futures_util::{SinkExt, StreamExt};
use jarvis_protocol::EarId;
use jarvis_protocol::ear::{BrainMessage, EarMessage, PROTOCOL_VERSION, RejectReason, StatusInfo};
use jarvis_protocol::frame::{MAX_FRAME_SAMPLES, encode_mic_frame};
use std::collections::HashSet;
use std::fmt;
use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::{Instant, MissedTickBehavior, timeout};
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::{self, Message};
use tokio_tungstenite::{WebSocketStream, client_async_with_config};

/// Ping toutes les 5 s.
pub const DEFAULT_HEARTBEAT: Duration = Duration::from_secs(5);
/// Rien reçu pendant 15 s : connexion tenue pour morte.
pub const DEFAULT_DEAD_AFTER: Duration = Duration::from_secs(15);

/// Délai de chaque étape de la connexion (TCP et TLS, WebSocket, `welcome`).
const STEP_TIMEOUT: Duration = Duration::from_secs(5);
/// Attente de la découverte mDNS.
const DISCOVERY_WAIT: Duration = Duration::from_secs(2);
/// Envoi des derniers messages à la fermeture.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(2);
/// Réponse du cerveau à `pair`.
const PAIR_TIMEOUT: Duration = Duration::from_secs(10);
/// Messages en route vers le cerveau (~25 s de son), perdus si la connexion tombe.
const OUT_CAPACITY: usize = 256;
/// Message reçu ≤ 8 Mio (voix de Jarvis).
const MAX_MESSAGE: usize = 8 << 20;
/// Adresse du WebSocket (l'hôte n'est jamais vérifié : le certificat est épinglé).
const URL: &str = "wss://jarvis-brain/";

type Ws = WebSocketStream<Box<dyn AsyncStream>>;

pub struct LinkConfig {
    pub pairing: Pairing,
    pub store: PairingStore,
    pub connector: Arc<dyn Connector>,
    pub discovery: Option<Arc<dyn Discovery>>,
    pub heartbeat: Duration,
    pub dead_after: Duration,
}

impl LinkConfig {
    /// Battement de cœur et délai de mort par défaut.
    pub fn new(
        pairing: Pairing,
        store: PairingStore,
        connector: Arc<dyn Connector>,
        discovery: Option<Arc<dyn Discovery>>,
    ) -> Self {
        LinkConfig {
            pairing,
            store,
            connector,
            discovery,
            heartbeat: DEFAULT_HEARTBEAT,
            dead_after: DEFAULT_DEAD_AFTER,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum LinkState {
    Connecting,
    Connected(StatusInfo),
    Unreachable,
    Rejected(RejectReason),
}

#[derive(Debug, Clone, PartialEq)]
pub enum LinkEvent {
    Connected(StatusInfo),
    Disconnected,
    /// Message du cerveau (sauf `OfflinePhrase`, enregistrée par le lien).
    Message(BrainMessage),
}

/// Pas de connexion : rien n'est envoyé, rien n'est gardé.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkDown;

impl fmt::Display for LinkDown {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("cerveau injoignable")
    }
}

impl std::error::Error for LinkDown {}

enum Cmd {
    Nudge,
    Close(oneshot::Sender<()>),
}

/// Envoi vers la connexion en cours ; `None` hors connexion.
type Outbox = Arc<Mutex<Option<mpsc::Sender<Message>>>>;

pub struct EarLink {
    out: Outbox,
    cmds: mpsc::Sender<Cmd>,
    state: watch::Receiver<LinkState>,
}

impl EarLink {
    /// Lance le lien (dans le runtime tokio courant). Il s'arrête quand l'`EarLink`
    /// est lâché (ou par [`EarLink::close`]).
    pub fn spawn(config: LinkConfig) -> (EarLink, mpsc::Receiver<LinkEvent>) {
        let out: Outbox = Arc::new(Mutex::new(None));
        let (cmd_tx, cmd_rx) = mpsc::channel(8);
        let (state_tx, state_rx) = watch::channel(LinkState::Connecting);
        let (event_tx, event_rx) = mpsc::channel(64);
        let addresses = config.pairing.addresses.clone();
        let task = LinkTask {
            config,
            out: out.clone(),
            cmds: cmd_rx,
            state: state_tx,
            events: event_tx,
            backoff: Backoff::new(),
            addresses,
        };
        tokio::spawn(task.run());
        let link = EarLink {
            out,
            cmds: cmd_tx,
            state: state_rx,
        };
        (link, event_rx)
    }

    pub fn send(&self, message: EarMessage) -> Result<(), LinkDown> {
        let text = serde_json::to_string(&message).map_err(|_| LinkDown)?;
        self.push(Message::text(text))
    }

    /// Jamais mis en attente hors connexion : échoue tout de suite.
    pub fn send_audio(&self, utterance: u32, samples: &[i16]) -> Result<(), LinkDown> {
        if samples.is_empty() {
            return if self.is_connected() {
                Ok(())
            } else {
                Err(LinkDown)
            };
        }
        for chunk in samples.chunks(MAX_FRAME_SAMPLES) {
            self.push(Message::binary(encode_mic_frame(utterance, chunk)))?;
        }
        Ok(())
    }

    /// Nouvelle tentative de connexion, tout de suite.
    pub fn nudge(&self) {
        let _ = self.cmds.try_send(Cmd::Nudge);
    }

    pub fn state(&self) -> watch::Receiver<LinkState> {
        self.state.clone()
    }

    /// Arrêt propre : les messages déjà acceptés partent (2 s au plus), puis la
    /// connexion est fermée.
    pub async fn close(&self) {
        let (tx, rx) = oneshot::channel();
        if self.cmds.send(Cmd::Close(tx)).await.is_ok() {
            let _ = timeout(CLOSE_TIMEOUT * 2, rx).await;
        }
    }

    fn is_connected(&self) -> bool {
        self.out.lock().unwrap_or_else(|e| e.into_inner()).is_some()
    }

    fn push(&self, message: Message) -> Result<(), LinkDown> {
        let out = self.out.lock().unwrap_or_else(|e| e.into_inner());
        // File pleine : le cerveau ne suit plus, rien n'est gardé de plus.
        out.as_ref()
            .ok_or(LinkDown)?
            .try_send(message)
            .map_err(|_| LinkDown)
    }
}

/// Liaison avec le cerveau, vue de la session de l'oreille (vrai [`EarLink`], lien
/// supervisé du PC, faux lien dans les tests).
#[async_trait::async_trait]
pub trait Link: Send + Sync + 'static {
    /// Échoue tout de suite hors connexion : rien n'est gardé.
    fn send(&self, message: EarMessage) -> Result<(), LinkDown>;
    /// Son du micro ; jamais mis en attente hors connexion.
    fn send_audio(&self, utterance: u32, samples: &[i16]) -> Result<(), LinkDown>;
    /// Nouvelle tentative de connexion, tout de suite.
    fn nudge(&self);
    fn state(&self) -> watch::Receiver<LinkState>;
    /// Envoie ce qui est déjà accepté, puis ferme.
    async fn close(&self);
}

#[async_trait::async_trait]
impl Link for EarLink {
    fn send(&self, message: EarMessage) -> Result<(), LinkDown> {
        EarLink::send(self, message)
    }
    fn send_audio(&self, utterance: u32, samples: &[i16]) -> Result<(), LinkDown> {
        EarLink::send_audio(self, utterance, samples)
    }
    fn nudge(&self) {
        EarLink::nudge(self)
    }
    fn state(&self) -> watch::Receiver<LinkState> {
        EarLink::state(self)
    }
    async fn close(&self) {
        EarLink::close(self).await
    }
}

/// Issue d'une tentative de connexion.
enum Attempt {
    Connected(Box<Ws>, StatusInfo, SocketAddr),
    Rejected(RejectReason),
    Unreachable,
}

/// Fin d'une session.
enum End {
    Lost,
    Quit(Option<oneshot::Sender<()>>),
}

struct LinkTask {
    config: LinkConfig,
    out: Outbox,
    cmds: mpsc::Receiver<Cmd>,
    state: watch::Sender<LinkState>,
    events: mpsc::Sender<LinkEvent>,
    backoff: Backoff,
    /// Dernières adresses connues, la plus récente d'abord.
    addresses: Vec<SocketAddr>,
}

impl LinkTask {
    async fn run(mut self) {
        loop {
            let attempt = {
                let connecting = connect(&self.config, &self.addresses);
                tokio::pin!(connecting);
                loop {
                    tokio::select! {
                        attempt = &mut connecting => break attempt,
                        cmd = self.cmds.recv() => match cmd {
                            // Déjà en train d'essayer.
                            Some(Cmd::Nudge) => {}
                            Some(Cmd::Close(reply)) => {
                                let _ = reply.send(());
                                return;
                            }
                            None => return,
                        },
                    }
                }
            };
            match attempt {
                Attempt::Connected(ws, status, addr) => {
                    self.backoff.reset();
                    self.remember(addr).await;
                    tracing::info!("connecté au cerveau ({addr})");
                    match self.session(*ws, status).await {
                        End::Quit(reply) => {
                            if let Some(reply) = reply {
                                let _ = reply.send(());
                            }
                            return;
                        }
                        End::Lost => {
                            tracing::info!("connexion au cerveau perdue");
                            self.state.send_replace(LinkState::Connecting);
                            let _ = self.events.send(LinkEvent::Disconnected).await;
                        }
                    }
                }
                Attempt::Rejected(reason) => {
                    tracing::warn!("cerveau : connexion refusée ({reason:?})");
                    // Inutile d'insister : il faut réappairer (ou mettre à jour).
                    self.backoff.rejected();
                    self.state.send_replace(LinkState::Rejected(reason));
                }
                Attempt::Unreachable => {
                    tracing::debug!("cerveau injoignable");
                    self.state.send_replace(LinkState::Unreachable);
                }
            }
            let delay = self.backoff.next_delay();
            tokio::select! {
                () = tokio::time::sleep(delay) => {}
                cmd = self.cmds.recv() => match cmd {
                    Some(Cmd::Nudge) => tracing::debug!("nouvelle tentative demandée"),
                    Some(Cmd::Close(reply)) => {
                        let _ = reply.send(());
                        return;
                    }
                    None => return,
                },
            }
        }
    }

    /// Retient une adresse qui a répondu (en mémoire et sur disque).
    async fn remember(&mut self, addr: SocketAddr) {
        if self.addresses.first() == Some(&addr) {
            return;
        }
        remember(&mut self.addresses, addr);
        let store = self.config.store.clone();
        match tokio::task::spawn_blocking(move || store.remember_address(addr)).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => tracing::warn!("adresse du cerveau non enregistrée : {e}"),
            Err(e) => tracing::warn!("adresse du cerveau non enregistrée : {e}"),
        }
    }

    async fn session(&mut self, ws: Ws, status: StatusInfo) -> End {
        let (sink, mut stream) = ws.split();
        let (tx, rx) = mpsc::channel(OUT_CAPACITY);
        *self.out.lock().unwrap_or_else(|e| e.into_inner()) = Some(tx.clone());
        let mut writer = tokio::spawn(write_loop(sink, rx, self.config.dead_after));
        self.state
            .send_replace(LinkState::Connected(status.clone()));
        let _ = self.events.send(LinkEvent::Connected(status)).await;

        let mut beat = tokio::time::interval(self.config.heartbeat);
        beat.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let mut last_seen = Instant::now();
        let end = loop {
            tokio::select! {
                message = stream.next() => match message {
                    Some(Ok(message)) => {
                        last_seen = Instant::now();
                        match message {
                            Message::Text(text) => self.on_text(text.as_str()).await,
                            Message::Close(_) => break End::Lost,
                            // Pong rendu par tungstenite ; trames binaires inconnues en v1.
                            _ => {}
                        }
                    }
                    Some(Err(e)) => {
                        tracing::debug!("lecture du cerveau : {e}");
                        break End::Lost;
                    }
                    None => break End::Lost,
                },
                _ = beat.tick() => {
                    if last_seen.elapsed() >= self.config.dead_after {
                        tracing::warn!("cerveau muet depuis {:?} : connexion tenue pour morte",
                            self.config.dead_after);
                        break End::Lost;
                    }
                    let _ = tx.try_send(Message::Ping(Default::default()));
                }
                _ = &mut writer => break End::Lost,
                cmd = self.cmds.recv() => match cmd {
                    Some(Cmd::Nudge) => {}
                    Some(Cmd::Close(reply)) => break End::Quit(Some(reply)),
                    None => break End::Quit(None),
                },
            }
        };
        // Plus rien n'est accepté ; ce qui n'est pas parti est perdu (sauf à l'arrêt).
        self.out.lock().unwrap_or_else(|e| e.into_inner()).take();
        drop(tx);
        match end {
            End::Quit(_) if !writer.is_finished() => {
                if timeout(CLOSE_TIMEOUT, &mut writer).await.is_err() {
                    writer.abort();
                }
            }
            _ => writer.abort(),
        }
        end
    }

    async fn on_text(&mut self, text: &str) {
        let message = match serde_json::from_str::<BrainMessage>(text) {
            Ok(message) => message,
            Err(e) => {
                // Ni le contenu (voix, jeton) ni le détail de l'erreur.
                tracing::warn!("message du cerveau illisible ({:?})", e.classify());
                return;
            }
        };
        match message {
            BrainMessage::OfflinePhrase { id, audio } => {
                let store = self.config.store.clone();
                let saved =
                    tokio::task::spawn_blocking(move || store.save_offline_phrase(&id, &audio))
                        .await;
                match saved {
                    Ok(Ok(())) => tracing::info!("phrase hors ligne enregistrée"),
                    Ok(Err(e)) => tracing::warn!("phrase hors ligne non enregistrée : {e}"),
                    Err(e) => tracing::warn!("phrase hors ligne non enregistrée : {e}"),
                }
            }
            BrainMessage::Unknown => tracing::debug!("message inconnu du cerveau ignoré"),
            BrainMessage::Status(status) => {
                self.state
                    .send_replace(LinkState::Connected(status.clone()));
                let _ = self
                    .events
                    .send(LinkEvent::Message(BrainMessage::Status(status)))
                    .await;
            }
            other => {
                let _ = self.events.send(LinkEvent::Message(other)).await;
            }
        }
    }
}

/// Écrit les messages de la session ; s'arrête si un envoi bloque `dead_after`.
async fn write_loop(
    mut sink: SplitSink<Ws, Message>,
    mut rx: mpsc::Receiver<Message>,
    dead_after: Duration,
) {
    while let Some(message) = rx.recv().await {
        match timeout(dead_after, sink.send(message)).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                tracing::debug!("écriture vers le cerveau : {e}");
                return;
            }
            Err(_) => {
                tracing::warn!("écriture vers le cerveau bloquée");
                return;
            }
        }
    }
    let _ = sink.close().await;
}

fn ws_config() -> WebSocketConfig {
    WebSocketConfig::default()
        .max_message_size(Some(MAX_MESSAGE))
        .max_frame_size(Some(MAX_MESSAGE))
}

/// Adresses dans l'ordre : manuelle, dernières connues, puis mDNS.
async fn connect(config: &LinkConfig, known: &[SocketAddr]) -> Attempt {
    let hello = EarMessage::Hello {
        version: PROTOCOL_VERSION,
        ear_id: config.pairing.ear_id.clone(),
        token: config.pairing.token.clone(),
        offline_phrase: config.store.offline_phrase_id(),
    };
    let Ok(hello) = serde_json::to_string(&hello) else {
        return Attempt::Unreachable;
    };
    let mut tried = HashSet::new();
    let mut candidates = Vec::new();
    if let Some(manual) = &config.pairing.manual {
        candidates.extend(resolve(manual).await);
    }
    candidates.extend_from_slice(known);
    for addr in candidates {
        if tried.insert(addr)
            && let Some(attempt) = try_addr(config, addr, &hello).await
        {
            return attempt;
        }
    }
    if let Some(discovery) = &config.discovery {
        for addr in discovery
            .find(&config.pairing.brain_id, DISCOVERY_WAIT)
            .await
        {
            if tried.insert(addr)
                && let Some(attempt) = try_addr(config, addr, &hello).await
            {
                return attempt;
            }
        }
    }
    Attempt::Unreachable
}

/// Adresse manuelle `hôte[:port]` (port par défaut : 7341).
async fn resolve(manual: &str) -> Vec<SocketAddr> {
    let manual = manual.trim();
    let with_port = if manual.parse::<SocketAddr>().is_ok() || manual.contains(':') {
        manual.to_string()
    } else {
        format!("{manual}:{}", jarvis_protocol::ear::DEFAULT_PORT)
    };
    match timeout(STEP_TIMEOUT, tokio::net::lookup_host(with_port)).await {
        Ok(Ok(addrs)) => addrs.collect(),
        Ok(Err(e)) => {
            tracing::warn!("adresse manuelle « {manual} » introuvable : {e}");
            Vec::new()
        }
        Err(_) => {
            tracing::warn!("adresse manuelle « {manual} » : pas de réponse");
            Vec::new()
        }
    }
}

/// `None` : rien à cette adresse (ou autre chose qu'un cerveau valide).
async fn try_addr(config: &LinkConfig, addr: SocketAddr, hello: &str) -> Option<Attempt> {
    let stream = match timeout(STEP_TIMEOUT, config.connector.connect(addr)).await {
        Ok(Ok(stream)) => stream,
        Ok(Err(e)) => {
            tracing::debug!("{addr} : {e}");
            return None;
        }
        Err(_) => {
            tracing::debug!("{addr} : délai dépassé");
            return None;
        }
    };
    let mut ws = match timeout(
        STEP_TIMEOUT,
        client_async_with_config(URL, stream, Some(ws_config())),
    )
    .await
    {
        Ok(Ok((ws, _))) => ws,
        Ok(Err(e)) => {
            tracing::debug!("{addr} : WebSocket : {e}");
            return None;
        }
        Err(_) => return None,
    };
    if !matches!(
        timeout(STEP_TIMEOUT, ws.send(Message::text(hello))).await,
        Ok(Ok(()))
    ) {
        return None;
    }
    let deadline = Instant::now() + STEP_TIMEOUT;
    loop {
        let message = match tokio::time::timeout_at(deadline, ws.next()).await {
            Ok(Some(Ok(message))) => message,
            _ => {
                tracing::debug!("{addr} : pas de réponse à hello");
                return None;
            }
        };
        let Message::Text(text) = message else {
            if matches!(message, Message::Close(_)) {
                return None;
            }
            continue;
        };
        match serde_json::from_str::<BrainMessage>(text.as_str()) {
            Ok(BrainMessage::Welcome {
                brain_id, status, ..
            }) => {
                if brain_id != config.pairing.brain_id {
                    tracing::warn!("{addr} : autre cerveau ({brain_id})");
                    return None;
                }
                return Some(Attempt::Connected(Box::new(ws), status, addr));
            }
            Ok(BrainMessage::Rejected { reason }) => return Some(Attempt::Rejected(reason)),
            Ok(BrainMessage::Unknown) => continue,
            _ => {
                tracing::debug!("{addr} : réponse inattendue à hello");
                return None;
            }
        }
    }
}

/// Résultat d'un appairage par code. `Debug` masque le jeton.
#[derive(Clone, PartialEq, Eq)]
pub struct Paired {
    pub ear_id: EarId,
    pub token: String,
    pub brain_id: String,
    pub brain_name: String,
}

impl fmt::Debug for Paired {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Paired")
            .field("ear_id", &self.ear_id)
            .field("token", &"***")
            .field("brain_id", &self.brain_id)
            .field("brain_name", &self.brain_name)
            .finish()
    }
}

#[derive(Debug)]
pub enum PairError {
    Rejected(RejectReason),
    Io(std::io::Error),
    /// Réponse illisible ou inattendue.
    Protocol(String),
}

impl fmt::Display for PairError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PairError::Rejected(r) => write!(f, "appairage refusé : {r:?}"),
            PairError::Io(e) => write!(f, "connexion impossible : {e}"),
            PairError::Protocol(m) => write!(f, "réponse inattendue : {m}"),
        }
    }
}

impl std::error::Error for PairError {}

fn pair_ws_error(e: tungstenite::Error) -> PairError {
    match e {
        tungstenite::Error::Io(e) => PairError::Io(e),
        other => PairError::Protocol(other.to_string()),
    }
}

fn timed_out() -> PairError {
    PairError::Io(io::Error::new(
        io::ErrorKind::TimedOut,
        "pas de réponse du cerveau",
    ))
}

/// Échange `pair` → `paired` / `rejected` sur un flux déjà ouvert (TLS compris).
pub async fn pair_with_code(
    stream: Box<dyn AsyncStream>,
    code: &str,
    name: &str,
    platform: &str,
) -> Result<Paired, PairError> {
    let (mut ws, _) = timeout(
        STEP_TIMEOUT,
        client_async_with_config(URL, stream, Some(ws_config())),
    )
    .await
    .map_err(|_| timed_out())?
    .map_err(pair_ws_error)?;
    let pair = EarMessage::Pair {
        version: PROTOCOL_VERSION,
        code: code.to_string(),
        name: name.to_string(),
        platform: platform.to_string(),
    };
    let text = serde_json::to_string(&pair).map_err(|e| PairError::Protocol(e.to_string()))?;
    ws.send(Message::text(text)).await.map_err(pair_ws_error)?;
    let result = read_paired(&mut ws).await;
    // Le cerveau ferme de son côté ; au mieux.
    let _ = timeout(Duration::from_secs(1), ws.close(None)).await;
    result
}

async fn read_paired(ws: &mut Ws) -> Result<Paired, PairError> {
    let deadline = Instant::now() + PAIR_TIMEOUT;
    loop {
        let message = tokio::time::timeout_at(deadline, ws.next())
            .await
            .map_err(|_| timed_out())?;
        let closed = || {
            PairError::Io(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "connexion fermée par le cerveau",
            ))
        };
        match message {
            None | Some(Ok(Message::Close(_))) => return Err(closed()),
            Some(Err(e)) => return Err(pair_ws_error(e)),
            Some(Ok(Message::Text(text))) => match serde_json::from_str::<BrainMessage>(&text) {
                Ok(BrainMessage::Paired {
                    ear_id,
                    token,
                    brain_id,
                    brain_name,
                }) => {
                    return Ok(Paired {
                        ear_id,
                        token,
                        brain_id,
                        // Affiché par `jarvis-ear pair`, gardé dans l'appairage.
                        brain_name: jarvis_protocol::printable(&brain_name),
                    });
                }
                Ok(BrainMessage::Rejected { reason }) => return Err(PairError::Rejected(reason)),
                Ok(BrainMessage::Unknown) => {}
                Ok(other) => return Err(PairError::Protocol(format!("{other:?}"))),
                Err(_) => return Err(PairError::Protocol("message illisible".into())),
            },
            Some(Ok(_)) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jarvis_protocol::frame::{Frame, decode_frame};
    use jarvis_protocol::{Fingerprint, Mode, SpeechAudio, encode_pcm};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::net::{TcpListener, TcpStream};
    use tokio_tungstenite::accept_async;

    /// Délai généreux pour tout ce qui doit arriver.
    const WAIT: Duration = Duration::from_secs(10);
    const BRAIN_ID: &str = "b1";

    /// TCP simple (le TLS est testé à part).
    struct Tcp;

    #[async_trait::async_trait]
    impl Connector for Tcp {
        async fn connect(&self, addr: SocketAddr) -> io::Result<Box<dyn AsyncStream>> {
            Ok(Box::new(TcpStream::connect(addr).await?))
        }
    }

    struct FakeDiscovery {
        found: Vec<SocketAddr>,
        calls: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl Discovery for FakeDiscovery {
        async fn find(&self, brain_id: &str, _: Duration) -> Vec<SocketAddr> {
            assert_eq!(brain_id, BRAIN_ID);
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.found.clone()
        }
    }

    type BrainWs = WebSocketStream<TcpStream>;

    fn pairing(addresses: Vec<SocketAddr>) -> Pairing {
        Pairing {
            brain_id: BRAIN_ID.into(),
            brain_name: "PC".into(),
            ear_id: EarId::new("00112233445566778899aabbccddeeff"),
            token: "jeton-secret".into(),
            fingerprint: Fingerprint::of_der(b"cert"),
            addresses,
            manual: None,
        }
    }

    /// Appairage enregistré dans un dossier temporaire ; battement court.
    fn config(p: Pairing) -> (tempfile::TempDir, LinkConfig) {
        let dir = tempfile::tempdir().unwrap();
        let store = PairingStore::new(dir.path().join("ear"));
        store.save(&p).unwrap();
        let mut config = LinkConfig::new(p, store, Arc::new(Tcp), None);
        config.heartbeat = Duration::from_millis(200);
        config.dead_after = Duration::from_secs(30);
        (dir, config)
    }

    async fn bind() -> (TcpListener, SocketAddr) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        (listener, addr)
    }

    /// Adresse où rien n'écoute.
    async fn dead_addr() -> SocketAddr {
        bind().await.1
    }

    async fn accept(listener: &TcpListener) -> BrainWs {
        let (tcp, _) = timeout(WAIT, listener.accept()).await.unwrap().unwrap();
        accept_async(tcp).await.unwrap()
    }

    /// Prochain message hors ping / pong.
    async fn recv_raw(ws: &mut BrainWs) -> Message {
        loop {
            let m = timeout(WAIT, ws.next()).await.unwrap().unwrap().unwrap();
            if !matches!(m, Message::Ping(_) | Message::Pong(_)) {
                return m;
            }
        }
    }

    async fn recv(ws: &mut BrainWs) -> EarMessage {
        match recv_raw(ws).await {
            Message::Text(t) => serde_json::from_str(&t).unwrap(),
            other => panic!("{other:?}"),
        }
    }

    async fn send(ws: &mut BrainWs, m: &BrainMessage) {
        ws.send(Message::text(serde_json::to_string(m).unwrap()))
            .await
            .unwrap();
    }

    fn status(mode: Mode) -> StatusInfo {
        StatusInfo {
            mode,
            version: "0.1.0".into(),
            local_model: "llama3.2:3b".into(),
            ears: vec![],
        }
    }

    fn welcome() -> BrainMessage {
        BrainMessage::Welcome {
            version: PROTOCOL_VERSION,
            brain_id: BRAIN_ID.into(),
            brain_name: "PC".into(),
            status: status(Mode::Local),
        }
    }

    /// Accepte une connexion, lit `hello` et répond `welcome`.
    async fn welcome_one(listener: &TcpListener) -> (BrainWs, EarMessage) {
        let mut ws = accept(listener).await;
        let hello = recv(&mut ws).await;
        send(&mut ws, &welcome()).await;
        (ws, hello)
    }

    async fn next_event(events: &mut mpsc::Receiver<LinkEvent>) -> LinkEvent {
        timeout(WAIT, events.recv()).await.unwrap().unwrap()
    }

    async fn wait_state(link: &EarLink, f: impl Fn(&LinkState) -> bool) {
        let mut state = link.state();
        timeout(WAIT, state.wait_for(|s| f(s)))
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn hello_envoye_en_premier() {
        let (listener, addr) = bind().await;
        let (_dir, config) = config(pairing(vec![addr]));
        let audio = SpeechAudio {
            pcm: encode_pcm(&[1, 2]),
            sample_rate: 22_050,
        };
        config.store.save_offline_phrase("p1", &audio).unwrap();
        let (_link, _events) = EarLink::spawn(config);
        let mut ws = accept(&listener).await;
        let hello = recv(&mut ws).await;
        assert_eq!(
            hello,
            EarMessage::Hello {
                version: PROTOCOL_VERSION,
                ear_id: EarId::new("00112233445566778899aabbccddeeff"),
                token: "jeton-secret".into(),
                offline_phrase: Some("p1".into()),
            }
        );
    }

    #[tokio::test]
    async fn welcome_publie_l_etat_connecte() {
        let (listener, addr) = bind().await;
        let (_dir, config) = config(pairing(vec![addr]));
        let (link, mut events) = EarLink::spawn(config);
        assert_eq!(*link.state().borrow(), LinkState::Connecting);
        let (mut ws, _) = welcome_one(&listener).await;
        assert_eq!(
            next_event(&mut events).await,
            LinkEvent::Connected(status(Mode::Local))
        );
        assert_eq!(
            *link.state().borrow(),
            LinkState::Connected(status(Mode::Local))
        );
        // Changement de mode : état mis à jour et transmis.
        send(&mut ws, &BrainMessage::Status(status(Mode::Online))).await;
        assert_eq!(
            next_event(&mut events).await,
            LinkEvent::Message(BrainMessage::Status(status(Mode::Online)))
        );
        assert_eq!(
            *link.state().borrow(),
            LinkState::Connected(status(Mode::Online))
        );
        // Message inconnu ignoré, le suivant passe.
        ws.send(Message::text(r#"{"type":"dance"}"#)).await.unwrap();
        send(&mut ws, &BrainMessage::Elected { utterance: 3 }).await;
        assert_eq!(
            next_event(&mut events).await,
            LinkEvent::Message(BrainMessage::Elected { utterance: 3 })
        );
    }

    #[tokio::test]
    async fn jeton_refuse_pas_de_reconnexion_effrenee() {
        let (listener, addr) = bind().await;
        let (_dir, config) = config(pairing(vec![addr]));
        let connections = Arc::new(AtomicUsize::new(0));
        let count = connections.clone();
        tokio::spawn(async move {
            loop {
                let mut ws = accept(&listener).await;
                count.fetch_add(1, Ordering::SeqCst);
                recv(&mut ws).await;
                let reason = RejectReason::BadToken;
                send(&mut ws, &BrainMessage::Rejected { reason }).await;
                let _ = ws.close(None).await;
            }
        });
        let (link, mut events) = EarLink::spawn(config);
        wait_state(&link, |s| *s == LinkState::Rejected(RejectReason::BadToken)).await;
        tokio::time::sleep(Duration::from_secs(2)).await;
        assert_eq!(connections.load(Ordering::SeqCst), 1);
        assert!(events.try_recv().is_err(), "aucun événement");
        assert_eq!(link.send(EarMessage::Status), Err(LinkDown));
    }

    #[tokio::test]
    async fn reconnexion_apres_coupure() {
        let (listener, addr) = bind().await;
        let (_dir, config) = config(pairing(vec![addr]));
        let (link, mut events) = EarLink::spawn(config);
        let (ws, _) = welcome_one(&listener).await;
        assert!(matches!(
            next_event(&mut events).await,
            LinkEvent::Connected(_)
        ));
        drop(ws);
        assert_eq!(next_event(&mut events).await, LinkEvent::Disconnected);
        assert_eq!(link.send(EarMessage::Status), Err(LinkDown));
        // Nouvelle tentative seule (≈ 500 ms après la coupure).
        let (_ws, hello) = welcome_one(&listener).await;
        assert!(matches!(hello, EarMessage::Hello { .. }));
        assert!(matches!(
            next_event(&mut events).await,
            LinkEvent::Connected(_)
        ));
        assert!(link.send(EarMessage::Status).is_ok());
    }

    #[tokio::test]
    async fn nudge_reconnecte_tout_de_suite() {
        let (listener, addr) = bind().await;
        let (_dir, config) = config(pairing(vec![addr]));
        let (link, mut events) = EarLink::spawn(config);
        // Refus : prochaine tentative dans 24 s au moins…
        let mut ws = accept(&listener).await;
        recv(&mut ws).await;
        let reason = RejectReason::UnknownEar;
        send(&mut ws, &BrainMessage::Rejected { reason }).await;
        wait_state(&link, |s| matches!(s, LinkState::Rejected(_))).await;
        // … sauf si on la demande.
        link.nudge();
        let started = std::time::Instant::now();
        let (_ws, _) = welcome_one(&listener).await;
        assert!(matches!(
            next_event(&mut events).await,
            LinkEvent::Connected(_)
        ));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[tokio::test]
    async fn silence_du_cerveau_detecte() {
        let (listener, addr) = bind().await;
        let (_dir, mut config) = config(pairing(vec![addr]));
        config.heartbeat = Duration::from_millis(100);
        config.dead_after = Duration::from_millis(600);
        let (link, mut events) = EarLink::spawn(config);
        // Le cerveau ne lit plus rien : pas de pong.
        let (_ws, _) = welcome_one(&listener).await;
        assert!(matches!(
            next_event(&mut events).await,
            LinkEvent::Connected(_)
        ));
        assert_eq!(next_event(&mut events).await, LinkEvent::Disconnected);
        assert_eq!(link.send_audio(1, &[1; 10]), Err(LinkDown));
    }

    #[tokio::test]
    async fn audio_jamais_mis_en_attente_hors_connexion() {
        let (listener, addr) = bind().await;
        let (_dir, config) = config(pairing(vec![addr]));
        let (link, mut events) = EarLink::spawn(config);
        // Pas encore de cerveau : refus immédiat.
        assert_eq!(link.send_audio(1, &[7; 1_600]), Err(LinkDown));
        assert_eq!(link.send_audio(1, &[]), Err(LinkDown));
        let wake = EarMessage::Wake {
            utterance: 1,
            score: 0.9,
            level_db: -30.0,
            muted: false,
        };
        assert_eq!(link.send(wake.clone()), Err(LinkDown));
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(link.send_audio(1, &[7; 1_600]), Err(LinkDown));

        let (mut ws, _) = welcome_one(&listener).await;
        assert!(matches!(
            next_event(&mut events).await,
            LinkEvent::Connected(_)
        ));
        link.send(EarMessage::Status).unwrap();
        link.send_audio(2, &[5; 10]).unwrap();
        // Rien d'avant la connexion : d'abord `status`, puis le seul son envoyé.
        assert_eq!(recv(&mut ws).await, EarMessage::Status);
        let Message::Binary(b) = recv_raw(&mut ws).await else {
            panic!()
        };
        assert_eq!(
            decode_frame(&b),
            Ok(Frame::MicAudio {
                utterance: 2,
                samples: vec![5; 10]
            })
        );
        // Plus d'une seconde : découpé en trames bornées.
        link.send_audio(3, &[1; 20_000]).unwrap();
        for len in [MAX_FRAME_SAMPLES, 20_000 - MAX_FRAME_SAMPLES] {
            let Message::Binary(b) = recv_raw(&mut ws).await else {
                panic!()
            };
            let Ok(Frame::MicAudio { samples, .. }) = decode_frame(&b) else {
                panic!()
            };
            assert_eq!(samples.len(), len);
        }

        // Coupure : refus immédiat, rien de gardé pour la connexion suivante.
        drop(ws);
        assert_eq!(next_event(&mut events).await, LinkEvent::Disconnected);
        assert_eq!(link.send_audio(4, &[9; 1_600]), Err(LinkDown));
        assert_eq!(link.send(wake), Err(LinkDown));
        let (mut ws, _) = welcome_one(&listener).await;
        assert!(matches!(
            next_event(&mut events).await,
            LinkEvent::Connected(_)
        ));
        link.send(EarMessage::Status).unwrap();
        assert_eq!(recv(&mut ws).await, EarMessage::Status);
    }

    #[tokio::test]
    async fn phrase_hors_ligne_enregistree() {
        let (listener, addr) = bind().await;
        let (_dir, config) = config(pairing(vec![addr]));
        let store = config.store.clone();
        let (_link, mut events) = EarLink::spawn(config);
        let (mut ws, hello) = welcome_one(&listener).await;
        let EarMessage::Hello { offline_phrase, .. } = hello else {
            panic!()
        };
        assert_eq!(offline_phrase, None);
        assert!(matches!(
            next_event(&mut events).await,
            LinkEvent::Connected(_)
        ));
        let audio = SpeechAudio {
            pcm: encode_pcm(&[100, -100, 3]),
            sample_rate: 22_050,
        };
        let phrase = BrainMessage::OfflinePhrase {
            id: "z9".into(),
            audio: audio.clone(),
        };
        send(&mut ws, &phrase).await;
        send(&mut ws, &BrainMessage::Elected { utterance: 4 }).await;
        // Non transmise : enregistrée.
        assert_eq!(
            next_event(&mut events).await,
            LinkEvent::Message(BrainMessage::Elected { utterance: 4 })
        );
        assert_eq!(store.offline_phrase(), Some(("z9".into(), audio)));
        // Annoncée au cerveau à la connexion suivante.
        drop(ws);
        let (_ws, hello) = welcome_one(&listener).await;
        let EarMessage::Hello { offline_phrase, .. } = hello else {
            panic!()
        };
        assert_eq!(offline_phrase.as_deref(), Some("z9"));
    }

    #[tokio::test]
    async fn adresse_mdns_utilisee_si_la_derniere_echoue() {
        let (listener, addr) = bind().await;
        let dead = dead_addr().await;
        let (_dir, mut config) = config(pairing(vec![dead]));
        let calls = Arc::new(AtomicUsize::new(0));
        config.discovery = Some(Arc::new(FakeDiscovery {
            found: vec![dead, addr],
            calls: calls.clone(),
        }));
        let store = config.store.clone();
        let (_link, mut events) = EarLink::spawn(config);
        let (_ws, _) = welcome_one(&listener).await;
        assert!(matches!(
            next_event(&mut events).await,
            LinkEvent::Connected(_)
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        // Adresse trouvée retenue, en tête.
        assert_eq!(store.load().unwrap().unwrap().addresses, vec![addr, dead]);
    }

    #[tokio::test]
    async fn adresse_manuelle_essayee_en_premier() {
        let (listener, addr) = bind().await;
        let dead = dead_addr().await;
        let mut p = pairing(vec![dead]);
        p.manual = Some(addr.to_string());
        let (_dir, mut config) = config(p);
        let calls = Arc::new(AtomicUsize::new(0));
        config.discovery = Some(Arc::new(FakeDiscovery {
            found: vec![],
            calls: calls.clone(),
        }));
        let (_link, mut events) = EarLink::spawn(config);
        let (_ws, _) = welcome_one(&listener).await;
        assert!(matches!(
            next_event(&mut events).await,
            LinkEvent::Connected(_)
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 0, "pas de mDNS");
    }

    #[tokio::test]
    async fn injoignable_puis_fermeture() {
        let dead = dead_addr().await;
        let (_dir, config) = config(pairing(vec![dead]));
        let (link, _events) = EarLink::spawn(config);
        wait_state(&link, |s| *s == LinkState::Unreachable).await;
        timeout(WAIT, link.close()).await.unwrap();
    }

    #[tokio::test]
    async fn fermeture_envoie_les_derniers_messages() {
        let (listener, addr) = bind().await;
        let (_dir, config) = config(pairing(vec![addr]));
        let (link, mut events) = EarLink::spawn(config);
        let (mut ws, _) = welcome_one(&listener).await;
        assert!(matches!(
            next_event(&mut events).await,
            LinkEvent::Connected(_)
        ));
        let end = EarMessage::ListenEnd {
            utterance: 1,
            muted: false,
            no_speech: true,
            cut: None,
        };
        link.send(end.clone()).unwrap();
        timeout(WAIT, link.close()).await.unwrap();
        assert_eq!(recv(&mut ws).await, end);
        assert!(matches!(recv_raw(&mut ws).await, Message::Close(_)));
    }

    async fn pair_once(reply: BrainMessage) -> (EarMessage, Result<Paired, PairError>) {
        let (listener, addr) = bind().await;
        let brain = tokio::spawn(async move {
            let mut ws = accept(&listener).await;
            let pair = recv(&mut ws).await;
            send(&mut ws, &reply).await;
            pair
        });
        let stream: Box<dyn AsyncStream> = Box::new(TcpStream::connect(addr).await.unwrap());
        let result = pair_with_code(stream, "482913", "Pixel 4 XL", "android").await;
        (brain.await.unwrap(), result)
    }

    #[tokio::test]
    async fn appairage_par_code_rend_le_jeton() {
        let (pair, result) = pair_once(BrainMessage::Paired {
            ear_id: EarId::new("ab"),
            token: "jeton-tout-neuf".into(),
            brain_id: BRAIN_ID.into(),
            brain_name: "PC".into(),
        })
        .await;
        assert_eq!(
            pair,
            EarMessage::Pair {
                version: PROTOCOL_VERSION,
                code: "482913".into(),
                name: "Pixel 4 XL".into(),
                platform: "android".into(),
            }
        );
        let paired = result.unwrap();
        assert_eq!(
            paired,
            Paired {
                ear_id: EarId::new("ab"),
                token: "jeton-tout-neuf".into(),
                brain_id: BRAIN_ID.into(),
                brain_name: "PC".into(),
            }
        );
        assert!(!format!("{paired:?}").contains("jeton-tout-neuf"));

        let reason = RejectReason::WrongCode { attempts_left: 4 };
        let (_, result) = pair_once(BrainMessage::Rejected {
            reason: reason.clone(),
        })
        .await;
        assert!(matches!(result, Err(PairError::Rejected(r)) if r == reason));
    }

    #[tokio::test]
    async fn nom_du_cerveau_nettoye_a_l_appairage() {
        let (_, result) = pair_once(BrainMessage::Paired {
            ear_id: EarId::new("ab"),
            token: "jeton".into(),
            brain_id: BRAIN_ID.into(),
            brain_name: "PC\u{1b}[2J\r\u{202E}lam".into(),
        })
        .await;
        assert_eq!(result.unwrap().brain_name, "PC[2Jlam");
    }
}
