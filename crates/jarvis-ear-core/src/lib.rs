//! Logique pure de l'oreille de Jarvis : traits de détection, états du micro.

pub mod earshot;
pub mod listener;
pub mod resample;
pub mod rustpotter;

/// Fréquence d'échantillonnage de tout le traitement audio (Hz, mono).
pub const SAMPLE_RATE: u32 = 16_000;

/// Détecteur de mot d'éveil.
pub trait WakeDetector: Send {
    /// Nombre d'échantillons attendus par trame.
    fn frame_len(&self) -> usize;
    /// Traite une trame ; renvoie le score si « Jarvis » est détecté.
    fn process(&mut self, frame: &[f32]) -> Option<f32>;
}

/// Détecteur d'activité vocale.
pub trait Vad: Send {
    /// Nombre d'échantillons attendus par trame (256).
    fn frame_len(&self) -> usize;
    fn is_speech(&mut self, frame: &[f32]) -> bool;
}

/// État du micro.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MicState {
    /// Écoute normale.
    Open,
    /// Écoute coupée : le mot d'éveil reste détecté.
    Muted,
    /// Coupure totale : flux fermé.
    HardMuted,
    /// Aucun micro disponible.
    Unavailable,
}
