//! Doublures de test partagées (`FakeLlm`, `FakeSkill`).

use crate::ducking::Ducker;
use crate::intent::Intent;
use crate::llm::{LlmBackend, LlmError, Understanding};
use crate::stt::Transcriber;
use crate::tts::{Speech, Synthesizer};
use jarvis_skills::text::fold;
use jarvis_skills::{Reply, Risk, Shortcut, Skill, SkillError, ToolSpec};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Backend LLM à réponse programmée, qui compte ses appels.
pub struct FakeLlm {
    response: Mutex<Result<Understanding, LlmError>>,
    calls: AtomicUsize,
    tools: Mutex<Vec<String>>,
    completion: Mutex<Result<String, LlmError>>,
    completions: AtomicUsize,
    prompt: Mutex<Option<(String, String)>>,
}

impl FakeLlm {
    pub fn new(response: Result<Understanding, LlmError>) -> Self {
        Self {
            response: Mutex::new(response),
            calls: AtomicUsize::new(0),
            tools: Mutex::new(Vec::new()),
            completion: Mutex::new(Err(LlmError::BadResponse(
                "complétion non programmée".into(),
            ))),
            completions: AtomicUsize::new(0),
            prompt: Mutex::new(None),
        }
    }

    /// Réponse programmée de `complete` (résumé).
    pub fn with_completion(self, completion: Result<String, LlmError>) -> Self {
        *self.completion.lock().unwrap() = completion;
        self
    }

    /// Nombre d'appels à `complete`.
    pub fn completions(&self) -> usize {
        self.completions.load(Ordering::SeqCst)
    }

    /// Consigne et message du dernier appel à `complete`.
    pub fn last_prompt(&self) -> Option<(String, String)> {
        self.prompt.lock().unwrap().clone()
    }

    /// Noms des outils reçus lors du dernier appel.
    pub fn last_tools(&self) -> Vec<String> {
        self.tools.lock().unwrap().clone()
    }

    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl LlmBackend for FakeLlm {
    async fn understand(&self, _text: &str, tools: &[ToolSpec]) -> Result<Understanding, LlmError> {
        *self.tools.lock().unwrap() = tools.iter().map(|t| t.name.clone()).collect();
        self.calls.fetch_add(1, Ordering::SeqCst);
        match &*self.response.lock().unwrap() {
            Ok(u) => Ok(u.clone()),
            Err(e) => Err(copy_err(e)),
        }
    }

    async fn complete(
        &self,
        system: &str,
        user: &str,
        _max_tokens: u32,
    ) -> Result<String, LlmError> {
        self.completions.fetch_add(1, Ordering::SeqCst);
        *self.prompt.lock().unwrap() = Some((system.to_string(), user.to_string()));
        match &*self.completion.lock().unwrap() {
            Ok(t) => Ok(t.clone()),
            Err(e) => Err(copy_err(e)),
        }
    }
}

/// `LlmError` n'est pas `Clone` : copie à la main.
fn copy_err(e: &LlmError) -> LlmError {
    match e {
        LlmError::Unreachable(m) => LlmError::Unreachable(m.clone()),
        LlmError::BadResponse(m) => LlmError::BadResponse(m.clone()),
        LlmError::MissingApiKey(m) => LlmError::MissingApiKey(m.clone()),
        LlmError::Rejected(m) => LlmError::Rejected(m.clone()),
    }
}

/// Compétence factice : déclare des outils, enregistre les appels.
pub struct FakeSkill {
    tool_names: Vec<String>,
    calls: Mutex<Vec<(String, serde_json::Value)>>,
    result: Mutex<Result<Reply, SkillError>>,
    delay: std::time::Duration,
    timeout: Option<std::time::Duration>,
    /// Phrase (forme `fold`) et outil de son raccourci propre.
    shortcut: Option<(String, Shortcut)>,
    llm_tools: bool,
    /// Appels allés jusqu'au bout (après `delay`).
    finished: std::sync::atomic::AtomicUsize,
}

impl FakeSkill {
    pub fn new(tool_names: &[&str]) -> Self {
        Self {
            tool_names: tool_names.iter().map(|s| s.to_string()).collect(),
            calls: Mutex::new(Vec::new()),
            result: Mutex::new(Ok(Reply::new("ok"))),
            delay: std::time::Duration::ZERO,
            timeout: None,
            shortcut: None,
            llm_tools: true,
            finished: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// Appels allés jusqu'au bout : un appel abandonné en route (future lâchée) n'y
    /// est pas.
    pub fn finished(&self) -> usize {
        self.finished.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Raccourci propre (`Skill::shortcut`) : `phrase` (comparée après `fold`)
    /// lance `tool` avec `args`.
    pub fn with_shortcut(mut self, phrase: &str, tool: &str, args: serde_json::Value) -> Self {
        self.shortcut = Some((
            fold(phrase),
            Shortcut {
                tool: tool.to_string(),
                args,
            },
        ));
        self
    }

    /// Faux : aucun outil proposé à l'IA (`Skill::llm_tools` vide).
    pub fn with_llm_tools(mut self, offered: bool) -> Self {
        self.llm_tools = offered;
        self
    }

    /// Délai propre à tous ses outils (`Skill::timeout`).
    pub fn with_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    pub fn with_result(self, result: Result<Reply, SkillError>) -> Self {
        *self.result.lock().unwrap() = result;
        self
    }

    /// Délai artificiel avant chaque réponse (tests de délai).
    pub fn with_delay(mut self, delay: std::time::Duration) -> Self {
        self.delay = delay;
        self
    }

    pub fn calls(&self) -> Vec<(String, serde_json::Value)> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl Skill for FakeSkill {
    fn tools(&self) -> Vec<ToolSpec> {
        self.tool_names
            .iter()
            .map(|name| ToolSpec {
                name: name.clone(),
                description: format!("outil factice {name}"),
                parameters: serde_json::json!({"type": "object", "properties": {}}),
                risk: Risk::Direct,
            })
            .collect()
    }

    fn llm_tools(&self) -> Vec<ToolSpec> {
        if self.llm_tools {
            self.tools()
        } else {
            Vec::new()
        }
    }

    fn shortcut(&self, text: &str) -> Option<Shortcut> {
        let (phrase, shortcut) = self.shortcut.as_ref()?;
        (fold(text) == *phrase).then(|| shortcut.clone())
    }

    fn timeout(&self, _tool: &str) -> Option<std::time::Duration> {
        self.timeout
    }

    async fn call(&self, tool: &str, args: serde_json::Value) -> Result<Reply, SkillError> {
        self.calls.lock().unwrap().push((tool.to_string(), args));
        if !self.delay.is_zero() {
            tokio::time::sleep(self.delay).await;
        }
        self.finished
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        match &*self.result.lock().unwrap() {
            Ok(r) => Ok(r.clone()),
            Err(SkillError::UnknownTool(m)) => Err(SkillError::UnknownTool(m.clone())),
            Err(SkillError::BadArgs(m)) => Err(SkillError::BadArgs(m.clone())),
            Err(SkillError::NotFound(m)) => Err(SkillError::NotFound(m.clone())),
            Err(SkillError::Failed(m)) => Err(SkillError::Failed(m.clone())),
        }
    }
}

/// Raccourci pour les tests : intention `Tool` sans arguments.
pub fn tool_understanding(tool: &str) -> Understanding {
    Understanding::Tool(Intent::new(tool, serde_json::json!({})))
}

/// Transcripteur factice : texte programmé (ou échec), compte ses appels.
pub struct FakeTranscriber {
    text: Option<String>,
    calls: AtomicUsize,
}

impl FakeTranscriber {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: Some(text.into()),
            calls: AtomicUsize::new(0),
        }
    }

    /// Transcripteur qui échoue à chaque appel.
    pub fn failing() -> Self {
        Self {
            text: None,
            calls: AtomicUsize::new(0),
        }
    }

    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl Transcriber for FakeTranscriber {
    async fn transcribe(&self, _audio: &[f32]) -> anyhow::Result<String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.text
            .clone()
            .ok_or_else(|| anyhow::anyhow!("transcription factice en échec"))
    }
}

/// Baisse de son factice : enregistre les appels `duck` / `restore`.
#[derive(Default)]
pub struct FakeDucker {
    events: Mutex<Vec<&'static str>>,
}

impl FakeDucker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Appels dans l'ordre : `"duck"` ou `"restore"`.
    pub fn events(&self) -> Vec<&'static str> {
        self.events.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl Ducker for FakeDucker {
    async fn duck(&self) {
        self.events.lock().unwrap().push("duck");
    }

    async fn restore(&self) {
        self.events.lock().unwrap().push("restore");
    }
}

/// Vérificateur de locuteur factice : l'empreinte renvoyée est `vectors[audio[0]]`.
pub struct FakeVerifier {
    vectors: Vec<Vec<f32>>,
}

impl FakeVerifier {
    pub fn new(vectors: Vec<Vec<f32>>) -> Self {
        Self { vectors }
    }
}

impl crate::speaker::SpeakerVerifier for FakeVerifier {
    fn embed(&self, audio: &[f32]) -> anyhow::Result<Vec<f32>> {
        let id = audio.first().copied().unwrap_or(0.0) as usize;
        self.vectors
            .get(id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("empreinte factice inconnue"))
    }
}

/// Voix programmée (son fixe ou échec), qui garde les phrases reçues.
pub struct FakeSynthesizer {
    speech: Option<Speech>,
    texts: Mutex<Vec<String>>,
    delay: std::time::Duration,
    active: AtomicUsize,
    max_active: AtomicUsize,
}

impl FakeSynthesizer {
    /// Répond toujours `speech`.
    pub fn new(speech: Speech) -> Self {
        Self::with_speech(Some(speech))
    }

    /// Échoue à chaque appel.
    pub fn failing() -> Self {
        Self::with_speech(None)
    }

    fn with_speech(speech: Option<Speech>) -> Self {
        Self {
            speech,
            texts: Mutex::new(Vec::new()),
            delay: std::time::Duration::ZERO,
            active: AtomicUsize::new(0),
            max_active: AtomicUsize::new(0),
        }
    }

    /// Synthèse lente : bloque le fil `delay` (temps réel) à chaque appel.
    pub fn with_delay(mut self, delay: std::time::Duration) -> Self {
        self.delay = delay;
        self
    }

    /// Phrases reçues, dans l'ordre.
    pub fn texts(&self) -> Vec<String> {
        self.texts.lock().unwrap().clone()
    }

    /// Plus grand nombre de synthèses simultanées observé.
    pub fn max_concurrent(&self) -> usize {
        self.max_active.load(Ordering::SeqCst)
    }
}

impl Synthesizer for FakeSynthesizer {
    fn synthesize(&self, text: &str) -> anyhow::Result<Speech> {
        self.texts.lock().unwrap().push(text.to_string());
        let now = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_active.fetch_max(now, Ordering::SeqCst);
        if !self.delay.is_zero() {
            std::thread::sleep(self.delay);
        }
        self.active.fetch_sub(1, Ordering::SeqCst);
        self.speech
            .clone()
            .ok_or_else(|| anyhow::anyhow!("voix en panne"))
    }
}

/// Horloge qui n'avance que sur demande.
pub struct ManualClock {
    start: std::time::Instant,
    offset: Mutex<std::time::Duration>,
}

impl ManualClock {
    pub fn new() -> Self {
        Self {
            start: std::time::Instant::now(),
            offset: Mutex::new(std::time::Duration::ZERO),
        }
    }

    pub fn advance(&self, d: std::time::Duration) {
        *self.offset.lock().unwrap() += d;
    }
}

impl Default for ManualClock {
    fn default() -> Self {
        Self::new()
    }
}

impl crate::brain::Clock for ManualClock {
    fn now(&self) -> std::time::Instant {
        self.start + *self.offset.lock().unwrap()
    }
}
