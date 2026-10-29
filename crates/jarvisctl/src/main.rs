mod ops;
mod paths;
mod system;

use clap::{Parser, Subcommand};
use paths::{Paths, Service};
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
    /// Démarre les services (cerveau et oreille, ou celui indiqué)
    Start {
        #[arg(value_enum)]
        service: Option<Service>,
    },
    /// Arrête les services (cerveau et oreille, ou celui indiqué)
    Stop {
        #[arg(value_enum)]
        service: Option<Service>,
    },
    /// Redémarre les services (cerveau et oreille, ou celui indiqué)
    Restart {
        #[arg(value_enum)]
        service: Option<Service>,
    },
    /// Active le démarrage automatique (cerveau et oreille, ou celui indiqué)
    Enable {
        #[arg(value_enum)]
        service: Option<Service>,
    },
    /// Désactive le démarrage automatique (cerveau et oreille, ou celui indiqué)
    Disable {
        #[arg(value_enum)]
        service: Option<Service>,
    },
    /// Affiche l'état du cerveau, de l'oreille et d'Ollama (code 3 si le cerveau est inactif)
    Status {
        #[arg(value_enum)]
        service: Option<Service>,
    },
    /// Affiche les journaux du cerveau (ou de l'oreille)
    Logs {
        /// Service dont on lit le journal (cerveau par défaut)
        #[arg(value_enum)]
        service: Option<Service>,
        /// Journaux de l'oreille (comme `logs oreille`)
        #[arg(long, conflicts_with = "service")]
        oreille: bool,
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

fn verb(
    paths: &Paths,
    sys: &dyn System,
    verb: &str,
    service: Option<Service>,
    out: &mut dyn io::Write,
) -> anyhow::Result<i32> {
    ops::service_verb(paths, sys, verb, &ops::targets(service), out)
}

/// Service des journaux : le cerveau sauf `--oreille` ou cible explicite.
fn logs_service(service: Option<Service>, oreille: bool) -> Service {
    match (service, oreille) {
        (Some(s), _) => s,
        (None, true) => Service::Ear,
        (None, false) => Service::Brain,
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
        Cmd::Start { service } => verb(&paths, &sys, "start", service, &mut out),
        Cmd::Stop { service } => verb(&paths, &sys, "stop", service, &mut out),
        Cmd::Restart { service } => verb(&paths, &sys, "restart", service, &mut out),
        Cmd::Enable { service } => verb(&paths, &sys, "enable", service, &mut out),
        Cmd::Disable { service } => verb(&paths, &sys, "disable", service, &mut out),
        Cmd::Status { service } => ops::status(&paths, &sys, &ops::targets(service), &mut out),
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
        Cmd::Logs {
            service,
            oreille,
            follow,
            lines,
        } => {
            let unit = logs_service(service, oreille).unit();
            let mut cmd = Command::new("journalctl");
            cmd.args(["--user", "-u", unit, "-n", &lines.to_string()]);
            if follow {
                cmd.arg("-f");
            }
            let err = cmd.exec();
            Err(anyhow::anyhow!("impossible de lancer journalctl : {err}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Cmd {
        let mut full = vec!["jarvisctl"];
        full.extend_from_slice(args);
        Cli::try_parse_from(full).unwrap().command
    }

    #[test]
    fn verbes_sans_cible_visent_les_deux_services() {
        assert!(matches!(
            parse(&["restart"]),
            Cmd::Restart { service: None }
        ));
        assert!(matches!(parse(&["status"]), Cmd::Status { service: None }));
        assert_eq!(ops::targets(None), vec![Service::Brain, Service::Ear]);
        assert_eq!(ops::targets(Some(Service::Ear)), vec![Service::Ear]);
    }

    #[test]
    fn cible_en_francais_ou_en_anglais() {
        for (arg, attendu) in [
            ("oreille", Service::Ear),
            ("ear", Service::Ear),
            ("cerveau", Service::Brain),
            ("brain", Service::Brain),
        ] {
            let Cmd::Restart { service } = parse(&["restart", arg]) else {
                panic!("{arg}")
            };
            assert_eq!(service, Some(attendu), "{arg}");
        }
        assert!(matches!(
            parse(&["stop", "oreille"]),
            Cmd::Stop {
                service: Some(Service::Ear)
            }
        ));
        assert!(Cli::try_parse_from(["jarvisctl", "start", "nez"]).is_err());
    }

    #[test]
    fn journaux_du_cerveau_par_defaut_ou_de_l_oreille() {
        let Cmd::Logs {
            service,
            oreille,
            follow,
            lines,
        } = parse(&["logs", "--oreille", "-f"])
        else {
            panic!()
        };
        assert!(follow);
        assert_eq!(lines, 50);
        assert_eq!(logs_service(service, oreille), Service::Ear);

        let Cmd::Logs {
            service, oreille, ..
        } = parse(&["logs", "-n", "10"])
        else {
            panic!()
        };
        assert_eq!(logs_service(service, oreille), Service::Brain);

        let Cmd::Logs {
            service, oreille, ..
        } = parse(&["logs", "ear", "-f"])
        else {
            panic!()
        };
        assert_eq!(logs_service(service, oreille), Service::Ear);
        assert!(Cli::try_parse_from(["jarvisctl", "logs", "cerveau", "--oreille"]).is_err());
    }
}
