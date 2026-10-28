//! Baisse du volume du système pendant que Jarvis écoute.

use jarvis_skills::CommandRunner;
use std::sync::{Arc, Mutex};
use tracing::{debug, warn};

const SINK: &str = "@DEFAULT_AUDIO_SINK@";
/// Part du volume d'origine conservée pendant l'écoute.
const DUCK_RATIO: f32 = 0.3;

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

    fn set_volume(&self, volume: f32) {
        let args = vec!["set-volume".into(), SINK.into(), format!("{volume:.2}")];
        match self.runner.run("wpctl", &args) {
            Ok(out) if out.success => {}
            Ok(_) => warn!("wpctl set-volume a échoué"),
            Err(e) => warn!(erreur = %e, "wpctl indisponible"),
        }
    }

    fn read_volume(&self) -> Option<f32> {
        let args = vec!["get-volume".into(), SINK.into()];
        let out = self.runner.run("wpctl", &args).ok().filter(|o| o.success)?;
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
        if self.saved.lock().unwrap().is_some() {
            return;
        }
        let Some(volume) = self.read_volume() else {
            debug!("volume illisible, pas de baisse du son");
            return;
        };
        *self.saved.lock().unwrap() = Some(volume);
        self.set_volume(volume * DUCK_RATIO);
    }

    async fn restore(&self) {
        let saved = self.saved.lock().unwrap().take();
        if let Some(volume) = saved {
            self.set_volume(volume);
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
    async fn restore_sans_baisse_ne_fait_rien() {
        let (d, runner) = ducker();
        d.restore().await;
        assert!(runner.calls().is_empty());
    }
}
