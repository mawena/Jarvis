use anyhow::Context;
use jarvis_brain::brain::Brain;
use jarvis_brain::config::Config;
use jarvis_brain::llm::{ClaudeBackend, OllamaBackend};
use jarvis_brain::logging::init_logging;
use jarvis_brain::mode_store::ModeStore;
use jarvis_brain::server::serve;
use jarvis_skills::apps::{AppIndex, AppsSkill};
use jarvis_skills::media::{MediaSkill, MprisPlayers};
use jarvis_skills::{Skill, SystemRunner};
use std::sync::Arc;
use tokio::signal::unix::{SignalKind, signal};
use tracing::info;

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
            runner,
        )),
    ];
    let brain = Arc::new(Brain::new(
        skills,
        Arc::new(OllamaBackend::new(&config.local, &config.address)),
        Arc::new(ClaudeBackend::from_env(&config.online, &config.address)),
        modes,
    ));

    let socket = jarvis_protocol::socket_path();
    let mut sigterm = signal(SignalKind::terminate()).context("signal SIGTERM")?;
    let result = tokio::select! {
        r = serve(brain, &socket) => r,
        _ = tokio::signal::ctrl_c() => Ok(()),
        _ = sigterm.recv() => Ok(()),
    };
    // Ne supprime pas le socket d'un autre cerveau si nous n'avons pas pu le prendre.
    if result.is_ok() {
        let _ = std::fs::remove_file(&socket);
        info!("cerveau arrêté");
    }
    result
}
