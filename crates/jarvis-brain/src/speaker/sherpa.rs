//! Extraction d'empreintes vocales avec sherpa-onnx (CAM++).

use super::SpeakerVerifier;
use crate::config::SpeakerConfig;
use anyhow::Context;
use sherpa_onnx::{SpeakerEmbeddingExtractor, SpeakerEmbeddingExtractorConfig};

pub struct SherpaVerifier {
    extractor: SpeakerEmbeddingExtractor,
}

impl SherpaVerifier {
    /// Charge le modèle en mémoire (à faire une fois, au démarrage).
    pub fn load(cfg: &SpeakerConfig) -> anyhow::Result<Self> {
        let path = crate::config::expand_home(&cfg.model_path);
        if !path.is_file() {
            anyhow::bail!(
                "modèle d'empreinte vocale introuvable : {} (lancez scripts/download-models.sh speaker)",
                path.display()
            );
        }
        let config = SpeakerEmbeddingExtractorConfig {
            model: Some(path.to_string_lossy().into_owned()),
            num_threads: cfg.threads,
            ..Default::default()
        };
        let extractor = SpeakerEmbeddingExtractor::create(&config)
            .with_context(|| format!("chargement du modèle {}", path.display()))?;
        Ok(Self { extractor })
    }
}

impl SpeakerVerifier for SherpaVerifier {
    fn embed(&self, audio: &[f32]) -> anyhow::Result<Vec<f32>> {
        let stream = self
            .extractor
            .create_stream()
            .context("création du flux d'empreinte")?;
        stream.accept_waveform(16_000, audio);
        stream.input_finished();
        if !self.extractor.is_ready(&stream) {
            anyhow::bail!("audio trop court pour une empreinte");
        }
        self.extractor
            .compute(&stream)
            .context("calcul de l'empreinte")
    }
}
