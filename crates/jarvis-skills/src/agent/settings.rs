//! Réglages de la section `[agent]`.

use serde::Deserialize;
use std::path::{Path, PathBuf};

/// Outils permis à Claude Code par défaut (`--allowedTools` et `permissions.allow`).
/// Lecture et écriture limitées au worktree (`./**`, relatif au dossier de
/// travail) ; l'outil `Grep` remplace `rg` et `git grep` (`--pre`, `-O`).
pub const DEFAULT_ALLOWED_TOOLS: &[&str] = &[
    "Read(./**)",
    "Edit(./**)",
    "Glob",
    "Grep",
    "Bash(cargo build*)",
    "Bash(cargo check*)",
    "Bash(cargo test*)",
    "Bash(cargo clippy*)",
    "Bash(cargo fmt*)",
    "Bash(cargo doc*)",
    "Bash(cargo tree*)",
    "Bash(scripts/check.sh*)",
    "Bash(./scripts/check.sh*)",
    "Bash(git status*)",
    "Bash(git diff*)",
    "Bash(git log*)",
    "Bash(git show*)",
    "Bash(git blame*)",
    "Bash(git ls-files*)",
    "Bash(ls*)",
    "Bash(grep *)",
    "Bash(head *)",
    "Bash(tail *)",
    "Bash(wc *)",
    "Bash(mkdir *)",
];

/// Outils toujours interdits (`--disallowedTools`), prioritaires sur les permis.
/// `cargo run` : dans le projet Jarvis, il ouvrirait le micro.
pub const DENIED_TOOLS: &[&str] = &[
    "Bash(git push*)",
    "Bash(git merge*)",
    "Bash(git rebase*)",
    "Bash(git reset*)",
    "Bash(git checkout*)",
    "Bash(git switch*)",
    "Bash(git branch*)",
    "Bash(git commit*)",
    "Bash(git worktree*)",
    "Bash(git remote*)",
    "Bash(git config*)",
    "Bash(git flow*)",
    "Bash(git tag*)",
    "Bash(gh*)",
    "Bash(cargo run*)",
    "Bash(cargo publish*)",
    "Bash(cargo install*)",
    "Bash(cargo update*)",
    "Bash(cargo login*)",
    "WebFetch",
    "WebSearch",
    // Secrets de Charles : jamais lus (bloque aussi l'écriture).
    "Read(~/.ssh/**)",
    "Read(~/.gnupg/**)",
    "Read(~/.config/jarvis/**)",
    "Read(~/.config/gh/**)",
    "Read(~/.git-credentials)",
    "Read(~/.aws/**)",
    "Read(~/.claude/.credentials.json)",
    // Configuration qui ferait tourner du code hors du bac à sable.
    "Edit(./.git)",
    "Edit(./.git/**)",
    "Edit(./.claude/**)",
    "Edit(./.cargo/**)",
    "Edit(./.githooks/**)",
    "Edit(./.husky/**)",
];

/// Réglages de l'agent de code. Les chemins peuvent commencer par `~/` :
/// [`AgentSettings::resolved`] les développe.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct AgentSettings {
    pub enabled: bool,
    pub claude_path: PathBuf,
    /// Durée maximale d'une tâche.
    pub max_minutes: u64,
    /// Sans aucune sortie pendant ce temps : tâche bloquée, arrêtée.
    pub idle_minutes: u64,
    /// Tâches en même temps ; une seule par projet.
    pub max_tasks: usize,
    /// Vide : modèle par défaut de Claude Code.
    pub model: String,
    /// 0 : sans plafond.
    pub max_budget_usd: f64,
    /// Faux : aucune `ANTHROPIC_API_KEY`, l'agent utilise le compte de Charles.
    pub pass_api_key: bool,
    /// Variable du cerveau qui contient la clé, passée en `ANTHROPIC_API_KEY`
    /// quand `pass_api_key` est vrai.
    pub api_key_env: String,
    /// Un commit de Jarvis à la fin d'une tâche réussie.
    pub commit_at_end: bool,
    pub worktree_dir: PathBuf,
    /// Dossier de compilation de l'agent, suivi de `/<projet>`.
    pub cargo_target_dir: PathBuf,
    /// `CARGO_HOME` de l'agent : jamais le `~/.cargo` de Charles.
    pub cargo_home: PathBuf,
    /// Placés en tête du `PATH` de l'agent.
    pub extra_path: Vec<PathBuf>,
    /// Ajoutés à [`DEFAULT_ALLOWED_TOOLS`] ; [`DENIED_TOOLS`] reste prioritaire.
    pub extra_allowed_tools: Vec<String>,
    pub projects: Vec<Project>,
}

/// Projet connu (`[[agent.projects]]`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Project {
    pub name: String,
    pub path: PathBuf,
    #[serde(default)]
    pub aliases: Vec<String>,
    /// Branche de base ; sinon `gitflow.branch.develop`, sinon `develop`.
    #[serde(default)]
    pub base: Option<String>,
    /// Outils permis en plus pour ce projet.
    #[serde(default)]
    pub extra_allowed_tools: Vec<String>,
}

impl Default for AgentSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            claude_path: PathBuf::from("~/.local/bin/claude"),
            max_minutes: 45,
            // Au-dessus des 25 min permises à une commande Bash de l'agent.
            idle_minutes: 30,
            max_tasks: 2,
            model: String::new(),
            max_budget_usd: 0.0,
            pass_api_key: false,
            api_key_env: "ANTHROPIC_API_KEY".into(),
            commit_at_end: true,
            worktree_dir: PathBuf::from("~/.local/share/jarvis/worktrees"),
            cargo_target_dir: PathBuf::from("~/.cache/jarvis/agent-target"),
            cargo_home: PathBuf::from("~/.cache/jarvis/agent-cargo"),
            extra_path: vec![PathBuf::from("~/.cargo/bin"), PathBuf::from("~/.local/bin")],
            extra_allowed_tools: Vec::new(),
            projects: vec![Project {
                name: "Jarvis".into(),
                path: PathBuf::from("~/Work/Mawena/Projects/Jarvis"),
                aliases: vec!["l'assistant".into()],
                base: None,
                extra_allowed_tools: Vec::new(),
            }],
        }
    }
}

impl AgentSettings {
    /// Copie avec les `~/` développés (`$HOME`).
    pub fn resolved(&self) -> AgentSettings {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let home = home.as_deref();
        let mut s = self.clone();
        s.claude_path = expand(&s.claude_path, home);
        s.worktree_dir = expand(&s.worktree_dir, home);
        s.cargo_target_dir = expand(&s.cargo_target_dir, home);
        s.cargo_home = expand(&s.cargo_home, home);
        for p in &mut s.extra_path {
            *p = expand(p, home);
        }
        for p in &mut s.projects {
            p.path = expand(&p.path, home);
        }
        s
    }

    /// Outils permis pour `project` : par défaut, puis ceux des réglages et du
    /// projet, sans doublon.
    pub fn allowed_tools(&self, project: &Project) -> Vec<String> {
        let mut tools: Vec<String> = Vec::new();
        let all = DEFAULT_ALLOWED_TOOLS
            .iter()
            .map(|t| t.to_string())
            .chain(self.extra_allowed_tools.iter().cloned())
            .chain(project.extra_allowed_tools.iter().cloned());
        for t in all {
            if !tools.contains(&t) {
                tools.push(t);
            }
        }
        tools
    }
}

/// Remplace un `~` en tête de chemin par `home`.
fn expand(p: &Path, home: Option<&Path>) -> PathBuf {
    match (p.strip_prefix("~"), home) {
        (Ok(rest), Some(home)) => home.join(rest),
        _ => p.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tilde_developpe() {
        let home = Path::new("/home/c");
        assert_eq!(
            expand(Path::new("~/.local/bin/claude"), Some(home)),
            PathBuf::from("/home/c/.local/bin/claude")
        );
        assert_eq!(expand(Path::new("~"), Some(home)), PathBuf::from("/home/c"));
        assert_eq!(
            expand(Path::new("/opt/x"), Some(home)),
            PathBuf::from("/opt/x")
        );
        assert_eq!(expand(Path::new("~/x"), None), PathBuf::from("~/x"));
        assert_eq!(
            expand(Path::new("~autre/x"), Some(home)),
            PathBuf::from("~autre/x")
        );
    }

    #[test]
    fn resolu_sans_tilde() {
        let s = AgentSettings::default().resolved();
        if std::env::var_os("HOME").is_some() {
            assert!(s.claude_path.is_absolute());
            assert!(s.worktree_dir.ends_with(".local/share/jarvis/worktrees"));
            assert!(s.cargo_home.is_absolute());
            assert!(s.cargo_home.ends_with(".cache/jarvis/agent-cargo"));
            assert!(s.extra_path.iter().all(|p| p.is_absolute()));
            assert!(s.projects[0].path.is_absolute());
        }
    }

    #[test]
    fn outils_permis_sans_doublon() {
        let s = AgentSettings {
            extra_allowed_tools: vec!["Bash(npm test*)".into(), "Read(./**)".into()],
            ..AgentSettings::default()
        };
        let mut p = s.projects[0].clone();
        p.extra_allowed_tools = vec!["Bash(make*)".into(), "Bash(npm test*)".into()];
        let tools = s.allowed_tools(&p);
        assert_eq!(tools.len(), DEFAULT_ALLOWED_TOOLS.len() + 2);
        assert_eq!(
            &tools[tools.len() - 2..],
            ["Bash(npm test*)", "Bash(make*)"]
        );
    }

    #[test]
    fn interdits_couvrent_push_et_commit() {
        for t in ["Bash(git push*)", "Bash(git commit*)", "Bash(cargo run*)"] {
            assert!(DENIED_TOOLS.contains(&t), "{t}");
        }
        assert!(
            DEFAULT_ALLOWED_TOOLS
                .iter()
                .all(|t| !DENIED_TOOLS.contains(t))
        );
    }

    #[test]
    fn permis_limites_au_worktree() {
        // Jamais `Read`, `Edit` ou `Write` nus : ils couvriraient tout le disque.
        for t in [
            "Read",
            "Edit",
            "Write",
            "MultiEdit",
            "Bash(rg *)",
            "Bash(git grep*)",
        ] {
            assert!(!DEFAULT_ALLOWED_TOOLS.contains(&t), "{t}");
        }
        assert!(DEFAULT_ALLOWED_TOOLS.contains(&"Read(./**)"));
        assert!(DEFAULT_ALLOWED_TOOLS.contains(&"Edit(./**)"));
        for t in [
            "Read(~/.ssh/**)",
            "Read(~/.claude/.credentials.json)",
            "Edit(./.git/**)",
            "Edit(./.claude/**)",
            "Edit(./.cargo/**)",
        ] {
            assert!(DENIED_TOOLS.contains(&t), "{t}");
        }
    }

    #[test]
    fn silence_plus_long_qu_une_commande() {
        let s = AgentSettings::default();
        assert_eq!((s.max_minutes, s.idle_minutes), (45, 30));
        // Une commande Bash de l'agent dure 25 min au plus.
        assert!(s.idle_minutes > 25);
    }

    #[test]
    fn section_toml_lue() {
        let s: AgentSettings = toml::from_str(
            "max_tasks = 1\n[[projects]]\nname = \"Essai\"\npath = \"/tmp/essai\"\n",
        )
        .unwrap();
        assert_eq!(s.max_tasks, 1);
        assert_eq!(s.max_minutes, 45);
        assert_eq!(s.idle_minutes, 30);
        assert_eq!(s.api_key_env, "ANTHROPIC_API_KEY");
        assert_eq!(s.projects.len(), 1);
        assert_eq!(s.projects[0].base, None);
        assert!(s.projects[0].aliases.is_empty());
    }
}
