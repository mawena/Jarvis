//! Annonces faites hors d'une réponse (fin d'une tâche d'agent…).

use crate::Outcome;

/// Une annonce : notification de bureau, et voix si `speak`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    /// Titre de la notification.
    pub title: String,
    /// Phrase dite par l'oreille.
    pub speech: String,
    /// Corps de la notification (détails : branche, journal…).
    pub body: String,
    /// Décide du bip.
    pub outcome: Outcome,
    /// Notification critique.
    pub urgent: bool,
    /// Faux : notification seule, rien n'est dit.
    pub speak: bool,
}

/// Reçoit les annonces des compétences.
pub trait Announcer: Send + Sync {
    fn announce(&self, notice: Notice);
}

/// N'annonce rien.
#[derive(Debug, Default, Clone, Copy)]
pub struct NullAnnouncer;

impl Announcer for NullAnnouncer {
    fn announce(&self, _notice: Notice) {}
}

#[cfg(any(test, feature = "test-util"))]
mod recording {
    use super::{Announcer, Notice};
    use std::sync::Mutex;

    /// Garde les annonces reçues, dans l'ordre (tests).
    #[derive(Debug, Default)]
    pub struct RecordingAnnouncer {
        notices: Mutex<Vec<Notice>>,
    }

    impl RecordingAnnouncer {
        pub fn new() -> Self {
            Self::default()
        }

        pub fn notices(&self) -> Vec<Notice> {
            self.notices.lock().unwrap().clone()
        }
    }

    impl Announcer for RecordingAnnouncer {
        fn announce(&self, notice: Notice) {
            self.notices.lock().unwrap().push(notice);
        }
    }
}

#[cfg(any(test, feature = "test-util"))]
pub use recording::RecordingAnnouncer;

#[cfg(test)]
mod tests {
    use super::*;

    fn notice(speech: &str) -> Notice {
        Notice {
            title: "Jarvis".into(),
            speech: speech.into(),
            body: String::new(),
            outcome: Outcome::Done,
            urgent: false,
            speak: true,
        }
    }

    #[test]
    fn annonces_enregistrees_dans_l_ordre() {
        let a = RecordingAnnouncer::new();
        a.announce(notice("un"));
        a.announce(notice("deux"));
        NullAnnouncer.announce(notice("perdu"));
        let speeches: Vec<String> = a.notices().into_iter().map(|n| n.speech).collect();
        assert_eq!(speeches, ["un", "deux"]);
    }
}
