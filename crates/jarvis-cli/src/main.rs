use clap::{Parser, Subcommand};
use jarvis_protocol::{ControlRequest, ControlResponse, Mode};
use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::os::unix::net::UnixStream;
use std::process::ExitCode;

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
}

const NOT_RUNNING: &str = "Le cerveau de Jarvis n'est pas lancé (jarvis-brain).";

fn main() -> ExitCode {
    let request = match Cli::parse().command {
        Command::Say { text } => ControlRequest::Say {
            text: text.join(" "),
        },
        Command::Status => ControlRequest::Status,
        Command::Mode { mode } => ControlRequest::SetMode { mode },
    };
    match exchange(&request) {
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
        Ok(ControlResponse::Error { message }) => fail(&message, 1),
        Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::ConnectionRefused) => {
            fail(NOT_RUNNING, 2)
        }
        Err(e) => fail(&format!("Échec de la communication : {e}."), 1),
    }
}

fn fail(message: &str, code: u8) -> ExitCode {
    eprintln!("{message}");
    ExitCode::from(code)
}

fn exchange(request: &ControlRequest) -> std::io::Result<ControlResponse> {
    let mut stream = UnixStream::connect(jarvis_protocol::socket_path())?;
    let mut line = serde_json::to_string(request).map_err(std::io::Error::other)?;
    line.push('\n');
    stream.write_all(line.as_bytes())?;
    let mut answer = String::new();
    BufReader::new(stream).read_line(&mut answer)?;
    serde_json::from_str(&answer).map_err(|e| {
        std::io::Error::new(ErrorKind::InvalidData, format!("réponse illisible ({e})"))
    })
}
