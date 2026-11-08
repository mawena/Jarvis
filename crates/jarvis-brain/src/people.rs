//! Annuaire des personnes et des rôles (phase 8) : fichier `people.toml` (0600,
//! écriture atomique), droits de chaque rôle, rattachement des oreilles.
//!
//! Le rôle `admin` est intégré (jamais écrit, tous les droits). Le propriétaire est
//! la seule personne de l'empreinte vocale : toujours `admin`, jamais supprimée.
//! Fichier illisible ou invalide : mode dégradé (moindre privilège).

use jarvis_protocol::{EarId, PersonSummary, RoleSummary};
use jarvis_skills::Risk;
use jarvis_skills::text::fold;
use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};
use tracing::warn;

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
    /// Peut-il lancer `tool` ? Les règles, dans l'ordre : l'agent exige le droit
    /// `agent` et une voix reconnue (ou du texte tapé) ; l'outil doit être dans la
    /// liste du rôle ; une action risquée exige `confirm` et une voix vérifiée.
    pub fn check(&self, tool: &str, risk: Risk) -> Access {
        let r = &self.rights;
        if tool.starts_with("agent_") {
            let sure = matches!(self.trust, Trust::Recognized | Trust::Typed);
            return allow((r.agent || r.all) && sure);
        }
        if !r.all && !r.tools.iter().any(|p| pattern_matches(p, tool)) {
            return Access::Denied;
        }
        if risk == Risk::Risky {
            return allow((r.confirm || r.all) && self.trust != Trust::Unverified);
        }
        Access::Allowed
    }

    /// Peut-il répondre « oui » à la question de `tool` ?
    pub fn may_confirm(&self, tool: &str, risk: Risk) -> bool {
        (self.rights.confirm || self.rights.all)
            && matches!(self.trust, Trust::Recognized | Trust::Accepted)
            && self.check(tool, risk) == Access::Allowed
    }
}

fn allow(ok: bool) -> Access {
    if ok { Access::Allowed } else { Access::Denied }
}

/// Motif exact, `préfixe*` ou `*`.
fn pattern_matches(pattern: &str, tool: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => tool.starts_with(prefix),
        None => pattern == tool,
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

const ADMIN: &str = "admin";
const NAME_MAX: usize = 40;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct RoleDef {
    name: String,
    #[serde(default)]
    tools: Vec<String>,
    #[serde(default)]
    confirm: bool,
    #[serde(default)]
    agent: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct PersonDef {
    name: String,
    role: String,
    #[serde(default)]
    owner: bool,
    #[serde(default)]
    ears: Vec<EarId>,
}

/// Format de `people.toml`. `attached` liste toutes les oreilles rattachées : une
/// oreille qui y figure sans appartenir à une personne (supprimée à la main)
/// n'a aucun droit au lieu de retomber sur le propriétaire.
#[derive(Debug, Default, Serialize, Deserialize)]
struct FileData {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    attached: Vec<EarId>,
    #[serde(default, rename = "role", skip_serializing_if = "Vec::is_empty")]
    roles: Vec<RoleDef>,
    #[serde(default, rename = "person")]
    people: Vec<PersonDef>,
}

/// Contenu de l'annuaire (copié, modifié, écrit, puis adopté).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Data {
    roles: Vec<RoleDef>,
    people: Vec<PersonDef>,
    /// Oreilles rattachées à une personne disparue.
    orphans: Vec<EarId>,
}

impl Data {
    fn new(owner: &str) -> Data {
        Data {
            roles: Vec::new(),
            people: vec![PersonDef {
                name: owner.to_string(),
                role: ADMIN.to_string(),
                owner: true,
                ears: Vec::new(),
            }],
            orphans: Vec::new(),
        }
    }

    fn person(&self, name: &str) -> Option<&PersonDef> {
        let key = fold(name);
        self.people.iter().find(|p| fold(&p.name) == key)
    }

    fn person_mut(&mut self, name: &str) -> Option<&mut PersonDef> {
        let key = fold(name);
        self.people.iter_mut().find(|p| fold(&p.name) == key)
    }

    fn role(&self, name: &str) -> Option<&RoleDef> {
        let key = fold(name);
        self.roles.iter().find(|r| fold(&r.name) == key)
    }

    fn owner(&self) -> Option<&PersonDef> {
        self.people.iter().find(|p| p.owner)
    }

    fn to_file(&self) -> FileData {
        let mut attached: Vec<EarId> = self
            .people
            .iter()
            .flat_map(|p| p.ears.iter().cloned())
            .collect();
        attached.extend(self.orphans.iter().cloned());
        FileData {
            attached,
            roles: self.roles.clone(),
            people: self.people.clone(),
        }
    }

    fn from_file(file: FileData) -> Result<Data, String> {
        let mut roles: Vec<RoleDef> = Vec::new();
        for role in file.roles {
            if fold(&role.name) == ADMIN {
                warn!("people.toml : le rôle « admin » est intégré, sa section est ignorée");
                continue;
            }
            let name = validate_name(&role.name).map_err(|e| e.to_string())?;
            if roles.iter().any(|r| fold(&r.name) == fold(&name)) {
                return Err(format!("le rôle « {name} » est en double"));
            }
            roles.push(RoleDef { name, ..role });
        }
        let mut people: Vec<PersonDef> = Vec::new();
        for person in file.people {
            let name = validate_name(&person.name).map_err(|e| e.to_string())?;
            if people.iter().any(|p| fold(&p.name) == fold(&name)) {
                return Err(format!("la personne « {name} » est en double"));
            }
            people.push(PersonDef { name, ..person });
        }
        match people.iter().filter(|p| p.owner).count() {
            0 => return Err("aucun propriétaire (owner = true)".to_string()),
            1 => {}
            _ => return Err("plusieurs propriétaires".to_string()),
        }
        if let Some(o) = people.iter().find(|p| p.owner)
            && fold(&o.role) != ADMIN
        {
            return Err(format!("le propriétaire « {} » doit être admin", o.name));
        }
        let mut seen: Vec<&EarId> = Vec::new();
        for ear in people.iter().flat_map(|p| p.ears.iter()) {
            if seen.contains(&ear) {
                return Err(format!("l'oreille {ear} est rattachée à deux personnes"));
            }
            seen.push(ear);
        }
        let orphans = file
            .attached
            .into_iter()
            .filter(|e| !seen.contains(&e))
            .collect();
        Ok(Data {
            roles,
            people,
            orphans,
        })
    }
}

/// État en mémoire.
struct State {
    data: Data,
    /// `None` : annuaire en mémoire seulement.
    path: Option<PathBuf>,
    degraded: Option<String>,
}

/// L'annuaire, partagé en `Arc` entre le cerveau, l'écoute, le hub et le serveur.
pub struct People {
    state: RwLock<State>,
}

/// Nom rogné, 1 à 40 caractères, sans caractère de contrôle, avec de quoi comparer.
fn validate_name(name: &str) -> Result<String, PeopleError> {
    let name = name.trim();
    let len = name.chars().count();
    if len == 0 {
        return Err(PeopleError::Invalid("Le nom est vide.".to_string()));
    }
    if len > NAME_MAX {
        return Err(PeopleError::Invalid(format!(
            "Le nom est trop long ({NAME_MAX} caractères au plus)."
        )));
    }
    if name.chars().any(char::is_control) {
        return Err(PeopleError::Invalid(
            "Le nom contient un caractère de contrôle.".to_string(),
        ));
    }
    if fold(name).is_empty() {
        return Err(PeopleError::Invalid(
            "Le nom doit contenir une lettre ou un chiffre.".to_string(),
        ));
    }
    Ok(name.to_string())
}

fn validate_role_name(name: &str) -> Result<String, PeopleError> {
    let name = validate_name(name)?;
    if fold(&name) == ADMIN {
        return Err(PeopleError::Protected(
            "Le rôle admin est intégré et réservé.".to_string(),
        ));
    }
    Ok(name)
}

/// Motifs d'outils valides : jamais `agent_*`, chacun doit viser un outil connu.
fn validate_tools(tools: Vec<String>, known: &[String]) -> Result<Vec<String>, PeopleError> {
    let mut out: Vec<String> = Vec::new();
    for tool in tools {
        let tool = tool.trim().to_string();
        if tool.is_empty() {
            continue;
        }
        if tool.starts_with("agent_") {
            return Err(PeopleError::Invalid(format!(
                "« {tool} » : l'agent ne s'écrit pas dans les outils, utilisez le droit agent."
            )));
        }
        if !known
            .iter()
            .any(|k| !k.starts_with("agent_") && pattern_matches(&tool, k))
        {
            return Err(PeopleError::Invalid(format!(
                "aucun outil ne correspond à « {tool} »"
            )));
        }
        if !out.contains(&tool) {
            out.push(tool);
        }
    }
    Ok(out)
}

fn lock_read(l: &RwLock<State>) -> RwLockReadGuard<'_, State> {
    l.read().unwrap_or_else(|e| e.into_inner())
}

fn lock_write(l: &RwLock<State>) -> RwLockWriteGuard<'_, State> {
    l.write().unwrap_or_else(|e| e.into_inner())
}

impl People {
    /// Annuaire en mémoire : le propriétaire `admin`, aucune oreille rattachée.
    pub fn admin_only(owner: &str) -> People {
        People {
            state: RwLock::new(State {
                data: Data::new(owner),
                path: None,
                degraded: None,
            }),
        }
    }

    /// Ouvre `path` ; le crée avec le propriétaire `admin` s'il manque.
    pub fn open(path: &Path, owner: &str) -> io::Result<People> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                let invalid = |why: String| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("{} invalide : {why}", path.display()),
                    )
                };
                let file: FileData = toml::from_str(&text).map_err(|e| invalid(e.to_string()))?;
                let data = Data::from_file(file).map_err(invalid)?;
                Ok(People {
                    state: RwLock::new(State {
                        data,
                        path: Some(path.to_path_buf()),
                        degraded: None,
                    }),
                })
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let owner = validate_name(owner)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e.to_string()))?;
                let data = Data::new(&owner);
                write_file(path, &data)?;
                Ok(People {
                    state: RwLock::new(State {
                        data,
                        path: Some(path.to_path_buf()),
                        degraded: None,
                    }),
                })
            }
            Err(e) => Err(io::Error::new(
                e.kind(),
                format!("{} illisible : {e}", path.display()),
            )),
        }
    }

    /// Fichier illisible ou invalide : mode dégradé (seuls le propriétaire reconnu
    /// et le texte tapé ont des droits, aucune écriture).
    pub fn open_or_degraded(path: &Path, owner: &str) -> People {
        match People::open(path, owner) {
            Ok(people) => people,
            Err(e) => People {
                state: RwLock::new(State {
                    data: Data::new(owner),
                    path: Some(path.to_path_buf()),
                    degraded: Some(e.to_string()),
                }),
            },
        }
    }

    /// `~/.local/share/jarvis/people.toml`.
    pub fn default_path() -> PathBuf {
        match directories::ProjectDirs::from("", "", "jarvis") {
            Some(dirs) => dirs.data_dir().join("people.toml"),
            None => PathBuf::from(".local/share/jarvis/people.toml"),
        }
    }

    /// Raison du mode dégradé, s'il y en a un.
    pub fn degraded(&self) -> Option<String> {
        lock_read(&self.state).degraded.clone()
    }

    pub fn owner_name(&self) -> String {
        let st = lock_read(&self.state);
        st.data.owner().map(|o| o.name.clone()).unwrap_or_default()
    }

    pub fn person_of(&self, ear: &EarId) -> EarPerson {
        let st = lock_read(&self.state);
        if st.degraded.is_some() || st.data.orphans.contains(ear) {
            return EarPerson::Unknown;
        }
        match st.data.people.iter().find(|p| p.ears.contains(ear)) {
            Some(p) if p.owner => EarPerson::Owner,
            Some(p) => EarPerson::Guest(p.name.clone()),
            None => EarPerson::Owner,
        }
    }

    /// Personne ou rôle inconnu : droits nuls.
    pub fn speaker(&self, person: &str, trust: Trust) -> Speaker {
        let st = lock_read(&self.state);
        let none = |role: &str| Speaker {
            person: person.to_string(),
            role: role.to_string(),
            trust,
            rights: Rights::default(),
        };
        if st.degraded.is_some() {
            return none("");
        }
        let Some(p) = st.data.person(person) else {
            return none("");
        };
        let rights = if fold(&p.role) == ADMIN {
            Rights {
                all: true,
                tools: Vec::new(),
                confirm: true,
                agent: true,
            }
        } else if let Some(r) = st.data.role(&p.role) {
            Rights {
                all: false,
                tools: r.tools.clone(),
                confirm: r.confirm,
                agent: r.agent,
            }
        } else {
            Rights::default()
        };
        Speaker {
            person: p.name.clone(),
            role: p.role.clone(),
            trust,
            rights,
        }
    }

    pub fn owner_speaker(&self, trust: Trust) -> Speaker {
        let (name, degraded) = {
            let st = lock_read(&self.state);
            let name = st.data.owner().map(|o| o.name.clone()).unwrap_or_default();
            (name, st.degraded.is_some())
        };
        if degraded {
            return Speaker {
                person: name,
                role: ADMIN.to_string(),
                trust,
                rights: Rights {
                    all: true,
                    tools: Vec::new(),
                    confirm: true,
                    agent: true,
                },
            };
        }
        self.speaker(&name, trust)
    }

    /// Texte tapé : le propriétaire, `Trust::Typed`.
    pub fn typed(&self) -> Speaker {
        self.owner_speaker(Trust::Typed)
    }

    /// Applique `f` sur une copie, l'écrit, et n'adopte l'état que si l'écriture
    /// réussit. Rien n'est écrit si `f` ne change rien.
    fn change<R>(
        &self,
        f: impl FnOnce(&mut Data) -> Result<R, PeopleError>,
    ) -> Result<R, PeopleError> {
        let mut st = lock_write(&self.state);
        if st.degraded.is_some() {
            return Err(PeopleError::Degraded);
        }
        let mut next = st.data.clone();
        let out = f(&mut next)?;
        if next != st.data {
            if let Some(path) = &st.path {
                write_file(path, &next)?;
            }
            st.data = next;
        }
        Ok(out)
    }

    pub fn add_person(&self, name: &str, role: &str) -> Result<(), PeopleError> {
        let name = validate_name(name)?;
        self.change(|d| {
            if d.person(&name).is_some() {
                return Err(PeopleError::Exists(name));
            }
            let role = existing_role(d, role)?;
            d.people.push(PersonDef {
                name,
                role,
                owner: false,
                ears: Vec::new(),
            });
            Ok(())
        })
    }

    pub fn remove_person(&self, name: &str) -> Result<(), PeopleError> {
        self.change(|d| {
            let p = d
                .person(name)
                .ok_or_else(|| PeopleError::Unknown(name.trim().to_string()))?;
            if p.owner {
                return Err(PeopleError::Protected(format!(
                    "{} est le propriétaire : impossible de la supprimer.",
                    p.name
                )));
            }
            if !p.ears.is_empty() {
                return Err(PeopleError::InUse(format!(
                    "{} a encore {} oreille(s) : retirez-les d'abord.",
                    p.name,
                    p.ears.len()
                )));
            }
            let key = fold(&p.name);
            d.people.retain(|p| fold(&p.name) != key);
            Ok(())
        })
    }

    pub fn set_role(&self, name: &str, role: &str) -> Result<(), PeopleError> {
        self.change(|d| {
            let role = existing_role(d, role)?;
            let p = d
                .person_mut(name)
                .ok_or_else(|| PeopleError::Unknown(name.trim().to_string()))?;
            if p.owner {
                return Err(PeopleError::Protected(format!(
                    "{} est le propriétaire : son rôle reste admin.",
                    p.name
                )));
            }
            p.role = role;
            Ok(())
        })
    }

    /// Rattache l'oreille à la personne (après l'avoir détachée de l'ancienne).
    pub fn attach(&self, person: &str, ear: &EarId) -> Result<(), PeopleError> {
        self.change(|d| {
            if d.person(person).is_none() {
                return Err(PeopleError::Unknown(person.trim().to_string()));
            }
            remove_ear(d, |e| e == ear);
            if let Some(p) = d.person_mut(person) {
                p.ears.push(ear.clone());
            }
            Ok(())
        })
    }

    pub fn detach(&self, ear: &EarId) -> Result<(), PeopleError> {
        if self.degraded().is_some() {
            return Ok(());
        }
        self.change(|d| {
            remove_ear(d, |e| e == ear);
            Ok(())
        })
    }

    /// Retire les oreilles qui ne sont plus au registre.
    pub fn prune(&self, known: &[EarId]) -> Result<(), PeopleError> {
        if self.degraded().is_some() {
            return Ok(());
        }
        self.change(|d| {
            remove_ear(d, |e| !known.contains(e));
            Ok(())
        })
    }

    pub fn add_role(
        &self,
        name: &str,
        tools: Vec<String>,
        confirm: bool,
        agent: bool,
        known_tools: &[String],
    ) -> Result<(), PeopleError> {
        let name = validate_role_name(name)?;
        let tools = validate_tools(tools, known_tools)?;
        self.change(|d| {
            if d.role(&name).is_some() {
                return Err(PeopleError::Exists(name));
            }
            d.roles.push(RoleDef {
                name,
                tools,
                confirm,
                agent,
            });
            Ok(())
        })
    }

    pub fn edit_role(
        &self,
        name: &str,
        tools: Option<Vec<String>>,
        confirm: Option<bool>,
        agent: Option<bool>,
        known_tools: &[String],
    ) -> Result<(), PeopleError> {
        if fold(name) == ADMIN {
            return Err(PeopleError::Protected(
                "Le rôle admin est intégré : il ne se modifie pas.".to_string(),
            ));
        }
        let tools = tools.map(|t| validate_tools(t, known_tools)).transpose()?;
        self.change(|d| {
            let key = fold(name);
            let role = d
                .roles
                .iter_mut()
                .find(|r| fold(&r.name) == key)
                .ok_or_else(|| PeopleError::Unknown(name.trim().to_string()))?;
            if let Some(t) = tools {
                role.tools = t;
            }
            if let Some(c) = confirm {
                role.confirm = c;
            }
            if let Some(a) = agent {
                role.agent = a;
            }
            Ok(())
        })
    }

    pub fn remove_role(&self, name: &str) -> Result<(), PeopleError> {
        if fold(name) == ADMIN {
            return Err(PeopleError::Protected(
                "Le rôle admin est intégré : il ne se supprime pas.".to_string(),
            ));
        }
        self.change(|d| {
            let role = d
                .role(name)
                .ok_or_else(|| PeopleError::Unknown(name.trim().to_string()))?;
            let key = fold(&role.name);
            let used = d.people.iter().filter(|p| fold(&p.role) == key).count();
            if used > 0 {
                return Err(PeopleError::InUse(format!(
                    "Le rôle {} est encore donné à {used} personne(s).",
                    role.name
                )));
            }
            d.roles.retain(|r| fold(&r.name) != key);
            Ok(())
        })
    }

    pub fn people_summary(
        &self,
        ear_names: &dyn Fn(&EarId) -> Option<String>,
        voiceprint: bool,
    ) -> Vec<PersonSummary> {
        let st = lock_read(&self.state);
        st.data
            .people
            .iter()
            .map(|p| PersonSummary {
                name: p.name.clone(),
                role: p.role.clone(),
                owner: p.owner,
                ears: p
                    .ears
                    .iter()
                    .map(|e| ear_names(e).unwrap_or_else(|| e.to_string()))
                    .collect(),
                voiceprint: p.owner && voiceprint,
            })
            .collect()
    }

    pub fn roles_summary(&self) -> Vec<RoleSummary> {
        let st = lock_read(&self.state);
        let count = |name: &str| {
            let key = fold(name);
            st.data
                .people
                .iter()
                .filter(|p| fold(&p.role) == key)
                .count()
        };
        let mut out = vec![RoleSummary {
            name: ADMIN.to_string(),
            tools: vec!["*".to_string()],
            confirm: true,
            agent: true,
            builtin: true,
            people: count(ADMIN),
        }];
        out.extend(st.data.roles.iter().map(|r| RoleSummary {
            name: r.name.clone(),
            tools: r.tools.clone(),
            confirm: r.confirm,
            agent: r.agent,
            builtin: false,
            people: count(&r.name),
        }));
        out
    }
}

/// Nom canonique d'un rôle existant (`admin` compris).
fn existing_role(d: &Data, role: &str) -> Result<String, PeopleError> {
    if fold(role) == ADMIN {
        return Ok(ADMIN.to_string());
    }
    d.role(role)
        .map(|r| r.name.clone())
        .ok_or_else(|| PeopleError::Unknown(role.trim().to_string()))
}

fn remove_ear(d: &mut Data, gone: impl Fn(&EarId) -> bool) {
    for p in &mut d.people {
        p.ears.retain(|e| !gone(e));
    }
    d.orphans.retain(|e| !gone(e));
}

fn write_file(path: &Path, data: &Data) -> io::Result<()> {
    let text = toml::to_string(&data.to_file()).map_err(io::Error::other)?;
    crate::ears::write_private(path, text.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::TempDir;

    fn tools() -> Vec<String> {
        [
            "media_next",
            "media_play",
            "app_launch",
            "web_search",
            "suspend",
            "agent_start",
        ]
        .map(String::from)
        .to_vec()
    }

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    fn ear(id: &str) -> EarId {
        EarId::new(id)
    }

    fn fresh() -> (TempDir, PathBuf, People) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("jarvis").join("people.toml");
        let people = People::open(&path, "Charles").unwrap();
        (dir, path, people)
    }

    fn with_lea(people: &People) {
        people
            .add_role(
                "famille",
                s(&["media_*", "app_launch"]),
                false,
                false,
                &tools(),
            )
            .unwrap();
        people.add_person("Léa", "famille").unwrap();
    }

    fn guest(rights: Rights, trust: Trust) -> Speaker {
        Speaker {
            person: "Léa".into(),
            role: "famille".into(),
            trust,
            rights,
        }
    }

    fn admin(trust: Trust) -> Speaker {
        Speaker {
            person: "Charles".into(),
            role: "admin".into(),
            trust,
            rights: Rights {
                all: true,
                tools: vec![],
                confirm: true,
                agent: true,
            },
        }
    }

    // --- annuaire ---

    #[test]
    fn annuaire_cree_avec_charles_admin() {
        let (_d, path, people) = fresh();
        assert!(path.exists());
        assert_eq!(people.owner_name(), "Charles");
        assert!(people.degraded().is_none());
        let sp = people.owner_speaker(Trust::Recognized);
        assert_eq!(sp.role, "admin");
        assert!(sp.rights.all);
        let list = people.people_summary(&|_| None, true);
        assert_eq!(list.len(), 1);
        assert!(list[0].owner && list[0].voiceprint);
        let roles = people.roles_summary();
        assert_eq!(roles.len(), 1);
        assert!(roles[0].builtin && roles[0].people == 1);
        assert_eq!(people.typed().trust, Trust::Typed);
    }

    #[test]
    fn fichier_prive_0600_dossier_0700() {
        let (_d, path, people) = fresh();
        people.add_person("Léa", "admin").unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(path.parent().unwrap()), 0o700);
    }

    #[test]
    fn relu_apres_redemarrage() {
        let (_d, path, people) = fresh();
        with_lea(&people);
        people.attach("Léa", &ear("e1")).unwrap();
        let again = People::open(&path, "Autre").unwrap();
        assert_eq!(again.owner_name(), "Charles");
        assert_eq!(again.person_of(&ear("e1")), EarPerson::Guest("Léa".into()));
        let lea = again.speaker("Léa", Trust::Unverified);
        assert_eq!(lea.rights.tools, s(&["media_*", "app_launch"]));
        assert!(!lea.rights.all);
        let roles = again.roles_summary();
        assert_eq!(roles.len(), 2);
        assert_eq!(roles[1].people, 1);
    }

    #[test]
    fn ecriture_atomique_en_cas_d_echec_etat_inchange() {
        let (_d, path, people) = fresh();
        std::fs::remove_file(&path).unwrap();
        // Un dossier à la place du fichier : le rename échoue.
        std::fs::create_dir(&path).unwrap();
        let err = people.add_person("Léa", "admin").unwrap_err();
        assert!(matches!(err, PeopleError::Io(_)));
        assert_eq!(people.people_summary(&|_| None, false).len(), 1);
        assert!(
            std::fs::read_dir(path.parent().unwrap())
                .unwrap()
                .all(|e| !e.unwrap().file_name().to_string_lossy().ends_with(".tmp"))
        );
    }

    #[test]
    fn oreille_non_rattachee_vaut_le_proprietaire() {
        let (_d, _p, people) = fresh();
        assert_eq!(people.person_of(&ear("inconnue")), EarPerson::Owner);
        let mem = People::admin_only("Charles");
        assert_eq!(mem.person_of(&ear("x")), EarPerson::Owner);
    }

    #[test]
    fn oreille_rattachee_a_une_personne() {
        let (_d, _p, people) = fresh();
        with_lea(&people);
        people.attach("léa", &ear("e1")).unwrap();
        assert_eq!(people.person_of(&ear("e1")), EarPerson::Guest("Léa".into()));
        people.attach("Charles", &ear("e2")).unwrap();
        assert_eq!(people.person_of(&ear("e2")), EarPerson::Owner);
        assert!(matches!(
            people.attach("Zoé", &ear("e3")),
            Err(PeopleError::Unknown(_))
        ));
    }

    #[test]
    fn attacher_detache_de_l_ancienne_personne() {
        let (_d, path, people) = fresh();
        with_lea(&people);
        people.attach("Léa", &ear("e1")).unwrap();
        people.attach("Charles", &ear("e1")).unwrap();
        assert_eq!(people.person_of(&ear("e1")), EarPerson::Owner);
        people.remove_person("Léa").unwrap();
        let again = People::open(&path, "Charles").unwrap();
        assert_eq!(again.person_of(&ear("e1")), EarPerson::Owner);
        again.detach(&ear("e1")).unwrap();
        assert_eq!(again.person_of(&ear("e1")), EarPerson::Owner);
    }

    #[test]
    fn personne_supprimee_oreille_sans_droit() {
        let (_d, path, people) = fresh();
        with_lea(&people);
        people.attach("Léa", &ear("e1")).unwrap();
        // Édition à la main : le bloc de Léa disparaît.
        let text = std::fs::read_to_string(&path).unwrap();
        let cut = text.find("[[person]]\nname = \"Léa\"").unwrap();
        std::fs::write(&path, &text[..cut]).unwrap();
        let again = People::open(&path, "Charles").unwrap();
        assert_eq!(again.person_of(&ear("e1")), EarPerson::Unknown);
        assert_eq!(
            again.speaker("Léa", Trust::Recognized).rights,
            Rights::default()
        );
        // Rattacher l'oreille ailleurs la remet d'aplomb ; le détachement la libère.
        again.prune(&[]).unwrap();
        assert_eq!(again.person_of(&ear("e1")), EarPerson::Owner);
    }

    #[test]
    fn proprietaire_non_supprimable() {
        let (_d, _p, people) = fresh();
        assert!(matches!(
            people.remove_person("charles"),
            Err(PeopleError::Protected(_))
        ));
        assert!(matches!(
            people.remove_person("Zoé"),
            Err(PeopleError::Unknown(_))
        ));
    }

    #[test]
    fn proprietaire_non_modifiable() {
        let (_d, _p, people) = fresh();
        people
            .add_role("famille", s(&["media_*"]), false, false, &tools())
            .unwrap();
        assert!(matches!(
            people.set_role("Charles", "famille"),
            Err(PeopleError::Protected(_))
        ));
        assert_eq!(people.owner_speaker(Trust::Typed).role, "admin");
    }

    #[test]
    fn personne_avec_oreilles_non_supprimable() {
        let (_d, _p, people) = fresh();
        with_lea(&people);
        people.attach("Léa", &ear("e1")).unwrap();
        assert!(matches!(
            people.remove_person("Léa"),
            Err(PeopleError::InUse(_))
        ));
        people.detach(&ear("e1")).unwrap();
        people.remove_person("Léa").unwrap();
    }

    #[test]
    fn role_admin_non_modifiable_ni_supprimable() {
        let (_d, _p, people) = fresh();
        assert!(matches!(
            people.add_role("Admin", vec![], true, true, &tools()),
            Err(PeopleError::Protected(_))
        ));
        assert!(matches!(
            people.edit_role("admin", None, Some(false), None, &tools()),
            Err(PeopleError::Protected(_))
        ));
        assert!(matches!(
            people.remove_role("admin"),
            Err(PeopleError::Protected(_))
        ));
    }

    #[test]
    fn role_en_service_non_supprimable() {
        let (_d, _p, people) = fresh();
        with_lea(&people);
        assert!(matches!(
            people.remove_role("famille"),
            Err(PeopleError::InUse(_))
        ));
        people.remove_person("Léa").unwrap();
        people.remove_role("famille").unwrap();
        assert_eq!(people.roles_summary().len(), 1);
    }

    #[test]
    fn nom_double_refuse_sans_casse_ni_accents() {
        let (_d, _p, people) = fresh();
        with_lea(&people);
        assert!(matches!(
            people.add_person("LEA", "famille"),
            Err(PeopleError::Exists(_))
        ));
        assert!(matches!(
            people.add_person("charlès", "famille"),
            Err(PeopleError::Exists(_))
        ));
        assert!(matches!(
            people.add_role("FAMILLE", vec![], false, false, &tools()),
            Err(PeopleError::Exists(_))
        ));
    }

    #[test]
    fn nom_vide_trop_long_ou_de_controle_refuse() {
        let (_d, _p, people) = fresh();
        for bad in ["", "   ", &"x".repeat(41), "Lé\na", "!!!"] {
            assert!(
                matches!(
                    people.add_person(bad, "admin"),
                    Err(PeopleError::Invalid(_))
                ),
                "{bad:?}"
            );
        }
        people.add_person(&"é".repeat(40), "admin").unwrap();
        people.add_person("  Zoé  ", "admin").unwrap();
        assert!(
            people
                .people_summary(&|_| None, false)
                .iter()
                .any(|p| p.name == "Zoé")
        );
    }

    #[test]
    fn motif_sans_outil_refuse() {
        let (_d, _p, people) = fresh();
        let err = people
            .add_role("x", s(&["mdia_*"]), false, false, &tools())
            .unwrap_err();
        assert_eq!(err.to_string(), "aucun outil ne correspond à « mdia_* »");
        people
            .add_role("x", s(&["*", "media_play"]), false, false, &tools())
            .unwrap();
    }

    #[test]
    fn motif_agent_refuse_dans_tools() {
        let (_d, _p, people) = fresh();
        for t in ["agent_start", "agent_*"] {
            let err = people
                .add_role("x", s(&[t]), false, false, &tools())
                .unwrap_err();
            assert!(err.to_string().contains("droit agent"), "{err}");
        }
    }

    #[test]
    fn fichier_illisible_moindre_privilege() {
        let (_d, path, _p) = fresh();
        std::fs::write(&path, "ceci n'est [pas du toml").unwrap();
        assert!(People::open(&path, "Charles").is_err());
        let people = People::open_or_degraded(&path, "Charles");
        assert!(people.degraded().unwrap().contains("people.toml"));
        let owner = people.owner_speaker(Trust::Recognized);
        assert!(owner.rights.all);
        assert!(people.typed().rights.all);
        assert_eq!(people.person_of(&ear("e1")), EarPerson::Unknown);
        assert_eq!(
            people.speaker("Charles", Trust::Recognized).rights,
            Rights::default()
        );
        // Le fichier abîmé n'est pas écrasé.
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "ceci n'est [pas du toml"
        );
    }

    #[test]
    fn role_inconnu_aucun_droit() {
        let (_d, path, _p) = fresh();
        std::fs::write(
            &path,
            "[[person]]\nname = \"Charles\"\nrole = \"admin\"\nowner = true\n\n\
             [[person]]\nname = \"Léa\"\nrole = \"fantome\"\n",
        )
        .unwrap();
        let people = People::open(&path, "Charles").unwrap();
        let lea = people.speaker("Léa", Trust::Recognized);
        assert_eq!(lea.rights, Rights::default());
        assert_eq!(lea.check("media_play", Risk::Direct), Access::Denied);
        let nobody = people.speaker("Inconnu", Trust::Recognized);
        assert_eq!(nobody.check("media_play", Risk::Direct), Access::Denied);
    }

    #[test]
    fn section_admin_du_fichier_ignoree() {
        let (_d, path, _p) = fresh();
        std::fs::write(
            &path,
            "[[role]]\nname = \"admin\"\n\n[[person]]\nname = \"Charles\"\nrole = \"admin\"\nowner = true\n",
        )
        .unwrap();
        let people = People::open(&path, "Charles").unwrap();
        assert_eq!(people.roles_summary().len(), 1);
    }

    #[test]
    fn deux_proprietaires_fichier_invalide() {
        let (_d, path, _p) = fresh();
        let cases = [
            // deux propriétaires
            "[[person]]\nname = \"A\"\nrole = \"admin\"\nowner = true\n[[person]]\nname = \"B\"\nrole = \"admin\"\nowner = true\n",
            // aucun
            "[[person]]\nname = \"A\"\nrole = \"admin\"\n",
            // propriétaire non admin
            "[[role]]\nname = \"r\"\n[[person]]\nname = \"A\"\nrole = \"r\"\nowner = true\n",
            // nom en double
            "[[person]]\nname = \"A\"\nrole = \"admin\"\nowner = true\n[[person]]\nname = \"a\"\nrole = \"admin\"\n",
        ];
        for text in cases {
            std::fs::write(&path, text).unwrap();
            assert!(People::open(&path, "Charles").is_err(), "{text}");
            assert!(
                People::open_or_degraded(&path, "Charles")
                    .degraded()
                    .is_some()
            );
        }
    }

    #[test]
    fn mode_degrade_refuse_les_ecritures() {
        let (_d, path, _p) = fresh();
        std::fs::write(&path, "[[person]]").unwrap();
        let people = People::open_or_degraded(&path, "Charles");
        assert!(matches!(
            people.add_person("Léa", "admin"),
            Err(PeopleError::Degraded)
        ));
        assert!(matches!(
            people.remove_person("Léa"),
            Err(PeopleError::Degraded)
        ));
        assert!(matches!(
            people.set_role("Léa", "admin"),
            Err(PeopleError::Degraded)
        ));
        assert!(matches!(
            people.attach("Charles", &ear("e")),
            Err(PeopleError::Degraded)
        ));
        assert!(matches!(
            people.add_role("x", vec![], false, false, &tools()),
            Err(PeopleError::Degraded)
        ));
        assert!(matches!(
            people.remove_role("x"),
            Err(PeopleError::Degraded)
        ));
        assert!(matches!(
            people.edit_role("x", None, None, None, &tools()),
            Err(PeopleError::Degraded)
        ));
    }

    #[test]
    fn prune_retire_les_oreilles_disparues() {
        let (_d, path, people) = fresh();
        with_lea(&people);
        people.attach("Léa", &ear("e1")).unwrap();
        people.attach("Léa", &ear("e2")).unwrap();
        people.attach("Charles", &ear("e3")).unwrap();
        people.prune(&[ear("e2"), ear("e3")]).unwrap();
        let again = People::open(&path, "Charles").unwrap();
        let list = again.people_summary(&|e| Some(format!("n-{e}")), false);
        let lea = list.iter().find(|p| p.name == "Léa").unwrap();
        assert_eq!(lea.ears, s(&["n-e2"]));
        assert_eq!(again.person_of(&ear("e1")), EarPerson::Owner);
    }

    #[test]
    fn edition_de_role_et_changement_de_role() {
        let (_d, _p, people) = fresh();
        with_lea(&people);
        people
            .edit_role(
                "famille",
                Some(s(&["media_play"])),
                Some(true),
                None,
                &tools(),
            )
            .unwrap();
        let lea = people.speaker("Léa", Trust::Accepted);
        assert_eq!(lea.rights.tools, s(&["media_play"]));
        assert!(lea.rights.confirm && !lea.rights.agent);
        people.set_role("Léa", "admin").unwrap();
        assert!(people.speaker("Léa", Trust::Accepted).rights.all);
        assert!(matches!(
            people.set_role("Léa", "nexiste"),
            Err(PeopleError::Unknown(_))
        ));
    }

    // --- droits ---

    fn media() -> Rights {
        Rights {
            all: false,
            tools: s(&["media_*", "app_launch"]),
            confirm: false,
            agent: false,
        }
    }

    #[test]
    fn admin_tout_sauf_ce_que_la_confiance_interdit() {
        for trust in [Trust::Recognized, Trust::Typed, Trust::Accepted] {
            let a = admin(trust);
            assert_eq!(a.check("media_play", Risk::Direct), Access::Allowed);
            assert_eq!(a.check("suspend", Risk::Risky), Access::Allowed);
            assert_eq!(a.check("outil_futur", Risk::Direct), Access::Allowed);
        }
    }

    #[test]
    fn admin_unverified_ni_risque_ni_agent() {
        let a = admin(Trust::Unverified);
        assert_eq!(a.check("media_play", Risk::Direct), Access::Allowed);
        assert_eq!(a.check("suspend", Risk::Risky), Access::Denied);
        assert_eq!(a.check("agent_start", Risk::Direct), Access::Denied);
    }

    #[test]
    fn admin_accepted_tout_sauf_l_agent() {
        let a = admin(Trust::Accepted);
        assert_eq!(a.check("suspend", Risk::Risky), Access::Allowed);
        assert_eq!(a.check("agent_start", Risk::Direct), Access::Denied);
        assert_eq!(a.check("agent_status", Risk::Direct), Access::Denied);
    }

    #[test]
    fn admin_recognized_et_typed_peuvent_l_agent() {
        for trust in [Trust::Recognized, Trust::Typed] {
            assert_eq!(
                admin(trust).check("agent_start", Risk::Direct),
                Access::Allowed
            );
        }
    }

    #[test]
    fn invite_media_seulement() {
        let g = guest(media(), Trust::Unverified);
        assert_eq!(g.check("media_next", Risk::Direct), Access::Allowed);
        assert_eq!(g.check("app_launch", Risk::Direct), Access::Allowed);
        assert_eq!(g.check("web_search", Risk::Direct), Access::Denied);
        assert_eq!(g.check("suspend", Risk::Risky), Access::Denied);
        assert_eq!(g.check("agent_start", Risk::Direct), Access::Denied);
        assert_eq!(g.check("outil_futur", Risk::Direct), Access::Denied);
    }

    #[test]
    fn motif_prefixe_media_etoile() {
        let g = guest(media(), Trust::Accepted);
        assert_eq!(g.check("media", Risk::Direct), Access::Denied);
        assert_eq!(g.check("media_", Risk::Direct), Access::Allowed);
        assert_eq!(g.check("xmedia_play", Risk::Direct), Access::Denied);
        let star = guest(
            Rights {
                tools: s(&["*"]),
                ..Rights::default()
            },
            Trust::Accepted,
        );
        assert_eq!(star.check("web_search", Risk::Direct), Access::Allowed);
        assert_eq!(star.check("agent_start", Risk::Direct), Access::Denied);
    }

    #[test]
    fn outil_risque_exige_confirm_et_confiance() {
        let mut r = Rights {
            tools: s(&["suspend"]),
            ..Rights::default()
        };
        assert_eq!(
            guest(r.clone(), Trust::Recognized).check("suspend", Risk::Risky),
            Access::Denied
        );
        r.confirm = true;
        assert_eq!(
            guest(r.clone(), Trust::Recognized).check("suspend", Risk::Risky),
            Access::Allowed
        );
        assert_eq!(
            guest(r, Trust::Unverified).check("suspend", Risk::Risky),
            Access::Denied
        );
    }

    #[test]
    fn agent_exige_droit_et_voix_reconnue() {
        let with = Rights {
            agent: true,
            ..Rights::default()
        };
        assert_eq!(
            guest(with.clone(), Trust::Recognized).check("agent_start", Risk::Direct),
            Access::Allowed
        );
        assert_eq!(
            guest(with.clone(), Trust::Typed).check("agent_stop", Risk::Direct),
            Access::Allowed
        );
        for t in [Trust::Accepted, Trust::Unverified] {
            assert_eq!(
                guest(with.clone(), t).check("agent_start", Risk::Direct),
                Access::Denied
            );
        }
        assert_eq!(
            guest(Rights::default(), Trust::Recognized).check("agent_start", Risk::Direct),
            Access::Denied
        );
    }

    #[test]
    fn may_confirm_exige_confiance_et_droit() {
        assert!(admin(Trust::Recognized).may_confirm("suspend", Risk::Risky));
        assert!(admin(Trust::Accepted).may_confirm("suspend", Risk::Risky));
        assert!(!admin(Trust::Typed).may_confirm("suspend", Risk::Risky));
        assert!(!admin(Trust::Unverified).may_confirm("suspend", Risk::Risky));
        let g = guest(media(), Trust::Recognized);
        assert!(!g.may_confirm("suspend", Risk::Risky));
        assert!(!g.may_confirm("media_play", Risk::Direct));
    }

    #[test]
    fn invite_avec_confirm_mais_unverified_ne_confirme_pas() {
        let r = Rights {
            tools: s(&["suspend"]),
            confirm: true,
            ..Rights::default()
        };
        assert!(guest(r.clone(), Trust::Recognized).may_confirm("suspend", Risk::Risky));
        assert!(!guest(r.clone(), Trust::Unverified).may_confirm("suspend", Risk::Risky));
        // Droit retiré (outil absent) : pas de confirmation non plus.
        let sans = Rights {
            confirm: true,
            ..Rights::default()
        };
        assert!(!guest(sans, Trust::Recognized).may_confirm("suspend", Risk::Risky));
    }
}
