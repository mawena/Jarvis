//! `jarvis-ear pair` : appairage d'une oreille au cerveau.
//!
//! - `pair <uri>` (QR code de `jarvis pair`) : l'empreinte du certificat est dans
//!   l'URI, épinglée dès le premier contact.
//! - `pair --code 123456 [--address hôte:port]` : le cerveau en appairage est trouvé
//!   par mDNS (ou à l'adresse donnée) ; un premier contact montre l'empreinte de son
//!   certificat, à comparer à l'œil avec celle du PC. Le code n'est envoyé qu'après
//!   « o », sur une connexion épinglée sur cette empreinte.

use std::io::{self, BufRead, Write};
use std::net::SocketAddr;
use std::time::Duration;

use anyhow::{Context, anyhow, bail};
use jarvis_ear_core::net::discovery::{FoundBrain, MdnsDiscovery};
use jarvis_ear_core::net::tls::{TlsConnector, pinned_client_config, tofu_client_config};
use jarvis_ear_core::net::{
    Connector, Discovery, PairError, Paired, Pairing, PairingStore, pair_with_code,
};
use jarvis_protocol::Fingerprint;
use jarvis_protocol::ear::{DEFAULT_PORT, RejectReason};
use jarvis_protocol::pairing_uri::PairingUri;

/// Plateforme annoncée au cerveau.
pub const PLATFORM: &str = "linux";

/// Attente des annonces mDNS.
const DISCOVERY_WAIT: Duration = Duration::from_secs(3);

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
pub struct RealNet;

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
        pair_with_code(stream, code, name, PLATFORM).await
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

/// Appairage par l'URI du QR code ; enregistré dans `store`.
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

/// Appairage par le code seul ; `confirm` pose la question de l'empreinte et rend la
/// réponse. Enregistré dans `store`.
pub async fn pair_code(
    net: &dyn PairNet,
    store: &PairingStore,
    code: &str,
    address: Option<&str>,
    name: &str,
    confirm: &mut (dyn FnMut(&str) -> bool + Send),
) -> anyhow::Result<Pairing> {
    let code = code.trim();
    if code.len() != 6 || !code.bytes().all(|b| b.is_ascii_digit()) {
        bail!("le code d'appairage a 6 chiffres");
    }
    let addrs = match address {
        Some(address) => resolve(address).await?,
        None => {
            let brains = net.find_pairing().await;
            let Some(brain) = brains.into_iter().next() else {
                bail!(
                    "aucun cerveau en appairage trouvé sur le réseau : lancez `jarvis pair` \
                     sur le PC, ou donnez son adresse (--address hôte:port)"
                );
            };
            println!("Cerveau « {} » trouvé.", brain.name);
            brain.addrs
        }
    };
    let mut last = None;
    for addr in addrs {
        let fingerprint = match net.fingerprint(addr).await {
            Ok(fingerprint) => fingerprint,
            Err(e) => {
                tracing::debug!("{addr} : {e}");
                last = Some(PairError::Io(e));
                continue;
            }
        };
        let question = format!(
            "Empreinte du cerveau : {}\nEst-ce la même que sur le PC ? [o/N] ",
            fingerprint.short()
        );
        if !confirm(&question) {
            bail!("appairage annulé : empreinte non confirmée (rien n'a été envoyé)");
        }
        return match net.pair(addr, &fingerprint, code, name).await {
            Ok(paired) => {
                let pairing =
                    pairing_of(paired, fingerprint, addr, address.map(|a| a.trim().into()));
                store.save(&pairing).context("appairage non enregistré")?;
                Ok(pairing)
            }
            Err(PairError::Rejected(reason)) => Err(anyhow!(refusal(&reason))),
            Err(e) => Err(anyhow!("appairage impossible : {e}")),
        };
    }
    Err(unreachable(last))
}

fn pairing_of(
    paired: Paired,
    fingerprint: Fingerprint,
    addr: SocketAddr,
    manual: Option<String>,
) -> Pairing {
    Pairing {
        brain_id: paired.brain_id,
        brain_name: paired.brain_name,
        ear_id: paired.ear_id,
        token: paired.token,
        fingerprint,
        addresses: vec![addr],
        manual,
    }
}

fn refusal(reason: &RejectReason) -> String {
    match reason {
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

fn unreachable(last: Option<PairError>) -> anyhow::Error {
    match last {
        Some(e) => anyhow!("cerveau injoignable : {e}"),
        None => anyhow!("cerveau introuvable sur le réseau"),
    }
}

/// `hôte[:port]` (port 7341 par défaut).
async fn resolve(address: &str) -> anyhow::Result<Vec<SocketAddr>> {
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

/// Question posée au terminal ; seuls « o » et « oui » valent oui.
fn ask(question: &str) -> bool {
    print!("{question}");
    let _ = io::stdout().flush();
    let mut line = String::new();
    if io::stdin().lock().read_line(&mut line).is_err() {
        return false;
    }
    matches!(line.trim().to_lowercase().as_str(), "o" | "oui")
}

/// Point d'entrée de `jarvis-ear pair`.
pub fn run(
    store: PairingStore,
    profile: Option<&str>,
    uri: Option<String>,
    code: Option<String>,
    address: Option<String>,
    name: String,
) -> anyhow::Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    if store.load().ok().flatten().is_some() {
        println!("Cette oreille est déjà appairée : l'ancien appairage sera remplacé.");
    }
    let net = RealNet;
    let pairing = runtime.block_on(async {
        match (uri, code) {
            (Some(uri), _) => pair_uri(&net, &store, &uri, &name).await,
            (None, Some(code)) => {
                pair_code(&net, &store, &code, address.as_deref(), &name, &mut ask).await
            }
            (None, None) => Err(anyhow!("donnez l'adresse d'appairage ou --code")),
        }
    })?;
    println!(
        "Oreille « {name} » appairée au cerveau « {} » (empreinte {}).",
        pairing.brain_name,
        pairing.fingerprint.short()
    );
    match profile {
        Some(profile) => println!("Lancez-la : jarvis-ear --profile {profile}"),
        None => println!("Relancez-la : systemctl --user restart jarvis-ear"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use jarvis_protocol::EarId;
    use std::sync::Mutex;

    /// Faux réseau : `dead` ne répond pas ; ailleurs, le cerveau `b1` présente le
    /// certificat `cert` et accepte le code `482913`.
    struct FakeNet {
        dead: Vec<SocketAddr>,
        found: Vec<FoundBrain>,
        probes: Mutex<Vec<SocketAddr>>,
        pairs: Mutex<Vec<(SocketAddr, Fingerprint, String, String)>>,
    }

    fn brain_cert() -> Fingerprint {
        Fingerprint::of_der(b"cert")
    }

    impl FakeNet {
        fn new(dead: Vec<SocketAddr>, found: Vec<FoundBrain>) -> Self {
            FakeNet {
                dead,
                found,
                probes: Mutex::new(Vec::new()),
                pairs: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait::async_trait]
    impl PairNet for FakeNet {
        async fn fingerprint(&self, addr: SocketAddr) -> io::Result<Fingerprint> {
            self.probes.lock().unwrap().push(addr);
            if self.dead.contains(&addr) {
                return Err(io::ErrorKind::ConnectionRefused.into());
            }
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
            self.found.clone()
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
        let net = FakeNet::new(vec![dead], vec![]);
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
        let net = FakeNet::new(vec![], vec![]);
        let (_dir, store) = store();
        let other = uri("10.0.0.5").replace("id=b1", "id=b9");
        assert!(pair_uri(&net, &store, &other, "x").await.is_err());
        assert_eq!(store.load().unwrap(), None);
        assert!(pair_uri(&net, &store, "http://x", "x").await.is_err());
    }

    fn found(addr: SocketAddr) -> FoundBrain {
        FoundBrain {
            brain_id: "b1".into(),
            name: "PC de Charles".into(),
            addrs: vec![addr],
        }
    }

    #[tokio::test]
    async fn code_appairage_demande_confirmation_de_l_empreinte() {
        let addr: SocketAddr = "10.0.0.5:7341".parse().unwrap();
        let net = FakeNet::new(vec![], vec![found(addr)]);
        let (_dir, store) = store();
        // « N » : rien n'est envoyé, rien n'est enregistré.
        let mut questions = Vec::new();
        let err = pair_code(&net, &store, "482913", None, "bureau", &mut |q: &str| {
            questions.push(q.to_string());
            false
        })
        .await
        .unwrap_err();
        assert!(err.to_string().contains("annulé"), "{err}");
        assert_eq!(questions.len(), 1);
        assert!(
            questions[0].contains(&brain_cert().short()),
            "{}",
            questions[0]
        );
        assert!(net.pairs.lock().unwrap().is_empty(), "code jamais envoyé");
        assert_eq!(store.load().unwrap(), None);
        // « o » : le code part, sur une connexion épinglée sur l'empreinte vue.
        let pairing = pair_code(&net, &store, "482913", None, "bureau", &mut |_: &str| true)
            .await
            .unwrap();
        assert_eq!(
            net.pairs.lock().unwrap().clone(),
            vec![(addr, brain_cert(), "482913".into(), "bureau".into())]
        );
        assert_eq!(pairing.fingerprint, brain_cert());
        assert_eq!(pairing.addresses, vec![addr]);
        assert_eq!(store.load().unwrap(), Some(pairing));
    }

    #[tokio::test]
    async fn code_avec_adresse_manuelle_retenue() {
        let net = FakeNet::new(vec![], vec![]);
        let (_dir, store) = store();
        let pairing = pair_code(
            &net,
            &store,
            "482913",
            Some("127.0.0.1"),
            "bureau",
            &mut |_: &str| true,
        )
        .await
        .unwrap();
        assert_eq!(pairing.addresses, vec!["127.0.0.1:7341".parse().unwrap()]);
        assert_eq!(pairing.manual.as_deref(), Some("127.0.0.1"));
    }

    #[tokio::test]
    async fn code_faux_ou_absent_refuse() {
        let addr: SocketAddr = "10.0.0.5:7341".parse().unwrap();
        let net = FakeNet::new(vec![], vec![found(addr)]);
        let (_dir, store) = store();
        let err = pair_code(&net, &store, "000000", None, "x", &mut |_: &str| true)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("code faux"), "{err}");
        assert!(
            pair_code(&net, &store, "12ab", None, "x", &mut |_: &str| true)
                .await
                .is_err()
        );
        let nobody = FakeNet::new(vec![], vec![]);
        let err = pair_code(&nobody, &store, "482913", None, "x", &mut |_: &str| true)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("jarvis pair"), "{err}");
        assert_eq!(store.load().unwrap(), None);
    }
}
