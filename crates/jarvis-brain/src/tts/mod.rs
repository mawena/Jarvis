//! Synthèse vocale : la voix de Jarvis.

/// Son produit par la synthèse : i16 mono à `sample_rate` Hz.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Speech {
    pub samples: Vec<i16>,
    pub sample_rate: u32,
}

/// Transforme une phrase en voix. Bloquant (dizaines à centaines de ms) : à appeler
/// dans `spawn_blocking`.
pub trait Synthesizer: Send + Sync {
    fn synthesize(&self, text: &str) -> anyhow::Result<Speech>;
}
