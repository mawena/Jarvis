//! Résumé des résultats par une IA. Le cerveau fournit l'implémentation
//! (`ModeSummarizer`), qui suit le mode actif.

use std::time::Duration;

/// Un extrait donné à l'IA pour le résumé.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub site: String,
    pub title: String,
    pub snippet: String,
}

#[async_trait::async_trait]
pub trait Summarizer: Send + Sync {
    /// Résumé parlé de `sources` pour répondre à `query`.
    async fn summarize(&self, query: &str, sources: &[Source]) -> anyhow::Result<String>;

    /// Temps laissé au résumé avant le repli sur l'extrait.
    fn budget(&self) -> Duration;
}

/// Aucun résumé : toujours en échec, budget nul.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoSummarizer;

#[async_trait::async_trait]
impl Summarizer for NoSummarizer {
    async fn summarize(&self, _query: &str, _sources: &[Source]) -> anyhow::Result<String> {
        anyhow::bail!("pas de résumé")
    }

    fn budget(&self) -> Duration {
        Duration::ZERO
    }
}

#[cfg(any(test, feature = "test-util"))]
pub use fake::FakeSummarizer;

#[cfg(any(test, feature = "test-util"))]
mod fake {
    use super::{Source, Summarizer};
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    /// Résumé programmé (texte ou échec), avec délai et budget réglables.
    pub struct FakeSummarizer {
        result: Result<String, String>,
        delay: Duration,
        budget: Duration,
        calls: AtomicUsize,
        last: Mutex<Option<(String, Vec<Source>)>>,
    }

    impl FakeSummarizer {
        /// `Ok(texte)` ou `Err(message)`. Sans délai, budget de 5 s.
        pub fn new(result: Result<String, String>) -> Self {
            Self {
                result,
                delay: Duration::ZERO,
                budget: Duration::from_secs(5),
                calls: AtomicUsize::new(0),
                last: Mutex::new(None),
            }
        }

        /// Répond toujours `text`.
        pub fn ok(text: impl Into<String>) -> Self {
            Self::new(Ok(text.into()))
        }

        /// Échoue toujours.
        pub fn failing() -> Self {
            Self::new(Err("résumé factice en échec".into()))
        }

        /// Attente (temps de tokio) avant chaque réponse.
        pub fn with_delay(mut self, delay: Duration) -> Self {
            self.delay = delay;
            self
        }

        pub fn with_budget(mut self, budget: Duration) -> Self {
            self.budget = budget;
            self
        }

        pub fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }

        /// Sources reçues lors du dernier appel.
        pub fn last_sources(&self) -> Option<Vec<Source>> {
            self.last.lock().unwrap().as_ref().map(|(_, s)| s.clone())
        }

        /// Question reçue lors du dernier appel.
        pub fn last_query(&self) -> Option<String> {
            self.last.lock().unwrap().as_ref().map(|(q, _)| q.clone())
        }
    }

    #[async_trait::async_trait]
    impl Summarizer for FakeSummarizer {
        async fn summarize(&self, query: &str, sources: &[Source]) -> anyhow::Result<String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            *self.last.lock().unwrap() = Some((query.to_string(), sources.to_vec()));
            if !self.delay.is_zero() {
                tokio::time::sleep(self.delay).await;
            }
            self.result.clone().map_err(anyhow::Error::msg)
        }

        fn budget(&self) -> Duration {
            self.budget
        }
    }
}
