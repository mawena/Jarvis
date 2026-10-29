//! Serveur WebSocket des oreilles, sur TLS 1.3 (port 7341 par défaut).
//! Rien n'est accepté avant un `hello` valide (ou un `pair` pendant une fenêtre).

pub mod hub;
pub mod session;

pub use hub::{Hub, HubSettings, PairOutcome, PairTicket, PairingHook, Services};

use crate::config::NetworkConfig;
use crate::discovery::{Advertiser, machine_name};
use crate::ears::EarRegistry;
use crate::pairing::PAIRING_TTL;
use crate::tls::BrainIdentity;
use futures_util::{SinkExt, StreamExt};
use hub::HelloOutcome;
use jarvis_protocol::ear::{BrainMessage, EarMessage, PROTOCOL_VERSION, RejectReason};
use jarvis_protocol::{EarId, Fingerprint};
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Semaphore, mpsc};
use tokio::task::JoinHandle;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tracing::{debug, info, warn};

/// Message reçu par le cerveau, au plus.
pub const MAX_MESSAGE_BYTES: usize = 1024 * 1024;
/// Sessions ouvertes au plus.
pub const MAX_SESSIONS: usize = 16;
/// Poignées de main en cours au plus.
pub const MAX_HANDSHAKES: usize = 8;
/// Délai pour TLS, puis pour le premier message.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
/// Son d'une phrase gardé au plus : 1 s de tête + 12 s, à 16 kHz.
pub const MAX_PHRASE_SAMPLES: usize = 13 * 16_000;
/// Fenêtre de l'arbitre au plus : l'oreille attend son élection 2 s après `wake`
/// (`ELECTION_TIMEOUT` de `jarvis-ear-core`), puis tient le PC pour endormi.
pub const MAX_ARBITER_WINDOW: Duration = Duration::from_secs(1);

/// `[network] arbiter_window_ms`, bornée à [`MAX_ARBITER_WINDOW`].
fn arbiter_window(ms: u64) -> Duration {
    Duration::from_millis(ms).min(MAX_ARBITER_WINDOW)
}

/// Délais du serveur (raccourcis dans les tests).
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub handshake_timeout: Duration,
    pub idle_timeout: Duration,
    pub ping_every: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            handshake_timeout: HANDSHAKE_TIMEOUT,
            idle_timeout: session::IDLE_TIMEOUT,
            ping_every: session::PING_EVERY,
        }
    }
}

/// Accepte les oreilles jusqu'à l'arrêt de la tâche.
pub async fn serve(
    listener: TcpListener,
    tls: Arc<rustls::ServerConfig>,
    hub: Hub,
) -> anyhow::Result<()> {
    serve_with_limits(listener, tls, hub, Limits::default()).await
}

/// Comme [`serve`], avec d'autres délais.
pub async fn serve_with_limits(
    listener: TcpListener,
    tls: Arc<rustls::ServerConfig>,
    hub: Hub,
    limits: Limits,
) -> anyhow::Result<()> {
    let acceptor = tokio_rustls::TlsAcceptor::from(tls);
    let handshakes = Arc::new(Semaphore::new(MAX_HANDSHAKES));
    loop {
        let (tcp, peer) = match listener.accept().await {
            Ok(accepted) => accepted,
            Err(e) => {
                // Trop de fichiers ouverts, connexion avortée… : on continue.
                warn!(erreur = %e, "acceptation d'une oreille impossible");
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        let Ok(permit) = handshakes.clone().try_acquire_owned() else {
            debug!(%peer, "trop de poignées de main en cours : connexion fermée");
            continue;
        };
        let (acceptor, hub) = (acceptor.clone(), hub.clone());
        tokio::spawn(async move {
            let opened = handshake(tcp, peer, acceptor, &hub, limits).await;
            drop(permit);
            if let Some(Opened {
                ws,
                ear,
                session,
                outbox,
            }) = opened
            {
                session::run(ws, ear, session, outbox, hub, limits).await;
            }
        });
    }
}

fn ws_config() -> WebSocketConfig {
    WebSocketConfig::default()
        .max_message_size(Some(MAX_MESSAGE_BYTES))
        .max_frame_size(Some(MAX_MESSAGE_BYTES))
}

/// Session prête à vivre.
struct Opened<S> {
    ws: WebSocketStream<S>,
    ear: EarId,
    session: hub::SessionId,
    outbox: mpsc::Receiver<BrainMessage>,
}

/// TLS, WebSocket et premier message. `None` : connexion refusée ou terminée
/// (appairage), déjà fermée.
async fn handshake(
    tcp: TcpStream,
    peer: SocketAddr,
    acceptor: tokio_rustls::TlsAcceptor,
    hub: &Hub,
    limits: Limits,
) -> Option<Opened<tokio_rustls::server::TlsStream<TcpStream>>> {
    let _ = tcp.set_nodelay(true);
    let tls = match tokio::time::timeout(limits.handshake_timeout, acceptor.accept(tcp)).await {
        Ok(Ok(tls)) => tls,
        Ok(Err(e)) => {
            debug!(%peer, "TLS refusé : {e}");
            return None;
        }
        Err(_) => {
            debug!(%peer, "TLS trop lent : connexion fermée");
            return None;
        }
    };
    let deadline = tokio::time::Instant::now() + limits.handshake_timeout;
    let accepted = tokio::time::timeout_at(
        deadline,
        tokio_tungstenite::accept_async_with_config(tls, Some(ws_config())),
    )
    .await;
    let mut ws = match accepted {
        Ok(Ok(ws)) => ws,
        Ok(Err(e)) => {
            debug!(%peer, "WebSocket refusé : {e}");
            return None;
        }
        Err(_) => {
            debug!(%peer, "WebSocket trop lent : connexion fermée");
            return None;
        }
    };
    let Some(first) = first_message(&mut ws, deadline).await else {
        debug!(%peer, "pas de hello valide à temps : connexion fermée");
        close(&mut ws).await;
        return None;
    };
    match first {
        EarMessage::Hello {
            version,
            ear_id,
            token,
            offline_phrase,
        } => {
            if version != PROTOCOL_VERSION {
                info!(%peer, version, "oreille d'une autre version refusée");
                reject(&mut ws, version_rejected()).await;
                return None;
            }
            let (tx, rx) = mpsc::channel(session::OUTBOX);
            match hub.hello(ear_id.clone(), token, offline_phrase, tx).await {
                HelloOutcome::Accepted { record, session } => {
                    debug!(%peer, oreille = %record.id, "session ouverte");
                    Some(Opened {
                        ws,
                        ear: ear_id,
                        session,
                        outbox: rx,
                    })
                }
                HelloOutcome::Rejected(reason) => {
                    reject(&mut ws, reason).await;
                    None
                }
                HelloOutcome::Full => {
                    close(&mut ws).await;
                    None
                }
            }
        }
        EarMessage::Pair {
            version,
            code,
            name,
            platform,
        } => {
            if version != PROTOCOL_VERSION {
                reject(&mut ws, version_rejected()).await;
                return None;
            }
            let answer = hub.pair(code, name, platform).await;
            send_then_close(&mut ws, &answer).await;
            None
        }
        _ => {
            debug!(%peer, "premier message inattendu : connexion fermée");
            close(&mut ws).await;
            None
        }
    }
}

fn version_rejected() -> RejectReason {
    RejectReason::Version {
        supported: vec![PROTOCOL_VERSION],
    }
}

/// Premier message texte avant `deadline`. Trame binaire (son avant `hello`),
/// fermeture ou message illisible : `None`.
async fn first_message<S>(
    ws: &mut WebSocketStream<S>,
    deadline: tokio::time::Instant,
) -> Option<EarMessage>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    loop {
        let message = tokio::time::timeout_at(deadline, ws.next())
            .await
            .ok()??
            .ok()?;
        match message {
            Message::Text(text) => return serde_json::from_str(text.as_str()).ok(),
            Message::Ping(_) | Message::Pong(_) => continue,
            _ => return None,
        }
    }
}

async fn reject<S>(ws: &mut WebSocketStream<S>, reason: RejectReason)
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    send_then_close(ws, &BrainMessage::Rejected { reason }).await;
}

async fn send_then_close<S>(ws: &mut WebSocketStream<S>, message: &BrainMessage)
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    if let Ok(text) = serde_json::to_string(message) {
        let _ = tokio::time::timeout(Duration::from_secs(1), ws.send(Message::text(text))).await;
    }
    close(ws).await;
}

async fn close<S>(ws: &mut WebSocketStream<S>)
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let _ = tokio::time::timeout(Duration::from_secs(1), ws.close(None)).await;
}

/// Fichiers du réseau des oreilles.
#[derive(Debug, Clone)]
pub struct NetworkPaths {
    /// Certificat, clé et identifiant du cerveau.
    pub tls_dir: PathBuf,
    /// Registre des oreilles appairées.
    pub ears_file: PathBuf,
}

impl Default for NetworkPaths {
    /// `~/.local/share/jarvis/tls/` et `~/.local/share/jarvis/ears.toml`.
    fn default() -> Self {
        NetworkPaths {
            tls_dir: BrainIdentity::default_dir(),
            ears_file: EarRegistry::default_path(),
        }
    }
}

/// Réseau des oreilles en marche.
pub struct Network {
    pub hub: Hub,
    pub port: u16,
    pub fingerprint: Fingerprint,
    advertiser: Arc<Mutex<Option<Advertiser>>>,
    server: JoinHandle<()>,
}

impl Network {
    /// Arrêt : serveur coupé, annonce mDNS retirée (jusqu'à 2 s).
    pub async fn shutdown(self) {
        self.server.abort();
        let advertiser = self
            .advertiser
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(advertiser) = advertiser {
            let _ = tokio::task::spawn_blocking(move || advertiser.shutdown()).await;
        }
    }
}

/// Démarre le réseau des oreilles : identité TLS, registre, port, annonce mDNS (si
/// `mdns`), hub et serveur. Désactivé ou impossible (identité illisible, registre
/// corrompu, port pris…) : avertissement et `None`, le cerveau tourne sans.
pub async fn start(
    config: &NetworkConfig,
    paths: &NetworkPaths,
    services: Services,
    mdns: bool,
) -> Option<Network> {
    if !config.enabled {
        info!("réseau des oreilles désactivé ([network] enabled = false)");
        return None;
    }
    let identity = match BrainIdentity::load_or_create(&paths.tls_dir) {
        Ok(identity) => identity,
        Err(e) => {
            warn!(erreur = %format!("{e:#}"), "identité TLS illisible : réseau des oreilles désactivé");
            return None;
        }
    };
    let tls = match identity.server_config() {
        Ok(tls) => tls,
        Err(e) => {
            warn!(erreur = %format!("{e:#}"), "TLS impossible : réseau des oreilles désactivé");
            return None;
        }
    };
    let registry = match EarRegistry::load(&paths.ears_file) {
        Ok(registry) => registry,
        Err(e) => {
            warn!(erreur = %e, "registre des oreilles illisible : réseau des oreilles désactivé");
            return None;
        }
    };
    let addr: SocketAddr = match config.listen.trim().parse() {
        Ok(addr) => addr,
        Err(_) => {
            warn!(
                adresse = %config.listen,
                "[network] listen invalide (attendu ip:port) : réseau des oreilles désactivé"
            );
            return None;
        }
    };
    let listener = match TcpListener::bind(addr).await {
        Ok(listener) => listener,
        Err(e) => {
            warn!(%addr, erreur = %e, "port des oreilles indisponible : réseau des oreilles désactivé");
            return None;
        }
    };
    let port = listener.local_addr().map_or(addr.port(), |a| a.port());
    let name = match config.name.trim() {
        "" => machine_name(),
        name => name.to_string(),
    };

    let advertiser = Arc::new(Mutex::new(None));
    if mdns {
        match Advertiser::start(&name, &identity.brain_id, port) {
            Ok(a) => *advertiser.lock().unwrap_or_else(|e| e.into_inner()) = Some(a),
            Err(e) => {
                warn!(erreur = %format!("{e:#}"), "annonce mDNS impossible : adresse manuelle seulement")
            }
        }
    }
    let hook: PairingHook = {
        let advertiser = advertiser.clone();
        Arc::new(move |open| {
            if let Some(a) = advertiser
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_ref()
                && let Err(e) = a.set_pairing(open)
            {
                warn!(erreur = %format!("{e:#}"), "annonce mDNS de l'appairage impossible");
            }
        })
    };

    let window = arbiter_window(config.arbiter_window_ms);
    if window.as_millis() < u128::from(config.arbiter_window_ms) {
        warn!(
            demandee_ms = config.arbiter_window_ms,
            retenue_ms = window.as_millis() as u64,
            "[network] arbiter_window_ms trop grande (les oreilles attendent leur élection 2 s) : bornée"
        );
    }
    let settings = HubSettings {
        brain_id: identity.brain_id.clone(),
        brain_name: name,
        port,
        fingerprint: identity.fingerprint,
        hosts: lan_hosts(addr.ip()),
        window,
        pairing_ttl: PAIRING_TTL,
    };
    let offline_text = services.brain.persona().addressed("Le PC est endormi.");
    let offline_id = hub::offline_phrase_id(&offline_text, &services.voice_tag);
    let voice = services.voice.clone();
    let hub = Hub::spawn(settings, registry, services, Some(hook));

    // Phrase « le PC est endormi », dite par la voix de Jarvis, une fois.
    let for_phrase = hub.clone();
    tokio::spawn(async move {
        match voice.audio_for(&offline_text).await {
            Some(audio) => for_phrase.set_offline_phrase(offline_id, audio).await,
            None => info!("pas de voix : les oreilles n'auront pas la phrase hors ligne"),
        }
    });

    let server_hub = hub.clone();
    let server = tokio::spawn(async move {
        if let Err(e) = serve(listener, tls, server_hub).await {
            warn!(erreur = %format!("{e:#}"), "serveur des oreilles arrêté");
        }
    });
    info!(
        port,
        empreinte = %identity.fingerprint.short(),
        "serveur des oreilles prêt"
    );
    Some(Network {
        hub,
        port,
        fingerprint: identity.fingerprint,
        advertiser,
        server,
    })
}

/// Adresses du PC pour l'URI d'appairage : l'adresse d'écoute si elle est précise,
/// sinon celle de la route par défaut (aucun paquet envoyé).
fn lan_hosts(listen: IpAddr) -> Vec<IpAddr> {
    if !listen.is_unspecified() {
        return if listen.is_loopback() {
            Vec::new()
        } else {
            vec![listen]
        };
    }
    let routed = std::net::UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| {
            // Adresse de documentation (RFC 5737) : seule la table de routage est lue.
            s.connect("192.0.2.1:9")?;
            s.local_addr()
        })
        .map(|a| a.ip());
    match routed {
        Ok(ip) if !ip.is_unspecified() && !ip.is_loopback() => vec![ip],
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fenetre_de_l_arbitre_bornee_sous_le_delai_de_l_oreille() {
        assert_eq!(arbiter_window(300), Duration::from_millis(300));
        assert_eq!(arbiter_window(0), Duration::ZERO);
        // Au-delà, l'oreille tiendrait le PC pour endormi avant son élection.
        assert_eq!(arbiter_window(5_000), MAX_ARBITER_WINDOW);
        // Marge d'au moins une seconde pour le réseau et le traitement.
        assert!(
            MAX_ARBITER_WINDOW + Duration::from_secs(1) <= jarvis_ear_core::net::ELECTION_TIMEOUT
        );
    }
}
