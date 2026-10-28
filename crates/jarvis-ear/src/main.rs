//! Oreille de Jarvis : micro, mot d'éveil, détection de fin de phrase.

use clap::{Parser, Subcommand};

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
    Enroll,
}

fn main() {
    match Cli::parse().command.unwrap_or(Command::Run) {
        Command::Run | Command::Enroll => println!("pas encore implémenté"),
    }
}
