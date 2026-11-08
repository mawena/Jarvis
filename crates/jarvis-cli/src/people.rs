//! `jarvis person …` et `jarvis role …` : tableaux et requêtes.

use clap::{Args, Subcommand};
use jarvis_protocol::{ControlRequest, PersonSummary, RoleSummary};

#[derive(Subcommand)]
pub enum PersonCommand {
    /// Liste les personnes, leur rôle et leurs oreilles
    List,
    /// Ajoute une personne
    Add {
        /// Nom de la personne (entre guillemets s'il a plusieurs mots)
        name: String,
        /// Rôle de la personne (voir `jarvis role list`)
        #[arg(long, required = true)]
        role: String,
    },
    /// Retire une personne (refusé pour le propriétaire, ou si elle a une oreille)
    Remove {
        /// Nom de la personne (entre guillemets s'il a plusieurs mots)
        name: String,
    },
    /// Change le rôle d'une personne : `jarvis person role Léa famille`
    Role {
        /// Nom de la personne (entre guillemets s'il a plusieurs mots)
        name: String,
        /// Nouveau rôle (un seul mot)
        role: String,
    },
    /// Rattache une oreille (nom ou identifiant) : `jarvis person attach Léa Pixel 4 XL`
    Attach {
        /// Nom de la personne (entre guillemets s'il a plusieurs mots)
        name: String,
        /// Oreille : nom (plusieurs mots permis), identifiant ou début d'identifiant
        #[arg(required = true, num_args = 1..)]
        ear: Vec<String>,
    },
}

#[derive(Args)]
#[group(required = true, multiple = true)]
pub struct RoleChanges {
    /// Outils autorisés, séparés par des virgules (`media_*,app_launch`)
    #[arg(long)]
    tools: Option<String>,
    /// Autoriser les actions risquées (veille, redémarrage…) et leur confirmation par « oui »
    #[arg(long, conflicts_with = "no_confirm")]
    confirm: bool,
    /// Ne plus exiger de confirmation
    #[arg(long)]
    no_confirm: bool,
    /// Autoriser l'agent (tâches longues)
    #[arg(long, conflicts_with = "no_agent")]
    agent: bool,
    /// Interdire l'agent
    #[arg(long)]
    no_agent: bool,
}

#[derive(Subcommand)]
pub enum RoleCommand {
    /// Liste les rôles
    List,
    /// Crée un rôle
    Add {
        /// Nom du rôle (un seul mot : lettres, chiffres, - et _)
        name: String,
        /// Outils autorisés, séparés par des virgules (`media_*,app_launch`)
        #[arg(long)]
        tools: Option<String>,
        /// Autoriser les actions risquées (veille, redémarrage…) et leur confirmation par « oui »
        #[arg(long)]
        confirm: bool,
        /// Autoriser l'agent (tâches longues)
        #[arg(long)]
        agent: bool,
    },
    /// Modifie un rôle (au moins une option ; les autres champs restent inchangés)
    Edit {
        /// Nom du rôle
        name: String,
        #[command(flatten)]
        changes: RoleChanges,
    },
    /// Supprime un rôle (refusé s'il est encore porté par quelqu'un)
    Remove {
        /// Nom du rôle
        name: String,
    },
}

/// « a, b,,c » -> [a, b, c].
fn split_tools(list: &str) -> Vec<String> {
    list.split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(String::from)
        .collect()
}

pub fn person_request(command: PersonCommand) -> ControlRequest {
    match command {
        PersonCommand::List => ControlRequest::PersonList,
        PersonCommand::Add { name, role } => ControlRequest::PersonAdd { name, role },
        PersonCommand::Remove { name } => ControlRequest::PersonRemove { name },
        PersonCommand::Role { name, role } => ControlRequest::PersonRole { name, role },
        PersonCommand::Attach { name, ear } => ControlRequest::PersonAttach {
            name,
            ear: ear.join(" "),
        },
    }
}

pub fn role_request(command: RoleCommand) -> ControlRequest {
    match command {
        RoleCommand::List => ControlRequest::RoleList,
        RoleCommand::Add {
            name,
            tools,
            confirm,
            agent,
        } => ControlRequest::RoleAdd {
            name,
            tools: tools.as_deref().map(split_tools).unwrap_or_default(),
            confirm,
            agent,
        },
        RoleCommand::Edit { name, changes } => ControlRequest::RoleEdit {
            name,
            tools: changes.tools.as_deref().map(split_tools),
            confirm: flag(changes.confirm, changes.no_confirm),
            agent: flag(changes.agent, changes.no_agent),
        },
        RoleCommand::Remove { name } => ControlRequest::RoleRemove { name },
    }
}

fn flag(on: bool, off: bool) -> Option<bool> {
    if on {
        Some(true)
    } else if off {
        Some(false)
    } else {
        None
    }
}

/// Tableau aligné comme `pair::ears_table` (deux espaces entre colonnes).
fn table(rows: Vec<Vec<String>>) -> String {
    let widths: Vec<usize> = (0..rows[0].len())
        .map(|c| rows.iter().map(|r| r[c].chars().count()).max().unwrap_or(0))
        .collect();
    rows.iter()
        .map(|r| {
            let mut line = String::new();
            for (c, cell) in r.iter().enumerate() {
                line.push_str(cell);
                line.push_str(&" ".repeat(widths[c] - cell.chars().count() + 2));
            }
            line.trim_end().to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Nom, Rôle, Oreilles, Empreinte vocale ; le propriétaire est suivi de « (propriétaire) ».
/// Mode dégradé : une ligne d'avertissement en tête.
pub fn people_table(people: &[PersonSummary], degraded: Option<&str>) -> String {
    let mut rows = vec![vec![
        "Nom".to_string(),
        "Rôle".into(),
        "Oreilles".into(),
        "Empreinte vocale".into(),
    ]];
    rows.extend(people.iter().map(|p| {
        vec![
            if p.owner {
                format!("{} (propriétaire)", p.name)
            } else {
                p.name.clone()
            },
            p.role.clone(),
            if p.ears.is_empty() {
                "-".into()
            } else {
                p.ears.join(", ")
            },
            if p.voiceprint { "oui" } else { "non" }.into(),
        ]
    }));
    let body = table(rows);
    match degraded {
        Some(reason) => format!(
            "Attention : annuaire illisible ({reason}), mode dégradé : seule votre voix reconnue et le texte tapé ont des droits.\n{body}"
        ),
        None => body,
    }
}

/// Nom, Outils, Confirmer, Agent, Personnes ; `admin` : « tout ».
pub fn roles_table(roles: &[RoleSummary]) -> String {
    let yes_no = |b: bool| if b { "oui" } else { "non" }.to_string();
    let mut rows = vec![vec![
        "Nom".to_string(),
        "Outils".into(),
        "Confirmer".into(),
        "Agent".into(),
        "Personnes".into(),
    ]];
    rows.extend(roles.iter().map(|r| {
        vec![
            r.name.clone(),
            if r.builtin && r.name == "admin" {
                "tout".into()
            } else if r.tools.is_empty() {
                "-".into()
            } else {
                r.tools.join(", ")
            },
            yes_no(r.confirm),
            yes_no(r.agent),
            r.people.to_string(),
        ]
    }));
    table(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn charles() -> PersonSummary {
        PersonSummary {
            name: "Charles".into(),
            role: "admin".into(),
            owner: true,
            ears: vec!["PC".into()],
            voiceprint: true,
        }
    }

    #[test]
    fn tableau_personnes_marque_le_proprietaire() {
        let lea = PersonSummary {
            name: "Léa".into(),
            role: "famille".into(),
            owner: false,
            ears: vec![],
            voiceprint: false,
        };
        let t = people_table(&[charles(), lea], None);
        let lines: Vec<&str> = t.lines().collect();
        assert!(lines[1].starts_with("Charles (propriétaire)"));
        assert!(!lines[2].contains("propriétaire"));
        let d = people_table(&[charles()], Some("fichier corrompu"));
        assert!(d.lines().next().unwrap().contains("fichier corrompu"));
        assert!(d.lines().next().unwrap().contains("dégradé"));
    }

    #[test]
    fn outils_decoupes_par_virgule() {
        assert_eq!(
            split_tools("media_*, app_launch,,x"),
            ["media_*", "app_launch", "x"]
        );
    }
}
