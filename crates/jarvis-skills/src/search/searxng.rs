//! Client du SearXNG local. Les requêtes partent en POST : jamais dans une URL.

use super::SearchSettings;
use crate::browser::checked_url;
use reqwest::Url;
use reqwest::header::CONTENT_TYPE;
use serde::Deserialize;
use std::time::Duration;

/// Un résultat de recherche.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    pub title: String,
    /// Adresse `http`/`https` déjà vérifiée par [`checked_url`].
    pub url: String,
    pub snippet: String,
    /// Nom court du site (`wikipedia`, `lemonde`).
    pub site: String,
}

/// Une page de résultats.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchPage {
    pub hits: Vec<SearchHit>,
    /// Réponse directe du moteur (« 1 euro = 655,957 francs CFA »).
    pub answer: Option<String>,
    /// Texte de l'encadré (Wikipédia, Wikidata).
    pub infobox: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SearxError {
    #[error("moteur de recherche injoignable : {0}")]
    Unreachable(String),
    #[error("format JSON désactivé dans SearXNG (403)")]
    JsonDisabled,
    #[error("réponse inattendue du moteur de recherche : {0}")]
    BadResponse(String),
}

pub struct SearxClient {
    http: reqwest::Client,
    base: Url,
    settings: SearchSettings,
}

impl SearxClient {
    pub fn new(settings: &SearchSettings) -> anyhow::Result<Self> {
        let base = checked_url(&settings.url)
            .ok_or_else(|| anyhow::anyhow!("adresse de SearXNG invalide : {}", settings.url))?;
        let http = reqwest::Client::builder()
            .timeout(Duration::from_millis(settings.timeout_ms))
            .build()?;
        Ok(Self {
            http,
            base,
            settings: settings.clone(),
        })
    }

    /// Recherche générale.
    pub async fn search(&self, q: &str) -> Result<SearchPage, SearxError> {
        let raw = self.post(q, &[("categories", "general")]).await?;
        let answer = raw.answers.iter().find_map(answer_text);
        let infobox = raw
            .infoboxes
            .iter()
            .map(|i| i.content.trim())
            .find(|c| !c.is_empty())
            .map(str::to_string);
        Ok(SearchPage {
            hits: self.hits(raw.results),
            answer,
            infobox,
        })
    }

    /// Recherche de vidéos, sur les moteurs `video_engines`.
    pub async fn videos(&self, q: &str) -> Result<Vec<SearchHit>, SearxError> {
        let engines = self.settings.video_engines.join(",");
        let raw = self
            .post(q, &[("categories", "videos"), ("engines", &engines)])
            .await?;
        Ok(self.hits(raw.results))
    }

    /// Page HTML des résultats, à ouvrir dans le navigateur.
    pub fn results_page_url(&self, q: &str) -> Url {
        let mut url = self.endpoint();
        url.query_pairs_mut().append_pair("q", q);
        url
    }

    fn endpoint(&self) -> Url {
        let mut url = self.base.clone();
        let path = format!("{}/search", self.base.path().trim_end_matches('/'));
        url.set_path(&path);
        url.set_query(None);
        url
    }

    async fn post(&self, q: &str, extra: &[(&str, &str)]) -> Result<RawPage, SearxError> {
        // Corps de formulaire encodé par `Url`, sans la fonction `form` de reqwest.
        let mut form = Url::parse("http://corps.invalid/").expect("adresse fixe valide");
        {
            let safesearch = self.settings.safesearch.to_string();
            let mut pairs = form.query_pairs_mut();
            pairs
                .append_pair("q", q)
                .append_pair("format", "json")
                .append_pair("language", &self.settings.language)
                .append_pair("safesearch", &safesearch);
            for (k, v) in extra {
                pairs.append_pair(k, v);
            }
        }
        let body = form.query().unwrap_or_default().to_string();
        let resp = self
            .http
            .post(self.endpoint())
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(body)
            .send()
            .await
            .map_err(|e| SearxError::Unreachable(e.to_string()))?;
        let status = resp.status();
        if status == reqwest::StatusCode::FORBIDDEN {
            return Err(SearxError::JsonDisabled);
        }
        if !status.is_success() {
            return Err(SearxError::BadResponse(format!("code {status}")));
        }
        resp.json()
            .await
            .map_err(|e| SearxError::BadResponse(e.to_string()))
    }

    /// Résultats gardés : avec titre, à adresse web, sans doublon, `max_results` au plus.
    fn hits(&self, results: Vec<RawResult>) -> Vec<SearchHit> {
        let mut hits: Vec<SearchHit> = Vec::new();
        for r in results {
            if hits.len() >= self.settings.max_results {
                break;
            }
            let title = r.title.trim();
            let Some(url) = checked_url(&r.url) else {
                continue;
            };
            if title.is_empty() || hits.iter().any(|h| h.url == url.as_str()) {
                continue;
            }
            hits.push(SearchHit {
                title: title.to_string(),
                site: url.host_str().map(short_site).unwrap_or_default(),
                url: url.into(),
                snippet: r.content.trim().to_string(),
            });
        }
        hits
    }
}

/// Nom court d'un site : `fr.wikipedia.org` → `wikipedia`, `www.bbc.co.uk` → `bbc`.
pub fn short_site(host: &str) -> String {
    let labels: Vec<&str> = host.trim_end_matches('.').split('.').collect();
    if labels.len() < 2 || host.parse::<std::net::IpAddr>().is_ok() || host.starts_with('[') {
        return host.to_string();
    }
    // Suffixes à deux niveaux courants (`co.uk`, `gouv.fr`, `com.br`).
    const SECOND: &[&str] = &["co", "com", "gouv", "gov", "org", "net", "ac", "edu"];
    let n = labels.len();
    let i = if n >= 3 && SECOND.contains(&labels[n - 2]) {
        n - 3
    } else {
        n - 2
    };
    labels[i].to_string()
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawPage {
    results: Vec<RawResult>,
    answers: Vec<serde_json::Value>,
    infoboxes: Vec<RawInfobox>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawResult {
    url: String,
    title: String,
    content: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawInfobox {
    content: String,
}

/// Une réponse directe : objet `{answer: …}` (récent) ou simple texte (ancien).
fn answer_text(v: &serde_json::Value) -> Option<String> {
    let text = v.get("answer").unwrap_or(v).as_str()?.trim();
    (!text.is_empty()).then(|| text.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{body_string_contains, header, method, path};
    use wiremock::{Mock, MockServer, Request, ResponseTemplate};

    fn settings(server: &MockServer) -> SearchSettings {
        SearchSettings {
            url: server.uri(),
            ..SearchSettings::default()
        }
    }

    fn client(server: &MockServer) -> SearxClient {
        SearxClient::new(&settings(server)).unwrap()
    }

    fn result(url: &str, title: &str) -> serde_json::Value {
        json!({"url": url, "title": title, "content": format!("extrait de {title}")})
    }

    async fn respond(server: &MockServer, body: serde_json::Value) {
        Mock::given(method("POST"))
            .and(path("/search"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(server)
            .await;
    }

    fn body_of(req: &Request) -> String {
        String::from_utf8(req.body.clone()).unwrap()
    }

    #[tokio::test]
    async fn searx_envoie_post_json_fr() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/search"))
            .and(header("content-type", "application/x-www-form-urlencoded"))
            .and(body_string_contains("q=m%C3%A9t%C3%A9o+de+demain+%26+co"))
            .and(body_string_contains("format=json"))
            .and(body_string_contains("language=fr-FR"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"results": []})))
            .expect(1)
            .mount(&server)
            .await;
        let page = client(&server)
            .search("météo de demain & co")
            .await
            .unwrap();
        assert_eq!(page, SearchPage::default());
        let reqs = server.received_requests().await.unwrap();
        let body = body_of(&reqs[0]);
        assert!(body.contains("safesearch=1"), "{body}");
        assert!(body.contains("categories=general"), "{body}");
        // La requête n'est jamais dans l'adresse.
        assert_eq!(reqs[0].url.query(), None);
    }

    #[tokio::test]
    async fn searx_garde_les_n_premiers_resultats() {
        let server = MockServer::start().await;
        let results: Vec<_> = (1..=8)
            .map(|i| result(&format!("https://site{i}.fr/"), &format!("Titre {i}")))
            .collect();
        respond(&server, json!({ "results": results })).await;
        let hits = client(&server).search("x").await.unwrap().hits;
        let titles: Vec<_> = hits.iter().map(|h| h.title.as_str()).collect();
        assert_eq!(
            titles,
            ["Titre 1", "Titre 2", "Titre 3", "Titre 4", "Titre 5"]
        );
        assert_eq!(
            hits[0],
            SearchHit {
                title: "Titre 1".into(),
                url: "https://site1.fr/".into(),
                snippet: "extrait de Titre 1".into(),
                site: "site1".into(),
            }
        );
    }

    #[tokio::test]
    async fn searx_ignore_les_liens_non_http() {
        let server = MockServer::start().await;
        let results = json!([
            result("javascript:alert(1)", "Piège"),
            result("file:///etc/passwd", "Fichier"),
            result("https://bon.fr/", "  "),
            {"title": "Sans adresse"},
            result("http://ok.fr/page", "Bon"),
        ]);
        respond(&server, json!({ "results": results })).await;
        let hits = client(&server).search("x").await.unwrap().hits;
        let urls: Vec<_> = hits.iter().map(|h| h.url.as_str()).collect();
        assert_eq!(urls, ["http://ok.fr/page"]);
    }

    #[tokio::test]
    async fn searx_dedoublonne() {
        let server = MockServer::start().await;
        let results = json!([
            result("https://a.fr/", "A"),
            result("https://a.fr/", "A bis"),
            result("https://b.fr/", "B"),
        ]);
        respond(&server, json!({ "results": results })).await;
        let hits = client(&server).search("x").await.unwrap().hits;
        let titles: Vec<_> = hits.iter().map(|h| h.title.as_str()).collect();
        assert_eq!(titles, ["A", "B"]);
    }

    #[tokio::test]
    async fn searx_lit_reponse_et_infobox() {
        let server = MockServer::start().await;
        respond(
            &server,
            json!({
                "results": [result("https://a.fr/", "A")],
                "answers": [{"answer": " 1 euro = 655,957 francs CFA "}],
                "infoboxes": [{"infobox": "Lomé", "content": "Lomé est la capitale du Togo."}],
                "suggestions": ["autre"],
            }),
        )
        .await;
        let page = client(&server).search("x").await.unwrap();
        assert_eq!(page.answer.as_deref(), Some("1 euro = 655,957 francs CFA"));
        assert_eq!(
            page.infobox.as_deref(),
            Some("Lomé est la capitale du Togo.")
        );
        assert_eq!(page.hits.len(), 1);
        // Ancien format : réponse en simple texte.
        assert_eq!(
            answer_text(&json!("42")).as_deref(),
            Some("42"),
            "réponse texte"
        );
    }

    #[tokio::test]
    async fn searx_403_json_desactive() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(403))
            .mount(&server)
            .await;
        let err = client(&server).search("x").await.unwrap_err();
        assert_eq!(err, SearxError::JsonDisabled);
        // Autre code : réponse inattendue.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        let err = client(&server).search("x").await.unwrap_err();
        assert!(matches!(err, SearxError::BadResponse(_)), "{err:?}");
    }

    #[tokio::test]
    async fn searx_delai_depasse() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"results": []}))
                    .set_delay(Duration::from_secs(2)),
            )
            .mount(&server)
            .await;
        let s = SearchSettings {
            timeout_ms: 100,
            ..settings(&server)
        };
        let started = std::time::Instant::now();
        let err = SearxClient::new(&s).unwrap().search("x").await.unwrap_err();
        assert!(matches!(err, SearxError::Unreachable(_)), "{err:?}");
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[tokio::test]
    async fn searx_videos_categorie_et_moteurs() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/search"))
            .and(body_string_contains("categories=videos"))
            .and(body_string_contains("engines=youtube%2Cpeertube"))
            .and(body_string_contains("q=daft+punk"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "results": [result("https://www.youtube.com/watch?v=abc", "Daft Punk")]
            })))
            .expect(1)
            .mount(&server)
            .await;
        let s = SearchSettings {
            video_engines: vec!["youtube".into(), "peertube".into()],
            ..settings(&server)
        };
        let hits = SearxClient::new(&s)
            .unwrap()
            .videos("daft punk")
            .await
            .unwrap();
        assert_eq!(hits[0].url, "https://www.youtube.com/watch?v=abc");
        assert_eq!(hits[0].site, "youtube");
    }

    #[tokio::test]
    async fn searx_injoignable_et_page_de_resultats() {
        // Port fermé : aucune vraie requête ne sort du PC.
        let s = SearchSettings {
            url: "http://127.0.0.1:9/".into(),
            ..SearchSettings::default()
        };
        let c = SearxClient::new(&s).unwrap();
        assert!(matches!(
            c.search("x").await,
            Err(SearxError::Unreachable(_))
        ));
        assert_eq!(
            c.results_page_url("météo à Lomé").as_str(),
            "http://127.0.0.1:9/search?q=m%C3%A9t%C3%A9o+%C3%A0+Lom%C3%A9"
        );
        let bad = SearchSettings {
            url: "file:///tmp".into(),
            ..SearchSettings::default()
        };
        assert!(SearxClient::new(&bad).is_err());
    }

    #[test]
    fn nom_court_du_site() {
        for (host, site) in [
            ("fr.wikipedia.org", "wikipedia"),
            ("www.lemonde.fr", "lemonde"),
            ("lemonde.fr", "lemonde"),
            ("www.bbc.co.uk", "bbc"),
            ("www.service-public.gouv.fr", "service-public"),
            ("localhost", "localhost"),
            ("127.0.0.1", "127.0.0.1"),
        ] {
            assert_eq!(short_site(host), site, "{host}");
        }
    }
}
