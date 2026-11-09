//! Lancement et surveillance du processus de l'agent : groupe de processus à part,
//! journal complet, suivi en direct, arrêt, durée maximale et silence trop long.

use super::claude::{Invocation, StreamParser};
use super::settings::AgentSettings;
use super::types::{EndReason, Progress};
use std::fmt;
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::process::{ExitStatus, Stdio};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStderr, ChildStdout, Command};
use tokio::sync::{Mutex, Notify, watch};
use tokio::task::JoinHandle;
use tokio::time::{Instant, sleep_until, timeout, timeout_at};

/// Délai entre `TERM` et `KILL`.
pub const GRACE: Duration = Duration::from_secs(10);

/// Taille maximale d'un journal de tâche.
const LOG_MAX: u64 = 20 * 1024 * 1024;

/// Limites de durée d'une tâche.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Durée maximale.
    pub max_run: Duration,
    /// Sans aucune sortie pendant ce temps : bloquée, arrêtée.
    pub idle: Duration,
    /// Délai entre `TERM` et `KILL`.
    pub grace: Duration,
}

impl Limits {
    /// Limites tirées des réglages (`max_minutes`, `idle_minutes`).
    pub fn from_settings(settings: &AgentSettings) -> Limits {
        Limits {
            max_run: Duration::from_secs(settings.max_minutes * 60),
            idle: Duration::from_secs(settings.idle_minutes * 60),
            grace: GRACE,
        }
    }
}

/// Fin d'une tâche.
#[derive(Debug, Clone, PartialEq)]
pub struct RunOutcome {
    pub reason: EndReason,
    /// Code de sortie ; `None` si tué par un signal.
    pub exit_code: Option<i32>,
    pub progress: Progress,
}

/// Demande l'arrêt d'une tâche. Idempotent ; sans effet une fois la tâche finie.
#[derive(Clone)]
pub struct Stopper(Arc<watch::Sender<bool>>);

impl Stopper {
    pub fn stop(&self) {
        self.0.send_replace(true);
    }
}

impl fmt::Debug for Stopper {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Stopper").field(&*self.0.borrow()).finish()
    }
}

/// Agent en cours.
pub struct Running {
    /// Pid du processus, aussi l'id de son groupe.
    pub pid: u32,
    progress: watch::Receiver<Progress>,
    stopper: Stopper,
    handle: JoinHandle<RunOutcome>,
}

impl Running {
    /// Suivi à cet instant.
    pub fn progress(&self) -> Progress {
        self.progress.borrow().clone()
    }

    pub fn stopper(&self) -> Stopper {
        self.stopper.clone()
    }

    /// Suivi en direct, lisible après `wait` lancé ailleurs.
    pub fn progress_watch(&self) -> watch::Receiver<Progress> {
        self.progress.clone()
    }

    /// Attend la fin du processus (et de tout son groupe).
    pub async fn wait(self) -> RunOutcome {
        let Running {
            progress, handle, ..
        } = self;
        match handle.await {
            Ok(outcome) => outcome,
            Err(_) => RunOutcome {
                reason: EndReason::Exited,
                exit_code: None,
                progress: progress.borrow().clone(),
            },
        }
    }
}

/// Lance l'agent dans son propre groupe de processus et le surveille dans une
/// tâche tokio. À appeler depuis le runtime tokio. Le journal `log` reçoit un
/// en-tête, chaque ligne de sortie et les erreurs préfixées `[stderr] `.
pub fn spawn(inv: Invocation, log: &Path, limits: Limits) -> io::Result<Running> {
    if !inv.cwd.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("dossier de travail introuvable : {}", inv.cwd.display()),
        ));
    }
    let mut file = open_log(log)?;
    file.write_all(header(&inv).as_bytes())?;
    // Sans eux, le bac à sable n'aurait rien à rendre inscriptible.
    for dir in &inv.create_dirs {
        if let Err(e) = std::fs::create_dir_all(dir) {
            let _ = writeln!(
                file,
                "# dossier impossible à créer : {} ({e})",
                dir.display()
            );
        }
    }

    let mut cmd = Command::new(&inv.program);
    // Environnement fermé : seulement ce que `claude::build` a choisi.
    cmd.args(&inv.args)
        .current_dir(&inv.cwd)
        .env_clear()
        .envs(inv.env.iter().map(|(k, v)| (k, v)))
        .process_group(0)
        .kill_on_drop(true)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            let err = launch_error(&inv.program, e);
            let _ = writeln!(file, "# {err}");
            return Err(err);
        }
    };
    let pid = child.id().unwrap_or(0);
    let guard = GroupGuard(pid);

    // Consigne sur l'entrée standard, puis fermée.
    if let Some(mut stdin) = child.stdin.take() {
        let text = inv.stdin;
        tokio::spawn(async move {
            let _ = stdin.write_all(text.as_bytes()).await;
            let _ = stdin.shutdown().await;
        });
    }

    let written = file.metadata().map(|m| m.len()).unwrap_or(0);
    let log = Arc::new(Mutex::new(Log {
        file: tokio::fs::File::from_std(file),
        written,
        full: false,
    }));
    let activity = Arc::new(Notify::new());
    let (progress_tx, progress_rx) = watch::channel(Progress::default());
    let mut readers = Vec::new();
    if let Some(out) = child.stdout.take() {
        readers.push(tokio::spawn(read_stdout(
            out,
            log.clone(),
            progress_tx,
            activity.clone(),
        )));
    }
    if let Some(err) = child.stderr.take() {
        readers.push(tokio::spawn(read_stderr(
            err,
            log.clone(),
            activity.clone(),
        )));
    }

    let stop = Arc::new(watch::Sender::new(false));
    let watched = Watched {
        child,
        guard,
        pid,
        limits,
        stop: stop.clone(),
        activity,
        readers,
        log,
        progress: progress_rx.clone(),
    };
    Ok(Running {
        pid,
        progress: progress_rx,
        stopper: Stopper(stop),
        handle: tokio::spawn(supervise(watched)),
    })
}

/// Erreur de lancement lisible.
fn launch_error(program: &Path, e: io::Error) -> io::Error {
    let text = match e.kind() {
        io::ErrorKind::NotFound => format!("programme introuvable : {}", program.display()),
        io::ErrorKind::PermissionDenied => {
            format!("programme non exécutable : {}", program.display())
        }
        _ => format!("lancement impossible de {} : {e}", program.display()),
    };
    io::Error::new(e.kind(), text)
}

/// Journal privé (0600), en ajout.
fn open_log(path: &Path) -> io::Result<std::fs::File> {
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)?;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    Ok(file)
}

/// En-tête du journal : id, programme, arguments (sans les consignes système),
/// dossier, heure.
fn header(inv: &Invocation) -> String {
    let id = inv
        .args
        .iter()
        .position(|a| a == "--session-id")
        .and_then(|i| inv.args.get(i + 1))
        .map_or("?", String::as_str);
    let mut args = Vec::new();
    let mut hide = false;
    for a in &inv.args {
        if hide {
            args.push("<consignes de Jarvis>".to_string());
            hide = false;
            continue;
        }
        hide = a == "--append-system-prompt";
        args.push(quote(a));
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    format!(
        "# tâche {id}\n# programme : {}\n# arguments : {}\n# dossier : {}\n# début : {}\n",
        inv.program.display(),
        args.join(" "),
        inv.cwd.display(),
        utc(now)
    )
}

/// Argument lisible dans le journal, entre apostrophes si besoin.
fn quote(arg: &str) -> String {
    let plain = !arg.is_empty()
        && arg
            .chars()
            .all(|c| c.is_alphanumeric() || "-_=./,:()*+@%".contains(c));
    if plain {
        arg.to_string()
    } else {
        format!("'{}'", arg.replace('\'', r"'\''"))
    }
}

/// Date et heure UTC d'une heure Unix (« 2026-10-29 14:03:12 UTC »).
fn utc(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Algorithme « civil from days » de Howard Hinnant.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02} UTC",
        rem / 3_600,
        rem % 3_600 / 60,
        rem % 60
    )
}

/// Journal d'une tâche, borné à [`LOG_MAX`].
struct Log {
    file: tokio::fs::File,
    written: u64,
    full: bool,
}

impl Log {
    async fn line(&mut self, text: &str) {
        if self.full {
            return;
        }
        let mut data = format!("{text}\n");
        if self.written + data.len() as u64 > LOG_MAX {
            data = "[journal tronqué : 20 Mo atteints]\n".to_string();
            self.full = true;
        }
        let ok =
            self.file.write_all(data.as_bytes()).await.is_ok() && self.file.flush().await.is_ok();
        if !ok {
            self.full = true;
        }
        self.written += data.len() as u64;
    }
}

type SharedLog = Arc<Mutex<Log>>;

/// Ligne suivante, sans fin de ligne ; `None` à la fin du flux.
async fn next_line<R: AsyncBufRead + Unpin>(reader: &mut R, buf: &mut Vec<u8>) -> Option<String> {
    buf.clear();
    match reader.read_until(b'\n', buf).await {
        Ok(0) | Err(_) => None,
        Ok(_) => {
            let text = String::from_utf8_lossy(buf);
            Some(text.trim_end_matches(['\n', '\r']).to_string())
        }
    }
}

/// Chaque ligne du flux : journal, analyseur, suivi.
async fn read_stdout(
    out: ChildStdout,
    log: SharedLog,
    progress: watch::Sender<Progress>,
    activity: Arc<Notify>,
) {
    let mut reader = BufReader::new(out);
    let mut buf = Vec::new();
    let mut parser = StreamParser::default();
    while let Some(line) = next_line(&mut reader, &mut buf).await {
        log.lock().await.line(&line).await;
        parser.feed(&line);
        progress.send_replace(parser.progress().clone());
        activity.notify_one();
    }
}

/// Chaque ligne d'erreur : journal, préfixée `[stderr] `.
async fn read_stderr(err: ChildStderr, log: SharedLog, activity: Arc<Notify>) {
    let mut reader = BufReader::new(err);
    let mut buf = Vec::new();
    while let Some(line) = next_line(&mut reader, &mut buf).await {
        log.lock().await.line(&format!("[stderr] {line}")).await;
        activity.notify_one();
    }
}

/// Ce que surveille la tâche de supervision.
struct Watched {
    child: Child,
    /// Tue tout le groupe si la supervision est abandonnée en route.
    guard: GroupGuard,
    pid: u32,
    limits: Limits,
    /// Gardé ici : l'arrêt reste possible même si tous les `Stopper` sont lâchés.
    stop: Arc<watch::Sender<bool>>,
    activity: Arc<Notify>,
    readers: Vec<JoinHandle<()>>,
    log: SharedLog,
    progress: watch::Receiver<Progress>,
}

async fn supervise(w: Watched) -> RunOutcome {
    let Watched {
        mut child,
        guard,
        pid,
        limits,
        stop,
        activity,
        readers,
        log,
        progress,
    } = w;
    let mut stop_rx = stop.subscribe();
    let deadline = Instant::now() + limits.max_run;
    let mut idle_at = Instant::now() + limits.idle;
    let ended: Result<io::Result<ExitStatus>, EndReason> = loop {
        tokio::select! {
            biased;
            status = child.wait() => break Ok(status),
            _ = stop_rx.wait_for(|stop| *stop) => break Err(EndReason::Stopped),
            _ = sleep_until(deadline) => break Err(EndReason::TimedOut),
            _ = activity.notified() => idle_at = Instant::now() + limits.idle,
            _ = sleep_until(idle_at) => break Err(EndReason::Idle),
        }
    };
    let (reason, status) = match ended {
        Ok(status) => (EndReason::Exited, status.ok()),
        Err(reason) => {
            // `TERM` à Claude Code seul : il arrête lui-même ses commandes Bash
            // et sort. Le groupe entier ne reçoit `KILL` qu'ensuite.
            signal(&pid.to_string(), "TERM").await;
            let status = match timeout(limits.grace, child.wait()).await {
                Ok(status) => status.ok(),
                Err(_) => {
                    kill_group(pid, "KILL").await;
                    child.wait().await.ok()
                }
            };
            (reason, status)
        }
    };
    // Restes du groupe : un `cargo test` laissé en arrière-plan par l'agent.
    kill_group(pid, "KILL").await;
    guard.disarm();

    // Fin du flux : les dernières lignes sont déjà dans les tubes.
    let until = Instant::now() + limits.grace;
    for mut reader in readers {
        if timeout_at(until, &mut reader).await.is_err() {
            reader.abort();
        }
    }

    let exit_code = status.and_then(|s| s.code());
    let code = exit_code.map_or("tué par un signal".to_string(), |c| format!("code {c}"));
    log.lock()
        .await
        .line(&format!("# fin : {}, {code}", reason_text(reason)))
        .await;
    let progress = progress.borrow().clone();
    RunOutcome {
        reason,
        exit_code,
        progress,
    }
}

fn reason_text(reason: EndReason) -> &'static str {
    match reason {
        EndReason::Exited => "sorti de lui-même",
        EndReason::Stopped => "arrêt demandé",
        EndReason::TimedOut => "durée maximale dépassée",
        EndReason::Idle => "silence trop long",
    }
}

/// Signal à tout le groupe par le `kill` du système.
async fn kill_group(pid: u32, sig: &str) {
    // Jamais `kill -- -0` : ce serait notre propre groupe.
    if pid == 0 {
        return;
    }
    signal(&format!("-{pid}"), sig).await;
}

/// `kill -s <sig> -- <cible>` : un pid, ou `-<pgid>` pour un groupe.
async fn signal(target: &str, sig: &str) {
    if target == "0" || target == "-0" {
        return;
    }
    let _ = Command::new("kill")
        .args(["-s", sig, "--", target])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await;
}

/// Groupe de l'agent, tué (`KILL`) si la supervision est lâchée avant la fin :
/// arrêt du cerveau, tâche annulée, panique d'un test.
struct GroupGuard(u32);

impl GroupGuard {
    /// Fin normale : le groupe est déjà tué, son id peut resservir.
    fn disarm(mut self) {
        self.0 = 0;
    }
}

impl Drop for GroupGuard {
    fn drop(&mut self) {
        if self.0 == 0 {
            return;
        }
        // Appel bloquant très court : plus rien ne peut être attendu ici.
        let _ = std::process::Command::new("kill")
            .args(["-s", "KILL", "--", &format!("-{}", self.0)])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::claude;
    use crate::agent::settings::AgentSettings;
    use crate::agent::testing::{fake_claude, init_line, result_line, shell_quote, tool_use_line};
    use crate::agent::types::{TaskId, TaskRecord, TaskState};
    use serde_json::json;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::time::Instant;
    use tempfile::TempDir;

    const LONG: Duration = Duration::from_secs(30);

    fn limits() -> Limits {
        Limits {
            max_run: LONG,
            idle: LONG,
            grace: Duration::from_secs(2),
        }
    }

    /// Faux `claude` qui exécute `body`, et son invocation complète.
    fn setup(body: &str) -> (TempDir, Invocation, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let program = fake_claude(dir.path(), body);
        let worktree = dir.path().join("wt");
        std::fs::create_dir(&worktree).unwrap();
        let settings = AgentSettings {
            claude_path: program,
            cargo_target_dir: dir.path().join("target"),
            cargo_home: dir.path().join("cargo"),
            ..AgentSettings::default()
        };
        let task = TaskRecord {
            id: TaskId("0b6e2f3c-1d2e-4f5a-8b9c-0d1e2f3a4b5c".into()),
            project: "Jarvis".into(),
            instruction: "Finis les tests, « vite ».\nMerci".into(),
            branch: "feature/jarvis-finis-tests".into(),
            worktree,
            base: "develop".into(),
            base_sha: "0123".into(),
            started_at: 0,
            ended_at: None,
            state: TaskState::Running,
            summary: None,
            files_changed: None,
            violations: vec![],
            cost_usd: None,
            log: dir.path().join("tache.log"),
        };
        let inv = claude::build(&settings, &settings.projects[0], &task, vec![], vec![]);
        let log = task.log.clone();
        (dir, inv, log)
    }

    fn print_line(line: &str) -> String {
        format!("printf '%s\\n' {}", shell_quote(line))
    }

    fn alive(pid: &str) -> bool {
        std::process::Command::new("kill")
            .args(["-0", pid])
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success()
    }

    /// Attend qu'un fichier écrit par le faux agent apparaisse.
    async fn wait_file(path: &Path) -> String {
        for _ in 0..200 {
            if let Ok(text) = std::fs::read_to_string(path)
                && !text.trim().is_empty()
            {
                return text.trim().to_string();
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("{} jamais écrit", path.display());
    }

    #[tokio::test]
    async fn processus_termine_normalement() {
        let body = [
            print_line(&init_line("abc")),
            print_line(&tool_use_line(
                "Bash",
                json!({"command": "cargo test --all"}),
            )),
            print_line(&result_line(true, "Fini.\nRÉSUMÉ : Tout passe.")),
            "exit 0".into(),
        ]
        .join("\n");
        let (dir, mut inv, log) = setup(&body);
        inv.env.push(("JARVIS_ESSAI".into(), "1".into()));
        inv.env.retain(|(k, _)| k != "HOME");
        let running = spawn(inv.clone(), &log, limits()).unwrap();
        assert!(running.pid > 0);
        let out = running.wait().await;
        assert_eq!(out.reason, EndReason::Exited);
        assert_eq!(out.exit_code, Some(0));
        assert_eq!(out.progress.lines, 3);
        assert_eq!(
            out.progress.last_action.as_deref(),
            Some("lance « cargo test --all »")
        );
        let result = out.progress.result.unwrap();
        assert_eq!(result.text, "Fini.\nRÉSUMÉ : Tout passe.");

        let read = |f: &str| std::fs::read_to_string(dir.path().join(f)).unwrap();
        // Consigne passée par l'entrée, jamais en argument.
        assert_eq!(read("stdin"), inv.stdin);
        let args: Vec<String> = read("args").lines().map(String::from).collect();
        assert_eq!(args.first().map(String::as_str), Some("-p"));
        assert!(args.iter().all(|a| !a.contains("Finis les tests")));
        let env = read("env");
        assert!(env.contains("JARVIS_ESSAI=1"));
        assert!(!env.lines().any(|l| l.starts_with("HOME=")));
        // Environnement fermé : rien du processus de test ne passe sans être choisi.
        let names: Vec<&str> = env.lines().filter_map(|l| l.split('=').next()).collect();
        for k in [
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "SSH_AUTH_SOCK",
            "DBUS_SESSION_BUS_ADDRESS",
            "XDG_RUNTIME_DIR",
            "CARGO_PKG_NAME",
            "RUST_TEST_THREADS",
        ] {
            assert!(!names.contains(&k), "{k}");
        }
        for (k, _) in &inv.env {
            assert!(names.contains(&k.as_str()), "{k}");
        }
        // `sh` ajoute au plus `PWD`, `OLDPWD`, `SHLVL` et `_`.
        let extra: Vec<&&str> = names
            .iter()
            .filter(|n| !inv.env.iter().any(|(k, _)| k == *n))
            .filter(|n| !["PWD", "OLDPWD", "SHLVL", "_"].contains(n))
            .collect();
        assert!(extra.is_empty(), "{extra:?}");
        assert!(env.contains("GIT_TERMINAL_PROMPT=0"));
        assert!(env.contains("BASH_MAX_TIMEOUT_MS=1500000"));
        // Dossiers de compilation créés pour le bac à sable.
        assert!(dir.path().join("target/Jarvis").is_dir());
        assert!(dir.path().join("cargo").is_dir());
    }

    #[tokio::test]
    async fn code_de_sortie_transmis() {
        let (_dir, inv, log) = setup("echo cassé >&2\nexit 3");
        let out = spawn(inv, &log, limits()).unwrap().wait().await;
        assert_eq!(out.reason, EndReason::Exited);
        assert_eq!(out.exit_code, Some(3));
        assert_eq!(out.progress.result, None);
    }

    #[tokio::test]
    async fn journal_contient_sortie_et_erreurs() {
        let body = [
            print_line(&init_line("abc")),
            "echo oups >&2".into(),
            print_line("pas du json"),
        ]
        .join("\n");
        let (_dir, inv, log) = setup(&body);
        let out = spawn(inv.clone(), &log, limits()).unwrap().wait().await;
        assert_eq!(out.reason, EndReason::Exited);
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(
            text.contains("0b6e2f3c-1d2e-4f5a-8b9c-0d1e2f3a4b5c"),
            "{text}"
        );
        assert!(text.contains(&inv.program.display().to_string()), "{text}");
        assert!(text.contains("--append-system-prompt"), "{text}");
        assert!(text.contains(&init_line("abc")), "{text}");
        assert!(text.contains("[stderr] oups"), "{text}");
        assert!(text.contains("pas du json"), "{text}");
        // Ni les consignes système, ni la consigne de Charles dans l'en-tête.
        assert!(!text.contains("Lance les tests avant de finir"), "{text}");
        let mode = std::fs::metadata(&log).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[tokio::test]
    async fn arret_tue_tout_le_groupe() {
        let (dir, inv, log) = setup("");
        let child = dir.path().join("enfant");
        let body = format!(
            "sleep 1000 &\necho $! > {}\n{}\nwait",
            shell_quote(&child.display().to_string()),
            print_line(&init_line("abc"))
        );
        let program = fake_claude(dir.path(), &body);
        let inv = Invocation { program, ..inv };
        let running = spawn(inv, &log, limits()).unwrap();
        let pid = wait_file(&child).await;
        assert!(alive(&pid));
        let stopper = running.stopper();
        stopper.stop();
        stopper.stop();
        let started = Instant::now();
        let out = running.wait().await;
        assert_eq!(out.reason, EndReason::Stopped);
        assert!(started.elapsed() < Duration::from_secs(2));
        // Le petit-enfant meurt aussi (le temps qu'init le ramasse).
        let mut dead = false;
        for _ in 0..100 {
            if !alive(&pid) {
                dead = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(dead, "le processus {pid} vit encore");
    }

    #[tokio::test]
    async fn term_ignore_tue_apres_le_delai() {
        let (dir, inv, log) = setup("");
        let ready = dir.path().join("pret");
        let body = format!(
            "trap '' TERM\necho oui > {}\nsleep 1000",
            shell_quote(&ready.display().to_string())
        );
        let inv = Invocation {
            program: fake_claude(dir.path(), &body),
            ..inv
        };
        let grace = Duration::from_millis(300);
        let running = spawn(inv, &log, Limits { grace, ..limits() }).unwrap();
        wait_file(&ready).await;
        running.stopper().stop();
        let started = Instant::now();
        let out = running.wait().await;
        let elapsed = started.elapsed();
        assert_eq!(out.reason, EndReason::Stopped);
        assert_eq!(out.exit_code, None);
        assert!(elapsed >= grace, "{elapsed:?}");
        assert!(elapsed < Duration::from_secs(2), "{elapsed:?}");
    }

    #[tokio::test]
    async fn duree_maximale_depassee() {
        let (_dir, inv, log) = setup("sleep 1000");
        let max_run = Duration::from_millis(300);
        let started = Instant::now();
        let out = spawn(
            inv,
            &log,
            Limits {
                max_run,
                ..limits()
            },
        )
        .unwrap()
        .wait()
        .await;
        assert_eq!(out.reason, EndReason::TimedOut);
        assert!(started.elapsed() >= max_run);
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[tokio::test]
    async fn silence_trop_long_arrete() {
        // Une ligne toutes les 100 ms remet le minuteur à zéro, puis plus rien.
        // Sous forte charge, un intervalle peut dépasser `idle` : seuls la
        // raison et des bornes larges sont vérifiées.
        let body = format!(
            "for i in 1 2 3 4 5; do {}; sleep 0.1; done\nsleep 1000",
            print_line(&init_line("abc"))
        );
        let (_dir, inv, log) = setup(&body);
        let idle = Duration::from_millis(800);
        let started = Instant::now();
        let out = spawn(inv, &log, Limits { idle, ..limits() })
            .unwrap()
            .wait()
            .await;
        let elapsed = started.elapsed();
        assert_eq!(out.reason, EndReason::Idle);
        assert!(out.progress.lines >= 1);
        assert!(elapsed >= idle, "{elapsed:?}");
        if out.progress.lines == 5 {
            // Minuteur remis à zéro par chaque ligne : 4 intervalles en plus.
            assert!(elapsed >= idle + Duration::from_millis(400), "{elapsed:?}");
        }
        assert!(elapsed < Duration::from_secs(15), "{elapsed:?}");
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(text.contains("# fin : silence trop long"), "{text}");
    }

    #[tokio::test]
    async fn arret_envoie_term_a_claude_d_abord() {
        // Le faux claude note le `TERM` reçu ; son enfant note s'il en reçoit un
        // aussi (il ne doit pas : seul Claude Code reçoit `TERM`).
        let (dir, inv, log) = setup("");
        let recu = shell_quote(&dir.path().join("recu").display().to_string());
        let enfant = dir.path().join("enfant");
        let pret = dir.path().join("pret");
        let body = format!(
            "( trap 'echo enfant-term >> {recu}' TERM; while :; do sleep 0.05; done ) &\n\
             echo $! > {}\n\
             trap 'echo term >> {recu}; sleep 0.4; exit 0' TERM\n\
             echo oui > {}\n\
             wait",
            shell_quote(&enfant.display().to_string()),
            shell_quote(&pret.display().to_string()),
        );
        let inv = Invocation {
            program: fake_claude(dir.path(), &body),
            ..inv
        };
        let running = spawn(inv, &log, limits()).unwrap();
        wait_file(&pret).await;
        let child = wait_file(&enfant).await;
        let started = Instant::now();
        running.stopper().stop();
        let out = running.wait().await;
        assert_eq!(out.reason, EndReason::Stopped);
        // Sorti de lui-même sur `TERM`, avant la fin du délai de grâce.
        assert_eq!(out.exit_code, Some(0));
        assert!(
            started.elapsed() < limits().grace,
            "{:?}",
            started.elapsed()
        );
        let recu = std::fs::read_to_string(dir.path().join("recu")).unwrap();
        assert_eq!(recu.trim(), "term");
        // Puis tout le groupe est tué.
        let mut dead = false;
        for _ in 0..200 {
            if !alive(&child) {
                dead = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(dead, "le processus {child} vit encore");
    }

    #[tokio::test]
    async fn supervision_lachee_tue_le_groupe() {
        let (dir, inv, log) = setup("");
        let child = dir.path().join("enfant");
        let body = format!(
            "sleep 1000 &\necho $! > {}\nwait",
            shell_quote(&child.display().to_string())
        );
        let inv = Invocation {
            program: fake_claude(dir.path(), &body),
            ..inv
        };
        let running = spawn(inv, &log, limits()).unwrap();
        let pid = wait_file(&child).await;
        assert!(alive(&pid));
        // Comme à l'arrêt du cerveau : la tâche de supervision disparaît.
        running.handle.abort();
        assert!(running.handle.await.unwrap_err().is_cancelled());
        let mut dead = false;
        for _ in 0..200 {
            if !alive(&pid) {
                dead = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(dead, "le processus {pid} vit encore");
    }

    #[tokio::test]
    async fn programme_introuvable_erreur_claire() {
        let (dir, mut inv, log) = setup("");
        inv.program = dir.path().join("absent/claude");
        let err = spawn(inv, &log, limits()).err().expect("erreur attendue");
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        let msg = err.to_string();
        assert!(msg.contains("introuvable"), "{msg}");
        assert!(msg.contains("absent/claude"), "{msg}");

        let (_dir, mut inv, log) = setup("");
        inv.cwd = PathBuf::from("/dossier/absent");
        let err = spawn(inv, &log, limits()).err().expect("erreur attendue");
        assert!(err.to_string().contains("/dossier/absent"), "{err}");
    }

    #[tokio::test]
    async fn arret_apres_fin_sans_effet() {
        let (_dir, inv, log) = setup(&print_line(&result_line(true, "Fini.")));
        let running = spawn(inv, &log, limits()).unwrap();
        let stopper = running.stopper();
        let mut progress = running.progress();
        for _ in 0..200 {
            if progress.result.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
            progress = running.progress();
        }
        assert!(progress.result.is_some());
        let out = running.wait().await;
        assert_eq!(out.reason, EndReason::Exited);
        assert_eq!(out.exit_code, Some(0));
        stopper.stop();
        stopper.stop();
    }

    #[test]
    fn heure_du_journal_en_utc() {
        assert_eq!(utc(0), "1970-01-01 00:00:00 UTC");
        assert_eq!(utc(1_793_282_592), "2026-10-29 14:03:12 UTC");
        assert_eq!(utc(1_709_251_199), "2024-02-29 23:59:59 UTC");
    }

    #[tokio::test]
    async fn journal_borne_a_20_mo() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("j.log");
        let file = tokio::fs::File::from_std(open_log(&path).unwrap());
        let mut log = Log {
            file,
            written: LOG_MAX - 10,
            full: false,
        };
        log.line("court").await;
        log.line("ligne trop longue pour la fin").await;
        log.line("encore").await;
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text, "court\n[journal tronqué : 20 Mo atteints]\n");
    }
}
