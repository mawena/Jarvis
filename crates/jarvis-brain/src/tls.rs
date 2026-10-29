//! Identité TLS du cerveau : certificat auto-signé (ECDSA P-256), épinglé par les
//! oreilles à l'appairage, et identifiant du cerveau. TLS 1.3 seulement, fournisseur
//! `ring` toujours donné explicitement.
// Squelette (tâche 1) : à retirer par la tâche 2.
#![allow(dead_code, unused_variables)]

use jarvis_protocol::Fingerprint;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Certificat, clé privée et identifiant du cerveau (`brain.crt.der`,
/// `brain.key.der` en 0600, `brain.id`, dans un dossier 0700).
pub struct BrainIdentity {
    pub cert_der: Vec<u8>,
    key_der: Vec<u8>,
    pub fingerprint: Fingerprint,
    /// 16 octets aléatoires en hexadécimal.
    pub brain_id: String,
}

impl BrainIdentity {
    /// Relit l'identité de `dir`, ou la crée.
    pub fn load_or_create(dir: &Path) -> anyhow::Result<Self> {
        todo!("tâche 2")
    }

    /// `~/.local/share/jarvis/tls/`.
    pub fn default_dir() -> PathBuf {
        todo!("tâche 2")
    }

    /// Configuration du serveur : `ring`, TLS 1.3, sans certificat client.
    pub fn server_config(&self) -> anyhow::Result<Arc<rustls::ServerConfig>> {
        todo!("tâche 2")
    }
}
