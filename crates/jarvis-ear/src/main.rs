//! Oreille de Jarvis : micro, mot d'éveil, détection de fin de phrase.

mod enroll;

use clap::{Parser, Subcommand};

#[allow(dead_code)]
mod tray;

#[derive(Parser)]
#[command(name = "jarvis-ear", version, about = "Oreille de Jarvis")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Écoute le micro (commande par défaut).
    Run,
    /// Enregistre le mot d'éveil.
    Enroll {
        /// Nombre de prises de « Jarvis ».
        #[arg(long, default_value_t = 8)]
        count: usize,
    },
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().command.unwrap_or(Command::Run) {
        Command::Run => println!("pas encore implémenté"),
        Command::Enroll { count } => enroll::run(count)?,
    }
    Ok(())
}
