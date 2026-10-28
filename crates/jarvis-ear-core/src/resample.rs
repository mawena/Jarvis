//! Conversion du flux micro (format du périphérique) vers du mono 16 kHz en `f32`, et
//! rééchantillonnage d'un son complet (voix de Jarvis vers la sortie).

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

/// Demi-largeur du filtre de [`resample`], en passages par zéro.
const ZERO_CROSSINGS: f64 = 16.0;
/// Points de la table du filtre par échantillon d'entrée.
const TABLE_STEPS: f64 = 256.0;

/// Rééchantillonne un son mono complet de `from` vers `to` Hz (sinus cardinal fenêtré,
/// passe-bas sous la plus petite des deux moitiés de fréquence).
///
/// # Panics
/// Si une des fréquences vaut 0.
pub fn resample(input: &[f32], from: u32, to: u32) -> Vec<f32> {
    use std::f64::consts::PI;
    assert!(from > 0 && to > 0, "fréquence nulle");
    if from == to {
        return input.to_vec();
    }
    let ratio = to as f64 / from as f64;
    // Coupure en cycles par échantillon d'entrée, avec une marge de transition.
    let cutoff = 0.5 * ratio.min(1.0) * 0.92;
    let half = ZERO_CROSSINGS / (2.0 * cutoff);
    // Filtre tabulé sur [0, half] (il est symétrique), lu par interpolation linéaire.
    let points = (half * TABLE_STEPS).ceil() as usize + 2;
    let table: Vec<f64> = (0..points)
        .map(|k| {
            let d = k as f64 / TABLE_STEPS;
            if d >= half {
                return 0.0;
            }
            let arg = 2.0 * cutoff * d;
            let sinc = if arg == 0.0 {
                1.0
            } else {
                (PI * arg).sin() / (PI * arg)
            };
            // Fenêtre de Blackman.
            let w = 0.42 + 0.5 * (PI * d / half).cos() + 0.08 * (2.0 * PI * d / half).cos();
            2.0 * cutoff * sinc * w
        })
        .collect();
    let kernel = |d: f64| {
        let pos = d.abs() * TABLE_STEPS;
        let k = pos as usize;
        if k + 1 >= points {
            return 0.0;
        }
        let frac = pos - k as f64;
        table[k] * (1.0 - frac) + table[k + 1] * frac
    };
    let len = (input.len() as f64 * ratio).round() as usize;
    (0..len)
        .map(|j| {
            let t = j as f64 / ratio;
            let first = (t - half).ceil().max(0.0) as usize;
            let last = ((t + half).floor() as usize).min(input.len().saturating_sub(1));
            let acc: f64 = input
                .iter()
                .enumerate()
                .take(last + 1)
                .skip(first)
                .map(|(i, &x)| x as f64 * kernel(t - i as f64))
                .sum();
            acc as f32
        })
        .collect()
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

    fn sine(freq: f32, rate: u32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (2.0 * std::f32::consts::PI * freq * i as f32 / rate as f32).sin() * 0.5)
            .collect()
    }

    #[test]
    fn reechantillonnage_meme_frequence_identique() {
        let input = sine(440.0, 22_050, 1_000);
        assert_eq!(resample(&input, 22_050, 22_050), input);
        assert!(resample(&[], 22_050, 48_000).is_empty());
    }

    #[test]
    fn reechantillonnage_22050_vers_48000_fidele() {
        let out = resample(&sine(440.0, 22_050, 22_050), 22_050, 48_000);
        assert_eq!(out.len(), 48_000);
        let ideal = sine(440.0, 48_000, 48_000);
        // Hors des bords, la sinusoïde est reproduite à 1 % près.
        let err = out[1_000..47_000]
            .iter()
            .zip(&ideal[1_000..47_000])
            .fold(0.0f32, |m, (a, b)| m.max((a - b).abs()));
        assert!(err < 0.005, "erreur = {err}");
    }

    #[test]
    fn reechantillonnage_vers_le_bas_sans_repliement() {
        // 10 kHz ne passe pas à 16 kHz (au-delà de 8 kHz) : il doit disparaître.
        let out = resample(&sine(10_000.0, 48_000, 48_000), 48_000, 16_000);
        assert_eq!(out.len(), 16_000);
        let peak = out[500..15_500].iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(peak < 0.01, "pic = {peak}");
        // 1 kHz passe intact.
        let out = resample(&sine(1_000.0, 48_000, 48_000), 48_000, 16_000);
        let peak = out[500..15_500].iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!((peak - 0.5).abs() < 0.01, "pic = {peak}");
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
