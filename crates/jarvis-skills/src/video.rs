//! « Lance la vidéo de… » : trouve une vidéo et la lance dans le navigateur.

use crate::browser::{checked_url, open_url};
use crate::media::{MediaPlayers, PlaybackStatus, PlayerCommand};
use crate::search::spoken::for_voice;
use crate::search::{SearchSettings, SearxClient};
use crate::{CommandRunner, Reply, Risk, Skill, SkillError, ToolSpec};
use reqwest::Url;
use std::sync::Arc;
use std::time::Duration;

/// Outil de la vidéo, proposé à l'IA et aux raccourcis.
pub fn tool_specs() -> Vec<ToolSpec> {
    vec![ToolSpec {
        name: "play_video".into(),
        description: "Trouve une vidéo (YouTube) et la lance dans le navigateur.".into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "La vidéo cherchée (ex. Daft Punk, bande-annonce de Dune)."
                }
            },
            "required": ["query"]
        }),
        risk: Risk::Direct,
    }]
}

/// Compétence vidéo : `play_video`.
pub struct VideoSkill {
    client: Arc<SearxClient>,
    players: Arc<dyn MediaPlayers>,
    runner: Arc<dyn CommandRunner>,
    settings: SearchSettings,
}

impl VideoSkill {
    pub fn new(
        client: Arc<SearxClient>,
        players: Arc<dyn MediaPlayers>,
        runner: Arc<dyn CommandRunner>,
        settings: SearchSettings,
    ) -> Self {
        Self {
            client,
            players,
            runner,
            settings,
        }
    }

    async fn play(&self, query: &str) -> Reply {
        let found = match self.find_on_searx(query).await {
            Some(video) => Some(video),
            None if self.settings.ytdlp => self.find_with_ytdlp(query).await,
            None => None,
        };
        let Some((url, title)) = found else {
            tracing::info!("aucune vidéo trouvée");
            return Reply::failed(format!("Je n'ai pas trouvé de vidéo pour « {query} »."));
        };
        tracing::debug!(%url, %title, "vidéo trouvée");
        self.pause_music().await;
        if let Err(e) = open_url(self.runner.as_ref(), &url) {
            tracing::warn!("xdg-open a échoué : {e}");
            return Reply::failed("Je ne peux pas ouvrir le navigateur.");
        }
        let title = for_voice(&title, 80);
        if title.is_empty() {
            Reply::done("Je lance la vidéo.")
        } else {
            Reply::done(format!("Je lance « {title} »."))
        }
    }

    /// Première vidéo de SearXNG à adresse sûre.
    async fn find_on_searx(&self, query: &str) -> Option<(Url, String)> {
        match self.client.videos(query).await {
            Ok(hits) => hits
                .into_iter()
                .find_map(|h| checked_url(&h.url).map(|u| (u, h.title))),
            Err(e) => {
                tracing::warn!("vidéos de SearXNG indisponibles : {e}");
                None
            }
        }
    }

    /// Secours : `yt-dlp`, borné par `timeout` et par tokio. Absent ou en échec = rien.
    async fn find_with_ytdlp(&self, query: &str) -> Option<(Url, String)> {
        let secs = self.settings.ytdlp_timeout_s;
        let args: Vec<String> = [
            &secs.to_string(),
            "yt-dlp",
            "--ignore-config",
            "--flat-playlist",
            "--no-warnings",
            "--print",
            "%(webpage_url)s\t%(title)s",
            &format!("ytsearch1:{query}"),
        ]
        .map(String::from)
        .to_vec();
        let runner = self.runner.clone();
        let task = tokio::task::spawn_blocking(move || runner.run("timeout", &args));
        let out = match tokio::time::timeout(Duration::from_secs(secs + 1), task).await {
            Ok(Ok(Ok(out))) if out.success => out,
            Ok(Ok(Ok(_))) => {
                tracing::info!("yt-dlp n'a rien trouvé");
                return None;
            }
            Ok(Ok(Err(e))) => {
                tracing::warn!("yt-dlp indisponible : {e}");
                return None;
            }
            Ok(Err(e)) => {
                tracing::warn!("yt-dlp interrompu : {e}");
                return None;
            }
            Err(_) => {
                tracing::warn!("yt-dlp trop lent");
                return None;
            }
        };
        let line = out.stdout.lines().next()?;
        let (raw, title) = line.split_once('\t').unwrap_or((line, ""));
        let url = checked_url(raw);
        if url.is_none() {
            tracing::warn!("adresse de yt-dlp refusée");
        }
        Some((url?, title.trim().to_string()))
    }

    /// Met en pause les lecteurs MPRIS en lecture ; un échec est seulement noté.
    async fn pause_music(&self) {
        let list = match self.players.players().await {
            Ok(list) => list,
            Err(e) => {
                tracing::warn!("lecteurs multimédia : {e:#}");
                return;
            }
        };
        for p in list.iter().filter(|p| p.status == PlaybackStatus::Playing) {
            if let Err(e) = self
                .players
                .command(&p.bus_name, PlayerCommand::Pause)
                .await
            {
                tracing::warn!(player = %p.bus_name, "pause refusée : {e:#}");
            }
        }
    }
}

#[async_trait::async_trait]
impl Skill for VideoSkill {
    fn tools(&self) -> Vec<ToolSpec> {
        tool_specs()
    }

    fn timeout(&self, tool: &str) -> Option<Duration> {
        (tool == "play_video").then(|| {
            Duration::from_millis(self.settings.timeout_ms)
                + Duration::from_secs(self.settings.ytdlp_timeout_s + 2)
        })
    }

    async fn call(&self, tool: &str, args: serde_json::Value) -> Result<Reply, SkillError> {
        if tool != "play_video" {
            return Err(SkillError::UnknownTool(tool.into()));
        }
        let query = args
            .get("query")
            .and_then(|q| q.as_str())
            .map(str::trim)
            .filter(|q| !q.is_empty())
            .ok_or_else(|| SkillError::BadArgs("query manquant".into()))?;
        Ok(self.play(query).await)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::PlayerInfo;
    use crate::{CommandOutput, FakeRunner, Outcome};
    use serde_json::json;
    use std::sync::Mutex;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Faux lecteurs : note chaque commande et si le navigateur était déjà ouvert.
    struct FakePlayers {
        list: Vec<PlayerInfo>,
        runner: Arc<FakeRunner>,
        sent: Mutex<Vec<(String, PlayerCommand, bool)>>,
    }

    #[async_trait::async_trait]
    impl MediaPlayers for FakePlayers {
        async fn players(&self) -> anyhow::Result<Vec<PlayerInfo>> {
            Ok(self.list.clone())
        }
        async fn command(&self, bus_name: &str, cmd: PlayerCommand) -> anyhow::Result<()> {
            let opened = self.runner.calls().iter().any(|(p, _)| p == "xdg-open");
            self.sent
                .lock()
                .unwrap()
                .push((bus_name.to_string(), cmd, opened));
            Ok(())
        }
    }

    fn player(name: &str, status: PlaybackStatus) -> PlayerInfo {
        PlayerInfo {
            bus_name: format!("org.mpris.MediaPlayer2.{name}"),
            identity: name.to_string(),
            status,
            title: None,
            artist: None,
        }
    }

    struct Setup {
        skill: VideoSkill,
        players: Arc<FakePlayers>,
        runner: Arc<FakeRunner>,
    }

    fn setup(settings: SearchSettings, list: Vec<PlayerInfo>) -> Setup {
        let runner = Arc::new(FakeRunner::new());
        let players = Arc::new(FakePlayers {
            list,
            runner: runner.clone(),
            sent: Mutex::new(Vec::new()),
        });
        let client = Arc::new(SearxClient::new(&settings).unwrap());
        let skill = VideoSkill::new(client, players.clone(), runner.clone(), settings);
        Setup {
            skill,
            players,
            runner,
        }
    }

    fn settings(server: &MockServer) -> SearchSettings {
        SearchSettings {
            url: server.uri(),
            ..SearchSettings::default()
        }
    }

    /// SearXNG qui répond par ces vidéos.
    async fn searx(results: serde_json::Value) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/search"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"results": results})))
            .mount(&server)
            .await;
        server
    }

    fn ytdlp_prints(runner: &FakeRunner, success: bool, stdout: &str) {
        runner.respond(
            "timeout",
            CommandOutput {
                success,
                stdout: stdout.to_string(),
            },
        );
    }

    fn ytdlp_call(q: &str) -> (String, Vec<String>) {
        let search = format!("ytsearch1:{q}");
        (
            "timeout".to_string(),
            [
                "8",
                "yt-dlp",
                "--ignore-config",
                "--flat-playlist",
                "--no-warnings",
                "--print",
                "%(webpage_url)s\t%(title)s",
                &search,
            ]
            .map(String::from)
            .to_vec(),
        )
    }

    fn xdg_open(url: &str) -> (String, Vec<String>) {
        ("xdg-open".to_string(), vec![url.to_string()])
    }

    async fn play(skill: &VideoSkill, q: &str) -> Reply {
        skill
            .call("play_video", json!({ "query": q }))
            .await
            .unwrap()
    }

    #[test]
    fn outil_video_direct() {
        let specs = tool_specs();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].name, "play_video");
        assert_eq!(specs[0].risk, Risk::Direct);
        assert_eq!(
            specs[0].parameters["required"],
            serde_json::json!(["query"])
        );
    }

    #[tokio::test]
    async fn video_trouvee_par_searxng_ouverte_dans_le_navigateur() {
        let server = searx(json!([
            {"url": "https://www.youtube.com/watch?v=abc", "title": "Daft Punk - One More Time"},
            {"url": "https://www.youtube.com/watch?v=def", "title": "Autre"}
        ]))
        .await;
        let s = setup(settings(&server), vec![]);
        let r = play(&s.skill, "daft punk").await;
        assert_eq!(r.outcome, Outcome::Done);
        assert_eq!(r.speech, "Je lance « Daft Punk - One More Time ».");
        assert_eq!(
            s.runner.calls(),
            vec![xdg_open("https://www.youtube.com/watch?v=abc")]
        );
    }

    #[tokio::test]
    async fn video_met_en_pause_la_musique_avant() {
        let server = searx(json!([
            {"url": "https://www.youtube.com/watch?v=abc", "title": "Clip"}
        ]))
        .await;
        let s = setup(
            settings(&server),
            vec![
                player("vlc", PlaybackStatus::Paused),
                player("spotify", PlaybackStatus::Playing),
            ],
        );
        let r = play(&s.skill, "clip").await;
        assert_eq!(r.outcome, Outcome::Done);
        // Seul le lecteur en lecture est mis en pause, avant le navigateur.
        assert_eq!(
            *s.players.sent.lock().unwrap(),
            vec![(
                "org.mpris.MediaPlayer2.spotify".to_string(),
                PlayerCommand::Pause,
                false
            )]
        );
        assert_eq!(
            s.runner.calls(),
            vec![xdg_open("https://www.youtube.com/watch?v=abc")]
        );
    }

    #[tokio::test]
    async fn video_repli_yt_dlp_si_searxng_vide() {
        let server = searx(json!([])).await;
        let s = setup(settings(&server), vec![]);
        ytdlp_prints(
            &s.runner,
            true,
            "https://www.youtube.com/watch?v=xyz\tBande-annonce de Dune\n\
             https://www.youtube.com/watch?v=zzz\tAutre\n",
        );
        let r = play(&s.skill, "bande-annonce de Dune").await;
        assert_eq!(r.outcome, Outcome::Done);
        assert_eq!(r.speech, "Je lance « Bande-annonce de Dune ».");
        assert_eq!(
            s.runner.calls(),
            vec![
                ytdlp_call("bande-annonce de Dune"),
                xdg_open("https://www.youtube.com/watch?v=xyz")
            ]
        );
    }

    #[tokio::test]
    async fn video_repli_yt_dlp_si_searxng_injoignable() {
        // Port fermé : aucune vraie requête ne sort du PC.
        let settings = SearchSettings {
            url: "http://127.0.0.1:9/".into(),
            ..SearchSettings::default()
        };
        let s = setup(settings, vec![]);
        ytdlp_prints(
            &s.runner,
            true,
            "https://www.youtube.com/watch?v=xyz\tClip\n",
        );
        let r = play(&s.skill, "clip").await;
        assert_eq!(r.outcome, Outcome::Done);
        assert_eq!(
            s.runner.calls(),
            vec![
                ytdlp_call("clip"),
                xdg_open("https://www.youtube.com/watch?v=xyz")
            ]
        );
    }

    #[tokio::test]
    async fn video_yt_dlp_desactive() {
        let server = searx(json!([])).await;
        let s = setup(
            SearchSettings {
                ytdlp: false,
                ..settings(&server)
            },
            vec![player("spotify", PlaybackStatus::Playing)],
        );
        let r = play(&s.skill, "clip").await;
        assert_eq!(r.outcome, Outcome::Failed);
        assert!(s.runner.calls().is_empty());
        assert!(s.players.sent.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn video_introuvable_phrase_claire() {
        let server = searx(json!([])).await;
        let s = setup(
            settings(&server),
            vec![player("spotify", PlaybackStatus::Playing)],
        );
        // `timeout` échoue si yt-dlp est absent (127) ou trop lent (124).
        ytdlp_prints(&s.runner, false, "");
        let r = play(&s.skill, "zzz introuvable").await;
        assert_eq!(r.outcome, Outcome::Failed);
        assert_eq!(
            r.speech,
            "Je n'ai pas trouvé de vidéo pour « zzz introuvable »."
        );
        assert_eq!(s.runner.calls(), vec![ytdlp_call("zzz introuvable")]);
        // La musique continue.
        assert!(s.players.sent.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn video_url_douteuse_refusee() {
        let server = searx(json!([])).await;
        for printed in [
            "file:///etc/passwd\tPiège\n",
            "javascript:alert(1)\tPiège\n",
            "-x\tPiège\n",
            "\n",
        ] {
            let s = setup(settings(&server), vec![]);
            ytdlp_prints(&s.runner, true, printed);
            let r = play(&s.skill, "piège").await;
            assert_eq!(r.outcome, Outcome::Failed, "{printed}");
            assert!(
                !s.runner.calls().iter().any(|(p, _)| p == "xdg-open"),
                "{printed}"
            );
        }
    }

    #[tokio::test]
    async fn titre_long_abrege_pour_la_voix() {
        let long = format!(
            "**Concert complet** {}https://exemple.fr/x 🎵",
            "très long titre de vidéo ".repeat(10)
        );
        let server = searx(json!([
            {"url": "https://www.youtube.com/watch?v=abc", "title": long}
        ]))
        .await;
        let s = setup(settings(&server), vec![]);
        let r = play(&s.skill, "concert").await;
        assert_eq!(r.outcome, Outcome::Done);
        let title = r
            .speech
            .strip_prefix("Je lance « ")
            .and_then(|t| t.strip_suffix(" »."))
            .unwrap();
        assert!(title.chars().count() <= 80, "{title}");
        assert!(title.starts_with("Concert complet très long"), "{title}");
        assert!(!title.contains('*') && !title.contains("http") && !title.contains('🎵'));
    }

    #[tokio::test]
    async fn requete_vide_mauvais_arguments() {
        let s = setup(SearchSettings::default(), vec![]);
        for args in [json!({}), json!({"query": "  "}), json!({"query": 3})] {
            let e = s.skill.call("play_video", args).await.unwrap_err();
            assert!(matches!(e, SkillError::BadArgs(_)));
        }
        let e = s.skill.call("autre", json!({})).await.unwrap_err();
        assert!(matches!(e, SkillError::UnknownTool(_)));
        assert!(s.runner.calls().is_empty());
    }

    #[test]
    fn delai_de_la_video() {
        let s = setup(SearchSettings::default(), vec![]);
        assert_eq!(
            s.skill.timeout("play_video"),
            Some(Duration::from_millis(4000 + 8000 + 2000))
        );
        assert_eq!(s.skill.timeout("autre"), None);
    }
}
