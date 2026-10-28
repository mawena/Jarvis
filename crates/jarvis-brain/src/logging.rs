use tracing_subscriber::EnvFilter;

/// Initialise les journaux : `info` par défaut, surcharge via `RUST_LOG`.
pub fn init_logging() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
}
