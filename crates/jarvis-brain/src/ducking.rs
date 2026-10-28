//! Baisse du volume du système pendant que Jarvis écoute.

use jarvis_skills::CommandOutput;
use jarvis_skills::CommandRunner;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

const SINK: &str = "@DEFAULT_AUDIO_SINK@";
/// Part du volume d'origine conservée pendant l'écoute.
const DUCK_RATIO: f32 = 0.3;
/// Délai maximal d'un appel `wpctl`.
const WPCTL_TIMEOUT: Duration = Duration::from_secs(2);
/// Son toujours baissé sans `restore` au bout de ce délai : on le rétablit quand même
/// (phrase de 10 s au plus, plus le temps de Whisper).
const WATCHDOG_DELAY: Duration = Duration::from_secs(15);

#[async_trait::async_trait]
pub trait Ducker: Send + Sync {
    /// Baisse le son (sans effet s'il est déjà baissé).
    async fn duck(&self);
    /// Remet le son d'origine (sans effet si rien n'a été baissé).
    async fn restore(&self);
}

/// Baisse le volume de la sortie par défaut avec `wpctl`.
pub struct WpctlDucker {
    inner: Arc<Inner>,
}

struct Inner {
    runner: Arc<dyn CommandRunner>,
    state: Mutex<State>,
    /// Copie sur disque du volume d'origine : survit à un arrêt brutal du cerveau.
    file: Option<PathBuf>,
}

#[derive(Default)]
struct State {
    /// Volume d'origine pendant qu'il est baissé.
    saved: Option<f32>,
    /// Numéro de la surveillance en cours : toute autre surveillance est périmée.
    generation: u64,
}

impl WpctlDucker {
    /// Sans copie sur disque.
    pub fn new(runner: Arc<dyn CommandRunner>) -> Self {
        Self::build(runner, None)
    }

    /// Garde aussi le volume d'origine dans `file` tant que le son est baissé.
    pub fn persistent(runner: Arc<dyn CommandRunner>, file: PathBuf) -> Self {
        Self::build(runner, Some(file))
    }

    fn build(runner: Arc<dyn CommandRunner>, file: Option<PathBuf>) -> Self {
        Self {
            inner: Arc::new(Inner {
                runner,
                state: Mutex::new(State::default()),
                file,
            }),
        }
    }

    /// `$XDG_RUNTIME_DIR/jarvis/ducked-volume`, à côté du socket du cerveau.
    pub fn default_file() -> PathBuf {
        jarvis_protocol::socket_path().with_file_name("ducked-volume")
    }

    /// Au démarrage : un volume laissé sur disque par un arrêt brutal est rétabli.
    pub async fn recover(&self) {
        let Some(path) = &self.inner.file else { return };
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return,
            Err(e) => {
                warn!(erreur = %e, "volume enregistré illisible");
                return;
            }
        };
        let mut state = self.inner.state.lock().await;
        match text.trim().parse::<f32>() {
            Ok(volume) if volume.is_finite() && volume >= 0.0 => {
                info!("son resté baissé par un arrêt précédent, rétablissement");
                state.saved = Some(volume);
                self.inner.restore_locked(&mut state).await;
            }
            _ => {
                warn!("volume enregistré invalide, ignoré");
                self.inner.forget_file();
            }
        }
    }
}

/// Écriture atomique (fichier temporaire puis renommage), fichier 0600, dossier 0700.
fn write_volume(path: &Path, volume: f32) -> io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    if let Some(parent) = path.parent() {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)?;
    }
    let tmp = path.with_extension("tmp");
    // Un ancien fichier temporaire garderait ses droits.
    let _ = std::fs::remove_file(&tmp);
    let result = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)
        .and_then(|mut f| writeln!(f, "{volume}"))
        .and_then(|()| std::fs::rename(&tmp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

impl Inner {
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

    /// Renvoie `true` si `wpctl` a confirmé le changement.
    async fn set_volume(&self, volume: f32) -> bool {
        let args = vec!["set-volume".into(), SINK.into(), format!("{volume:.2}")];
        self.wpctl(args).await.is_some_and(|o| o.success)
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

    fn save_file(&self, volume: f32) {
        if let Some(path) = &self.file
            && let Err(e) = write_volume(path, volume)
        {
            warn!(erreur = %e, "volume d'origine non enregistré sur disque");
        }
    }

    fn forget_file(&self) {
        if let Some(path) = &self.file
            && let Err(e) = std::fs::remove_file(path)
            && e.kind() != io::ErrorKind::NotFound
        {
            warn!(erreur = %e, "volume enregistré non supprimé");
        }
    }

    /// (Re)lance la surveillance : sans `restore` d'ici `WATCHDOG_DELAY`, le son revient
    /// tout seul. Appelé verrou pris.
    fn arm_watchdog(self: &Arc<Self>, state: &mut State) {
        state.generation += 1;
        let generation = state.generation;
        let inner = Arc::clone(self);
        tokio::spawn(async move {
            tokio::time::sleep(WATCHDOG_DELAY).await;
            let mut state = inner.state.lock().await;
            // Un `restore` ou un nouveau `duck` est passé entre-temps.
            if state.generation != generation {
                return;
            }
            warn!("son toujours baissé sans fin d'écoute, rétablissement automatique");
            inner.restore_locked(&mut state).await;
        });
    }

    async fn restore_locked(&self, state: &mut State) {
        let Some(volume) = state.saved else { return };
        // L'état n'est vidé qu'après un rétablissement confirmé : sinon le prochain
        // `restore` réessaiera, au lieu de laisser le son baissé pour de bon.
        if self.set_volume(volume).await {
            state.saved = None;
            self.forget_file();
            // Annule la surveillance en cours.
            state.generation += 1;
        } else {
            warn!("rétablissement du son non confirmé, nouvel essai au prochain passage");
        }
    }
}

#[async_trait::async_trait]
impl Ducker for WpctlDucker {
    async fn duck(&self) {
        // Le verrou est gardé pendant tout l'appel : deux `duck` concurrents ne peuvent
        // jamais prendre le volume déjà baissé pour le volume d'origine.
        let inner = &self.inner;
        let mut state = inner.state.lock().await;
        if state.saved.is_some() {
            inner.arm_watchdog(&mut state);
            return;
        }
        let Some(volume) = inner.read_volume().await else {
            debug!("volume illisible, pas de baisse du son");
            return;
        };
        // Enregistré avant la baisse : même si `set-volume` échoue ou s'applique en retard,
        // le prochain `restore` remettra le volume d'origine.
        state.saved = Some(volume);
        inner.save_file(volume);
        inner.arm_watchdog(&mut state);
        if !inner.set_volume(volume * DUCK_RATIO).await {
            warn!("baisse du son non confirmée");
        }
    }

    async fn restore(&self) {
        let mut state = self.inner.state.lock().await;
        self.inner.restore_locked(&mut state).await;
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
    async fn restauration_echouee_reessayee() {
        let (d, runner) = ducker();
        d.duck().await;
        runner.respond(
            "wpctl",
            CommandOutput {
                success: false,
                stdout: String::new(),
            },
        );
        d.restore().await;
        runner.respond(
            "wpctl",
            CommandOutput {
                success: true,
                stdout: "Volume: 0.18\n".into(),
            },
        );
        // Le duck suivant ne doit pas prendre le volume baissé pour l'original.
        d.duck().await;
        d.restore().await;
        d.restore().await;
        assert_eq!(set_calls(&runner), ["0.18", "0.60", "0.60"]);
    }

    #[tokio::test]
    async fn duck_concurrents_gardent_le_volume_d_origine() {
        let (d, runner) = ducker();
        tokio::join!(d.duck(), d.duck(), d.duck());
        d.restore().await;
        assert_eq!(set_calls(&runner), ["0.18", "0.60"]);
    }

    #[tokio::test(start_paused = true)]
    async fn sans_listen_le_son_revient_apres_15_s() {
        let (d, runner) = ducker();
        d.duck().await;
        tokio::time::sleep(Duration::from_secs(14)).await;
        assert_eq!(set_calls(&runner), ["0.18"]);
        tokio::time::sleep(Duration::from_secs(2)).await;
        assert_eq!(set_calls(&runner), ["0.18", "0.60"]);
    }

    #[tokio::test(start_paused = true)]
    async fn restauration_normale_annule_la_surveillance() {
        let (d, runner) = ducker();
        d.duck().await;
        d.restore().await;
        tokio::time::sleep(Duration::from_secs(30)).await;
        assert_eq!(set_calls(&runner), ["0.18", "0.60"]);
    }

    #[tokio::test(start_paused = true)]
    async fn nouveau_duck_relance_la_surveillance() {
        let (d, runner) = ducker();
        d.duck().await;
        tokio::time::sleep(Duration::from_secs(10)).await;
        d.duck().await;
        // 24 s après le premier duck, 14 s après le second : toujours baissé.
        tokio::time::sleep(Duration::from_secs(14)).await;
        assert_eq!(set_calls(&runner), ["0.18"]);
        tokio::time::sleep(Duration::from_secs(2)).await;
        assert_eq!(set_calls(&runner), ["0.18", "0.60"]);
    }

    fn persistent(path: &std::path::Path) -> (WpctlDucker, Arc<FakeRunner>) {
        let runner = Arc::new(FakeRunner::new());
        runner.respond(
            "wpctl",
            CommandOutput {
                success: true,
                stdout: "Volume: 0.60\n".into(),
            },
        );
        (
            WpctlDucker::persistent(runner.clone(), path.to_path_buf()),
            runner,
        )
    }

    fn mode(path: &std::path::Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[tokio::test]
    async fn volume_d_origine_sur_disque_pendant_la_baisse() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jarvis/ducked-volume");
        let (d, _) = persistent(&path);
        d.duck().await;
        assert_eq!(std::fs::read_to_string(&path).unwrap().trim(), "0.6");
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(path.parent().unwrap()), 0o700);
        // Écriture atomique : pas de fichier temporaire laissé.
        assert_eq!(
            std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
            1
        );
        d.restore().await;
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn fichier_garde_si_le_retablissement_echoue() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ducked-volume");
        let (d, runner) = persistent(&path);
        d.duck().await;
        runner.respond(
            "wpctl",
            CommandOutput {
                success: false,
                stdout: String::new(),
            },
        );
        d.restore().await;
        assert!(path.exists());
    }

    #[tokio::test]
    async fn volume_retabli_au_demarrage_apres_un_arret_brutal() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ducked-volume");
        std::fs::write(&path, "0.6\n").unwrap();
        let (d, runner) = persistent(&path);
        d.recover().await;
        assert_eq!(set_calls(&runner), ["0.60"]);
        assert!(!path.exists());
        // Rien de plus à rétablir ensuite.
        d.restore().await;
        assert_eq!(set_calls(&runner), ["0.60"]);
    }

    #[tokio::test]
    async fn demarrage_sans_fichier_ne_touche_pas_au_son() {
        let dir = tempfile::tempdir().unwrap();
        let (d, runner) = persistent(&dir.path().join("ducked-volume"));
        d.recover().await;
        assert!(runner.calls().is_empty());
    }

    #[tokio::test]
    async fn fichier_illisible_supprime_sans_toucher_au_son() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ducked-volume");
        std::fs::write(&path, "n'importe quoi").unwrap();
        let (d, runner) = persistent(&path);
        d.recover().await;
        assert!(runner.calls().is_empty());
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn restore_sans_baisse_ne_fait_rien() {
        let (d, runner) = ducker();
        d.restore().await;
        assert!(runner.calls().is_empty());
    }
}
