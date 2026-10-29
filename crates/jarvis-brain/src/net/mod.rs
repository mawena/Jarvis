//! Serveur WebSocket des oreilles, sur TLS 1.3 (port 7341 par défaut).
//! Rien n'est accepté avant un `hello` valide (ou un `pair` pendant une fenêtre).
// Squelette (tâche 1) : à retirer par la tâche 6.
#![allow(dead_code, unused_variables)]

pub mod hub;
pub mod session;

pub use hub::Hub;

use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;

/// Message reçu par le cerveau, au plus.
pub const MAX_MESSAGE_BYTES: usize = 1024 * 1024;
/// Sessions ouvertes au plus.
pub const MAX_SESSIONS: usize = 16;
/// Poignées de main en cours au plus.
pub const MAX_HANDSHAKES: usize = 8;
/// Délai pour TLS, puis pour le premier message.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

/// Accepte les oreilles jusqu'à une erreur d'acceptation.
pub async fn serve(
    listener: TcpListener,
    tls: Arc<rustls::ServerConfig>,
    hub: Hub,
) -> anyhow::Result<()> {
    todo!("tâche 6")
}
