//! Appairage d'une oreille par l'URI du QR code de `jarvis pair`, partagé par les
//! oreilles (PC, Android) : l'empreinte du certificat est dans l'URI, épinglée dès le
//! premier contact. Les aides ([`pairing_of`], [`refusal`], [`unreachable`],
//! [`resolve`]) servent aussi à l'appairage par code du PC.

use std::io;
use std::net::SocketAddr;
use std::time::Duration;

use anyhow::{Context, anyhow, bail};
use jarvis_protocol::ear::{DEFAULT_PORT, RejectReason};
use jarvis_protocol::pairing_uri::PairingUri;
use jarvis_protocol::{Fingerprint, printable};

use super::discovery::{FoundBrain, MdnsDiscovery};
use super::tls::{TlsConnector, pinned_client_config, tofu_client_config};
use super::{Connector, Discovery, PairError, Paired, Pairing, PairingStore, pair_with_code};

/// Attente des annonces mDNS.
pub const DISCOVERY_WAIT: Duration = Duration::from_secs(3);

/// Le réseau vu de l'appairage (remplaçable dans les tests).
#[async_trait::async_trait]
pub trait PairNet: Send + Sync {
    /// Empreinte du certificat présenté à `addr` (premier contact : rien n'est envoyé).
    async fn fingerprint(&self, addr: SocketAddr) -> io::Result<Fingerprint>;
    /// Connexion épinglée sur `fingerprint`, puis échange `pair` → `paired`.
    async fn pair(
        &self,
        addr: SocketAddr,
        fingerprint: &Fingerprint,
        code: &str,
        name: &str,
    ) -> Result<Paired, PairError>;
    /// Cerveaux dont la fenêtre d'appairage est ouverte.
    async fn find_pairing(&self) -> Vec<FoundBrain>;
    /// Adresses du cerveau `brain_id`.
    async fn find(&self, brain_id: &str) -> Vec<SocketAddr>;
}

/// TLS et mDNS réels.
pub struct RealNet {
    /// Plateforme annoncée au cerveau (`"linux"`, `"android"`…).
    pub platform: &'static str,
}

#[async_trait::async_trait]
impl PairNet for RealNet {
    async fn fingerprint(&self, addr: SocketAddr) -> io::Result<Fingerprint> {
        let (config, seen) = tofu_client_config();
        let stream = TlsConnector::new(config).connect(addr).await?;
        drop(stream);
        let seen = *seen.lock().unwrap_or_else(|e| e.into_inner());
        seen.ok_or_else(|| io::Error::other("aucun certificat présenté"))
    }

    async fn pair(
        &self,
        addr: SocketAddr,
        fingerprint: &Fingerprint,
        code: &str,
        name: &str,
    ) -> Result<Paired, PairError> {
        let stream = TlsConnector::new(pinned_client_config(fingerprint))
            .connect(addr)
            .await
            .map_err(PairError::Io)?;
        pair_with_code(stream, code, name, self.platform).await
    }

    async fn find_pairing(&self) -> Vec<FoundBrain> {
        match MdnsDiscovery::new() {
            Ok(mdns) => mdns.find_pairing(DISCOVERY_WAIT).await,
            Err(e) => {
                tracing::warn!("découverte mDNS impossible : {e:#}");
                Vec::new()
            }
        }
    }

    async fn find(&self, brain_id: &str) -> Vec<SocketAddr> {
        match MdnsDiscovery::new() {
            Ok(mdns) => mdns.find(brain_id, DISCOVERY_WAIT).await,
            Err(e) => {
                tracing::warn!("découverte mDNS impossible : {e:#}");
                Vec::new()
            }
        }
    }
}

/// Appairage par l'URI du QR code (espaces autour retirés) ; enregistré dans `store`.
pub async fn pair_uri(
    net: &dyn PairNet,
    store: &PairingStore,
    uri: &str,
    name: &str,
) -> anyhow::Result<Pairing> {
    let uri = PairingUri::parse(uri.trim()).context("adresse d'appairage illisible")?;
    let mut addrs: Vec<SocketAddr> = uri
        .hosts
        .iter()
        .map(|ip| SocketAddr::new(*ip, uri.port))
        .collect();
    if addrs.is_empty() {
        addrs = net.find(&uri.brain_id).await;
    }
    let mut last = None;
    for addr in addrs {
        match net.pair(addr, &uri.fingerprint, &uri.code, name).await {
            Ok(paired) => {
                if paired.brain_id != uri.brain_id {
                    bail!("le cerveau à {addr} n'est pas celui du QR code");
                }
                let pairing = pairing_of(paired, uri.fingerprint, addr, None);
                store.save(&pairing).context("appairage non enregistré")?;
                return Ok(pairing);
            }
            Err(PairError::Rejected(reason)) => bail!(refusal(&reason)),
            Err(e) => {
                tracing::debug!("{addr} : {e}");
                last = Some(e);
            }
        }
    }
    Err(unreachable(last))
}

/// Appairage à enregistrer, d'après la réponse `paired` du cerveau joint à `addr`.
pub fn pairing_of(
    paired: Paired,
    fingerprint: Fingerprint,
    addr: SocketAddr,
    manual: Option<String>,
) -> Pairing {
    Pairing {
        brain_id: paired.brain_id,
        // Venu du réseau : affiché ensuite au terminal ou à l'écran.
        brain_name: printable(&paired.brain_name),
        ear_id: paired.ear_id,
        token: paired.token,
        fingerprint,
        addresses: vec![addr],
        manual,
    }
}

/// Message clair pour un refus du cerveau.
pub fn refusal(reason: &RejectReason) -> String {
    match reason {
        RejectReason::WrongCode { attempts_left: 0 } => {
            "code faux, plus aucun essai depuis cette machine : relancez `jarvis pair` sur le PC"
                .into()
        }
        RejectReason::WrongCode { attempts_left } => {
            format!("code faux (encore {attempts_left} essai(s))")
        }
        RejectReason::PairingClosed => {
            "aucune fenêtre d'appairage ouverte : relancez `jarvis pair` sur le PC".into()
        }
        RejectReason::Version { supported } => {
            format!("version du protocole incompatible (le cerveau accepte {supported:?})")
        }
        other => format!("appairage refusé ({other:?})"),
    }
}

/// Erreur « cerveau injoignable » (avec la dernière erreur vue) ou « introuvable ».
pub fn unreachable(last: Option<PairError>) -> anyhow::Error {
    match last {
        Some(e) => anyhow!("cerveau injoignable : {e}"),
        None => anyhow!("cerveau introuvable sur le réseau"),
    }
}

/// `hôte[:port]` (port 7341 par défaut).
pub async fn resolve(address: &str) -> anyhow::Result<Vec<SocketAddr>> {
    let address = address.trim();
    let with_port = if address.parse::<SocketAddr>().is_ok() || address.contains(':') {
        address.to_string()
    } else {
        format!("{address}:{DEFAULT_PORT}")
    };
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host(&with_port)
        .await
        .with_context(|| format!("adresse « {address} » introuvable"))?
        .collect();
    anyhow::ensure!(!addrs.is_empty(), "adresse « {address} » introuvable");
    Ok(addrs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use jarvis_protocol::EarId;
    use std::sync::Mutex;

    #[test]
    fn code_faux_dit_ce_qu_il_reste() {
        let left = |n| refusal(&RejectReason::WrongCode { attempts_left: n });
        assert_eq!(left(2), "code faux (encore 2 essai(s))");
        assert!(left(0).contains("plus aucun essai"), "{}", left(0));
        assert!(left(0).contains("jarvis pair"), "{}", left(0));
    }

    /// Faux réseau : `dead` ne répond pas ; ailleurs, le cerveau `b1` présente le
    /// certificat `cert` et accepte le code `482913`.
    struct FakeNet {
        dead: Vec<SocketAddr>,
        pairs: Mutex<Vec<(SocketAddr, Fingerprint, String, String)>>,
        probes: Mutex<Vec<SocketAddr>>,
    }

    fn brain_cert() -> Fingerprint {
        Fingerprint::of_der(b"cert")
    }

    impl FakeNet {
        fn new(dead: Vec<SocketAddr>) -> Self {
            FakeNet {
                dead,
                pairs: Mutex::new(Vec::new()),
                probes: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait::async_trait]
    impl PairNet for FakeNet {
        async fn fingerprint(&self, addr: SocketAddr) -> io::Result<Fingerprint> {
            self.probes.lock().unwrap().push(addr);
            Ok(brain_cert())
        }
        async fn pair(
            &self,
            addr: SocketAddr,
            fingerprint: &Fingerprint,
            code: &str,
            name: &str,
        ) -> Result<Paired, PairError> {
            self.pairs
                .lock()
                .unwrap()
                .push((addr, *fingerprint, code.into(), name.into()));
            if self.dead.contains(&addr) {
                return Err(PairError::Io(io::ErrorKind::ConnectionRefused.into()));
            }
            if *fingerprint != brain_cert() {
                return Err(PairError::Io(io::Error::other("empreinte différente")));
            }
            if code != "482913" {
                return Err(PairError::Rejected(RejectReason::WrongCode {
                    attempts_left: 4,
                }));
            }
            Ok(Paired {
                ear_id: EarId::new("e2"),
                token: "jeton".into(),
                brain_id: "b1".into(),
                brain_name: "PC de Charles".into(),
            })
        }
        async fn find_pairing(&self) -> Vec<FoundBrain> {
            Vec::new()
        }
        async fn find(&self, _: &str) -> Vec<SocketAddr> {
            Vec::new()
        }
    }

    fn uri(hosts: &str) -> String {
        format!(
            "jarvis://pair?v=1&id=b1&h={hosts}&p=7341&fp={}&c=482913",
            brain_cert().to_b64()
        )
    }

    fn store() -> (tempfile::TempDir, PairingStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = PairingStore::new(dir.path().join("ear-essai"));
        (dir, store)
    }

    #[tokio::test]
    async fn uri_d_appairage_enregistree() {
        let dead: SocketAddr = "192.168.1.20:7341".parse().unwrap();
        let net = FakeNet::new(vec![dead]);
        let (_dir, store) = store();
        let pairing = pair_uri(&net, &store, &uri("192.168.1.20,10.0.0.5"), "bureau")
            .await
            .unwrap();
        let live: SocketAddr = "10.0.0.5:7341".parse().unwrap();
        assert_eq!(pairing.fingerprint, brain_cert());
        assert_eq!(pairing.addresses, vec![live]);
        assert_eq!(pairing.token, "jeton");
        assert_eq!(pairing.manual, None);
        assert_eq!(store.load().unwrap(), Some(pairing));
        // Empreinte de l'URI épinglée tout de suite : aucun premier contact à l'aveugle.
        assert!(net.probes.lock().unwrap().is_empty());
        let pairs = net.pairs.lock().unwrap().clone();
        assert_eq!(pairs.len(), 2);
        assert_eq!(
            pairs[1],
            (live, brain_cert(), "482913".into(), "bureau".into())
        );
    }

    #[tokio::test]
    async fn uri_d_un_autre_cerveau_refusee() {
        let net = FakeNet::new(vec![]);
        let (_dir, store) = store();
        let other = uri("10.0.0.5").replace("id=b1", "id=b9");
        assert!(pair_uri(&net, &store, &other, "x").await.is_err());
        assert_eq!(store.load().unwrap(), None);
        assert!(pair_uri(&net, &store, "http://x", "x").await.is_err());
    }

    #[tokio::test]
    async fn uri_entouree_d_espaces_acceptee() {
        let net = FakeNet::new(vec![]);
        let (_dir, store) = store();
        let pasted = format!("  {}\n", uri("10.0.0.5"));
        let pairing = pair_uri(&net, &store, &pasted, "Pixel 4 XL").await.unwrap();
        assert_eq!(store.load().unwrap(), Some(pairing));
    }

    #[tokio::test]
    async fn refus_du_cerveau_rien_enregistre() {
        let net = FakeNet::new(vec![]);
        let (_dir, store) = store();
        let wrong = uri("10.0.0.5").replace("c=482913", "c=000000");
        let err = pair_uri(&net, &store, &wrong, "x").await.unwrap_err();
        assert!(err.to_string().contains("code faux"), "{err}");
        assert_eq!(store.load().unwrap(), None);
    }

    #[tokio::test]
    async fn adresse_sans_port_prend_le_port_par_defaut() {
        let addrs = resolve(" 127.0.0.1 ").await.unwrap();
        assert_eq!(
            addrs,
            vec![SocketAddr::from(([127, 0, 0, 1], DEFAULT_PORT))]
        );
    }
}
