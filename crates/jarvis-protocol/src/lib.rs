//! Messages échangés entre le cerveau et ses clients sur le socket de contrôle.
//! Format : une ligne JSON par message (terminée par `\n`).
//! Le protocole réseau des oreilles (WebSocket) est dans [`ear`] et [`frame`].

pub mod ear;
pub mod fingerprint;
pub mod frame;
pub mod pairing_uri;

pub use ear::{EarId, EarSummary};
pub use fingerprint::Fingerprint;

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

/// Nom de l'oreille quand `ear` est absent.
pub const DEFAULT_EAR: &str = "pc";

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
    // Pas de `wake` ni de `listen` : une phrase n'arrive que par le réseau des oreilles
    // (`ear`), où l'arbitre choisit une seule oreille. Une oreille trop ancienne reçoit
    // « requête invalide ».
    /// Annonces en attente pour cette oreille (fin d'une tâche d'agent…) ;
    /// oreille absente : [`DEFAULT_EAR`].
    Announcements {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ear: Option<String>,
    },
    /// Enregistre l'empreinte vocale : chaque élément est un échantillon audio
    /// (i16 little-endian 16 kHz mono en base64, voir [`encode_pcm`]).
    EnrollVoice {
        samples: Vec<String>,
    },
    /// Ouvre la fenêtre d'appairage : réponse `Pairing`, puis, sur la même connexion,
    /// `PairingDone` ou `PairingEnded`. Connexion fermée : fenêtre fermée.
    PairStart,
    /// Appairage de l'oreille du PC (le socket Unix vaut preuve, sans code).
    PairLocal {
        name: String,
    },
    ListEars,
    /// Retire une oreille (nom ou identifiant) : réponse `Reply`.
    Unpair {
        ear: String,
    },
}

/// Action demandée à l'oreille après une phrase entendue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EarAction {
    Mute,
    Unmute,
    /// Le cerveau attend une réponse (oui / non) : après la lecture de `speech`,
    /// l'oreille écoute sans mot d'éveil, 8 s au plus pour commencer à parler.
    AwaitReply,
}

/// Bip joué par l'oreille avant la réponse. Le bip de réveil, lui, est local.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sound {
    Success,
    Failure,
}

/// Voix de Jarvis : `pcm` = i16 little-endian mono en base64 (voir [`encode_pcm`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpeechAudio {
    pub pcm: String,
    pub sample_rate: u32,
}

/// Annonce à dire par l'oreille ; voix et bip facultatifs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Announcement {
    pub speech: String,
    #[serde(default)]
    pub audio: Option<SpeechAudio>,
    #[serde(default)]
    pub sound: Option<Sound>,
}

/// `Debug` écrit à la main : jeton et code d'appairage masqués.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
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
        /// `speech` dit par la voix de Jarvis ; absent si la voix n'est pas installée.
        #[serde(default)]
        audio: Option<SpeechAudio>,
        #[serde(default)]
        sound: Option<Sound>,
    },
    /// Réponse à `Announcements` : vide s'il n'y a rien à dire.
    Announcements {
        items: Vec<Announcement>,
    },
    Error {
        message: String,
    },
    /// Fenêtre d'appairage ouverte (réponse à `PairStart`).
    Pairing {
        code: String,
        /// URI du QR code (voir [`pairing_uri::PairingUri`]).
        uri: String,
        fingerprint_short: String,
        expires_in_secs: u64,
    },
    /// Une oreille s'est appairée : fenêtre fermée.
    PairingDone {
        name: String,
    },
    /// Fenêtre fermée sans appairage (délai écoulé, trop d'essais…).
    PairingEnded {
        reason: String,
    },
    /// Oreille du PC appairée (réponse à `PairLocal`).
    PairedLocal {
        ear_id: EarId,
        token: String,
        port: u16,
        fingerprint: Fingerprint,
        brain_id: String,
        brain_name: String,
    },
    Ears {
        ears: Vec<EarSummary>,
    },
}

const MASKED: &str = "***";

impl fmt::Debug for ControlResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ControlResponse::Reply { speech } => {
                f.debug_struct("Reply").field("speech", speech).finish()
            }
            ControlResponse::Status {
                mode,
                version,
                local_model,
            } => f
                .debug_struct("Status")
                .field("mode", mode)
                .field("version", version)
                .field("local_model", local_model)
                .finish(),
            ControlResponse::Heard {
                text,
                speech,
                ear_action,
                audio,
                sound,
            } => f
                .debug_struct("Heard")
                .field("text", text)
                .field("speech", speech)
                .field("ear_action", ear_action)
                .field("audio", audio)
                .field("sound", sound)
                .finish(),
            ControlResponse::Announcements { items } => f
                .debug_struct("Announcements")
                .field("items", items)
                .finish(),
            ControlResponse::Error { message } => {
                f.debug_struct("Error").field("message", message).finish()
            }
            ControlResponse::Pairing {
                code: _,
                uri: _,
                fingerprint_short,
                expires_in_secs,
            } => f
                .debug_struct("Pairing")
                .field("code", &MASKED)
                .field("uri", &MASKED)
                .field("fingerprint_short", fingerprint_short)
                .field("expires_in_secs", expires_in_secs)
                .finish(),
            ControlResponse::PairingDone { name } => {
                f.debug_struct("PairingDone").field("name", name).finish()
            }
            ControlResponse::PairingEnded { reason } => f
                .debug_struct("PairingEnded")
                .field("reason", reason)
                .finish(),
            ControlResponse::PairedLocal {
                ear_id,
                token: _,
                port,
                fingerprint,
                brain_id,
                brain_name,
            } => f
                .debug_struct("PairedLocal")
                .field("ear_id", ear_id)
                .field("token", &MASKED)
                .field("port", port)
                .field("fingerprint", fingerprint)
                .field("brain_id", brain_id)
                .field("brain_name", brain_name)
                .finish(),
            ControlResponse::Ears { ears } => f.debug_struct("Ears").field("ears", ears).finish(),
        }
    }
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

/// Texte venu d'ailleurs (réseau, annonce mDNS), sûr à afficher : sans caractère de
/// contrôle (retour chariot, échappement ANSI…) ni marque de direction (U+061C,
/// U+200E, U+200F, U+202A à U+202E, U+2066 à U+2069), qui pourraient maquiller un
/// terminal ou un nom.
pub fn printable(text: &str) -> String {
    text.chars()
        .filter(|&c| {
            !c.is_control()
                && !matches!(
                    c,
                    '\u{061C}'
                        | '\u{200E}'
                        | '\u{200F}'
                        | '\u{202A}'..='\u{202E}'
                        | '\u{2066}'..='\u{2069}'
                )
        })
        .collect()
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
    fn texte_affichable_sans_controle_ni_direction() {
        assert_eq!(printable("PC de Charles"), "PC de Charles");
        assert_eq!(printable("Bureau é ü 😀"), "Bureau é ü 😀");
        // Échappement ANSI, retour chariot, retour à la ligne, nul.
        assert_eq!(printable("PC\u{1b}[2J\r\nfaux\0"), "PC[2Jfaux");
        // Inversion de direction (« txt.exe » affiché « exe.txt »), isolats, marques.
        assert_eq!(
            printable("a\u{202E}b\u{2066}c\u{2069}d\u{200E}e\u{200F}f\u{061C}g\u{202A}h"),
            "abcdefgh"
        );
        // C1 (U+0080–U+009F) aussi.
        assert_eq!(printable("a\u{9b}31mb"), "a31mb");
    }

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
            audio: None,
            sound: None,
        };
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains(r#""ear_action":"mute""#), "{json}");
        assert_eq!(serde_json::from_str::<ControlResponse>(&json).unwrap(), r);
    }

    #[test]
    fn heard_avec_voix_et_bip() {
        let r = ControlResponse::Heard {
            text: "mets le PC en veille".into(),
            speech: "Je mets le PC en veille ?".into(),
            ear_action: Some(EarAction::AwaitReply),
            audio: Some(SpeechAudio {
                pcm: encode_pcm(&[1, -1]),
                sample_rate: 22_050,
            }),
            sound: Some(Sound::Failure),
        };
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains(r#""ear_action":"await_reply""#), "{json}");
        assert!(json.contains(r#""sound":"failure""#), "{json}");
        assert!(json.contains(r#""sample_rate":22050"#), "{json}");
        assert_eq!(serde_json::from_str::<ControlResponse>(&json).unwrap(), r);
    }

    #[test]
    fn ancien_heard_reste_lisible() {
        let r: ControlResponse = serde_json::from_str(
            r#"{"type":"heard","text":"suivant","speech":"Bien.","ear_action":null}"#,
        )
        .unwrap();
        assert_eq!(
            r,
            ControlResponse::Heard {
                text: "suivant".into(),
                speech: "Bien.".into(),
                ear_action: None,
                audio: None,
                sound: None,
            }
        );
    }

    #[test]
    fn ancienne_requete_listen_refusee() {
        // Une oreille d'avant le réseau : refusée (« requête invalide » côté cerveau),
        // aucune phrase n'échappe à l'arbitre.
        for line in [
            r#"{"type":"wake"}"#,
            r#"{"type":"listen","audio":"","muted":false}"#,
            r#"{"type":"listen","audio":"","muted":true,"follow_up":true,"ear":"salon"}"#,
        ] {
            assert!(
                serde_json::from_str::<ControlRequest>(line).is_err(),
                "{line}"
            );
        }
    }

    #[test]
    fn annonces_sans_ear_restent_lisibles() {
        let r: ControlRequest = serde_json::from_str(r#"{"type":"announcements"}"#).unwrap();
        assert_eq!(r, ControlRequest::Announcements { ear: None });
        assert_eq!(DEFAULT_EAR, "pc");
    }

    #[test]
    fn announcements_aller_retour() {
        let r = ControlRequest::Announcements {
            ear: Some("pc".into()),
        };
        let json = serde_json::to_string(&r).unwrap();
        assert_eq!(json, r#"{"type":"announcements","ear":"pc"}"#);
        assert_eq!(serde_json::from_str::<ControlRequest>(&json).unwrap(), r);
        let r = ControlResponse::Announcements {
            items: vec![Announcement {
                speech: "La tâche sur Jarvis est terminée.".into(),
                audio: Some(SpeechAudio {
                    pcm: encode_pcm(&[1, -1]),
                    sample_rate: 22_050,
                }),
                sound: Some(Sound::Success),
            }],
        };
        let json = serde_json::to_string(&r).unwrap();
        assert!(
            json.starts_with(r#"{"type":"announcements","items":["#),
            "{json}"
        );
        assert_eq!(serde_json::from_str::<ControlResponse>(&json).unwrap(), r);
    }

    #[test]
    fn annonce_sans_audio_lisible() {
        let r: ControlResponse =
            serde_json::from_str(r#"{"type":"announcements","items":[{"speech":"Fini."}]}"#)
                .unwrap();
        assert_eq!(
            r,
            ControlResponse::Announcements {
                items: vec![Announcement {
                    speech: "Fini.".into(),
                    audio: None,
                    sound: None,
                }],
            }
        );
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
    fn pair_start_et_pairing_serialises() {
        let json = serde_json::to_string(&ControlRequest::PairStart).unwrap();
        assert_eq!(json, r#"{"type":"pair_start"}"#);
        let requests = [
            ControlRequest::PairStart,
            ControlRequest::PairLocal { name: "PC".into() },
            ControlRequest::ListEars,
            ControlRequest::Unpair {
                ear: "Pixel 4 XL".into(),
            },
        ];
        for r in requests {
            let json = serde_json::to_string(&r).unwrap();
            assert_eq!(serde_json::from_str::<ControlRequest>(&json).unwrap(), r);
        }
        let pairing = ControlResponse::Pairing {
            code: "482913".into(),
            uri: "jarvis://pair?v=1&c=482913".into(),
            fingerprint_short: "A1B2-C3D4-E5F6-0718".into(),
            expires_in_secs: 120,
        };
        let json = serde_json::to_string(&pairing).unwrap();
        assert!(
            json.starts_with(r#"{"type":"pairing","code":"482913","#),
            "{json}"
        );
        assert!(!format!("{pairing:?}").contains("482913"));
        let paired = ControlResponse::PairedLocal {
            ear_id: EarId::new("ab"),
            token: "jeton-tres-secret".into(),
            port: 7341,
            fingerprint: Fingerprint::of_der(b"cert"),
            brain_id: "b1".into(),
            brain_name: "PC".into(),
        };
        assert!(!format!("{paired:?}").contains("jeton-tres-secret"));
        let responses = [
            pairing,
            paired,
            ControlResponse::PairingDone {
                name: "Pixel 4 XL".into(),
            },
            ControlResponse::PairingEnded {
                reason: "délai écoulé".into(),
            },
            ControlResponse::Ears {
                ears: vec![EarSummary {
                    id: EarId::new("ab"),
                    name: "PC".into(),
                    platform: "linux".into(),
                    connected: false,
                    local: true,
                }],
            },
        ];
        for r in responses {
            let json = serde_json::to_string(&r).unwrap();
            assert_eq!(serde_json::from_str::<ControlResponse>(&json).unwrap(), r);
        }
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
