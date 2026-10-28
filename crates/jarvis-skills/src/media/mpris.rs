//! Real MPRIS access over the D-Bus session bus.

use std::collections::HashMap;

use anyhow::Context;
use zbus::OwnedVariant;
use zbus::{Connection, Proxy};

use super::{MediaPlayers, PlaybackStatus, PlayerCommand, PlayerInfo};

const PREFIX: &str = "org.mpris.MediaPlayer2.";
const PATH: &str = "/org/mpris/MediaPlayer2";
const ROOT_IFACE: &str = "org.mpris.MediaPlayer2";
const PLAYER_IFACE: &str = "org.mpris.MediaPlayer2.Player";

/// MPRIS players on the D-Bus session bus. Connects on every call, so it
/// works even when the bus appears after Jarvis starts.
#[derive(Debug, Default, Clone, Copy)]
pub struct MprisPlayers;

impl MprisPlayers {
    pub fn new() -> Self {
        Self
    }
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
    let status = player
        .get_property::<String>("PlaybackStatus")
        .await
        .map(|s| parse_status(&s))
        .unwrap_or(PlaybackStatus::Stopped);
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
        let conn = Connection::session()
            .await
            .context("connexion au bus de session")?;
        let names = zbus::fdo::DBusProxy::new(&conn)
            .await?
            .list_names()
            .await
            .context("liste des noms D-Bus")?;
        let mut out = Vec::new();
        for name in names.iter().filter(|n| n.starts_with(PREFIX)) {
            // A player that vanishes mid-listing is skipped, not fatal.
            match describe(&conn, name.as_str()).await {
                Ok(info) => out.push(info),
                Err(e) => tracing::warn!(player = %name, "lecteur MPRIS ignoré : {e}"),
            }
        }
        Ok(out)
    }

    async fn command(&self, bus_name: &str, cmd: PlayerCommand) -> anyhow::Result<()> {
        let conn = Connection::session()
            .await
            .context("connexion au bus de session")?;
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

    #[tokio::test]
    #[ignore = "nécessite un bus de session et un lecteur ouvert"]
    async fn mpris_reel_liste_les_lecteurs() {
        let players = MprisPlayers::new().players().await.unwrap();
        for p in &players {
            println!("{p:?}");
        }
    }
}
