//! Real MPRIS access over the D-Bus session bus.

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use anyhow::Context;
use zbus::OwnedVariant;
use zbus::{Connection, Proxy};

use super::{MediaPlayers, PlaybackStatus, PlayerCommand, PlayerInfo};

const PREFIX: &str = "org.mpris.MediaPlayer2.";
const PATH: &str = "/org/mpris/MediaPlayer2";
const ROOT_IFACE: &str = "org.mpris.MediaPlayer2";
const PLAYER_IFACE: &str = "org.mpris.MediaPlayer2.Player";
/// `playerctld` is a proxy daemon, not a real player.
const IGNORED: [&str; 1] = ["org.mpris.MediaPlayer2.playerctld"];
/// Deadline for D-Bus method calls and for describing one player.
const DBUS_TIMEOUT: Duration = Duration::from_millis(1500);

/// MPRIS players on the D-Bus session bus. Connects on every call, so it
/// works even when the bus appears after Jarvis starts.
#[derive(Debug, Default, Clone, Copy)]
pub struct MprisPlayers;

impl MprisPlayers {
    pub fn new() -> Self {
        Self
    }
}

/// Keep only real MPRIS player bus names.
fn player_names(names: &[String]) -> Vec<String> {
    names
        .iter()
        .filter(|n| n.starts_with(PREFIX) && !IGNORED.contains(&n.as_str()))
        .cloned()
        .collect()
}

/// Session connection whose method calls give up after [`DBUS_TIMEOUT`].
async fn connect() -> anyhow::Result<Connection> {
    zbus::connection::Builder::session()
        .method_timeout(DBUS_TIMEOUT)
        .build()
        .await
        .context("connexion au bus de session")
}

/// Bus names already reported in the log, so a broken player (e.g. a snap
/// Firefox answering "Access denied") does not flood it on every call.
fn warn_once(name: &str) -> bool {
    static WARNED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    let set = WARNED.get_or_init(Default::default);
    set.lock()
        .map(|mut s| s.insert(name.to_string()))
        .unwrap_or(false)
}

/// Readable status, or `None` when the player must be skipped.
fn status_or_skip<E>(raw: Result<String, E>) -> Option<PlaybackStatus> {
    raw.ok().map(|s| parse_status(&s))
}

fn parse_status(s: &str) -> PlaybackStatus {
    match s {
        "Playing" => PlaybackStatus::Playing,
        "Paused" => PlaybackStatus::Paused,
        _ => PlaybackStatus::Stopped,
    }
}

async fn proxy<'a>(
    conn: &Connection,
    bus_name: &'a str,
    iface: &'a str,
) -> zbus::Result<Proxy<'a>> {
    Proxy::new(conn, bus_name, PATH, iface).await
}

async fn describe(conn: &Connection, bus_name: &str) -> anyhow::Result<PlayerInfo> {
    let root = proxy(conn, bus_name, ROOT_IFACE).await?;
    let player = proxy(conn, bus_name, PLAYER_IFACE).await?;

    let identity = root
        .get_property::<String>("Identity")
        .await
        .unwrap_or_else(|_| bus_name.trim_start_matches(PREFIX).to_string());
    // Unreadable status (Access denied, timeout...) means unusable player.
    let status = status_or_skip(player.get_property::<String>("PlaybackStatus").await)
        .context("PlaybackStatus illisible")?;
    let metadata = player
        .get_property::<HashMap<String, OwnedVariant>>("Metadata")
        .await
        .unwrap_or_default();

    let title = metadata
        .get("xesam:title")
        .and_then(|v| String::try_from(v.try_clone().ok()?).ok())
        .filter(|s| !s.is_empty());
    // xesam:artist is a string array per spec, but some players send a string.
    let artist = metadata
        .get("xesam:artist")
        .and_then(|v| {
            let v = v.try_clone().ok()?;
            Vec::<String>::try_from(v.try_clone().ok()?)
                .ok()
                .and_then(|a| a.into_iter().next())
                .or_else(|| String::try_from(v).ok())
        })
        .filter(|s| !s.is_empty());

    Ok(PlayerInfo {
        bus_name: bus_name.to_string(),
        identity,
        status,
        title,
        artist,
    })
}

#[async_trait::async_trait]
impl MediaPlayers for MprisPlayers {
    async fn players(&self) -> anyhow::Result<Vec<PlayerInfo>> {
        let conn = connect().await?;
        let names = zbus::fdo::DBusProxy::new(&conn)
            .await?
            .list_names()
            .await
            .context("liste des noms D-Bus")?;
        let mut out = Vec::new();
        let names: Vec<String> = names.iter().map(|n| n.to_string()).collect();
        for name in player_names(&names) {
            // A player that vanishes or hangs is skipped, not fatal.
            match tokio::time::timeout(DBUS_TIMEOUT, describe(&conn, &name)).await {
                Ok(Ok(info)) => out.push(info),
                Ok(Err(e)) => {
                    if warn_once(&name) {
                        tracing::warn!(player = %name, "lecteur MPRIS ignoré : {e:#}");
                    }
                }
                Err(_) => {
                    if warn_once(&name) {
                        tracing::warn!(player = %name, "lecteur MPRIS ignoré : délai dépassé");
                    }
                }
            }
        }
        Ok(out)
    }

    async fn command(&self, bus_name: &str, cmd: PlayerCommand) -> anyhow::Result<()> {
        let conn = connect().await?;
        let player = proxy(&conn, bus_name, PLAYER_IFACE).await?;
        let method = match cmd {
            PlayerCommand::Next => "Next",
            PlayerCommand::Previous => "Previous",
            PlayerCommand::Play => "Play",
            PlayerCommand::Pause => "Pause",
        };
        player
            .call_method(method, &())
            .await
            .with_context(|| format!("{method} sur {bus_name}"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignore_playerctld() {
        let names = [
            "org.freedesktop.DBus".to_string(),
            "org.mpris.MediaPlayer2.playerctld".to_string(),
            "org.mpris.MediaPlayer2.vlc".to_string(),
        ];
        assert_eq!(
            player_names(&names),
            vec!["org.mpris.MediaPlayer2.vlc".to_string()]
        );
    }

    #[test]
    fn lecteur_inaccessible_ignore() {
        assert_eq!(status_or_skip::<()>(Err(())), None);
        assert_eq!(
            status_or_skip::<()>(Ok("Playing".into())),
            Some(PlaybackStatus::Playing)
        );
    }

    #[test]
    fn avertit_une_seule_fois_par_nom() {
        assert!(warn_once("org.mpris.MediaPlayer2.test_unique"));
        assert!(!warn_once("org.mpris.MediaPlayer2.test_unique"));
    }

    #[tokio::test]
    #[ignore = "nécessite un bus de session et un lecteur ouvert"]
    async fn mpris_reel_liste_les_lecteurs() {
        let players = MprisPlayers::new().players().await.unwrap();
        for p in &players {
            println!("{p:?}");
        }
    }
}
