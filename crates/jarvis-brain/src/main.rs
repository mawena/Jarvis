use anyhow::Context;
use jarvis_brain::brain::Brain;
use jarvis_brain::config::{Config, LocalLlmConfig};
use jarvis_brain::ducking::{Ducker, WpctlDucker};
use jarvis_brain::listen::{ListenService, NoTranscriber};
use jarvis_brain::llm::{ClaudeBackend, OllamaBackend};
use jarvis_brain::logging::init_logging;
use jarvis_brain::mode_store::ModeStore;
use jarvis_brain::persona::Persona;
use jarvis_brain::server::{SpeakerState, serve_with_voice};
use jarvis_brain::speaker::{SpeakerGate, sherpa::SherpaVerifier};
use jarvis_brain::stt::{Transcriber, whisper::WhisperTranscriber};
use jarvis_brain::summarizer::{ModeSummarizer, Rewarmer, WarmUp, separate_summary_model};
use jarvis_brain::tts::{Voice, piper::PiperSynthesizer};
use jarvis_protocol::Mode;
use jarvis_skills::apps::{AppIndex, AppsSkill};
use jarvis_skills::media::{MediaSkill, MprisPlayers};
use jarvis_skills::search::{SearchSkill, SearxClient};
use jarvis_skills::video::VideoSkill;
use jarvis_skills::{Skill, SystemRunner, ToolSpec};
use std::sync::{Arc, OnceLock};
use tokio::signal::unix::{SignalKind, signal};
use tracing::{info, warn};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_logging();
    let config = Config::load(&Config::default_path())?;
    let modes = Arc::new(ModeStore::new(ModeStore::default_path(), config.mode));

    let runner = Arc::new(SystemRunner);
    // Lecteurs MPRIS partagés : la vidéo met la musique en pause.
    let players = Arc::new(MprisPlayers::new());
    let mut skills: Vec<Arc<dyn Skill>> = vec![
        Arc::new(MediaSkill::new(players.clone(), runner.clone())),
        Arc::new(AppsSkill::new(
            AppIndex::load(&AppIndex::default_dirs()),
            config.apps.aliases.clone(),
            runner.clone(),
        )),
        Arc::new(jarvis_skills::power::PowerSkill::new(runner.clone())),
    ];
    let local = Arc::new(OllamaBackend::new(&config.local, &config.address));
    let online = Arc::new(ClaudeBackend::from_env(&config.online, &config.address));

    // Outils proposés à l'IA, connus une fois le cerveau construit : le
    // re-préchauffage qui suit un résumé local en a besoin.
    let warm_tools: Arc<OnceLock<Vec<ToolSpec>>> = Arc::new(OnceLock::new());
    match SearxClient::new(&config.search) {
        Ok(client) => {
            let client = Arc::new(client);
            // Modèle de résumé distinct si `local_summary_model` est renseigné.
            let summary_model = config.search.local_summary_model.trim();
            let local_summary: Arc<OllamaBackend> = if summary_model.is_empty() {
                local.clone()
            } else {
                let cfg = LocalLlmConfig {
                    model: summary_model.to_string(),
                    ..config.local.clone()
                };
                Arc::new(OllamaBackend::new(&cfg, &config.address))
            };
            let (rewarm_local, rewarm_tools) = (local.clone(), warm_tools.clone());
            let warm: WarmUp = Arc::new(move || {
                let local = rewarm_local.clone();
                let tools = rewarm_tools.get().cloned();
                Box::pin(async move {
                    let Some(tools) = tools else {
                        return;
                    };
                    if let Err(e) = local.warm_up(&tools).await {
                        warn!(erreur = %e, "re-préchauffage du modèle local impossible");
                    }
                })
            });
            // La consigne du résumé a pris la place des outils dans le cache d'Ollama :
            // on les lui refait lire, en tâche de fond, sans retarder la réponse
            // (sauf modèle de résumé distinct, mode passé en ligne ou re-préchauffage
            // déjà en cours : voir `Rewarmer`).
            let rewarmer = Rewarmer::new(
                modes.clone(),
                separate_summary_model(summary_model, &config.local.model),
                warm,
            );
            let summarizer = ModeSummarizer::new(
                local_summary,
                online.clone(),
                modes.clone(),
                &config.address,
                &config.search,
            )
            .with_after_local(rewarmer.into_after_local());
            skills.push(Arc::new(SearchSkill::new(
                client.clone(),
                Arc::new(summarizer),
                runner.clone(),
                config.search.clone(),
            )));
            skills.push(Arc::new(VideoSkill::new(
                client,
                players,
                runner.clone(),
                config.search.clone(),
            )));
        }
        Err(e) => warn!(
            erreur = %e,
            "recherche et vidéos désactivées (réglage [search] url à corriger)"
        ),
    }

    let brain = Arc::new(
        Brain::new(skills, local.clone(), online, modes)
            .with_local_model(config.local.model.clone())
            .with_persona(Persona::from(&config)),
    );
    let _ = warm_tools.set(brain.tools());
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
