mod pair;

use clap::{Parser, Subcommand};
use jarvis_protocol::{ControlRequest, ControlResponse, Mode, pairing_ended};
use std::io::{BufRead, BufReader, ErrorKind, IsTerminal, Write};
use std::os::unix::net::UnixStream;
use std::process::ExitCode;
use std::time::Duration;

#[derive(Parser)]
#[command(name = "jarvis", about = "Parler au cerveau de Jarvis")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Envoie une phrase à Jarvis
    Say {
        #[arg(required = true, num_args = 1..)]
        text: Vec<String>,
    },
    /// Affiche le mode et la version du cerveau
    Status,
    /// Change de mode (local, online ou en-ligne)
    Mode { mode: Mode },
    /// Ouvre la fenêtre d'appairage d'une oreille (code, QR code)
    Pair,
    /// Liste les oreilles appairées
    Ears,
    /// Retire une oreille (nom, ou identifiant : la colonne Id de `jarvis ears` suffit)
    Unpair {
        #[arg(required = true, num_args = 1..)]
        ear: Vec<String>,
    },
}

const NOT_RUNNING: &str = "Le cerveau de Jarvis n'est pas lancé (jarvis-brain).";

fn main() -> ExitCode {
    let request = match Cli::parse().command {
        Command::Say { text } => ControlRequest::Say {
            text: text.join(" "),
        },
        Command::Status => ControlRequest::Status,
        Command::Mode { mode } => ControlRequest::SetMode { mode },
        Command::Pair => ControlRequest::PairStart { person: None },
        Command::Ears => ControlRequest::ListEars,
        Command::Unpair { ear } => ControlRequest::Unpair { ear: ear.join(" ") },
    };
    let mut conn = match send(&request) {
        Ok(conn) => conn,
        Err(e) => return io_failure(&e),
    };
    match read_response(&mut conn) {
        Ok(ControlResponse::Reply { speech } | ControlResponse::Heard { speech, .. }) => {
            if !speech.is_empty() {
                println!("{speech}");
            }
            ExitCode::SUCCESS
        }
        Ok(ControlResponse::Status {
            mode,
            version,
            local_model,
        }) => {
            if mode == Mode::Local && !local_model.is_empty() {
                println!("Mode : {mode} ({local_model}) (jarvis-brain {version})");
            } else {
                println!("Mode : {mode} (jarvis-brain {version})");
            }
            ExitCode::SUCCESS
        }
        // Jamais demandées par la CLI ; dites telles quelles si elles arrivent.
        Ok(ControlResponse::Announcements { items }) => {
            for item in items {
                println!("{}", item.speech);
            }
            ExitCode::SUCCESS
        }
        Ok(ControlResponse::Pairing {
            code,
            uri,
            fingerprint_short,
            expires_in_secs,
        }) => {
            let colors = std::io::stdout().is_terminal();
            println!(
                "{}\n",
                pair::screen(&code, &uri, &fingerprint_short, colors)
            );
            wait_pairing(conn, expires_in_secs)
        }
        Ok(ControlResponse::Ears { ears }) => {
            println!("{}", pair::ears_table(&ears));
            ExitCode::SUCCESS
        }
        Ok(ControlResponse::Error { message }) => fail(&message, 1),
        Ok(_) => fail("Réponse inattendue du cerveau.", 1),
        Err(e) => io_failure(&e),
    }
}

/// Attend la fin de la fenêtre sur la même connexion. Ctrl-C : le processus
/// s'arrête, la connexion se ferme, le cerveau ferme la fenêtre.
fn wait_pairing(mut conn: BufReader<UnixStream>, expires_in_secs: u64) -> ExitCode {
    // Filet si le cerveau ne dit jamais rien : délai de la fenêtre + 15 s.
    let _ = conn
        .get_ref()
        .set_read_timeout(Some(Duration::from_secs(expires_in_secs + 15)));
    let (stop, countdown) = pair::countdown(expires_in_secs);
    let end = read_response(&mut conn);
    drop(stop);
    let _ = countdown.join();
    match end {
        Ok(ControlResponse::PairingDone { name }) => {
            println!("Oreille « {name} » appairée.");
            ExitCode::SUCCESS
        }
        Ok(ControlResponse::PairingEnded { reason }) => {
            fail(&format!("Fenêtre d'appairage fermée : {reason}."), 1)
        }
        Ok(ControlResponse::Error { message }) => fail(&message, 1),
        Ok(_) => fail("Réponse inattendue du cerveau.", 1),
        Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => fail(
            &format!("Fenêtre d'appairage fermée : {}.", pairing_ended::TIMED_OUT),
            1,
        ),
        Err(e) => fail(&format!("Échec de la communication : {e}."), 1),
    }
}

fn io_failure(e: &std::io::Error) -> ExitCode {
    if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::ConnectionRefused) {
        fail(NOT_RUNNING, 2)
    } else {
        fail(&format!("Échec de la communication : {e}."), 1)
    }
}

fn fail(message: &str, code: u8) -> ExitCode {
    eprintln!("{message}");
    ExitCode::from(code)
}

fn send(request: &ControlRequest) -> std::io::Result<BufReader<UnixStream>> {
    let socket = jarvis_protocol::socket_path();
    // Dossier à soi et fermé aux autres, sinon rien n'est envoyé.
    jarvis_protocol::check_socket_dir(&socket)?;
    let mut stream = UnixStream::connect(socket)?;
    let mut line = serde_json::to_string(request).map_err(std::io::Error::other)?;
    line.push('\n');
    stream.write_all(line.as_bytes())?;
    Ok(BufReader::new(stream))
}

/// Une réponse (une ligne JSON) ; connexion fermée : erreur `UnexpectedEof`.
fn read_response(conn: &mut BufReader<UnixStream>) -> std::io::Result<ControlResponse> {
    let mut answer = String::new();
    if conn.read_line(&mut answer)? == 0 {
        return Err(std::io::Error::new(
            ErrorKind::UnexpectedEof,
            "connexion fermée par le cerveau",
        ));
    }
    serde_json::from_str(&answer).map_err(|e| {
        std::io::Error::new(ErrorKind::InvalidData, format!("réponse illisible ({e})"))
    })
}
