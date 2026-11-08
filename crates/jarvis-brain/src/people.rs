//! Annuaire des personnes et des rôles (phase 8). SQUELETTE de la tâche 1 : les
//! signatures sont figées, les corps (`todo!`) sont écrits par la tâche 2.
//! Seul [`People::admin_only`] est réel : annuaire en mémoire, propriétaire `admin`.

use jarvis_protocol::{EarId, PersonSummary, RoleSummary};
use jarvis_skills::Risk;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

/// Niveau de confiance dans l'identité de celui qui parle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trust {
    /// Texte tapé (socket Unix du même utilisateur).
    Typed,
    /// Voix reconnue par l'empreinte du propriétaire.
    Recognized,
    /// Pas d'empreinte à vérifier : phrase acceptée (tout sauf l'agent).
    Accepted,
    /// Voix non vérifiée : ni action risquée, ni agent.
    Unverified,
}

/// Droits d'une personne, déduits de son rôle.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Rights {
    /// Rôle `admin` : tout.
    pub all: bool,
    /// Noms d'outils, préfixes `x_*` ou `*`.
    pub tools: Vec<String>,
    pub confirm: bool,
    pub agent: bool,
}

/// Qui parle : personne, rôle, confiance et droits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Speaker {
    pub person: String,
    pub role: String,
    pub trust: Trust,
    pub rights: Rights,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    Allowed,
    Denied,
}

impl Speaker {
    /// Peut-il lancer `tool` ? Règles de la tâche 2 du plan.
    pub fn check(&self, _tool: &str, _risk: Risk) -> Access {
        todo!("tâche 2")
    }

    /// Peut-il répondre « oui » à la question de `tool` ?
    pub fn may_confirm(&self, _tool: &str, _risk: Risk) -> bool {
        todo!("tâche 2")
    }
}

/// À qui appartient une oreille.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EarPerson {
    /// Non rattachée (ou locale) : le propriétaire.
    Owner,
    Guest(String),
    /// Rattachée à une personne disparue : aucun droit.
    Unknown,
}

#[derive(Debug, thiserror::Error)]
pub enum PeopleError {
    #[error("{0}")]
    Invalid(String),
    #[error("« {0} » existe déjà.")]
    Exists(String),
    #[error("« {0} » est inconnu.")]
    Unknown(String),
    #[error("{0}")]
    Protected(String),
    #[error("{0}")]
    InUse(String),
    #[error("L'annuaire est illisible : modifications refusées.")]
    Degraded,
    #[error("Écriture impossible : {0}.")]
    Io(#[from] io::Error),
}

/// État en mémoire (la tâche 2 y ajoute rôles, personnes, oreilles, chemin).
#[allow(dead_code)]
struct State {
    owner: String,
}

/// L'annuaire, partagé en `Arc` entre le cerveau, l'écoute, le hub et le serveur.
pub struct People {
    #[allow(dead_code)]
    state: RwLock<State>,
}

impl People {
    /// Annuaire en mémoire : le propriétaire `admin`, aucune oreille rattachée.
    pub fn admin_only(owner: &str) -> People {
        People {
            state: RwLock::new(State {
                owner: owner.to_string(),
            }),
        }
    }

    /// Ouvre `path` ; le crée avec le propriétaire `admin` s'il manque.
    pub fn open(_path: &Path, _owner: &str) -> io::Result<People> {
        todo!("tâche 2")
    }

    /// Fichier illisible ou invalide : mode dégradé.
    pub fn open_or_degraded(_path: &Path, _owner: &str) -> People {
        todo!("tâche 2")
    }

    /// `~/.local/share/jarvis/people.toml`.
    pub fn default_path() -> PathBuf {
        todo!("tâche 2")
    }

    /// Raison du mode dégradé, s'il y en a un.
    pub fn degraded(&self) -> Option<String> {
        todo!("tâche 2")
    }

    pub fn owner_name(&self) -> String {
        todo!("tâche 2")
    }

    pub fn person_of(&self, _ear: &EarId) -> EarPerson {
        todo!("tâche 2")
    }

    /// Personne ou rôle inconnu : droits nuls.
    pub fn speaker(&self, _person: &str, _trust: Trust) -> Speaker {
        todo!("tâche 2")
    }

    pub fn owner_speaker(&self, _trust: Trust) -> Speaker {
        todo!("tâche 2")
    }

    /// Texte tapé : le propriétaire, `Trust::Typed`.
    pub fn typed(&self) -> Speaker {
        todo!("tâche 2")
    }

    pub fn add_person(&self, _name: &str, _role: &str) -> Result<(), PeopleError> {
        todo!("tâche 2")
    }

    pub fn remove_person(&self, _name: &str) -> Result<(), PeopleError> {
        todo!("tâche 2")
    }

    pub fn set_role(&self, _name: &str, _role: &str) -> Result<(), PeopleError> {
        todo!("tâche 2")
    }

    /// Rattache l'oreille à la personne (après l'avoir détachée de l'ancienne).
    pub fn attach(&self, _person: &str, _ear: &EarId) -> Result<(), PeopleError> {
        todo!("tâche 2")
    }

    pub fn detach(&self, _ear: &EarId) -> Result<(), PeopleError> {
        todo!("tâche 2")
    }

    /// Retire les oreilles qui ne sont plus au registre.
    pub fn prune(&self, _known: &[EarId]) -> Result<(), PeopleError> {
        todo!("tâche 2")
    }

    pub fn add_role(
        &self,
        _name: &str,
        _tools: Vec<String>,
        _confirm: bool,
        _agent: bool,
        _known_tools: &[String],
    ) -> Result<(), PeopleError> {
        todo!("tâche 2")
    }

    pub fn edit_role(
        &self,
        _name: &str,
        _tools: Option<Vec<String>>,
        _confirm: Option<bool>,
        _agent: Option<bool>,
        _known_tools: &[String],
    ) -> Result<(), PeopleError> {
        todo!("tâche 2")
    }

    pub fn remove_role(&self, _name: &str) -> Result<(), PeopleError> {
        todo!("tâche 2")
    }

    pub fn people_summary(
        &self,
        _ear_names: &dyn Fn(&EarId) -> Option<String>,
        _voiceprint: bool,
    ) -> Vec<PersonSummary> {
        todo!("tâche 2")
    }

    pub fn roles_summary(&self) -> Vec<RoleSummary> {
        todo!("tâche 2")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admin_only_se_construit() {
        let _ = People::admin_only("Charles");
    }
}
