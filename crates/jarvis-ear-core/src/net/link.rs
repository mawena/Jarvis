//! Lien avec le cerveau : connexion, `hello`, reconnexion avec délai croissant,
//! battement de cœur. Hors connexion, rien n'est envoyé ni gardé.
// Squelette (tâche 1) : à retirer par la tâche 4.
#![allow(dead_code, unused_variables)]

use super::store::{Pairing, PairingStore};
use super::{AsyncStream, Connector, Discovery};
use jarvis_protocol::EarId;
use jarvis_protocol::ear::{BrainMessage, EarMessage, RejectReason, StatusInfo};
use std::fmt;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, watch};

/// Ping toutes les 5 s.
pub const DEFAULT_HEARTBEAT: Duration = Duration::from_secs(5);
/// Rien reçu pendant 15 s : connexion tenue pour morte.
pub const DEFAULT_DEAD_AFTER: Duration = Duration::from_secs(15);

pub struct LinkConfig {
    pub pairing: Pairing,
    pub store: PairingStore,
    pub connector: Arc<dyn Connector>,
    pub discovery: Option<Arc<dyn Discovery>>,
    pub heartbeat: Duration,
    pub dead_after: Duration,
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

pub struct EarLink {
    // Canaux vers la tâche du lien : tâche 4.
}

impl EarLink {
    pub fn spawn(config: LinkConfig) -> (EarLink, mpsc::Receiver<LinkEvent>) {
        todo!("tâche 4")
    }

    pub fn send(&self, message: EarMessage) -> Result<(), LinkDown> {
        todo!("tâche 4")
    }

    /// Jamais mis en attente hors connexion : échoue tout de suite.
    pub fn send_audio(&self, utterance: u32, samples: &[i16]) -> Result<(), LinkDown> {
        todo!("tâche 4")
    }

    /// Nouvelle tentative de connexion, tout de suite.
    pub fn nudge(&self) {
        todo!("tâche 4")
    }

    pub fn state(&self) -> watch::Receiver<LinkState> {
        todo!("tâche 4")
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

/// Échange `pair` → `paired` / `rejected` sur un flux déjà ouvert (TLS compris).
pub async fn pair_with_code(
    stream: Box<dyn AsyncStream>,
    code: &str,
    name: &str,
    platform: &str,
) -> Result<Paired, PairError> {
    todo!("tâche 4")
}
