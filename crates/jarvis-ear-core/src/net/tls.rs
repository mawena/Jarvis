//! TLS de l'oreille : certificat du cerveau épinglé par son SHA-256 (pas d'autorité
//! de certification, pas de nom d'hôte), TLS 1.3 seulement, fournisseur `ring`
//! toujours donné explicitement.

use super::{AsyncStream, Connector};
use jarvis_protocol::Fingerprint;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, WebPkiSupportedAlgorithms};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{CertificateError, ClientConfig, DigitallySignedStruct, SignatureScheme};
use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Nom présenté au cerveau (SNI) ; jamais vérifié.
pub const SERVER_NAME: &str = "jarvis-brain";

/// Délai pour la connexion TCP puis la poignée de main TLS.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// N'accepte que le certificat dont le SHA-256 vaut `fingerprint`.
pub fn pinned_client_config(fingerprint: &Fingerprint) -> Arc<ClientConfig> {
    let provider = ring_provider();
    let verifier = PinnedVerifier {
        expected: *fingerprint,
        seen: None,
        algorithms: provider.signature_verification_algorithms,
    };
    client_config(provider, verifier)
}

/// « Au premier contact » (appairage par code seul) : accepte tout certificat et
/// retient l'empreinte vue, à faire comparer à l'œil.
pub fn tofu_client_config() -> (Arc<ClientConfig>, Arc<Mutex<Option<Fingerprint>>>) {
    let provider = ring_provider();
    let seen = Arc::new(Mutex::new(None));
    let verifier = PinnedVerifier {
        expected: Fingerprint::from_bytes([0; 32]),
        seen: Some(Arc::clone(&seen)),
        algorithms: provider.signature_verification_algorithms,
    };
    (client_config(provider, verifier), seen)
}

fn ring_provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

fn client_config(provider: Arc<CryptoProvider>, verifier: PinnedVerifier) -> Arc<ClientConfig> {
    let config = ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .expect("ring prend en charge TLS 1.3")
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth();
    Arc::new(config)
}

/// Vérifie l'empreinte du certificat final (ou la retient, au premier contact) ;
/// la signature de la poignée de main TLS 1.3 est toujours vérifiée.
#[derive(Debug)]
struct PinnedVerifier {
    expected: Fingerprint,
    /// `Some` : premier contact, tout certificat est accepté et son empreinte retenue.
    seen: Option<Arc<Mutex<Option<Fingerprint>>>>,
    algorithms: WebPkiSupportedAlgorithms,
}

impl ServerCertVerifier for PinnedVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let fingerprint = Fingerprint::of_der(end_entity);
        if let Some(seen) = &self.seen {
            *seen.lock().unwrap_or_else(|e| e.into_inner()) = Some(fingerprint);
            return Ok(ServerCertVerified::assertion());
        }
        if same(fingerprint.as_bytes(), self.expected.as_bytes()) {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::InvalidCertificate(
                CertificateError::ApplicationVerificationFailure,
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Err(rustls::Error::General("TLS 1.2 refusé".into()))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}

/// Comparaison en temps constant.
fn same(a: &[u8; 32], b: &[u8; 32]) -> bool {
    let diff = a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y));
    std::hint::black_box(diff) == 0
}

/// TCP puis TLS avec `config`.
pub struct TlsConnector {
    config: Arc<ClientConfig>,
}

impl TlsConnector {
    pub fn new(config: Arc<ClientConfig>) -> Self {
        TlsConnector { config }
    }
}

#[async_trait::async_trait]
impl Connector for TlsConnector {
    async fn connect(&self, addr: SocketAddr) -> io::Result<Box<dyn AsyncStream>> {
        let connector = tokio_rustls::TlsConnector::from(Arc::clone(&self.config));
        let name = ServerName::try_from(SERVER_NAME)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        let connect = async {
            let tcp = tokio::net::TcpStream::connect(addr).await?;
            tcp.set_nodelay(true)?;
            connector.connect(name, tcp).await
        };
        let tls = tokio::time::timeout(CONNECT_TIMEOUT, connect)
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "cerveau muet (TLS)"))??;
        Ok(Box::new(tls))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustls::ServerConfig;
    use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Certificat auto-signé comme celui du cerveau, et configuration du serveur.
    fn brain() -> (Arc<ServerConfig>, Fingerprint) {
        let key = rcgen::KeyPair::generate().unwrap();
        let params = rcgen::CertificateParams::new(vec!["jarvis-brain".to_string()]).unwrap();
        let cert = params.self_signed(&key).unwrap();
        let der = cert.der().to_vec();
        let config =
            ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_protocol_versions(&[&rustls::version::TLS13])
                .unwrap()
                .with_no_client_auth()
                .with_single_cert(
                    vec![CertificateDer::from(der.clone())],
                    PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())),
                )
                .unwrap();
        (Arc::new(config), Fingerprint::of_der(&der))
    }

    /// Poignée de main sur un `duplex` ; rend l'erreur du client s'il refuse.
    async fn handshake(server: Arc<ServerConfig>, client: Arc<ClientConfig>) -> io::Result<()> {
        let (a, b) = tokio::io::duplex(64 * 1024);
        let accept = async move {
            let mut tls = tokio_rustls::TlsAcceptor::from(server).accept(b).await?;
            let mut buf = [0u8; 4];
            tls.read_exact(&mut buf).await?;
            tls.write_all(&buf).await?;
            tls.flush().await?;
            io::Result::Ok(())
        };
        let connect = async move {
            let name = ServerName::try_from(SERVER_NAME).unwrap();
            let mut tls = tokio_rustls::TlsConnector::from(client)
                .connect(name, a)
                .await?;
            tls.write_all(b"ping").await?;
            tls.flush().await?;
            let mut buf = [0u8; 4];
            tls.read_exact(&mut buf).await?;
            assert_eq!(&buf, b"ping");
            io::Result::Ok(())
        };
        let (server, client) = tokio::join!(accept, connect);
        client?;
        server
    }

    #[tokio::test]
    async fn poignee_tls_avec_empreinte_epinglee() {
        let (server, fingerprint) = brain();
        handshake(server, pinned_client_config(&fingerprint))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn empreinte_differente_refusee() {
        let (server, _) = brain();
        let (_, other) = brain();
        let err = handshake(server, pinned_client_config(&other))
            .await
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData, "{err}");
    }

    /// Versions offertes par le `ClientHello` (extension `supported_versions`).
    async fn offered_versions(config: Arc<ClientConfig>) -> Vec<u16> {
        let (a, mut b) = tokio::io::duplex(64 * 1024);
        tokio::spawn(async move {
            let name = ServerName::try_from(SERVER_NAME).unwrap();
            let _ = tokio_rustls::TlsConnector::from(config)
                .connect(name, a)
                .await;
        });
        let mut header = [0u8; 5];
        b.read_exact(&mut header).await.unwrap();
        assert_eq!(header[0], 22, "enregistrement de poignée de main");
        let mut body = vec![0u8; u16::from_be_bytes([header[3], header[4]]) as usize];
        b.read_exact(&mut body).await.unwrap();
        assert_eq!(body[0], 1, "ClientHello");
        let u16_at = |i: usize| u16::from_be_bytes([body[i], body[i + 1]]);
        // Type (1) + longueur (3) + version (2) + aléa (32).
        let mut i = 4 + 2 + 32;
        i += 1 + body[i] as usize; // identifiant de session
        i += 2 + u16_at(i) as usize; // suites
        i += 1 + body[i] as usize; // compression
        let end = i + 2 + u16_at(i) as usize;
        i += 2;
        let mut versions = Vec::new();
        while i < end {
            let (kind, len) = (u16_at(i), u16_at(i + 2) as usize);
            if kind == 0x002b {
                let n = body[i + 4] as usize;
                versions = (0..n / 2).map(|k| u16_at(i + 5 + 2 * k)).collect();
            }
            i += 4 + len;
        }
        versions
    }

    /// Seul TLS 1.3 est offert. Poignée de main réelle contre un serveur TLS 1.2 :
    /// `serveur_tls12_refuse_par_l_oreille` (jarvis-brain, où rustls a TLS 1.2).
    #[tokio::test]
    async fn tls12_refuse() {
        let (_, fingerprint) = brain();
        let (tofu, _) = tofu_client_config();
        for config in [pinned_client_config(&fingerprint), tofu] {
            assert_eq!(offered_versions(config).await, vec![0x0304], "TLS 1.3 seul");
        }
    }

    #[tokio::test]
    async fn tofu_capture_l_empreinte() {
        let (server, fingerprint) = brain();
        let (config, seen) = tofu_client_config();
        assert_eq!(*seen.lock().unwrap(), None);
        handshake(server, config).await.unwrap();
        assert_eq!(*seen.lock().unwrap(), Some(fingerprint));
    }

    #[tokio::test]
    async fn connecteur_tcp_puis_tls() {
        let (server, fingerprint) = brain();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let echo = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let mut tls = tokio_rustls::TlsAcceptor::from(server)
                .accept(tcp)
                .await
                .unwrap();
            let mut buf = [0u8; 4];
            tls.read_exact(&mut buf).await.unwrap();
            tls.write_all(&buf).await.unwrap();
            tls.flush().await.unwrap();
        });
        let connector = TlsConnector::new(pinned_client_config(&fingerprint));
        let mut stream = connector.connect(addr).await.unwrap();
        stream.write_all(b"oui!").await.unwrap();
        stream.flush().await.unwrap();
        let mut buf = [0u8; 4];
        stream.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"oui!");
        echo.await.unwrap();
    }

    #[tokio::test]
    async fn connecteur_refuse_un_autre_certificat() {
        let (server, _) = brain();
        let (_, other) = brain();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let _ = tokio_rustls::TlsAcceptor::from(server).accept(tcp).await;
        });
        let connector = TlsConnector::new(pinned_client_config(&other));
        assert!(connector.connect(addr).await.is_err());
    }
}
