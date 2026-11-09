use tracing_subscriber::EnvFilter;

/// Filtre par défaut : `info`, mais seulement les avertissements de whisper.cpp et ggml
/// (leurs messages `info` décrivent chaque chargement et chaque transcription).
const DEFAULT_FILTER: &str = "info,whisper_rs=warn";

/// Initialise les journaux : [`DEFAULT_FILTER`] par défaut, surcharge via `RUST_LOG`.
pub fn init_logging() {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
}
