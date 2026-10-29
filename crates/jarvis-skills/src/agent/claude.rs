//! Ligne de commande de Claude Code et lecture de son flux `stream-json`.

use super::settings::{AgentSettings, DENIED_TOOLS, Project};
use super::types::{Progress, RunResult, TaskRecord};
use crate::text::fold;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// Tout ce qu'il faut pour lancer l'agent.
#[derive(Debug, Clone, PartialEq)]
pub struct Invocation {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    /// Environnement complet du processus : rien d'autre n'est hérité.
    pub env: Vec<(String, String)>,
    /// Dossiers créés avant le lancement (écriture permise par le bac à sable).
    pub create_dirs: Vec<PathBuf>,
    /// Consigne de Charles, écrite sur l'entrée standard puis fermée.
    pub stdin: String,
}

/// Outils intégrés offerts à l'agent (`--tools`) ; aucun autre n'existe pour lui.
pub const TOOLS: &str = "Bash,Read,Edit,Write,Glob,Grep";

/// Aucun serveur MCP (`--mcp-config` avec `--strict-mcp-config`).
const NO_MCP: &str = r#"{"mcpServers":{}}"#;

/// Secrets jamais lisibles depuis le bac à sable.
const SANDBOX_DENY_READ: &[&str] = &[
    "~/.ssh",
    "~/.gnupg",
    "~/.config/jarvis",
    "~/.config/gh",
    "~/.git-credentials",
    "~/.aws",
    "~/.docker",
    "~/.local/share/keyrings",
    "~/.claude/.credentials.json",
];

/// Dossiers du worktree en lecture seule dans le bac à sable : leur contenu
/// ferait tourner du code hors du bac à sable (crochets git, réglages).
const SANDBOX_DENY_WRITE: &[&str] = &[".git", ".claude", ".cargo", ".githooks", ".husky"];

/// Seuls domaines joignables depuis le bac à sable : le registre de cargo.
const ALLOWED_DOMAINS: &[&str] = &["crates.io", "index.crates.io", "static.crates.io"];

/// Variables héritées du cerveau (plus `LC_*`). Jamais `XDG_RUNTIME_DIR`,
/// `DBUS_*`, `SSH_AUTH_SOCK`, `DISPLAY`, `WAYLAND_DISPLAY`, ni aucun identifiant.
const INHERITED_ENV: &[&str] = &[
    "HOME",
    "USER",
    "LOGNAME",
    "LANG",
    "LANGUAGE",
    "TERM",
    "TZ",
    "SHELL",
    "TMPDIR",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_CACHE_HOME",
    "XDG_STATE_HOME",
];

/// Variables git qui désigneraient un autre dépôt : jamais posées.
const GIT_REDIRECTS: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_COMMON_DIR",
    "GIT_NAMESPACE",
    "GIT_CEILING_DIRECTORIES",
    "GIT_EXEC_PATH",
    "GIT_ASKPASS",
    "GIT_TEMPLATE_DIR",
];

/// Délai par défaut d'une commande Bash de l'agent : 15 min.
pub const BASH_DEFAULT_TIMEOUT_MS: u64 = 15 * 60 * 1000;
/// Délai maximal d'une commande Bash de l'agent : 25 min (sous `idle_minutes`).
pub const BASH_MAX_TIMEOUT_MS: u64 = 25 * 60 * 1000;

/// Ligne de commande de l'agent pour `task` (voir « Décisions » du plan de la
/// phase 7). `git_set` vient de `git::no_push_env` ; `git_remove` est sans
/// effet : l'environnement part vide.
pub fn build(
    settings: &AgentSettings,
    project: &Project,
    task: &TaskRecord,
    git_set: Vec<(String, String)>,
    git_remove: Vec<String>,
) -> Invocation {
    let _ = git_remove;
    let inherited: Vec<(String, String)> = std::env::vars_os()
        .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)))
        .collect();
    build_with_env(settings, project, task, git_set, &inherited)
}

/// [`build`] avec l'environnement du cerveau donné (tests).
fn build_with_env(
    settings: &AgentSettings,
    project: &Project,
    task: &TaskRecord,
    git_set: Vec<(String, String)>,
    inherited: &[(String, String)],
) -> Invocation {
    // Les interdits sont prioritaires chez Claude Code ; on les retire quand même.
    let allowed: Vec<String> = settings
        .allowed_tools(project)
        .into_iter()
        .filter(|t| !DENIED_TOOLS.contains(&t.as_str()))
        .collect();
    let slug = task.branch.rsplit('/').next().unwrap_or(&task.branch);
    let slug = slug.strip_prefix("jarvis-").unwrap_or(slug);
    let target_dir = settings.cargo_target_dir.join(&project.name);

    let mut args: Vec<String> = [
        "-p",
        "--output-format",
        "stream-json",
        "--verbose",
        "--permission-mode",
        "dontAsk",
        "--permission-prompts",
        "none",
        // Ni réglages utilisateur, projet ou local ; outils de fichiers
        // confinés au worktree ; écritures sensibles jamais approuvées.
        "--restricted",
        "--strict-mcp-config",
    ]
    .iter()
    .map(|a| a.to_string())
    .collect();
    // Forme `--option=valeur` : ces options acceptent plusieurs valeurs.
    args.push(format!("--tools={TOOLS}"));
    args.push(format!("--mcp-config={NO_MCP}"));
    args.push("--settings".into());
    args.push(
        agent_settings_json(&allowed, &target_dir, &settings.cargo_home, &task.worktree)
            .to_string(),
    );
    args.push(format!("--allowedTools={}", allowed.join(",")));
    args.push(format!("--disallowedTools={}", DENIED_TOOLS.join(",")));
    args.push("--append-system-prompt".into());
    args.push(system_prompt(&task.branch));
    args.push("--session-id".into());
    args.push(task.id.to_string());
    args.push("--name".into());
    args.push(format!("jarvis {} {slug}", project.name));
    if !settings.model.trim().is_empty() {
        args.push("--model".into());
        args.push(settings.model.trim().to_string());
    }
    if settings.max_budget_usd > 0.0 {
        args.push("--max-budget-usd".into());
        args.push(settings.max_budget_usd.to_string());
    }

    Invocation {
        program: settings.claude_path.clone(),
        args,
        cwd: task.worktree.clone(),
        env: agent_env(settings, git_set, inherited, &target_dir),
        create_dirs: vec![target_dir, settings.cargo_home.clone()],
        stdin: task.instruction.clone(),
    }
}

/// Réglages passés par `--settings` : aucune attribution, aucun connecteur,
/// bac à sable obligatoire pour Bash, permissions limitées au worktree.
fn agent_settings_json(
    allowed: &[String],
    target_dir: &Path,
    cargo_home: &Path,
    worktree: &Path,
) -> Value {
    let path = |p: &Path| p.to_string_lossy().into_owned();
    let deny_write: Vec<String> = SANDBOX_DENY_WRITE
        .iter()
        .map(|d| path(&worktree.join(d)))
        .collect();
    json!({
        "includeCoAuthoredBy": false,
        "attribution": {"commit": "", "pr": ""},
        "disableClaudeAiConnectors": true,
        "strictMcpConfig": true,
        "permissions": {
            "allow": allowed,
            "deny": DENIED_TOOLS,
        },
        "sandbox": {
            "enabled": true,
            // Bac à sable indisponible : Claude Code s'arrête au lieu de lancer
            // les commandes sans lui.
            "failIfUnavailable": true,
            "allowUnsandboxedCommands": false,
            // La liste des commandes permises reste une couche de plus.
            "autoAllowBashIfSandboxed": false,
            "excludedCommands": [],
            "filesystem": {
                "allowWrite": [path(target_dir), path(cargo_home)],
                "denyWrite": deny_write,
                "denyRead": SANDBOX_DENY_READ,
            },
            "network": {
                "allowedDomains": ALLOWED_DOMAINS,
                "strictAllowlist": true,
                "allowLocalBinding": false,
                "allowUnixSockets": [],
                "allowAllUnixSockets": false,
            },
        },
    })
}

/// Environnement complet de l'agent : liste fermée de variables du cerveau,
/// environnement git de Jarvis, chemins de cargo, délais de Bash, et la clé
/// d'API seulement si `pass_api_key`.
fn agent_env(
    settings: &AgentSettings,
    git_set: Vec<(String, String)>,
    inherited: &[(String, String)],
    target_dir: &Path,
) -> Vec<(String, String)> {
    let lookup = |key: &str| {
        inherited
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
    };
    let mut env: Vec<(String, String)> = inherited
        .iter()
        .filter(|(k, _)| INHERITED_ENV.contains(&k.as_str()) || k.starts_with("LC_"))
        .cloned()
        .collect();
    env.extend(
        git_set
            .into_iter()
            .filter(|(k, _)| k.starts_with("GIT_") && !GIT_REDIRECTS.contains(&k.as_str())),
    );
    for (key, value) in [("GIT_SSH_COMMAND", "false"), ("GIT_TERMINAL_PROMPT", "0")] {
        if !env.iter().any(|(k, _)| k == key) {
            env.push((key.into(), value.into()));
        }
    }
    let path = |p: &Path| p.to_string_lossy().into_owned();
    env.push((
        "PATH".into(),
        agent_path(&settings.extra_path, lookup("PATH")),
    ));
    env.push(("CARGO_TARGET_DIR".into(), path(target_dir)));
    env.push(("CARGO_HOME".into(), path(&settings.cargo_home)));
    // Les mandataires de rustup (`~/.cargo/bin/cargo`) y trouvent les chaînes.
    let rustup = lookup("RUSTUP_HOME")
        .map(PathBuf::from)
        .or_else(|| lookup("HOME").map(|h| Path::new(&h).join(".rustup")))
        .filter(|p| p.is_dir());
    if let Some(rustup) = rustup {
        env.push(("RUSTUP_HOME".into(), path(&rustup)));
    }
    env.push((
        "BASH_DEFAULT_TIMEOUT_MS".into(),
        BASH_DEFAULT_TIMEOUT_MS.to_string(),
    ));
    env.push((
        "BASH_MAX_TIMEOUT_MS".into(),
        BASH_MAX_TIMEOUT_MS.to_string(),
    ));
    if settings.pass_api_key {
        let name = settings.api_key_env.trim();
        let name = if name.is_empty() {
            "ANTHROPIC_API_KEY"
        } else {
            name
        };
        if let Some(key) = lookup(name).filter(|k| !k.is_empty()) {
            env.push(("ANTHROPIC_API_KEY".into(), key));
        }
    }
    env
}

/// `extra_path` puis le `PATH` hérité : le service systemd n'a pas `~/.cargo/bin`.
fn agent_path(extra: &[PathBuf], inherited: Option<String>) -> String {
    let mut parts: Vec<String> = extra
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    if let Some(inherited) = inherited.filter(|p| !p.is_empty()) {
        parts.push(inherited);
    }
    parts.join(":")
}

/// Consignes ajoutées à celles de Claude Code. La dernière ligne demande le
/// résumé lu à voix haute à la fin.
pub fn system_prompt(branch: &str) -> String {
    format!(
        "Tu es lancé par Jarvis, l'assistant vocal de Charles, et personne ne peut te répondre : \
ne pose aucune question ; si un point est flou, fais le choix le plus raisonnable et dis ce que tu as supposé.\n\
La consigne vient d'une transcription vocale : des mots peuvent être mal compris.\n\
Tu travailles dans un worktree git à part, sur la branche « {branch} ».\n\
Interdit : pousser, fusionner, changer de branche, committer (Jarvis enregistre le travail à la fin), \
modifier la configuration git, toucher à quoi que ce soit hors de ce dossier, \
lancer les binaires du projet (cargo run) ou les tests marqués #[ignore].\n\
Tes commandes tournent dans un bac à sable : écriture limitée à ce dossier et au dossier de compilation, \
réseau limité à crates.io. Si une commande échoue à cause de lui, ne cherche pas à le contourner : dis-le.\n\
Lance les tests avant de finir.\n\
Termine par une seule ligne : RÉSUMÉ : <une phrase en français, 25 mots au plus, lisible à voix haute, sans markdown>\n"
    )
}

/// Lit le flux `stream-json` ligne à ligne. Ligne invalide ou inconnue : ignorée.
#[derive(Debug, Default)]
pub struct StreamParser {
    progress: Progress,
    session_id: Option<String>,
}

/// Longueur gardée du dernier texte de l'agent.
const LAST_TEXT_MAX: usize = 500;

impl StreamParser {
    pub fn feed(&mut self, line: &str) {
        let line = line.trim();
        if line.is_empty() {
            return;
        }
        self.progress.lines += 1;
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            return;
        };
        match v["type"].as_str() {
            Some("system") if v["subtype"] == "init" => {
                if let Some(id) = v["session_id"].as_str() {
                    self.session_id = Some(id.to_string());
                }
            }
            Some("assistant") => {
                let Some(content) = v["message"]["content"].as_array() else {
                    return;
                };
                for item in content {
                    match item["type"].as_str() {
                        Some("text") => {
                            let text = item["text"].as_str().unwrap_or("").trim();
                            if !text.is_empty() {
                                self.progress.last_text = Some(bounded(text, LAST_TEXT_MAX));
                            }
                        }
                        Some("tool_use") => {
                            if let Some(name) = item["name"].as_str() {
                                self.progress.last_action =
                                    Some(describe_tool(name, &item["input"]));
                            }
                        }
                        _ => {}
                    }
                }
            }
            Some("result") => {
                self.progress.result = Some(RunResult {
                    is_error: v["is_error"].as_bool().unwrap_or(false),
                    subtype: v["subtype"].as_str().unwrap_or("").to_string(),
                    text: v["result"].as_str().unwrap_or("").to_string(),
                    cost_usd: v["total_cost_usd"].as_f64(),
                    num_turns: v["num_turns"].as_u64().and_then(|n| u32::try_from(n).ok()),
                });
            }
            _ => {}
        }
    }

    pub fn progress(&self) -> &Progress {
        &self.progress
    }

    /// Id de session annoncé par la ligne `system/init`.
    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }
}

/// Action d'un outil, prête à être dite : « lance « cargo test » ».
pub fn describe_tool(name: &str, input: &Value) -> String {
    let file = || {
        input["file_path"]
            .as_str()
            .or_else(|| input["notebook_path"].as_str())
            .and_then(|p| Path::new(p).file_name())
            .map(|f| f.to_string_lossy().into_owned())
    };
    match name {
        "Bash" => match input["command"].as_str().map(spoken_command) {
            Some(cmd) if !cmd.is_empty() => format!("lance « {cmd} »"),
            _ => "lance une commande".into(),
        },
        "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => match file() {
            Some(f) => format!("modifie {f}"),
            None => "modifie un fichier".into(),
        },
        "Read" => match file() {
            Some(f) => format!("lit {f}"),
            None => "lit un fichier".into(),
        },
        "Grep" | "Glob" => "cherche dans le code".into(),
        // TodoWrite, outils MCP… : jamais de nom brut à voix haute.
        _ => "travaille".into(),
    }
}

/// Commande courte, lisible à voix haute : sans `cd … &&` en tête, sans
/// variables posées devant, chemins absolus réduits à leur dernier nom,
/// 4 mots et 60 caractères au plus.
fn spoken_command(cmd: &str) -> String {
    let mut rest = cmd.trim();
    // `cd dossier && …` ou `cd dossier; …`, éventuellement plusieurs fois.
    while let Some(after) = rest.strip_prefix("cd ") {
        let cut = [
            after.find("&&").map(|i| (i, 2)),
            after.find(';').map(|i| (i, 1)),
        ]
        .into_iter()
        .flatten()
        .min();
        match cut {
            Some((i, len)) => rest = after[i + len..].trim_start(),
            None => return String::new(),
        }
    }
    let words: Vec<String> = rest
        .split_whitespace()
        .skip_while(|w| is_assignment(w))
        .take(4)
        .map(|w| {
            let bare = w.trim_matches(|c| c == '"' || c == '\'');
            if bare.starts_with('/') || bare.starts_with('~') {
                let name = bare.trim_end_matches('/').rsplit('/').next().unwrap_or("");
                if name.is_empty() || name == "~" {
                    "…".into()
                } else {
                    name.to_string()
                }
            } else {
                w.to_string()
            }
        })
        .collect();
    bounded(&words.join(" "), 60)
}

/// `NOM=valeur` devant une commande.
fn is_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty()
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            && !name.starts_with(|c: char| c.is_ascii_digit())
    })
}

/// Longueur maximale d'un résumé.
const SUMMARY_MAX: usize = 200;

/// Résumé de fin : la ligne `RÉSUMÉ :` de l'agent, sinon la dernière phrase du
/// résultat. Sans markdown, 200 caractères au plus.
pub fn summary(result_text: &str) -> Option<String> {
    let labelled = result_text
        .lines()
        .rev()
        .filter_map(after_summary_label)
        .map(clean_markdown)
        .find(|s| !s.is_empty());
    if let Some(s) = labelled {
        return Some(bounded(&s, SUMMARY_MAX));
    }
    let last_line = result_text
        .lines()
        .filter(|l| !l.trim_start().starts_with("```") && after_summary_label(l).is_none())
        .map(clean_markdown)
        .rfind(|l| !l.is_empty())?;
    let sentence = last_sentence(&last_line);
    Some(bounded(sentence, SUMMARY_MAX))
}

/// Texte après « RÉSUMÉ : » (casse, accents et markdown indifférents).
fn after_summary_label(line: &str) -> Option<&str> {
    let line = line.trim_start_matches(|c: char| c.is_whitespace() || "*_#>-`".contains(c));
    let (head, rest) = line.split_once(':')?;
    (fold(head) == "resume").then_some(rest)
}

/// Retire le markdown courant : gras, italique, code, titres, listes, citations.
fn clean_markdown(text: &str) -> String {
    let text = text.trim_start_matches(|c: char| c.is_whitespace() || "#>-".contains(c));
    let text = text
        .replace("**", "")
        .replace("__", "")
        .replace(['*', '`'], "");
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Dernière phrase d'une ligne.
fn last_sentence(line: &str) -> &str {
    let mut start = 0;
    let mut last = line;
    let mut chars = line.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        let ends = matches!(c, '.' | '!' | '?' | '…');
        let next_is_space = chars.peek().is_some_and(|(_, n)| n.is_whitespace());
        if ends && next_is_space {
            let end = i + c.len_utf8();
            let sentence = line[start..end].trim();
            if !sentence.is_empty() {
                last = sentence;
            }
            start = end;
        }
    }
    let rest = line[start..].trim();
    if rest.is_empty() { last.trim() } else { rest }
}

/// Coupe `text` à `max` caractères, de préférence entre deux mots.
fn bounded(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let cut: String = text.chars().take(max).collect();
    let cut = match cut.rfind(char::is_whitespace) {
        Some(i) if i > 0 => &cut[..i],
        _ => cut.as_str(),
    };
    cut.trim_end_matches(|c: char| c.is_whitespace() || ",;:".contains(c))
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::DEFAULT_ALLOWED_TOOLS;
    use crate::agent::testing::{init_line, result_line, text_line, tool_use_line};
    use crate::agent::types::{TaskId, TaskState};
    use serde_json::json;

    fn task() -> TaskRecord {
        TaskRecord {
            id: TaskId("0b6e2f3c-1d2e-4f5a-8b9c-0d1e2f3a4b5c".into()),
            project: "Jarvis".into(),
            instruction: "Finis les tests du projet Jarvis".into(),
            branch: "feature/jarvis-finis-tests".into(),
            worktree: PathBuf::from("/tmp/wt/jarvis-finis-tests"),
            base: "develop".into(),
            base_sha: "0123".into(),
            started_at: 10,
            ended_at: None,
            state: TaskState::Running,
            summary: None,
            files_changed: None,
            violations: vec![],
            cost_usd: None,
            log: PathBuf::from("/tmp/x.log"),
        }
    }

    fn settings() -> AgentSettings {
        AgentSettings {
            claude_path: PathBuf::from("/opt/claude"),
            cargo_target_dir: PathBuf::from("/cache/target"),
            cargo_home: PathBuf::from("/cache/agent-cargo"),
            extra_path: vec![
                PathBuf::from("/h/.cargo/bin"),
                PathBuf::from("/h/.local/bin"),
            ],
            ..AgentSettings::default()
        }
    }

    /// Environnement du cerveau, avec tout ce qui ne doit jamais passer.
    fn brain_env() -> Vec<(String, String)> {
        [
            ("HOME", "/h"),
            ("USER", "charles"),
            ("LANG", "fr_FR.UTF-8"),
            ("LC_TIME", "fr_FR.UTF-8"),
            ("PATH", "/usr/bin:/bin"),
            ("TMPDIR", "/tmp/c"),
            ("XDG_CONFIG_HOME", "/h/.config"),
            ("XDG_RUNTIME_DIR", "/run/user/1000"),
            ("SSH_AUTH_SOCK", "/run/user/1000/ssh"),
            ("DBUS_SESSION_BUS_ADDRESS", "unix:path=/run/user/1000/bus"),
            ("DISPLAY", ":0"),
            ("WAYLAND_DISPLAY", "wayland-0"),
            ("ANTHROPIC_API_KEY", "sk-ant-cerveau"),
            ("JARVIS_CLE", "sk-ant-jarvis"),
            ("ANTHROPIC_AUTH_TOKEN", "jeton"),
            ("ANTHROPIC_BASE_URL", "https://ailleurs.invalid"),
            ("CLAUDE_CODE_OAUTH_TOKEN", "oauth"),
            ("CLAUDE_CODE_USE_BEDROCK", "1"),
            ("CLAUDE_CODE_USE_VERTEX", "1"),
            ("AWS_SECRET_ACCESS_KEY", "aws"),
            ("GH_TOKEN", "gh"),
            ("GITHUB_TOKEN", "gh"),
            ("GIT_DIR", "/h/Work/Jarvis/.git"),
            ("GIT_WORK_TREE", "/h/Work/Jarvis"),
            ("GIT_INDEX_FILE", "/h/Work/Jarvis/.git/index"),
            ("CLAUDECODE", "1"),
            ("CARGO_HOME", "/h/.cargo"),
        ]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
    }

    fn git_set() -> Vec<(String, String)> {
        vec![
            ("GIT_CONFIG_COUNT".to_string(), "1".to_string()),
            (
                "GIT_CONFIG_KEY_0".to_string(),
                "credential.helper".to_string(),
            ),
            ("GIT_CONFIG_VALUE_0".to_string(), String::new()),
            ("GIT_SSH_COMMAND".to_string(), "false".to_string()),
        ]
    }

    fn invocation(s: &AgentSettings) -> Invocation {
        build_with_env(s, &s.projects[0], &task(), git_set(), &brain_env())
    }

    fn value<'a>(inv: &'a Invocation, flag: &str) -> Option<&'a str> {
        let i = inv.args.iter().position(|a| a == flag)?;
        inv.args.get(i + 1).map(String::as_str)
    }

    fn eq_value<'a>(inv: &'a Invocation, flag: &str) -> Option<&'a str> {
        let prefix = format!("{flag}=");
        inv.args
            .iter()
            .find_map(|a| a.strip_prefix(prefix.as_str()))
    }

    fn env<'a>(inv: &'a Invocation, key: &str) -> Option<&'a str> {
        inv.env
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    fn settings_json(inv: &Invocation) -> Value {
        serde_json::from_str(value(inv, "--settings").unwrap()).unwrap()
    }

    fn strings(v: &Value) -> Vec<&str> {
        v.as_array()
            .unwrap()
            .iter()
            .map(|s| s.as_str().unwrap())
            .collect()
    }

    #[test]
    fn arguments_de_lancement() {
        let s = settings();
        let inv = invocation(&s);
        assert_eq!(inv.program, PathBuf::from("/opt/claude"));
        assert_eq!(inv.cwd, PathBuf::from("/tmp/wt/jarvis-finis-tests"));
        assert_eq!(
            &inv.args[..10],
            [
                "-p",
                "--output-format",
                "stream-json",
                "--verbose",
                "--permission-mode",
                "dontAsk",
                "--permission-prompts",
                "none",
                "--restricted",
                "--strict-mcp-config",
            ]
        );
        // Les réglages du dépôt (`.claude/settings.json`) ne s'appliquent jamais.
        assert!(!inv.args.iter().any(|a| a.starts_with("--setting-sources")));
        assert_eq!(
            eq_value(&inv, "--tools"),
            Some("Bash,Read,Edit,Write,Glob,Grep")
        );
        let mcp: Value = serde_json::from_str(eq_value(&inv, "--mcp-config").unwrap()).unwrap();
        assert_eq!(mcp, json!({"mcpServers": {}}));
        let allowed = eq_value(&inv, "--allowedTools").unwrap();
        assert_eq!(allowed, DEFAULT_ALLOWED_TOOLS.join(","));
        assert_eq!(
            value(&inv, "--session-id"),
            Some("0b6e2f3c-1d2e-4f5a-8b9c-0d1e2f3a4b5c")
        );
        assert_eq!(value(&inv, "--name"), Some("jarvis Jarvis finis-tests"));
        assert_eq!(
            value(&inv, "--append-system-prompt"),
            Some(system_prompt("feature/jarvis-finis-tests").as_str())
        );
        // Modèle par défaut, sans plafond.
        assert!(!inv.args.iter().any(|a| a == "--model"));
        assert!(!inv.args.iter().any(|a| a == "--max-budget-usd"));
        assert_eq!(
            inv.create_dirs,
            [
                PathBuf::from("/cache/target/Jarvis"),
                PathBuf::from("/cache/agent-cargo")
            ]
        );

        let s = AgentSettings {
            model: "opus".into(),
            max_budget_usd: 2.5,
            ..settings()
        };
        let inv = invocation(&s);
        assert_eq!(value(&inv, "--model"), Some("opus"));
        assert_eq!(value(&inv, "--max-budget-usd"), Some("2.5"));
    }

    #[test]
    fn reglages_sans_attribution_ni_connecteurs() {
        let v = settings_json(&invocation(&settings()));
        assert_eq!(v["includeCoAuthoredBy"], false);
        assert_eq!(v["attribution"]["commit"], "");
        assert_eq!(v["attribution"]["pr"], "");
        assert_eq!(v["disableClaudeAiConnectors"], true);
        assert_eq!(v["strictMcpConfig"], true);
    }

    #[test]
    fn bac_a_sable_obligatoire() {
        let v = settings_json(&invocation(&settings()));
        let sb = &v["sandbox"];
        assert_eq!(sb["enabled"], true);
        // Sans bac à sable, Claude Code s'arrête au lieu de tout lancer.
        assert_eq!(sb["failIfUnavailable"], true);
        assert_eq!(sb["allowUnsandboxedCommands"], false);
        assert_eq!(sb["autoAllowBashIfSandboxed"], false);
        assert_eq!(sb["excludedCommands"], json!([]));
        let fs = &sb["filesystem"];
        assert_eq!(
            strings(&fs["allowWrite"]),
            ["/cache/target/Jarvis", "/cache/agent-cargo"]
        );
        let deny_read = strings(&fs["denyRead"]);
        for p in [
            "~/.ssh",
            "~/.gnupg",
            "~/.config/jarvis",
            "~/.config/gh",
            "~/.git-credentials",
            "~/.aws",
            "~/.docker",
            "~/.local/share/keyrings",
            "~/.claude/.credentials.json",
        ] {
            assert!(deny_read.contains(&p), "{p}");
        }
        let deny_write = strings(&fs["denyWrite"]);
        for d in [".git", ".claude", ".cargo", ".githooks", ".husky"] {
            let p = format!("/tmp/wt/jarvis-finis-tests/{d}");
            assert!(deny_write.contains(&p.as_str()), "{p}");
        }
        let net = &sb["network"];
        assert_eq!(
            strings(&net["allowedDomains"]),
            ["crates.io", "index.crates.io", "static.crates.io"]
        );
        assert_eq!(net["strictAllowlist"], true);
        assert_eq!(net["allowLocalBinding"], false);
        assert_eq!(net["allowAllUnixSockets"], false);
        assert_eq!(net["allowUnixSockets"], json!([]));
    }

    #[test]
    fn permissions_limitees_au_worktree() {
        let mut s = settings();
        s.extra_allowed_tools = vec!["Bash(make*)".into(), "Read(~/.ssh/**)".into()];
        let inv = invocation(&s);
        let v = settings_json(&inv);
        let allow = strings(&v["permissions"]["allow"]);
        for t in [
            "Read(./**)",
            "Edit(./**)",
            "Glob",
            "Grep",
            "Bash(cargo test*)",
            "Bash(make*)",
        ] {
            assert!(allow.contains(&t), "{t}");
        }
        for t in [
            "Read",
            "Edit",
            "Write",
            "MultiEdit",
            "TodoWrite",
            "Bash(rg *)",
            "Bash(git grep*)",
            "Read(~/.ssh/**)",
        ] {
            assert!(!allow.contains(&t), "{t}");
        }
        assert_eq!(
            eq_value(&inv, "--allowedTools"),
            Some(allow.join(",").as_str())
        );
        let deny = strings(&v["permissions"]["deny"]);
        assert_eq!(deny, DENIED_TOOLS);
        for t in [
            "Bash(git push*)",
            "Read(~/.ssh/**)",
            "Read(~/.gnupg/**)",
            "Read(~/.config/jarvis/**)",
            "Read(~/.config/gh/**)",
            "Read(~/.git-credentials)",
            "Read(~/.aws/**)",
            "Read(~/.claude/.credentials.json)",
            "Edit(./.git/**)",
            "Edit(./.claude/**)",
            "Edit(./.cargo/**)",
            "Edit(./.githooks/**)",
            "Edit(./.husky/**)",
        ] {
            assert!(deny.contains(&t), "{t}");
        }
    }

    #[test]
    fn arguments_interdisent_push_et_commit() {
        let mut s = settings();
        // Un outil interdit ajouté par erreur aux permis n'y figure pas.
        s.extra_allowed_tools = vec!["Bash(git push*)".into(), "Bash(make*)".into()];
        let inv = invocation(&s);
        let denied = eq_value(&inv, "--disallowedTools").unwrap();
        assert_eq!(denied, DENIED_TOOLS.join(","));
        for t in ["Bash(git push*)", "Bash(git commit*)", "Bash(git merge*)"] {
            assert!(denied.split(',').any(|d| d == t), "{t}");
        }
        let allowed = eq_value(&inv, "--allowedTools").unwrap();
        assert!(allowed.split(',').all(|a| !DENIED_TOOLS.contains(&a)));
        assert!(allowed.ends_with(",Bash(make*)"));
        for a in &inv.args {
            assert!(!a.contains("dangerously"), "{a}");
            assert!(!a.contains("bypassPermissions"), "{a}");
        }
        assert_eq!(value(&inv, "--permission-mode"), Some("dontAsk"));
        assert_eq!(value(&inv, "--permission-prompts"), Some("none"));
        let prompt = system_prompt("feature/jarvis-x");
        assert!(prompt.contains("pousser"));
        assert!(prompt.contains("committer"));
    }

    #[test]
    fn consigne_passee_par_l_entree() {
        let s = settings();
        let inv = invocation(&s);
        assert_eq!(inv.stdin, "Finis les tests du projet Jarvis");
        assert!(inv.args.iter().all(|a| !a.contains("Finis les tests")));
    }

    #[test]
    fn consignes_systeme_finissent_par_le_resume() {
        let prompt = system_prompt("feature/jarvis-finis-tests");
        assert!(prompt.contains("« feature/jarvis-finis-tests »"));
        let last = prompt.trim_end().lines().last().unwrap();
        assert!(last.contains("RÉSUMÉ :"), "{last}");
    }

    #[test]
    fn environnement_sans_cle_ni_agent_ssh() {
        let s = settings();
        assert!(!s.pass_api_key);
        let inv = invocation(&s);
        for k in [
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "ANTHROPIC_BASE_URL",
            "CLAUDE_CODE_OAUTH_TOKEN",
            "CLAUDE_CODE_USE_BEDROCK",
            "CLAUDE_CODE_USE_VERTEX",
            "AWS_SECRET_ACCESS_KEY",
            "SSH_AUTH_SOCK",
            "DBUS_SESSION_BUS_ADDRESS",
            "XDG_RUNTIME_DIR",
            "DISPLAY",
            "WAYLAND_DISPLAY",
            "GH_TOKEN",
            "GITHUB_TOKEN",
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_INDEX_FILE",
            "CLAUDECODE",
            "JARVIS_CLE",
        ] {
            assert_eq!(env(&inv, k), None, "{k}");
        }
        // Liste fermée : chaque variable est connue.
        for (k, _) in &inv.env {
            let known = [
                "HOME",
                "USER",
                "LANG",
                "TMPDIR",
                "XDG_CONFIG_HOME",
                "PATH",
                "CARGO_TARGET_DIR",
                "CARGO_HOME",
                "BASH_DEFAULT_TIMEOUT_MS",
                "BASH_MAX_TIMEOUT_MS",
            ];
            assert!(
                known.contains(&k.as_str()) || k.starts_with("LC_") || k.starts_with("GIT_"),
                "{k}"
            );
        }
        let mut keys: Vec<&str> = inv.env.iter().map(|(k, _)| k.as_str()).collect();
        keys.sort();
        let len = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), len, "pas de doublon");
        assert_eq!(env(&inv, "HOME"), Some("/h"));
        assert_eq!(env(&inv, "LC_TIME"), Some("fr_FR.UTF-8"));
        assert_eq!(env(&inv, "GIT_SSH_COMMAND"), Some("false"));
        assert_eq!(env(&inv, "GIT_TERMINAL_PROMPT"), Some("0"));
        assert_eq!(env(&inv, "GIT_CONFIG_COUNT"), Some("1"));
        assert_eq!(env(&inv, "GIT_CONFIG_KEY_0"), Some("credential.helper"));
        // Jamais le registre cargo de Charles.
        assert_eq!(env(&inv, "CARGO_HOME"), Some("/cache/agent-cargo"));
        assert_eq!(env(&inv, "BASH_DEFAULT_TIMEOUT_MS"), Some("900000"));
        assert_eq!(env(&inv, "BASH_MAX_TIMEOUT_MS"), Some("1500000"));
        // `/h/.rustup` n'existe pas.
        assert_eq!(env(&inv, "RUSTUP_HOME"), None);
    }

    #[test]
    fn delai_bash_sous_le_silence_maximal() {
        let s = AgentSettings::default();
        const { assert!(BASH_DEFAULT_TIMEOUT_MS <= BASH_MAX_TIMEOUT_MS) };
        assert!(BASH_MAX_TIMEOUT_MS < s.idle_minutes * 60 * 1000);
    }

    #[test]
    fn rustup_trouve_dans_le_dossier_personnel() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir(home.path().join(".rustup")).unwrap();
        let mut brain = brain_env();
        brain.retain(|(k, _)| k != "HOME");
        brain.push(("HOME".into(), home.path().display().to_string()));
        let s = settings();
        let inv = build_with_env(&s, &s.projects[0], &task(), git_set(), &brain);
        let expected = home.path().join(".rustup").display().to_string();
        assert_eq!(env(&inv, "RUSTUP_HOME"), Some(expected.as_str()));
    }

    #[test]
    fn cle_api_transmise_si_option() {
        let s = AgentSettings {
            pass_api_key: true,
            ..settings()
        };
        let inv = invocation(&s);
        assert_eq!(env(&inv, "ANTHROPIC_API_KEY"), Some("sk-ant-cerveau"));
        assert_eq!(env(&inv, "SSH_AUTH_SOCK"), None);
        assert_eq!(env(&inv, "ANTHROPIC_AUTH_TOKEN"), None);

        // Clé lue dans la variable configurée, passée sous le nom attendu.
        let s = AgentSettings {
            pass_api_key: true,
            api_key_env: "JARVIS_CLE".into(),
            ..settings()
        };
        let inv = invocation(&s);
        assert_eq!(env(&inv, "ANTHROPIC_API_KEY"), Some("sk-ant-jarvis"));
        assert_eq!(env(&inv, "JARVIS_CLE"), None);

        // Variable absente : aucune clé.
        let s = AgentSettings {
            pass_api_key: true,
            api_key_env: "ABSENTE".into(),
            ..settings()
        };
        assert_eq!(env(&invocation(&s), "ANTHROPIC_API_KEY"), None);
    }

    #[test]
    fn chemin_cargo_en_tete_du_path() {
        let s = settings();
        let inv = invocation(&s);
        assert_eq!(
            env(&inv, "PATH"),
            Some("/h/.cargo/bin:/h/.local/bin:/usr/bin:/bin")
        );
        assert_eq!(env(&inv, "CARGO_TARGET_DIR"), Some("/cache/target/Jarvis"));
    }

    #[test]
    fn environnement_reel_ferme() {
        // `build` lit l'environnement du processus de test, sans rien y ajouter.
        let s = settings();
        let inv = build(
            &s,
            &s.projects[0],
            &task(),
            git_set(),
            vec!["SSH_AUTH_SOCK".into()],
        );
        for k in [
            "SSH_AUTH_SOCK",
            "DBUS_SESSION_BUS_ADDRESS",
            "ANTHROPIC_API_KEY",
            "XDG_RUNTIME_DIR",
        ] {
            assert_eq!(env(&inv, k), None, "{k}");
        }
        assert!(
            env(&inv, "PATH")
                .unwrap()
                .starts_with("/h/.cargo/bin:/h/.local/bin")
        );
    }

    #[test]
    fn flux_suit_la_derniere_action() {
        let mut p = StreamParser::default();
        p.feed(&init_line("abc"));
        assert_eq!(p.session_id(), Some("abc"));
        p.feed(&text_line("Je regarde les tests."));
        assert_eq!(
            p.progress().last_text.as_deref(),
            Some("Je regarde les tests.")
        );
        assert_eq!(p.progress().last_action, None);
        p.feed(&tool_use_line(
            "Bash",
            json!({"command": "cargo test -p jarvis-brain --lib -- --nocapture"}),
        ));
        assert_eq!(
            p.progress().last_action.as_deref(),
            Some("lance « cargo test -p jarvis-brain »")
        );
        p.feed(&tool_use_line(
            "Edit",
            json!({"file_path": "/w/crates/jarvis-brain/src/brain.rs"}),
        ));
        assert_eq!(
            p.progress().last_action.as_deref(),
            Some("modifie brain.rs")
        );
        assert_eq!(p.progress().lines, 4);
        assert_eq!(p.progress().result, None);
    }

    #[test]
    fn description_des_outils() {
        let d = describe_tool;
        assert_eq!(
            d("Bash", &json!({"command": "cargo test"})),
            "lance « cargo test »"
        );
        assert_eq!(
            d("Write", &json!({"file_path": "src/a.rs"})),
            "modifie a.rs"
        );
        assert_eq!(
            d("Read", &json!({"file_path": "/x/README.md"})),
            "lit README.md"
        );
        assert_eq!(d("Grep", &json!({"pattern": "fn"})), "cherche dans le code");
        assert_eq!(
            d("Glob", &json!({"pattern": "*.rs"})),
            "cherche dans le code"
        );
        assert_eq!(d("Bash", &json!({})), "lance une commande");
        assert_eq!(d("Edit", &json!(null)), "modifie un fichier");
        assert_eq!(
            d(
                "Edit",
                &json!({"file_path": "/home/c/Work/Jarvis/src/brain.rs"})
            ),
            "modifie brain.rs"
        );
        assert_eq!(
            d("Read", &json!({"file_path": "/home/c/.ssh/id_ed25519"})),
            "lit id_ed25519"
        );
    }

    #[test]
    fn outils_inconnus_sans_nom_brut() {
        let d = describe_tool;
        for name in ["TodoWrite", "mcp__serveur__outil", "Task", "WebFetch", ""] {
            assert_eq!(d(name, &json!({"x": 1})), "travaille", "{name}");
        }
    }

    #[test]
    fn commande_courte_sans_chemin_absolu() {
        let d = |cmd: &str| describe_tool("Bash", &json!({ "command": cmd }));
        assert_eq!(
            d("cd /home/c/.local/share/jarvis/worktrees/x && cargo test -p jarvis-brain --lib"),
            "lance « cargo test -p jarvis-brain »"
        );
        assert_eq!(d("cd a; cd b && ls -la"), "lance « ls -la »");
        assert_eq!(
            d("CARGO_TARGET_DIR=/home/c/target RUST_LOG=debug cargo build"),
            "lance « cargo build »"
        );
        assert_eq!(d("ls /home/c/Work/Jarvis/crates/"), "lance « ls crates »");
        assert_eq!(
            d("head -n 5 '/home/c/x/lib.rs'"),
            "lance « head -n 5 lib.rs »"
        );
        assert_eq!(d("cd /tmp"), "lance une commande");
        assert_eq!(d("ls /"), "lance « ls … »");
        let long = d(&format!("cargo {}", "x".repeat(200)));
        assert!(
            long.chars().count() <= 60 + "lance «  »".chars().count(),
            "{long}"
        );
        for cmd in [
            "cd /home/c && rg secret /home/c/Work",
            "/home/c/.cargo/bin/cargo test",
        ] {
            assert!(!d(cmd).contains("/home"), "{cmd}");
        }
    }

    #[test]
    fn flux_lit_le_resultat() {
        let mut p = StreamParser::default();
        p.feed(&init_line("abc"));
        p.feed(&result_line(true, "Fini.\nRÉSUMÉ : Les tests passent."));
        let r = p.progress().result.clone().unwrap();
        assert!(!r.is_error);
        assert_eq!(r.subtype, "success");
        assert_eq!(r.text, "Fini.\nRÉSUMÉ : Les tests passent.");
        assert_eq!(r.cost_usd, Some(0.12));
        assert_eq!(r.num_turns, Some(3));

        let mut p = StreamParser::default();
        p.feed(&result_line(false, "raté"));
        let r = p.progress().result.clone().unwrap();
        assert!(r.is_error);
        assert_eq!(r.subtype, "error_during_execution");
    }

    #[test]
    fn flux_ignore_les_lignes_invalides() {
        let mut p = StreamParser::default();
        p.feed(&tool_use_line("Read", json!({"file_path": "a.rs"})));
        let before = p.progress().clone();
        for line in [
            "pas du json",
            "{\"type\":",
            "{\"type\":\"inconnu\",\"x\":1}",
            "{\"type\":\"assistant\",\"message\":\"bizarre\"}",
            "{\"type\":\"result\"}",
            "[1,2]",
            "",
        ] {
            p.feed(line);
        }
        assert_eq!(p.progress().last_action, before.last_action);
        assert_eq!(p.progress().last_text, before.last_text);
        assert_eq!(p.session_id(), None);
        // Un résultat sans texte reste lisible.
        assert_eq!(
            p.progress().result.as_ref().map(|r| r.text.as_str()),
            Some("")
        );
    }

    #[test]
    fn resume_extrait_de_la_ligne_resume() {
        let text = "J'ai corrigé deux tests.\n\nRÉSUMÉ : Les tests du cerveau passent tous.";
        assert_eq!(
            summary(text).as_deref(),
            Some("Les tests du cerveau passent tous.")
        );
        for line in [
            "Résumé : Tout passe.",
            "resume: Tout passe.",
            "RESUME :Tout passe.",
            "**RÉSUMÉ :** Tout passe.",
            "- Résumé\u{a0}: Tout passe.",
        ] {
            assert_eq!(
                summary(&format!("Blabla.\n{line}")).as_deref(),
                Some("Tout passe."),
                "{line}"
            );
        }
    }

    #[test]
    fn resume_de_repli_sans_ligne() {
        assert_eq!(
            summary("J'ai regardé le code. Les tests passent maintenant !\n").as_deref(),
            Some("Les tests passent maintenant !")
        );
        assert_eq!(
            summary("Premier point.\n\n```\ncargo test\n```\nVersion 1.2 publiée").as_deref(),
            Some("Version 1.2 publiée")
        );
        assert_eq!(summary(""), None);
        assert_eq!(summary("  \n```\n"), None);
        // Ligne de résumé vide : repli.
        assert_eq!(
            summary("Tout est prêt.\nRÉSUMÉ :").as_deref(),
            Some("Tout est prêt.")
        );
    }

    #[test]
    fn resume_sans_markdown_et_borne() {
        assert_eq!(
            summary("RÉSUMÉ : J'ai **corrigé** le test `fold` dans *text_util.rs*.").as_deref(),
            Some("J'ai corrigé le test fold dans text_util.rs.")
        );
        let long = format!("RÉSUMÉ : {}", "mot ".repeat(100));
        let s = summary(&long).unwrap();
        assert!(s.chars().count() <= 200, "{}", s.chars().count());
        assert!(s.ends_with("mot"), "{s}");
        let accents = format!("RÉSUMÉ : {}", "é".repeat(300));
        assert!(summary(&accents).unwrap().chars().count() <= 200);
        assert_eq!(
            summary("Fini.\n## Bilan\nTout passe.").as_deref(),
            Some("Tout passe.")
        );
    }
}
