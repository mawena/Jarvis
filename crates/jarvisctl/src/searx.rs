//! SearXNG dans Docker : `jarvisctl searx up|down|status` et les vérifications de `doctor`.

use crate::paths::{Paths, SEARXNG_ADDR, SEARXNG_CONTAINER, SEARXNG_IMAGE, SEARXNG_PROJECT};
use crate::system::System;
use anyhow::{Context, Result};
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

const COMPOSE: &str = include_str!("../assets/searxng/compose.yml");
const SETTINGS: &str = include_str!("../assets/searxng/settings.yml");
/// Attente de `/healthz` : 60 essais, une seconde entre chaque.
const HEALTH_TRIES: u32 = 60;
const JSON_PROBE: &str = "/search?q=jarvis&format=json";
const INSTALL_HINT: &str = "sudo apt install docker.io docker-compose-v2";

fn compose_file(paths: &Paths) -> PathBuf {
    paths.searx_dir.join("compose.yml")
}

fn settings_file(paths: &Paths) -> PathBuf {
    paths.searx_dir.join("settings.yml")
}

fn has_compose(sys: &dyn System) -> bool {
    sys.run("docker", &["compose", "version"])
        .is_ok_and(|o| o.code == 0)
}

/// Lance `docker compose -f … -p jarvis-searxng <args>` en affichant sa sortie.
fn compose(paths: &Paths, sys: &dyn System, args: &[&str]) -> Result<i32> {
    let file = compose_file(paths);
    let file = file.to_string_lossy();
    let mut full = vec!["compose", "-f", &file, "-p", SEARXNG_PROJECT];
    full.extend_from_slice(args);
    sys.run_inherit("docker", &full, None)
        .context("impossible de lancer docker")
}

/// Écrit un fichier d'un coup (fichier temporaire puis renommage), avec ces droits.
fn write_file(path: &Path, content: &str, mode: u32) -> Result<()> {
    let name = path.file_name().unwrap().to_string_lossy().into_owned();
    let tmp = path.with_file_name(format!(".{name}.tmp"));
    let _ = fs::remove_file(&tmp);
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(&tmp)
        .with_context(|| format!("écriture de {}", tmp.display()))?;
    f.set_permissions(fs::Permissions::from_mode(mode))?;
    f.write_all(content.as_bytes())?;
    drop(f);
    fs::rename(&tmp, path)?;
    Ok(())
}

/// Secret de 48 caractères hexadécimaux tiré de `/dev/urandom`.
fn random_secret() -> Result<String> {
    let mut bytes = [0u8; 24];
    fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut bytes))
        .context("lecture de /dev/urandom")?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// `compose.yml` toujours réécrit ; `settings.yml` et `.env` créés s'ils manquent.
fn write_files(paths: &Paths, out: &mut dyn Write) -> Result<()> {
    fs::create_dir_all(&paths.searx_dir)
        .with_context(|| format!("création de {}", paths.searx_dir.display()))?;
    write_file(&compose_file(paths), COMPOSE, 0o644)?;
    let settings = settings_file(paths);
    if settings.exists() {
        writeln!(out, "Réglages gardés : {}", settings.display())?;
    } else {
        write_file(&settings, SETTINGS, 0o644)?;
        writeln!(out, "Réglages créés : {}", settings.display())?;
    }
    let env = paths.searx_dir.join(".env");
    if !env.exists() {
        write_file(
            &env,
            &format!("SEARXNG_SECRET={}\n", random_secret()?),
            0o600,
        )?;
    }
    Ok(())
}

fn healthy(sys: &dyn System) -> bool {
    sys.http_get(SEARXNG_ADDR, "/healthz").is_some()
}

/// `jarvisctl searx up` : écrit les fichiers, démarre le conteneur, attend `/healthz`.
pub fn up(paths: &Paths, sys: &dyn System, out: &mut dyn Write) -> Result<i32> {
    if !has_compose(sys) {
        writeln!(
            out,
            "Docker Compose introuvable : installez-le (`{INSTALL_HINT}`), puis relancez `jarvisctl searx up`."
        )?;
        return Ok(1);
    }
    write_files(paths, out)?;
    let code = compose(paths, sys, &["up", "-d"])?;
    if code != 0 {
        writeln!(out, "docker compose a échoué (code {code}).")?;
        return Ok(1);
    }
    for _ in 0..HEALTH_TRIES {
        if healthy(sys) {
            writeln!(out, "SearXNG prêt sur http://{SEARXNG_ADDR}")?;
            return Ok(0);
        }
        sys.sleep(Duration::from_secs(1));
    }
    writeln!(
        out,
        "SearXNG ne répond pas après 60 s : voyez `docker logs {SEARXNG_CONTAINER}`."
    )?;
    Ok(1)
}

/// `jarvisctl searx down` : arrête le conteneur, garde le volume et les fichiers.
pub fn down(paths: &Paths, sys: &dyn System, out: &mut dyn Write) -> Result<i32> {
    let code = if compose_file(paths).is_file() {
        compose(paths, sys, &["down"])?
    } else {
        sys.run_inherit("docker", &["compose", "-p", SEARXNG_PROJECT, "down"], None)
            .context("impossible de lancer docker")?
    };
    if code == 0 {
        writeln!(out, "SearXNG arrêté.")?;
        Ok(0)
    } else {
        writeln!(out, "docker compose a échoué (code {code}).")?;
        Ok(1)
    }
}

fn warn(out: &mut dyn Write, msg: &str) -> Result<()> {
    writeln!(out, "! {msg}")?;
    Ok(())
}

fn line(out: &mut dyn Write, ok: bool, good: &str, bad: &str) -> Result<()> {
    if ok {
        writeln!(out, "✓ {good}")?;
        Ok(())
    } else {
        warn(out, bad)
    }
}

/// Vérifie le conteneur ; renvoie vrai s'il est en marche et répond.
fn container_checks(paths: &Paths, sys: &dyn System, out: &mut dyn Write) -> Result<bool> {
    if !has_compose(sys) {
        warn(
            out,
            &format!("Docker Compose absent — optionnel (recherche) : {INSTALL_HINT}"),
        )?;
        return Ok(false);
    }
    writeln!(out, "✓ Docker Compose présent")?;
    let running = sys
        .run(
            "docker",
            &["inspect", "-f", "{{.State.Running}}", SEARXNG_CONTAINER],
        )
        .is_ok_and(|o| o.code == 0 && o.stdout.trim() == "true");
    if !running {
        warn(
            out,
            "SearXNG arrêté — optionnel (recherche) : lancez `jarvisctl searx up`",
        )?;
        return Ok(false);
    }
    writeln!(out, "✓ SearXNG en marche")?;
    let health = healthy(sys);
    line(
        out,
        health,
        &format!("SearXNG répond sur http://{SEARXNG_ADDR}"),
        &format!("SearXNG ne répond pas — voyez `docker logs {SEARXNG_CONTAINER}`"),
    )?;
    if health {
        line(
            out,
            sys.http_get_slow(SEARXNG_ADDR, JSON_PROBE).is_some(),
            "Format JSON de SearXNG actif",
            &format!(
                "Format JSON de SearXNG désactivé (ou recherche trop lente) — ajoutez `json` à `search.formats` dans {}, puis `docker restart {SEARXNG_CONTAINER}`",
                settings_file(paths).display()
            ),
        )?;
    }
    let ports = sys
        .run("docker", &["port", SEARXNG_CONTAINER, "8080/tcp"])
        .map(|o| o.stdout)
        .unwrap_or_default();
    let bound: Vec<&str> = ports
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if bound.is_empty() {
        warn(
            out,
            "Port de SearXNG non publié — lancez `jarvisctl searx up`",
        )?;
    } else {
        line(
            out,
            bound.iter().all(|a| a.starts_with("127.0.0.1:")),
            "Port de SearXNG lié à 127.0.0.1 seulement",
            &format!(
                "Port de SearXNG exposé au réseau ({}) — lancez `jarvisctl searx up` pour le lier à 127.0.0.1",
                bound.join(", ")
            ),
        )?;
    }
    Ok(health)
}

/// `jarvisctl searx status` : code 3 si SearXNG n'est pas en marche ou ne répond pas.
pub fn status(paths: &Paths, sys: &dyn System, out: &mut dyn Write) -> Result<i32> {
    writeln!(out, "Image   : {SEARXNG_IMAGE}")?;
    writeln!(out, "Dossier : {}", paths.searx_dir.display())?;
    Ok(if container_checks(paths, sys, out)? {
        0
    } else {
        3
    })
}

/// Recherche et vidéos dans `doctor` : avertissements seulement (la recherche est facultative).
pub fn diagnostics(paths: &Paths, sys: &dyn System, out: &mut dyn Write) -> Result<()> {
    container_checks(paths, sys, out)?;
    match sys.run("yt-dlp", &["--version"]) {
        Ok(o) if o.code == 0 => match o.stdout.trim() {
            "" => writeln!(out, "✓ yt-dlp présent")?,
            v => writeln!(out, "✓ yt-dlp présent ({v})")?,
        },
        _ => writeln!(
            out,
            "! yt-dlp absent — optionnel (vidéos) : sudo apt install yt-dlp"
        )?,
    }
    let xdg = sys
        .run("sh", &["-c", "command -v xdg-open"])
        .is_ok_and(|o| o.code == 0);
    line(
        out,
        xdg,
        "xdg-open présent",
        "xdg-open absent — optionnel (navigateur) : sudo apt install xdg-utils",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::{SEARXNG_ADDR, SEARXNG_IMAGE};
    use crate::system::fake::Fake;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    const RUNNING: &str = "docker inspect -f {{.State.Running}} jarvis-searxng";
    const PORT: &str = "docker port jarvis-searxng 8080/tcp";
    const JSON: &str = "/search?q=jarvis&format=json";

    fn setup() -> (tempfile::TempDir, Paths) {
        let home = tempfile::tempdir().unwrap();
        let paths = Paths::from_home(home.path());
        (home, paths)
    }

    fn healthy() -> Fake {
        let mut sys = Fake::default();
        sys.http.insert("/healthz".into(), "OK".into());
        sys
    }

    /// SearXNG en marche, sain, JSON actif, port local.
    fn complete() -> Fake {
        let mut sys = healthy();
        sys.http.insert(JSON.into(), "{\"results\": []}".into());
        sys.replies.insert(RUNNING.into(), (0, "true\n".into()));
        sys.replies
            .insert(PORT.into(), (0, format!("{SEARXNG_ADDR}\n")));
        sys.replies
            .insert("yt-dlp --version".into(), (0, "2024.04.09\n".into()));
        sys
    }

    fn mode_of(p: &Path) -> u32 {
        fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    fn run_up(paths: &Paths, sys: &Fake) -> (i32, String) {
        let mut out = Vec::new();
        let code = up(paths, sys, &mut out).unwrap();
        (code, String::from_utf8(out).unwrap())
    }

    fn doctor_out(paths: &Paths, sys: &Fake) -> (i32, String) {
        let mut out = Vec::new();
        let code = crate::ops::doctor(paths, sys, &mut out).unwrap();
        (code, String::from_utf8(out).unwrap())
    }

    fn compose_cmd(paths: &Paths, verb: &str) -> String {
        format!(
            "docker compose -f {} -p jarvis-searxng {verb}",
            paths.searx_dir.join("compose.yml").display()
        )
    }

    #[test]
    fn searx_up_ecrit_compose_reglages_et_secret() {
        let (_home, paths) = setup();
        let (code, o) = run_up(&paths, &healthy());
        assert_eq!(code, 0, "{o}");
        let dir = &paths.searx_dir;

        let compose = fs::read_to_string(dir.join("compose.yml")).unwrap();
        assert!(
            compose.contains(&format!("image: {SEARXNG_IMAGE}")),
            "{compose}"
        );
        assert!(compose.contains("\"127.0.0.1:8888:8080\""), "{compose}");
        assert!(compose.contains("géré par jarvisctl") || compose.contains("Géré par jarvisctl"));
        assert!(compose.contains("./settings.yml:/etc/searxng/settings.yml:ro"));
        assert!(compose.contains("FORCE_OWNERSHIP=false"));
        assert!(compose.contains("restart: unless-stopped"));
        // Aucun autre port publié.
        assert_eq!(compose.matches(":8080").count(), 1, "{compose}");

        let settings = fs::read_to_string(dir.join("settings.yml")).unwrap();
        let formats = settings.split("formats:").nth(1).unwrap();
        assert!(
            formats.lines().take(3).any(|l| l.trim() == "- json"),
            "{settings}"
        );
        assert!(settings.contains("limiter: false"));
        assert!(settings.contains("enable_metrics: false"));
        assert_eq!(mode_of(&dir.join("settings.yml")), 0o644);

        let env = fs::read_to_string(dir.join(".env")).unwrap();
        let secret = env.trim().strip_prefix("SEARXNG_SECRET=").unwrap();
        assert_eq!(secret.len(), 48, "{env}");
        assert!(secret.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(mode_of(&dir.join(".env")), 0o600);
        assert!(!o.contains(secret), "secret jamais affiché : {o}");

        // Un deuxième tirage donne un autre secret.
        let (_home2, other) = setup();
        run_up(&other, &healthy());
        let env2 = fs::read_to_string(other.searx_dir.join(".env")).unwrap();
        assert_ne!(env, env2);
    }

    #[test]
    fn searx_up_garde_reglages_et_secret_existants() {
        let (_home, paths) = setup();
        let dir = &paths.searx_dir;
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join("settings.yml"), "# à moi\n").unwrap();
        fs::write(dir.join(".env"), "SEARXNG_SECRET=le-mien\n").unwrap();
        fs::set_permissions(dir.join(".env"), fs::Permissions::from_mode(0o600)).unwrap();
        fs::write(dir.join("compose.yml"), "ancien").unwrap();

        let (code, o) = run_up(&paths, &healthy());
        assert_eq!(code, 0, "{o}");
        assert_eq!(
            fs::read_to_string(dir.join("settings.yml")).unwrap(),
            "# à moi\n"
        );
        assert_eq!(
            fs::read_to_string(dir.join(".env")).unwrap(),
            "SEARXNG_SECRET=le-mien\n"
        );
        // compose.yml, lui, est toujours réécrit.
        let compose = fs::read_to_string(dir.join("compose.yml")).unwrap();
        assert!(compose.contains(SEARXNG_IMAGE));
        assert!(!o.contains("le-mien"), "{o}");
    }

    #[test]
    fn searx_up_lance_docker_compose() {
        let (_home, paths) = setup();
        let sys = healthy();
        let (code, o) = run_up(&paths, &sys);
        assert_eq!(code, 0, "{o}");
        let cmds = sys.cmds.borrow();
        assert_eq!(cmds[0], "docker compose version");
        assert!(cmds.contains(&compose_cmd(&paths, "up -d")), "{cmds:?}");
        assert!(o.contains("http://127.0.0.1:8888"), "{o}");
    }

    #[test]
    fn searx_up_sans_docker_message_clair() {
        let (_home, paths) = setup();
        let mut sys = healthy();
        sys.replies
            .insert("docker compose version".into(), (127, String::new()));
        let (code, o) = run_up(&paths, &sys);
        assert_eq!(code, 1);
        assert!(o.contains("Docker Compose introuvable"), "{o}");
        assert!(o.contains("sudo apt install"), "{o}");
        assert_eq!(sys.cmds.borrow().len(), 1, "rien d'autre n'est lancé");
        assert!(!paths.searx_dir.exists(), "aucun fichier écrit");
    }

    #[test]
    fn searx_up_attend_healthz() {
        let (_home, paths) = setup();
        let sys = healthy();
        sys.http_misses.set(5);
        let (code, o) = run_up(&paths, &sys);
        assert_eq!(code, 0, "{o}");
        assert_eq!(sys.sleeps.get(), 5);
        assert!(o.contains("SearXNG prêt"), "{o}");
    }

    #[test]
    fn searx_up_sans_healthz_code_1() {
        let (_home, paths) = setup();
        let sys = Fake::default();
        let (code, o) = run_up(&paths, &sys);
        assert_eq!(code, 1);
        assert_eq!(sys.sleeps.get(), 60, "environ 60 s d'attente");
        assert!(o.contains("ne répond pas"), "{o}");
        assert!(o.contains("docker logs jarvis-searxng"), "{o}");
    }

    #[test]
    fn searx_down_arrete_sans_effacer() {
        let (_home, paths) = setup();
        run_up(&paths, &healthy());
        let sys = Fake::default();
        let mut out = Vec::new();
        assert_eq!(down(&paths, &sys, &mut out).unwrap(), 0);
        let cmds = sys.cmds.borrow();
        assert_eq!(*cmds, vec![compose_cmd(&paths, "down")]);
        assert!(!cmds[0].contains("-v"), "le volume est gardé");
        for f in ["compose.yml", "settings.yml", ".env"] {
            assert!(paths.searx_dir.join(f).is_file(), "{f}");
        }
    }

    #[test]
    fn searx_status_en_marche_ou_arrete() {
        let (_home, paths) = setup();
        let mut out = Vec::new();
        assert_eq!(status(&paths, &complete(), &mut out).unwrap(), 0);
        let o = String::from_utf8(out).unwrap();
        assert!(o.contains("✓ SearXNG en marche"), "{o}");

        let mut out = Vec::new();
        assert_eq!(status(&paths, &Fake::default(), &mut out).unwrap(), 3);
        let o = String::from_utf8(out).unwrap();
        assert!(o.contains("! SearXNG arrêté"), "{o}");
    }

    #[test]
    fn doctor_searxng_arrete_avertit_sans_echec() {
        let (_home, paths) = setup();
        let (code_arrete, o) = doctor_out(&paths, &Fake::default());
        assert!(o.contains("✓ Docker Compose présent"), "{o}");
        assert!(o.contains("! SearXNG arrêté"), "{o}");
        assert!(o.contains("`jarvisctl searx up`"), "{o}");
        assert!(
            !o.lines()
                .any(|l| l.starts_with('✗') && l.contains("SearXNG")),
            "{o}"
        );

        let mut sans_docker = Fake::default();
        sans_docker
            .replies
            .insert("docker compose version".into(), (127, String::new()));
        let (code_sans_docker, o) = doctor_out(&paths, &sans_docker);
        assert!(o.contains("! Docker Compose absent"), "{o}");
        assert!(!o.contains("SearXNG arrêté"), "{o}");

        let (code_complet, _) = doctor_out(&paths, &complete());
        assert_eq!(code_arrete, code_complet, "code de sortie inchangé");
        assert_eq!(code_sans_docker, code_complet, "code de sortie inchangé");
    }

    #[test]
    fn doctor_port_expose_avertit() {
        let (_home, paths) = setup();
        let mut sys = complete();
        sys.replies
            .insert(PORT.into(), (0, "0.0.0.0:8888\n[::]:8888\n".into()));
        let (code, o) = doctor_out(&paths, &sys);
        assert!(o.contains("! Port de SearXNG exposé"), "{o}");
        assert!(o.contains("0.0.0.0:8888"), "{o}");
        assert_eq!(code, doctor_out(&paths, &complete()).0);

        // Un lien local plus un lien ouvert : toujours exposé.
        sys.replies
            .insert(PORT.into(), (0, "127.0.0.1:8888\n[::]:8888\n".into()));
        let (_, o) = doctor_out(&paths, &sys);
        assert!(o.contains("! Port de SearXNG exposé"), "{o}");
    }

    #[test]
    fn doctor_json_desactive_avertit() {
        let (_home, paths) = setup();
        let mut sys = complete();
        sys.http.remove(JSON);
        let (code, o) = doctor_out(&paths, &sys);
        assert!(o.contains("✓ SearXNG répond"), "{o}");
        assert!(o.contains("! Format JSON de SearXNG désactivé"), "{o}");
        assert!(o.contains("settings.yml"), "{o}");
        assert_eq!(code, doctor_out(&paths, &complete()).0);
    }

    #[test]
    fn doctor_searxng_complet() {
        let (_home, paths) = setup();
        let (_, o) = doctor_out(&paths, &complete());
        for attendu in [
            "✓ Docker Compose présent",
            "✓ SearXNG en marche",
            "✓ SearXNG répond",
            "✓ Format JSON de SearXNG actif",
            "✓ Port de SearXNG lié à 127.0.0.1 seulement",
            "✓ yt-dlp présent (2024.04.09)",
            "✓ xdg-open présent",
        ] {
            assert!(o.contains(attendu), "{attendu}\n{o}");
        }

        let mut sys = complete();
        sys.replies
            .insert("yt-dlp --version".into(), (127, String::new()));
        sys.replies
            .insert("sh -c command -v xdg-open".into(), (1, String::new()));
        let (_, o) = doctor_out(&paths, &sys);
        assert!(o.contains("! yt-dlp absent"), "{o}");
        assert!(o.contains("! xdg-open absent"), "{o}");
    }
}
