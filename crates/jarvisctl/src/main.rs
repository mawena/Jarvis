mod ops;
mod paths;
mod system;

use clap::{Parser, Subcommand};
use paths::{Paths, SERVICE};
use std::io::{self, BufRead, IsTerminal};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, ExitCode};
use system::{RealSystem, System};

#[derive(Parser)]
#[command(
    name = "jarvisctl",
    about = "Gérer l'installation et le service Jarvis"
)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Compile, installe les binaires et active le service systemd utilisateur
    Install {
        /// Dépôt source (par défaut : celui de la compilation)
        #[arg(long)]
        source: Option<PathBuf>,
        /// N'exécute pas `cargo build`
        #[arg(long)]
        no_build: bool,
    },
    /// Arrête le service et supprime l'installation
    Uninstall {
        /// Efface aussi configuration, état et données
        #[arg(long)]
        purge: bool,
        /// Ne demande pas de confirmation pour --purge
        #[arg(long)]
        yes: bool,
    },
    /// Démarre le service
    Start,
    /// Arrête le service
    Stop,
    /// Redémarre le service
    Restart,
    /// Active le démarrage automatique
    Enable,
    /// Désactive le démarrage automatique
    Disable,
    /// Affiche l'état du service, du cerveau et d'Ollama (code 3 si inactif)
    Status,
    /// Affiche les journaux du service
    Logs {
        /// Suit les journaux en direct
        #[arg(short, long)]
        follow: bool,
        /// Nombre de lignes
        #[arg(short = 'n', long, default_value_t = 50)]
        lines: u32,
    },
    /// Vérifie l'installation et les dépendances
    Doctor,
    /// Enregistre la clé API Claude (saisie masquée si omise)
    SetApiKey { key: Option<String> },
    /// Recompile depuis le dépôt source, réinstalle et redémarre
    Update,
}

fn main() -> ExitCode {
    match run(Cli::parse().command) {
        Ok(code) => ExitCode::from(code as u8),
        Err(e) => {
            eprintln!("Erreur : {e:#}");
            ExitCode::from(1)
        }
    }
}

fn read_secret(sys: &dyn System) -> anyhow::Result<String> {
    let tty = io::stdin().is_terminal();
    if tty {
        eprint!("Clé API Claude : ");
        let _ = sys.run_inherit("stty", &["-echo"], None);
    }
    let mut line = String::new();
    let res = io::stdin().lock().read_line(&mut line);
    if tty {
        let _ = sys.run_inherit("stty", &["echo"], None);
        eprintln!();
    }
    res?;
    Ok(line)
}

fn run(cmd: Cmd) -> anyhow::Result<i32> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("HOME n'est pas défini"))?;
    let paths = Paths::from_home(&home);
    let sys = RealSystem;
    let mut out = io::stdout();
    match cmd {
        Cmd::Install { source, no_build } => {
            let source = match source {
                Some(s) => s,
                None => PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."),
            };
            let source = source.canonicalize().map_err(|e| {
                anyhow::anyhow!("dépôt source introuvable ({}) : {e}", source.display())
            })?;
            ops::install(
                &paths,
                &sys,
                &ops::InstallOpts { source, no_build },
                &mut out,
            )?;
            Ok(0)
        }
        Cmd::Uninstall { purge, yes } => {
            ops::uninstall(&paths, &sys, purge, yes, &mut io::stdin().lock(), &mut out)?;
            Ok(0)
        }
        Cmd::Start => ops::service_verb(&sys, "start", &mut out),
        Cmd::Stop => ops::service_verb(&sys, "stop", &mut out),
        Cmd::Restart => ops::service_verb(&sys, "restart", &mut out),
        Cmd::Enable => ops::service_verb(&sys, "enable", &mut out),
        Cmd::Disable => ops::service_verb(&sys, "disable", &mut out),
        Cmd::Status => ops::status(&paths, &sys, &mut out),
        Cmd::Doctor => ops::doctor(&paths, &sys, &mut out),
        Cmd::Update => {
            ops::update(&paths, &sys, &mut out)?;
            Ok(0)
        }
        Cmd::SetApiKey { key } => {
            let key = match key {
                Some(k) => k,
                None => read_secret(&sys)?,
            };
            ops::set_api_key(&paths, &key, &mut out)?;
            Ok(0)
        }
        Cmd::Logs { follow, lines } => {
            let mut cmd = Command::new("journalctl");
            cmd.args(["--user", "-u", SERVICE, "-n", &lines.to_string()]);
            if follow {
                cmd.arg("-f");
            }
            let err = cmd.exec();
            Err(anyhow::anyhow!("impossible de lancer journalctl : {err}"))
        }
    }
}
