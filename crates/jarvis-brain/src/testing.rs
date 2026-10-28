//! Doublures de test partagées (`FakeLlm`, `FakeSkill`).

use crate::ducking::Ducker;
use crate::intent::Intent;
use crate::llm::{LlmBackend, LlmError, Understanding};
use crate::stt::Transcriber;
use crate::tts::{Speech, Synthesizer};
use jarvis_skills::{Reply, Risk, Skill, SkillError, ToolSpec};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Backend LLM à réponse programmée, qui compte ses appels.
pub struct FakeLlm {
    response: Mutex<Result<Understanding, LlmError>>,
    calls: AtomicUsize,
    tools: Mutex<Vec<String>>,
}

impl FakeLlm {
    pub fn new(response: Result<Understanding, LlmError>) -> Self {
        Self {
            response: Mutex::new(response),
            calls: AtomicUsize::new(0),
            tools: Mutex::new(Vec::new()),
        }
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
            Err(LlmError::Unreachable(m)) => Err(LlmError::Unreachable(m.clone())),
            Err(LlmError::BadResponse(m)) => Err(LlmError::BadResponse(m.clone())),
            Err(LlmError::MissingApiKey(m)) => Err(LlmError::MissingApiKey(m.clone())),
            Err(LlmError::Rejected(m)) => Err(LlmError::Rejected(m.clone())),
        }
    }
}

/// Compétence factice : déclare des outils, enregistre les appels.
pub struct FakeSkill {
    tool_names: Vec<String>,
    calls: Mutex<Vec<(String, serde_json::Value)>>,
    result: Mutex<Result<Reply, SkillError>>,
    delay: std::time::Duration,
}

impl FakeSkill {
    pub fn new(tool_names: &[&str]) -> Self {
        Self {
            tool_names: tool_names.iter().map(|s| s.to_string()).collect(),
            calls: Mutex::new(Vec::new()),
            result: Mutex::new(Ok(Reply::new("ok"))),
            delay: std::time::Duration::ZERO,
        }
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

    async fn call(&self, tool: &str, args: serde_json::Value) -> Result<Reply, SkillError> {
        self.calls.lock().unwrap().push((tool.to_string(), args));
        if !self.delay.is_zero() {
            tokio::time::sleep(self.delay).await;
        }
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
}

impl FakeSynthesizer {
    /// Répond toujours `speech`.
    pub fn new(speech: Speech) -> Self {
        Self {
            speech: Some(speech),
            texts: Mutex::new(Vec::new()),
        }
    }

    /// Échoue à chaque appel.
    pub fn failing() -> Self {
        Self {
            speech: None,
            texts: Mutex::new(Vec::new()),
        }
    }

    /// Phrases reçues, dans l'ordre.
    pub fn texts(&self) -> Vec<String> {
        self.texts.lock().unwrap().clone()
    }
}

impl Synthesizer for FakeSynthesizer {
    fn synthesize(&self, text: &str) -> anyhow::Result<Speech> {
        self.texts.lock().unwrap().push(text.to_string());
        self.speech
            .clone()
            .ok_or_else(|| anyhow::anyhow!("voix en panne"))
    }
}
