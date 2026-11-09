//! Voix Piper (VITS) avec sherpa-onnx ; le texte passe d'abord en phonèmes
//! avec `piper-phonemize` (espeak-ng embarqué).

use super::{Speech, Synthesizer};
use crate::config::{TtsConfig, expand_home};
use anyhow::Context;
use sherpa_onnx::{
    GenerationConfig, OfflineTts, OfflineTtsConfig, OfflineTtsModelConfig,
    OfflineTtsVitsModelConfig,
};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Langue d'espeak-ng pour les phonèmes (celle de la voix).
const LANGUAGE: &str = "fr";

pub struct PiperSynthesizer {
    tts: OfflineTts,
    speaker: i32,
    speed: f32,
    /// espeak-ng n'est pas sûr entre threads : une phrase à la fois.
    lock: Mutex<()>,
}

impl PiperSynthesizer {
    /// Charge la voix en mémoire (à faire une fois, au démarrage).
    pub fn load(cfg: &TtsConfig) -> anyhow::Result<Self> {
        let dir = expand_home(&cfg.model_dir);
        let (model, tokens) = model_files(&dir)?;
        let config = OfflineTtsConfig {
            model: OfflineTtsModelConfig {
                vits: OfflineTtsVitsModelConfig {
                    model: Some(model.to_string_lossy().into_owned()),
                    tokens: Some(tokens.to_string_lossy().into_owned()),
                    ..Default::default()
                },
                num_threads: cfg.threads.max(1),
                ..Default::default()
            },
            ..Default::default()
        };
        let tts = OfflineTts::create(&config)
            .with_context(|| format!("chargement de la voix {}", model.display()))?;
        let speakers = tts.num_speakers();
        if cfg.speaker < 0 || (speakers > 0 && cfg.speaker >= speakers) {
            anyhow::bail!(
                "locuteur {} inexistant dans la voix ({} locuteurs)",
                cfg.speaker,
                speakers
            );
        }
        let voice = Self {
            tts,
            speaker: cfg.speaker,
            speed: if cfg.speed > 0.0 { cfg.speed } else { 1.0 },
            lock: Mutex::new(()),
        };
        // Prépare espeak-ng tout de suite : une panne se voit au démarrage.
        phonemes("Bonjour.")?;
        Ok(voice)
    }
}

/// Le modèle `.onnx` (un seul attendu) et `tokens.txt` du dossier de la voix.
fn model_files(dir: &Path) -> anyhow::Result<(PathBuf, PathBuf)> {
    let hint = "lancez scripts/download-models.sh voice";
    let tokens = dir.join("tokens.txt");
    let mut models: Vec<PathBuf> = std::fs::read_dir(dir)
        .with_context(|| format!("voix introuvable : {} ({hint})", dir.display()))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|e| e == "onnx"))
        .collect();
    models.sort();
    let model = match models.as_slice() {
        [m] => m.clone(),
        [] => anyhow::bail!("aucun modèle .onnx dans {} ({hint})", dir.display()),
        _ => anyhow::bail!("plusieurs modèles .onnx dans {}", dir.display()),
    };
    if !tokens.is_file() {
        anyhow::bail!("tokens.txt absent de {} ({hint})", dir.display());
    }
    Ok((model, tokens))
}

/// Texte → phonèmes (points de code), une liste par phrase.
fn phonemes(text: &str) -> anyhow::Result<Vec<Vec<i32>>> {
    let ph = piper_phonemize::phonemize(text, LANGUAGE)
        .context("conversion du texte en phonèmes impossible")?;
    Ok((0..ph.num_sentences())
        .map(|k| ph.get_phonemes(k).unwrap_or_default())
        .filter(|s| !s.is_empty())
        .collect())
}

/// f32 dans [-1, 1] → i16, en bornant ce qui dépasse.
fn to_i16(samples: &[f32]) -> Vec<i16> {
    samples
        .iter()
        .map(|&s| (s.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16)
        .collect()
}

impl Synthesizer for PiperSynthesizer {
    fn synthesize(&self, text: &str) -> anyhow::Result<Speech> {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let codepoints = phonemes(text)?;
        if codepoints.is_empty() {
            anyhow::bail!("rien à dire");
        }
        let config = GenerationConfig {
            sid: self.speaker,
            speed: self.speed,
            phoneme_codepoints: Some(codepoints),
            ..Default::default()
        };
        let audio = self
            .tts
            .generate_with_config("", &config, None::<fn(&[f32], f32) -> bool>)
            .context("synthèse vocale impossible")?;
        let sample_rate =
            u32::try_from(audio.sample_rate()).context("fréquence de la voix invalide")?;
        Ok(Speech {
            samples: to_i16(audio.samples()),
            sample_rate,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversion_en_i16_bornee() {
        assert_eq!(
            to_i16(&[0.0, 0.5, -0.5, 1.0, -1.0, 2.0, -3.0]),
            [0, 16383, -16383, 32767, -32767, 32767, -32767]
        );
    }

    #[test]
    fn voix_absente_indique_le_telechargement() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = TtsConfig {
            model_dir: dir.path().join("absente"),
            ..TtsConfig::default()
        };
        let err = PiperSynthesizer::load(&cfg).err().unwrap().to_string();
        assert!(err.contains("download-models.sh voice"), "{err}");
        // Dossier présent mais incomplet.
        std::fs::write(dir.path().join("voix.onnx"), "").unwrap();
        let cfg = TtsConfig {
            model_dir: dir.path().to_path_buf(),
            ..TtsConfig::default()
        };
        let err = PiperSynthesizer::load(&cfg).err().unwrap().to_string();
        assert!(err.contains("tokens.txt"), "{err}");
    }

    /// Vraie voix, si elle est installée : `cargo test -p jarvis-brain -- --ignored vraie_voix`.
    #[test]
    #[ignore = "voix réelle (~/.local/share/jarvis/models/vits-piper-fr_FR-upmc-medium)"]
    fn vraie_voix_dit_une_phrase() {
        let t = std::time::Instant::now();
        let voice = PiperSynthesizer::load(&TtsConfig::default()).unwrap();
        println!("chargement : {:.2} s", t.elapsed().as_secs_f32());
        for text in [
            "Bien, Monsieur.",
            "Il est quatorze heures trente, et la musique reprendra dès que vous aurez terminé votre appel.",
        ] {
            let mut best = f32::MAX;
            let mut speech = None;
            for _ in 0..3 {
                let t = std::time::Instant::now();
                speech = Some(voice.synthesize(text).unwrap());
                best = best.min(t.elapsed().as_secs_f32());
            }
            let speech = speech.unwrap();
            assert_eq!(speech.sample_rate, 22_050);
            let seconds = speech.samples.len() as f32 / speech.sample_rate as f32;
            assert!(seconds > 0.5, "{seconds} s");
            assert!(speech.samples.iter().any(|&s| s.unsigned_abs() > 1000));
            println!("« {text} » : {best:.2} s de calcul pour {seconds:.2} s de voix");
        }
    }
}
