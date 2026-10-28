use anyhow::Context;
use jarvis_brain::brain::Brain;
use jarvis_brain::config::Config;
use jarvis_brain::ducking::{Ducker, WpctlDucker};
use jarvis_brain::listen::{ListenService, NoTranscriber};
use jarvis_brain::llm::{ClaudeBackend, OllamaBackend};
use jarvis_brain::logging::init_logging;
use jarvis_brain::mode_store::ModeStore;
use jarvis_brain::server::{SpeakerState, serve_with_voice};
use jarvis_brain::speaker::{SpeakerGate, sherpa::SherpaVerifier};
use jarvis_brain::stt::{Transcriber, whisper::WhisperTranscriber};
use jarvis_brain::tts::{Voice, piper::PiperSynthesizer};
use jarvis_protocol::Mode;
use jarvis_skills::apps::{AppIndex, AppsSkill};
use jarvis_skills::media::{MediaSkill, MprisPlayers};
use jarvis_skills::{Skill, SystemRunner};
use std::sync::Arc;
use tokio::signal::unix::{SignalKind, signal};
use tracing::{info, warn};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_logging();
    let config = Config::load(&Config::default_path())?;
    let modes = Arc::new(ModeStore::new(ModeStore::default_path(), config.mode));

    let runner = Arc::new(SystemRunner);
    let skills: Vec<Arc<dyn Skill>> = vec![
        Arc::new(MediaSkill::new(
            Arc::new(MprisPlayers::new()),
            runner.clone(),
        )),
        Arc::new(AppsSkill::new(
            AppIndex::load(&AppIndex::default_dirs()),
            config.apps.aliases.clone(),
            runner.clone(),
        )),
    ];
    let local = Arc::new(OllamaBackend::new(&config.local, &config.address));
    let brain = Arc::new(
        Brain::new(
            skills,
            local.clone(),
            Arc::new(ClaudeBackend::from_env(&config.online, &config.address)),
            modes,
        )
        .with_local_model(config.local.model.clone()),
    );
    if brain.mode() == Mode::Local {
        let tools = brain.tools();
        tokio::spawn(async move {
            match local.warm_up(&tools).await {
                Ok(()) => info!("modèle local prêt"),
                Err(e) => warn!(erreur = %e, "préchauffage du modèle local impossible"),
            }
        });
    }

    let speaker = if config.speaker.enabled {
        match SherpaVerifier::load(&config.speaker) {
            Ok(v) => SpeakerState::Ready(Arc::new(SpeakerGate::new(
                Arc::new(v),
                SpeakerGate::default_path(),
                config.speaker.threshold,
                config.speaker.strict_threshold,
            ))),
            Err(e) => {
                warn!(erreur = %e, "modèle d'empreinte vocale indisponible");
                SpeakerState::unavailable(&SpeakerGate::default_path())
            }
        }
    } else {
        SpeakerState::Disabled
    };

    let stt: Arc<dyn Transcriber> = match WhisperTranscriber::load(&config.stt) {
        Ok(w) => {
            info!(modele = %config.stt.model_path.display(), "reconnaissance vocale prête");
            Arc::new(w)
        }
        Err(e) => {
            warn!(
                erreur = %e,
                "reconnaissance vocale indisponible (modèle attendu : {} ; voir scripts/download-models.sh)",
                config.stt.model_path.display()
            );
            Arc::new(NoTranscriber)
        }
    };
    let voice = if config.tts.enabled {
        match PiperSynthesizer::load(&config.tts) {
            Ok(v) => {
                info!(voix = %config.tts.model_dir.display(), "voix de Jarvis prête");
                Voice::new(Arc::new(v))
            }
            Err(e) => {
                warn!(erreur = %e, "voix de Jarvis indisponible, réponses en texte seul");
                Voice::none()
            }
        }
    } else {
        Voice::none()
    };
    let ducker = Arc::new(WpctlDucker::persistent(runner, WpctlDucker::default_file()));
    ducker.recover().await;
    let listen = Arc::new(
        ListenService::new(brain.clone(), stt, ducker.clone()).with_speaker_state(speaker.clone()),
    );
    let socket = jarvis_protocol::socket_path();
    let mut sigterm = signal(SignalKind::terminate()).context("signal SIGTERM")?;
    let result = tokio::select! {
        r = serve_with_voice(brain, listen, speaker, voice, &socket) => r,
        _ = tokio::signal::ctrl_c() => Ok(()),
        _ = sigterm.recv() => Ok(()),
    };
    // Arrêt entre un réveil et la fin de l'écoute : la musique ne reste pas baissée.
    ducker.restore().await;
    // Ne supprime pas le socket d'un autre cerveau si nous n'avons pas pu le prendre.
    if result.is_ok() {
        let _ = std::fs::remove_file(&socket);
        info!("cerveau arrêté");
    }
    result
}
