//! Délai entre deux tentatives de connexion : 500 ms, doublé à chaque échec,
//! plafonné à 30 s (5 min après un refus du jeton), ±20 % d'aléa.
// Squelette (tâche 1) : à retirer par la tâche 4.
#![allow(dead_code, unused_variables)]

use std::time::Duration;

pub const MIN_DELAY: Duration = Duration::from_millis(500);
pub const MAX_DELAY: Duration = Duration::from_secs(30);
/// Plafond après `Rejected { UnknownEar | BadToken }`.
pub const REJECTED_MAX_DELAY: Duration = Duration::from_secs(300);

pub struct Backoff {
    min: Duration,
    max: Duration,
    factor: u32,
    /// Part d'aléa (0,2 : ±20 %).
    jitter: f64,
    current: Duration,
}

impl Backoff {
    pub fn new() -> Self {
        todo!("tâche 4")
    }

    /// Délai avant la prochaine tentative.
    pub fn next_delay(&mut self) -> Duration {
        todo!("tâche 4")
    }

    /// Connexion réussie : on repart du minimum.
    pub fn reset(&mut self) {
        todo!("tâche 4")
    }

    /// Change le plafond (refus du jeton : [`REJECTED_MAX_DELAY`]).
    pub fn set_max(&mut self, max: Duration) {
        todo!("tâche 4")
    }
}

impl Default for Backoff {
    fn default() -> Self {
        Self::new()
    }
}
