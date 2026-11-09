//! Modèle local servi par LM Studio (API compatible OpenAI, `/v1/chat/completions`).

use super::ollama::parse;
use super::router::{Route, classify};
use super::{LlmBackend, LlmError, Understanding, chat_prompt, system_prompt};
use crate::config::{LocalLlmConfig, keep_alive_secs};
use jarvis_skills::ToolSpec;
use serde_json::{Value, json};
use std::time::Duration;
use tracing::debug;

const TIMEOUT: Duration = Duration::from_secs(20);
/// Le premier chargement du modèle en mémoire peut dépasser largement TIMEOUT.
const WARM_UP_TIMEOUT: Duration = Duration::from_secs(120);

pub struct LmStudioBackend {
    client: reqwest::Client,
    url: String,
    model: String,
    /// Secondes d'inactivité avant que LM Studio décharge le modèle (`ttl`).
    ttl: Option<u64>,
    address: String,
}

impl LmStudioBackend {
    pub fn new(cfg: &LocalLlmConfig, address: &str) -> Self {
        Self {
            client: reqwest::Client::new(),
            url: cfg.url.trim_end_matches('/').to_string(),
            model: cfg.model.clone(),
            ttl: keep_alive_secs(&cfg.keep_alive),
            address: address.to_string(),
        }
    }

    /// Corps commun : modèle, réflexion coupée, durée de garde en mémoire.
    fn body(&self, messages: Value, max_tokens: u32) -> Value {
        let mut body = json!({
            "model": self.model,
            "stream": false,
            "messages": messages,
            "max_tokens": max_tokens,
        });
        no_thinking(&mut body);
        if let Some(ttl) = self.ttl {
            body["ttl"] = json!(ttl);
        }
        body
    }

    /// Conversation libre : aucun outil, consigne courte, réponse brève.
    fn conversation_body(&self, text: &str) -> Value {
        self.body(
            json!([
                {"role": "system", "content": chat_prompt(&self.address)},
                {"role": "user", "content": text},
            ]),
            100,
        )
    }

    fn chat_body(&self, text: &str, tools: &[ToolSpec]) -> Value {
        let tools: Vec<Value> = tools
            .iter()
            .map(|t| {
                json!({"type": "function", "function": {
                    "name": t.name, "description": t.description, "parameters": t.parameters}})
            })
            .collect();
        let mut body = self.body(
            json!([
                {"role": "system", "content": system_prompt(&self.address)},
                {"role": "user", "content": text},
            ]),
            200,
        );
        body["tools"] = json!(tools);
        body
    }

    async fn post(&self, body: &Value, timeout: Duration) -> Result<Value, LlmError> {
        let resp = self
            .client
            .post(format!("{}/v1/chat/completions", self.url))
            .timeout(timeout)
            .json(body)
            .send()
            .await
            .map_err(|e| LlmError::Unreachable(e.to_string()))?;
        let status = resp.status();
        if status.is_client_error() {
            return Err(LlmError::Rejected(format!("LM Studio a répondu {status}")));
        }
        if !status.is_success() {
            return Err(LlmError::Unreachable(format!(
                "LM Studio a répondu {status}"
            )));
        }
        resp.json()
            .await
            .map_err(|e| LlmError::BadResponse(e.to_string()))
    }
}

/// Coupe la réflexion de Qwen3.5 : sinon les 100 jetons de la réponse vocale partent
/// dans le raisonnement.
fn no_thinking(body: &mut Value) {
    body["reasoning_effort"] = json!("none");
}

/// Texte de la réponse, sans bloc de réflexion éventuel.
fn content(v: &Value) -> Option<String> {
    let text = v.pointer("/choices/0/message/content")?.as_str()?;
    let text = match text.rfind("</think>") {
        Some(i) => &text[i + "</think>".len()..],
        None => text,
    };
    Some(text.trim().to_string())
}

#[async_trait::async_trait]
impl LlmBackend for LmStudioBackend {
    /// Charge le modèle et lui fait lire les consignes et les outils, sans rien générer.
    async fn warm_up(&self, tools: &[ToolSpec]) -> Result<(), LlmError> {
        let mut body = self.chat_body("bonjour", tools);
        body["max_tokens"] = json!(1);
        self.post(&body, WARM_UP_TIMEOUT).await.map(|_| ())
    }

    async fn understand(&self, text: &str, tools: &[ToolSpec]) -> Result<Understanding, LlmError> {
        let route = classify(text);
        debug!(?route, "aiguillage local");
        let (body, tools) = match route {
            Route::Command => (self.chat_body(text, tools), tools),
            Route::Conversation => (self.conversation_body(text), &[][..]),
        };
        let v = self.post(&body, TIMEOUT).await?;
        if route == Route::Conversation {
            return content(&v)
                .map(Understanding::Answer)
                .ok_or_else(|| LlmError::BadResponse("contenu absent".into()));
        }
        let choice = v
            .pointer("/choices/0")
            .ok_or_else(|| LlmError::BadResponse("champ choices absent".into()))?;
        parse(choice, tools)
    }

    /// Résumé d'une recherche : aucun outil, réponse courte et peu inventive.
    async fn complete(
        &self,
        system: &str,
        user: &str,
        max_tokens: u32,
    ) -> Result<String, LlmError> {
        let mut body = self.body(
            json!([
                {"role": "system", "content": system},
                {"role": "user", "content": user},
            ]),
            max_tokens,
        );
        body["temperature"] = json!(0.2);
        let v = self.post(&body, TIMEOUT).await?;
        match content(&v) {
            Some(t) if !t.is_empty() => Ok(t),
            _ => Err(LlmError::BadResponse("résumé vide".into())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::intent::Intent;
    use jarvis_skills::Risk;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn backend(url: &str) -> LmStudioBackend {
        let cfg = LocalLlmConfig {
            url: url.to_string(),
            model: "qwen3.5-4b".to_string(),
            ..LocalLlmConfig::default()
        };
        LmStudioBackend::new(&cfg, "Monsieur")
    }

    fn tool() -> ToolSpec {
        ToolSpec {
            name: "app_launch".into(),
            description: "Lance une application".into(),
            parameters: json!({"type": "object", "properties": {"name": {"type": "string"}}}),
            risk: Risk::Direct,
        }
    }

    fn reponse(message: Value) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(json!({"choices": [{"message": message}]}))
    }

    async fn serveur(message: Value) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(reponse(message))
            .mount(&server)
            .await;
        server
    }

    async fn corps(server: &MockServer) -> Value {
        let reqs = server.received_requests().await.unwrap();
        serde_json::from_slice(&reqs[0].body).unwrap()
    }

    #[tokio::test]
    async fn appel_outil_arguments_en_chaine() {
        let server = serveur(json!({
            "role": "assistant", "content": "",
            "tool_calls": [{"type": "function", "function": {
                "name": "app_launch", "arguments": "{\"name\":\"firefox\"}"}}]
        }))
        .await;
        let got = backend(&server.uri())
            .understand("ouvre firefox", &[tool()])
            .await
            .unwrap();
        assert_eq!(
            got,
            Understanding::Tool(Intent::new("app_launch", json!({"name": "firefox"})))
        );
        let body = corps(&server).await;
        assert_eq!(body["stream"], false);
        assert_eq!(body["model"], "qwen3.5-4b");
        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["tools"][0]["function"]["name"], "app_launch");
        assert_eq!(body["messages"][1]["content"], "ouvre firefox");
    }

    #[tokio::test]
    async fn la_requete_coupe_la_reflexion_et_fixe_la_duree_de_garde() {
        let server = serveur(json!({"content": "Il est midi."})).await;
        backend(&server.uri())
            .understand("quelle heure est-il ?", &[])
            .await
            .unwrap();
        let body = corps(&server).await;
        assert_eq!(body["reasoning_effort"], "none");
        assert_eq!(body["ttl"], 1800);
        assert_eq!(body["max_tokens"], 100);
    }

    #[tokio::test]
    async fn reponse_texte_sans_bloc_de_reflexion() {
        let server = serveur(json!({"content": "<think>\nhum\n</think>\n\nIl est midi."})).await;
        let got = backend(&server.uri())
            .understand("l'heure ?", &[])
            .await
            .unwrap();
        assert_eq!(got, Understanding::Answer("Il est midi.".into()));
    }

    #[tokio::test]
    async fn appel_d_outil_ecrit_en_texte() {
        let server = serveur(json!({
            "content": "{\"name\":\"app_launch\",\"parameters\":{\"name\":\"firefox\"}}"
        }))
        .await;
        let got = backend(&server.uri())
            .understand("ouvre firefox", &[tool()])
            .await
            .unwrap();
        assert_eq!(
            got,
            Understanding::Tool(Intent::new("app_launch", json!({"name": "firefox"})))
        );
    }

    #[tokio::test]
    async fn arguments_invalides() {
        let server = serveur(json!({"content": "", "tool_calls": [
            {"function": {"name": "a", "arguments": 42}}]}))
        .await;
        let err = backend(&server.uri())
            .understand("ouvre firefox", &[])
            .await
            .unwrap_err();
        assert!(matches!(err, LlmError::BadResponse(_)), "{err:?}");
    }

    #[tokio::test]
    async fn injoignable_et_refuse() {
        let err = backend("http://127.0.0.1:1")
            .understand("salut", &[])
            .await
            .unwrap_err();
        assert!(matches!(err, LlmError::Unreachable(_)), "{err:?}");

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        let err = backend(&server.uri())
            .understand("salut", &[])
            .await
            .unwrap_err();
        assert!(matches!(err, LlmError::Rejected(_)), "{err:?}");

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        let err = backend(&server.uri())
            .understand("salut", &[])
            .await
            .unwrap_err();
        assert!(matches!(err, LlmError::Unreachable(_)), "{err:?}");
    }

    #[tokio::test]
    async fn complete_sans_outils() {
        let server = serveur(json!({"content": " Lima. "})).await;
        let got = backend(&server.uri())
            .complete("système", "question", 60)
            .await
            .unwrap();
        assert_eq!(got, "Lima.");
        let body = corps(&server).await;
        assert!(body.get("tools").is_none());
        assert_eq!(body["max_tokens"], 60);
        assert_eq!(body["messages"][0]["content"], "système");
    }

    #[tokio::test]
    async fn complete_vide_est_une_erreur() {
        let server = serveur(json!({"content": ""})).await;
        let err = backend(&server.uri())
            .complete("s", "u", 60)
            .await
            .unwrap_err();
        assert!(matches!(err, LlmError::BadResponse(_)), "{err:?}");
    }

    #[tokio::test]
    async fn prechauffage_envoie_les_outils_sans_rien_generer() {
        let server = serveur(json!({"content": ""})).await;
        backend(&server.uri()).warm_up(&[tool()]).await.unwrap();
        let body = corps(&server).await;
        assert_eq!(body["max_tokens"], 1);
        assert_eq!(body["tools"][0]["function"]["name"], "app_launch");
    }

    #[tokio::test]
    async fn prechauffage_en_echec() {
        let err = backend("http://127.0.0.1:1")
            .warm_up(&[])
            .await
            .unwrap_err();
        assert!(matches!(err, LlmError::Unreachable(_)), "{err:?}");
    }
}
