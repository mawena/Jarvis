//! Session d'une oreille authentifiée : une tâche de lecture, une d'écriture (canal
//! borné à 64). Texte → `EarMessage`, binaire → `decode_frame`. Fermée après 30 s
//! sans rien reçu ; à la fermeture, `hub.disconnected(ear)`.
// Squelette (tâche 1) : à retirer par la tâche 6.
#![allow(dead_code, unused_variables)]

use super::Hub;
use crate::ears::EarRecord;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio_rustls::server::TlsStream;
use tokio_tungstenite::WebSocketStream;

/// Rien reçu pendant ce délai : session fermée.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(30);
/// Messages en attente d'écriture, au plus.
pub const OUTBOX: usize = 64;

pub async fn run(
    ws: WebSocketStream<TlsStream<TcpStream>>,
    ear: EarRecord,
    hub: Hub,
) -> anyhow::Result<()> {
    todo!("tâche 6")
}
