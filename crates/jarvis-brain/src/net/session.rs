//! Session d'une oreille authentifiée : lecture et écriture en parallèle (file
//! bornée à 64). Texte → `EarMessage`, binaire → `decode_frame`. Ping du cerveau ;
//! fermée après 30 s sans rien reçu ; à la fermeture, `hub.disconnected(ear)`.

use super::Limits;
use super::hub::{Hub, SessionId};
use futures_util::{SinkExt, StreamExt};
use jarvis_protocol::EarId;
use jarvis_protocol::ear::{BrainMessage, EarMessage};
use jarvis_protocol::frame::{Frame, decode_frame};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, info, warn};

/// Rien reçu pendant ce délai : session fermée.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(30);
/// Ping du cerveau.
pub const PING_EVERY: Duration = Duration::from_secs(10);
/// Messages en attente d'écriture, au plus.
pub const OUTBOX: usize = 64;
/// Un envoi bloqué plus longtemps : oreille tenue pour morte.
const WRITE_TIMEOUT: Duration = Duration::from_secs(15);

/// Fait vivre la session jusqu'à sa fin (oreille partie, muette, trame invalide, ou
/// file lâchée par le hub).
pub async fn run<S>(
    ws: WebSocketStream<S>,
    ear: EarId,
    session: SessionId,
    outbox: mpsc::Receiver<BrainMessage>,
    hub: Hub,
    limits: Limits,
) where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    let (sink, stream) = ws.split();
    tokio::select! {
        () = write_loop(sink, outbox, limits.ping_every) => {}
        () = read_loop(stream, &ear, session, &hub, limits.idle_timeout) => {}
    }
    hub.disconnected(&ear, session).await;
}

async fn write_loop<S>(
    mut sink: futures_util::stream::SplitSink<WebSocketStream<S>, Message>,
    mut outbox: mpsc::Receiver<BrainMessage>,
    ping_every: Duration,
) where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut ping = tokio::time::interval_at(tokio::time::Instant::now() + ping_every, ping_every);
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        let message = tokio::select! {
            message = outbox.recv() => match message {
                Some(message) => match serde_json::to_string(&message) {
                    Ok(text) => Message::text(text),
                    Err(e) => {
                        warn!(erreur = %e, "message pour l'oreille impossible à écrire");
                        continue;
                    }
                },
                // Session fermée par le hub (remplacée, oreille retirée…).
                None => {
                    let _ = tokio::time::timeout(Duration::from_secs(1), sink.send(Message::Close(None))).await;
                    return;
                }
            },
            _ = ping.tick() => Message::Ping(Default::default()),
        };
        match tokio::time::timeout(WRITE_TIMEOUT, sink.send(message)).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                debug!("écriture vers l'oreille : {e}");
                return;
            }
            Err(_) => {
                warn!("écriture vers l'oreille bloquée : session fermée");
                return;
            }
        }
    }
}

async fn read_loop<S>(
    mut stream: futures_util::stream::SplitStream<WebSocketStream<S>>,
    ear: &EarId,
    session: SessionId,
    hub: &Hub,
    idle: Duration,
) where
    S: AsyncRead + AsyncWrite + Unpin,
{
    loop {
        let message = match tokio::time::timeout(idle, stream.next()).await {
            Err(_) => {
                info!(oreille = %ear, "oreille muette : session fermée");
                return;
            }
            Ok(None) | Ok(Some(Ok(Message::Close(_)))) => return,
            Ok(Some(Err(e))) => {
                // Message trop grand compris ; jamais le contenu.
                info!(oreille = %ear, "session fermée : {e}");
                return;
            }
            Ok(Some(Ok(message))) => message,
        };
        match message {
            Message::Text(text) => match serde_json::from_str::<EarMessage>(text.as_str()) {
                Ok(message) => hub.received(ear, session, message).await,
                // Ni le contenu ni le détail (un jeton pourrait s'y trouver).
                Err(e) => warn!(oreille = %ear, "message illisible ({:?})", e.classify()),
            },
            Message::Binary(bytes) => match decode_frame(&bytes) {
                Ok(Frame::MicAudio { utterance, samples }) => {
                    hub.audio(ear, session, utterance, samples).await;
                }
                Ok(Frame::Unknown(kind)) => debug!(oreille = %ear, kind, "trame inconnue ignorée"),
                Err(e) => {
                    warn!(oreille = %ear, "trame invalide ({e}) : session fermée");
                    return;
                }
            },
            // Ping : pong rendu par tungstenite.
            _ => {}
        }
    }
}
