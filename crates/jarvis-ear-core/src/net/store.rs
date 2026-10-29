//! Appairage enregistré sur l'oreille (`pairing.toml`, 0600, dans un dossier 0700
//! fourni par l'appelant) et phrase « le PC est endormi » (`offline-phrase.bin`).
// Squelette (tâche 1) : à retirer par la tâche 4.
#![allow(dead_code, unused_variables)]

use jarvis_protocol::{EarId, Fingerprint, SpeechAudio};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;

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
        todo!("tâche 4")
    }

    /// Écriture atomique, fichier 0600, dossier 0700.
    pub fn save(&self, pairing: &Pairing) -> io::Result<()> {
        todo!("tâche 4")
    }

    /// Retient une adresse qui a répondu.
    pub fn remember_address(&self, addr: SocketAddr) -> io::Result<()> {
        todo!("tâche 4")
    }

    /// Id et voix de la phrase hors ligne, si elle est enregistrée.
    pub fn offline_phrase(&self) -> Option<(String, SpeechAudio)> {
        todo!("tâche 4")
    }

    /// Fréquence (u32 LE) puis échantillons i16 LE.
    pub fn save_offline_phrase(&self, id: &str, audio: &SpeechAudio) -> io::Result<()> {
        todo!("tâche 4")
    }
}
