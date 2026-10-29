//! Regroupe les trames du micro en paquets d'environ 100 ms (1 600 échantillons i16).
// Squelette (tâche 1) : à retirer par la tâche 4.
#![allow(dead_code, unused_variables)]

/// Échantillons par paquet (100 ms à 16 kHz).
pub const BATCH_SAMPLES: usize = 1_600;

pub struct AudioBatcher {
    pending: Vec<i16>,
}

impl AudioBatcher {
    pub fn new() -> Self {
        todo!("tâche 4")
    }

    /// Ajoute des échantillons f32 ; rend les paquets complets.
    pub fn push(&mut self, chunk: &[f32]) -> Vec<Vec<i16>> {
        todo!("tâche 4")
    }

    /// Fin de phrase : le reste, s'il y en a.
    pub fn flush(&mut self) -> Option<Vec<i16>> {
        todo!("tâche 4")
    }
}

impl Default for AudioBatcher {
    fn default() -> Self {
        Self::new()
    }
}
