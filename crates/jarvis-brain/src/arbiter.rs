//! Arbitre des oreilles : une seule oreille gagne par phrase. Logique pure, sans
//! entrée-sortie ; l'heure (`now`) est passée en argument.
// Squelette (tâche 1) : à retirer par la tâche 3.
#![allow(dead_code, unused_variables)]

use jarvis_protocol::EarId;
use std::time::{Duration, Instant};

/// Fenêtre de regroupement des réveils, comptée à l'arrivée au cerveau.
pub const DEFAULT_WINDOW: Duration = Duration::from_millis(300);

/// Réveil d'une oreille, avec ses réglages (`ears.toml`).
#[derive(Debug, Clone, PartialEq)]
pub struct WakeReport {
    pub ear: EarId,
    pub utterance: u32,
    pub score: f32,
    pub level_db: f32,
    pub muted: bool,
    pub bias: f32,
    pub duck: bool,
}

/// `score + 0.5 × clamp((level_db + 60) / 60, 0, 1) + bias`.
pub fn rank(report: &WakeReport) -> f32 {
    todo!("tâche 3")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArbiterAction {
    /// Baisser le son (une fois par phrase).
    Duck,
    Elect {
        ear: EarId,
        utterance: u32,
    },
    Stop {
        ear: EarId,
        utterance: u32,
    },
    /// Rétablir le son.
    Restore,
}

/// Que faire de la fin d'une phrase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndVerdict {
    /// La gagnante : à transcrire.
    Process,
    /// Avant la décision : garder l'audio, rappeler `ended` après `Elect`.
    Wait,
    /// Une perdante, ou une phrase inconnue.
    Ignore,
}

pub struct Arbiter {
    window: Duration,
}

impl Arbiter {
    pub fn new(window: Duration) -> Self {
        todo!("tâche 3")
    }

    pub fn wake(&mut self, report: WakeReport, now: Instant) -> Vec<ArbiterAction> {
        todo!("tâche 3")
    }

    /// Fin de la fenêtre en cours, s'il y en a une.
    pub fn deadline(&self) -> Option<Instant> {
        todo!("tâche 3")
    }

    pub fn tick(&mut self, now: Instant) -> Vec<ArbiterAction> {
        todo!("tâche 3")
    }

    pub fn ended(&mut self, ear: &EarId, utterance: u32) -> EndVerdict {
        todo!("tâche 3")
    }

    /// Capture sans mot d'éveil (`ListenStart`) : acceptée si aucune phrase n'est en cours.
    pub fn follow_up(&mut self, ear: &EarId, utterance: u32) -> bool {
        todo!("tâche 3")
    }

    /// Phrase traitée : le groupe est fermé.
    pub fn finished(&mut self, ear: &EarId, utterance: u32) {
        todo!("tâche 3")
    }

    pub fn disconnected(&mut self, ear: &EarId) -> Vec<ArbiterAction> {
        todo!("tâche 3")
    }

    /// Phrase abandonnée (trop longue…).
    pub fn cancelled(&mut self, ear: &EarId, utterance: u32) -> Vec<ArbiterAction> {
        todo!("tâche 3")
    }
}
