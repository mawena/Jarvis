//! Bips de l'oreille, générés à la fréquence voulue (mono, `f32`).

use std::f32::consts::PI;

/// Volume des bips.
const VOLUME: f32 = 0.3;
/// Fondu en début et en fin de note (évite les clics).
const FADE_MS: f32 = 10.0;

/// « Jarvis » entendu : court bip montant (660 → 880 Hz, 120 ms).
pub fn wake(rate: u32) -> Vec<f32> {
    sweep(rate, 660.0, 880.0, 120.0)
}

/// Réussite : deux notes montantes.
pub fn success(rate: u32) -> Vec<f32> {
    notes(rate, &[(660.0, 70.0), (990.0, 90.0)])
}

/// Échec : deux notes graves descendantes.
pub fn failure(rate: u32) -> Vec<f32> {
    notes(rate, &[(400.0, 90.0), (300.0, 140.0)])
}

/// Notes `(fréquence, durée en ms)` séparées par un court silence.
fn notes(rate: u32, list: &[(f32, f32)]) -> Vec<f32> {
    let gap = vec![0.0; ms(rate, 20.0)];
    let mut out = Vec::new();
    for (i, &(freq, dur)) in list.iter().enumerate() {
        if i > 0 {
            out.extend_from_slice(&gap);
        }
        out.extend(sweep(rate, freq, freq, dur));
    }
    out
}

/// Sinusoïde dont la fréquence glisse de `from` à `to` Hz, avec fondus.
fn sweep(rate: u32, from: f32, to: f32, dur_ms: f32) -> Vec<f32> {
    let n = ms(rate, dur_ms);
    let fade = ms(rate, FADE_MS).min(n / 2).max(1);
    let mut phase = 0.0f32;
    (0..n)
        .map(|i| {
            let progress = i as f32 / n as f32;
            let freq = from + (to - from) * progress;
            let v = phase.sin();
            phase = (phase + 2.0 * PI * freq / rate as f32) % (2.0 * PI);
            // Enveloppe en cosinus surélevé sur les bords (nulle au premier et au dernier).
            let edge = i.min(n - 1 - i);
            let gain = if edge < fade {
                0.5 - 0.5 * (PI * edge as f32 / fade as f32).cos()
            } else {
                1.0
            };
            v * gain * VOLUME
        })
        .collect()
}

fn ms(rate: u32, ms: f32) -> usize {
    (rate as f32 * ms / 1000.0).round() as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peak(s: &[f32]) -> f32 {
        s.iter().fold(0.0f32, |m, v| m.max(v.abs()))
    }

    /// Plus grand saut entre deux échantillons voisins.
    fn max_step(s: &[f32]) -> f32 {
        s.windows(2).fold(0.0f32, |m, w| m.max((w[1] - w[0]).abs()))
    }

    fn all(rate: u32) -> [(&'static str, Vec<f32>); 3] {
        [
            ("réveil", wake(rate)),
            ("réussite", success(rate)),
            ("échec", failure(rate)),
        ]
    }

    #[test]
    fn durees_attendues() {
        assert_eq!(wake(48_000).len(), 5_760);
        assert_eq!(wake(16_000).len(), 1_920);
        // 70 + 20 + 90 ms.
        assert_eq!(success(48_000).len(), 8_640);
        // 90 + 20 + 140 ms.
        assert_eq!(failure(48_000).len(), 12_000);
    }

    #[test]
    fn volume_modere() {
        for rate in [16_000, 44_100, 48_000] {
            for (name, s) in all(rate) {
                let p = peak(&s);
                assert!(p <= VOLUME + 1e-4 && p > VOLUME * 0.9, "{name} : pic {p}");
            }
        }
    }

    #[test]
    fn sans_clic() {
        for rate in [16_000, 48_000] {
            for (name, s) in all(rate) {
                assert!(s[0].abs() < 1e-3, "{name} : début {}", s[0]);
                assert!(s[s.len() - 1].abs() < 1e-3, "{name} : fin");
                // Pas de saut plus grand que la pente d'une sinusoïde à 1 kHz.
                let limit = 2.0 * PI * 1_000.0 * VOLUME / rate as f32;
                assert!(max_step(&s) < limit, "{name} : saut {}", max_step(&s));
            }
        }
    }

    #[test]
    fn reveil_monte() {
        // Moins de passages par zéro dans la première moitié que dans la seconde.
        let s = wake(48_000);
        let crossings = |s: &[f32]| s.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
        let (a, b) = s.split_at(s.len() / 2);
        assert!(
            crossings(a) < crossings(b),
            "{} / {}",
            crossings(a),
            crossings(b)
        );
    }
}
