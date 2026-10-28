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
    Say {
        text: String,
    },
    Status,
    SetMode {
        mode: Mode,
    },
    /// « Jarvis » entendu : le cerveau baisse le son.
    Wake,
    /// Phrase captée : `audio` = i16 little-endian 16 kHz mono en base64 (vide si rien
    /// n'a été dit). Rétablit toujours le son.
    Listen {
        audio: String,
        muted: bool,
    },
    /// Enregistre l'empreinte vocale : chaque élément est un échantillon audio
    /// (même format que `Listen.audio`).
    EnrollVoice {
        samples: Vec<String>,
    },
}

/// Action demandée à l'oreille après une phrase entendue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EarAction {
    Mute,
    Unmute,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlResponse {
    Reply {
        speech: String,
    },
    Status {
        mode: Mode,
        version: String,
        #[serde(default)]
        local_model: String,
    },
    Heard {
        text: String,
        speech: String,
        ear_action: Option<EarAction>,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    #[error("audio base64 invalide : {0}")]
    Base64(#[from] base64::DecodeError),
    #[error("audio de longueur impaire")]
    OddLength,
}

/// Encode des échantillons i16 (little-endian) en base64 standard.
pub fn encode_pcm(samples: &[i16]) -> String {
    use base64::Engine;
    let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Inverse de [`encode_pcm`].
pub fn decode_pcm(b64: &str) -> Result<Vec<i16>, ProtocolError> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD.decode(b64)?;
    if bytes.len() % 2 != 0 {
        return Err(ProtocolError::OddLength);
    }
    Ok(bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| i16::from_le_bytes(*c))
        .collect())
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
                local_model: "llama3.2:3b".into(),
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
    fn wake_serialise() {
        let json = serde_json::to_string(&ControlRequest::Wake).unwrap();
        assert_eq!(json, r#"{"type":"wake"}"#);
    }

    #[test]
    fn listen_aller_retour() {
        let r = ControlRequest::Listen {
            audio: encode_pcm(&[1, 2, 3]),
            muted: true,
        };
        let json = serde_json::to_string(&r).unwrap();
        assert_eq!(serde_json::from_str::<ControlRequest>(&json).unwrap(), r);
    }

    #[test]
    fn enroll_voice_aller_retour() {
        let r = ControlRequest::EnrollVoice {
            samples: vec![encode_pcm(&[1, 2]), encode_pcm(&[3])],
        };
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.starts_with(r#"{"type":"enroll_voice""#), "{json}");
        assert_eq!(serde_json::from_str::<ControlRequest>(&json).unwrap(), r);
    }

    #[test]
    fn heard_avec_action() {
        let r = ControlResponse::Heard {
            text: "coupe l'écoute".into(),
            speech: "D'accord.".into(),
            ear_action: Some(EarAction::Mute),
        };
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains(r#""ear_action":"mute""#), "{json}");
        assert_eq!(serde_json::from_str::<ControlResponse>(&json).unwrap(), r);
    }

    #[test]
    fn status_sans_local_model_reste_lisible() {
        let r: ControlResponse =
            serde_json::from_str(r#"{"type":"status","mode":"local","version":"0.1.0"}"#).unwrap();
        assert_eq!(
            r,
            ControlResponse::Status {
                mode: Mode::Local,
                version: "0.1.0".into(),
                local_model: String::new(),
            }
        );
    }

    #[test]
    fn pcm_aller_retour() {
        let samples = [0, 1, -1, i16::MAX, i16::MIN];
        assert_eq!(decode_pcm(&encode_pcm(&samples)).unwrap(), samples);
        assert_eq!(decode_pcm("").unwrap(), Vec::<i16>::new());
    }

    #[test]
    fn pcm_base64_invalide() {
        assert!(decode_pcm("!!pas du base64!!").is_err());
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
