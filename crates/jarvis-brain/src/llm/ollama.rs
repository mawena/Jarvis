use super::{LlmBackend, LlmError, Understanding, system_prompt};
use crate::config::LocalLlmConfig;
use crate::intent::Intent;
use jarvis_skills::ToolSpec;
use serde_json::{Value, json};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(20);

pub struct OllamaBackend {
    client: reqwest::Client,
    url: String,
    model: String,
    address: String,
}

impl OllamaBackend {
    pub fn new(cfg: &LocalLlmConfig, address: &str) -> Self {
        Self {
            client: reqwest::Client::new(),
            url: cfg.url.trim_end_matches('/').to_string(),
            model: cfg.model.clone(),
            address: address.to_string(),
        }
    }
}

#[async_trait::async_trait]
impl LlmBackend for OllamaBackend {
    async fn understand(&self, text: &str, tools: &[ToolSpec]) -> Result<Understanding, LlmError> {
        let tools: Vec<Value> = tools
            .iter()
            .map(|t| {
                json!({"type": "function", "function": {
                    "name": t.name, "description": t.description, "parameters": t.parameters}})
            })
            .collect();
        let body = json!({
            "model": self.model,
            "stream": false,
            "messages": [
                {"role": "system", "content": system_prompt(&self.address)},
                {"role": "user", "content": text},
            ],
            "tools": tools,
        });
        let resp = self
            .client
            .post(format!("{}/api/chat", self.url))
            .timeout(TIMEOUT)
            .json(&body)
            .send()
            .await
            .map_err(|e| LlmError::Unreachable(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(LlmError::Unreachable(format!("Ollama a répondu {status}")));
        }
        let v: Value = resp
            .json()
            .await
            .map_err(|e| LlmError::BadResponse(e.to_string()))?;
        parse(&v)
    }
}

fn parse(v: &Value) -> Result<Understanding, LlmError> {
    let message = v
        .get("message")
        .ok_or_else(|| LlmError::BadResponse("champ message absent".into()))?;
    if let Some(call) = message.pointer("/tool_calls/0/function") {
        let name = call
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| LlmError::BadResponse("nom d'outil absent".into()))?;
        let args = match call.get("arguments") {
            None | Some(Value::Null) => json!({}),
            Some(obj @ Value::Object(_)) => obj.clone(),
            Some(Value::String(s)) => match serde_json::from_str::<Value>(s) {
                Ok(obj @ Value::Object(_)) => obj,
                _ => return Err(LlmError::BadResponse("arguments illisibles".into())),
            },
            Some(_) => return Err(LlmError::BadResponse("arguments invalides".into())),
        };
        return Ok(Understanding::Tool(Intent::new(name, args)));
    }
    let content = message
        .get("content")
        .and_then(Value::as_str)
        .ok_or_else(|| LlmError::BadResponse("contenu absent".into()))?;
    Ok(Understanding::Answer(content.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use jarvis_skills::Risk;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn backend(url: &str) -> OllamaBackend {
        let cfg = LocalLlmConfig {
            url: url.to_string(),
            model: "qwen2.5:3b".to_string(),
        };
        OllamaBackend::new(&cfg, "Monsieur")
    }

    fn tool() -> ToolSpec {
        ToolSpec {
            name: "app_launch".into(),
            description: "Lance une application".into(),
            parameters: json!({"type": "object", "properties": {"name": {"type": "string"}}}),
            risk: Risk::Direct,
        }
    }

    #[tokio::test]
    async fn ollama_appel_outil() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"message": {
                "role": "assistant", "content": "",
                "tool_calls": [{"function": {"name": "app_launch", "arguments": {"name": "firefox"}}}]
            }})))
            .mount(&server)
            .await;
        let got = backend(&server.uri())
            .understand("ouvre firefox", &[tool()])
            .await
            .unwrap();
        assert_eq!(
            got,
            Understanding::Tool(Intent::new("app_launch", json!({"name": "firefox"})))
        );
        let reqs = server.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&reqs[0].body).unwrap();
        assert_eq!(body["stream"], false);
        assert_eq!(body["model"], "qwen2.5:3b");
        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["tools"][0]["function"]["name"], "app_launch");
        assert_eq!(body["messages"][1]["content"], "ouvre firefox");
    }

    #[tokio::test]
    async fn ollama_arguments_en_chaine() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"message": {
                "content": "",
                "tool_calls": [{"function": {"name": "app_launch", "arguments": "{\"name\":\"firefox\"}"}}]
            }})))
            .mount(&server)
            .await;
        let got = backend(&server.uri()).understand("x", &[]).await.unwrap();
        assert_eq!(
            got,
            Understanding::Tool(Intent::new("app_launch", json!({"name": "firefox"})))
        );
    }

    #[tokio::test]
    async fn ollama_arguments_invalides() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"message": {
                "content": "",
                "tool_calls": [{"function": {"name": "a", "arguments": 42}}]
            }})))
            .mount(&server)
            .await;
        let err = backend(&server.uri())
            .understand("x", &[])
            .await
            .unwrap_err();
        assert!(matches!(err, LlmError::BadResponse(_)), "{err:?}");
    }

    #[tokio::test]
    async fn ollama_reponse_texte() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                json!({"message": {"role": "assistant", "content": "Il est midi."}}),
            ))
            .mount(&server)
            .await;
        let got = backend(&server.uri())
            .understand("l'heure ?", &[])
            .await
            .unwrap();
        assert_eq!(got, Understanding::Answer("Il est midi.".into()));
    }

    #[tokio::test]
    async fn ollama_injoignable_erreur() {
        let err = backend("http://127.0.0.1:1")
            .understand("salut", &[])
            .await
            .unwrap_err();
        assert!(matches!(err, LlmError::Unreachable(_)), "{err:?}");
    }
}
