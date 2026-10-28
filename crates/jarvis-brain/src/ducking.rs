//! Baisse du volume du système pendant que Jarvis écoute.

use jarvis_skills::CommandOutput;
use jarvis_skills::CommandRunner;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tracing::{debug, warn};

const SINK: &str = "@DEFAULT_AUDIO_SINK@";
/// Part du volume d'origine conservée pendant l'écoute.
const DUCK_RATIO: f32 = 0.3;
/// Délai maximal d'un appel `wpctl`.
const WPCTL_TIMEOUT: Duration = Duration::from_secs(2);

#[async_trait::async_trait]
pub trait Ducker: Send + Sync {
    /// Baisse le son (sans effet s'il est déjà baissé).
    async fn duck(&self);
    /// Remet le son d'origine (sans effet si rien n'a été baissé).
    async fn restore(&self);
}

/// Baisse le volume de la sortie par défaut avec `wpctl`.
pub struct WpctlDucker {
    runner: Arc<dyn CommandRunner>,
    /// Volume d'origine pendant qu'il est baissé.
    saved: Mutex<Option<f32>>,
}

impl WpctlDucker {
    pub fn new(runner: Arc<dyn CommandRunner>) -> Self {
        Self {
            runner,
            saved: Mutex::new(None),
        }
    }

    /// Lance `wpctl` hors du runtime (`spawn_blocking`), avec un délai maximal.
    async fn wpctl(&self, args: Vec<String>) -> Option<CommandOutput> {
        let runner = Arc::clone(&self.runner);
        let task = tokio::task::spawn_blocking(move || runner.run("wpctl", &args));
        match tokio::time::timeout(WPCTL_TIMEOUT, task).await {
            Ok(Ok(Ok(out))) => Some(out),
            Ok(Ok(Err(e))) => {
                warn!(erreur = %e, "wpctl indisponible");
                None
            }
            Ok(Err(e)) => {
                warn!(erreur = %e, "appel wpctl interrompu");
                None
            }
            Err(_) => {
                warn!("wpctl ne répond pas");
                None
            }
        }
    }

    async fn set_volume(&self, volume: f32) {
        let args = vec!["set-volume".into(), SINK.into(), format!("{volume:.2}")];
        if !self.wpctl(args).await.is_some_and(|o| o.success) {
            warn!("wpctl set-volume a échoué");
        }
    }

    async fn read_volume(&self) -> Option<f32> {
        let args = vec!["get-volume".into(), SINK.into()];
        let out = self.wpctl(args).await.filter(|o| o.success)?;
        // « Volume: 0.65 » ou « Volume: 0.65 [MUTED] »
        out.stdout
            .split_whitespace()
            .nth(1)
            .and_then(|v| v.parse().ok())
    }
}

#[async_trait::async_trait]
impl Ducker for WpctlDucker {
    async fn duck(&self) {
        // Le verrou est gardé pendant tout l'appel : deux `duck` concurrents ne peuvent
        // jamais prendre le volume déjà baissé pour le volume d'origine.
        let mut saved = self.saved.lock().await;
        if saved.is_some() {
            return;
        }
        let Some(volume) = self.read_volume().await else {
            debug!("volume illisible, pas de baisse du son");
            return;
        };
        *saved = Some(volume);
        self.set_volume(volume * DUCK_RATIO).await;
    }

    async fn restore(&self) {
        let mut saved = self.saved.lock().await;
        if let Some(volume) = saved.take() {
            self.set_volume(volume).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jarvis_skills::{CommandOutput, FakeRunner};

    fn ducker() -> (WpctlDucker, Arc<FakeRunner>) {
        let runner = Arc::new(FakeRunner::new());
        runner.respond(
            "wpctl",
            CommandOutput {
                success: true,
                stdout: "Volume: 0.60\n".into(),
            },
        );
        (WpctlDucker::new(runner.clone()), runner)
    }

    fn set_calls(runner: &FakeRunner) -> Vec<String> {
        runner
            .calls()
            .into_iter()
            .filter(|(_, a)| a[0] == "set-volume")
            .map(|(_, a)| a[2].clone())
            .collect()
    }

    #[tokio::test]
    async fn ducker_ne_perd_pas_le_volume() {
        let (d, runner) = ducker();
        d.duck().await;
        d.duck().await;
        d.restore().await;
        assert_eq!(set_calls(&runner), ["0.18", "0.60"]);
    }

    #[tokio::test]
    async fn duck_concurrents_gardent_le_volume_d_origine() {
        let (d, runner) = ducker();
        tokio::join!(d.duck(), d.duck(), d.duck());
        d.restore().await;
        assert_eq!(set_calls(&runner), ["0.18", "0.60"]);
    }

    #[tokio::test]
    async fn restore_sans_baisse_ne_fait_rien() {
        let (d, runner) = ducker();
        d.restore().await;
        assert!(runner.calls().is_empty());
    }
}
