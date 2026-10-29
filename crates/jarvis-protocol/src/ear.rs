//! Protocole réseau oreille ↔ cerveau (v1) : WebSocket sur TLS 1.3.
//! Messages texte en JSON (`type` en tête), son du micro en trames binaires
//! (voir [`crate::frame`]).
//!
//! Compatibilité : les champs ajoutés ont une valeur par défaut, les types de message
//! inconnus sont lus comme `Unknown` (à ignorer). Tout changement incompatible fait
//! passer [`PROTOCOL_VERSION`] à 2.

use crate::{ControlResponse, DEFAULT_EAR, Mode, SpeechAudio};
use serde::{Deserialize, Serialize};
use std::fmt;

pub const PROTOCOL_VERSION: u16 = 1;
pub const DEFAULT_PORT: u16 = 7341;
pub const MDNS_SERVICE: &str = "_jarvis._tcp.local.";

/// Remplace un secret dans `Debug`.
const MASKED: &str = "***";

/// Identifiant d'une oreille (16 octets en hexadécimal, attribué à l'appairage).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EarId(String);

impl EarId {
    pub fn new(id: impl Into<String>) -> Self {
        EarId(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Provisoire (retiré en tâche 9) : oreille passée par le socket Unix
    /// (`Listen`), nommée par son champ `ear` (absent : [`DEFAULT_EAR`]).
    /// Le préfixe évite toute confusion avec une oreille appairée.
    pub fn unix_socket(name: Option<&str>) -> Self {
        EarId(format!("unix-socket:{}", name.unwrap_or(DEFAULT_EAR)))
    }
}

impl fmt::Display for EarId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Oreille → cerveau.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EarMessage {
    /// Premier message d'une oreille appairée.
    Hello {
        version: u16,
        ear_id: EarId,
        token: String,
        /// Id de la phrase « le PC est endormi » déjà stockée.
        #[serde(default)]
        offline_phrase: Option<String>,
    },
    /// Premier message d'une oreille à appairer (fenêtre ouverte par `jarvis pair`).
    Pair {
        version: u16,
        code: String,
        name: String,
        platform: String,
    },
    /// « Jarvis » entendu ; le son de la phrase suit en trames binaires.
    Wake {
        utterance: u32,
        score: f32,
        level_db: f32,
        muted: bool,
    },
    /// Réponse à `AwaitReply`, captée sans mot d'éveil ; le son suit.
    ListenStart {
        utterance: u32,
    },
    /// Fin de la phrase. `cut` : plage d'échantillons à retirer du flux.
    ListenEnd {
        utterance: u32,
        muted: bool,
        no_speech: bool,
        #[serde(default)]
        cut: Option<(u32, u32)>,
    },
    Status,
    SetMode {
        mode: Mode,
    },
    #[serde(other)]
    Unknown,
}

/// Écrit à la main : le jeton et le code n'apparaissent jamais dans les journaux.
impl fmt::Debug for EarMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EarMessage::Hello {
                version,
                ear_id,
                token: _,
                offline_phrase,
            } => f
                .debug_struct("Hello")
                .field("version", version)
                .field("ear_id", ear_id)
                .field("token", &MASKED)
                .field("offline_phrase", offline_phrase)
                .finish(),
            EarMessage::Pair {
                version,
                code: _,
                name,
                platform,
            } => f
                .debug_struct("Pair")
                .field("version", version)
                .field("code", &MASKED)
                .field("name", name)
                .field("platform", platform)
                .finish(),
            EarMessage::Wake {
                utterance,
                score,
                level_db,
                muted,
            } => f
                .debug_struct("Wake")
                .field("utterance", utterance)
                .field("score", score)
                .field("level_db", level_db)
                .field("muted", muted)
                .finish(),
            EarMessage::ListenStart { utterance } => f
                .debug_struct("ListenStart")
                .field("utterance", utterance)
                .finish(),
            EarMessage::ListenEnd {
                utterance,
                muted,
                no_speech,
                cut,
            } => f
                .debug_struct("ListenEnd")
                .field("utterance", utterance)
                .field("muted", muted)
                .field("no_speech", no_speech)
                .field("cut", cut)
                .finish(),
            EarMessage::Status => f.write_str("Status"),
            EarMessage::SetMode { mode } => f.debug_struct("SetMode").field("mode", mode).finish(),
            EarMessage::Unknown => f.write_str("Unknown"),
        }
    }
}

/// Cerveau → oreille.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BrainMessage {
    Welcome {
        version: u16,
        brain_id: String,
        brain_name: String,
        status: StatusInfo,
    },
    /// Refus, puis fermeture de la connexion.
    Rejected {
        reason: RejectReason,
    },
    /// Appairage réussi : le jeton n'est donné qu'une fois. Connexion fermée ensuite,
    /// l'oreille revient avec `Hello`.
    Paired {
        ear_id: EarId,
        token: String,
        brain_id: String,
        brain_name: String,
    },
    /// L'oreille a gagné : la capture continue.
    Elected {
        utterance: u32,
    },
    /// L'oreille a perdu (ou une autre phrase est en cours) : capture abandonnée.
    Stop {
        utterance: u32,
    },
    /// Réponse à une phrase (`Heard`, voix comprise) ou à `Status` / `SetMode`.
    Response {
        utterance: Option<u32>,
        response: ControlResponse,
    },
    Status(StatusInfo),
    /// Phrase « le PC est endormi », dite par la voix de Jarvis, à stocker.
    OfflinePhrase {
        id: String,
        audio: SpeechAudio,
    },
    #[serde(other)]
    Unknown,
}

/// Écrit à la main : le jeton n'apparaît jamais dans les journaux, ni le son.
impl fmt::Debug for BrainMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BrainMessage::Welcome {
                version,
                brain_id,
                brain_name,
                status,
            } => f
                .debug_struct("Welcome")
                .field("version", version)
                .field("brain_id", brain_id)
                .field("brain_name", brain_name)
                .field("status", status)
                .finish(),
            BrainMessage::Rejected { reason } => {
                f.debug_struct("Rejected").field("reason", reason).finish()
            }
            BrainMessage::Paired {
                ear_id,
                token: _,
                brain_id,
                brain_name,
            } => f
                .debug_struct("Paired")
                .field("ear_id", ear_id)
                .field("token", &MASKED)
                .field("brain_id", brain_id)
                .field("brain_name", brain_name)
                .finish(),
            BrainMessage::Elected { utterance } => f
                .debug_struct("Elected")
                .field("utterance", utterance)
                .finish(),
            BrainMessage::Stop { utterance } => f
                .debug_struct("Stop")
                .field("utterance", utterance)
                .finish(),
            BrainMessage::Response {
                utterance,
                response,
            } => f
                .debug_struct("Response")
                .field("utterance", utterance)
                .field("response", response)
                .finish(),
            BrainMessage::Status(s) => f.debug_tuple("Status").field(s).finish(),
            BrainMessage::OfflinePhrase { id, audio } => f
                .debug_struct("OfflinePhrase")
                .field("id", id)
                .field("sample_rate", &audio.sample_rate)
                .finish_non_exhaustive(),
            BrainMessage::Unknown => f.write_str("Unknown"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusInfo {
    pub mode: Mode,
    pub version: String,
    pub local_model: String,
    #[serde(default)]
    pub ears: Vec<EarSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EarSummary {
    pub id: EarId,
    pub name: String,
    pub platform: String,
    pub connected: bool,
    /// Oreille du PC (appairée par le socket Unix).
    pub local: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RejectReason {
    UnknownEar,
    BadToken,
    Version {
        supported: Vec<u16>,
    },
    /// `Pair` sans fenêtre d'appairage ouverte.
    PairingClosed,
    WrongCode {
        attempts_left: u8,
    },
    /// Raison ajoutée par une version plus récente.
    #[serde(other)]
    Unknown,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EarAction, Sound, encode_pcm};

    fn hello() -> EarMessage {
        EarMessage::Hello {
            version: PROTOCOL_VERSION,
            ear_id: EarId::new("00112233445566778899aabbccddeeff"),
            token: "jeton-tres-secret".into(),
            offline_phrase: Some("a1b2".into()),
        }
    }

    fn round_trip_ear(m: &EarMessage) {
        let json = serde_json::to_string(m).unwrap();
        assert_eq!(
            &serde_json::from_str::<EarMessage>(&json).unwrap(),
            m,
            "{json}"
        );
    }

    fn round_trip_brain(m: &BrainMessage) {
        let json = serde_json::to_string(m).unwrap();
        assert_eq!(
            &serde_json::from_str::<BrainMessage>(&json).unwrap(),
            m,
            "{json}"
        );
    }

    fn status() -> StatusInfo {
        StatusInfo {
            mode: Mode::Local,
            version: "0.1.0".into(),
            local_model: "llama3.2:3b".into(),
            ears: vec![EarSummary {
                id: EarId::new("ab"),
                name: "PC".into(),
                platform: "linux".into(),
                connected: true,
                local: true,
            }],
        }
    }

    #[test]
    fn version_du_protocole_vaut_1() {
        assert_eq!(PROTOCOL_VERSION, 1);
        assert_eq!(DEFAULT_PORT, 7341);
        assert_eq!(MDNS_SERVICE, "_jarvis._tcp.local.");
        let id = EarId::new("ab12");
        assert_eq!(id.as_str(), "ab12");
        assert_eq!(id.to_string(), "ab12");
        assert_eq!(serde_json::to_string(&id).unwrap(), r#""ab12""#);
        assert_eq!(EarId::unix_socket(None), EarId::unix_socket(Some("pc")));
        assert_eq!(EarId::unix_socket(None).as_str(), "unix-socket:pc");
        assert_ne!(EarId::unix_socket(Some("salon")), EarId::unix_socket(None));
    }

    #[test]
    fn hello_aller_retour() {
        let json = serde_json::to_string(&hello()).unwrap();
        assert!(
            json.starts_with(r#"{"type":"hello","version":1,"#),
            "{json}"
        );
        round_trip_ear(&hello());
        for m in [
            EarMessage::Pair {
                version: 1,
                code: "482913".into(),
                name: "Pixel 4 XL".into(),
                platform: "android".into(),
            },
            EarMessage::Wake {
                utterance: 3,
                score: 0.75,
                level_db: -32.5,
                muted: false,
            },
            EarMessage::ListenStart { utterance: 4 },
            EarMessage::ListenEnd {
                utterance: 3,
                muted: true,
                no_speech: false,
                cut: Some((16_000, 24_000)),
            },
            EarMessage::Status,
            EarMessage::SetMode { mode: Mode::Online },
        ] {
            round_trip_ear(&m);
        }
        assert_eq!(
            serde_json::to_string(&EarMessage::Status).unwrap(),
            r#"{"type":"status"}"#
        );
        for m in [
            BrainMessage::Welcome {
                version: 1,
                brain_id: "b1".into(),
                brain_name: "PC de Charles".into(),
                status: status(),
            },
            BrainMessage::Rejected {
                reason: RejectReason::Version { supported: vec![1] },
            },
            BrainMessage::Rejected {
                reason: RejectReason::WrongCode { attempts_left: 3 },
            },
            BrainMessage::Rejected {
                reason: RejectReason::BadToken,
            },
            BrainMessage::Paired {
                ear_id: EarId::new("ab"),
                token: "t".into(),
                brain_id: "b1".into(),
                brain_name: "PC".into(),
            },
            BrainMessage::Elected { utterance: 1 },
            BrainMessage::Stop { utterance: 1 },
            BrainMessage::Status(status()),
            BrainMessage::OfflinePhrase {
                id: "a1b2".into(),
                audio: SpeechAudio {
                    pcm: encode_pcm(&[1, -1]),
                    sample_rate: 22_050,
                },
            },
        ] {
            round_trip_brain(&m);
        }
        let json = serde_json::to_string(&BrainMessage::Rejected {
            reason: RejectReason::UnknownEar,
        })
        .unwrap();
        assert_eq!(
            json,
            r#"{"type":"rejected","reason":{"kind":"unknown_ear"}}"#
        );
    }

    #[test]
    fn hello_debug_masque_le_jeton() {
        let debug = format!("{:?}", hello());
        assert!(!debug.contains("jeton-tres-secret"), "{debug}");
        assert!(debug.contains("***"), "{debug}");
        assert!(
            debug.contains("00112233445566778899aabbccddeeff"),
            "{debug}"
        );
        let pair = EarMessage::Pair {
            version: 1,
            code: "482913".into(),
            name: "Pixel".into(),
            platform: "android".into(),
        };
        assert!(!format!("{pair:?}").contains("482913"));
        let paired = BrainMessage::Paired {
            ear_id: EarId::new("ab"),
            token: "jeton-tres-secret".into(),
            brain_id: "b1".into(),
            brain_name: "PC".into(),
        };
        assert!(!format!("{paired:?}").contains("jeton-tres-secret"));
        // Ni le son de la phrase hors ligne.
        let pcm = encode_pcm(&[1234, -1234, 999]);
        let offline = BrainMessage::OfflinePhrase {
            id: "a1".into(),
            audio: SpeechAudio {
                pcm: pcm.clone(),
                sample_rate: 22_050,
            },
        };
        assert!(!format!("{offline:?}").contains(&pcm));
    }

    #[test]
    fn message_inconnu_ignore() {
        let m: EarMessage = serde_json::from_str(r#"{"type":"teleport","to":"mars"}"#).unwrap();
        assert_eq!(m, EarMessage::Unknown);
        let m: BrainMessage = serde_json::from_str(r#"{"type":"dance","style":"valse"}"#).unwrap();
        assert_eq!(m, BrainMessage::Unknown);
        let m: BrainMessage =
            serde_json::from_str(r#"{"type":"rejected","reason":{"kind":"lune"}}"#).unwrap();
        assert_eq!(
            m,
            BrainMessage::Rejected {
                reason: RejectReason::Unknown,
            }
        );
        // Pas de `type` : illisible.
        assert!(serde_json::from_str::<EarMessage>(r#"{"utterance":1}"#).is_err());
    }

    #[test]
    fn champ_ajoute_reste_lisible() {
        // Champ inconnu ajouté, champ facultatif absent.
        let m: EarMessage = serde_json::from_str(
            r#"{"type":"hello","version":1,"ear_id":"ab","token":"t","battery":80}"#,
        )
        .unwrap();
        assert_eq!(
            m,
            EarMessage::Hello {
                version: 1,
                ear_id: EarId::new("ab"),
                token: "t".into(),
                offline_phrase: None,
            }
        );
        let m: EarMessage = serde_json::from_str(
            r#"{"type":"listen_end","utterance":2,"muted":false,"no_speech":true,"rssi":-40}"#,
        )
        .unwrap();
        assert_eq!(
            m,
            EarMessage::ListenEnd {
                utterance: 2,
                muted: false,
                no_speech: true,
                cut: None,
            }
        );
        let m: BrainMessage = serde_json::from_str(
            r#"{"type":"status","mode":"online","version":"0.2.0","local_model":"x","uptime":5}"#,
        )
        .unwrap();
        assert_eq!(
            m,
            BrainMessage::Status(StatusInfo {
                mode: Mode::Online,
                version: "0.2.0".into(),
                local_model: "x".into(),
                ears: vec![],
            })
        );
    }

    #[test]
    fn response_contient_heard() {
        let m = BrainMessage::Response {
            utterance: Some(9),
            response: ControlResponse::Heard {
                text: "mets le PC en veille".into(),
                speech: "Je mets le PC en veille ?".into(),
                ear_action: Some(EarAction::AwaitReply),
                audio: Some(SpeechAudio {
                    pcm: encode_pcm(&[1, -1]),
                    sample_rate: 22_050,
                }),
                sound: Some(Sound::Success),
            },
        };
        let json = serde_json::to_string(&m).unwrap();
        assert!(
            json.starts_with(r#"{"type":"response","utterance":9,"response":{"type":"heard","#),
            "{json}"
        );
        round_trip_brain(&m);
        round_trip_brain(&BrainMessage::Response {
            utterance: None,
            response: ControlResponse::Reply {
                speech: "Fait.".into(),
            },
        });
    }
}
