//! `jarvis-ear pair` : appairage d'une oreille au cerveau.
//!
//! - `pair <uri>` (QR code de `jarvis pair`) : l'empreinte du certificat est dans
//!   l'URI, épinglée dès le premier contact (code commun : `jarvis_ear_core::net::pair`).
//! - `pair --code 123456 [--address hôte:port]` : le cerveau en appairage est trouvé
//!   par mDNS (ou à l'adresse donnée) ; un premier contact montre l'empreinte de son
//!   certificat, à comparer à l'œil avec celle du PC. Le code n'est envoyé qu'après
//!   « o », sur une connexion épinglée sur cette empreinte.

use std::io::{self, BufRead, Write};

use anyhow::{Context, anyhow, bail};
use jarvis_ear_core::net::pair::{
    PairNet, RealNet, pair_uri, pairing_of, refusal, resolve, unreachable,
};
use jarvis_ear_core::net::{PairError, Pairing, PairingStore};
use jarvis_protocol::printable;

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
            println!("Cerveau « {} » trouvé.", printable(&brain.name));
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
    let net = RealNet { platform: "linux" };
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
        printable(&pairing.brain_name),
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
    use jarvis_ear_core::net::Paired;
    use jarvis_ear_core::net::discovery::FoundBrain;
    use jarvis_protocol::ear::RejectReason;
    use jarvis_protocol::{EarId, Fingerprint};
    use std::net::SocketAddr;
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

    fn store() -> (tempfile::TempDir, PairingStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = PairingStore::new(dir.path().join("ear-essai"));
        (dir, store)
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
