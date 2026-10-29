//! Registre des tâches sur disque : un fichier JSON par tâche, à côté de son
//! journal.

use super::types::{TaskId, TaskRecord, TaskState};
use std::fs::{self, OpenOptions, Permissions};
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// Dossier des tâches (`~/.local/state/jarvis/tasks`).
#[derive(Debug, Clone)]
pub struct Registry {
    dir: PathBuf,
}

impl Registry {
    /// Ouvre le dossier, créé au besoin (0700).
    pub fn open(dir: impl Into<PathBuf>) -> io::Result<Registry> {
        let dir = dir.into();
        fs::create_dir_all(&dir)?;
        fs::set_permissions(&dir, Permissions::from_mode(0o700))?;
        Ok(Registry { dir })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Journal brut d'une tâche.
    pub fn log_path(&self, id: &TaskId) -> PathBuf {
        self.dir.join(format!("{id}.log"))
    }

    fn record_path(&self, id: &TaskId) -> PathBuf {
        self.dir.join(format!("{id}.json"))
    }

    /// Enregistre une tâche (fichier temporaire puis renommage, 0600).
    pub fn save(&self, record: &TaskRecord) -> io::Result<()> {
        let json = serde_json::to_vec_pretty(record).map_err(io::Error::other)?;
        let tmp = self.dir.join(format!(".{}.json.tmp", record.id));
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        file.set_permissions(Permissions::from_mode(0o600))?;
        file.write_all(&json)?;
        file.sync_all()?;
        fs::rename(&tmp, self.record_path(&record.id))
    }

    /// Toutes les tâches, des plus anciennes aux plus récentes. Un fichier
    /// illisible est ignoré.
    pub fn load_all(&self) -> Vec<TaskRecord> {
        read_records(&self.dir)
    }

    /// Au démarrage : les tâches encore `Running` deviennent `Interrupted`.
    pub fn recover(&self, now: u64) -> Vec<TaskRecord> {
        let mut recovered = Vec::new();
        for mut r in self.load_all() {
            if r.state != TaskState::Running {
                continue;
            }
            r.state = TaskState::Interrupted;
            r.ended_at = Some(now);
            if let Err(e) = self.save(&r) {
                tracing::warn!("tâche {} non enregistrée : {e}", r.id);
            }
            recovered.push(r);
        }
        recovered
    }

    /// Garde les `keep` tâches terminées les plus récentes ; retire les autres
    /// et leurs journaux. Une tâche en cours n'est jamais retirée.
    pub fn prune(&self, keep: usize) -> usize {
        let mut done: Vec<TaskRecord> = self
            .load_all()
            .into_iter()
            .filter(|r| r.state != TaskState::Running)
            .collect();
        if done.len() <= keep {
            return 0;
        }
        done.sort_by_key(|r| (r.ended_at.unwrap_or(r.started_at), r.started_at));
        let old = done.len() - keep;
        let mut removed = 0;
        for r in &done[..old] {
            match fs::remove_file(self.record_path(&r.id)) {
                Ok(()) => removed += 1,
                Err(e) => tracing::warn!("tâche {} non retirée : {e}", r.id),
            }
            let _ = fs::remove_file(self.log_path(&r.id));
        }
        removed
    }

    /// Nombre de tâches `Running` dans `dir`, sans rien créer (`jarvisctl`).
    pub fn running_on_disk(dir: &Path) -> usize {
        read_records(dir)
            .iter()
            .filter(|r| r.state == TaskState::Running)
            .count()
    }
}

/// Enregistrements lisibles de `dir`, triés par heure de départ.
fn read_records(dir: &Path) -> Vec<TaskRecord> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut records: Vec<TaskRecord> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .filter_map(|p| {
            let parsed = fs::read(&p)
                .map_err(|e| e.to_string())
                .and_then(|b| serde_json::from_slice(&b).map_err(|e| e.to_string()));
            parsed
                .inspect_err(|e| tracing::warn!("tâche illisible {} : {e}", p.display()))
                .ok()
        })
        .filter(|r: &TaskRecord| {
            let ok = valid_id(&r.id);
            if !ok {
                tracing::warn!("tâche ignorée : identifiant invalide {:?}", r.id.as_str());
            }
            ok
        })
        .collect();
    records.sort_by(|a, b| (a.started_at, &a.id).cmp(&(b.started_at, &b.id)));
    records
}

/// Identifiant sûr dans un nom de fichier : forme d'UUID (`[0-9a-f-]`).
fn valid_id(id: &TaskId) -> bool {
    let s = id.as_str();
    !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b) || b == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::types::TaskState;
    use std::os::unix::fs::PermissionsExt;

    fn tache(reg: &Registry, id: &str, started_at: u64, state: TaskState) -> TaskRecord {
        let id = TaskId(id.into());
        TaskRecord {
            log: reg.log_path(&id),
            id,
            project: "Jarvis".into(),
            instruction: "finis les tests".into(),
            branch: "feature/jarvis-finis-tests".into(),
            worktree: PathBuf::from("/tmp/w"),
            base: "develop".into(),
            base_sha: "0123".into(),
            started_at,
            ended_at: (state != TaskState::Running).then_some(started_at + 5),
            state,
            summary: None,
            files_changed: None,
            violations: vec![],
            cost_usd: None,
        }
    }

    fn mode(p: &Path) -> u32 {
        std::fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    fn ids(records: &[TaskRecord]) -> Vec<&str> {
        records.iter().map(|r| r.id.as_str()).collect()
    }

    #[test]
    fn registre_aller_retour() {
        let tmp = tempfile::tempdir().unwrap();
        let reg = Registry::open(tmp.path().join("tasks")).unwrap();
        let b = tache(&reg, "b", 20, TaskState::Running);
        let mut a = tache(&reg, "a", 10, TaskState::Succeeded);
        a.summary = Some("Tests réparés.".into());
        reg.save(&b).unwrap();
        reg.save(&a).unwrap();
        assert_eq!(reg.load_all(), vec![a.clone(), b.clone()]);
        // Nouvel enregistrement de la même tâche : remplacé.
        a.files_changed = Some(3);
        reg.save(&a).unwrap();
        assert_eq!(reg.load_all(), vec![a, b]);
        assert_eq!(
            reg.log_path(&TaskId("a".into())),
            tmp.path().join("tasks/a.log")
        );
        // Aucun fichier temporaire laissé.
        let noms: Vec<_> = std::fs::read_dir(reg.dir())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(noms.len(), 2, "{noms:?}");
    }

    #[test]
    fn registre_marque_les_taches_interrompues() {
        let tmp = tempfile::tempdir().unwrap();
        let reg = Registry::open(tmp.path()).unwrap();
        reg.save(&tache(&reg, "a", 10, TaskState::Running)).unwrap();
        reg.save(&tache(&reg, "b", 20, TaskState::Failed)).unwrap();
        assert_eq!(Registry::running_on_disk(tmp.path()), 1);
        let rattrapees = reg.recover(99);
        assert_eq!(ids(&rattrapees), ["a"]);
        assert_eq!(rattrapees[0].state, TaskState::Interrupted);
        assert_eq!(rattrapees[0].ended_at, Some(99));
        let a = &reg.load_all()[0];
        assert_eq!((a.state, a.ended_at), (TaskState::Interrupted, Some(99)));
        assert_eq!(Registry::running_on_disk(tmp.path()), 0);
        assert!(reg.recover(100).is_empty());
        assert_eq!(Registry::running_on_disk(&tmp.path().join("absent")), 0);
        assert!(!tmp.path().join("absent").exists());
    }

    #[test]
    fn registre_ignore_un_fichier_corrompu() {
        let tmp = tempfile::tempdir().unwrap();
        let reg = Registry::open(tmp.path()).unwrap();
        reg.save(&tache(&reg, "a", 10, TaskState::Running)).unwrap();
        std::fs::write(tmp.path().join("casse.json"), "{ pas du json").unwrap();
        std::fs::write(tmp.path().join("a.log"), "journal").unwrap();
        assert_eq!(ids(&reg.load_all()), ["a"]);
        assert_eq!(Registry::running_on_disk(tmp.path()), 1);
        assert_eq!(ids(&reg.recover(50)), ["a"]);
    }

    #[test]
    fn registre_purge_les_anciennes() {
        let tmp = tempfile::tempdir().unwrap();
        let reg = Registry::open(tmp.path()).unwrap();
        let taches = [
            tache(&reg, "e0-c0", 1, TaskState::Running),
            tache(&reg, "01d", 10, TaskState::Succeeded),
            tache(&reg, "02d", 20, TaskState::Failed),
            tache(&reg, "03d", 30, TaskState::Stopped),
        ];
        for t in &taches {
            reg.save(t).unwrap();
            std::fs::write(&t.log, "journal").unwrap();
        }
        assert_eq!(reg.prune(2), 1);
        assert_eq!(ids(&reg.load_all()), ["e0-c0", "02d", "03d"]);
        assert!(!taches[1].log.exists());
        assert!(taches[0].log.exists());
        assert_eq!(reg.prune(0), 2);
        assert_eq!(ids(&reg.load_all()), ["e0-c0"]);
        assert!(taches[0].log.exists());
        assert_eq!(reg.prune(0), 0);
    }

    #[test]
    fn identifiant_invalide_ignore_par_la_purge() {
        let tmp = tempfile::tempdir().unwrap();
        let reg = Registry::open(tmp.path().join("tasks")).unwrap();
        // Fichier hors du registre que l'identifiant viserait.
        let victime = tmp.path().join("victime.log");
        std::fs::write(&victime, "à garder").unwrap();
        for (file, id) in [
            ("piege.json", "../victime"),
            ("majuscules.json", "ABC"),
            ("vide.json", ""),
        ] {
            let mut t = tache(&reg, "0a", 1, TaskState::Succeeded);
            t.id = TaskId(id.into());
            std::fs::write(reg.dir().join(file), serde_json::to_vec(&t).unwrap()).unwrap();
        }
        reg.save(&tache(&reg, "0b", 5, TaskState::Running)).unwrap();
        assert_eq!(ids(&reg.load_all()), ["0b"]);
        assert_eq!(reg.prune(0), 0);
        assert!(reg.recover(9).iter().all(|r| r.id.as_str() == "0b"));
        assert!(victime.exists());
        assert!(!tmp.path().join("victime.json").exists());
    }

    #[test]
    fn registre_permissions_privees() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("state/tasks");
        let reg = Registry::open(&dir).unwrap();
        assert_eq!(mode(&dir), 0o700);
        let t = tache(&reg, "a", 10, TaskState::Running);
        reg.save(&t).unwrap();
        assert_eq!(mode(&dir.join("a.json")), 0o600);
        // Dossier existant trop ouvert : refermé.
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        Registry::open(&dir).unwrap();
        assert_eq!(mode(&dir), 0o700);
    }
}
