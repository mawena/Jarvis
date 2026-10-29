//! Hub des oreilles : une seule tâche possède l'arbitre, les tampons des phrases,
//! les sessions, le registre et la fenêtre d'appairage. `Hub` en est la poignée
//! (clonable) : chaque appel est un message à cette tâche.

use crate::announce::{AnnouncementBoard, Pending};
use crate::arbiter::{Arbiter, ArbiterAction, EndVerdict, WakeReport};
use crate::brain::Brain;
use crate::ears::{EarRecord, EarRegistry, hex};
use crate::listen::ListenService;
use crate::pairing::{PairAttempt, PairingWindow};
use crate::tts::Voice;
use jarvis_protocol::ear::{BrainMessage, EarMessage, PROTOCOL_VERSION, RejectReason, StatusInfo};
use jarvis_protocol::pairing_ended;
use jarvis_protocol::pairing_uri::PairingUri;
use jarvis_protocol::{ControlResponse, EarId, EarSummary, Fingerprint, Mode, SpeechAudio};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot, watch};
use tracing::{debug, info, warn};

use super::{MAX_PHRASE_SAMPLES, MAX_SESSIONS};

/// Capture sans fin de phrase au-delà de ce délai : abandonnée (14 s de son au plus
/// côté oreille, `jarvis_protocol::frame::MAX_PHRASE_SAMPLES`, envoyé en temps réel,
/// plus la marge du réseau).
pub const CAPTURE_TIMEOUT: Duration = Duration::from_secs(20);
/// Capture sans son depuis ce délai : l'oreille l'a abandonnée sans prévenir (pas
/// d'élection dans ses 2 s, `ELECTION_TIMEOUT`, ou micro bloqué). L'oreille envoie son
/// son en continu, par paquets de 100 ms, jusqu'à la fin de phrase.
pub const AUDIO_STALL: Duration = Duration::from_secs(3);
/// Traitement d'une phrase (transcription, compétence, voix) au plus : au-dessus du
/// pire cas (45 s, voir le test `budget_ecoute`), sous l'attente de la réponse par
/// l'oreille (50 s, `LISTEN_TIMEOUT` de jarvis-ear) moins la fenêtre de l'arbitre et le
/// réseau. Ainsi l'oreille reçoit toujours `Error` (bip d'échec) plutôt que rien.
pub const PROCESS_TIMEOUT: Duration = Duration::from_secs(47);
/// Relève des annonces pour les oreilles connectées.
const ANNOUNCE_EVERY: Duration = Duration::from_secs(5);
/// Messages en attente vers la tâche du hub.
const COMMANDS: usize = 256;
/// Longueur maximale du nom et de la plateforme donnés par une oreille.
const MAX_NAME_CHARS: usize = 64;
const MAX_PLATFORM_CHARS: usize = 32;

/// Réponse à une oreille distante qui demande le mode en ligne.
pub const ONLINE_FROM_PC_ONLY: &str =
    "Le mode en ligne se demande à la voix (avec confirmation) ou depuis le PC.";

/// Rappel de l'annonce mDNS : `true` pendant une fenêtre d'appairage.
pub type PairingHook = Arc<dyn Fn(bool) + Send + Sync>;

/// Identité et réglages du hub.
#[derive(Clone)]
pub struct HubSettings {
    pub brain_id: String,
    pub brain_name: String,
    /// Port réel du serveur (rendu à l'oreille locale et mis dans l'URI).
    pub port: u16,
    pub fingerprint: Fingerprint,
    /// Adresses du PC mises dans l'URI d'appairage.
    pub hosts: Vec<IpAddr>,
    /// Fenêtre de l'arbitre.
    pub window: Duration,
    /// Durée d'ouverture d'une fenêtre d'appairage.
    pub pairing_ttl: Duration,
    /// Traitement d'une phrase au plus ([`PROCESS_TIMEOUT`]) : passé ce délai, il est
    /// arrêté.
    pub process_timeout: Duration,
}

/// Ce que le hub appelle pour traiter une phrase.
#[derive(Clone)]
pub struct Services {
    pub brain: Arc<Brain>,
    pub listen: Arc<ListenService>,
    pub voice: Voice,
    pub board: Arc<AnnouncementBoard>,
    /// Changements de mode (`ModeStore::subscribe`) : `Status` diffusé. Absent :
    /// diffusé seulement après un `SetMode` venu d'une oreille.
    pub modes: Option<watch::Receiver<Mode>>,
    /// Décrit la voix (modèle, locuteur…) : entre dans l'id de la phrase hors ligne.
    pub voice_tag: String,
}

/// Numéro d'une session (une connexion authentifiée).
pub type SessionId = u64;

/// Issue d'un `hello`.
#[derive(Debug)]
pub enum HelloOutcome {
    Accepted {
        record: EarRecord,
        session: SessionId,
    },
    Rejected(RejectReason),
    /// Trop de sessions ouvertes.
    Full,
}

/// Fenêtre d'appairage ouverte (réponse à `PairStart`).
pub struct PairTicket {
    pub id: u64,
    pub code: String,
    pub uri: String,
    pub fingerprint_short: String,
    pub expires_in_secs: u64,
    /// Fin de la fenêtre : oreille appairée, ou fermée sans appairage.
    pub outcome: oneshot::Receiver<PairOutcome>,
}

impl std::fmt::Debug for PairTicket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Ni le code ni l'URI (qui le contient).
        f.debug_struct("PairTicket")
            .field("id", &self.id)
            .field("expires_in_secs", &self.expires_in_secs)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairOutcome {
    Done { name: String },
    Ended { reason: String },
}

enum Command {
    Hello {
        ear_id: EarId,
        token: String,
        offline_phrase: Option<String>,
        outbox: mpsc::Sender<BrainMessage>,
        from: IpAddr,
        reply: oneshot::Sender<HelloOutcome>,
    },
    Pair {
        code: String,
        name: String,
        platform: String,
        from: IpAddr,
        reply: oneshot::Sender<BrainMessage>,
    },
    FromEar {
        ear: EarId,
        session: SessionId,
        message: EarMessage,
    },
    Audio {
        ear: EarId,
        session: SessionId,
        utterance: u32,
        samples: Vec<i16>,
    },
    Disconnected {
        ear: EarId,
        session: SessionId,
    },
    Processed {
        ear: EarId,
        utterance: u32,
        response: ControlResponse,
    },
    PairStart {
        reply: oneshot::Sender<Result<PairTicket, String>>,
    },
    PairCancel {
        id: u64,
    },
    PairLocal {
        name: String,
        reply: oneshot::Sender<ControlResponse>,
    },
    ListEars {
        reply: oneshot::Sender<Vec<EarSummary>>,
    },
    Unpair {
        ear: String,
        reply: oneshot::Sender<ControlResponse>,
    },
    OfflinePhrase {
        id: String,
        audio: SpeechAudio,
    },
    PollAnnouncements,
    /// Annonces prêtes pour `ear` ; `items` : les mêmes, telles que relevées (remises
    /// en file si l'oreille est partie).
    Deliver {
        ear: EarId,
        message: BrainMessage,
        items: Vec<Pending>,
    },
}

/// Poignée du hub. La tâche s'arrête quand toutes les poignées sont lâchées.
#[derive(Clone)]
pub struct Hub {
    tx: mpsc::Sender<Command>,
}

impl Hub {
    /// Lance la tâche du hub (dans le runtime tokio courant).
    pub fn spawn(
        settings: HubSettings,
        registry: EarRegistry,
        services: Services,
        on_pairing: Option<PairingHook>,
    ) -> Hub {
        let (tx, rx) = mpsc::channel(COMMANDS);
        let (sound_tx, sound_rx) = mpsc::unbounded_channel();
        tokio::spawn(sound_loop(services.listen.clone(), sound_rx));
        let state = State {
            arbiter: Arbiter::new(settings.window),
            settings,
            registry,
            services: services.clone(),
            on_pairing,
            sessions: HashMap::new(),
            captures: HashMap::new(),
            processing: 0,
            pairing: None,
            next_id: 1,
            refusals: 0,
            offline: None,
            sound: sound_tx,
            me: tx.downgrade(),
        };
        tokio::spawn(state.run(rx, services.modes));
        let hub = Hub { tx };
        tokio::spawn(watch_announcements(services.board, hub.tx.downgrade()));
        hub
    }

    async fn ask<T>(&self, make: impl FnOnce(oneshot::Sender<T>) -> Command) -> Option<T> {
        let (reply, rx) = oneshot::channel();
        self.tx.send(make(reply)).await.ok()?;
        rx.await.ok()
    }

    /// Authentifie une oreille venue de l'adresse `from` et ouvre sa session :
    /// `Welcome`, `Status` (et la phrase hors ligne si besoin) sont déjà dans `outbox`
    /// au retour. L'oreille du PC (enregistrement local) n'est acceptée que par la
    /// boucle locale.
    pub async fn hello(
        &self,
        ear_id: EarId,
        token: String,
        offline_phrase: Option<String>,
        outbox: mpsc::Sender<BrainMessage>,
        from: IpAddr,
    ) -> HelloOutcome {
        self.ask(|reply| Command::Hello {
            ear_id,
            token,
            offline_phrase,
            outbox,
            from,
            reply,
        })
        .await
        .unwrap_or(HelloOutcome::Full)
    }

    /// Essai d'appairage par code venu de l'adresse `from` : `Paired` ou `Rejected`.
    pub async fn pair(
        &self,
        code: String,
        name: String,
        platform: String,
        from: IpAddr,
    ) -> BrainMessage {
        self.ask(|reply| Command::Pair {
            code,
            name,
            platform,
            from,
            reply,
        })
        .await
        .unwrap_or(BrainMessage::Rejected {
            reason: RejectReason::PairingClosed,
        })
    }

    /// Message texte d'une oreille authentifiée.
    pub async fn received(&self, ear: &EarId, session: SessionId, message: EarMessage) {
        let _ = self
            .tx
            .send(Command::FromEar {
                ear: ear.clone(),
                session,
                message,
            })
            .await;
    }

    /// Son du micro d'une oreille authentifiée.
    pub async fn audio(&self, ear: &EarId, session: SessionId, utterance: u32, samples: Vec<i16>) {
        let _ = self
            .tx
            .send(Command::Audio {
                ear: ear.clone(),
                session,
                utterance,
                samples,
            })
            .await;
    }

    /// Session fermée : l'arbitre en est prévenu (son rétabli si besoin).
    pub async fn disconnected(&self, ear: &EarId, session: SessionId) {
        let _ = self
            .tx
            .send(Command::Disconnected {
                ear: ear.clone(),
                session,
            })
            .await;
    }

    /// Ouvre une fenêtre d'appairage (une seule à la fois : la précédente est fermée).
    pub async fn pair_start(&self) -> Result<PairTicket, String> {
        self.ask(|reply| Command::PairStart { reply })
            .await
            .unwrap_or_else(|| Err("Le réseau des oreilles est arrêté.".into()))
    }

    /// Ferme la fenêtre `id` si elle est encore ouverte (`jarvis pair` interrompu).
    pub async fn pair_cancel(&self, id: u64) {
        let _ = self.tx.send(Command::PairCancel { id }).await;
    }

    /// Appairage de l'oreille du PC : `PairedLocal` ou `Error`.
    pub async fn pair_local(&self, name: String) -> ControlResponse {
        self.ask(|reply| Command::PairLocal { name, reply })
            .await
            .unwrap_or_else(stopped)
    }

    pub async fn list_ears(&self) -> Vec<EarSummary> {
        self.ask(|reply| Command::ListEars { reply })
            .await
            .unwrap_or_default()
    }

    /// Retire une oreille (nom ou identifiant) et la déconnecte : `Reply` ou `Error`.
    pub async fn unpair(&self, ear: String) -> ControlResponse {
        self.ask(|reply| Command::Unpair { ear, reply })
            .await
            .unwrap_or_else(stopped)
    }

    /// Phrase « le PC est endormi » prête : envoyée aux oreilles qui ne l'ont pas.
    pub async fn set_offline_phrase(&self, id: String, audio: SpeechAudio) {
        let _ = self.tx.send(Command::OfflinePhrase { id, audio }).await;
    }
}

fn stopped() -> ControlResponse {
    ControlResponse::Error {
        message: "Le réseau des oreilles est arrêté.".into(),
    }
}

/// Id de la phrase hors ligne : SHA-256 court du texte et de la voix.
pub fn offline_phrase_id(text: &str, voice_tag: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(text.as_bytes());
    hash.update([0]);
    hash.update(voice_tag.as_bytes());
    hex(&hash.finalize()[..8])
}

/// Baisse et rétablissement du son, dans l'ordre, hors de la tâche du hub.
enum SoundCmd {
    Duck,
    Restore,
    /// Prévient quand tout ce qui précède est fait : le traitement d'une phrase
    /// l'attend avant de rétablir le son lui-même (`ListenService::listen`), pour
    /// qu'une baisse encore en file ne passe pas après (son laissé baissé).
    Flush(oneshot::Sender<()>),
}

async fn sound_loop(listen: Arc<ListenService>, mut rx: mpsc::UnboundedReceiver<SoundCmd>) {
    while let Some(cmd) = rx.recv().await {
        match cmd {
            SoundCmd::Duck => listen.wake().await,
            SoundCmd::Restore => listen.restore().await,
            SoundCmd::Flush(done) => {
                let _ = done.send(());
            }
        }
    }
}

/// Nouvelle annonce, ou toutes les 5 s : le hub relève les annonces.
async fn watch_announcements(board: Arc<AnnouncementBoard>, hub: mpsc::WeakSender<Command>) {
    loop {
        let changed = board.changed().notified();
        tokio::pin!(changed);
        changed.as_mut().enable();
        tokio::select! {
            () = changed => {}
            () = tokio::time::sleep(ANNOUNCE_EVERY) => {}
        }
        let Some(tx) = hub.upgrade() else { return };
        if tx.send(Command::PollAnnouncements).await.is_err() {
            return;
        }
    }
}

struct Session {
    id: SessionId,
    record: EarRecord,
    outbox: mpsc::Sender<BrainMessage>,
    /// Id de la phrase hors ligne que l'oreille a déjà.
    offline: Option<String>,
}

/// Fin de phrase annoncée par l'oreille.
#[derive(Clone, Copy, Default)]
struct End {
    muted: bool,
    no_speech: bool,
    cut: Option<(u32, u32)>,
}

/// Son d'une phrase en cours de capture.
struct Capture {
    utterance: u32,
    follow_up: bool,
    samples: Vec<i16>,
    started: Instant,
    /// Dernier son reçu (ou début de la capture).
    heard: Instant,
    /// Fin reçue avant la décision de l'arbitre.
    ended: Option<End>,
}

impl Capture {
    fn new(utterance: u32, follow_up: bool) -> Self {
        let now = Instant::now();
        Capture {
            utterance,
            follow_up,
            samples: Vec::new(),
            started: now,
            heard: now,
            ended: None,
        }
    }
}

struct OpenWindow {
    id: u64,
    window: PairingWindow,
    waiter: oneshot::Sender<PairOutcome>,
}

struct State {
    settings: HubSettings,
    registry: EarRegistry,
    services: Services,
    on_pairing: Option<PairingHook>,
    arbiter: Arbiter,
    sessions: HashMap<EarId, Session>,
    captures: HashMap<EarId, Capture>,
    /// Phrases en traitement.
    processing: usize,
    pairing: Option<OpenWindow>,
    next_id: u64,
    /// Connexions non authentifiées refusées depuis le démarrage.
    refusals: u64,
    offline: Option<(String, SpeechAudio)>,
    sound: mpsc::UnboundedSender<SoundCmd>,
    me: mpsc::WeakSender<Command>,
}

/// Attend `deadline` ; sans échéance, jamais.
async fn until(deadline: Option<Instant>) {
    match deadline {
        Some(d) => tokio::time::sleep_until(tokio::time::Instant::from_std(d)).await,
        None => std::future::pending().await,
    }
}

impl State {
    async fn run(
        mut self,
        mut rx: mpsc::Receiver<Command>,
        mut modes: Option<watch::Receiver<Mode>>,
    ) {
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            let decision = self.arbiter.deadline();
            let expiry = self.pairing.as_ref().map(|p| p.window.expires_at());
            tokio::select! {
                command = rx.recv() => match command {
                    Some(command) => self.handle(command),
                    None => break,
                },
                () = until(decision) => {
                    let actions = self.arbiter.tick(Instant::now());
                    self.apply(actions);
                }
                () = until(expiry) => self.close_pairing(pairing_ended::TIMED_OUT),
                _ = tick.tick() => self.expire_captures(),
                changed = async { modes.as_mut().expect("garde").changed().await }, if modes.is_some() => {
                    match changed {
                        Ok(()) => self.broadcast_status(),
                        Err(_) => modes = None,
                    }
                }
            }
        }
        // Plus personne : la phrase en cours ne laisse pas le son baissé.
        if !self.captures.is_empty() {
            let _ = self.sound.send(SoundCmd::Restore);
        }
        if let Some(open) = self.pairing.take() {
            let _ = open.waiter.send(PairOutcome::Ended {
                reason: pairing_ended::BRAIN_STOPPING.into(),
            });
        }
    }

    fn handle(&mut self, command: Command) {
        match command {
            Command::Hello {
                ear_id,
                token,
                offline_phrase,
                outbox,
                from,
                reply,
            } => {
                let outcome = self.hello(ear_id, &token, offline_phrase, outbox, from);
                let _ = reply.send(outcome);
            }
            Command::Pair {
                code,
                name,
                platform,
                from,
                reply,
            } => {
                let _ = reply.send(self.pair(&code, &name, &platform, from));
            }
            Command::FromEar {
                ear,
                session,
                message,
            } => {
                if self.is_current(&ear, session) {
                    self.received(&ear, message);
                }
            }
            Command::Audio {
                ear,
                session,
                utterance,
                samples,
            } => {
                if self.is_current(&ear, session) {
                    self.audio(&ear, utterance, &samples);
                }
            }
            Command::Disconnected { ear, session } => {
                if self.is_current(&ear, session) {
                    info!(oreille = %ear, "oreille déconnectée");
                    self.drop_session(&ear);
                }
            }
            Command::Processed {
                ear,
                utterance,
                response,
            } => self.processed(&ear, utterance, response),
            Command::PairStart { reply } => {
                let _ = reply.send(self.pair_start());
            }
            Command::PairCancel { id } => {
                if self.pairing.as_ref().is_some_and(|p| p.id == id) {
                    self.close_pairing(pairing_ended::CANCELLED);
                }
            }
            Command::PairLocal { name, reply } => {
                let _ = reply.send(self.pair_local(&name));
            }
            Command::ListEars { reply } => {
                let _ = reply.send(self.summaries());
            }
            Command::Unpair { ear, reply } => {
                let _ = reply.send(self.unpair(&ear));
            }
            Command::OfflinePhrase { id, audio } => self.offline_phrase(id, audio),
            Command::PollAnnouncements => self.poll_announcements(),
            Command::Deliver {
                ear,
                message,
                items,
            } => {
                if !self.send(&ear, message) {
                    debug!(oreille = %ear, "oreille partie : annonces remises en file");
                    self.services.board.requeue(items);
                }
            }
        }
    }

    /// Une connexion non authentifiée refusée (`hello` refusé, appairage hors
    /// fenêtre) : le détail est en `debug`, le journal n'en garde qu'un résumé, de
    /// plus en plus rare (1, 2, 4, 8… refus) pour ne pas être noyé.
    fn refused(&mut self) {
        self.refusals += 1;
        if self.refusals.is_power_of_two() {
            info!(
                refus = self.refusals,
                "connexions refusées depuis le démarrage (détail : RUST_LOG=debug)"
            );
        }
    }

    fn is_current(&self, ear: &EarId, session: SessionId) -> bool {
        self.sessions.get(ear).is_some_and(|s| s.id == session)
    }

    // --- Sessions ---

    fn hello(
        &mut self,
        ear_id: EarId,
        token: &str,
        offline: Option<String>,
        outbox: mpsc::Sender<BrainMessage>,
        from: IpAddr,
    ) -> HelloOutcome {
        let Some(record) = self.registry.authenticate(&ear_id, token) else {
            let reason = if self.registry.get(&ear_id).is_some() {
                RejectReason::BadToken
            } else {
                RejectReason::UnknownEar
            };
            debug!(oreille = shown(&ear_id), raison = ?reason, "oreille refusée");
            self.refused();
            return HelloOutcome::Rejected(reason);
        };
        if record.local && !from.to_canonical().is_loopback() {
            // Le jeton de l'oreille du PC n'a de valeur que sur le PC : venu d'ailleurs,
            // il a fuité. La vraie session, elle, reste ouverte.
            warn!(oreille = %ear_id, %from, "jeton de l'oreille du PC présenté depuis le réseau : refusé");
            return HelloOutcome::Rejected(RejectReason::BadToken);
        }
        let replaced = self.sessions.contains_key(&ear_id);
        if replaced {
            // Nouvelle connexion de la même oreille : l'ancienne est fermée.
            info!(oreille = %ear_id, "nouvelle session : l'ancienne est fermée");
            self.drop_session(&ear_id);
        } else if self.sessions.len() >= MAX_SESSIONS {
            warn!(
                max = MAX_SESSIONS,
                "trop d'oreilles connectées : connexion refusée"
            );
            return HelloOutcome::Full;
        }
        let id = self.next_id;
        self.next_id += 1;
        let session = Session {
            id,
            record: record.clone(),
            outbox,
            offline,
        };
        self.sessions.insert(ear_id.clone(), session);
        let status = self.status();
        // File neuve (64 places) : ces envois ne peuvent pas échouer faute de place.
        self.send(
            &ear_id,
            BrainMessage::Welcome {
                version: PROTOCOL_VERSION,
                brain_id: self.settings.brain_id.clone(),
                brain_name: self.settings.brain_name.clone(),
                status: status.clone(),
            },
        );
        self.send(&ear_id, BrainMessage::Status(status));
        self.offer_offline_phrase(&ear_id);
        info!(oreille = %ear_id, nom = %record.name, "oreille connectée");
        self.broadcast_status_except(&ear_id);
        HelloOutcome::Accepted {
            record,
            session: id,
        }
    }

    /// Ferme la session de `ear` (file lâchée : la tâche d'écriture s'arrête) et
    /// prévient l'arbitre.
    fn drop_session(&mut self, ear: &EarId) {
        if self.sessions.remove(ear).is_none() {
            return;
        }
        self.captures.remove(ear);
        let actions = self.arbiter.disconnected(ear);
        self.apply(actions);
        self.broadcast_status();
    }

    /// Envoie à la session courante de `ear`. Faux si elle n'existe pas ; file pleine :
    /// oreille trop lente, session fermée.
    fn send(&mut self, ear: &EarId, message: BrainMessage) -> bool {
        let Some(session) = self.sessions.get(ear) else {
            return false;
        };
        match session.outbox.try_send(message) {
            Ok(()) => true,
            Err(mpsc::error::TrySendError::Full(_)) => {
                warn!(oreille = %ear, "oreille trop lente : session fermée");
                self.drop_session(ear);
                false
            }
            Err(mpsc::error::TrySendError::Closed(_)) => false,
        }
    }

    fn status(&self) -> StatusInfo {
        let brain = &self.services.brain;
        StatusInfo {
            mode: brain.mode(),
            version: env!("CARGO_PKG_VERSION").into(),
            local_model: brain.local_model().into(),
            ears: self.summaries(),
        }
    }

    fn summaries(&self) -> Vec<EarSummary> {
        self.registry
            .list()
            .into_iter()
            .map(|r| EarSummary {
                connected: self.sessions.contains_key(&r.id),
                id: r.id,
                name: r.name,
                platform: r.platform,
                local: r.local,
            })
            .collect()
    }

    fn broadcast_status(&mut self) {
        self.broadcast(None);
    }

    fn broadcast_status_except(&mut self, ear: &EarId) {
        self.broadcast(Some(ear));
    }

    fn broadcast(&mut self, except: Option<&EarId>) {
        let status = self.status();
        let ears: Vec<EarId> = self
            .sessions
            .keys()
            .filter(|e| Some(*e) != except)
            .cloned()
            .collect();
        for ear in ears {
            self.send(&ear, BrainMessage::Status(status.clone()));
        }
    }

    /// Ferme les sessions des oreilles qui ne sont plus au registre.
    fn prune_sessions(&mut self) {
        let gone: Vec<EarId> = self
            .sessions
            .keys()
            .filter(|e| self.registry.get(e).is_none())
            .cloned()
            .collect();
        for ear in gone {
            info!(oreille = %ear, "oreille retirée : session fermée");
            self.drop_session(&ear);
        }
    }

    // --- Messages des oreilles ---

    fn received(&mut self, ear: &EarId, message: EarMessage) {
        match message {
            EarMessage::Wake {
                utterance,
                score,
                level_db,
                muted,
            } => self.wake(ear, utterance, score, level_db, muted),
            EarMessage::ListenStart { utterance } => {
                if self.arbiter.follow_up(ear, utterance) {
                    self.captures
                        .insert(ear.clone(), Capture::new(utterance, true));
                    self.send(ear, BrainMessage::Elected { utterance });
                } else {
                    debug!(oreille = %ear, "réponse refusée : une phrase est en cours");
                    self.send(ear, BrainMessage::Stop { utterance });
                }
            }
            EarMessage::ListenEnd {
                utterance,
                muted,
                no_speech,
                cut,
            } => self.ended(
                ear,
                utterance,
                End {
                    muted,
                    no_speech,
                    cut,
                },
            ),
            EarMessage::Status => {
                let response = self.control_status();
                self.send(
                    ear,
                    BrainMessage::Response {
                        utterance: None,
                        response,
                    },
                );
            }
            EarMessage::SetMode { mode } => {
                let local = self.sessions.get(ear).is_some_and(|s| s.record.local);
                if mode == Mode::Online && !local {
                    // À la voix, le mode en ligne demande une confirmation exacte : une
                    // oreille d'une autre machine ne peut pas s'en passer.
                    warn!(oreille = %ear, "mode en ligne demandé par une oreille distante : refusé");
                    self.send(
                        ear,
                        BrainMessage::Response {
                            utterance: None,
                            response: ControlResponse::Error {
                                message: ONLINE_FROM_PC_ONLY.into(),
                            },
                        },
                    );
                    return;
                }
                let response = match self.services.brain.set_mode(mode) {
                    Ok(()) => {
                        info!(oreille = %ear, mode = %mode, "mode changé par une oreille");
                        self.control_status()
                    }
                    Err(e) => ControlResponse::Error {
                        message: format!("Impossible d'enregistrer le mode : {e}."),
                    },
                };
                let changed = matches!(response, ControlResponse::Status { .. });
                self.send(
                    ear,
                    BrainMessage::Response {
                        utterance: None,
                        response,
                    },
                );
                // Avec l'abonnement, la diffusion vient du changement lui-même.
                if changed && self.services.modes.is_none() {
                    self.broadcast_status();
                }
            }
            EarMessage::Hello { .. } | EarMessage::Pair { .. } => {
                debug!(oreille = %ear, "hello ou pair en pleine session : ignoré");
            }
            EarMessage::Unknown => debug!(oreille = %ear, "message inconnu ignoré"),
        }
    }

    fn control_status(&self) -> ControlResponse {
        let brain = &self.services.brain;
        ControlResponse::Status {
            mode: brain.mode(),
            version: env!("CARGO_PKG_VERSION").into(),
            local_model: brain.local_model().into(),
        }
    }

    fn wake(&mut self, ear: &EarId, utterance: u32, score: f32, level_db: f32, muted: bool) {
        let Some(record) = self.sessions.get(ear).map(|s| s.record.clone()) else {
            return;
        };
        let report = WakeReport {
            ear: ear.clone(),
            utterance,
            score,
            level_db,
            muted,
            bias: record.bias,
            duck: record.duck,
        };
        let actions = self.arbiter.wake(report, Instant::now());
        let refused = actions.iter().any(|a| {
            matches!(a, ArbiterAction::Stop { ear: e, utterance: u } if e == ear && *u == utterance)
        });
        debug!(oreille = %ear, phrase = utterance, refuse = refused, "réveil");
        if !refused
            && !self
                .captures
                .get(ear)
                .is_some_and(|c| c.utterance == utterance)
        {
            self.captures
                .insert(ear.clone(), Capture::new(utterance, false));
        }
        self.apply(actions);
    }

    fn audio(&mut self, ear: &EarId, utterance: u32, samples: &[i16]) {
        let Some(capture) = self.captures.get_mut(ear) else {
            return;
        };
        if capture.utterance != utterance || capture.ended.is_some() {
            return;
        }
        if capture.samples.len() + samples.len() > MAX_PHRASE_SAMPLES {
            warn!(oreille = %ear, "phrase trop longue : abandonnée");
            self.cancel(ear, utterance);
            return;
        }
        capture.samples.extend_from_slice(samples);
        capture.heard = Instant::now();
    }

    /// Phrase abandonnée : son rétabli si besoin, `Stop` à l'oreille.
    fn cancel(&mut self, ear: &EarId, utterance: u32) {
        self.captures.remove(ear);
        let actions = self.arbiter.cancelled(ear, utterance);
        self.send(ear, BrainMessage::Stop { utterance });
        self.apply(actions);
    }

    fn expire_captures(&mut self) {
        // (oreille, phrase, sans fin depuis CAPTURE_TIMEOUT ; sinon : plus de son).
        let late: Vec<(EarId, u32, bool)> = self
            .captures
            .iter()
            .filter(|(_, c)| c.ended.is_none())
            .filter_map(|(e, c)| {
                let endless = c.started.elapsed() > CAPTURE_TIMEOUT;
                (endless || c.heard.elapsed() > AUDIO_STALL)
                    .then(|| (e.clone(), c.utterance, endless))
            })
            .collect();
        for (ear, utterance, endless) in late {
            if endless {
                warn!(oreille = %ear, "phrase sans fin : abandonnée");
            } else {
                warn!(oreille = %ear, "plus de son de l'oreille : phrase abandonnée");
            }
            self.cancel(&ear, utterance);
        }
    }

    fn ended(&mut self, ear: &EarId, utterance: u32, end: End) {
        match self.arbiter.ended(ear, utterance) {
            EndVerdict::Process => {
                let capture = self
                    .captures
                    .entry(ear.clone())
                    .or_insert_with(|| Capture::new(utterance, false));
                if capture.utterance == utterance {
                    capture.ended = Some(end);
                } else {
                    *capture = Capture::new(utterance, false);
                    capture.ended = Some(end);
                }
                self.process(ear, utterance);
            }
            EndVerdict::Wait => {
                let capture = self
                    .captures
                    .entry(ear.clone())
                    .or_insert_with(|| Capture::new(utterance, false));
                if capture.utterance == utterance {
                    capture.ended = Some(end);
                }
            }
            EndVerdict::Ignore => {
                if self
                    .captures
                    .get(ear)
                    .is_some_and(|c| c.utterance == utterance)
                {
                    self.captures.remove(ear);
                }
            }
        }
    }

    fn apply(&mut self, actions: Vec<ArbiterAction>) {
        let mut ready = Vec::new();
        for action in actions {
            match action {
                ArbiterAction::Duck => {
                    let _ = self.sound.send(SoundCmd::Duck);
                }
                ArbiterAction::Restore => {
                    let _ = self.sound.send(SoundCmd::Restore);
                }
                ArbiterAction::Elect { ear, utterance } => {
                    info!(oreille = %ear, phrase = utterance, "oreille élue");
                    self.send(&ear, BrainMessage::Elected { utterance });
                    if self
                        .captures
                        .get(&ear)
                        .is_some_and(|c| c.utterance == utterance && c.ended.is_some())
                    {
                        ready.push((ear, utterance));
                    }
                }
                ArbiterAction::Stop { ear, utterance } => {
                    debug!(oreille = %ear, phrase = utterance, "stop");
                    if self
                        .captures
                        .get(&ear)
                        .is_some_and(|c| c.utterance == utterance)
                    {
                        self.captures.remove(&ear);
                    }
                    self.send(&ear, BrainMessage::Stop { utterance });
                }
            }
        }
        // Fin reçue avant la décision : traitée maintenant.
        for (ear, utterance) in ready {
            if self.arbiter.ended(&ear, utterance) == EndVerdict::Process {
                self.process(&ear, utterance);
            }
        }
    }

    /// Lance le traitement de la phrase élue : écoute, voix, puis `Processed`.
    fn process(&mut self, ear: &EarId, utterance: u32) {
        let capture = match self.captures.remove(ear) {
            Some(c) if c.utterance == utterance => c,
            _ => Capture::new(utterance, false),
        };
        let end = capture.ended.unwrap_or_default();
        let audio = if end.no_speech {
            Vec::new()
        } else {
            without_cut(capture.samples, end.cut)
        };
        debug!(
            oreille = %ear,
            phrase = utterance,
            echantillons = audio.len(),
            "phrase à traiter"
        );
        self.services.board.touch(ear.as_str());
        self.processing += 1;
        let (listen, voice) = (self.services.listen.clone(), self.services.voice.clone());
        let (ear, follow_up, muted) = (ear.clone(), capture.follow_up, end.muted);
        let me = self.me.clone();
        let limit = self.settings.process_timeout;
        // La baisse du son de cette phrase peut être encore en file : `listen` ne
        // rétablit le son qu'après elle.
        let (flushed_tx, flushed) = oneshot::channel();
        let _ = self.sound.send(SoundCmd::Flush(flushed_tx));
        tokio::spawn(async move {
            let (job_ear, job_listen) = (ear.clone(), listen.clone());
            let mut job = tokio::spawn(async move {
                let _ = flushed.await;
                let response = job_listen.listen(&job_ear, &audio, muted, follow_up).await;
                voice.speak(response).await
            });
            let response = match tokio::time::timeout(limit, &mut job).await {
                Ok(Ok(response)) => response,
                Ok(Err(e)) => {
                    warn!(erreur = %e, "traitement d'une phrase interrompu");
                    listen.abandoned().await;
                    ControlResponse::Error {
                        message: "Traitement interrompu.".into(),
                    }
                }
                Err(_) => {
                    warn!("traitement d'une phrase trop long : abandonné");
                    // Arrêté pour de bon (la compétence en cours est lâchée), et attendu :
                    // plus rien de cette phrase ne touche l'action en attente ni le son
                    // une fois la suivante lancée.
                    job.abort();
                    let _ = job.await;
                    listen.abandoned().await;
                    ControlResponse::Error {
                        message: "Traitement trop long.".into(),
                    }
                }
            };
            if let Some(tx) = me.upgrade() {
                let _ = tx
                    .send(Command::Processed {
                        ear,
                        utterance,
                        response,
                    })
                    .await;
            }
        });
    }

    fn processed(&mut self, ear: &EarId, utterance: u32, response: ControlResponse) {
        self.processing = self.processing.saturating_sub(1);
        // Avant la réponse : le `ListenStart` qui suit une question doit passer.
        self.arbiter.finished(ear, utterance);
        let message = BrainMessage::Response {
            utterance: Some(utterance),
            response,
        };
        if !self.send(ear, message) {
            info!(oreille = %ear, "oreille partie avant la réponse");
        }
    }

    // --- Phrase hors ligne et annonces ---

    fn offline_phrase(&mut self, id: String, audio: SpeechAudio) {
        info!("phrase hors ligne prête");
        self.offline = Some((id, audio));
        let ears: Vec<EarId> = self.sessions.keys().cloned().collect();
        for ear in ears {
            self.offer_offline_phrase(&ear);
        }
    }

    /// Envoie la phrase hors ligne si l'oreille n'a pas la bonne.
    fn offer_offline_phrase(&mut self, ear: &EarId) {
        let Some((id, audio)) = &self.offline else {
            return;
        };
        let Some(session) = self.sessions.get_mut(ear) else {
            return;
        };
        if session.offline.as_deref() == Some(id.as_str()) {
            return;
        }
        session.offline = Some(id.clone());
        let message = BrainMessage::OfflinePhrase {
            id: id.clone(),
            audio: audio.clone(),
        };
        self.send(ear, message);
    }

    /// Annonces en attente : à l'oreille connectée qui peut les prendre, hors phrase.
    fn poll_announcements(&mut self) {
        let busy =
            self.processing > 0 || !self.captures.is_empty() || self.arbiter.deadline().is_some();
        if busy {
            return;
        }
        let ears: Vec<EarId> = self.sessions.keys().cloned().collect();
        for ear in ears {
            let taken = self.services.board.take_pending_for(ear.as_str());
            if taken.is_empty() {
                continue;
            }
            let voice = self.services.voice.clone();
            let me = self.me.clone();
            tokio::spawn(async move {
                let raw = taken
                    .iter()
                    .map(|p| (p.speech.clone(), p.outcome))
                    .collect();
                let items = crate::server::voiced(&voice, raw).await;
                let message = BrainMessage::Response {
                    utterance: None,
                    response: ControlResponse::Announcements { items },
                };
                if let Some(tx) = me.upgrade() {
                    let _ = tx
                        .send(Command::Deliver {
                            ear,
                            message,
                            items: taken,
                        })
                        .await;
                }
            });
            return;
        }
    }

    // --- Appairage et registre ---

    fn pair_start(&mut self) -> Result<PairTicket, String> {
        if let Some(old) = self.pairing.take() {
            let _ = old.waiter.send(PairOutcome::Ended {
                reason: pairing_ended::REPLACED.into(),
            });
        }
        let (window, code) = PairingWindow::open(Instant::now(), self.settings.pairing_ttl);
        let uri = PairingUri {
            brain_id: self.settings.brain_id.clone(),
            hosts: self.settings.hosts.clone(),
            port: self.settings.port,
            fingerprint: self.settings.fingerprint,
            code: code.clone(),
        }
        .to_string();
        let id = self.next_id;
        self.next_id += 1;
        let (waiter, outcome) = oneshot::channel();
        self.pairing = Some(OpenWindow { id, window, waiter });
        self.announce_pairing(true);
        info!("fenêtre d'appairage ouverte");
        Ok(PairTicket {
            id,
            code,
            uri,
            fingerprint_short: self.settings.fingerprint.short(),
            expires_in_secs: self.settings.pairing_ttl.as_secs(),
            outcome,
        })
    }

    fn close_pairing(&mut self, reason: &str) {
        if let Some(open) = self.pairing.take() {
            info!(raison = reason, "fenêtre d'appairage fermée");
            let _ = open.waiter.send(PairOutcome::Ended {
                reason: reason.into(),
            });
            self.announce_pairing(false);
        }
    }

    fn announce_pairing(&self, open: bool) {
        if let Some(hook) = &self.on_pairing {
            hook(open);
        }
    }

    fn pair(&mut self, code: &str, name: &str, platform: &str, from: IpAddr) -> BrainMessage {
        let closed = BrainMessage::Rejected {
            reason: RejectReason::PairingClosed,
        };
        let Some(open) = self.pairing.as_mut() else {
            debug!(%from, "appairage refusé : aucune fenêtre ouverte");
            self.refused();
            return closed;
        };
        let now = Instant::now();
        match open.window.try_code(code, from, now) {
            PairAttempt::Accepted => {
                let name = clean(name, MAX_NAME_CHARS, "Oreille");
                let platform = clean(platform, MAX_PLATFORM_CHARS, "inconnue");
                match self.registry.add(&name, &platform, false) {
                    Ok((record, token)) => {
                        info!(oreille = %record.id, nom = %name, "oreille appairée");
                        if let Some(open) = self.pairing.take() {
                            let _ = open.waiter.send(PairOutcome::Done { name });
                            self.announce_pairing(false);
                        }
                        self.broadcast_status();
                        BrainMessage::Paired {
                            ear_id: record.id,
                            token,
                            brain_id: self.settings.brain_id.clone(),
                            brain_name: self.settings.brain_name.clone(),
                        }
                    }
                    Err(e) => {
                        warn!(erreur = %e, "registre des oreilles : écriture impossible");
                        self.close_pairing(&pairing_ended::not_saved(&e.to_string()));
                        closed
                    }
                }
            }
            PairAttempt::WrongCode { attempts_left } => {
                info!(%from, restants = attempts_left, "code d'appairage faux");
                if !open.window.is_open(now) {
                    self.close_pairing(pairing_ended::TOO_MANY_ATTEMPTS);
                }
                BrainMessage::Rejected {
                    reason: RejectReason::WrongCode { attempts_left },
                }
            }
            PairAttempt::Blocked => {
                debug!(%from, "appairage refusé : essais épuisés pour cette adresse");
                BrainMessage::Rejected {
                    reason: RejectReason::WrongCode { attempts_left: 0 },
                }
            }
            PairAttempt::Closed => {
                self.close_pairing(pairing_ended::TIMED_OUT);
                closed
            }
        }
    }

    fn pair_local(&mut self, name: &str) -> ControlResponse {
        let name = clean(name, MAX_NAME_CHARS, "PC");
        match self.registry.add_local(&name) {
            Ok((record, token)) => {
                info!(oreille = %record.id, nom = %name, "oreille locale appairée");
                // L'ancienne oreille locale du même nom n'existe plus.
                self.prune_sessions();
                self.broadcast_status();
                ControlResponse::PairedLocal {
                    ear_id: record.id,
                    token,
                    port: self.settings.port,
                    fingerprint: self.settings.fingerprint,
                    brain_id: self.settings.brain_id.clone(),
                    brain_name: self.settings.brain_name.clone(),
                }
            }
            Err(e) => ControlResponse::Error {
                message: format!("Impossible d'enregistrer l'oreille : {e}."),
            },
        }
    }

    fn unpair(&mut self, ear: &str) -> ControlResponse {
        match self.registry.remove(ear.trim()) {
            Ok(Some(record)) => {
                info!(oreille = %record.id, nom = %record.name, "oreille retirée");
                self.prune_sessions();
                self.broadcast_status();
                ControlResponse::Reply {
                    speech: format!("Oreille « {} » retirée.", record.name),
                }
            }
            Ok(None) => ControlResponse::Error {
                message: format!("Aucune oreille « {} ».", ear.trim()),
            },
            Err(e) if e.kind() == std::io::ErrorKind::InvalidInput => ControlResponse::Error {
                message: e.to_string(),
            },
            Err(e) => ControlResponse::Error {
                message: format!("Impossible de retirer l'oreille : {e}."),
            },
        }
    }
}

/// Retire `cut` (plage `[début, fin)` du flux), bornée à la longueur du son.
fn without_cut(mut samples: Vec<i16>, cut: Option<(u32, u32)>) -> Vec<i16> {
    let Some((from, to)) = cut else {
        return samples;
    };
    let len = samples.len();
    let from = (from as usize).min(len);
    let to = (to as usize).clamp(from, len);
    samples.drain(from..to);
    samples
}

/// Identifiant donné par une oreille pas encore authentifiée, sûr à journaliser.
fn shown(id: &EarId) -> &str {
    let s = id.as_str();
    if s.len() <= 64 && s.bytes().all(|b| b.is_ascii_alphanumeric()) {
        s
    } else {
        "(identifiant invalide)"
    }
}

/// Texte d'une oreille : sans caractère de contrôle ni marque de direction, borné ;
/// vide : `fallback`.
fn clean(text: &str, max: usize, fallback: &str) -> String {
    let text: String = jarvis_protocol::printable(text).chars().take(max).collect();
    match text.trim() {
        "" => fallback.to_string(),
        t => t.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phrase_la_plus_longue_de_l_oreille_gardee_en_entier() {
        let sent = jarvis_protocol::frame::MAX_PHRASE_SAMPLES;
        assert!(
            MAX_PHRASE_SAMPLES >= sent + 16_000 / 2,
            "marge d'au moins 0,5 s"
        );
        // Envoyée en temps réel : la capture n'est pas tenue pour sans fin avant.
        let secs = Duration::from_secs_f64(MAX_PHRASE_SAMPLES as f64 / 16_000.0);
        assert!(
            CAPTURE_TIMEOUT >= secs + Duration::from_secs(3),
            "{CAPTURE_TIMEOUT:?}"
        );
    }

    #[test]
    fn coupe_retiree_et_bornee() {
        let s: Vec<i16> = (0..10).collect();
        assert_eq!(without_cut(s.clone(), None), s);
        assert_eq!(without_cut(s.clone(), Some((2, 5))), [0, 1, 5, 6, 7, 8, 9]);
        // Plage hors du son ou à l'envers : bornée, jamais de panique.
        assert_eq!(
            without_cut(s.clone(), Some((8, 50))),
            [0, 1, 2, 3, 4, 5, 6, 7]
        );
        assert_eq!(without_cut(s.clone(), Some((5, 2))), s);
        assert_eq!(without_cut(s.clone(), Some((40, 50))), s);
    }

    #[test]
    fn nom_d_oreille_nettoye() {
        assert_eq!(clean("  Pixel\n4 XL ", 64, "Oreille"), "Pixel4 XL");
        assert_eq!(clean("\u{7}", 64, "Oreille"), "Oreille");
        assert_eq!(clean("Salon\u{202E}gnp.exe", 64, "Oreille"), "Salongnp.exe");
        assert_eq!(clean(&"a".repeat(100), 64, "x").len(), 64);
    }

    #[test]
    fn id_de_la_phrase_hors_ligne() {
        let a = offline_phrase_id("Le PC est endormi, Monsieur.", "voix 1");
        assert_eq!(a.len(), 16);
        assert_eq!(
            a,
            offline_phrase_id("Le PC est endormi, Monsieur.", "voix 1")
        );
        assert_ne!(
            a,
            offline_phrase_id("Le PC est endormi, Monsieur.", "voix 2")
        );
        assert_ne!(a, offline_phrase_id("Le PC est endormi.", "voix 1"));
    }
}
