//! Résumé des recherches par l'IA du mode actif.
//!
//! Le mode est lu à chaque appel. En mode local, seul Ollama est appelé : s'il
//! échoue, c'est une erreur (la compétence dit l'extrait), jamais Claude.

use crate::llm::{LlmBackend, summary_message, summary_prompt};
use crate::mode_store::ModeStore;
use jarvis_protocol::Mode;
use jarvis_skills::search::{SearchSettings, Source, Summarizer};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

/// Action lancée après un résumé local (re-préchauffage d'Ollama).
pub type AfterLocal = Arc<dyn Fn() + Send + Sync>;

/// Résumeur qui suit le mode actif.
pub struct ModeSummarizer {
    local: Arc<dyn LlmBackend>,
    online: Arc<dyn LlmBackend>,
    modes: Arc<ModeStore>,
    address: String,
    max_tokens: u32,
    local_budget: Duration,
    online_budget: Duration,
    after_local: Option<AfterLocal>,
}

impl ModeSummarizer {
    pub fn new(
        local: Arc<dyn LlmBackend>,
        online: Arc<dyn LlmBackend>,
        modes: Arc<ModeStore>,
        address: &str,
        settings: &SearchSettings,
    ) -> Self {
        Self {
            local,
            online,
            modes,
            address: address.to_string(),
            max_tokens: settings.summary_max_tokens,
            local_budget: Duration::from_millis(settings.local_summary_timeout_ms),
            online_budget: Duration::from_millis(settings.online_summary_timeout_ms),
            after_local: None,
        }
    }

    /// `after` est appelé après chaque résumé local, même en échec ou abandonné :
    /// la consigne du résumé a pris la place des outils dans le cache d'Ollama.
    pub fn with_after_local(mut self, after: AfterLocal) -> Self {
        self.after_local = Some(after);
        self
    }
}

/// Appelle l'action à la fin du résumé, y compris s'il est abandonné (délai dépassé).
struct AfterGuard(Option<AfterLocal>);

impl Drop for AfterGuard {
    fn drop(&mut self) {
        if let Some(after) = self.0.take() {
            after();
        }
    }
}

#[async_trait::async_trait]
impl Summarizer for ModeSummarizer {
    async fn summarize(&self, query: &str, sources: &[Source]) -> anyhow::Result<String> {
        let mode = self.modes.get();
        let (backend, path, _guard) = match mode {
            Mode::Local => (
                &self.local,
                "ia-locale",
                AfterGuard(self.after_local.clone()),
            ),
            Mode::Online => (&self.online, "ia-en-ligne", AfterGuard(None)),
        };
        let start = Instant::now();
        let system = summary_prompt(&self.address);
        let user = summary_message(query, sources);
        let result = backend.complete(&system, &user, self.max_tokens).await;
        let duree_ms = start.elapsed().as_millis() as u64;
        match result {
            Ok(text) if !text.trim().is_empty() => {
                info!(chemin = path, duree_ms, "résumé de la recherche");
                debug!(resume = %text.trim(), "résumé");
                Ok(text.trim().to_string())
            }
            Ok(_) => {
                warn!(chemin = path, duree_ms, "résumé vide");
                anyhow::bail!("résumé vide")
            }
            Err(e) => {
                warn!(chemin = path, duree_ms, erreur = %e, "résumé impossible");
                Err(e.into())
            }
        }
    }

    fn budget(&self) -> Duration {
        match self.modes.get() {
            Mode::Local => self.local_budget,
            Mode::Online => self.online_budget,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::LlmError;
    use crate::mode_store::ModeStore;
    use crate::testing::FakeLlm;
    use jarvis_protocol::Mode;
    use jarvis_skills::search::{SearchSettings, Source, Summarizer};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    fn llm(completion: Result<String, LlmError>) -> Arc<FakeLlm> {
        Arc::new(
            FakeLlm::new(Ok(crate::llm::Understanding::Answer("non".into())))
                .with_completion(completion),
        )
    }

    fn modes(mode: Mode) -> (tempfile::TempDir, Arc<ModeStore>) {
        let dir = tempfile::tempdir().unwrap();
        let store = ModeStore::new(dir.path().join("mode"), Mode::Local);
        store.set(mode).unwrap();
        (dir, Arc::new(store))
    }

    fn sources() -> Vec<Source> {
        vec![Source {
            site: "meteofrance".into(),
            title: "Météo de Lomé".into(),
            snippet: "Soleil et 31 degrés.".into(),
        }]
    }

    #[tokio::test]
    async fn resume_mode_local_n_appelle_jamais_le_modele_en_ligne() {
        let local = llm(Err(LlmError::Unreachable("Ollama arrêté".into())));
        let online = llm(Ok("Résumé en ligne.".into()));
        let (_d, store) = modes(Mode::Local);
        let s = ModeSummarizer::new(
            local.clone(),
            online.clone(),
            store,
            "Monsieur",
            &SearchSettings::default(),
        );
        assert!(s.summarize("météo", &sources()).await.is_err());
        assert_eq!(local.completions(), 1);
        assert_eq!(online.completions(), 0);
        assert_eq!(online.calls(), 0);
    }

    #[tokio::test]
    async fn resume_mode_local_utilise_ollama() {
        let local = llm(Ok(" Il fera beau, Monsieur. ".into()));
        let online = llm(Ok("Résumé en ligne.".into()));
        let (_d, store) = modes(Mode::Local);
        let s = ModeSummarizer::new(
            local.clone(),
            online.clone(),
            store,
            "Monsieur",
            &SearchSettings::default(),
        );
        let got = s.summarize("météo à Lomé", &sources()).await.unwrap();
        assert_eq!(got, "Il fera beau, Monsieur.");
        let (system, user) = local.last_prompt().unwrap();
        assert_eq!(system, crate::llm::summary_prompt("Monsieur"));
        assert_eq!(
            user,
            crate::llm::summary_message("météo à Lomé", &sources())
        );
        assert_eq!(online.completions(), 0);
    }

    #[tokio::test]
    async fn resume_mode_en_ligne_utilise_claude() {
        let local = llm(Ok("Résumé local.".into()));
        let online = llm(Ok("Résumé en ligne.".into()));
        let (_d, store) = modes(Mode::Online);
        let s = ModeSummarizer::new(
            local.clone(),
            online.clone(),
            store,
            "Monsieur",
            &SearchSettings::default(),
        );
        assert_eq!(
            s.summarize("météo", &sources()).await.unwrap(),
            "Résumé en ligne."
        );
        assert_eq!(online.completions(), 1);
        assert_eq!(local.completions(), 0);
    }

    #[tokio::test]
    async fn resume_mode_en_ligne_en_panne_sans_repli_local() {
        let local = llm(Ok("Résumé local.".into()));
        let online = llm(Err(LlmError::MissingApiKey("ANTHROPIC_API_KEY".into())));
        let (_d, store) = modes(Mode::Online);
        let s = ModeSummarizer::new(
            local.clone(),
            online.clone(),
            store,
            "Monsieur",
            &SearchSettings::default(),
        );
        assert!(s.summarize("météo", &sources()).await.is_err());
        assert_eq!(local.completions(), 0);
    }

    #[tokio::test]
    async fn resume_vide_erreur() {
        let local = llm(Ok("   ".into()));
        let online = llm(Ok("Résumé en ligne.".into()));
        let (_d, store) = modes(Mode::Local);
        let s = ModeSummarizer::new(
            local,
            online.clone(),
            store,
            "Monsieur",
            &SearchSettings::default(),
        );
        assert!(s.summarize("météo", &sources()).await.is_err());
        assert_eq!(online.completions(), 0);
    }

    #[tokio::test]
    async fn resume_suit_le_changement_de_mode() {
        let local = llm(Ok("Résumé local.".into()));
        let online = llm(Ok("Résumé en ligne.".into()));
        let (_d, store) = modes(Mode::Local);
        let s = ModeSummarizer::new(
            local.clone(),
            online.clone(),
            store.clone(),
            "Monsieur",
            &SearchSettings::default(),
        );
        assert_eq!(s.summarize("q", &sources()).await.unwrap(), "Résumé local.");
        store.set(Mode::Online).unwrap();
        assert_eq!(
            s.summarize("q", &sources()).await.unwrap(),
            "Résumé en ligne."
        );
        store.set(Mode::Local).unwrap();
        assert_eq!(s.summarize("q", &sources()).await.unwrap(), "Résumé local.");
        assert_eq!(local.completions(), 2);
        assert_eq!(online.completions(), 1);
    }

    #[tokio::test]
    async fn budget_selon_le_mode() {
        let settings = SearchSettings {
            local_summary_timeout_ms: 9000,
            online_summary_timeout_ms: 4000,
            ..SearchSettings::default()
        };
        let (_d, store) = modes(Mode::Local);
        let s = ModeSummarizer::new(
            llm(Ok("a".into())),
            llm(Ok("b".into())),
            store.clone(),
            "Monsieur",
            &settings,
        );
        assert_eq!(s.budget(), Duration::from_millis(9000));
        store.set(Mode::Online).unwrap();
        assert_eq!(s.budget(), Duration::from_millis(4000));
    }

    #[tokio::test]
    async fn rechauffe_apres_resume_local_seulement() {
        let count = Arc::new(AtomicUsize::new(0));
        let c = count.clone();
        let (_d, store) = modes(Mode::Online);
        let local = llm(Ok("Résumé local.".into()));
        let s = ModeSummarizer::new(
            local.clone(),
            llm(Ok("Résumé en ligne.".into())),
            store.clone(),
            "Monsieur",
            &SearchSettings::default(),
        )
        .with_after_local(Arc::new(move || {
            // Appelé après la complétion locale, jamais avant.
            assert_eq!(local.completions(), 1);
            c.fetch_add(1, Ordering::SeqCst);
        }));
        s.summarize("q", &sources()).await.unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 0);
        store.set(Mode::Local).unwrap();
        s.summarize("q", &sources()).await.unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn rechauffe_meme_si_le_resume_local_echoue() {
        let count = Arc::new(AtomicUsize::new(0));
        let c = count.clone();
        let (_d, store) = modes(Mode::Local);
        let s = ModeSummarizer::new(
            llm(Err(LlmError::BadResponse("vide".into()))),
            llm(Ok("Résumé en ligne.".into())),
            store,
            "Monsieur",
            &SearchSettings::default(),
        )
        .with_after_local(Arc::new(move || {
            c.fetch_add(1, Ordering::SeqCst);
        }));
        assert!(s.summarize("q", &sources()).await.is_err());
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    /// Backend local qui ne répond jamais.
    struct Muet;

    #[async_trait::async_trait]
    impl crate::llm::LlmBackend for Muet {
        async fn understand(
            &self,
            _: &str,
            _: &[jarvis_skills::ToolSpec],
        ) -> Result<crate::llm::Understanding, LlmError> {
            std::future::pending().await
        }

        async fn complete(&self, _: &str, _: &str, _: u32) -> Result<String, LlmError> {
            std::future::pending().await
        }
    }

    #[tokio::test(start_paused = true)]
    async fn rechauffe_meme_si_le_resume_local_est_abandonne() {
        let count = Arc::new(AtomicUsize::new(0));
        let c = count.clone();
        let (_d, store) = modes(Mode::Local);
        let s = ModeSummarizer::new(
            Arc::new(Muet),
            llm(Ok("Résumé en ligne.".into())),
            store,
            "Monsieur",
            &SearchSettings::default(),
        )
        .with_after_local(Arc::new(move || {
            c.fetch_add(1, Ordering::SeqCst);
        }));
        let r = tokio::time::timeout(s.budget(), s.summarize("q", &sources())).await;
        assert!(r.is_err(), "le résumé aurait dû dépasser le délai");
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }
}
