//! Reconnaissance vocale.

/// Transforme de l'audio (f32, 16 kHz mono) en texte.
#[async_trait::async_trait]
pub trait Transcriber: Send + Sync {
    async fn transcribe(&self, audio: &[f32]) -> anyhow::Result<String>;
}
pub mod whisper;
