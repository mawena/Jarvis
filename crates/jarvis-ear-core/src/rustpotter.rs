//! Détection du mot d'éveil « Jarvis » avec `rustpotter`.

use std::path::{Path, PathBuf};

use ::rustpotter::{
    AudioFmt, DetectorConfig, Endianness, Rustpotter, RustpotterConfig, SampleFormat, WakewordRef,
    WakewordRefBuildFromFiles, WakewordSave,
};
use anyhow::{Context, anyhow};

use crate::{SAMPLE_RATE, WakeDetector};

const WAKEWORD_KEY: &str = "jarvis";
/// Nombre de coefficients MFCC utilisés pour la référence.
const MFCC_SIZE: u16 = 16;

/// Détecteur de mot d'éveil basé sur un fichier `.rpw` (référence rustpotter).
pub struct RustpotterDetector {
    inner: Rustpotter,
    frame_len: usize,
}

impl RustpotterDetector {
    /// Seuil de détection par défaut.
    pub const DEFAULT_THRESHOLD: f32 = 0.5;

    /// Charge le modèle `path` ; `threshold` est le score minimal (0 à 1).
    pub fn load(path: &Path, threshold: f32) -> anyhow::Result<Self> {
        if !path.is_file() {
            return Err(anyhow!(
                "fichier du mot d'éveil introuvable : {} (lance `jarvis-ear enroll` pour le créer)",
                path.display()
            ));
        }
        let config = RustpotterConfig {
            fmt: AudioFmt {
                sample_rate: SAMPLE_RATE as usize,
                sample_format: SampleFormat::F32,
                channels: 1,
                endianness: Endianness::Little,
            },
            detector: DetectorConfig {
                threshold,
                ..DetectorConfig::default()
            },
            ..RustpotterConfig::default()
        };
        let mut inner = Rustpotter::new(&config).map_err(|e| anyhow!("rustpotter : {e}"))?;
        let path_str = path.to_str().context("chemin du mot d'éveil non UTF-8")?;
        inner
            .add_wakeword_from_file(WAKEWORD_KEY, path_str)
            .map_err(|e| anyhow!("mot d'éveil illisible ({}) : {e}", path.display()))?;
        let frame_len = inner.get_samples_per_frame();
        Ok(Self { inner, frame_len })
    }
}

impl WakeDetector for RustpotterDetector {
    fn frame_len(&self) -> usize {
        self.frame_len
    }

    fn process(&mut self, frame: &[f32]) -> Option<f32> {
        if frame.len() != self.frame_len {
            return None;
        }
        self.inner.process_samples(frame.to_vec()).map(|d| d.score)
    }
}

/// Construit la référence `output` (.rpw) à partir d'enregistrements WAV.
pub fn build_reference(samples: &[PathBuf], output: &Path) -> anyhow::Result<()> {
    if samples.is_empty() {
        return Err(anyhow!("aucun échantillon pour construire le mot d'éveil"));
    }
    let paths = samples
        .iter()
        .map(|p| {
            p.to_str()
                .map(str::to_owned)
                .context("chemin d'échantillon non UTF-8")
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let wakeword =
        WakewordRef::new_from_sample_files(WAKEWORD_KEY.to_owned(), None, None, paths, MFCC_SIZE)
            .map_err(|e| anyhow!("construction du mot d'éveil : {e}"))?;
    if let Some(dir) = output.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let out = output.to_str().context("chemin de sortie non UTF-8")?;
    wakeword
        .save_to_file(out)
        .map_err(|e| anyhow!("écriture de {} : {e}", output.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_synthetic_wav(path: &Path, freq: f32) {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: SAMPLE_RATE,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(path, spec).unwrap();
        for i in 0..(SAMPLE_RATE as usize * 3 / 2) {
            let t = i as f32 / SAMPLE_RATE as f32;
            let env = (t * std::f32::consts::PI / 1.5).sin();
            let v = (t * freq * std::f32::consts::TAU).sin() * env * 0.5;
            w.write_sample((v * i16::MAX as f32) as i16).unwrap();
        }
        w.finalize().unwrap();
    }

    #[test]
    fn rustpotter_fichier_absent_message_clair() {
        let err = RustpotterDetector::load(Path::new("/inexistant/jarvis.rpw"), 0.5)
            .err()
            .expect("doit échouer");
        let msg = err.to_string();
        assert!(
            msg.contains("introuvable") && msg.contains("jarvis.rpw"),
            "{msg}"
        );
    }

    #[test]
    fn rustpotter_construit_et_charge_une_reference() {
        let dir = tempfile::tempdir().unwrap();
        let wavs: Vec<PathBuf> = (0..3)
            .map(|i| {
                let p = dir.path().join(format!("jarvis-{i}.wav"));
                write_synthetic_wav(&p, 300.0 + 40.0 * i as f32);
                p
            })
            .collect();
        let rpw = dir.path().join("jarvis.rpw");
        build_reference(&wavs, &rpw).unwrap();
        let mut det = RustpotterDetector::load(&rpw, 0.5).unwrap();
        assert_eq!(det.frame_len(), 480);
        assert!(det.process(&[0.0; 480]).is_none());
        assert!(det.process(&[0.0; 10]).is_none());
    }

    #[test]
    #[ignore = "demande les WAV réels de ~/.local/share/jarvis/wakeword (jarvis-ear enroll)"]
    fn rustpotter_detecte_les_echantillons() {
        let base = directories::BaseDirs::new()
            .unwrap()
            .data_dir()
            .join("jarvis/wakeword");
        let mut det = RustpotterDetector::load(&base.join("jarvis.rpw"), 0.5).unwrap();
        let mut seen = 0;
        for entry in std::fs::read_dir(base.join("samples")).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_none_or(|e| e != "wav") {
                continue;
            }
            seen += 1;
            let mut r = hound::WavReader::open(&path).unwrap();
            let mut s: Vec<f32> = r
                .samples::<i16>()
                .map(|x| x.unwrap() as f32 / i16::MAX as f32)
                .collect();
            s.extend(std::iter::repeat_n(0.0, 16_000));
            let hit = s
                .chunks_exact(det.frame_len())
                .any(|f| det.process(f).is_some());
            assert!(hit, "{} ne déclenche pas", path.display());
        }
        assert!(seen > 0, "aucun échantillon trouvé");
    }
}
