use jarvis_protocol::Mode;
use std::io;
use std::path::PathBuf;
use tokio::sync::watch;

/// Mode persistant : un fichier contenant `local` ou `online`.
#[derive(Debug, Clone)]
pub struct ModeStore {
    pub path: PathBuf,
    pub initial: Mode,
    /// Chaque mode enregistré (CLI, icône ou voix) y est publié.
    changes: watch::Sender<Mode>,
}

impl ModeStore {
    pub fn new(path: PathBuf, initial: Mode) -> Self {
        Self {
            path,
            initial,
            changes: watch::Sender::new(initial),
        }
    }

    /// Changements de mode, après leur écriture sur disque.
    pub fn subscribe(&self) -> watch::Receiver<Mode> {
        self.changes.subscribe()
    }

    /// `~/.local/state/jarvis/mode`
    pub fn default_path() -> PathBuf {
        match directories::ProjectDirs::from("", "", "jarvis") {
            Some(dirs) => dirs
                .state_dir()
                .unwrap_or_else(|| dirs.data_local_dir())
                .join("mode"),
            None => PathBuf::from(".local/state/jarvis/mode"),
        }
    }

    /// Fichier absent, illisible ou corrompu : renvoie `initial`.
    pub fn get(&self) -> Mode {
        std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(self.initial)
    }

    pub fn set(&self, mode: Mode) -> io::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let word = match mode {
            Mode::Local => "local",
            Mode::Online => "online",
        };
        // Écriture atomique : fichier temporaire du même dossier, puis renommage.
        let tmp = self.path.with_extension("tmp");
        std::fs::write(&tmp, format!("{word}\n"))?;
        std::fs::rename(&tmp, &self.path).inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })?;
        self.changes.send_replace(mode);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jarvis_protocol::Mode;

    #[test]
    fn mode_persiste() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sous/dossier/mode");
        ModeStore::new(path.clone(), Mode::Local)
            .set(Mode::Online)
            .unwrap();
        assert_eq!(ModeStore::new(path, Mode::Local).get(), Mode::Online);
    }

    #[test]
    fn ecriture_atomique_sans_reste() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mode");
        let store = ModeStore::new(path.clone(), Mode::Local);
        store.set(Mode::Online).unwrap();
        store.set(Mode::Local).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "local\n");
        let names: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
        assert_eq!(names.len(), 1);
    }

    #[test]
    fn changement_de_mode_publie() {
        let dir = tempfile::tempdir().unwrap();
        let store = ModeStore::new(dir.path().join("mode"), Mode::Local);
        let mut rx = store.subscribe();
        assert!(!rx.has_changed().unwrap());
        // Une copie publie sur le même canal.
        store.clone().set(Mode::Online).unwrap();
        assert!(rx.has_changed().unwrap());
        assert_eq!(*rx.borrow_and_update(), Mode::Online);
    }

    #[test]
    fn mode_fichier_corrompu() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mode");
        std::fs::write(&path, "n'importe quoi").unwrap();
        assert_eq!(ModeStore::new(path, Mode::Online).get(), Mode::Online);
    }
}
