//! Messages échangés entre le cerveau et ses clients sur le socket de contrôle.
//! Format : une ligne JSON par message (terminée par `\n`).

use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::fmt;
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Local,
    Online,
}

impl fmt::Display for Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Mode::Local => "local",
            Mode::Online => "en ligne",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseModeError(pub String);

impl fmt::Display for ParseModeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "mode inconnu : {}", self.0)
    }
}

impl std::error::Error for ParseModeError {}

impl FromStr for Mode {
    type Err = ParseModeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "local" => Ok(Mode::Local),
            "online" | "en-ligne" => Ok(Mode::Online),
            other => Err(ParseModeError(other.to_string())),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlRequest {
    Say { text: String },
    Status,
    SetMode { mode: Mode },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlResponse {
    Reply { speech: String },
    Status { mode: Mode, version: String },
    Error { message: String },
}

/// Chemin du socket de contrôle du cerveau.
pub fn socket_path() -> PathBuf {
    let uid = std::fs::metadata("/proc/self")
        .map(|m| m.uid())
        .unwrap_or(0);
    socket_path_from(std::env::var_os("XDG_RUNTIME_DIR"), uid)
}

/// Logique pure de [`socket_path`] : `xdg` est la valeur de `XDG_RUNTIME_DIR`.
pub fn socket_path_from(xdg: Option<OsString>, uid: u32) -> PathBuf {
    match xdg.filter(|v| !v.is_empty()) {
        Some(dir) => PathBuf::from(dir).join("jarvis/brain.sock"),
        None => PathBuf::from(format!("/tmp/jarvis-{uid}/brain.sock")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::path::PathBuf;

    #[test]
    fn say_serialise() {
        let json = serde_json::to_string(&ControlRequest::Say {
            text: "suivant".into(),
        })
        .unwrap();
        assert_eq!(json, r#"{"type":"say","text":"suivant"}"#);
    }

    #[test]
    fn set_mode_serialise() {
        let json = serde_json::to_string(&ControlRequest::SetMode { mode: Mode::Online }).unwrap();
        assert_eq!(json, r#"{"type":"set_mode","mode":"online"}"#);
    }

    #[test]
    fn aller_retour_reponse() {
        let responses = [
            ControlResponse::Reply {
                speech: "Fait.".into(),
            },
            ControlResponse::Status {
                mode: Mode::Local,
                version: "0.1.0".into(),
            },
            ControlResponse::Error {
                message: "Erreur.".into(),
            },
        ];
        for r in responses {
            let json = serde_json::to_string(&r).unwrap();
            assert_eq!(serde_json::from_str::<ControlResponse>(&json).unwrap(), r);
        }
    }

    #[test]
    fn mode_depuis_texte() {
        assert_eq!("en-ligne".parse::<Mode>(), Ok(Mode::Online));
        assert_eq!("local".parse::<Mode>(), Ok(Mode::Local));
        assert_eq!("online".parse::<Mode>(), Ok(Mode::Online));
        assert!("bof".parse::<Mode>().is_err());
        assert_eq!(Mode::Online.to_string(), "en ligne");
        assert_eq!(Mode::Local.to_string(), "local");
    }

    #[test]
    fn socket_sous_xdg_runtime() {
        let p = socket_path_from(Some(OsString::from("/run/user/1000")), 1000);
        assert_eq!(p, PathBuf::from("/run/user/1000/jarvis/brain.sock"));
    }

    #[test]
    fn socket_repli_tmp() {
        assert_eq!(
            socket_path_from(None, 42),
            PathBuf::from("/tmp/jarvis-42/brain.sock")
        );
        assert_eq!(
            socket_path_from(Some(OsString::new()), 42),
            PathBuf::from("/tmp/jarvis-42/brain.sock")
        );
    }
}
