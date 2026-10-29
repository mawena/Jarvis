//! Media skill: MPRIS players over D-Bus and PipeWire volume.

pub mod mpris;

use std::sync::Arc;

use serde_json::json;

use crate::{CommandRunner, Reply, Risk, Skill, SkillError, ToolSpec};

pub use mpris::MprisPlayers;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackStatus {
    Playing,
    Paused,
    Stopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayerCommand {
    Next,
    Previous,
    Play,
    Pause,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerInfo {
    pub bus_name: String,
    pub identity: String,
    pub status: PlaybackStatus,
    pub title: Option<String>,
    pub artist: Option<String>,
}

/// Access to the MPRIS media players of the session.
#[async_trait::async_trait]
pub trait MediaPlayers: Send + Sync {
    async fn players(&self) -> anyhow::Result<Vec<PlayerInfo>>;
    async fn command(&self, bus_name: &str, cmd: PlayerCommand) -> anyhow::Result<()>;
}

pub struct MediaSkill {
    players: Arc<dyn MediaPlayers>,
    runner: Arc<dyn CommandRunner>,
}

const NO_PLAYER: &str = "Aucun lecteur n'est ouvert.";
const NO_ACCESSIBLE_PLAYER: &str =
    "Je ne vois aucun lecteur. Les vidéos de Firefox en snap me restent invisibles.";

/// X11 media key GNOME handles for each command (used when no MPRIS player
/// is reachable, e.g. a snap Firefox confined by AppArmor).
fn media_key(cmd: PlayerCommand) -> &'static str {
    match cmd {
        PlayerCommand::Play => "XF86AudioPlay",
        PlayerCommand::Pause => "XF86AudioPause",
        PlayerCommand::Next => "XF86AudioNext",
        PlayerCommand::Previous => "XF86AudioPrev",
    }
}

impl MediaSkill {
    pub fn new(players: Arc<dyn MediaPlayers>, runner: Arc<dyn CommandRunner>) -> Self {
        Self { players, runner }
    }

    /// First playing player, else first paused one, else the first of the list.
    async fn target(&self) -> Result<Option<PlayerInfo>, SkillError> {
        let list = self
            .players
            .players()
            .await
            .map_err(|e| SkillError::Failed(format!("lecteurs multimédia : {e}")))?;
        let pick = |status| list.iter().find(|p| p.status == status).cloned();
        Ok(pick(PlaybackStatus::Playing)
            .or_else(|| pick(PlaybackStatus::Paused))
            .or_else(|| list.first().cloned()))
    }

    /// Send the media key through xdotool; `NO_PLAYER` if that fails.
    fn send_media_key(&self, cmd: PlayerCommand) -> Reply {
        let args = ["key", media_key(cmd)].map(String::from);
        match self.runner.run("xdotool", &args) {
            Ok(out) if out.success => Reply::done(""),
            Ok(_) => {
                tracing::warn!("xdotool a échoué pour {}", media_key(cmd));
                Reply::failed(NO_PLAYER)
            }
            Err(e) => {
                tracing::warn!("xdotool indisponible : {e}");
                Reply::failed(NO_PLAYER)
            }
        }
    }

    async fn send(&self, cmd: PlayerCommand) -> Result<Reply, SkillError> {
        let Some(player) = self.target().await? else {
            return Ok(self.send_media_key(cmd));
        };
        let already = match cmd {
            PlayerCommand::Play => player.status == PlaybackStatus::Playing,
            PlayerCommand::Pause => player.status != PlaybackStatus::Playing,
            _ => false,
        };
        if already {
            return Ok(Reply::done(""));
        }
        if let Err(e) = self.players.command(&player.bus_name, cmd).await {
            tracing::warn!(player = %player.bus_name, "commande {cmd:?} refusée : {e:#}");
            let who = if player.identity.is_empty() {
                player
                    .bus_name
                    .trim_start_matches("org.mpris.MediaPlayer2.")
            } else {
                &player.identity
            };
            return Ok(Reply::failed(format!("{who} refuse la commande.")));
        }
        Ok(Reply::done(""))
    }

    async fn now_playing(&self) -> Result<Reply, SkillError> {
        let Some(p) = self.target().await? else {
            return Ok(Reply::new(NO_ACCESSIBLE_PLAYER));
        };
        Ok(Reply::new(match (p.title, p.artist) {
            (Some(title), Some(artist)) => format!("C'est {title}, par {artist}."),
            (Some(title), None) => format!("C'est {title}."),
            (None, _) => format!("Je ne sais pas ce que joue {}.", p.identity),
        }))
    }

    fn volume(&self, step: &str) -> Result<Reply, SkillError> {
        let args = ["set-volume", "-l", "1.0", "@DEFAULT_AUDIO_SINK@", step].map(String::from);
        let out = self
            .runner
            .run("wpctl", &args)
            .map_err(|e| SkillError::Failed(format!("wpctl : {e}")))?;
        if out.success {
            Ok(Reply::done(""))
        } else {
            Err(SkillError::Failed("wpctl a échoué".into()))
        }
    }
}

fn spec(name: &str, description: &str) -> ToolSpec {
    ToolSpec {
        name: name.into(),
        description: description.into(),
        parameters: json!({"type": "object", "properties": {}}),
        risk: Risk::Direct,
    }
}

#[async_trait::async_trait]
impl Skill for MediaSkill {
    fn tools(&self) -> Vec<ToolSpec> {
        vec![
            spec(
                "media_next",
                "Passe à la piste suivante du lecteur multimédia.",
            ),
            spec(
                "media_previous",
                "Revient à la piste précédente du lecteur multimédia.",
            ),
            spec(
                "media_play",
                "Lance ou reprend la lecture de la musique ou de la vidéo.",
            ),
            spec(
                "media_pause",
                "Met en pause la musique ou la vidéo en cours.",
            ),
            spec(
                "media_now_playing",
                "Dit quel titre et quel artiste sont en cours de lecture.",
            ),
            spec("volume_up", "Augmente le volume sonore de 10 %."),
            spec("volume_down", "Baisse le volume sonore de 10 %."),
        ]
    }

    async fn call(&self, tool: &str, _args: serde_json::Value) -> Result<Reply, SkillError> {
        match tool {
            "media_next" => self.send(PlayerCommand::Next).await,
            "media_previous" => self.send(PlayerCommand::Previous).await,
            "media_play" => self.send(PlayerCommand::Play).await,
            "media_pause" => self.send(PlayerCommand::Pause).await,
            "media_now_playing" => self.now_playing().await,
            "volume_up" => self.volume("10%+"),
            "volume_down" => self.volume("10%-"),
            other => Err(SkillError::UnknownTool(other.into())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FakeRunner, Skill, SkillError};
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct FakePlayers {
        list: Vec<PlayerInfo>,
        sent: Mutex<Vec<(String, PlayerCommand)>>,
    }

    #[async_trait::async_trait]
    impl MediaPlayers for FakePlayers {
        async fn players(&self) -> anyhow::Result<Vec<PlayerInfo>> {
            Ok(self.list.clone())
        }
        async fn command(&self, bus_name: &str, cmd: PlayerCommand) -> anyhow::Result<()> {
            self.sent.lock().unwrap().push((bus_name.to_string(), cmd));
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

    fn skill(list: Vec<PlayerInfo>) -> (MediaSkill, Arc<FakePlayers>, Arc<FakeRunner>) {
        let players = Arc::new(FakePlayers {
            list,
            ..Default::default()
        });
        let runner = Arc::new(FakeRunner::new());
        (
            MediaSkill::new(players.clone(), runner.clone()),
            players,
            runner,
        )
    }

    #[tokio::test]
    async fn cible_le_lecteur_en_lecture() {
        let (s, p, _) = skill(vec![
            player("vlc", PlaybackStatus::Paused),
            player("spotify", PlaybackStatus::Playing),
        ]);
        s.call("media_next", serde_json::json!({})).await.unwrap();
        assert_eq!(
            *p.sent.lock().unwrap(),
            vec![(
                "org.mpris.MediaPlayer2.spotify".to_string(),
                PlayerCommand::Next
            )]
        );
    }

    #[tokio::test]
    async fn sinon_le_lecteur_en_pause() {
        let (s, p, _) = skill(vec![
            player("vlc", PlaybackStatus::Stopped),
            player("mpv", PlaybackStatus::Paused),
        ]);
        s.call("media_play", serde_json::json!({})).await.unwrap();
        assert_eq!(
            *p.sent.lock().unwrap(),
            vec![(
                "org.mpris.MediaPlayer2.mpv".to_string(),
                PlayerCommand::Play
            )]
        );
    }

    #[tokio::test]
    async fn sans_lecteur_reponse_claire() {
        let (s, p, runner) = skill(vec![]);
        runner.respond(
            "xdotool",
            crate::CommandOutput {
                success: false,
                stdout: String::new(),
            },
        );
        let r = s.call("media_pause", serde_json::json!({})).await.unwrap();
        assert_eq!(r.outcome, crate::Outcome::Failed);
        assert_eq!(r.speech, "Aucun lecteur n'est ouvert.");
        assert!(p.sent.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn volume_appelle_wpctl() {
        let (s, _, runner) = skill(vec![]);
        let r = s.call("volume_up", serde_json::json!({})).await.unwrap();
        assert_eq!(r.outcome, crate::Outcome::Done);
        assert_eq!(r.speech, "");
        assert_eq!(
            runner.calls(),
            vec![(
                "wpctl".to_string(),
                ["set-volume", "-l", "1.0", "@DEFAULT_AUDIO_SINK@", "10%+"]
                    .map(String::from)
                    .to_vec()
            )]
        );
    }

    #[tokio::test]
    async fn quoi_qui_passe() {
        let mut sp = player("spotify", PlaybackStatus::Playing);
        sp.title = Some("Hello".into());
        sp.artist = Some("Adele".into());
        let (s, _, _) = skill(vec![sp]);
        let r = s
            .call("media_now_playing", serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(r.speech, "C'est Hello, par Adele.");
    }

    #[tokio::test]
    async fn outil_inconnu() {
        let (s, _, _) = skill(vec![]);
        let e = s
            .call("media_explode", serde_json::json!({}))
            .await
            .unwrap_err();
        assert!(matches!(e, SkillError::UnknownTool(_)));
    }

    fn xdotool(key: &str) -> (String, Vec<String>) {
        (
            "xdotool".to_string(),
            vec!["key".to_string(), key.to_string()],
        )
    }

    #[tokio::test]
    async fn touches_multimedia_sans_lecteur() {
        for (tool, key) in [
            ("media_next", "XF86AudioNext"),
            ("media_previous", "XF86AudioPrev"),
            ("media_play", "XF86AudioPlay"),
            ("media_pause", "XF86AudioPause"),
        ] {
            let (s, _, runner) = skill(vec![]);
            let r = s.call(tool, serde_json::json!({})).await.unwrap();
            assert_eq!(r.speech, "");
            assert_eq!(runner.calls(), vec![xdotool(key)]);
        }
    }

    #[tokio::test]
    async fn sans_lecteur_ni_xdotool() {
        let (s, _, runner) = skill(vec![]);
        runner.respond(
            "xdotool",
            crate::CommandOutput {
                success: false,
                stdout: String::new(),
            },
        );
        let r = s.call("media_next", serde_json::json!({})).await.unwrap();
        assert_eq!(r.speech, "Aucun lecteur n'est ouvert.");
    }

    #[tokio::test]
    async fn deja_en_lecture_pas_de_commande() {
        let (s, p, runner) = skill(vec![player("vlc", PlaybackStatus::Playing)]);
        let r = s.call("media_play", serde_json::json!({})).await.unwrap();
        assert_eq!(r.speech, "");
        assert!(p.sent.lock().unwrap().is_empty());
        assert!(runner.calls().is_empty());
    }

    #[tokio::test]
    async fn deja_en_pause_pas_de_commande() {
        for st in [PlaybackStatus::Paused, PlaybackStatus::Stopped] {
            let (s, p, _) = skill(vec![player("vlc", st)]);
            let r = s.call("media_pause", serde_json::json!({})).await.unwrap();
            assert_eq!(r.speech, "");
            assert!(p.sent.lock().unwrap().is_empty());
        }
    }

    struct FailingPlayers(PlayerInfo);

    #[async_trait::async_trait]
    impl MediaPlayers for FailingPlayers {
        async fn players(&self) -> anyhow::Result<Vec<PlayerInfo>> {
            Ok(vec![self.0.clone()])
        }
        async fn command(&self, _: &str, _: PlayerCommand) -> anyhow::Result<()> {
            Err(anyhow::anyhow!("Access denied org.mpris"))
        }
    }

    #[tokio::test]
    async fn erreur_lecteur_phrase_courte() {
        let mut info = player("firefox.instance_1_154", PlaybackStatus::Playing);
        info.identity = "Firefox".into();
        let s = MediaSkill::new(Arc::new(FailingPlayers(info)), Arc::new(FakeRunner::new()));
        let r = s.call("media_pause", serde_json::json!({})).await.unwrap();
        assert_eq!(r.speech, "Firefox refuse la commande.");
        assert!(!r.speech.contains("denied"));
    }

    #[tokio::test]
    async fn quoi_qui_passe_sans_lecteur_accessible() {
        let (s, _, _) = skill(vec![]);
        let r = s
            .call("media_now_playing", serde_json::json!({}))
            .await
            .unwrap();
        assert!(r.speech.starts_with("Je ne vois aucun lecteur."));
    }
}
