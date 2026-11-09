//! Identité TLS du cerveau : certificat auto-signé (ECDSA P-256), épinglé par les
//! oreilles à l'appairage, et identifiant du cerveau. TLS 1.3 seulement, fournisseur
//! `ring` toujours donné explicitement.

use crate::ears::{hex, random_bytes, write_private};
use anyhow::{Context, bail};
use jarvis_protocol::Fingerprint;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const CERT_FILE: &str = "brain.crt.der";
const KEY_FILE: &str = "brain.key.der";
const ID_FILE: &str = "brain.id";

/// Certificat, clé privée et identifiant du cerveau (`brain.crt.der`,
/// `brain.key.der` en 0600, `brain.id`, dans un dossier 0700).
pub struct BrainIdentity {
    pub cert_der: Vec<u8>,
    key_der: Vec<u8>,
    pub fingerprint: Fingerprint,
    /// 16 octets aléatoires en hexadécimal.
    pub brain_id: String,
}

impl std::fmt::Debug for BrainIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // La clé privée n'apparaît jamais.
        f.debug_struct("BrainIdentity")
            .field("fingerprint", &self.fingerprint)
            .field("brain_id", &self.brain_id)
            .finish_non_exhaustive()
    }
}

impl BrainIdentity {
    /// Relit l'identité de `dir`, ou la crée.
    ///
    /// Certificat présent sans clé : erreur (le recréer obligerait à tout réappairer).
    /// Clé seule (arrêt pendant la création) : tout est recréé.
    pub fn load_or_create(dir: &Path) -> anyhow::Result<Self> {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .with_context(|| format!("création de {}", dir.display()))?;
        set_mode(dir, 0o700)?;

        let (cert_path, key_path) = (dir.join(CERT_FILE), dir.join(KEY_FILE));
        let (cert_der, key_der) = if cert_path.exists() {
            if !key_path.exists() {
                bail!(
                    "clé privée absente ({}) : supprimez {} pour recréer le certificat \
                     (toutes les oreilles seront à réappairer)",
                    key_path.display(),
                    dir.display()
                );
            }
            set_mode(&key_path, 0o600)?;
            let cert = std::fs::read(&cert_path)
                .with_context(|| format!("lecture de {}", cert_path.display()))?;
            let key = std::fs::read(&key_path)
                .with_context(|| format!("lecture de {}", key_path.display()))?;
            (cert, key)
        } else {
            let (cert, key) = generate()?;
            // La clé d'abord : un arrêt entre les deux laisse « clé seule », recréé.
            write_private(&key_path, &key)
                .with_context(|| format!("écriture de {}", key_path.display()))?;
            write_private(&cert_path, &cert)
                .with_context(|| format!("écriture de {}", cert_path.display()))?;
            (cert, key)
        };

        let identity = BrainIdentity {
            fingerprint: Fingerprint::of_der(&cert_der),
            cert_der,
            key_der,
            brain_id: load_or_create_id(&dir.join(ID_FILE))?,
        };
        // Certificat et clé illisibles ou dépareillés : erreur dès maintenant.
        identity.server_config()?;
        Ok(identity)
    }

    /// `~/.local/share/jarvis/tls/`.
    pub fn default_dir() -> PathBuf {
        match directories::ProjectDirs::from("", "", "jarvis") {
            Some(dirs) => dirs.data_dir().join("tls"),
            None => PathBuf::from(".local/share/jarvis/tls"),
        }
    }

    /// Configuration du serveur : `ring`, TLS 1.3, sans certificat client.
    pub fn server_config(&self) -> anyhow::Result<Arc<rustls::ServerConfig>> {
        let config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .context("TLS 1.3 indisponible")?
        .with_no_client_auth()
        .with_single_cert(self.cert_chain(), self.private_key())
        .context("certificat ou clé du cerveau invalide")?;
        Ok(Arc::new(config))
    }

    fn cert_chain(&self) -> Vec<CertificateDer<'static>> {
        vec![CertificateDer::from(self.cert_der.clone())]
    }

    fn private_key(&self) -> PrivateKeyDer<'static> {
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(self.key_der.clone()))
    }
}

/// Certificat auto-signé ECDSA P-256 (DER) et sa clé (PKCS#8 DER).
fn generate() -> anyhow::Result<(Vec<u8>, Vec<u8>)> {
    let key = rcgen::KeyPair::generate().context("génération de la clé")?;
    let mut params =
        rcgen::CertificateParams::new(vec!["jarvis-brain".to_string(), "localhost".to_string()])
            .context("paramètres du certificat")?;
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "Jarvis");
    params.not_before = rcgen::date_time_ymd(2025, 1, 1);
    // Épinglé, jamais renouvelé : validité lointaine.
    params.not_after = rcgen::date_time_ymd(2126, 1, 1);
    let cert = params
        .self_signed(&key)
        .context("signature du certificat")?;
    Ok((cert.der().to_vec(), key.serialize_der()))
}

/// Identifiant du cerveau : relu, ou 16 octets aléatoires écrits en hexadécimal.
fn load_or_create_id(path: &Path) -> anyhow::Result<String> {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            let id = text.trim();
            if id.len() != 32 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
                bail!("identifiant du cerveau invalide dans {}", path.display());
            }
            Ok(id.to_string())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let id = hex(&random_bytes::<16>()?);
            write_private(path, format!("{id}\n").as_bytes())
                .with_context(|| format!("écriture de {}", path.display()))?;
            Ok(id)
        }
        Err(e) => Err(e).with_context(|| format!("lecture de {}", path.display())),
    }
}

fn set_mode(path: &Path, mode: u32) -> anyhow::Result<()> {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .with_context(|| format!("permissions de {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustls::pki_types::ServerName;
    use std::os::unix::fs::PermissionsExt;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn mode(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn certificat_cree_puis_relu_meme_empreinte() {
        let dir = tempfile::tempdir().unwrap();
        let tls = dir.path().join("jarvis/tls");
        let first = BrainIdentity::load_or_create(&tls).unwrap();
        assert_eq!(first.fingerprint, Fingerprint::of_der(&first.cert_der));
        assert_eq!(first.brain_id.len(), 32);
        assert!(first.brain_id.bytes().all(|b| b.is_ascii_hexdigit()));
        let again = BrainIdentity::load_or_create(&tls).unwrap();
        assert_eq!(again.fingerprint, first.fingerprint);
        assert_eq!(again.cert_der, first.cert_der);
        assert_eq!(again.brain_id, first.brain_id);
        // Un autre dossier : une autre identité.
        let other = BrainIdentity::load_or_create(&dir.path().join("autre")).unwrap();
        assert_ne!(other.fingerprint, first.fingerprint);
        assert_ne!(other.brain_id, first.brain_id);
    }

    #[test]
    fn fichiers_tls_prives() {
        let dir = tempfile::tempdir().unwrap();
        let tls = dir.path().join("tls");
        let identity = BrainIdentity::load_or_create(&tls).unwrap();
        assert_eq!(mode(&tls), 0o700);
        assert_eq!(mode(&tls.join("brain.key.der")), 0o600);
        assert!(tls.join("brain.crt.der").exists());
        assert!(tls.join("brain.id").exists());
        // Droits élargis à la main : resserrés à la relecture.
        std::fs::set_permissions(
            tls.join("brain.key.der"),
            std::fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        std::fs::set_permissions(&tls, std::fs::Permissions::from_mode(0o755)).unwrap();
        BrainIdentity::load_or_create(&tls).unwrap();
        assert_eq!(mode(&tls), 0o700);
        assert_eq!(mode(&tls.join("brain.key.der")), 0o600);
        // Ni la clé ni son contenu dans `Debug`.
        let debug = format!("{identity:?}");
        assert!(debug.contains(&identity.fingerprint.short()));
        assert!(!debug.contains("key"));
    }

    #[test]
    fn cle_perdue_erreur_sans_remplacer_le_certificat() {
        let dir = tempfile::tempdir().unwrap();
        let first = BrainIdentity::load_or_create(dir.path()).unwrap();
        std::fs::remove_file(dir.path().join("brain.key.der")).unwrap();
        assert!(BrainIdentity::load_or_create(dir.path()).is_err());
        // Le certificat épinglé par les oreilles n'a pas été écrasé.
        assert_eq!(
            std::fs::read(dir.path().join("brain.crt.der")).unwrap(),
            first.cert_der
        );
    }

    #[test]
    fn cle_seule_regeneree() {
        // Arrêt entre l'écriture de la clé et celle du certificat.
        let dir = tempfile::tempdir().unwrap();
        BrainIdentity::load_or_create(dir.path()).unwrap();
        std::fs::remove_file(dir.path().join("brain.crt.der")).unwrap();
        let identity = BrainIdentity::load_or_create(dir.path()).unwrap();
        identity.server_config().unwrap();
    }

    #[tokio::test]
    async fn poignee_tls_avec_l_oreille() {
        let dir = tempfile::tempdir().unwrap();
        let identity = BrainIdentity::load_or_create(dir.path()).unwrap();
        let client = jarvis_ear_core::net::tls::pinned_client_config(&identity.fingerprint);
        let (a, b) = tokio::io::duplex(64 * 1024);
        let server = tokio_rustls::TlsAcceptor::from(identity.server_config().unwrap());
        let accept = async move {
            let mut tls = server.accept(b).await.unwrap();
            tls.write_all(b"ok").await.unwrap();
            tls.flush().await.unwrap();
        };
        let connect = async move {
            let name = ServerName::try_from("jarvis-brain").unwrap();
            let mut tls = tokio_rustls::TlsConnector::from(client)
                .connect(name, a)
                .await
                .unwrap();
            let mut buf = [0u8; 2];
            tls.read_exact(&mut buf).await.unwrap();
            assert_eq!(&buf, b"ok");
        };
        tokio::join!(accept, connect);
    }

    #[tokio::test]
    async fn serveur_tls12_refuse_par_l_oreille() {
        let dir = tempfile::tempdir().unwrap();
        let identity = BrainIdentity::load_or_create(dir.path()).unwrap();
        // Même certificat, mais TLS 1.2 seulement : l'oreille doit refuser.
        let server = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_protocol_versions(&[&rustls::version::TLS12])
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(identity.cert_chain(), identity.private_key())
        .unwrap();
        let client = jarvis_ear_core::net::tls::pinned_client_config(&identity.fingerprint);
        let (a, b) = tokio::io::duplex(64 * 1024);
        let accept = async move {
            let _ = tokio_rustls::TlsAcceptor::from(Arc::new(server))
                .accept(b)
                .await;
        };
        let connect = async move {
            let name = ServerName::try_from("jarvis-brain").unwrap();
            tokio_rustls::TlsConnector::from(client)
                .connect(name, a)
                .await
        };
        let ((), result) = tokio::join!(accept, connect);
        assert!(result.is_err());
    }
}
