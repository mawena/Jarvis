//! Qui parle ? (phase 8) SQUELETTE de la tâche 1 : la fonction pure `identify` est
//! écrite par la tâche 6 (table « Décisions » du plan).

use crate::people::{People, Speaker};
use crate::speaker::Verdict;
use jarvis_protocol::EarId;

/// Décision de l'écoute pour une phrase.
#[derive(Debug, Clone, PartialEq)]
pub enum Identity {
    /// Phrase ignorée (voix inconnue sur l'oreille du propriétaire).
    Ignore,
    /// Phrase attribuée à `speaker` ; `short_only` : raccourcis seulement (jamais l'IA).
    Speak { speaker: Speaker, short_only: bool },
}

/// `verdict` : `None` si aucune vérification (désactivée ou installation neuve).
pub fn identify(_people: &People, _ear: &EarId, _verdict: Option<Verdict>) -> Identity {
    todo!("tâche 6")
}
