//! Serveur du socket de contrôle : une ligne JSON par requête, une par réponse.

use crate::brain::Brain;
use crate::listen::ListenService;
use crate::speaker::SpeakerGate;
use crate::tts::Voice;
use anyhow::{Context, bail};
use jarvis_protocol::{ControlRequest, ControlResponse, decode_pcm};
use std::io::{self, ErrorKind};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;
use tokio::io::{
    AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader,
};
use tokio::net::{UnixListener, UnixStream};
use tracing::{debug, info, warn};

/// Écoute sur `socket` jusqu'à une erreur d'acceptation. Chaque client a sa tâche.
pub async fn serve(
    brain: Arc<Brain>,
    listen: Arc<ListenService>,
    socket: &Path,
) -> anyhow::Result<()> {
    serve_with_speaker(brain, listen, None, socket).await
}

pub use crate::speaker::SpeakerState;

/// Comme [`serve`], avec la vérification du locuteur (`EnrollVoice`).
pub async fn serve_with_speaker(
    brain: Arc<Brain>,
    listen: Arc<ListenService>,
    speaker: Option<Arc<SpeakerGate>>,
    socket: &Path,
) -> anyhow::Result<()> {
    let state = speaker.map_or(SpeakerState::Disabled, SpeakerState::Ready);
    serve_with_state(brain, listen, state, socket).await
}

/// Comme [`serve_with_speaker`], en distinguant « désactivée » de « modèle introuvable ».
pub async fn serve_with_state(
    brain: Arc<Brain>,
    listen: Arc<ListenService>,
    speaker: SpeakerState,
    socket: &Path,
) -> anyhow::Result<()> {
    serve_with_voice(brain, listen, speaker, Voice::none(), socket).await
}

/// Comme [`serve_with_state`], avec la voix de Jarvis pour les réponses `Heard`.
pub async fn serve_with_voice(
    brain: Arc<Brain>,
    listen: Arc<ListenService>,
    speaker: SpeakerState,
    voice: Voice,
    socket: &Path,
) -> anyhow::Result<()> {
    let listener = bind(socket).await?;
    info!(socket = %socket.display(), mode = %brain.mode(), "cerveau prêt");
    loop {
        let (stream, _) = listener.accept().await.context("acceptation d'un client")?;
        let (brain, listen, speaker) = (brain.clone(), listen.clone(), speaker.clone());
        let voice = voice.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_client(brain, listen, speaker, voice, stream).await {
                debug!("connexion terminée : {e}");
            }
        });
    }
}

async fn bind(socket: &Path) -> anyhow::Result<UnixListener> {
    if let Some(parent) = socket.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("création de {}", parent.display()))?;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
            .with_context(|| format!("permissions de {}", parent.display()))?;
    }
    if socket.symlink_metadata().is_ok() {
        match UnixStream::connect(socket).await {
            Ok(_) => bail!("Un cerveau Jarvis tourne déjà."),
            Err(e)
                if e.kind() == ErrorKind::ConnectionRefused || e.kind() == ErrorKind::NotFound =>
            {
                warn!(socket = %socket.display(), "socket orphelin supprimé");
            }
            // Pas un socket (fichier ordinaire) ou autre erreur : orphelin aussi.
            Err(_) => {}
        }
        std::fs::remove_file(socket)
            .with_context(|| format!("suppression de {}", socket.display()))?;
    }
    let listener = UnixListener::bind(socket)
        .with_context(|| format!("ouverture du socket {}", socket.display()))?;
    std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

/// Taille maximale d'une ligne de requête. Pire cas permis par le serveur : un
/// enrôlement de 20 échantillons de 30 s (16 kHz, i16), soit 19,2 Mo d'audio et
/// 25,6 Mo en base64 ; l'oreille envoie en réalité environ 1 Mo.
pub const MAX_REQUEST_BYTES: usize = 32 * 1024 * 1024;

#[derive(Debug, PartialEq)]
enum Line {
    /// Ligne complète, sans le retour à la ligne.
    Request(Vec<u8>),
    /// Plus de `max` octets sans retour à la ligne.
    TooLong,
    /// Connexion fermée.
    Eof,
}

/// Lit une ligne d'au plus `max` octets, sans jamais en mettre plus en mémoire.
async fn read_request<R: AsyncBufRead + Unpin>(reader: &mut R, max: usize) -> io::Result<Line> {
    let mut buf = Vec::new();
    let limit = u64::try_from(max).unwrap_or(u64::MAX).saturating_add(1);
    let n = reader.take(limit).read_until(b'\n', &mut buf).await?;
    if n == 0 {
        return Ok(Line::Eof);
    }
    if buf.last() == Some(&b'\n') {
        buf.pop();
        return Ok(Line::Request(buf));
    }
    Ok(if buf.len() > max {
        Line::TooLong
    } else {
        Line::Request(buf)
    })
}

async fn handle_client(
    brain: Arc<Brain>,
    listen: Arc<ListenService>,
    speaker: SpeakerState,
    voice: Voice,
    stream: UnixStream,
) -> io::Result<()> {
    let (read, mut write) = stream.into_split();
    let mut reader = BufReader::new(read);
    loop {
        let response = match read_request(&mut reader, MAX_REQUEST_BYTES).await? {
            Line::Eof => return Ok(()),
            Line::TooLong => {
                warn!(
                    max = MAX_REQUEST_BYTES,
                    "requête trop grande, connexion fermée"
                );
                send(
                    &mut write,
                    &ControlResponse::Error {
                        message: "requête trop grande".into(),
                    },
                )
                .await?;
                return Ok(());
            }
            Line::Request(line) => match serde_json::from_slice::<ControlRequest>(&line) {
                Ok(request) => respond(&brain, &listen, &speaker, &voice, request).await,
                Err(_) => ControlResponse::Error {
                    message: "requête invalide".into(),
                },
            },
        };
        send(&mut write, &response).await?;
    }
}

async fn send<W: AsyncWrite + Unpin>(write: &mut W, response: &ControlResponse) -> io::Result<()> {
    let mut out = serde_json::to_string(response).map_err(io::Error::other)?;
    out.push('\n');
    write.write_all(out.as_bytes()).await
}

async fn respond(
    brain: &Brain,
    listen: &ListenService,
    speaker: &SpeakerState,
    voice: &Voice,
    request: ControlRequest,
) -> ControlResponse {
    match request {
        ControlRequest::Say { text } => ControlResponse::Reply {
            speech: brain.handle_text(&text).await.speech,
        },
        ControlRequest::Status => status(brain),
        ControlRequest::Wake => {
            listen.wake().await;
            status(brain)
        }
        ControlRequest::Listen { audio, muted, .. } => match decode_pcm(&audio) {
            Ok(samples) => voice.speak(listen.listen(&samples, muted).await).await,
            Err(_) => ControlResponse::Error {
                message: "audio invalide".into(),
            },
        },
        ControlRequest::EnrollVoice { samples } => enroll_voice(speaker, samples).await,
        ControlRequest::SetMode { mode } => match brain.set_mode(mode) {
            Ok(()) => status(brain),
            Err(e) => ControlResponse::Error {
                message: format!("Impossible d'enregistrer le mode : {e}."),
            },
        },
    }
}

fn status(brain: &Brain) -> ControlResponse {
    ControlResponse::Status {
        mode: brain.mode(),
        version: env!("CARGO_PKG_VERSION").into(),
        local_model: brain.local_model().into(),
    }
}

/// Bornes d'un enrôlement : au plus 20 échantillons de 30 s ; moins d'1 s, ignoré.
const MAX_ENROLL_SAMPLES: usize = 20;
const MAX_ENROLL_SECONDS: usize = 30;
const MIN_ENROLL_SECONDS: usize = 1;
const RATE: usize = 16_000;

async fn enroll_voice(speaker: &SpeakerState, samples: Vec<String>) -> ControlResponse {
    let error = |message: &str| ControlResponse::Error {
        message: message.into(),
    };
    let gate = match speaker {
        SpeakerState::Disabled => return error("L'empreinte vocale n'est pas activée."),
        SpeakerState::Unavailable { .. } => {
            return error(
                "Empreinte vocale indisponible : modèle introuvable (voir scripts/download-models.sh speaker).",
            );
        }
        SpeakerState::Ready(g) => g.clone(),
    };
    if samples.len() > MAX_ENROLL_SAMPLES {
        return error("Trop d'échantillons (20 au plus).");
    }
    let mut audio = Vec::with_capacity(samples.len());
    for s in &samples {
        match decode_pcm(s) {
            Ok(pcm) => {
                // Trop court : ignoré. Trop long : tronqué à 30 s.
                if pcm.len() < MIN_ENROLL_SECONDS * RATE {
                    continue;
                }
                audio.push(
                    pcm.iter()
                        .take(MAX_ENROLL_SECONDS * RATE)
                        .map(|&v| f32::from(v) / 32768.0)
                        .collect::<Vec<f32>>(),
                );
            }
            Err(_) => return error("audio invalide"),
        }
    }
    if audio.is_empty() {
        return error("Aucun échantillon exploitable (1 s au moins chacun).");
    }
    match tokio::task::spawn_blocking(move || gate.enroll(&audio)).await {
        Ok(Ok(n)) => {
            info!(echantillons = n, "empreinte vocale enregistrée");
            ControlResponse::Reply {
                speech: format!("Empreinte vocale enregistrée ({n} échantillons)."),
            }
        }
        Ok(Err(e)) => {
            warn!(erreur = %e, "enrôlement vocal impossible");
            ControlResponse::Error {
                message: format!("Impossible d'enregistrer l'empreinte vocale : {e}."),
            }
        }
        Err(_) => error("Enrôlement interrompu."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn lines(mut input: &[u8], max: usize) -> Vec<Line> {
        let mut out = Vec::new();
        loop {
            match read_request(&mut input, max).await.unwrap() {
                Line::Eof => return out,
                l => out.push(l),
            }
        }
    }

    #[tokio::test]
    async fn ligne_de_requete_bornee() {
        assert_eq!(
            lines(b"abcd\nef\n", 4).await,
            [
                Line::Request(b"abcd".to_vec()),
                Line::Request(b"ef".to_vec())
            ]
        );
        // Dernière ligne sans retour : gardée, comme avant.
        assert_eq!(lines(b"ab", 4).await, [Line::Request(b"ab".to_vec())]);
        // Une ligne trop longue s'arrête à la borne, sans tout lire.
        let mut input: &[u8] = b"abcde\nf\n";
        assert_eq!(read_request(&mut input, 4).await.unwrap(), Line::TooLong);
        assert_eq!(input, b"\nf\n");
    }

    #[test]
    fn borne_couvre_le_pire_enrolement() {
        // 20 échantillons de 30 s, 16 kHz, i16, en base64, plus l'enveloppe JSON.
        let audio = MAX_ENROLL_SAMPLES * MAX_ENROLL_SECONDS * RATE * 2;
        assert!(audio.div_ceil(3) * 4 + 4096 <= MAX_REQUEST_BYTES);
    }
}
