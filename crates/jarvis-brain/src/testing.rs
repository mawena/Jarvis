//! Doublures de test partagées (`FakeLlm`, `FakeSkill`).

use crate::intent::Intent;
use crate::llm::{LlmBackend, LlmError, Understanding};
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
    async fn understand(
        &self,
        _text: &str,
        tools: &[ToolSpec],
    ) -> Result<Understanding, LlmError> {
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
