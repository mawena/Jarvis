//! Conversion du flux micro (format du périphérique) vers du mono 16 kHz en `f32`.

use crate::SAMPLE_RATE;

/// Nombre de coefficients du filtre passe-bas (impair).
const TAPS: usize = 47;

/// Convertit un échantillon `s32` en `f32` dans [-1, 1).
pub fn i32_to_f32(s: i32) -> f32 {
    s as f32 / 2_147_483_648.0
}

/// Convertit un échantillon `s16` en `f32` dans [-1, 1).
pub fn i16_to_f32(s: i16) -> f32 {
    s as f32 / 32_768.0
}

/// Convertit un `f32` en `s16` avec saturation.
pub fn f32_to_i16(s: f32) -> i16 {
    (s * 32_768.0).round().clamp(-32_768.0, 32_767.0) as i16
}

enum Mode {
    /// 48 kHz : passe-bas FIR puis décimation par 3.
    Decimate3 { kernel: Vec<f32> },
    /// Autre fréquence : interpolation linéaire.
    Linear { step: f64, pos: f64 },
}

/// Mélange les canaux en mono et ramène le signal à 16 kHz.
pub struct Downmixer {
    channels: usize,
    mode: Mode,
    /// Échantillons mono pas encore entièrement consommés.
    buf: Vec<f32>,
    /// Échantillons incomplets (trame entrelacée coupée entre deux appels).
    partial: Vec<f32>,
    /// Position de la prochaine sortie (mode décimation).
    pos: usize,
}

impl Downmixer {
    /// # Panics
    /// Si la fréquence ou le nombre de canaux vaut 0 (le calcul du pas bouclerait sans fin).
    pub fn new(input_rate: u32, channels: u16) -> Self {
        assert!(input_rate > 0, "fréquence d'entrée nulle");
        assert!(channels > 0, "nombre de canaux nul");
        let mode = if input_rate == 3 * SAMPLE_RATE {
            Mode::Decimate3 {
                kernel: lowpass_kernel(7_000.0 / input_rate as f32),
            }
        } else {
            Mode::Linear {
                step: input_rate as f64 / SAMPLE_RATE as f64,
                pos: 0.0,
            }
        };
        // Amorçage à zéro : une entrée de N échantillons donne exactement N/3 sorties.
        let buf = match mode {
            Mode::Decimate3 { .. } => vec![0.0; TAPS - 1],
            Mode::Linear { .. } => Vec::new(),
        };
        Self {
            channels: channels as usize,
            mode,
            buf,
            partial: Vec::new(),
            pos: 0,
        }
    }

    /// Consomme des échantillons entrelacés (taille quelconque) et renvoie le mono 16 kHz.
    pub fn push(&mut self, interleaved: &[f32]) -> Vec<f32> {
        let ch = self.channels;
        self.partial.extend_from_slice(interleaved);
        let whole = self.partial.len() / ch * ch;
        for frame in self.partial[..whole].chunks_exact(ch) {
            self.buf.push(frame.iter().sum::<f32>() / ch as f32);
        }
        self.partial.drain(..whole);

        let mut out = Vec::with_capacity(self.buf.len() / 3 + 1);
        match &mut self.mode {
            Mode::Decimate3 { kernel } => {
                // Choix : FIR à fenêtre de Hamming (47 coefficients, coupure ~7 kHz à 48 kHz)
                // avant décimation par 3, pour limiter le repliement au-delà de 8 kHz.
                while self.pos + TAPS <= self.buf.len() {
                    let acc: f32 = self.buf[self.pos..self.pos + TAPS]
                        .iter()
                        .zip(kernel.iter())
                        .map(|(x, h)| x * h)
                        .sum();
                    out.push(acc);
                    self.pos += 3;
                }
                let drop = self.pos.min(self.buf.len());
                self.buf.drain(..drop);
                self.pos -= drop;
            }
            Mode::Linear { step, pos } => {
                while *pos + 1.0 < self.buf.len() as f64 {
                    let i = *pos as usize;
                    let frac = (*pos - i as f64) as f32;
                    out.push(self.buf[i] * (1.0 - frac) + self.buf[i + 1] * frac);
                    *pos += *step;
                }
                let drop = (*pos as usize).min(self.buf.len());
                self.buf.drain(..drop);
                *pos -= drop as f64;
            }
        }
        out
    }
}

/// Sinus cardinal fenêtré (Hamming), gain continu normalisé à 1.
/// `cutoff` est exprimé en fraction de la fréquence d'échantillonnage.
fn lowpass_kernel(cutoff: f32) -> Vec<f32> {
    use std::f32::consts::PI;
    let m = (TAPS - 1) as f32 / 2.0;
    let mut k: Vec<f32> = (0..TAPS)
        .map(|n| {
            let x = n as f32 - m;
            let sinc = if x == 0.0 {
                2.0 * cutoff
            } else {
                (2.0 * PI * cutoff * x).sin() / (PI * x)
            };
            let w = 0.54 - 0.46 * (2.0 * PI * n as f32 / (TAPS - 1) as f32).cos();
            sinc * w
        })
        .collect();
    let sum: f32 = k.iter().sum();
    k.iter_mut().for_each(|v| *v /= sum);
    k
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine_stereo_48k(freq: f32, n: usize) -> Vec<f32> {
        (0..n)
            .flat_map(|i| {
                let v = (2.0 * std::f32::consts::PI * freq * i as f32 / 48_000.0).sin() * 0.5;
                [v, v]
            })
            .collect()
    }

    #[test]
    fn downmix_48k_stereo_vers_16k() {
        let mut d = Downmixer::new(48_000, 2);
        let out = d.push(&sine_stereo_48k(440.0, 48_000));
        assert!(
            (out.len() as i64 - 16_000).abs() <= 1,
            "len = {}",
            out.len()
        );
        let peak = out[1_000..].iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!((peak - 0.5).abs() < 0.05, "peak = {peak}");
    }

    #[test]
    fn downmix_morceaux_irreguliers() {
        let input = sine_stereo_48k(440.0, 48_000);
        let whole = Downmixer::new(48_000, 2).push(&input);
        let mut d = Downmixer::new(48_000, 2);
        let mut chunked = Vec::new();
        let mut rest = &input[..];
        for size in [1usize, 7, 333, 1001, 5].iter().cycle() {
            if rest.is_empty() {
                break;
            }
            let n = (*size).min(rest.len());
            chunked.extend(d.push(&rest[..n]));
            rest = &rest[n..];
        }
        assert_eq!(whole.len(), chunked.len());
        for (a, b) in whole.iter().zip(&chunked) {
            assert!((a - b).abs() < 1e-5);
        }
    }

    #[test]
    fn downmix_autre_frequence_interpolation() {
        let mut d = Downmixer::new(44_100, 1);
        let out = d.push(&vec![0.25; 44_100]);
        assert!((out.len() as i64 - 16_000).abs() <= 2);
        assert!(out.iter().all(|v| (v - 0.25).abs() < 1e-5));
    }

    #[test]
    #[should_panic(expected = "fréquence")]
    fn frequence_nulle_refusee() {
        let _ = Downmixer::new(0, 2);
    }

    #[test]
    #[should_panic(expected = "canaux")]
    fn zero_canal_refuse() {
        let _ = Downmixer::new(48_000, 0);
    }

    #[test]
    fn downmix_lineaire_independant_du_decoupage() {
        let input: Vec<f32> = (0..44_100)
            .map(|i| (2.0 * std::f32::consts::PI * 300.0 * i as f32 / 44_100.0).sin() * 0.5)
            .collect();
        let whole = Downmixer::new(44_100, 1).push(&input);
        let mut d = Downmixer::new(44_100, 1);
        let mut chunked = Vec::new();
        let mut rest = &input[..];
        for size in [1usize, 7, 333, 1001, 5].iter().cycle() {
            if rest.is_empty() {
                break;
            }
            let n = (*size).min(rest.len());
            chunked.extend(d.push(&rest[..n]));
            rest = &rest[n..];
        }
        assert_eq!(whole.len(), chunked.len());
        for (a, b) in whole.iter().zip(&chunked) {
            assert!((a - b).abs() < 1e-4);
        }
    }

    #[test]
    fn conversion_saturee() {
        assert_eq!(f32_to_i16(2.0), i16::MAX);
        assert_eq!(f32_to_i16(-2.0), i16::MIN);
        assert_eq!(f32_to_i16(0.0), 0);
        assert!((i32_to_f32(i32::MIN) + 1.0).abs() < 1e-6);
        assert!((i16_to_f32(16_384) - 0.5).abs() < 1e-6);
    }
}
