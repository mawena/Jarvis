//! Enrôlement du mot d'éveil : enregistre « Jarvis » plusieurs fois puis
//! construit `jarvis.rpw`. Les WAV sont les seuls sons conservés sur disque.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, anyhow};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use jarvis_ear_core::SAMPLE_RATE;
use jarvis_ear_core::rustpotter::build_reference;

/// Durée de chaque prise (secondes).
const TAKE_SECONDS: f32 = 1.5;

/// Dossier `~/.local/share/jarvis/wakeword`.
pub fn wakeword_dir() -> anyhow::Result<PathBuf> {
    let base = directories::BaseDirs::new().context("dossier personnel introuvable")?;
    Ok(base.data_dir().join("jarvis").join("wakeword"))
}

/// Nom de fichier de la prise `index` (à partir de 1).
pub fn sample_file_name(index: usize) -> String {
    format!("jarvis-{index:02}.wav")
}

/// Nombre d'échantillons mono 16 kHz d'une prise.
#[cfg(test)]
pub fn samples_per_take() -> usize {
    (SAMPLE_RATE as f32 * TAKE_SECONDS) as usize
}

/// Mélange en mono (moyenne des canaux) et ramène à 16 kHz (moyenne par fenêtre).
pub fn to_mono_16k(interleaved: &[f32], channels: usize, rate: u32) -> Vec<f32> {
    let channels = channels.max(1);
    let mono: Vec<f32> = interleaved
        .chunks_exact(channels)
        .map(|f| f.iter().sum::<f32>() / channels as f32)
        .collect();
    if rate == SAMPLE_RATE {
        return mono;
    }
    let ratio = rate as f64 / SAMPLE_RATE as f64;
    let out_len = (mono.len() as f64 / ratio) as usize;
    (0..out_len)
        .map(|i| {
            let start = (i as f64 * ratio) as usize;
            let end = (((i + 1) as f64 * ratio) as usize).clamp(start + 1, mono.len());
            mono[start..end].iter().sum::<f32>() / (end - start) as f32
        })
        .collect()
}

/// Écrit une prise en WAV 16 bits mono 16 kHz.
pub fn write_wav(path: &Path, samples: &[f32]) -> anyhow::Result<()> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec)?;
    for s in samples {
        writer.write_sample((s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)?;
    }
    writer.finalize()?;
    Ok(())
}

/// Liste triée des WAV d'échantillons d'un dossier.
pub fn list_samples(dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "wav"))
        .collect();
    files.sort();
    Ok(files)
}

/// Construit `jarvis.rpw` à partir des WAV du dossier d'échantillons.
pub fn build_model(samples_dir: &Path, output: &Path) -> anyhow::Result<usize> {
    let files = list_samples(samples_dir)?;
    build_reference(&files, output)?;
    Ok(files.len())
}

/// Enregistre une prise de `TAKE_SECONDS` secondes au format natif du micro.
fn record_take() -> anyhow::Result<Vec<f32>> {
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or_else(|| anyhow!("aucun micro disponible"))?;
    let supported = device.default_input_config()?;
    let channels = supported.channels() as usize;
    let rate = supported.sample_rate();
    let config = supported.config();
    let wanted = (rate as f32 * TAKE_SECONDS) as usize * channels;
    // Capacité réservée d'avance : le callback n'alloue pas.
    let buffer = Arc::new(Mutex::new(Vec::<f32>::with_capacity(wanted + 16_384)));
    let sink = buffer.clone();
    let on_err = |e| eprintln!("erreur micro : {e}");

    macro_rules! stream {
        ($t:ty) => {
            device.build_input_stream(
                config,
                move |data: &[$t], _| {
                    if let Ok(mut b) = sink.try_lock() {
                        let room = wanted.saturating_sub(b.len());
                        b.extend(
                            data.iter()
                                .take(room)
                                .map(|s| cpal::Sample::to_sample::<f32>(*s)),
                        );
                    }
                },
                on_err,
                None,
            )?
        };
    }
    let stream = match supported.sample_format() {
        cpal::SampleFormat::F32 => stream!(f32),
        cpal::SampleFormat::I16 => stream!(i16),
        cpal::SampleFormat::I32 => stream!(i32),
        cpal::SampleFormat::U16 => stream!(u16),
        other => return Err(anyhow!("format de micro non géré : {other:?}")),
    };
    stream.play()?;
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while buffer.lock().map_err(|_| anyhow!("verrou"))?.len() < wanted {
        if std::time::Instant::now() > deadline {
            return Err(anyhow!("le micro ne délivre pas d'audio"));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(stream);
    let raw = buffer.lock().map_err(|_| anyhow!("verrou"))?.clone();
    Ok(to_mono_16k(&raw, channels, rate))
}

/// Point d'entrée de `jarvis-ear enroll`.
pub fn run(count: usize) -> anyhow::Result<()> {
    if count < 3 {
        return Err(anyhow!("il faut au moins 3 prises (8 recommandées)"));
    }
    let dir = wakeword_dir()?;
    let samples_dir = dir.join("samples");
    std::fs::create_dir_all(&samples_dir)?;
    let stdin = std::io::stdin();
    println!("Enrôlement du mot d'éveil : dis « Jarvis » à chaque prise ({count} au total).");
    println!(
        "Les WAV sont conservés dans {} pour reconstruire le modèle.",
        samples_dir.display()
    );
    for i in 1..=count {
        print!("Prise {i}/{count} : appuie sur Entrée puis dis « Jarvis »… ");
        std::io::stdout().flush()?;
        stdin.lock().read_line(&mut String::new())?;
        let take = record_take()?;
        write_wav(&samples_dir.join(sample_file_name(i)), &take)?;
        println!("ok");
    }
    let output = dir.join("jarvis.rpw");
    let n = build_model(&samples_dir, &output)?;
    println!("{} créé à partir de {n} échantillons.", output.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nom_de_fichier_des_prises() {
        assert_eq!(sample_file_name(1), "jarvis-01.wav");
        assert_eq!(sample_file_name(12), "jarvis-12.wav");
    }

    #[test]
    fn nombre_d_echantillons_par_prise() {
        assert_eq!(samples_per_take(), 24_000);
    }

    #[test]
    fn mono_et_reechantillonnage_48k() {
        let stereo: Vec<f32> = (0..4800).flat_map(|_| [0.2, 0.4]).collect();
        let out = to_mono_16k(&stereo, 2, 48_000);
        assert_eq!(out.len(), 1600);
        assert!(out.iter().all(|s| (s - 0.3).abs() < 1e-6));
    }

    #[test]
    fn construction_du_modele_depuis_des_wav() {
        let dir = tempfile::tempdir().unwrap();
        let samples = dir.path().join("samples");
        std::fs::create_dir_all(&samples).unwrap();
        for i in 1..=3 {
            let f = 300.0 + 50.0 * i as f32;
            let take: Vec<f32> = (0..samples_per_take())
                .map(|n| {
                    let t = n as f32 / SAMPLE_RATE as f32;
                    (t * f * std::f32::consts::TAU).sin() * (t * 2.0).sin().abs() * 0.5
                })
                .collect();
            write_wav(&samples.join(sample_file_name(i)), &take).unwrap();
        }
        let rpw = dir.path().join("jarvis.rpw");
        assert_eq!(build_model(&samples, &rpw).unwrap(), 3);
        assert!(rpw.metadata().unwrap().len() > 0);
    }
}
