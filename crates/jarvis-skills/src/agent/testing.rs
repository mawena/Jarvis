//! Doublures de test de l'agent : dépôts git temporaires, faux `claude`, lignes
//! du flux `stream-json`. Jamais le vrai `claude`, jamais le réseau.

use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

/// Dépôt git temporaire `<dossier>/projet` sur `develop`, avec une branche `main`,
/// les noms git flow, une identité locale, sans signature, et un commit.
pub fn temp_repo() -> (TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("dossier temporaire");
    let repo = dir.path().join("projet");
    std::fs::create_dir(&repo).expect("dossier du dépôt");
    git(&repo, &["init", "-q", "-b", "develop"]);
    for (key, value) in [
        ("user.name", "Test Jarvis"),
        ("user.email", "test@jarvis.invalid"),
        ("commit.gpgsign", "false"),
        ("tag.gpgsign", "false"),
        ("gitflow.branch.master", "main"),
        ("gitflow.branch.develop", "develop"),
        ("gitflow.prefix.feature", "feature/"),
        ("gitflow.prefix.release", "release/"),
        ("gitflow.prefix.hotfix", "hotfix/"),
    ] {
        git(&repo, &["config", key, value]);
    }
    std::fs::write(repo.join("README.md"), "# Projet d'essai\n").expect("README");
    std::fs::write(repo.join("lib.rs"), "pub fn un() -> u32 {\n    1\n}\n").expect("lib.rs");
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "premier commit"]);
    git(&repo, &["branch", "main"]);
    (dir, repo)
}

/// `git -C repo args…` ; panique si git échoue. Sortie standard sans espaces finaux.
pub fn git(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .expect("git introuvable");
    assert!(
        out.status.success(),
        "git {args:?} a échoué : {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim_end().to_string()
}

/// Faux `claude` : script `#!/bin/sh` exécutable `dir/claude` qui écrit ses
/// arguments (un par ligne), son environnement et son entrée standard dans
/// `dir/{args,env,stdin}`, puis exécute `body`.
pub fn fake_claude(dir: &Path, body: &str) -> PathBuf {
    let quoted = shell_quote(&dir.display().to_string());
    let script = format!(
        "#!/bin/sh\n\
         printf '%s\\n' \"$@\" > {quoted}/args\n\
         env > {quoted}/env\n\
         cat > {quoted}/stdin\n\
         {body}\n"
    );
    let source = dir.join("claude.source");
    std::fs::write(&source, script).expect("script du faux claude");
    // Copie exécutable faite par un autre processus : notre processus n'a jamais le
    // fichier ouvert en écriture, donc pas de « Text file busy » si un autre test
    // lance un processus au même moment.
    let path = dir.join("claude");
    let status = Command::new("install")
        .arg("-m")
        .arg("755")
        .arg(&source)
        .arg(&path)
        .status()
        .expect("install introuvable");
    assert!(status.success(), "installation du faux claude");
    path
}

/// Texte entre apostrophes pour `sh`.
pub fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// Ligne `system/init` du flux.
pub fn init_line(session: &str) -> String {
    line(json!({
        "type": "system",
        "subtype": "init",
        "session_id": session,
        "cwd": "/tmp",
        "model": "claude-test",
        "tools": ["Read", "Edit", "Bash"],
    }))
}

/// Message de l'assistant contenant du texte.
pub fn text_line(text: &str) -> String {
    assistant(json!({"type": "text", "text": text}))
}

/// Message de l'assistant qui appelle un outil.
pub fn tool_use_line(name: &str, input: Value) -> String {
    assistant(json!({
        "type": "tool_use",
        "id": "toolu_test",
        "name": name,
        "input": input,
    }))
}

/// Ligne `result` finale : réussite ou erreur, avec `text` comme résultat.
pub fn result_line(ok: bool, text: &str) -> String {
    line(json!({
        "type": "result",
        "subtype": if ok { "success" } else { "error_during_execution" },
        "is_error": !ok,
        "duration_ms": 1200,
        "num_turns": 3,
        "result": text,
        "session_id": "session-test",
        "total_cost_usd": 0.12,
    }))
}

fn assistant(content: Value) -> String {
    line(json!({
        "type": "assistant",
        "message": {
            "role": "assistant",
            "content": [content],
        },
        "session_id": "session-test",
    }))
}

fn line(v: Value) -> String {
    serde_json::to_string(&v).expect("JSON")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn depot_temporaire_sur_develop() {
        let (_dir, repo) = temp_repo();
        assert_eq!(git(&repo, &["branch", "--show-current"]), "develop");
        assert_eq!(
            git(&repo, &["config", "gitflow.prefix.feature"]),
            "feature/"
        );
        assert_eq!(
            git(&repo, &["rev-parse", "main"]),
            git(&repo, &["rev-parse", "develop"])
        );
        assert_eq!(git(&repo, &["status", "--porcelain"]), "");
    }

    #[test]
    fn faux_claude_enregistre_ses_entrees() {
        let dir = tempfile::tempdir().unwrap();
        let line = result_line(true, "Fini. L'agent a réussi.");
        let claude = fake_claude(dir.path(), &format!("echo {}", shell_quote(&line)));
        let out = Command::new(&claude)
            .args(["-p", "--verbose"])
            .env("JARVIS_ESSAI", "1")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                use std::io::Write;
                child.stdin.take().unwrap().write_all(b"consigne")?;
                child.wait_with_output()
            })
            .unwrap();
        assert!(out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim_end(), line);
        let read = |f: &str| std::fs::read_to_string(dir.path().join(f)).unwrap();
        assert_eq!(read("args"), "-p\n--verbose\n");
        assert!(read("env").contains("JARVIS_ESSAI=1"));
        assert_eq!(read("stdin"), "consigne");
    }

    #[test]
    fn lignes_du_flux_valides() {
        let v: Value =
            serde_json::from_str(&tool_use_line("Bash", json!({"command": "ls"}))).unwrap();
        assert_eq!(v["message"]["content"][0]["name"], "Bash");
        let v: Value = serde_json::from_str(&result_line(false, "raté")).unwrap();
        assert_eq!(v["is_error"], true);
        let v: Value = serde_json::from_str(&init_line("abc")).unwrap();
        assert_eq!(v["session_id"], "abc");
        let v: Value = serde_json::from_str(&text_line("bonjour")).unwrap();
        assert_eq!(v["message"]["content"][0]["text"], "bonjour");
    }
}
