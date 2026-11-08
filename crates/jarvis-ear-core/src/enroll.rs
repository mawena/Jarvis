//! Enrôlement du mot d'éveil, partagé par les oreilles (PC, Android) : prises
//! `jarvis-NN.wav` (les seuls sons conservés sur disque), construction de
//! `jarvis.rpw`, dossiers et fichiers privés. Aucun micro ici : l'appelant enregistre.

use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use crate::SAMPLE_RATE;
use crate::rustpotter::build_reference;

/// Durée de chaque prise (secondes).
pub const TAKE_SECONDS: f32 = 1.5;

/// Fenêtre de mesure du niveau d'une prise (100 ms à 16 kHz).
const LEVEL_WINDOW: usize = SAMPLE_RATE as usize / 10;

/// Pas entre deux fenêtres de mesure (50 ms : fenêtres à moitié superposées).
const LEVEL_HOP: usize = LEVEL_WINDOW / 2;

/// Niveau RMS minimal d'une prise audible : −40 dBFS (10^(−40/20)).
const MIN_TAKE_RMS: f32 = 0.01;

/// Nom de fichier de la prise `index` (à partir de 1).
pub fn sample_file_name(index: usize) -> String {
    format!("jarvis-{index:02}.wav")
}

/// Nombre d'échantillons mono 16 kHz d'une prise.
pub fn samples_per_take() -> usize {
    (SAMPLE_RATE as f32 * TAKE_SECONDS) as usize
}

/// Vrai pour un nom de la forme `jarvis-NN.wav` (deux chiffres).
fn is_take_file(name: &str) -> bool {
    name.strip_prefix("jarvis-")
        .and_then(|n| n.strip_suffix(".wav"))
        .is_some_and(|n| n.len() == 2 && n.bytes().all(|b| b.is_ascii_digit()))
}

/// Supprime les anciennes prises `jarvis-NN.wav` (un `--count` plus petit ne doit pas
/// laisser d'anciens échantillons), et elles seules. Renvoie le nombre de fichiers supprimés.
pub fn clear_samples(dir: &Path) -> anyhow::Result<usize> {
    let mut removed = 0;
    for path in list_samples(dir)? {
        let old_take = path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(is_take_file);
        if old_take {
            std::fs::remove_file(&path)?;
            removed += 1;
        }
    }
    Ok(removed)
}

/// Crée le dossier (et ses parents) en 0700 ; un dossier existant passe aussi en 0700.
pub fn create_private_dir(path: &Path) -> std::io::Result<()> {
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
}

/// Écrit une prise en WAV 16 bits mono 16 kHz, lisible par moi seul (0600).
pub fn write_wav(path: &Path, samples: &[f32]) -> anyhow::Result<()> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    // `mode` ne vaut qu'à la création : un ancien fichier garderait ses droits.
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    let mut writer = hound::WavWriter::new(std::io::BufWriter::new(file), spec)?;
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

/// Construit `jarvis.rpw` (0600) à partir des WAV du dossier d'échantillons.
pub fn build_model(samples_dir: &Path, output: &Path) -> anyhow::Result<usize> {
    let files = list_samples(samples_dir)?;
    build_reference(&files, output)?;
    std::fs::set_permissions(output, std::fs::Permissions::from_mode(0o600))?;
    Ok(files.len())
}

/// Vrai si la prise (mono 16 kHz) est audible : au moins une fenêtre de 100 ms dépasse
/// −40 dBFS RMS. Une prise plus courte qu'une fenêtre est mesurée d'un bloc ; une prise
/// vide est refusée.
pub fn take_level_ok(samples: &[f32]) -> bool {
    let loud = |w: &[f32]| {
        let energy: f64 = w.iter().map(|s| f64::from(*s) * f64::from(*s)).sum();
        (energy / w.len() as f64).sqrt() > f64::from(MIN_TAKE_RMS)
    };
    if samples.is_empty() {
        return false;
    }
    if samples.len() <= LEVEL_WINDOW {
        return loud(samples);
    }
    let last = samples.len() - LEVEL_WINDOW;
    // Fenêtres tous les 50 ms, plus la dernière collée à la fin de la prise.
    (0..=last)
        .step_by(LEVEL_HOP)
        .chain(std::iter::once(last))
        .any(|start| loud(&samples[start..start + LEVEL_WINDOW]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    /// Sinusoïde à 440 Hz de niveau RMS `dbfs`, sur `seconds` secondes.
    fn sine(dbfs: f32, seconds: f32) -> Vec<f32> {
        let peak = 10f32.powf(dbfs / 20.0) * std::f32::consts::SQRT_2;
        (0..(SAMPLE_RATE as f32 * seconds) as usize)
            .map(|n| {
                let t = n as f32 / SAMPLE_RATE as f32;
                (t * 440.0 * std::f32::consts::TAU).sin() * peak
            })
            .collect()
    }

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
    fn anciennes_prises_supprimees() {
        let dir = tempfile::tempdir().unwrap();
        for i in 1..=5 {
            write_wav(&dir.path().join(sample_file_name(i)), &[0.0; 16]).unwrap();
        }
        write_wav(&dir.path().join("autre.wav"), &[0.0; 16]).unwrap();
        assert_eq!(clear_samples(dir.path()).unwrap(), 5);
        let left = list_samples(dir.path()).unwrap();
        assert_eq!(left.len(), 1);
        assert!(left[0].ends_with("autre.wav"));
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
        assert_eq!(mode(&rpw), 0o600);
    }

    #[test]
    fn seules_les_prises_a_deux_chiffres_sont_supprimees() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["jarvis-01.wav", "jarvis-12.wav"] {
            write_wav(&dir.path().join(name), &[0.0; 16]).unwrap();
        }
        for name in [
            "jarvis-extra.wav",
            "jarvis-1.wav",
            "jarvis-001.wav",
            "jarvis-ab.wav",
        ] {
            write_wav(&dir.path().join(name), &[0.0; 16]).unwrap();
        }
        assert_eq!(clear_samples(dir.path()).unwrap(), 2);
        assert_eq!(list_samples(dir.path()).unwrap().len(), 4);
    }

    #[test]
    fn prise_reecrite_rendue_privee() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(sample_file_name(1));
        std::fs::write(&path, "ancien").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        write_wav(&path, &[0.0; 16]).unwrap();
        assert_eq!(mode(&path), 0o600);
        assert_eq!(hound::WavReader::open(&path).unwrap().len(), 16);
    }

    #[test]
    fn dossier_prive_cree_ou_rendu_prive() {
        let base = tempfile::tempdir().unwrap();
        let dir = base.path().join("wakeword").join("samples");
        create_private_dir(&dir).unwrap();
        assert_eq!(mode(&dir), 0o700);
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        create_private_dir(&dir).unwrap();
        assert_eq!(mode(&dir), 0o700);
    }

    #[test]
    fn prise_silencieuse_refusee() {
        assert!(!take_level_ok(&vec![0.0; samples_per_take()]));
        assert!(!take_level_ok(&[]));
    }

    #[test]
    fn prise_parlee_acceptee() {
        // 0,3 s de voix à −20 dBFS au milieu d'une prise silencieuse.
        let mut take = vec![0.0; samples_per_take()];
        let voice = sine(-20.0, 0.3);
        take[10_000..10_000 + voice.len()].copy_from_slice(&voice);
        assert!(take_level_ok(&take));
        // Collée à la fin de la prise : encore vue.
        let mut take = vec![0.0; samples_per_take()];
        let end = take.len();
        take[end - voice.len()..].copy_from_slice(&voice);
        assert!(take_level_ok(&take));
    }

    #[test]
    fn prise_tres_faible_refusee() {
        assert!(!take_level_ok(&sine(-50.0, TAKE_SECONDS)));
    }
}
