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
    pair_with(&RealNet { platform: "android" }, data_dir, uri, name)
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
