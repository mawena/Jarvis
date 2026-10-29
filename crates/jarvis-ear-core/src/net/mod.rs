//! Réseau des oreilles : connexion au cerveau (WebSocket sur TLS épinglé),
//! reconnexion, découverte, appairage, phrase hors ligne. Portable (Linux, Android,
//! iOS) : ni `cpal`, ni `directories`, ni D-Bus ; les chemins viennent de l'appelant.

pub mod backoff;
pub mod batch;
pub mod discovery;
pub mod link;
pub mod store;
pub mod tls;

pub use link::{
    EarLink, LinkConfig, LinkDown, LinkEvent, LinkState, PairError, Paired, pair_with_code,
};
pub use store::{Pairing, PairingStore};

use std::io;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};

/// Sans `elected` ni `stop` dans ce délai après `wake` : le PC est tenu pour endormi.
pub const ELECTION_TIMEOUT: Duration = Duration::from_secs(2);

/// Flux d'octets vers le cerveau (TLS en vrai, TCP simple dans les tests).
pub trait AsyncStream: AsyncRead + AsyncWrite + Unpin + Send {}

impl<T: AsyncRead + AsyncWrite + Unpin + Send> AsyncStream for T {}

/// Ouvre un flux vers une adresse du cerveau.
#[async_trait::async_trait]
pub trait Connector: Send + Sync {
    async fn connect(&self, addr: SocketAddr) -> io::Result<Box<dyn AsyncStream>>;
}

/// Trouve les adresses d'un cerveau (mDNS en vrai).
#[async_trait::async_trait]
pub trait Discovery: Send + Sync {
    async fn find(&self, brain_id: &str, wait: Duration) -> Vec<SocketAddr>;
}
