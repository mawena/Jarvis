use super::router::{Route, classify};
use super::{LlmBackend, LlmError, Understanding, chat_prompt, system_prompt};
use crate::config::LocalLlmConfig;
use crate::intent::Intent;
use jarvis_skills::ToolSpec;
use serde_json::{Value, json};
use std::time::Duration;
use tracing::debug;

const TIMEOUT: Duration = Duration::from_secs(20);
/// Le premier chargement du modèle en mémoire peut dépasser largement TIMEOUT.
const WARM_UP_TIMEOUT: Duration = Duration::from_secs(120);

pub struct OllamaBackend {
    client: reqwest::Client,
    url: String,
    model: String,
    keep_alive: String,
    address: String,
}

impl OllamaBackend {
    pub fn new(cfg: &LocalLlmConfig, address: &str) -> Self {
        Self {
            client: reqwest::Client::new(),
            url: cfg.url.trim_end_matches('/').to_string(),
            model: cfg.model.clone(),
            keep_alive: cfg.keep_alive.clone(),
            address: address.to_string(),
        }
    }

    /// Charge le modèle et lui fait lire les consignes et les outils, sans rien générer :
    /// Ollama garde ce préfixe en cache, et la première vraie phrase est rapide.
    pub async fn warm_up(&self, tools: &[ToolSpec]) -> Result<(), LlmError> {
        let mut body = self.chat_body("bonjour", tools);
        body["options"] = json!({"num_predict": 1});
        let resp = self
            .client
            .post(format!("{}/api/chat", self.url))
            .timeout(WARM_UP_TIMEOUT)
            .json(&body)
            .send()
            .await
            .map_err(|e| LlmError::Unreachable(e.to_string()))?;
        check_status(resp.status())
    }

    /// Conversation libre : aucun outil, consigne courte, réponse brève.
    fn conversation_body(&self, text: &str) -> Value {
        json!({
            "model": self.model,
            "stream": false,
            "messages": [
                {"role": "system", "content": chat_prompt(&self.address)},
                {"role": "user", "content": text},
            ],
            "options": {"num_predict": 100},
            "keep_alive": self.keep_alive,
        })
    }

    fn chat_body(&self, text: &str, tools: &[ToolSpec]) -> Value {
        let tools: Vec<Value> = tools
            .iter()
            .map(|t| {
                json!({"type": "function", "function": {
                    "name": t.name, "description": t.description, "parameters": t.parameters}})
            })
            .collect();
        json!({
            "model": self.model,
            "stream": false,
            "messages": [
                {"role": "system", "content": system_prompt(&self.address)},
                {"role": "user", "content": text},
            ],
            "tools": tools,
            "keep_alive": self.keep_alive,
        })
    }
}

fn check_status(status: reqwest::StatusCode) -> Result<(), LlmError> {
    if status.is_client_error() {
        return Err(LlmError::Rejected(format!("Ollama a répondu {status}")));
    }
    if !status.is_success() {
        return Err(LlmError::Unreachable(format!("Ollama a répondu {status}")));
    }
    Ok(())
}

#[async_trait::async_trait]
impl LlmBackend for OllamaBackend {
    async fn understand(&self, text: &str, tools: &[ToolSpec]) -> Result<Understanding, LlmError> {
        let route = classify(text);
        debug!(?route, "aiguillage local");
        let (body, tools) = match route {
            Route::Command => (self.chat_body(text, tools), tools),
            Route::Conversation => (self.conversation_body(text), &[][..]),
        };
        let resp = self
            .client
            .post(format!("{}/api/chat", self.url))
            .timeout(TIMEOUT)
            .json(&body)
            .send()
            .await
            .map_err(|e| LlmError::Unreachable(e.to_string()))?;
        check_status(resp.status())?;
        let v: Value = resp
            .json()
            .await
            .map_err(|e| LlmError::BadResponse(e.to_string()))?;
        if route == Route::Conversation {
            return v
                .pointer("/message/content")
                .and_then(Value::as_str)
                .map(|c| Understanding::Answer(c.trim().to_string()))
                .ok_or_else(|| LlmError::BadResponse("contenu absent".into()));
        }
        parse(&v, tools)
    }
}

fn parse_args(raw: Option<&Value>) -> Result<Value, LlmError> {
    match raw {
        None | Some(Value::Null) => Ok(json!({})),
        Some(obj @ Value::Object(_)) => Ok(obj.clone()),
        Some(Value::String(s)) => match serde_json::from_str::<Value>(s) {
            Ok(obj @ Value::Object(_)) => Ok(obj),
            _ => Err(LlmError::BadResponse("arguments illisibles".into())),
        },
        Some(_) => Err(LlmError::BadResponse("arguments invalides".into())),
    }
}

/// Les petits modèles écrivent parfois l'appel d'outil en texte (`{"name": ..., "parameters": ...}`),
/// seul ou entouré de balises ou de phrases. Renvoie le nom et les arguments bruts s'il y en a un.
fn text_tool_call(content: &str) -> Option<(String, Value)> {
    let start = content.find('{')?;
    let Some(Ok(value)) = serde_json::Deserializer::from_str(&content[start..])
        .into_iter::<Value>()
        .next()
    else {
        return malformed_tool_call(&content[start..]);
    };
    let name = value.get("name")?.as_str()?.to_string();
    let args = value.get("parameters").or_else(|| value.get("arguments"))?;
    Some((name, args.clone()))
}

/// JSON cassé (ex. `{"name":"volume_up","parameters":{"}}`) : on ne garde que le nom, sans arguments.
fn malformed_tool_call(text: &str) -> Option<(String, Value)> {
    if !text.contains("\"parameters\"") && !text.contains("\"arguments\"") {
        return None;
    }
    let rest = text.split("\"name\"").nth(1)?;
    let rest = rest.trim_start().strip_prefix(':')?.trim_start();
    let rest = rest.strip_prefix('"')?;
    let name = rest.split('"').next()?;
    Some((name.to_string(), json!({})))
}

fn parse(v: &Value, tools: &[ToolSpec]) -> Result<Understanding, LlmError> {
    let message = v
        .get("message")
        .ok_or_else(|| LlmError::BadResponse("champ message absent".into()))?;
    if let Some(call) = message.pointer("/tool_calls/0/function") {
        let name = call
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| LlmError::BadResponse("nom d'outil absent".into()))?;
        let args = parse_args(call.get("arguments"))?;
        return Ok(Understanding::Tool(Intent::new(name, args)));
    }
    let content = message
        .get("content")
        .and_then(Value::as_str)
        .ok_or_else(|| LlmError::BadResponse("contenu absent".into()))?;
    if let Some((name, raw)) = text_tool_call(content) {
        if !tools.iter().any(|t| t.name == name) {
            return Err(LlmError::BadResponse(format!(
                "appel d'un outil inexistant écrit en texte : {name}"
            )));
        }
        return Ok(Understanding::Tool(Intent::new(
            name,
            parse_args(Some(&raw))?,
        )));
    }
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
            keep_alive: "30m".to_string(),
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

    async fn reponse_texte(content: &str) -> Result<Understanding, LlmError> {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"message": {"content": content}})),
            )
            .mount(&server)
            .await;
        backend(&server.uri()).understand("x", &[tool()]).await
    }

    #[tokio::test]
    async fn appel_d_outil_ecrit_en_texte_connu() {
        let attendu = Understanding::Tool(Intent::new("app_launch", json!({"name": "firefox"})));
        for texte in [
            r#"{"name":"app_launch","parameters":{"name":"firefox"}}"#,
            r#"{"name":"app_launch","arguments":{"name":"firefox"}}"#,
            "```json\n{\"name\":\"app_launch\",\"parameters\":{\"name\":\"firefox\"}}\n```",
            r#"<Tuple>{"name":"app_launch","parameters":{"name":"firefox"}}</Tuple>"#,
            r#"Voici la commande : {"name": "app_launch", "parameters": {"name": "firefox"}}"#,
        ] {
            assert_eq!(reponse_texte(texte).await.unwrap(), attendu, "{texte}");
        }
    }

    #[tokio::test]
    async fn appel_d_outil_ecrit_en_texte_inconnu() {
        for texte in [
            r#"{"name":"geocode","parameters":{"query":"Pérou"}}"#,
            "```\n{\"name\":\"geocode\",\"arguments\":{}}\n```",
        ] {
            let err = reponse_texte(texte).await.unwrap_err();
            assert!(matches!(err, LlmError::BadResponse(_)), "{texte}: {err:?}");
        }
    }

    #[tokio::test]
    async fn appel_d_outil_en_texte_json_casse() {
        let casse = r#"{"name":"app_launch","parameters":{"}}"#;
        assert_eq!(
            reponse_texte(casse).await.unwrap(),
            Understanding::Tool(Intent::new("app_launch", json!({})))
        );
        let inconnu = r#"{"name":"geocode","parameters":{"}}"#;
        let err = reponse_texte(inconnu).await.unwrap_err();
        assert!(matches!(err, LlmError::BadResponse(_)), "{err:?}");
    }

    #[tokio::test]
    async fn json_ordinaire_reste_une_reponse() {
        let got = reponse_texte(r#"Le Pérou a pour capitale Lima {"ok": true}."#)
            .await
            .unwrap();
        assert!(matches!(got, Understanding::Answer(_)));
    }

    #[tokio::test]
    async fn ollama_injoignable_erreur() {
        let err = backend("http://127.0.0.1:1")
            .understand("salut", &[])
            .await
            .unwrap_err();
        assert!(matches!(err, LlmError::Unreachable(_)), "{err:?}");
    }

    #[tokio::test]
    async fn ollama_4xx_refuse() {
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
    }

    #[tokio::test]
    async fn ollama_envoie_keep_alive() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"message": {"content": "ok"}})),
            )
            .mount(&server)
            .await;
        backend(&server.uri()).understand("x", &[]).await.unwrap();
        let reqs = server.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&reqs[0].body).unwrap();
        assert_eq!(body["keep_alive"], "30m");
    }

    #[tokio::test]
    async fn conversation_sans_outils_repond() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                json!({"message": {"role": "assistant", "content": "Lima, Monsieur."}}),
            ))
            .mount(&server)
            .await;
        let got = backend(&server.uri())
            .understand("quelle est la capitale du Pérou", &[tool()])
            .await
            .unwrap();
        assert_eq!(got, Understanding::Answer("Lima, Monsieur.".into()));
        let reqs = server.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&reqs[0].body).unwrap();
        assert!(body.get("tools").is_none(), "{body}");
        assert_eq!(body["keep_alive"], "30m");
    }

    #[tokio::test]
    async fn commande_envoie_les_outils() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"message": {"content": "ok"}})),
            )
            .mount(&server)
            .await;
        backend(&server.uri())
            .understand("ouvre firefox", &[tool()])
            .await
            .unwrap();
        let reqs = server.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&reqs[0].body).unwrap();
        assert_eq!(body["tools"][0]["function"]["name"], "app_launch");
    }

    #[tokio::test]
    async fn prechauffage_lit_les_outils() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"message": {"content": "ok"}})),
            )
            .mount(&server)
            .await;
        let b = backend(&server.uri());
        b.warm_up(&[tool()]).await.unwrap();
        b.understand("ouvre firefox", &[tool()]).await.unwrap();
        let reqs = server.received_requests().await.unwrap();
        let warm: Value = serde_json::from_slice(&reqs[0].body).unwrap();
        let real: Value = serde_json::from_slice(&reqs[1].body).unwrap();
        // Même préfixe (modèle, consignes, outils) que les vraies phrases, pour qu'Ollama le garde en cache.
        assert_eq!(warm["model"], real["model"]);
        assert_eq!(warm["messages"][0], real["messages"][0]);
        assert_eq!(warm["tools"], real["tools"]);
        assert_eq!(warm["keep_alive"], "30m");
        assert_eq!(warm["options"]["num_predict"], 1);
    }

    #[tokio::test]
    async fn prechauffage_injoignable() {
        let err = backend("http://127.0.0.1:1")
            .warm_up(&[])
            .await
            .unwrap_err();
        assert!(matches!(err, LlmError::Unreachable(_)), "{err:?}");
    }

    #[tokio::test]
    async fn prechauffage_modele_absent() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        let err = backend(&server.uri()).warm_up(&[]).await.unwrap_err();
        assert!(matches!(err, LlmError::Rejected(_)), "{err:?}");
    }
}
