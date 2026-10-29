//! Oreille de Jarvis : micro, mot d'éveil, détection de fin de phrase, voix de Jarvis.
//! Elle parle au cerveau par le réseau (WebSocket sur TLS épinglé), même sur le PC.

mod app;
mod capture;
mod client;
mod config;
mod enroll;
mod link;
mod pair;
mod playback;
mod service;
mod sounds;
mod tray;

use std::sync::Arc;

use anyhow::Context;
use clap::{Parser, Subcommand};
use jarvis_ear_core::MicState;
use jarvis_ear_core::earshot::EarshotVad;
use jarvis_ear_core::listener::Listener;
use jarvis_ear_core::net::discovery::MdnsDiscovery;
use jarvis_ear_core::net::tls::{TlsConnector, pinned_client_config};
use jarvis_ear_core::net::{Discovery, EarLink, LinkConfig, Pairing, PairingStore};
use jarvis_ear_core::rustpotter::RustpotterDetector;
use tokio::sync::{mpsc, watch};

use crate::app::{App, OfflinePhrase};
use crate::capture::CpalCapture;
use crate::client::{Brain, SocketBrain};
use crate::config::{EarConfig, Paths};
use crate::link::{LOCAL_NAME, LOCAL_RETRY, Link, LocalPairing, Spawner, Supervised};
use crate::playback::CpalPlayer;
use crate::tray::{BrainLink, JarvisTray, TrayCommand, TrayState};

#[derive(Parser)]
#[command(name = "jarvis-ear", version, about = "Oreille de Jarvis")]
struct Cli {
    /// Écoute même si le service jarvis-ear tourne déjà (deux oreilles).
    #[arg(long)]
    force: bool,
    /// Profil : appairage, réglages (`ear-<profil>.toml`) et coupure séparés, pour une
    /// seconde oreille sur la même machine (essai).
    #[arg(long, global = true)]
    profile: Option<String>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Écoute le micro (commande par défaut).
    Run {
        /// Écoute même si le service jarvis-ear tourne déjà (deux oreilles).
        #[arg(long)]
        force: bool,
    },
    /// Enregistre le mot d'éveil, puis l'empreinte vocale.
    Enroll {
        /// Nombre de prises de « Jarvis ».
        #[arg(long, default_value_t = 8)]
        count: usize,
        /// Refait uniquement l'empreinte vocale (6 phrases), sans le mot d'éveil.
        #[arg(long)]
        voice_only: bool,
    },
    /// Appaire cette oreille au cerveau (lancer d'abord `jarvis pair` sur le PC).
    Pair {
        /// Adresse d'appairage affichée par `jarvis pair` (jarvis://pair?…).
        #[arg(required_unless_present = "code", conflicts_with = "code")]
        uri: Option<String>,
        /// Code à 6 chiffres affiché par `jarvis pair` (empreinte à confirmer).
        #[arg(long)]
        code: Option<String>,
        /// Adresse du cerveau (hôte[:port]) si la découverte mDNS ne le trouve pas.
        #[arg(long, requires = "code")]
        address: Option<String>,
        /// Nom de l'oreille (défaut : `name` de la configuration, sinon la machine).
        #[arg(long)]
        name: Option<String>,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let paths = Paths::for_profile(cli.profile.as_deref())?;
    match cli.command.unwrap_or(Command::Run { force: false }) {
        Command::Run { force } => {
            // Un profil est une seconde oreille voulue.
            refuse_if_service_running(cli.force || force || cli.profile.is_some());
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?
                .block_on(run(paths, cli.profile))?
        }
        Command::Enroll { count, voice_only } => enroll::run(count, voice_only)?,
        Command::Pair {
            uri,
            code,
            address,
            name,
        } => {
            let config = EarConfig::load(&paths.config)?;
            let name = name.unwrap_or_else(|| config.pair_name());
            let address = address.or_else(|| config.brain_address());
            let store = PairingStore::new(paths.pairing_dir);
            pair::run(store, cli.profile.as_deref(), uri, code, address, name)?
        }
    }
    Ok(())
}

/// Lancée à la main alors que le service tourne : message clair et code 1.
fn refuse_if_service_running(force: bool) {
    let by_systemd = service::started_by_systemd(|name| std::env::var_os(name));
    // Le service lui-même ne s'interroge pas : il est forcément actif à son démarrage.
    if by_systemd || force {
        return;
    }
    let active = service::is_active(&service::RealSystemctl);
    if service::refuse_second_ear(active, by_systemd, force) {
        eprintln!("{}", service::ALREADY_RUNNING);
        std::process::exit(1);
    }
}

/// Lien vers le cerveau : appairage enregistré, sinon (oreille du PC) appairage local
/// par le socket Unix.
fn spawn_link(
    store: &PairingStore,
    pairing: Option<Pairing>,
    local: bool,
    control: Arc<dyn Brain>,
    manual: Option<String>,
) -> (Supervised, mpsc::Receiver<jarvis_ear_core::net::LinkEvent>) {
    // L'oreille du PC passe par 127.0.0.1 : pas de mDNS.
    let discovery: Option<Arc<dyn Discovery>> = if local {
        None
    } else {
        match MdnsDiscovery::new() {
            Ok(mdns) => Some(Arc::new(mdns)),
            Err(e) => {
                tracing::warn!("découverte mDNS indisponible : {e:#}");
                None
            }
        }
    };
    let link_store = store.clone();
    let spawner: Spawner = Arc::new(move |mut pairing: Pairing| {
        if manual.is_some() {
            pairing.manual = manual.clone();
        }
        let connector = Arc::new(TlsConnector::new(pinned_client_config(
            &pairing.fingerprint,
        )));
        let config = LinkConfig::new(pairing, link_store.clone(), connector, discovery.clone());
        let (link, events) = EarLink::spawn(config);
        (Arc::new(link) as Arc<dyn Link>, events)
    });
    let local = local.then(|| LocalPairing {
        control,
        name: LOCAL_NAME.into(),
        retry: LOCAL_RETRY,
    });
    Supervised::spawn(pairing, local, store.clone(), spawner)
}

async fn run(paths: Paths, profile: Option<String>) -> anyhow::Result<()> {
    use std::io::IsTerminal;
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        // Dans le journal (service), pas de codes de couleur.
        .with_ansi(std::io::stdout().is_terminal())
        .init();

    let config = EarConfig::load(&paths.config)?;
    let store = PairingStore::new(paths.pairing_dir.clone());
    let pairing = store.load().context("appairage illisible")?;
    // Oreille du PC : profil par défaut, pas encore appairée ou appairée en local.
    let local = profile.is_none() && pairing.as_ref().is_none_or(link::is_local);
    let socket = jarvis_protocol::socket_path();
    if pairing.is_none() {
        if let Some(profile) = &profile {
            anyhow::bail!(
                "aucun appairage pour le profil « {profile} » : lancez `jarvis pair` sur le PC, \
                 puis `jarvis-ear --profile {profile} pair '<adresse>'`"
            );
        }
        if !socket.exists() {
            tracing::warn!(
                "pas d'appairage et pas de cerveau sur cette machine : sur une autre \
                 machine que le PC, lancez `jarvis-ear pair` (l'oreille attend le cerveau)"
            );
        }
    }

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
        brain: BrainLink::Connecting,
        last_heard: None,
    });

    // L'icône est facultative : sans zone de notification, l'oreille fonctionne quand même.
    // En service, l'oreille peut démarrer avant la zone de notification de GNOME (extension
    // AppIndicator) : l'icône s'y inscrit dès qu'elle apparaît.
    let tray_handle = {
        use ksni::TrayMethods;
        let tray = JarvisTray {
            state: tray_tx.borrow().clone(),
            commands: commands.clone(),
        };
        match tray.assume_sni_available(true).spawn().await {
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

    let control: Arc<dyn Brain> = Arc::new(SocketBrain { socket });
    let (link, events) = spawn_link(
        &store,
        pairing,
        local,
        control.clone(),
        config.brain_address(),
    );
    let offline: OfflinePhrase = {
        let store = store.clone();
        Arc::new(move || store.offline_phrase().map(|(_, audio)| audio))
    };
    let name = config.name();
    tracing::info!(
        "oreille « {name} » démarrée (mot d'éveil : {})",
        wakeword.display()
    );
    let mut app = App::new(
        listener,
        CpalCapture::new(config.device),
        Arc::new(link),
        events,
        commands_rx,
        tray_tx,
    )
    .with_wake_threshold(threshold)
    .with_name(name)
    .with_offline_phrase(offline)
    .with_mute_file(paths.mute_file);
    // Annonces relevées par le socket Unix (oreille du PC seulement).
    if local {
        app = app.with_control(control);
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
