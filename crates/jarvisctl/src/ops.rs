//! Logique des sous-commandes de `jarvisctl`.

use crate::paths::{
    BINARIES, DEFAULT_MODEL, Paths, SERVICE, SHARED_LIBS, SPEAKER_MODEL, WHISPER_MODEL,
};
use crate::system::{OLLAMA_ADDR, System};
use anyhow::{Context, Result, bail};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const KEY_VAR: &str = "ANTHROPIC_API_KEY";

/// Contenu exact de l'unité systemd utilisateur.
pub fn brain_unit() -> String {
    "[Unit]
Description=Jarvis — cerveau de l'assistant vocal
After=graphical-session.target
PartOf=graphical-session.target

[Service]
Type=simple
ExecStart=%h/.local/bin/jarvis-brain
EnvironmentFile=-%h/.config/jarvis/env
Restart=on-failure
RestartSec=3

[Install]
WantedBy=graphical-session.target
"
    .to_string()
}

const DEFAULT_ENV: &str =
    "# Variables d'environnement du cerveau Jarvis (chargées par systemd).\n# ANTHROPIC_API_KEY=\n";

const DEFAULT_CONFIG: &str = r#"# Configuration de Jarvis (valeurs par défaut commentées).
# mode = "local"          # "local" ou "online"
# address = "Monsieur"

# [local]
# url = "http://127.0.0.1:11434"
# model = "llama3.2:3b"
# keep_alive = "30m"

# [online]
# url = "https://api.anthropic.com"
# model = "claude-haiku-5-5"
# api_key_env = "ANTHROPIC_API_KEY"
"#;

fn systemctl(sys: &dyn System, args: &[&str]) -> Result<crate::system::Output> {
    let mut full = vec!["--user"];
    full.extend_from_slice(args);
    sys.run("systemctl", &full)
        .context("impossible de lancer systemctl")
}

fn write_private(path: &Path, content: &str) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, content)?;
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))?;
    fs::rename(&tmp, path)?;
    Ok(())
}

/// Copie atomique : écrit un fichier temporaire puis renomme.
fn copy_atomic(from: &Path, to: &Path) -> Result<()> {
    let name = to.file_name().unwrap().to_string_lossy().into_owned();
    let tmp = to.with_file_name(format!(".{name}.tmp"));
    fs::copy(from, &tmp).with_context(|| format!("copie de {}", from.display()))?;
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o755))?;
    fs::rename(&tmp, to)?;
    Ok(())
}

pub struct InstallOpts {
    pub source: PathBuf,
    pub no_build: bool,
}

fn build(sys: &dyn System, source: &Path) -> Result<()> {
    let code = sys
        .run_inherit(
            "cargo",
            &[
                "build",
                "--release",
                "-p",
                "jarvis-brain",
                "-p",
                "jarvis-cli",
                "-p",
                "jarvisctl",
            ],
            Some(source),
        )
        .context("impossible de lancer cargo")?;
    if code != 0 {
        bail!("la compilation a échoué (code {code})");
    }
    Ok(())
}

fn copy_binaries(source: &Path, paths: &Paths, out: &mut dyn Write) -> Result<()> {
    let release = source.join("target/release");
    fs::create_dir_all(&paths.bin_dir)?;
    for bin in BINARIES {
        let from = release.join(bin);
        if !from.is_file() {
            bail!("binaire introuvable : {}", from.display());
        }
        copy_atomic(&from, &paths.bin_dir.join(bin))?;
        writeln!(out, "Installé : {}", paths.bin_dir.join(bin).display())?;
    }
    Ok(())
}

/// Noms de bibliothèques partagées : `libonnxruntime.so*` couvre les versions numérotées.
fn is_shared_lib(name: &str) -> bool {
    SHARED_LIBS.contains(&name) || name.starts_with("libonnxruntime.so")
}

/// Copie les bibliothèques de sherpa-onnx de `target/release` vers `~/.local/lib/jarvis/`.
fn copy_libraries(source: &Path, paths: &Paths, out: &mut dyn Write) -> Result<()> {
    let release = source.join("target/release");
    let mut libs: Vec<PathBuf> = fs::read_dir(&release)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| is_shared_lib(&e.file_name().to_string_lossy()) && e.path().is_file())
        .map(|e| e.path())
        .collect();
    libs.sort();
    if libs.is_empty() {
        writeln!(
            out,
            "Attention : aucune bibliothèque sherpa-onnx dans {} (empreinte vocale indisponible).",
            release.display()
        )?;
        return Ok(());
    }
    fs::create_dir_all(&paths.lib_dir)?;
    for from in libs {
        let to = paths.lib_dir.join(from.file_name().unwrap());
        copy_atomic(&from, &to)?;
        writeln!(out, "Installé : {}", to.display())?;
    }
    Ok(())
}

pub fn install(
    paths: &Paths,
    sys: &dyn System,
    opts: &InstallOpts,
    out: &mut dyn Write,
) -> Result<()> {
    if !opts.no_build {
        writeln!(out, "Compilation (release)…")?;
        build(sys, &opts.source)?;
    }
    copy_binaries(&opts.source, paths, out)?;
    copy_libraries(&opts.source, paths, out)?;

    fs::create_dir_all(paths.unit_path.parent().unwrap())?;
    fs::write(&paths.unit_path, brain_unit())?;
    writeln!(out, "Unité écrite : {}", paths.unit_path.display())?;

    fs::create_dir_all(&paths.config_dir)?;
    if !paths.env_file.exists() {
        write_private(&paths.env_file, DEFAULT_ENV)?;
        writeln!(out, "Créé : {}", paths.env_file.display())?;
    }
    if !paths.config_file.exists() {
        fs::write(&paths.config_file, DEFAULT_CONFIG)?;
        writeln!(out, "Créé : {}", paths.config_file.display())?;
    }
    let source = toml::Value::String(opts.source.display().to_string());
    fs::write(&paths.install_file, format!("source = {source}\n"))?;

    systemctl(sys, &["daemon-reload"])?;
    let r = systemctl(sys, &["enable", "--now", SERVICE])?;
    if r.code != 0 {
        bail!(
            "`systemctl --user enable --now {SERVICE}` a échoué (code {})",
            r.code
        );
    }
    writeln!(out, "Service {SERVICE} activé et démarré.")?;

    let in_path = sys
        .env_var("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d == paths.bin_dir))
        .unwrap_or(false);
    if !in_path {
        writeln!(
            out,
            "Attention : {} n'est pas dans le PATH. Ajoutez-le à votre ~/.profile.",
            paths.bin_dir.display()
        )?;
    }
    if paths.cargo_bin.join("jarvis-brain").exists() {
        writeln!(
            out,
            "Attention : un autre jarvis-brain existe dans {}. Supprimez-le avec : cargo uninstall jarvis-brain jarvis-cli",
            paths.cargo_bin.display()
        )?;
    }
    writeln!(
        out,
        "Clé Claude (optionnelle) : jarvisctl set-api-key ; vérification : jarvisctl doctor"
    )?;
    Ok(())
}

/// Lit `source` dans install.toml.
pub fn installed_source(paths: &Paths) -> Result<PathBuf> {
    let text = fs::read_to_string(&paths.install_file).context(
        "Jarvis n'est pas installé (install.toml introuvable) : lancez `jarvisctl install`",
    )?;
    let v: toml::Value = toml::from_str(&text).context("install.toml invalide")?;
    v.get("source")
        .and_then(|s| s.as_str())
        .map(PathBuf::from)
        .context("install.toml : clé `source` absente")
}

pub fn update(paths: &Paths, sys: &dyn System, out: &mut dyn Write) -> Result<()> {
    let source = installed_source(paths)?;
    writeln!(out, "Compilation (release) dans {}…", source.display())?;
    build(sys, &source)?;
    copy_binaries(&source, paths, out)?;
    copy_libraries(&source, paths, out)?;
    let r = systemctl(sys, &["restart", SERVICE])?;
    if r.code != 0 {
        bail!("le redémarrage du service a échoué (code {})", r.code);
    }
    writeln!(out, "Mise à jour terminée, service redémarré.")?;
    Ok(())
}

pub fn uninstall(
    paths: &Paths,
    sys: &dyn System,
    purge: bool,
    yes: bool,
    input: &mut dyn BufRead,
    out: &mut dyn Write,
) -> Result<()> {
    let _ = systemctl(sys, &["stop", SERVICE]);
    let _ = systemctl(sys, &["disable", SERVICE]);
    if paths.unit_path.exists() {
        fs::remove_file(&paths.unit_path)?;
    }
    let _ = systemctl(sys, &["daemon-reload"]);
    writeln!(out, "Service arrêté, désactivé et unité supprimée.")?;

    // jarvisctl en dernier.
    for bin in BINARIES {
        let p = paths.bin_dir.join(bin);
        if p.exists() {
            fs::remove_file(&p)?;
        }
    }
    writeln!(out, "Binaires supprimés de {}.", paths.bin_dir.display())?;
    if paths.lib_dir.exists() {
        fs::remove_dir_all(&paths.lib_dir)?;
    }
    writeln!(
        out,
        "Bibliothèques supprimées de {}.",
        paths.lib_dir.display()
    )?;

    if purge {
        let dirs = [&paths.config_dir, &paths.state_dir, &paths.data_dir];
        let confirmed = yes || {
            writeln!(out, "Supprimer définitivement :")?;
            for d in dirs {
                writeln!(out, "  {}", d.display())?;
            }
            write!(out, "Confirmer ? [o/N] ")?;
            out.flush()?;
            let mut line = String::new();
            input.read_line(&mut line)?;
            matches!(line.trim().to_lowercase().as_str(), "o" | "oui")
        };
        if confirmed {
            for d in dirs {
                if d.exists() {
                    fs::remove_dir_all(d)?;
                }
            }
            writeln!(out, "Configuration, état et données supprimés.")?;
        } else {
            writeln!(out, "Purge annulée : configuration conservée.")?;
        }
    } else {
        writeln!(out, "Configuration conservée (--purge pour l'effacer).")?;
    }
    writeln!(out, "Ollama et ses modèles n'ont pas été touchés.")?;
    Ok(())
}

/// Verbe systemctl simple ; renvoie le code de sortie.
pub fn service_verb(sys: &dyn System, verb: &str, out: &mut dyn Write) -> Result<i32> {
    let r = systemctl(sys, &[verb, SERVICE])?;
    let (done, label) = match verb {
        "start" => ("démarré", "démarrer"),
        "stop" => ("arrêté", "arrêter"),
        "restart" => ("redémarré", "redémarrer"),
        "enable" => ("activé au démarrage", "activer"),
        _ => ("désactivé au démarrage", "désactiver"),
    };
    if r.code == 0 {
        writeln!(out, "Service {done}.")?;
    } else {
        writeln!(out, "Impossible de {label} le service (code {}).", r.code)?;
    }
    Ok(r.code)
}

pub fn parse_show(text: &str) -> HashMap<String, String> {
    text.lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect()
}

/// Heure `HH:MM` extraite d'un `ActiveEnterTimestamp` systemd.
fn since(ts: &str) -> Option<&str> {
    ts.split_whitespace()
        .find(|t| t.matches(':').count() == 2)
        .map(|t| &t[..5])
}

fn api_key_set(paths: &Paths) -> bool {
    fs::read_to_string(&paths.env_file)
        .map(|t| {
            t.lines().any(|l| {
                l.trim()
                    .strip_prefix(KEY_VAR)
                    .and_then(|r| r.trim_start().strip_prefix('='))
                    .is_some_and(|v| !v.trim().trim_matches(['"', '\'']).is_empty())
            })
        })
        .unwrap_or(false)
}

fn local_model(paths: &Paths) -> String {
    fs::read_to_string(&paths.config_file)
        .ok()
        .and_then(|t| toml::from_str::<toml::Value>(&t).ok())
        .and_then(|v| v.get("local")?.get("model")?.as_str().map(String::from))
        .unwrap_or_else(|| DEFAULT_MODEL.to_string())
}

fn model_names(json: &str, key: &str) -> Vec<String> {
    serde_json::from_str::<serde_json::Value>(json)
        .ok()
        .and_then(|v| {
            v.get(key)?.as_array().map(|a| {
                a.iter()
                    .filter_map(|m| m.get("name")?.as_str().map(String::from))
                    .collect()
            })
        })
        .unwrap_or_default()
}

/// Affiche l'état ; code 0 si actif, 3 sinon.
pub fn status(paths: &Paths, sys: &dyn System, out: &mut dyn Write) -> Result<i32> {
    let r = systemctl(
        sys,
        &[
            "show",
            SERVICE,
            "--property=ActiveState,SubState,MainPID,ActiveEnterTimestamp",
        ],
    )?;
    let props = parse_show(&r.stdout);
    let get = |k: &str| props.get(k).map(String::as_str).unwrap_or("");
    let active = get("ActiveState") == "active";
    if active {
        let mut detail = format!("pid {}", get("MainPID"));
        if let Some(t) = since(get("ActiveEnterTimestamp")) {
            detail.push_str(&format!(", depuis {t}"));
        }
        writeln!(out, "Service   : actif ({detail})")?;
    } else {
        let state = match get("ActiveState") {
            "" => "inconnu",
            s => s,
        };
        writeln!(out, "Service   : inactif ({state}, {})", get("SubState"))?;
    }

    let mut model = local_model(paths);
    if active {
        match sys.brain_status() {
            Some((mode, version, reported)) => {
                if !reported.is_empty() {
                    model = reported;
                }
                writeln!(out, "Mode      : {mode} ({model})")?;
                writeln!(out, "Version   : {version}")?;
            }
            None => writeln!(out, "Mode      : cerveau injoignable sur son socket")?,
        }
    }

    match sys.http_get(OLLAMA_ADDR, "/api/version") {
        Some(_) => {
            let loaded = sys
                .http_get(OLLAMA_ADDR, "/api/ps")
                .map(|b| model_names(&b, "models").contains(&model))
                .unwrap_or(false);
            let state = if loaded {
                "modèle chargé"
            } else {
                "modèle non chargé"
            };
            writeln!(out, "Ollama    : joignable, {state}")?;
        }
        None => writeln!(out, "Ollama    : injoignable")?,
    }
    let key = if api_key_set(paths) {
        "présente"
    } else {
        "absente"
    };
    writeln!(out, "Clé Claude: {key}")?;
    Ok(if active { 0 } else { 3 })
}

/// Affiche les vérifications ; renvoie 1 si une vérification obligatoire échoue.
pub fn doctor(paths: &Paths, sys: &dyn System, out: &mut dyn Write) -> Result<i32> {
    let mut failures = 0;
    let mut check = |out: &mut dyn Write, ok: bool, label: &str, hint: &str| -> Result<()> {
        if ok {
            writeln!(out, "✓ {label}")?;
        } else {
            failures += 1;
            writeln!(out, "✗ {label} — {hint}")?;
        }
        Ok(())
    };

    let missing: Vec<&str> = BINARIES
        .into_iter()
        .filter(|b| !paths.bin_dir.join(b).is_file())
        .collect();
    check(
        out,
        missing.is_empty(),
        &format!("Binaires dans {}", paths.bin_dir.display()),
        &format!(
            "manquants : {} ; lancez `jarvisctl install`",
            missing.join(", ")
        ),
    )?;
    check(
        out,
        paths.unit_path.is_file(),
        "Unité systemd installée",
        "lancez `jarvisctl install`",
    )?;
    let enabled = systemctl(sys, &["is-enabled", SERVICE])?;
    check(
        out,
        enabled.stdout.trim() == "enabled",
        "Service activé au démarrage",
        "lancez `jarvisctl enable`",
    )?;
    let active = systemctl(sys, &["is-active", SERVICE])?;
    check(
        out,
        active.stdout.trim() == "active",
        "Service actif",
        "lancez `jarvisctl start` puis `jarvisctl logs`",
    )?;

    let ollama = sys
        .run("ollama", &["--version"])
        .map(|o| o.code == 0)
        .unwrap_or(false);
    check(
        out,
        ollama,
        "Ollama installé",
        "installez-le : https://ollama.com/download",
    )?;
    let tags = sys.http_get(OLLAMA_ADDR, "/api/tags");
    check(
        out,
        tags.is_some(),
        "Ollama joignable",
        "lancez `systemctl start ollama` ou `ollama serve`",
    )?;
    let pulled = tags
        .map(|b| model_names(&b, "models").iter().any(|n| n == DEFAULT_MODEL))
        .unwrap_or(false);
    check(
        out,
        pulled,
        &format!("Modèle {DEFAULT_MODEL} téléchargé"),
        &format!("lancez `ollama pull {DEFAULT_MODEL}`"),
    )?;

    for (tool, pkg) in [
        ("wmctrl", "wmctrl"),
        ("xdotool", "xdotool"),
        ("wpctl", "wireplumber"),
        ("gio", "libglib2.0-bin"),
    ] {
        let found = sys
            .run("sh", &["-c", &format!("command -v {tool}")])
            .map(|o| o.code == 0)
            .unwrap_or(false);
        check(
            out,
            found,
            &format!("{tool} présent"),
            &format!("sudo apt install {pkg}"),
        )?;
    }

    let env = systemctl(sys, &["show-environment"])?;
    check(
        out,
        env.stdout.lines().any(|l| l.starts_with("DISPLAY=")),
        "DISPLAY connu du gestionnaire systemd utilisateur",
        "lancez `systemctl --user import-environment DISPLAY XAUTHORITY` depuis une session graphique",
    )?;

    match fs::metadata(&paths.env_file) {
        Ok(m) => check(
            out,
            m.permissions().mode() & 0o077 == 0,
            "Permissions du fichier env (0600)",
            &format!("lancez `chmod 600 {}`", paths.env_file.display()),
        )?,
        Err(_) => check(
            out,
            false,
            "Fichier env présent",
            "lancez `jarvisctl install`",
        )?,
    }

    if api_key_set(paths) {
        writeln!(out, "✓ Clé API Claude définie")?;
    } else {
        writeln!(
            out,
            "! Clé API Claude absente — optionnelle (mode en ligne) : `jarvisctl set-api-key`"
        )?;
    }
    ear_diagnostics(paths, out)?;
    Ok(if failures > 0 { 1 } else { 0 })
}

/// Oreille du PC : avertissements seulement (le cerveau fonctionne sans).
fn ear_diagnostics(paths: &Paths, out: &mut dyn Write) -> Result<()> {
    let has_lib = |lib: &str| {
        fs::read_dir(&paths.lib_dir)
            .into_iter()
            .flatten()
            .flatten()
            .any(|e| {
                let n = e.file_name().to_string_lossy().into_owned();
                n == lib || (lib == "libonnxruntime.so" && n.starts_with("libonnxruntime.so"))
            })
    };
    let missing: Vec<&str> = SHARED_LIBS.into_iter().filter(|l| !has_lib(l)).collect();
    let models = paths.data_dir.join("models");
    let items: [(bool, String, String); 5] = [
        (
            missing.is_empty(),
            format!("Bibliothèques sherpa-onnx dans {}", paths.lib_dir.display()),
            format!(
                "manquantes : {} ; lancez `jarvisctl install` ou `jarvisctl update`",
                missing.join(", ")
            ),
        ),
        (
            models.join(WHISPER_MODEL).is_file(),
            "Modèle Whisper".into(),
            "lancez `scripts/download-models.sh all`".into(),
        ),
        (
            models.join(SPEAKER_MODEL).is_file(),
            "Modèle d'empreinte vocale".into(),
            "lancez `scripts/download-models.sh speaker`".into(),
        ),
        (
            paths.data_dir.join("wakeword/jarvis.rpw").is_file(),
            "Mot d'éveil enregistré".into(),
            "lancez `jarvis-ear enroll`".into(),
        ),
        (
            paths.data_dir.join("voiceprint/charles.json").is_file(),
            "Empreinte vocale enregistrée".into(),
            "lancez `jarvis-ear enroll`".into(),
        ),
    ];
    for (ok, label, hint) in items {
        if ok {
            writeln!(out, "✓ {label}")?;
        } else {
            writeln!(out, "! {label} absent — optionnel (oreille du PC) : {hint}")?;
        }
    }
    Ok(())
}

/// Écrit ou remplace la ligne `ANTHROPIC_API_KEY=` ; ne l'affiche jamais.
pub fn set_api_key(paths: &Paths, key: &str, out: &mut dyn Write) -> Result<()> {
    let key = key.trim();
    if key.is_empty() || key.contains(['\n', '\r']) {
        bail!("clé vide ou invalide");
    }
    let new_line = format!("{KEY_VAR}={key}");
    let existing = fs::read_to_string(&paths.env_file).unwrap_or_default();
    let is_var = |l: &str, commented: bool| {
        let l = l.trim_start();
        let l = if commented {
            match l.strip_prefix('#') {
                Some(r) => r.trim_start(),
                None => return false,
            }
        } else {
            l
        };
        l.strip_prefix(KEY_VAR)
            .is_some_and(|r| r.trim_start().starts_with('='))
    };
    let mut lines: Vec<String> = existing.lines().map(String::from).collect();
    if let Some(i) = lines.iter().position(|l| is_var(l, false)) {
        lines[i] = new_line;
    } else if let Some(i) = lines.iter().position(|l| is_var(l, true)) {
        lines[i] = new_line;
    } else {
        lines.push(new_line);
    }
    write_private(&paths.env_file, &(lines.join("\n") + "\n"))?;
    writeln!(
        out,
        "Clé enregistrée dans {}. Lancez `jarvisctl restart` pour l'appliquer.",
        paths.env_file.display()
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system::Output;
    use jarvis_protocol::Mode;
    use std::cell::RefCell;
    use std::io;

    #[derive(Default)]
    struct Fake {
        cmds: RefCell<Vec<String>>,
        replies: HashMap<String, (i32, String)>,
        http: HashMap<String, String>,
        brain: Option<(Mode, String, String)>,
        path: Option<String>,
    }

    impl System for Fake {
        fn run(&self, prog: &str, args: &[&str]) -> io::Result<Output> {
            let cmd = format!("{prog} {}", args.join(" "));
            self.cmds.borrow_mut().push(cmd.clone());
            let (code, stdout) = self
                .replies
                .get(&cmd)
                .cloned()
                .unwrap_or((0, String::new()));
            Ok(Output { code, stdout })
        }
        fn run_inherit(&self, prog: &str, args: &[&str], _: Option<&Path>) -> io::Result<i32> {
            self.cmds
                .borrow_mut()
                .push(format!("{prog} {}", args.join(" ")));
            Ok(0)
        }
        fn http_get(&self, _: &str, path: &str) -> Option<String> {
            self.http.get(path).cloned()
        }
        fn brain_status(&self) -> Option<(Mode, String, String)> {
            self.brain.clone()
        }
        fn env_var(&self, _: &str) -> Option<String> {
            self.path.clone()
        }
    }

    fn setup() -> (tempfile::TempDir, Paths) {
        let home = tempfile::tempdir().unwrap();
        let paths = Paths::from_home(home.path());
        (home, paths)
    }

    fn fake_source(home: &Path) -> PathBuf {
        let src = home.join("src");
        let rel = src.join("target/release");
        fs::create_dir_all(&rel).unwrap();
        for b in BINARIES {
            fs::write(rel.join(b), format!("bin {b}")).unwrap();
        }
        for lib in [
            "libsherpa-onnx-c-api.so",
            "libsherpa-onnx-cxx-api.so",
            "libonnxruntime.so",
            "libonnxruntime.so.1.23.2",
        ] {
            fs::write(rel.join(lib), format!("lib {lib}")).unwrap();
        }
        fs::write(rel.join("libautre.so"), "pas à copier").unwrap();
        src
    }

    fn text(out: Vec<u8>) -> String {
        String::from_utf8(out).unwrap()
    }

    fn mode_of(p: &Path) -> u32 {
        fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    fn do_install(home: &Path, paths: &Paths, sys: &Fake) -> String {
        let opts = InstallOpts {
            source: fake_source(home),
            no_build: true,
        };
        let mut out = Vec::new();
        install(paths, sys, &opts, &mut out).unwrap();
        text(out)
    }

    #[test]
    fn unite_systemd_exacte() {
        let u = brain_unit();
        assert!(u.starts_with("[Unit]\nDescription=Jarvis — cerveau de l'assistant vocal\n"));
        assert!(u.contains("ExecStart=%h/.local/bin/jarvis-brain\n"));
        assert!(u.contains("EnvironmentFile=-%h/.config/jarvis/env\n"));
        assert!(u.contains("Restart=on-failure\nRestartSec=3\n"));
        assert!(u.ends_with("[Install]\nWantedBy=graphical-session.target\n"));
    }

    #[test]
    fn parse_systemctl_show() {
        let m = parse_show(
            "ActiveState=active\nSubState=running\nMainPID=1234\nActiveEnterTimestamp=Wed 2026-10-28 16:28:03 CET\n",
        );
        assert_eq!(m["ActiveState"], "active");
        assert_eq!(m["MainPID"], "1234");
        assert_eq!(since(&m["ActiveEnterTimestamp"]), Some("16:28"));
        assert_eq!(since(""), None);
    }

    #[test]
    fn install_ecrit_unite_env_et_config() {
        let (home, paths) = setup();
        let sys = Fake::default();
        do_install(home.path(), &paths, &sys);

        for b in BINARIES {
            assert_eq!(
                fs::read_to_string(paths.bin_dir.join(b)).unwrap(),
                format!("bin {b}")
            );
            assert_eq!(mode_of(&paths.bin_dir.join(b)), 0o755);
        }
        assert_eq!(fs::read_to_string(&paths.unit_path).unwrap(), brain_unit());
        assert_eq!(mode_of(&paths.env_file), 0o600);
        assert!(
            fs::read_to_string(&paths.env_file)
                .unwrap()
                .contains("# ANTHROPIC_API_KEY=")
        );
        assert!(paths.config_file.is_file());
        let src = installed_source(&paths).unwrap();
        assert_eq!(src, home.path().join("src"));

        let cmds = sys.cmds.borrow();
        let reload = cmds
            .iter()
            .position(|c| c == "systemctl --user daemon-reload")
            .unwrap();
        let enable = cmds
            .iter()
            .position(|c| c == "systemctl --user enable --now jarvis-brain.service")
            .unwrap();
        assert!(reload < enable);
    }

    #[test]
    fn install_ne_remplace_pas_config_existante() {
        let (home, paths) = setup();
        fs::create_dir_all(&paths.config_dir).unwrap();
        fs::write(&paths.config_file, "mode = \"online\"\n").unwrap();
        fs::write(&paths.env_file, "ANTHROPIC_API_KEY=secret\n").unwrap();
        let sys = Fake::default();
        do_install(home.path(), &paths, &sys);
        assert_eq!(
            fs::read_to_string(&paths.config_file).unwrap(),
            "mode = \"online\"\n"
        );
        assert_eq!(
            fs::read_to_string(&paths.env_file).unwrap(),
            "ANTHROPIC_API_KEY=secret\n"
        );
    }

    #[test]
    fn uninstall_sans_purge_garde_la_config() {
        let (home, paths) = setup();
        let sys = Fake::default();
        do_install(home.path(), &paths, &sys);
        let mut out = Vec::new();
        uninstall(
            &paths,
            &sys,
            false,
            false,
            &mut io::BufReader::new(io::empty()),
            &mut out,
        )
        .unwrap();
        for b in BINARIES {
            assert!(!paths.bin_dir.join(b).exists());
        }
        assert!(!paths.unit_path.exists());
        assert!(paths.env_file.exists());
        assert!(paths.config_file.exists());
        let cmds = sys.cmds.borrow();
        assert!(
            cmds.iter()
                .any(|c| c == "systemctl --user stop jarvis-brain.service")
        );
        assert!(
            cmds.iter()
                .any(|c| c == "systemctl --user disable jarvis-brain.service")
        );
    }

    #[test]
    fn uninstall_purge_yes_efface_tout() {
        let (home, paths) = setup();
        let sys = Fake::default();
        do_install(home.path(), &paths, &sys);
        fs::create_dir_all(&paths.state_dir).unwrap();
        fs::create_dir_all(&paths.data_dir).unwrap();
        let mut out = Vec::new();
        uninstall(
            &paths,
            &sys,
            true,
            true,
            &mut io::BufReader::new(io::empty()),
            &mut out,
        )
        .unwrap();
        assert!(!paths.config_dir.exists());
        assert!(!paths.state_dir.exists());
        assert!(!paths.data_dir.exists());
    }

    #[test]
    fn purge_refusee_sans_confirmation() {
        let (home, paths) = setup();
        let sys = Fake::default();
        do_install(home.path(), &paths, &sys);
        let mut out = Vec::new();
        let mut input = io::Cursor::new(b"n\n".to_vec());
        uninstall(&paths, &sys, true, false, &mut input, &mut out).unwrap();
        assert!(paths.config_dir.exists());
    }

    #[test]
    fn set_api_key_remplace_la_ligne() {
        let (_home, paths) = setup();
        fs::create_dir_all(&paths.config_dir).unwrap();
        fs::write(
            &paths.env_file,
            "FOO=1\nANTHROPIC_API_KEY=ancienne\nBAR=2\n",
        )
        .unwrap();
        fs::set_permissions(&paths.env_file, fs::Permissions::from_mode(0o644)).unwrap();
        let mut out = Vec::new();
        set_api_key(&paths, "sk-ant-nouvelle", &mut out).unwrap();
        assert_eq!(
            fs::read_to_string(&paths.env_file).unwrap(),
            "FOO=1\nANTHROPIC_API_KEY=sk-ant-nouvelle\nBAR=2\n"
        );
        assert_eq!(mode_of(&paths.env_file), 0o600);
        let o = text(out);
        assert!(!o.contains("sk-ant-nouvelle"));
        assert!(o.contains("jarvisctl restart"));
    }

    #[test]
    fn status_service_inactif_code_3() {
        let (_home, paths) = setup();
        let mut sys = Fake::default();
        sys.replies.insert(
            "systemctl --user show jarvis-brain.service --property=ActiveState,SubState,MainPID,ActiveEnterTimestamp".into(),
            (0, "ActiveState=inactive\nSubState=dead\nMainPID=0\nActiveEnterTimestamp=\n".into()),
        );
        let mut out = Vec::new();
        assert_eq!(status(&paths, &sys, &mut out).unwrap(), 3);
        let o = text(out);
        assert!(o.contains("Service   : inactif"));
        assert!(o.contains("Ollama    : injoignable"));
        assert!(o.contains("Clé Claude: absente"));
    }

    #[test]
    fn status_service_actif() {
        let (_home, paths) = setup();
        let mut sys = Fake::default();
        sys.replies.insert(
            "systemctl --user show jarvis-brain.service --property=ActiveState,SubState,MainPID,ActiveEnterTimestamp".into(),
            (0, "ActiveState=active\nSubState=running\nMainPID=1234\nActiveEnterTimestamp=Wed 2026-10-28 16:28:03 CET\n".into()),
        );
        sys.brain = Some((Mode::Local, "0.1.0".into(), String::new()));
        sys.http.insert("/api/version".into(), "{}".into());
        sys.http.insert(
            "/api/ps".into(),
            r#"{"models":[{"name":"llama3.2:3b"}]}"#.into(),
        );
        let mut out = Vec::new();
        assert_eq!(status(&paths, &sys, &mut out).unwrap(), 0);
        let o = text(out);
        assert!(o.contains("Service   : actif (pid 1234, depuis 16:28)"));
        assert!(o.contains("Mode      : local (llama3.2:3b)"));
        assert!(o.contains("Ollama    : joignable, modèle chargé"));
    }

    #[test]
    fn doctor_signale_cle_absente() {
        let (home, paths) = setup();
        let sys = Fake::default();
        do_install(home.path(), &paths, &sys);
        let mut out = Vec::new();
        doctor(&paths, &sys, &mut out).unwrap();
        let o = text(out);
        assert!(o.contains("! Clé API Claude absente"), "{o}");
        assert!(o.contains("✓ Binaires dans"), "{o}");
        assert!(o.contains("✗ Ollama joignable"), "{o}");
    }

    #[test]
    fn install_copie_les_bibliotheques_sherpa() {
        let (home, paths) = setup();
        let sys = Fake::default();
        do_install(home.path(), &paths, &sys);
        for lib in [
            "libsherpa-onnx-c-api.so",
            "libsherpa-onnx-cxx-api.so",
            "libonnxruntime.so",
            "libonnxruntime.so.1.23.2",
        ] {
            let p = paths.lib_dir.join(lib);
            assert_eq!(fs::read_to_string(&p).unwrap(), format!("lib {lib}"));
            assert_eq!(mode_of(&p), 0o755);
        }
        assert!(!paths.lib_dir.join("libautre.so").exists());
        // Aucun fichier temporaire ne reste.
        assert_eq!(fs::read_dir(&paths.lib_dir).unwrap().count(), 4);
        assert!(paths.lib_dir.ends_with(".local/lib/jarvis"));
    }

    #[test]
    fn update_recopie_les_bibliotheques() {
        let (home, paths) = setup();
        let sys = Fake::default();
        do_install(home.path(), &paths, &sys);
        let src = home.path().join("src/target/release");
        fs::write(src.join("libonnxruntime.so"), "nouvelle version").unwrap();
        update(&paths, &sys, &mut Vec::new()).unwrap();
        assert_eq!(
            fs::read_to_string(paths.lib_dir.join("libonnxruntime.so")).unwrap(),
            "nouvelle version"
        );
    }

    #[test]
    fn uninstall_supprime_les_bibliotheques() {
        let (home, paths) = setup();
        let sys = Fake::default();
        do_install(home.path(), &paths, &sys);
        assert!(paths.lib_dir.is_dir());
        uninstall(
            &paths,
            &sys,
            false,
            false,
            &mut io::BufReader::new(io::empty()),
            &mut Vec::new(),
        )
        .unwrap();
        assert!(!paths.lib_dir.exists());
    }

    #[test]
    fn doctor_avertit_pour_l_oreille_sans_echec() {
        let (home, paths) = setup();
        let sys = Fake::default();
        // Rien d'installé côté oreille : seulement des avertissements.
        let mut out = Vec::new();
        let sans_oreille = doctor(&paths, &sys, &mut out).unwrap();
        let o = text(out);
        for attendu in [
            "! Bibliothèques sherpa-onnx",
            "! Modèle Whisper absent",
            "! Modèle d'empreinte vocale absent",
            "! Mot d'éveil enregistré absent",
            "! Empreinte vocale enregistrée absent",
        ] {
            assert!(o.contains(attendu), "{attendu}\n{o}");
        }
        // Tout est présent : cases cochées, et le code de sortie ne change pas.
        do_install(home.path(), &paths, &sys);
        let models = paths.data_dir.join("models");
        fs::create_dir_all(&models).unwrap();
        fs::write(models.join(WHISPER_MODEL), "").unwrap();
        fs::write(models.join(SPEAKER_MODEL), "").unwrap();
        fs::create_dir_all(paths.data_dir.join("wakeword")).unwrap();
        fs::write(paths.data_dir.join("wakeword/jarvis.rpw"), "").unwrap();
        fs::create_dir_all(paths.data_dir.join("voiceprint")).unwrap();
        fs::write(paths.data_dir.join("voiceprint/charles.json"), "{}").unwrap();
        let mut out = Vec::new();
        let complet = doctor(&paths, &sys, &mut out).unwrap();
        let o = text(out);
        for attendu in [
            "✓ Bibliothèques sherpa-onnx",
            "✓ Modèle Whisper",
            "✓ Modèle d'empreinte vocale",
            "✓ Mot d'éveil enregistré",
            "✓ Empreinte vocale enregistrée",
        ] {
            assert!(o.contains(attendu), "{attendu}\n{o}");
        }
        assert_eq!(sans_oreille, 1);
        assert_eq!(
            complet, 1,
            "échec Ollama inchangé, aucune dépendance à l'oreille"
        );
    }
}
