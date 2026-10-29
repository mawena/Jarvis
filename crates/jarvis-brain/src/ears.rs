//! Registre des oreilles appairées (`~/.local/share/jarvis/ears.toml`, 0600,
//! écriture atomique). Seul le SHA-256 du jeton est stocké, comparé en temps constant.
// Squelette (tâche 1) : à retirer par la tâche 2.
#![allow(dead_code, unused_variables)]

use jarvis_protocol::EarId;
use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EarRecord {
    pub id: EarId,
    pub name: String,
    pub platform: String,
    /// SHA-256 du jeton, en hexadécimal. Jamais le jeton lui-même.
    pub token_sha256: String,
    /// Oreille du PC (appairée par le socket Unix).
    pub local: bool,
    /// Baisse du son des lecteurs du PC à son réveil.
    #[serde(default = "default_duck")]
    pub duck: bool,
    /// Ajouté au rang de l'oreille par l'arbitre.
    #[serde(default)]
    pub bias: f32,
    /// Secondes depuis l'epoch Unix.
    pub paired_at: u64,
}

fn default_duck() -> bool {
    true
}

pub struct EarRegistry {
    path: PathBuf,
    ears: Vec<EarRecord>,
}

impl EarRegistry {
    /// Fichier absent : registre vide.
    pub fn load(path: &Path) -> io::Result<Self> {
        todo!("tâche 2")
    }

    /// `~/.local/share/jarvis/ears.toml`.
    pub fn default_path() -> PathBuf {
        todo!("tâche 2")
    }

    /// Oreille connue et jeton correct (comparaison en temps constant).
    pub fn authenticate(&self, id: &EarId, token: &str) -> Option<EarRecord> {
        todo!("tâche 2")
    }

    /// Nouvelle oreille ; le jeton en clair n'est rendu qu'ici, une seule fois.
    pub fn add(
        &mut self,
        name: &str,
        platform: &str,
        local: bool,
    ) -> io::Result<(EarRecord, String)> {
        todo!("tâche 2")
    }

    /// Oreille locale : remplace celle du même nom (nouveau jeton).
    pub fn add_local(&mut self, name: &str) -> io::Result<(EarRecord, String)> {
        todo!("tâche 2")
    }

    /// Retire une oreille par son nom ou son identifiant.
    pub fn remove(&mut self, name_or_id: &str) -> io::Result<Option<EarRecord>> {
        todo!("tâche 2")
    }

    pub fn list(&self) -> Vec<EarRecord> {
        todo!("tâche 2")
    }

    pub fn get(&self, id: &EarId) -> Option<EarRecord> {
        todo!("tâche 2")
    }
}
