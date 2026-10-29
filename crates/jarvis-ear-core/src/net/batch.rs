//! Regroupe les trames du micro en paquets d'environ 100 ms (1 600 échantillons i16).

use crate::resample::f32_to_i16;

/// Échantillons par paquet (100 ms à 16 kHz).
pub const BATCH_SAMPLES: usize = 1_600;

pub struct AudioBatcher {
    pending: Vec<i16>,
}

impl AudioBatcher {
    pub fn new() -> Self {
        AudioBatcher {
            pending: Vec::with_capacity(BATCH_SAMPLES),
        }
    }

    /// Ajoute des échantillons f32 ; rend les paquets complets.
    pub fn push(&mut self, chunk: &[f32]) -> Vec<Vec<i16>> {
        let mut out = Vec::new();
        for &s in chunk {
            self.pending.push(f32_to_i16(s));
            if self.pending.len() == BATCH_SAMPLES {
                out.push(std::mem::replace(
                    &mut self.pending,
                    Vec::with_capacity(BATCH_SAMPLES),
                ));
            }
        }
        out
    }

    /// Fin de phrase : le reste, s'il y en a.
    pub fn flush(&mut self) -> Option<Vec<i16>> {
        (!self.pending.is_empty()).then(|| std::mem::take(&mut self.pending))
    }
}

impl Default for AudioBatcher {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paquets_de_cent_millisecondes() {
        let mut b = AudioBatcher::new();
        // Trames de 256 (comme le VAD) : un paquet toutes les 6,25 trames.
        let mut packets = Vec::new();
        for _ in 0..13 {
            packets.extend(b.push(&[0.5; 256]));
        }
        assert_eq!(packets.len(), 2);
        assert!(packets.iter().all(|p| p.len() == BATCH_SAMPLES));
        assert_eq!(packets[0][0], 16_384);
        // Un gros bloc donne plusieurs paquets d'un coup.
        let many = b.push(&[-1.0; 2 * BATCH_SAMPLES]);
        assert_eq!(many.len(), 2);
        assert_eq!(many[1][0], -32_768);
        // Reste : 13 × 256 − 3 200 = 128 échantillons, puis plus rien.
        let rest = b.flush().unwrap();
        assert_eq!(rest.len(), 13 * 256 + 2 * BATCH_SAMPLES - 4 * BATCH_SAMPLES);
        assert_eq!(b.flush(), None);
        // Valeurs bornées.
        let p = b.push(&[2.0; BATCH_SAMPLES]);
        assert_eq!(p[0][0], i16::MAX);
    }
}
