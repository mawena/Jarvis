use super::{LlmBackend, LlmError, Understanding, system_prompt};
use crate::config::OnlineLlmConfig;
use crate::intent::Intent;
use jarvis_skills::ToolSpec;
use serde_json::{Value, json};
use std::time::Duration;
use tracing::warn;

const TIMEOUT: Duration = Duration::from_secs(15);

pub struct ClaudeBackend {
    client: reqwest::Client,
    url: String,
    model: String,
    api_key_env: String,
    api_key: Option<String>,
    address: String,
}

impl ClaudeBackend {
    pub fn new(cfg: &OnlineLlmConfig, address: &str, api_key: Option<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            url: cfg.url.trim_end_matches('/').to_string(),
            model: cfg.model.clone(),
            api_key_env: cfg.api_key_env.clone(),
            api_key: api_key.filter(|k| !k.is_empty()),
            address: address.to_string(),
        }
    }

    /// Lit la clé dans la variable d'environnement nommée par la configuration.
    pub fn from_env(cfg: &OnlineLlmConfig, address: &str) -> Self {
        Self::new(cfg, address, std::env::var(&cfg.api_key_env).ok())
    }
}

#[async_trait::async_trait]
impl LlmBackend for ClaudeBackend {
    async fn understand(&self, text: &str, tools: &[ToolSpec]) -> Result<Understanding, LlmError> {
        let key = self
            .api_key
            .as_deref()
            .ok_or_else(|| LlmError::MissingApiKey(self.api_key_env.clone()))?;
        let tools: Vec<Value> = tools
            .iter()
            .map(|t| {
                json!({"name": t.name, "description": t.description, "input_schema": t.parameters})
            })
            .collect();
        let body = json!({
            "model": self.model,
            "max_tokens": 1024,
            "system": system_prompt(&self.address),
            "messages": [{"role": "user", "content": text}],
            "tools": tools,
            // Documenté pour claude-haiku-5-5 (GA, sans en-tête bêta).
            "output_config": {"effort": "low"},
        });
        let resp = self
            .client
            .post(format!("{}/v1/messages", self.url))
            .timeout(TIMEOUT)
            .header("x-api-key", key)
            .header("anthropic-version", "2023-06-01")
            .json(&body)
            .send()
            .await
            .map_err(|e| LlmError::Unreachable(e.to_string()))?;
        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(LlmError::Rejected(format!(
                "l'API Claude a répondu {status}"
            )));
        }
        if !status.is_success() {
            return Err(LlmError::Unreachable(format!(
                "l'API Claude a répondu {status}"
            )));
        }
        let v: Value = resp
            .json()
            .await
            .map_err(|e| LlmError::BadResponse(e.to_string()))?;
        parse(&v)
    }
}

fn parse(v: &Value) -> Result<Understanding, LlmError> {
    let blocks = v
        .get("content")
        .and_then(Value::as_array)
        .ok_or_else(|| LlmError::BadResponse("champ content absent".into()))?;
    if let Some(b) = blocks
        .iter()
        .find(|b| b.get("type").and_then(Value::as_str) == Some("tool_use"))
    {
        let name = b
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| LlmError::BadResponse("nom d'outil absent".into()))?;
        let input = b.get("input").cloned().unwrap_or_else(|| json!({}));
        return Ok(Understanding::Tool(Intent::new(name, input)));
    }
    let stop_reason = v.get("stop_reason").and_then(Value::as_str);
    if matches!(stop_reason, Some("max_tokens" | "refusal")) {
        warn!(stop_reason = ?stop_reason, "réponse Claude inexploitable");
        return Err(LlmError::BadResponse(format!(
            "arrêt du modèle : {}",
            stop_reason.unwrap_or_default()
        )));
    }
    // Les blocs `thinking` sont ignorés : seuls les blocs `text` comptent.
    let text: String = blocks
        .iter()
        .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|b| b.get("text").and_then(Value::as_str))
        .collect();
    if text.trim().is_empty() {
        return Err(LlmError::BadResponse("réponse sans outil ni texte".into()));
    }
    Ok(Understanding::Answer(text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn backend(url: &str, key: Option<&str>) -> ClaudeBackend {
        let cfg = OnlineLlmConfig {
            url: url.to_string(),
            ..OnlineLlmConfig::default()
        };
        ClaudeBackend::new(&cfg, "Monsieur", key.map(String::from))
    }

    #[tokio::test]
    async fn claude_appel_outil() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .and(header("x-api-key", "test-key"))
            .and(header("anthropic-version", "2023-06-01"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "content": [{"type": "tool_use", "id": "t1", "name": "media_next", "input": {}}],
                "stop_reason": "tool_use"
            })))
            .expect(1)
            .mount(&server)
            .await;
        let got = backend(&server.uri(), Some("test-key"))
            .understand("suivant", &[])
            .await
            .unwrap();
        assert_eq!(
            got,
            Understanding::Tool(Intent::new("media_next", json!({})))
        );
        let reqs = server.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&reqs[0].body).unwrap();
        assert_eq!(body["model"], "claude-haiku-5-5");
        assert_eq!(body["max_tokens"], 1024);
    }

    #[tokio::test]
    async fn claude_texte() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"content": [{"type": "text", "text": "Bonjour."}]})),
            )
            .mount(&server)
            .await;
        let got = backend(&server.uri(), Some("k"))
            .understand("salut", &[])
            .await
            .unwrap();
        assert_eq!(got, Understanding::Answer("Bonjour.".into()));
    }

    #[tokio::test]
    async fn claude_sans_cle_ne_contacte_rien() {
        let server = MockServer::start().await;
        let err = backend(&server.uri(), None)
            .understand("salut", &[])
            .await
            .unwrap_err();
        assert!(
            matches!(&err, LlmError::MissingApiKey(v) if v == "ANTHROPIC_API_KEY"),
            "{err:?}"
        );
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn claude_500() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        let err = backend(&server.uri(), Some("k"))
            .understand("salut", &[])
            .await
            .unwrap_err();
        assert!(
            matches!(&err, LlmError::Unreachable(m) if m.contains("500")),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn claude_corps_de_requete() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"content": [{"type": "text", "text": "ok"}]})),
            )
            .mount(&server)
            .await;
        let tool = ToolSpec {
            name: "media_next".into(),
            description: "Piste suivante".into(),
            parameters: json!({"type": "object", "properties": {}}),
            risk: jarvis_skills::Risk::Direct,
        };
        backend(&server.uri(), Some("k"))
            .understand("suivant", &[tool])
            .await
            .unwrap();
        let reqs = server.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&reqs[0].body).unwrap();
        assert!(body["system"].as_str().unwrap().starts_with("Tu es Jarvis"));
        assert_eq!(body["max_tokens"], 1024);
        assert_eq!(body["output_config"], json!({"effort": "low"}));
        assert_eq!(body["tools"][0]["name"], "media_next");
        assert_eq!(body["tools"][0]["description"], "Piste suivante");
        assert_eq!(body["tools"][0]["input_schema"]["type"], "object");
    }

    #[tokio::test]
    async fn claude_max_tokens_sans_outil() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "content": [{"type": "thinking", "thinking": "hmm", "signature": "s"}],
                "stop_reason": "max_tokens"
            })))
            .mount(&server)
            .await;
        let err = backend(&server.uri(), Some("k"))
            .understand("salut", &[])
            .await
            .unwrap_err();
        assert!(matches!(err, LlmError::BadResponse(_)), "{err:?}");
    }

    #[tokio::test]
    async fn claude_ignore_thinking_et_refuse_reponse_vide() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "content": [
                    {"type": "thinking", "thinking": "hmm", "signature": "s"},
                    {"type": "text", "text": "Bonjour."}
                ],
                "stop_reason": "end_turn"
            })))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        let got = backend(&server.uri(), Some("k"))
            .understand("salut", &[])
            .await
            .unwrap();
        assert_eq!(got, Understanding::Answer("Bonjour.".into()));
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "content": [{"type": "thinking", "thinking": "hmm", "signature": "s"}],
                "stop_reason": "end_turn"
            })))
            .mount(&server)
            .await;
        let err = backend(&server.uri(), Some("k"))
            .understand("salut", &[])
            .await
            .unwrap_err();
        assert!(matches!(err, LlmError::BadResponse(_)), "{err:?}");
    }

    #[tokio::test]
    async fn claude_cle_vide() {
        let server = MockServer::start().await;
        let err = backend(&server.uri(), Some(""))
            .understand("salut", &[])
            .await
            .unwrap_err();
        assert!(matches!(err, LlmError::MissingApiKey(_)), "{err:?}");
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn claude_cle_refusee() {
        for code in [401, 403] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .respond_with(ResponseTemplate::new(code))
                .mount(&server)
                .await;
            let err = backend(&server.uri(), Some("k"))
                .understand("salut", &[])
                .await
                .unwrap_err();
            assert!(matches!(err, LlmError::Rejected(_)), "{code}: {err:?}");
        }
    }
}
