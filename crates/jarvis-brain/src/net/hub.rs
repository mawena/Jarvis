//! Hub des oreilles : une seule tâche possède l'arbitre, les tampons des phrases,
//! les sessions et la fenêtre d'appairage. `Hub` en est la poignée (clonable).
// Squelette (tâche 1) : à retirer par la tâche 6.
#![allow(dead_code, unused_variables)]

use jarvis_protocol::EarId;

#[derive(Clone)]
pub struct Hub {
    // Canal vers la tâche du hub : tâche 6.
}

impl Hub {
    /// Session fermée : l'arbitre en est prévenu (son rétabli si besoin).
    pub fn disconnected(&self, ear: &EarId) {
        todo!("tâche 6")
    }
}
