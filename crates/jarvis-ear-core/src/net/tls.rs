//! TLS de l'oreille : certificat du cerveau épinglé par son SHA-256 (pas d'autorité
//! de certification, pas de nom d'hôte), TLS 1.3 seulement, fournisseur `ring`
//! toujours donné explicitement.
// Squelette (tâche 1) : à retirer par la tâche 2.
#![allow(dead_code, unused_variables)]

use super::{AsyncStream, Connector};
use jarvis_protocol::Fingerprint;
use rustls::ClientConfig;
use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

/// Nom présenté au cerveau (SNI) ; jamais vérifié.
pub const SERVER_NAME: &str = "jarvis-brain";

/// N'accepte que le certificat dont le SHA-256 vaut `fingerprint`.
pub fn pinned_client_config(fingerprint: &Fingerprint) -> Arc<ClientConfig> {
    todo!("tâche 2")
}

/// « Au premier contact » (appairage par code seul) : accepte tout certificat et
/// retient l'empreinte vue, à faire comparer à l'œil.
pub fn tofu_client_config() -> (Arc<ClientConfig>, Arc<Mutex<Option<Fingerprint>>>) {
    todo!("tâche 2")
}

/// TCP puis TLS avec `config`.
pub struct TlsConnector {
    config: Arc<ClientConfig>,
}

impl TlsConnector {
    pub fn new(config: Arc<ClientConfig>) -> Self {
        todo!("tâche 2")
    }
}

#[async_trait::async_trait]
impl Connector for TlsConnector {
    async fn connect(&self, addr: SocketAddr) -> io::Result<Box<dyn AsyncStream>> {
        todo!("tâche 2")
    }
}
