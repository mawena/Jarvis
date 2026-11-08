//! Appairage par l'adresse du QR code, sur le téléphone.

use std::path::{Path, PathBuf};

use jarvis_ear_core::net::PairingStore;
use jarvis_ear_core::net::pair::{PairNet, RealNet, pair_uri};

use crate::EarError;

/// Dossier de l'appairage (jeton, empreinte, phrase « le PC est endormi »).
pub fn pairing_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("pairing")
}

pub fn store_of(data_dir: &Path) -> PairingStore {
    PairingStore::new(pairing_dir(data_dir))
}

/// Nom du cerveau appairé ; un appairage illisible compte pour une absence.
pub fn paired_brain(data_dir: &Path) -> Option<String> {
    match store_of(data_dir).load() {
        Ok(pairing) => pairing.map(|p| p.brain_name),
        Err(e) => {
            tracing::warn!("appairage illisible : {e}");
            None
        }
    }
}

/// Traduit l'erreur de `pair_uri` (messages français de `jarvis-ear-core`) en erreur
/// pour Kotlin ; le détail technique va au journal, pas à l'écran.
pub fn classify(error: &anyhow::Error) -> EarError {
    let text = error.to_string();
    tracing::warn!("appairage échoué : {error:#}");
    if text.starts_with("adresse d'appairage illisible") {
        EarError::InvalidUri {
            message: "ce n'est pas une adresse d'appairage de Jarvis".into(),
        }
    } else if text.starts_with("cerveau injoignable") || text.starts_with("cerveau introuvable") {
        EarError::Unreachable {
            message: "PC injoignable : même Wi-Fi que le PC ? `jarvis pair` est-il ouvert ?".into(),
        }
    } else if text.starts_with("appairage non enregistré") {
        EarError::Io { message: text }
    } else {
        EarError::Refused { message: text }
    }
}

/// Appairage avec un réseau donné (les tests passent un faux). Bloquant.
pub fn pair_with(
    net: &dyn PairNet,
    data_dir: &Path,
    uri: &str,
    name: &str,
) -> Result<String, EarError> {
    // Une adresse collée peut porter espaces et retours à la ligne, même au milieu.
    let uri: String = uri.split_whitespace().collect();
    let name = name.trim();
    let name = if name.is_empty() { "Android" } else { name };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| EarError::Internal {
            message: e.to_string(),
        })?;
    let store = store_of(data_dir);
    runtime
        .block_on(pair_uri(net, &store, &uri, name))
        .map(|pairing| pairing.brain_name)
        .map_err(|e| classify(&e))
}

pub fn pair_real(data_dir: &Path, uri: &str, name: &str) -> Result<String, EarError> {
    pair_with(
        &RealNet {
            platform: "android",
        },
        data_dir,
        uri,
        name,
    )
}

/// Supprime l'appairage et la phrase hors ligne (pas le mot d'éveil).
pub fn unpair(data_dir: &Path) -> Result<(), EarError> {
    match std::fs::remove_dir_all(pairing_dir(data_dir)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(EarError::Io {
            message: format!("appairage non supprimé : {e}"),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jarvis_ear_core::net::discovery::FoundBrain;
    use jarvis_ear_core::net::{PairError, Paired};
    use jarvis_protocol::pairing_uri::PairingUri;
    use jarvis_protocol::{EarId, Fingerprint};
    use std::io;
    use std::net::SocketAddr;

    fn cert() -> Fingerprint {
        Fingerprint::of_der(b"cert")
    }

    fn uri() -> String {
        format!(
            "jarvis://pair?v=1&id=b1&h=192.168.1.20&p=7341&fp={}&c=482913",
            cert().to_b64()
        )
    }

    /// Faux cerveau `b1` qui accepte le code 482913.
    struct FakeNet;

    #[async_trait::async_trait]
    impl PairNet for FakeNet {
        async fn fingerprint(&self, _: SocketAddr) -> io::Result<Fingerprint> {
            Ok(cert())
        }
        async fn pair(
            &self,
            _: SocketAddr,
            _: &Fingerprint,
            code: &str,
            _: &str,
        ) -> Result<Paired, PairError> {
            assert_eq!(code, "482913");
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

    #[test]
    fn uri_avec_espaces_acceptee() {
        let dir = tempfile::tempdir().unwrap();
        let (head, tail) = uri().split_at(20);
        let pasted = format!("  \n{head}\r\n{tail} \n");
        let name = pair_with(&FakeNet, dir.path(), &pasted, "Pixel").unwrap();
        assert_eq!(name, "PC de Charles");
        assert_eq!(paired_brain(dir.path()).as_deref(), Some("PC de Charles"));
    }

    #[test]
    fn uri_invalide_rien_enregistre() {
        let dir = tempfile::tempdir().unwrap();
        for bad in ["https://exemple.fr/", "", "jarvis://pair?v=1", "bonjour"] {
            let err = pair_with(&FakeNet, dir.path(), bad, "Pixel").unwrap_err();
            assert!(
                matches!(&err, EarError::InvalidUri { .. }),
                "{bad} : {err:?}"
            );
            assert_eq!(
                err.to_string(),
                "ce n'est pas une adresse d'appairage de Jarvis"
            );
        }
        assert_eq!(paired_brain(dir.path()), None);
        assert!(!pairing_dir(dir.path()).exists(), "rien d'écrit");
    }

    #[test]
    fn cerveau_injoignable_message_clair() {
        // Un port fermé : on réserve un port libre, puis on le lâche.
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let uri = PairingUri {
            brain_id: "b1".into(),
            hosts: vec!["127.0.0.1".parse().unwrap()],
            port,
            fingerprint: cert(),
            code: "482913".into(),
        }
        .to_string();
        let dir = tempfile::tempdir().unwrap();
        let err = pair_real(dir.path(), &uri, "Pixel").unwrap_err();
        assert!(matches!(&err, EarError::Unreachable { .. }), "{err:?}");
        assert!(err.to_string().contains("PC injoignable"), "{err}");
        assert!(err.to_string().contains("Wi-Fi"), "{err}");
        assert_eq!(paired_brain(dir.path()), None);
    }

    #[test]
    fn desappairage_garde_le_mot_d_eveil() {
        let dir = tempfile::tempdir().unwrap();
        pair_with(&FakeNet, dir.path(), &uri(), "Pixel").unwrap();
        let model = crate::enroll::model_path(dir.path());
        std::fs::create_dir_all(model.parent().unwrap()).unwrap();
        std::fs::write(&model, b"modele").unwrap();
        unpair(dir.path()).unwrap();
        assert_eq!(paired_brain(dir.path()), None);
        assert!(model.exists());
        // Deux fois : sans erreur.
        unpair(dir.path()).unwrap();
    }
}
