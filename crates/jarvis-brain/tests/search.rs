//! Recherche de bout en bout : vrai cerveau, vrais backends (Ollama, Claude) et
//! SearXNG simulés par wiremock. Aucun accès au réseau.

use jarvis_brain::brain::Brain;
use jarvis_brain::config::{LocalLlmConfig, OnlineLlmConfig};
use jarvis_brain::llm::{ClaudeBackend, OllamaBackend};
use jarvis_brain::mode_store::ModeStore;
use jarvis_brain::summarizer::ModeSummarizer;
use jarvis_protocol::Mode;
use jarvis_skills::search::{SearchSettings, SearchSkill, SearxClient};
use jarvis_skills::{FakeRunner, Outcome, Skill};
use reqwest::Url;
use serde_json::{Value, json};
use std::sync::Arc;
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const PHRASE: &str = "cherche la météo de demain à Lomé";
const REQUETE: &str = "la météo de demain à Lomé";
const RESUME_LOCAL: &str = "Demain à Lomé, il fera beau et chaud, Monsieur.";
const RESUME_EN_LIGNE: &str = "Demain, soleil et trente et un degrés à Lomé.";

/// Page de résultats de SearXNG (format JSON).
fn resultats() -> Value {
    json!({
        "results": [
            {
                "url": "https://meteo.example.tg/lome",
                "title": "Météo de Lomé",
                "content": "Demain, 31°C au soleil. Vent faible."
            },
            {
                "url": "https://site2.fr/page",
                "title": "Prévisions au Togo",
                "content": "Ciel dégagé sur la côte."
            }
        ]
    })
}

async fn searxng() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .and(body_string_contains("format=json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(resultats()))
        .mount(&server)
        .await;
    server
}

async fn ollama() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "message": {"role": "assistant", "content": RESUME_LOCAL},
            "done": true
        })))
        .mount(&server)
        .await;
    server
}

async fn claude() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "content": [{"type": "text", "text": RESUME_EN_LIGNE}],
            "stop_reason": "end_turn"
        })))
        .mount(&server)
        .await;
    server
}

/// Cerveau monté comme dans `main.rs`, avec la seule compétence de recherche.
struct Banc {
    brain: Brain,
    runner: Arc<FakeRunner>,
    ollama: MockServer,
    claude: MockServer,
    _dir: tempfile::TempDir,
}

async fn banc(mode: Mode, searx_url: &str) -> Banc {
    let (ollama, claude) = (ollama().await, claude().await);
    let dir = tempfile::tempdir().unwrap();
    let modes = Arc::new(ModeStore::new(dir.path().join("mode"), Mode::Local));
    modes.set(mode).unwrap();
    let local = Arc::new(OllamaBackend::new(
        &LocalLlmConfig {
            url: ollama.uri(),
            ..LocalLlmConfig::default()
        },
        "Monsieur",
    ));
    let online = Arc::new(ClaudeBackend::new(
        &OnlineLlmConfig {
            url: claude.uri(),
            ..OnlineLlmConfig::default()
        },
        "Monsieur",
        Some("test-key".into()),
    ));
    let settings = SearchSettings {
        url: searx_url.to_string(),
        ..SearchSettings::default()
    };
    let client = Arc::new(SearxClient::new(&settings).unwrap());
    let summarizer = ModeSummarizer::new(
        local.clone(),
        online.clone(),
        modes.clone(),
        "Monsieur",
        &settings,
    );
    let runner = Arc::new(FakeRunner::new());
    let skills: Vec<Arc<dyn Skill>> = vec![Arc::new(SearchSkill::new(
        client,
        Arc::new(summarizer),
        runner.clone(),
        settings,
    ))];
    let brain = Brain::new(skills, local, online, modes);
    Banc {
        brain,
        runner,
        ollama,
        claude,
        _dir: dir,
    }
}

async fn corps(server: &MockServer) -> Vec<Value> {
    server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|r| serde_json::from_slice(&r.body).unwrap())
        .collect()
}

#[tokio::test]
async fn recherche_bout_en_bout_mode_local_sans_ia_en_ligne() {
    let searx = searxng().await;
    let b = banc(Mode::Local, &searx.uri()).await;
    let reply = b.brain.handle_text(PHRASE).await;
    assert_eq!(reply.speech, RESUME_LOCAL);
    assert_eq!(reply.outcome, Outcome::Info);
    // Mode local étanche : le faux Claude n'a rien reçu.
    assert!(b.claude.received_requests().await.unwrap().is_empty());
    let ollama = corps(&b.ollama).await;
    assert_eq!(ollama.len(), 1, "{ollama:?}");
    assert!(ollama[0].get("tools").is_none(), "{}", ollama[0]);
    let user = ollama[0]["messages"][1]["content"].as_str().unwrap();
    assert!(user.contains(REQUETE), "{user}");
    assert!(user.contains("Météo de Lomé"), "{user}");
    // La requête part en POST, jamais dans l'URL.
    let reqs = searx.received_requests().await.unwrap();
    assert_eq!(reqs.len(), 1);
    assert!(reqs[0].url.query().is_none(), "{}", reqs[0].url);
}

#[tokio::test]
async fn recherche_bout_en_bout_mode_en_ligne() {
    let searx = searxng().await;
    let b = banc(Mode::Online, &searx.uri()).await;
    let reply = b.brain.handle_text(PHRASE).await;
    assert_eq!(reply.speech, RESUME_EN_LIGNE);
    let claude = corps(&b.claude).await;
    assert_eq!(claude.len(), 1, "{claude:?}");
    assert!(claude[0].get("tools").is_none(), "{}", claude[0]);
    assert!(
        b.ollama.received_requests().await.unwrap().is_empty(),
        "aucune complétion locale en mode en ligne"
    );
}

#[tokio::test]
async fn recherche_puis_ouvre_les_resultats() {
    let searx = searxng().await;
    let b = banc(Mode::Local, &searx.uri()).await;
    b.brain.handle_text(PHRASE).await;

    let reply = b.brain.handle_text("ouvre les résultats").await;
    assert_eq!(reply.outcome, Outcome::Done, "{reply:?}");
    let calls = b.runner.calls();
    assert_eq!(calls.len(), 1, "{calls:?}");
    let (program, args) = &calls[0];
    assert_eq!(program, "xdg-open");
    assert_eq!(args.len(), 1, "un seul argument, sans shell");
    // Page HTML du SearXNG local, avec la requête.
    let url = Url::parse(&args[0]).unwrap();
    assert_eq!(url.origin(), Url::parse(&searx.uri()).unwrap().origin());
    assert_eq!(url.path(), "/search");
    let q: Vec<_> = url.query_pairs().collect();
    assert_eq!(q, [("q".into(), REQUETE.into())]);

    let reply = b.brain.handle_text("ouvre le deuxième résultat").await;
    assert_eq!(reply.outcome, Outcome::Done, "{reply:?}");
    let calls = b.runner.calls();
    assert_eq!(
        calls[1],
        ("xdg-open".into(), vec!["https://site2.fr/page".into()])
    );
    // « Ouvre les résultats » ne refait pas de recherche.
    assert_eq!(searx.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn searxng_arrete_reponse_claire() {
    // Port libre : rien n'y écoute.
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let b = banc(Mode::Local, &format!("http://127.0.0.1:{port}")).await;
    let reply = b.brain.handle_text(PHRASE).await;
    assert_eq!(reply.speech, "Le moteur de recherche ne répond pas.");
    assert_eq!(reply.outcome, Outcome::Failed);
    assert!(b.ollama.received_requests().await.unwrap().is_empty());
    assert!(b.claude.received_requests().await.unwrap().is_empty());
}

/// SearXNG réel (`jarvisctl searx up`) : `cargo test --test search -- --ignored`.
#[tokio::test]
#[ignore]
async fn searxng_reel_repond() {
    let client = SearxClient::new(&SearchSettings::default()).unwrap();
    let page = client.search("Lomé Togo").await.expect("SearXNG local");
    assert!(!page.hits.is_empty(), "aucun résultat : {page:?}");
    for hit in &page.hits {
        assert!(hit.url.starts_with("http"), "{}", hit.url);
    }
}
