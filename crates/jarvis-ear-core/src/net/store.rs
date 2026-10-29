//! Appairage enregistré sur l'oreille (`pairing.toml`, 0600, dans un dossier 0700
//! fourni par l'appelant) et phrase « le PC est endormi » (`offline-phrase.bin`,
//! son id dans `offline-phrase.id`).

use jarvis_protocol::{EarId, Fingerprint, SpeechAudio, decode_pcm, encode_pcm};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

const PAIRING_FILE: &str = "pairing.toml";
const PHRASE_FILE: &str = "offline-phrase.bin";
const PHRASE_ID_FILE: &str = "offline-phrase.id";
/// Adresses retenues au plus.
const MAX_ADDRESSES: usize = 4;

/// `Debug` masque le jeton.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pairing {
    pub brain_id: String,
    pub brain_name: String,
    pub ear_id: EarId,
    pub token: String,
    /// Empreinte épinglée du certificat du cerveau.
    pub fingerprint: Fingerprint,
    /// Dernières adresses connues, la plus récente d'abord.
    #[serde(default)]
    pub addresses: Vec<SocketAddr>,
    /// Adresse donnée à la main (`hôte:port`), essayée en premier.
    #[serde(default)]
    pub manual: Option<String>,
}

impl fmt::Debug for Pairing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pairing")
            .field("brain_id", &self.brain_id)
            .field("brain_name", &self.brain_name)
            .field("ear_id", &self.ear_id)
            .field("token", &"***")
            .field("fingerprint", &self.fingerprint)
            .field("addresses", &self.addresses)
            .field("manual", &self.manual)
            .finish()
    }
}

#[derive(Debug, Clone)]
pub struct PairingStore {
    dir: PathBuf,
}

impl PairingStore {
    pub fn new(dir: PathBuf) -> Self {
        PairingStore { dir }
    }

    pub fn load(&self) -> io::Result<Option<Pairing>> {
        let text = match fs::read_to_string(self.dir.join(PAIRING_FILE)) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        // Pas le texte dans l'erreur : il contient le jeton.
        toml::from_str(&text)
            .map(Some)
            .map_err(|e| invalid(format!("{PAIRING_FILE} illisible : {}", e.message())))
    }

    /// Écriture atomique, fichier 0600, dossier 0700.
    pub fn save(&self, pairing: &Pairing) -> io::Result<()> {
        let text = toml::to_string(pairing).map_err(|e| invalid(e.to_string()))?;
        write_private(&self.dir, PAIRING_FILE, text.as_bytes())
    }

    /// Retient une adresse qui a répondu.
    pub fn remember_address(&self, addr: SocketAddr) -> io::Result<()> {
        let Some(mut pairing) = self.load()? else {
            return Err(io::Error::new(io::ErrorKind::NotFound, "pas d'appairage"));
        };
        if pairing.addresses.first() == Some(&addr) {
            return Ok(());
        }
        remember(&mut pairing.addresses, addr);
        self.save(&pairing)
    }

    /// Id et voix de la phrase hors ligne, si elle est enregistrée.
    pub fn offline_phrase(&self) -> Option<(String, SpeechAudio)> {
        let id = self.offline_phrase_id()?;
        let bytes = fs::read(self.dir.join(PHRASE_FILE)).ok()?;
        let (rate, pcm) = bytes.split_first_chunk::<4>()?;
        if pcm.len() % 2 != 0 {
            return None;
        }
        let samples: Vec<i16> = pcm
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| i16::from_le_bytes(*c))
            .collect();
        Some((
            id,
            SpeechAudio {
                pcm: encode_pcm(&samples),
                sample_rate: u32::from_le_bytes(*rate),
            },
        ))
    }

    /// Id seul de la phrase hors ligne (pour `hello`).
    pub fn offline_phrase_id(&self) -> Option<String> {
        let id = fs::read_to_string(self.dir.join(PHRASE_ID_FILE)).ok()?;
        let id = id.trim();
        (!id.is_empty()).then(|| id.to_string())
    }

    /// Fréquence (u32 LE) puis échantillons i16 LE.
    pub fn save_offline_phrase(&self, id: &str, audio: &SpeechAudio) -> io::Result<()> {
        let samples = decode_pcm(&audio.pcm).map_err(|e| invalid(e.to_string()))?;
        let mut bytes = Vec::with_capacity(4 + 2 * samples.len());
        bytes.extend_from_slice(&audio.sample_rate.to_le_bytes());
        bytes.extend(samples.iter().flat_map(|s| s.to_le_bytes()));
        write_private(&self.dir, PHRASE_FILE, &bytes)?;
        // L'id en dernier : un son sans id n'est pas lu.
        write_private(&self.dir, PHRASE_ID_FILE, id.trim().as_bytes())
    }
}

/// Place `addr` en tête, sans doublon, [`MAX_ADDRESSES`] au plus.
pub(crate) fn remember(addresses: &mut Vec<SocketAddr>, addr: SocketAddr) {
    addresses.retain(|a| *a != addr);
    addresses.insert(0, addr);
    addresses.truncate(MAX_ADDRESSES);
}

fn invalid(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

/// Écrit `name` dans `dir` (créé 0700) par un fichier temporaire 0600 renommé.
fn write_private(dir: &Path, name: &str, bytes: &[u8]) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    set_mode(dir, 0o700)?;
    let tmp = dir.join(format!(".{name}.tmp"));
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(&tmp)?;
    // Fichier temporaire préexistant : droits remis.
    set_mode(&tmp, 0o600)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&tmp, dir.join(name))
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn set_mode(_: &Path, _: u32) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn pairing() -> Pairing {
        Pairing {
            brain_id: "b1".into(),
            brain_name: "PC de Charles".into(),
            ear_id: EarId::new("00112233445566778899aabbccddeeff"),
            token: "jeton-tres-secret".into(),
            fingerprint: Fingerprint::of_der(b"cert"),
            addresses: vec![],
            manual: None,
        }
    }

    fn mode(path: &Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn fichier_d_appairage_prive() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("ear");
        let store = PairingStore::new(dir.clone());
        assert_eq!(store.load().unwrap(), None);
        let mut p = pairing();
        store.save(&p).unwrap();
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(&dir.join(PAIRING_FILE)), 0o600);
        assert_eq!(store.load().unwrap(), Some(p.clone()));
        // Pas de fichier temporaire laissé.
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        // Adresses : la plus récente d'abord, sans doublon.
        let a: SocketAddr = "192.168.1.20:7341".parse().unwrap();
        let b: SocketAddr = "10.0.0.5:7341".parse().unwrap();
        store.remember_address(a).unwrap();
        store.remember_address(b).unwrap();
        store.remember_address(a).unwrap();
        assert_eq!(store.load().unwrap().unwrap().addresses, vec![a, b]);
        p.manual = Some("pc.local:7341".into());
        p.addresses = vec![b];
        store.save(&p).unwrap();
        assert_eq!(store.load().unwrap(), Some(p.clone()));
        assert_eq!(mode(&dir.join(PAIRING_FILE)), 0o600);
        // Champs facultatifs absents : valeurs par défaut.
        let text = fs::read_to_string(dir.join(PAIRING_FILE)).unwrap();
        let minimal: String = text
            .lines()
            .filter(|l| !l.starts_with("addresses") && !l.starts_with("manual"))
            .map(|l| format!("{l}\n"))
            .collect();
        fs::write(dir.join(PAIRING_FILE), minimal).unwrap();
        let read = store.load().unwrap().unwrap();
        assert!(
            read.addresses.is_empty() && read.manual.is_none(),
            "{read:?}"
        );
        // Fichier abîmé : erreur sans le jeton.
        fs::write(dir.join(PAIRING_FILE), "token = \"jeton-tres-secret\"\n[").unwrap();
        let err = store.load().unwrap_err().to_string();
        assert!(!err.contains("jeton-tres-secret"), "{err}");
        // `Debug` masque le jeton.
        let debug = format!("{p:?}");
        assert!(!debug.contains("jeton-tres-secret") && debug.contains("***"));
    }

    #[test]
    fn phrase_hors_ligne_stockee() {
        let tmp = tempfile::tempdir().unwrap();
        let store = PairingStore::new(tmp.path().join("ear"));
        assert_eq!(store.offline_phrase(), None);
        let audio = SpeechAudio {
            pcm: encode_pcm(&[1, -1, i16::MAX, i16::MIN]),
            sample_rate: 22_050,
        };
        store.save_offline_phrase("a1b2", &audio).unwrap();
        assert_eq!(store.offline_phrase(), Some(("a1b2".into(), audio)));
        assert_eq!(store.offline_phrase_id().as_deref(), Some("a1b2"));
        let bin = fs::read(tmp.path().join("ear").join(PHRASE_FILE)).unwrap();
        assert_eq!(bin[..4], 22_050u32.to_le_bytes());
        assert_eq!(bin.len(), 4 + 8);
        assert_eq!(mode(&tmp.path().join("ear").join(PHRASE_FILE)), 0o600);
    }
}
