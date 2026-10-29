//! Recherche sur internet par le SearXNG local, résumée à voix haute.

pub mod extract;
pub mod searxng;
pub mod spoken;
pub mod summary;

pub use searxng::{SearchHit, SearchPage, SearxClient, SearxError};
#[cfg(any(test, feature = "test-util"))]
pub use summary::FakeSummarizer;
pub use summary::{NoSummarizer, Source, Summarizer};

use crate::browser::{checked_url, open_url};
use crate::{CommandRunner, Reply, Risk, Skill, SkillError, ToolSpec};
use serde::Deserialize;
use serde_json::Value;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

/// Réglages de la section `[search]`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct SearchSettings {
    /// Adresse du SearXNG local.
    pub url: String,
    pub language: String,
    /// 0 : aucun filtre, 1 : modéré, 2 : strict.
    pub safesearch: u8,
    /// Résultats gardés en mémoire (« ouvre le deuxième résultat »).
    pub max_results: usize,
    /// Résultats donnés à l'IA pour le résumé.
    pub summary_sources: usize,
    /// Longueur maximale d'un extrait donné à l'IA.
    pub snippet_chars: usize,
    /// Délai de SearXNG.
    pub timeout_ms: u64,
    /// Résumé par l'IA ; sinon l'extrait seul.
    pub summarize: bool,
    pub local_summary_timeout_ms: u64,
    pub online_summary_timeout_ms: u64,
    pub summary_max_tokens: u32,
    /// Modèle Ollama du résumé ; vide : le modèle de compréhension.
    pub local_summary_model: String,
    /// Moteurs de SearXNG pour les vidéos.
    pub video_engines: Vec<String>,
    /// `yt-dlp` en secours si SearXNG ne trouve pas de vidéo.
    pub ytdlp: bool,
    pub ytdlp_timeout_s: u64,
}

impl Default for SearchSettings {
    fn default() -> Self {
        Self {
            url: "http://127.0.0.1:8888".into(),
            language: "fr-FR".into(),
            safesearch: 1,
            max_results: 5,
            summary_sources: 3,
            snippet_chars: 200,
            timeout_ms: 4000,
            summarize: true,
            local_summary_timeout_ms: 10_000,
            online_summary_timeout_ms: 5000,
            summary_max_tokens: 80,
            local_summary_model: String::new(),
            video_engines: vec!["youtube".into()],
            ytdlp: true,
            ytdlp_timeout_s: 8,
        }
    }
}

/// Outils de la recherche, proposés à l'IA et aux raccourcis.
pub fn tool_specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "web_search".into(),
            description: "Cherche sur internet et résume la réponse à voix haute \
                          (actualité, météo, horaires, faits récents)."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Ce qu'il faut chercher (ex. météo de demain à Lomé)."
                    }
                },
                "required": ["query"]
            }),
            risk: Risk::Direct,
        },
        ToolSpec {
            name: "open_results".into(),
            description: "Ouvre dans le navigateur les résultats de la dernière recherche, \
                          ou le résultat numéro index."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "index": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Numéro du résultat (1 pour le premier)."
                    }
                }
            }),
            risk: Risk::Direct,
        },
    ]
}

/// Compétence de recherche : `web_search` et `open_results`.
pub struct SearchSkill {
    client: Arc<SearxClient>,
    summarizer: Arc<dyn Summarizer>,
    runner: Arc<dyn CommandRunner>,
    settings: SearchSettings,
    /// Dernière recherche, en mémoire seulement.
    last: Mutex<Option<LastResults>>,
}

/// Dernière recherche, pour « ouvre les résultats ».
struct LastResults {
    query: String,
    hits: Vec<SearchHit>,
}

// Phrases neutres : la personnalité est ajoutée par le cerveau.
const UNREACHABLE: &str = "Le moteur de recherche ne répond pas.";
const JSON_DISABLED: &str = "Le moteur de recherche refuse les demandes de Jarvis.";
const NO_SEARCH_YET: &str = "Je n'ai pas encore fait de recherche.";
const NO_BROWSER: &str = "Je ne peux pas ouvrir le navigateur.";
/// Longueur maximale d'une réponse dite.
const SPOKEN_CHARS: usize = 300;

/// Phrase quand le numéro demandé dépasse les `n` résultats.
fn too_few(n: usize) -> String {
    match n {
        0 => "Il n'y a aucun résultat.".into(),
        1 => "Il n'y a qu'un résultat.".into(),
        n => format!("Il n'y a que {n} résultats."),
    }
}

/// Numéro de résultat : entier ≥ 1 ou chaîne de chiffres. Absent : `None`.
fn parse_index(value: Option<&Value>) -> Result<Option<usize>, SkillError> {
    let n = match value {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::Number(n)) => n.as_u64().and_then(|n| usize::try_from(n).ok()),
        Some(Value::String(s)) => {
            let s = s.trim();
            if !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()) {
                s.parse().ok()
            } else {
                None
            }
        }
        Some(_) => None,
    };
    match n {
        Some(n) if n >= 1 => Ok(Some(n)),
        _ => Err(SkillError::BadArgs(format!("index invalide : {value:?}"))),
    }
}

impl SearchSkill {
    pub fn new(
        client: Arc<SearxClient>,
        summarizer: Arc<dyn Summarizer>,
        runner: Arc<dyn CommandRunner>,
        settings: SearchSettings,
    ) -> Self {
        Self {
            client,
            summarizer,
            runner,
            settings,
            last: Mutex::new(None),
        }
    }

    fn last(&self) -> std::sync::MutexGuard<'_, Option<LastResults>> {
        self.last.lock().unwrap_or_else(|e| e.into_inner())
    }

    async fn web_search(&self, args: &Value) -> Result<Reply, SkillError> {
        let query = args
            .get("query")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default();
        if query.is_empty() {
            return Err(SkillError::BadArgs("requête vide".into()));
        }
        let started = Instant::now();
        debug!(requete = %query, "recherche");
        let page = match self.client.search(query).await {
            Ok(page) => page,
            Err(SearxError::Unreachable(e)) => {
                warn!(erreur = %e, "SearXNG injoignable ; le lancer : jarvisctl searx up");
                return Ok(Reply::failed(UNREACHABLE));
            }
            Err(SearxError::JsonDisabled) => {
                warn!("SearXNG refuse le format JSON (403)");
                return Ok(Reply::failed(JSON_DISABLED));
            }
            Err(e @ SearxError::BadResponse(_)) => {
                debug!(erreur = %e, "réponse de SearXNG illisible");
                return Err(SkillError::Failed(
                    "réponse inattendue du moteur de recherche".into(),
                ));
            }
        };
        for (i, hit) in page.hits.iter().enumerate() {
            debug!(rang = i + 1, titre = %hit.title, url = %hit.url, "résultat");
        }
        *self.last() = Some(LastResults {
            query: query.to_string(),
            hits: page.hits.clone(),
        });
        let duree_ms = || started.elapsed().as_millis() as u64;
        let Some((speech, path)) = self.answer(query, &page).await else {
            info!(
                resultats = 0,
                duree_ms = duree_ms(),
                "recherche sans résultat"
            );
            return Ok(Reply::new(format!("Je n'ai rien trouvé pour « {query} ».")));
        };
        info!(
            resultats = page.hits.len(),
            duree_ms = duree_ms(),
            resume = path,
            "recherche faite"
        );
        debug!(reponse = %speech, "réponse de la recherche");
        Ok(Reply::new(speech))
    }

    /// Réponse dite et chemin suivi (`ia` ou `extrait`). `None` : page vide.
    async fn answer(&self, query: &str, page: &SearchPage) -> Option<(String, &'static str)> {
        let extract = extract::extract_answer(page)?;
        if self.settings.summarize {
            let budget = self.summarizer.budget();
            let sources = self.sources(page);
            let summary = self.summarizer.summarize(query, &sources);
            match tokio::time::timeout(budget, summary).await {
                Ok(Ok(text)) => {
                    let spoken = spoken::for_voice(&text, SPOKEN_CHARS);
                    if !spoken.is_empty() {
                        return Some((spoken, "ia"));
                    }
                    warn!("résumé vide, repli sur l'extrait");
                }
                Ok(Err(e)) => {
                    warn!("résumé en échec, repli sur l'extrait");
                    debug!(erreur = %e, "échec du résumé");
                }
                Err(_) => warn!(
                    budget_ms = budget.as_millis() as u64,
                    "résumé trop lent, repli sur l'extrait"
                ),
            }
        }
        Some((spoken::for_voice(&extract, SPOKEN_CHARS), "extrait"))
    }

    /// Sources du résumé : réponse directe ou encadré, puis les premiers résultats.
    fn sources(&self, page: &SearchPage) -> Vec<Source> {
        let cut = |text: &str| extract::cut_on_word(text, self.settings.snippet_chars);
        let direct = [
            ("Réponse directe", &page.answer),
            ("Encadré", &page.infobox),
        ]
        .into_iter()
        .find_map(|(title, text)| {
            let text = text.as_deref().map(str::trim).filter(|t| !t.is_empty())?;
            Some(Source {
                site: "searxng".into(),
                title: title.into(),
                snippet: cut(text),
            })
        });
        let hits = page
            .hits
            .iter()
            .take(self.settings.summary_sources)
            .map(|h| Source {
                site: h.site.clone(),
                title: h.title.clone(),
                snippet: cut(&h.snippet),
            });
        direct.into_iter().chain(hits).collect()
    }

    fn open_results(&self, args: &Value) -> Result<Reply, SkillError> {
        let index = parse_index(args.get("index"))?;
        let (url, speech) = {
            let last = self.last();
            let Some(last) = last.as_ref() else {
                return Ok(Reply::failed(NO_SEARCH_YET));
            };
            match index {
                None => (
                    self.client.results_page_url(&last.query),
                    "J'ouvre les résultats.".to_string(),
                ),
                Some(n) => {
                    let Some(hit) = last.hits.get(n - 1) else {
                        return Ok(Reply::failed(too_few(last.hits.len())));
                    };
                    let url = checked_url(&hit.url)
                        .ok_or_else(|| SkillError::Failed("adresse de résultat refusée".into()))?;
                    (url, format!("J'ouvre le résultat {n}."))
                }
            }
        };
        debug!(url = %url, "ouverture dans le navigateur");
        match open_url(self.runner.as_ref(), &url) {
            Ok(()) => Ok(Reply::done(speech)),
            Err(e) => {
                warn!(erreur = %e, "xdg-open impossible");
                Ok(Reply::failed(NO_BROWSER))
            }
        }
    }
}

#[async_trait::async_trait]
impl Skill for SearchSkill {
    fn tools(&self) -> Vec<ToolSpec> {
        tool_specs()
    }

    /// La recherche a le délai de SearXNG, plus celui du résumé, plus 1 s.
    fn timeout(&self, tool: &str) -> Option<Duration> {
        (tool == "web_search").then(|| {
            Duration::from_millis(self.settings.timeout_ms)
                + self.summarizer.budget()
                + Duration::from_secs(1)
        })
    }

    async fn call(&self, tool: &str, args: Value) -> Result<Reply, SkillError> {
        match tool {
            "web_search" => self.web_search(&args).await,
            "open_results" => self.open_results(&args),
            other => Err(SkillError::UnknownTool(other.into())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FakeRunner, Outcome};
    use serde_json::json;
    use std::time::Duration;
    use wiremock::matchers::{body_string_contains, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Environ trois cents caractères, sans fin de phrase avant la fin.
    fn long_snippet(i: usize) -> String {
        format!("Extrait {i} : ") + &"mot ".repeat(80) + "fin."
    }

    fn results(n: usize) -> serde_json::Value {
        let r: Vec<_> = (1..=n)
            .map(|i| {
                let content = if i == 1 {
                    "Demain, 31°C au soleil. Vent faible.".to_string()
                } else {
                    long_snippet(i)
                };
                json!({
                    "url": format!("https://site{i}.fr/page"),
                    "title": format!("Titre {i}"),
                    "content": content,
                })
            })
            .collect();
        json!({ "results": r })
    }

    async fn searx(body: serde_json::Value) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/search"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&server)
            .await;
        server
    }

    struct Banc {
        skill: SearchSkill,
        summarizer: Arc<FakeSummarizer>,
        runner: Arc<FakeRunner>,
    }

    fn banc(url: &str, summarizer: FakeSummarizer, s: SearchSettings) -> Banc {
        let summarizer = Arc::new(summarizer);
        let runner = Arc::new(FakeRunner::new());
        let s = SearchSettings {
            url: url.into(),
            ..s
        };
        let client = Arc::new(SearxClient::new(&s).unwrap());
        let skill = SearchSkill::new(client, summarizer.clone(), runner.clone(), s);
        Banc {
            skill,
            summarizer,
            runner,
        }
    }

    fn defaut(url: &str, summarizer: FakeSummarizer) -> Banc {
        banc(url, summarizer, SearchSettings::default())
    }

    async fn search(b: &Banc, q: &str) -> Reply {
        b.skill
            .call("web_search", json!({ "query": q }))
            .await
            .unwrap()
    }

    async fn open(b: &Banc, args: serde_json::Value) -> Reply {
        b.skill.call("open_results", args).await.unwrap()
    }

    const EXTRAIT: &str = "D'après site1 : Demain, 31 degrés au soleil.";

    #[tokio::test]
    async fn recherche_resume_par_l_ia() {
        let server = searx(results(5)).await;
        let b = defaut(
            &server.uri(),
            FakeSummarizer::ok("Demain, il fera beau à Lomé."),
        );
        let reply = search(&b, "  météo de demain ").await;
        assert_eq!(reply, Reply::new("Demain, il fera beau à Lomé."));
        assert_eq!(b.summarizer.calls(), 1);
        assert_eq!(
            b.summarizer.last_query().as_deref(),
            Some("météo de demain")
        );
        let sources = b.summarizer.last_sources().unwrap();
        assert_eq!(sources.len(), 3);
        assert_eq!(
            sources[0],
            Source {
                site: "site1".into(),
                title: "Titre 1".into(),
                snippet: "Demain, 31°C au soleil. Vent faible.".into(),
            }
        );
        for s in &sources[1..] {
            let n = s.snippet.chars().count();
            assert!(n <= 200 && n > 150, "{n} : {}", s.snippet);
            // Coupé sur un mot.
            assert!(s.snippet.ends_with(" mot…"), "{}", s.snippet);
        }
    }

    #[tokio::test]
    async fn resume_en_echec_repli_sur_l_extrait() {
        let server = searx(results(2)).await;
        let b = defaut(&server.uri(), FakeSummarizer::failing());
        assert_eq!(search(&b, "météo").await, Reply::new(EXTRAIT));
        assert_eq!(b.summarizer.calls(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn resume_trop_lent_repli_sur_l_extrait() {
        // Sans réseau : la page est donnée directement, le temps est simulé.
        let b = defaut(
            "http://127.0.0.1:9",
            FakeSummarizer::ok("trop tard")
                .with_delay(Duration::from_secs(60))
                .with_budget(Duration::from_secs(10)),
        );
        let page = SearchPage {
            hits: vec![SearchHit {
                title: "Titre 1".into(),
                url: "https://site1.fr/".into(),
                snippet: "Demain, 31°C au soleil. Vent faible.".into(),
                site: "site1".into(),
            }],
            ..SearchPage::default()
        };
        let started = tokio::time::Instant::now();
        let (speech, path) = b.skill.answer("météo", &page).await.unwrap();
        assert_eq!((speech.as_str(), path), (EXTRAIT, "extrait"));
        assert_eq!(b.summarizer.calls(), 1);
        let waited = started.elapsed();
        assert!(
            waited >= Duration::from_secs(10) && waited < Duration::from_secs(11),
            "{waited:?}"
        );
    }

    #[tokio::test]
    async fn resume_desactive_extrait_sans_ia() {
        let server = searx(results(3)).await;
        let s = SearchSettings {
            summarize: false,
            ..SearchSettings::default()
        };
        let b = banc(&server.uri(), FakeSummarizer::ok("jamais dit"), s);
        assert_eq!(search(&b, "météo").await, Reply::new(EXTRAIT));
        assert_eq!(b.summarizer.calls(), 0);
    }

    #[tokio::test]
    async fn reponse_directe_prioritaire() {
        let mut body = results(4);
        body["answers"] = json!([{"answer": "1 euro = 655,957 francs CFA"}]);
        body["infoboxes"] =
            json!([{"content": "Le franc CFA est une monnaie. Il a cours au Togo."}]);
        let server = searx(body).await;
        let s = SearchSettings {
            summarize: false,
            ..SearchSettings::default()
        };
        let b = banc(&server.uri(), FakeSummarizer::ok("x"), s);
        assert_eq!(
            search(&b, "euro en cfa").await,
            Reply::new("1 euro = 655,957 francs CFA")
        );
        // Avec l'IA : la réponse directe est la première source.
        let b = defaut(
            &server.uri(),
            FakeSummarizer::ok("Un euro vaut 656 francs."),
        );
        search(&b, "euro en cfa").await;
        let sources = b.summarizer.last_sources().unwrap();
        assert_eq!(sources.len(), 4);
        assert_eq!(sources[0].snippet, "1 euro = 655,957 francs CFA");
        assert_eq!(sources[1].site, "site1");
    }

    #[tokio::test]
    async fn aucun_resultat_phrase_claire() {
        let server = searx(json!({"results": []})).await;
        let b = defaut(&server.uri(), FakeSummarizer::ok("x"));
        let reply = search(&b, "xyzzy plugh").await;
        assert_eq!(
            reply,
            Reply::new("Je n'ai rien trouvé pour « xyzzy plugh ».")
        );
        assert_eq!(reply.outcome, Outcome::Info);
        assert_eq!(b.summarizer.calls(), 0);
    }

    #[tokio::test]
    async fn moteur_injoignable_phrase_claire() {
        // Port fermé : aucune vraie requête ne sort du PC.
        let b = defaut("http://127.0.0.1:9", FakeSummarizer::ok("x"));
        assert_eq!(
            search(&b, "météo").await,
            Reply::failed("Le moteur de recherche ne répond pas.")
        );
        assert_eq!(b.summarizer.calls(), 0);
    }

    #[tokio::test]
    async fn json_desactive_phrase_claire() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(403))
            .mount(&server)
            .await;
        let b = defaut(&server.uri(), FakeSummarizer::ok("x"));
        assert_eq!(
            search(&b, "météo").await,
            Reply::failed("Le moteur de recherche refuse les demandes de Jarvis.")
        );
    }

    #[tokio::test]
    async fn requete_vide_mauvais_arguments() {
        let server = searx(results(1)).await;
        let b = defaut(&server.uri(), FakeSummarizer::ok("x"));
        for args in [
            json!({"query": ""}),
            json!({"query": "   "}),
            json!({}),
            json!({"query": 3}),
        ] {
            let err = b.skill.call("web_search", args.clone()).await.unwrap_err();
            assert!(matches!(err, SkillError::BadArgs(_)), "{args} : {err:?}");
        }
        assert!(server.received_requests().await.unwrap().is_empty());
        let err = b.skill.call("autre", json!({})).await.unwrap_err();
        assert!(matches!(err, SkillError::UnknownTool(_)), "{err:?}");
    }

    #[tokio::test]
    async fn resume_nettoye_pour_la_voix() {
        let server = searx(results(2)).await;
        let b = defaut(
            &server.uri(),
            FakeSummarizer::ok("**Demain** 🌞 il fera 31°C. Voir https://meteo.fr/lome"),
        );
        assert_eq!(
            search(&b, "météo").await,
            Reply::new("Demain il fera 31 degrés. Voir")
        );
        // Long : coupé à la phrase, 300 caractères au plus.
        let b = defaut(
            &server.uri(),
            FakeSummarizer::ok("Il fera beau demain. ".repeat(30)),
        );
        let speech = search(&b, "météo").await.speech;
        assert!(
            speech.chars().count() <= 300 && speech.ends_with("demain."),
            "{speech}"
        );
        // Rien à dire après nettoyage : repli sur l'extrait.
        let b = defaut(&server.uri(), FakeSummarizer::ok("🌞 ** "));
        assert_eq!(search(&b, "météo").await, Reply::new(EXTRAIT));
    }

    #[tokio::test]
    async fn ouvre_les_resultats_page_searxng() {
        let server = searx(results(3)).await;
        let b = defaut(&server.uri(), FakeSummarizer::ok("x"));
        search(&b, "météo").await;
        let done = Reply::done("J'ouvre les résultats.");
        assert_eq!(open(&b, json!({})).await, done);
        assert_eq!(open(&b, json!({"index": null})).await, done);
        let page = format!("{}/search?q=m%C3%A9t%C3%A9o", server.uri());
        assert_eq!(
            b.runner.calls(),
            vec![
                ("xdg-open".to_string(), vec![page.clone()]),
                ("xdg-open".to_string(), vec![page]),
            ]
        );
    }

    #[tokio::test]
    async fn ouvre_le_deuxieme_resultat() {
        let server = searx(results(3)).await;
        let b = defaut(&server.uri(), FakeSummarizer::ok("x"));
        search(&b, "météo").await;
        assert_eq!(
            open(&b, json!({"index": 2})).await,
            Reply::done("J'ouvre le résultat 2.")
        );
        assert_eq!(
            b.runner.calls(),
            vec![(
                "xdg-open".to_string(),
                vec!["https://site2.fr/page".to_string()]
            )]
        );
    }

    #[tokio::test]
    async fn ouvre_resultat_index_en_chaine() {
        let server = searx(results(3)).await;
        let b = defaut(&server.uri(), FakeSummarizer::ok("x"));
        search(&b, "météo").await;
        assert_eq!(
            open(&b, json!({"index": " 3 "})).await,
            Reply::done("J'ouvre le résultat 3.")
        );
        assert_eq!(
            b.runner.calls(),
            vec![(
                "xdg-open".to_string(),
                vec!["https://site3.fr/page".to_string()]
            )]
        );
        for bad in [
            json!("deux"),
            json!(""),
            json!(0),
            json!("0"),
            json!(-1),
            json!(1.5),
        ] {
            let args = json!({ "index": bad });
            let err = b.skill.call("open_results", args).await.unwrap_err();
            assert!(matches!(err, SkillError::BadArgs(_)), "{bad} : {err:?}");
        }
        assert_eq!(b.runner.calls().len(), 1);
    }

    #[tokio::test]
    async fn ouvre_resultat_hors_limite() {
        let server = searx(results(5)).await;
        let b = defaut(&server.uri(), FakeSummarizer::ok("x"));
        search(&b, "météo").await;
        assert_eq!(
            open(&b, json!({"index": 6})).await,
            Reply::failed("Il n'y a que 5 résultats.")
        );
        assert!(b.runner.calls().is_empty());
    }

    #[tokio::test]
    async fn ouvre_les_resultats_sans_recherche() {
        let b = defaut("http://127.0.0.1:9", FakeSummarizer::ok("x"));
        for args in [json!({}), json!({"index": 1})] {
            assert_eq!(
                open(&b, args).await,
                Reply::failed("Je n'ai pas encore fait de recherche.")
            );
        }
        assert!(b.runner.calls().is_empty());
    }

    /// Navigateur absent : `xdg-open` introuvable.
    struct SansNavigateur;

    impl CommandRunner for SansNavigateur {
        fn run(&self, _: &str, _: &[String]) -> std::io::Result<crate::CommandOutput> {
            Err(std::io::ErrorKind::NotFound.into())
        }

        fn spawn_detached(&self, _: &str, _: &[String]) -> std::io::Result<()> {
            Err(std::io::ErrorKind::NotFound.into())
        }
    }

    #[tokio::test]
    async fn ouvre_les_resultats_navigateur_absent() {
        let server = searx(results(2)).await;
        let s = SearchSettings {
            url: server.uri(),
            ..SearchSettings::default()
        };
        let skill = SearchSkill::new(
            Arc::new(SearxClient::new(&s).unwrap()),
            Arc::new(FakeSummarizer::ok("x")),
            Arc::new(SansNavigateur),
            s,
        );
        skill
            .call("web_search", json!({"query": "météo"}))
            .await
            .unwrap();
        for args in [json!({}), json!({"index": 1})] {
            assert_eq!(
                skill.call("open_results", args).await.unwrap(),
                Reply::failed("Je ne peux pas ouvrir le navigateur.")
            );
        }
    }

    #[tokio::test]
    async fn nouvelle_recherche_remplace_les_resultats() {
        let server = MockServer::start().await;
        for animal in ["chats", "chiens"] {
            let body = json!({
                "results": [{"url": format!("https://{animal}.fr/"), "title": animal}]
            });
            Mock::given(method("POST"))
                .and(body_string_contains(format!("q={animal}&")))
                .respond_with(ResponseTemplate::new(200).set_body_json(body))
                .mount(&server)
                .await;
        }
        let b = defaut(&server.uri(), FakeSummarizer::ok("x"));
        search(&b, "chats").await;
        search(&b, "chiens").await;
        open(&b, json!({"index": 1})).await;
        open(&b, json!({})).await;
        let calls = b.runner.calls();
        assert_eq!(calls[0].1, ["https://chiens.fr/"]);
        assert_eq!(calls[1].1, [format!("{}/search?q=chiens", server.uri())]);
        assert_eq!(
            open(&b, json!({"index": 2})).await,
            Reply::failed("Il n'y a qu'un résultat.")
        );
    }

    #[tokio::test]
    async fn delai_de_la_competence_couvre_recherche_et_resume() {
        let budget = |secs| FakeSummarizer::ok("x").with_budget(Duration::from_secs(secs));
        let b = defaut("http://127.0.0.1:9", budget(10));
        assert_eq!(
            b.skill.timeout("web_search"),
            Some(Duration::from_secs(4 + 10 + 1))
        );
        assert_eq!(b.skill.timeout("open_results"), None);
        let b = defaut("http://127.0.0.1:9", budget(5));
        assert_eq!(
            b.skill.timeout("web_search"),
            Some(Duration::from_secs(4 + 5 + 1))
        );
        // Recherche lente et résumé lent, chacun sous son délai : la réponse de
        // l'IA arrive dans le délai de la compétence.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(results(2))
                    .set_delay(Duration::from_millis(150)),
            )
            .mount(&server)
            .await;
        let s = SearchSettings {
            timeout_ms: 300,
            ..SearchSettings::default()
        };
        let b = banc(
            &server.uri(),
            FakeSummarizer::ok("Il fera beau.")
                .with_delay(Duration::from_millis(150))
                .with_budget(Duration::from_millis(300)),
            s,
        );
        let limit = b.skill.timeout("web_search").unwrap();
        assert_eq!(limit, Duration::from_millis(300 + 300 + 1000));
        let reply = tokio::time::timeout(limit, search(&b, "météo"))
            .await
            .unwrap();
        assert_eq!(reply, Reply::new("Il fera beau."));
    }

    #[test]
    fn outils_de_recherche_directs() {
        let specs = tool_specs();
        let names: Vec<_> = specs.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["web_search", "open_results"]);
        assert!(specs.iter().all(|t| t.risk == Risk::Direct));
        assert_eq!(
            specs[0].parameters["required"],
            serde_json::json!(["query"])
        );
        assert_eq!(specs[1].parameters["properties"]["index"]["minimum"], 1);
    }

    #[test]
    fn reglages_par_defaut() {
        let s: SearchSettings = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(s, SearchSettings::default());
        assert_eq!(s.url, "http://127.0.0.1:8888");
        let s: SearchSettings =
            serde_json::from_value(serde_json::json!({"summarize": false})).unwrap();
        assert!(!s.summarize);
        assert_eq!(s.max_results, 5);
    }
}
