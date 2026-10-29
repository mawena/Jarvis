//! Ligne de commande de Claude Code et lecture de son flux `stream-json`.

use super::settings::{AgentSettings, DENIED_TOOLS, Project};
use super::types::{Progress, RunResult, TaskRecord};
use crate::text::fold;
use serde_json::Value;
use std::path::{Path, PathBuf};

/// Tout ce qu'il faut pour lancer l'agent.
#[derive(Debug, Clone, PartialEq)]
pub struct Invocation {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env_set: Vec<(String, String)>,
    pub env_remove: Vec<String>,
    /// Consigne de Charles, écrite sur l'entrée standard puis fermée.
    pub stdin: String,
}

/// Réglages passés à Claude Code : aucune ligne d'attribution.
const ATTRIBUTION_SETTINGS: &str =
    r#"{"includeCoAuthoredBy":false,"attribution":{"commit":"","pr":""}}"#;

/// Variables toujours retirées : aucun identifiant pour pousser.
const ALWAYS_REMOVED: &[&str] = &[
    "SSH_AUTH_SOCK",
    "GH_TOKEN",
    "GITHUB_TOKEN",
    "GIT_ASKPASS",
    "SSH_ASKPASS",
    // Claude Code lancé depuis une session Claude Code se croirait imbriqué.
    "CLAUDECODE",
];

/// Ligne de commande de l'agent pour `task` (voir « Décisions » du plan de la
/// phase 7). `git_set` et `git_remove` viennent de `git::no_push_env`.
pub fn build(
    settings: &AgentSettings,
    project: &Project,
    task: &TaskRecord,
    git_set: Vec<(String, String)>,
    git_remove: Vec<String>,
) -> Invocation {
    // Les interdits sont prioritaires chez Claude Code ; on les retire quand même.
    let allowed: Vec<String> = settings
        .allowed_tools(project)
        .into_iter()
        .filter(|t| !DENIED_TOOLS.contains(&t.as_str()))
        .collect();
    let slug = task.branch.rsplit('/').next().unwrap_or(&task.branch);
    let slug = slug.strip_prefix("jarvis-").unwrap_or(slug);

    let mut args: Vec<String> = [
        "-p",
        "--output-format",
        "stream-json",
        "--verbose",
        "--permission-mode",
        "dontAsk",
        "--permission-prompts",
        "none",
        "--setting-sources",
        "project",
        "--settings",
        ATTRIBUTION_SETTINGS,
    ]
    .iter()
    .map(|a| a.to_string())
    .collect();
    // Forme `--option=liste` : ces options acceptent plusieurs valeurs.
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

    let mut env_set = git_set;
    for (key, value) in [("GIT_SSH_COMMAND", "false"), ("GIT_TERMINAL_PROMPT", "0")] {
        if !env_set.iter().any(|(k, _)| k == key) {
            env_set.push((key.into(), value.into()));
        }
    }
    env_set.retain(|(k, _)| k != "PATH" && k != "CARGO_TARGET_DIR");
    env_set.push(("PATH".into(), agent_path(&settings.extra_path)));
    env_set.push((
        "CARGO_TARGET_DIR".into(),
        settings
            .cargo_target_dir
            .join(&project.name)
            .to_string_lossy()
            .into_owned(),
    ));

    let mut env_remove: Vec<String> = Vec::new();
    let api_key = (!settings.pass_api_key).then_some("ANTHROPIC_API_KEY");
    let removed = git_remove
        .into_iter()
        .chain(ALWAYS_REMOVED.iter().map(|k| k.to_string()))
        .chain(api_key.map(String::from));
    for key in removed {
        if !env_remove.contains(&key) && !env_set.iter().any(|(k, _)| *k == key) {
            env_remove.push(key);
        }
    }

    Invocation {
        program: settings.claude_path.clone(),
        args,
        cwd: task.worktree.clone(),
        env_set,
        env_remove,
        stdin: task.instruction.clone(),
    }
}

/// `extra_path` puis le `PATH` hérité : le service systemd n'a pas `~/.cargo/bin`.
fn agent_path(extra: &[PathBuf]) -> String {
    let mut parts: Vec<String> = extra
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    if let Some(inherited) = std::env::var_os("PATH") {
        let inherited = inherited.to_string_lossy().into_owned();
        if !inherited.is_empty() {
            parts.push(inherited);
        }
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
        "Bash" => match input["command"].as_str() {
            Some(cmd) if !cmd.trim().is_empty() => {
                let words: Vec<&str> = cmd.split_whitespace().take(4).collect();
                format!("lance « {} »", bounded(&words.join(" "), 60))
            }
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
        other => other.to_string(),
    }
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
            extra_path: vec![
                PathBuf::from("/h/.cargo/bin"),
                PathBuf::from("/h/.local/bin"),
            ],
            ..AgentSettings::default()
        }
    }

    fn invocation(s: &AgentSettings) -> Invocation {
        let git_set = vec![
            ("GIT_CONFIG_COUNT".to_string(), "1".to_string()),
            (
                "GIT_CONFIG_KEY_0".to_string(),
                "credential.helper".to_string(),
            ),
            ("GIT_CONFIG_VALUE_0".to_string(), String::new()),
            ("GIT_SSH_COMMAND".to_string(), "false".to_string()),
        ];
        build(
            s,
            &s.projects[0],
            &task(),
            git_set,
            vec!["SSH_AUTH_SOCK".into()],
        )
    }

    fn value<'a>(inv: &'a Invocation, flag: &str) -> Option<&'a str> {
        let i = inv.args.iter().position(|a| a == flag)?;
        inv.args.get(i + 1).map(String::as_str)
    }

    fn env<'a>(inv: &'a Invocation, key: &str) -> Option<&'a str> {
        inv.env_set
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    #[test]
    fn arguments_de_lancement() {
        let s = settings();
        let inv = invocation(&s);
        assert_eq!(inv.program, PathBuf::from("/opt/claude"));
        assert_eq!(inv.cwd, PathBuf::from("/tmp/wt/jarvis-finis-tests"));
        assert_eq!(
            &inv.args[..9],
            [
                "-p",
                "--output-format",
                "stream-json",
                "--verbose",
                "--permission-mode",
                "dontAsk",
                "--permission-prompts",
                "none",
                "--setting-sources",
            ]
        );
        assert_eq!(value(&inv, "--setting-sources"), Some("project"));
        let settings_json: Value =
            serde_json::from_str(value(&inv, "--settings").unwrap()).unwrap();
        assert_eq!(settings_json["includeCoAuthoredBy"], false);
        assert_eq!(settings_json["attribution"]["commit"], "");
        assert_eq!(settings_json["attribution"]["pr"], "");
        let allowed = inv
            .args
            .iter()
            .find_map(|a| a.strip_prefix("--allowedTools="))
            .unwrap();
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
    fn arguments_interdisent_push_et_commit() {
        let mut s = settings();
        // Un outil interdit ajouté par erreur aux permis n'y figure pas.
        s.extra_allowed_tools = vec!["Bash(git push*)".into(), "Bash(make*)".into()];
        let inv = invocation(&s);
        let denied = inv
            .args
            .iter()
            .find_map(|a| a.strip_prefix("--disallowedTools="))
            .unwrap();
        assert_eq!(denied, DENIED_TOOLS.join(","));
        for t in ["Bash(git push*)", "Bash(git commit*)", "Bash(git merge*)"] {
            assert!(denied.split(',').any(|d| d == t), "{t}");
        }
        let allowed = inv
            .args
            .iter()
            .find_map(|a| a.strip_prefix("--allowedTools="))
            .unwrap();
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
            "SSH_AUTH_SOCK",
            "GH_TOKEN",
            "GITHUB_TOKEN",
            "GIT_ASKPASS",
            "SSH_ASKPASS",
        ] {
            assert!(inv.env_remove.iter().any(|r| r == k), "{k}");
        }
        // Pas de doublon, rien de retiré qui soit aussi posé.
        let mut sorted = inv.env_remove.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), inv.env_remove.len());
        assert!(inv.env_set.iter().all(|(k, _)| !inv.env_remove.contains(k)));
        assert_eq!(env(&inv, "GIT_SSH_COMMAND"), Some("false"));
        assert_eq!(env(&inv, "GIT_TERMINAL_PROMPT"), Some("0"));
        assert_eq!(env(&inv, "GIT_CONFIG_COUNT"), Some("1"));
        assert_eq!(env(&inv, "GIT_CONFIG_KEY_0"), Some("credential.helper"));
    }

    #[test]
    fn cle_api_transmise_si_option() {
        let s = AgentSettings {
            pass_api_key: true,
            ..settings()
        };
        let inv = invocation(&s);
        assert!(!inv.env_remove.iter().any(|r| r == "ANTHROPIC_API_KEY"));
        assert!(inv.env_remove.iter().any(|r| r == "SSH_AUTH_SOCK"));
    }

    #[test]
    fn chemin_cargo_en_tete_du_path() {
        let s = settings();
        let inv = invocation(&s);
        let path = env(&inv, "PATH").unwrap();
        assert!(path.starts_with("/h/.cargo/bin:/h/.local/bin"), "{path}");
        if let Some(inherited) = std::env::var_os("PATH") {
            let inherited = inherited.to_string_lossy().to_string();
            if !inherited.is_empty() {
                assert!(path.ends_with(&format!(":{inherited}")), "{path}");
            }
        }
        assert_eq!(env(&inv, "CARGO_TARGET_DIR"), Some("/cache/target/Jarvis"));
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
        assert_eq!(d("TodoWrite", &json!({})), "TodoWrite");
        assert_eq!(d("Bash", &json!({})), "lance une commande");
        assert_eq!(d("Edit", &json!(null)), "modifie un fichier");
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
