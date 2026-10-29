//! Fenêtre d'appairage ouverte par `jarvis pair` : code à 6 chiffres, usage unique,
//! 5 essais au plus, 2 minutes. Horloge injectée (`now` en argument).
// Squelette (tâche 1) : à retirer par la tâche 2.
#![allow(dead_code, unused_variables)]

use std::time::{Duration, Instant};

/// Durée d'ouverture de la fenêtre.
pub const PAIRING_TTL: Duration = Duration::from_secs(120);
/// Essais de code au plus.
pub const MAX_ATTEMPTS: u8 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairAttempt {
    /// Code juste : rendu une seule fois, la fenêtre se ferme.
    Accepted,
    WrongCode {
        attempts_left: u8,
    },
    /// Fenêtre expirée, déjà utilisée ou essais épuisés.
    Closed,
}

pub struct PairingWindow {
    code: String,
    expires_at: Instant,
    attempts_left: u8,
    used: bool,
}

impl PairingWindow {
    /// Ouvre une fenêtre ; rend aussi le code (tirage sans biais).
    pub fn open(now: Instant, ttl: Duration) -> (Self, String) {
        todo!("tâche 2")
    }

    pub fn try_code(&mut self, code: &str, now: Instant) -> PairAttempt {
        todo!("tâche 2")
    }

    pub fn is_open(&self, now: Instant) -> bool {
        todo!("tâche 2")
    }

    pub fn expires_at(&self) -> Instant {
        todo!("tâche 2")
    }
}
