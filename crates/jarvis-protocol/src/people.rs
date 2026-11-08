//! Personnes et rôles tels que le socket de contrôle les décrit (phase 8).

use serde::{Deserialize, Serialize};

/// Une personne de l'annuaire, avec ses oreilles (noms d'affichage).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersonSummary {
    pub name: String,
    pub role: String,
    /// Propriétaire : celle de l'empreinte vocale, toujours `admin`.
    pub owner: bool,
    /// Noms des oreilles rattachées.
    pub ears: Vec<String>,
    /// Une empreinte vocale est enregistrée pour cette personne.
    pub voiceprint: bool,
}

/// Un rôle : outils autorisés et droits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleSummary {
    pub name: String,
    pub tools: Vec<String>,
    pub confirm: bool,
    pub agent: bool,
    /// Rôle intégré (`admin`) : ni modifiable ni supprimable.
    pub builtin: bool,
    /// Nombre de personnes qui ont ce rôle.
    pub people: usize,
}
