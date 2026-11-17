//! Mesures des critères de réussite (spec §9), côté cerveau : le hub ouvre une
//! [`Tracker`] autour du traitement de chaque phrase, le `Brain` et l'écoute y notent la
//! route et l'éventuel rejet, puis le hub écrit une ligne dans `mesures.jsonl`.

use jarvis_protocol::ControlResponse;
use jarvis_protocol::metrics::{Outcome, PhraseRecord, Route};
use std::cell::Cell;
use std::fs::{self, OpenOptions};
use std::future::Future;
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tracing::warn;

/// Taille au-delà de laquelle le fichier est renommé en `.1` (l'ancien `.1` est perdu).
pub const MAX_BYTES: u64 = 5 * 1024 * 1024;

/// Notes prises pendant le traitement d'une phrase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Notes {
    pub route: Route,
    /// Phrase écartée sans réponse (voix inconnue, phrase courte non vérifiée…).
    pub ignored: bool,
}

impl Default for Notes {
    fn default() -> Self {
        Self {
            route: Route::Aucune,
            ignored: false,
        }
    }
}

tokio::task_local! {
    static NOTES: Cell<Notes>;
}

/// Exécute `job` en recueillant ses notes.
pub async fn track<F: Future>(job: F) -> (F::Output, Notes) {
    NOTES
        .scope(Cell::new(Notes::default()), async {
            let out = job.await;
            (out, NOTES.with(Cell::get))
        })
        .await
}

fn update(f: impl FnOnce(&mut Notes)) {
    let _ = NOTES.try_with(|cell| {
        let mut notes = cell.get();
        f(&mut notes);
        cell.set(notes);
    });
}

/// Route de compréhension (hors d'un [`track`] : rien).
pub fn note_route(route: Route) {
    update(|n| n.route = route);
}

/// Phrase écartée sans réponse.
pub fn note_ignored() {
    update(|n| n.ignored = true);
}

/// Issue d'une phrase d'après sa réponse et les notes.
pub fn outcome(response: &ControlResponse, notes: Notes) -> Outcome {
    match response {
        ControlResponse::Error { .. } => Outcome::Erreur,
        _ if notes.ignored => Outcome::Ignoree,
        ControlResponse::Heard { text, .. } if text.is_empty() => Outcome::Vide,
        _ => Outcome::Comprise,
    }
}

/// Fichier des mesures (dossier 0700, fichier 0600).
pub struct MetricsLog {
    path: PathBuf,
    lock: Mutex<()>,
}

impl MetricsLog {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            lock: Mutex::new(()),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Ajoute une ligne ; une erreur est journalisée, jamais remontée.
    pub fn append(&self, record: &PhraseRecord) {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        if let Err(e) = self.write(record) {
            warn!(erreur = %e, fichier = %self.path.display(), "mesure non enregistrée");
        }
    }

    fn write(&self, record: &PhraseRecord) -> io::Result<()> {
        if let Some(dir) = self.path.parent() {
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(dir)?;
        }
        if fs::metadata(&self.path).is_ok_and(|m| m.len() >= MAX_BYTES) {
            fs::rename(&self.path, self.path.with_extension("jsonl.1"))?;
        }
        let mut line = serde_json::to_string(record).map_err(io::Error::other)?;
        line.push('\n');
        OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(&self.path)?
            .write_all(line.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jarvis_protocol::metrics::parse_lines;
    use std::os::unix::fs::PermissionsExt;

    fn rec(text: &str) -> PhraseRecord {
        PhraseRecord {
            at_ms: 1,
            ear: "PC".into(),
            ms: 420,
            route: Route::Raccourci,
            outcome: Outcome::Comprise,
            text: text.into(),
        }
    }

    #[test]
    fn lignes_ajoutees_fichier_prive() {
        let dir = tempfile::tempdir().unwrap();
        let log = MetricsLog::new(dir.path().join("etat/mesures.jsonl"));
        log.append(&rec("suivant"));
        log.append(&rec("pause"));
        let text = fs::read_to_string(log.path()).unwrap();
        assert_eq!(parse_lines(&text), [rec("suivant"), rec("pause")]);
        let mode = fs::metadata(log.path()).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let dir_mode = fs::metadata(dir.path().join("etat"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(dir_mode & 0o777, 0o700);
    }

    #[test]
    fn rotation_au_dela_de_la_taille() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mesures.jsonl");
        fs::write(&path, vec![b'x'; MAX_BYTES as usize]).unwrap();
        let log = MetricsLog::new(&path);
        log.append(&rec("suivant"));
        assert_eq!(
            parse_lines(&fs::read_to_string(&path).unwrap()),
            [rec("suivant")]
        );
        assert_eq!(
            fs::metadata(dir.path().join("mesures.jsonl.1"))
                .unwrap()
                .len(),
            MAX_BYTES
        );
    }

    #[test]
    fn ecriture_impossible_sans_panique() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("fichier"), "").unwrap();
        MetricsLog::new(dir.path().join("fichier/mesures.jsonl")).append(&rec("x"));
    }

    #[tokio::test]
    async fn notes_recueillies_dans_le_traitement() {
        let ((), notes) = track(async {
            note_route(Route::Raccourci);
            note_route(Route::IaLocale);
        })
        .await;
        assert_eq!(notes.route, Route::IaLocale);
        assert!(!notes.ignored);
        // Hors traitement : sans effet.
        note_ignored();
        let ((), notes) = track(async { note_ignored() }).await;
        assert!(notes.ignored);
    }

    #[test]
    fn issue_selon_la_reponse() {
        let heard = |text: &str| ControlResponse::Heard {
            text: text.into(),
            speech: String::new(),
            ear_action: None,
            audio: None,
            sound: None,
        };
        let n = Notes::default();
        let ignored = Notes { ignored: true, ..n };
        assert_eq!(outcome(&heard(""), n), Outcome::Vide);
        assert_eq!(outcome(&heard(""), ignored), Outcome::Ignoree);
        assert_eq!(outcome(&heard("suivant"), n), Outcome::Comprise);
        let error = ControlResponse::Error {
            message: "x".into(),
        };
        assert_eq!(outcome(&error, n), Outcome::Erreur);
    }
}
