//! Oreille de Jarvis : micro, mot d'éveil, détection de fin de phrase, voix de Jarvis.

mod app;
mod capture;
mod client;
mod config;
mod enroll;
mod playback;
mod sounds;
mod tray;

use std::sync::Arc;

use anyhow::Context;
use clap::{Parser, Subcommand};
use jarvis_ear_core::MicState;
use jarvis_ear_core::earshot::EarshotVad;
use jarvis_ear_core::listener::Listener;
use jarvis_ear_core::rustpotter::RustpotterDetector;
use tokio::sync::{mpsc, watch};

use crate::app::{App, SocketBrain, default_mute_file};
use crate::capture::CpalCapture;
use crate::config::EarConfig;
use crate::playback::CpalPlayer;
use crate::tray::{BrainLink, JarvisTray, TrayCommand, TrayState};

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
    /// Enregistre le mot d'éveil, puis l'empreinte vocale.
    Enroll {
        /// Nombre de prises de « Jarvis ».
        #[arg(long, default_value_t = 8)]
        count: usize,
        /// Refait uniquement l'empreinte vocale (6 phrases), sans le mot d'éveil.
        #[arg(long)]
        voice_only: bool,
    },
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().command.unwrap_or(Command::Run) {
        Command::Run => tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?
            .block_on(run())?,
        Command::Enroll { count, voice_only } => enroll::run(count, voice_only)?,
    }
    Ok(())
}

async fn run() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let config = EarConfig::load(&EarConfig::default_path()?)?;
    let wakeword = match &config.wakeword_path {
        Some(path) => path.clone(),
        None => enroll::wakeword_dir()?.join("jarvis.rpw"),
    };
    let threshold = config
        .threshold
        .unwrap_or(RustpotterDetector::DEFAULT_THRESHOLD);
    let wake = RustpotterDetector::load(&wakeword, threshold)?;
    let listener = Listener::new(wake, EarshotVad::new());

    let (commands, commands_rx) = mpsc::unbounded_channel();
    let (tray_tx, mut tray_rx) = watch::channel(TrayState {
        mic: MicState::Open,
        capturing: false,
        brain: BrainLink::Unreachable,
        last_heard: None,
    });

    // L'icône est facultative : sans zone de notification, l'oreille fonctionne quand même.
    let tray_handle = {
        use ksni::TrayMethods;
        let tray = JarvisTray {
            state: tray_tx.borrow().clone(),
            commands: commands.clone(),
        };
        match tray.spawn().await {
            Ok(handle) => Some(handle),
            Err(e) => {
                tracing::warn!("icône de notification indisponible : {e}");
                None
            }
        }
    };
    if let Some(handle) = tray_handle.clone() {
        tokio::spawn(async move {
            while tray_rx.changed().await.is_ok() {
                let state = tray_rx.borrow_and_update().clone();
                if handle.update(|t| t.state = state).await.is_none() {
                    break;
                }
            }
        });
    }

    // Ctrl-C et SIGTERM quittent proprement.
    let quit = commands.clone();
    tokio::spawn(async move {
        use tokio::signal::unix::{SignalKind, signal};
        let mut term = signal(SignalKind::terminate()).ok();
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = async {
                match term.as_mut() {
                    Some(t) => { t.recv().await; }
                    None => std::future::pending::<()>().await,
                }
            } => {}
        }
        let _ = quit.send(TrayCommand::Quit);
    });

    let brain = Arc::new(SocketBrain {
        socket: jarvis_protocol::socket_path(),
    });
    tracing::info!("oreille démarrée (mot d'éveil : {})", wakeword.display());
    let mut app = App::new(
        listener,
        CpalCapture::new(config.device),
        brain,
        commands_rx,
        tray_tx,
    )
    .with_wake_threshold(threshold);
    if let Some(path) = default_mute_file() {
        app = app.with_mute_file(path);
    }
    // Sans sortie audio, l'oreille fonctionne quand même, sans son.
    match CpalPlayer::new() {
        Ok(player) => app = app.with_player(Box::new(player)),
        Err(e) => tracing::warn!("lecture des sons impossible, l'oreille reste muette : {e:#}"),
    }
    let result = app.run().await.context("boucle principale");
    if let Some(handle) = tray_handle {
        handle.shutdown().await;
    }
    result
}
