//! Enrôlement du mot d'éveil sur le téléphone : prises, puis modèle `jarvis.rpw`.

use std::path::{Path, PathBuf};

use jarvis_ear_core::SAMPLE_RATE;
use jarvis_ear_core::enroll::{
    build_model, clear_samples, create_private_dir, list_samples, sample_file_name, take_level_ok,
    write_wav,
};
use jarvis_ear_core::resample::{i16_to_f32, resample};

use crate::EarError;

/// Prises minimales pour construire un modèle (Kotlin en demande 8).
pub const MIN_TAKES: usize = 3;

pub fn wakeword_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("wakeword")
}

pub fn samples_dir(data_dir: &Path) -> PathBuf {
    wakeword_dir(data_dir).join("samples")
}

pub fn model_path(data_dir: &Path) -> PathBuf {
    wakeword_dir(data_dir).join("jarvis.rpw")
}

fn io(context: &str, e: impl std::fmt::Display) -> EarError {
    EarError::Io {
        message: format!("{context} : {e}"),
    }
}

/// PCM 16 bits little-endian vers flottants ; un octet de trop est ignoré.
pub fn pcm_to_f32(pcm_le: &[u8]) -> Vec<f32> {
    pcm_le
        .chunks_exact(2)
        .map(|b| i16_to_f32(i16::from_le_bytes([b[0], b[1]])))
        .collect()
}

pub fn reset(data_dir: &Path) -> Result<(), EarError> {
    let dir = samples_dir(data_dir);
    create_private_dir(&dir).map_err(|e| io("dossier des prises", e))?;
    clear_samples(&dir).map_err(|e| io("anciennes prises", e))?;
    Ok(())
}

pub fn take(data_dir: &Path, index: u32, pcm_le: &[u8], sample_rate: u32) -> Result<(), EarError> {
    if !(1..=99).contains(&index) || sample_rate == 0 {
        return Err(EarError::Io {
            message: "prise invalide (numéro ou fréquence)".into(),
        });
    }
    let mut samples = pcm_to_f32(pcm_le);
    if sample_rate != SAMPLE_RATE {
        samples = resample(&samples, sample_rate, SAMPLE_RATE);
    }
    if !take_level_ok(&samples) {
        return Err(EarError::TooQuiet);
    }
    let dir = samples_dir(data_dir);
    create_private_dir(&dir).map_err(|e| io("dossier des prises", e))?;
    write_wav(&dir.join(sample_file_name(index as usize)), &samples)
        .map_err(|e| io("prise non enregistrée", e))
}

pub fn finish(data_dir: &Path) -> Result<u32, EarError> {
    let dir = samples_dir(data_dir);
    let count = match list_samples(&dir) {
        Ok(files) => files.len(),
        Err(e) if matches!(e.downcast_ref::<std::io::Error>(), Some(e) if e.kind() == std::io::ErrorKind::NotFound) => {
            0
        }
        Err(e) => return Err(io("prises illisibles", e)),
    };
    if count < MIN_TAKES {
        return Err(EarError::Io {
            message: "il faut au moins 3 prises".into(),
        });
    }
    let used =
        build_model(&dir, &model_path(data_dir)).map_err(|e| io("modèle non construit", e))?;
    Ok(used as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// Une prise synthétique : une note enveloppée, en PCM 16 bits little-endian.
    fn tone(freq: f32, rate: u32) -> Vec<u8> {
        (0..(rate as usize * 3 / 2))
            .flat_map(|i| {
                let t = i as f32 / rate as f32;
                let env = (t * std::f32::consts::PI / 1.5).sin();
                let v = (t * freq * std::f32::consts::TAU).sin() * env * 0.5;
                ((v * i16::MAX as f32) as i16).to_le_bytes()
            })
            .collect()
    }

    #[test]
    fn prise_trop_faible_refusee() {
        let dir = tempfile::tempdir().unwrap();
        reset(dir.path()).unwrap();
        let silence = vec![0u8; 48_000];
        assert!(matches!(
            take(dir.path(), 1, &silence, 16_000),
            Err(EarError::TooQuiet)
        ));
        assert!(matches!(
            take(dir.path(), 1, &[], 16_000),
            Err(EarError::TooQuiet)
        ));
        assert!(list_samples(&samples_dir(dir.path())).unwrap().is_empty());
    }

    #[test]
    fn huit_prises_construisent_le_modele() {
        let dir = tempfile::tempdir().unwrap();
        reset(dir.path()).unwrap();
        for i in 1..=8u32 {
            // La dernière à 48 kHz : rééchantillonnée en 16 kHz.
            let rate = if i == 8 { 48_000 } else { 16_000 };
            take(dir.path(), i, &tone(300.0 + 40.0 * i as f32, rate), rate).unwrap();
        }
        let wav = samples_dir(dir.path()).join("jarvis-08.wav");
        let spec = hound::WavReader::open(&wav).unwrap().spec();
        assert_eq!((spec.channels, spec.sample_rate), (1, 16_000));
        assert_eq!(finish(dir.path()).unwrap(), 8);
        assert!(model_path(dir.path()).exists());
        assert!(crate::has_wakeword(
            dir.path().to_string_lossy().into_owned()
        ));
    }

    #[test]
    fn moins_de_trois_prises_refusees() {
        let dir = tempfile::tempdir().unwrap();
        // Aucun dossier de prises encore.
        assert!(matches!(finish(dir.path()), Err(EarError::Io { .. })));
        reset(dir.path()).unwrap();
        take(dir.path(), 1, &tone(300.0, 16_000), 16_000).unwrap();
        take(dir.path(), 2, &tone(340.0, 16_000), 16_000).unwrap();
        let err = finish(dir.path()).unwrap_err();
        assert_eq!(err.to_string(), "il faut au moins 3 prises");
        assert!(!model_path(dir.path()).exists());
    }

    #[test]
    fn prises_et_modele_prives() {
        let dir = tempfile::tempdir().unwrap();
        reset(dir.path()).unwrap();
        for i in 1..=3u32 {
            take(
                dir.path(),
                i,
                &tone(300.0 + 50.0 * i as f32, 16_000),
                16_000,
            )
            .unwrap();
        }
        finish(dir.path()).unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&wakeword_dir(dir.path())), 0o700);
        assert_eq!(mode(&samples_dir(dir.path())), 0o700);
        assert_eq!(mode(&samples_dir(dir.path()).join("jarvis-01.wav")), 0o600);
        assert_eq!(mode(&model_path(dir.path())), 0o600);
    }

    #[test]
    fn nouvel_enregistrement_efface_les_anciennes_prises() {
        let dir = tempfile::tempdir().unwrap();
        reset(dir.path()).unwrap();
        take(dir.path(), 1, &tone(300.0, 16_000), 16_000).unwrap();
        reset(dir.path()).unwrap();
        assert!(list_samples(&samples_dir(dir.path())).unwrap().is_empty());
        assert!(matches!(
            take(dir.path(), 0, &tone(300.0, 16_000), 16_000),
            Err(EarError::Io { .. })
        ));
    }
}
