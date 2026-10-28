//! Détection d'activité vocale avec `earshot`.

use crate::Vad;

/// Taille d'une trame earshot (16 ms à 16 kHz).
pub const FRAME_LEN: usize = 256;

/// Détecteur de parole `earshot` (réseau embarqué, trames de 256 échantillons).
pub struct EarshotVad {
    detector: Box<::earshot::Detector>,
    threshold: f32,
}

impl EarshotVad {
    /// Seuil par défaut de décision parole / non-parole.
    pub const DEFAULT_THRESHOLD: f32 = 0.5;

    pub fn new() -> Self {
        Self::with_threshold(Self::DEFAULT_THRESHOLD)
    }

    pub fn with_threshold(threshold: f32) -> Self {
        Self {
            detector: ::earshot::Detector::default_boxed(),
            threshold,
        }
    }
}

impl Default for EarshotVad {
    fn default() -> Self {
        Self::new()
    }
}

impl Vad for EarshotVad {
    fn frame_len(&self) -> usize {
        FRAME_LEN
    }

    fn is_speech(&mut self, frame: &[f32]) -> bool {
        if frame.len() != FRAME_LEN {
            return false;
        }
        self.detector.predict_f32(frame) >= self.threshold
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn earshot_silence_n_est_pas_parole() {
        let mut vad = EarshotVad::new();
        let silence = vec![0.0f32; FRAME_LEN];
        for _ in 0..20 {
            assert!(!vad.is_speech(&silence));
        }
    }

    #[test]
    fn earshot_trame_de_256() {
        let mut vad = EarshotVad::new();
        assert_eq!(vad.frame_len(), 256);
        // Une trame de mauvaise taille est ignorée sans panique.
        assert!(!vad.is_speech(&[0.5; 100]));
    }
}
