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
        Err(e) if matches!(e.downcast_ref::<std::io::Error>(), Some(e) if e.kind() == std::io::ErrorKind::NotFound) => 0,
        Err(e) => return Err(io("prises illisibles", e)),
    };
    if count < MIN_TAKES {
        return Err(EarError::Io {
            message: "il faut au moins 3 prises".into(),
        });
    }
    let used = build_model(&dir, &model_path(data_dir)).map_err(|e| io("modèle non construit", e))?;
    Ok(used as u32)
}
