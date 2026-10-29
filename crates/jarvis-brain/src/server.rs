//! Serveur du socket de contrôle : une ligne JSON par requête, une par réponse.
//!
//! Le socket sert au CLI, à l'enrôlement vocal, à l'appairage (`jarvis pair`, oreille
//! du PC) et aux annonces. Aucune phrase n'y passe : elles arrivent toutes par le
//! réseau des oreilles ([`crate::net`]), où l'arbitre n'en retient qu'une.

use crate::announce::AnnouncementBoard;
use crate::brain::{Brain, SystemClock};
use crate::listen::ListenService;
use crate::net::{Hub, PairOutcome};
use crate::speaker::SpeakerGate;
use crate::tts::Voice;
use anyhow::{Context, bail};
use jarvis_protocol::{
    Announcement, ControlRequest, ControlResponse, DEFAULT_EAR, Sound, decode_pcm, pairing_ended,
};
use jarvis_skills::{Outcome, SystemRunner};
use std::io::{self, ErrorKind};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
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
    // Tableau neuf : personne n'y annonce rien.
    let board = Arc::new(AnnouncementBoard::new(
        Arc::new(SystemRunner),
        Arc::new(SystemClock),
    ));
    serve_with_board(brain, listen, speaker, voice, board, socket).await
}

/// Comme [`serve_with_voice`], avec le tableau des annonces relevées par les oreilles.
pub async fn serve_with_board(
    brain: Arc<Brain>,
    listen: Arc<ListenService>,
    speaker: SpeakerState,
    voice: Voice,
    board: Arc<AnnouncementBoard>,
    socket: &Path,
) -> anyhow::Result<()> {
    serve_with_hub(brain, listen, speaker, voice, board, None, socket).await
}

/// Comme [`serve_with_board`], avec le hub des oreilles en réseau (appairage,
/// liste et retrait des oreilles). Sans hub : ces requêtes rendent une erreur.
///
/// `_listen` ne sert plus ici (les phrases passent par le hub) ; gardé pour ne pas
/// changer la signature des appelants.
pub async fn serve_with_hub(
    brain: Arc<Brain>,
    _listen: Arc<ListenService>,
    speaker: SpeakerState,
    voice: Voice,
    board: Arc<AnnouncementBoard>,
    hub: Option<Hub>,
    socket: &Path,
) -> anyhow::Result<()> {
    let listener = bind(socket).await?;
    info!(socket = %socket.display(), mode = %brain.mode(), "cerveau prêt");
    let shared = Arc::new(Shared {
        brain,
        speaker,
        voice,
        board,
        hub,
    });
    loop {
        let (stream, _) = listener.accept().await.context("acceptation d'un client")?;
        let shared = shared.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_client(shared, stream).await {
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

/// Ce que partagent tous les clients.
struct Shared {
    brain: Arc<Brain>,
    speaker: SpeakerState,
    voice: Voice,
    board: Arc<AnnouncementBoard>,
    hub: Option<Hub>,
}

/// Réseau des oreilles absent.
const NO_NETWORK: &str = "Le réseau des oreilles est désactivé (voir les journaux du cerveau).";

async fn handle_client(shared: Arc<Shared>, stream: UnixStream) -> io::Result<()> {
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
                Ok(ControlRequest::PairStart) => match &shared.hub {
                    Some(hub) => {
                        if pair(hub, &mut reader, &mut write).await? {
                            continue;
                        }
                        return Ok(());
                    }
                    None => ControlResponse::Error {
                        message: NO_NETWORK.into(),
                    },
                },
                Ok(request) => respond(&shared, request).await,
                Err(_) => ControlResponse::Error {
                    message: "requête invalide".into(),
                },
            },
        };
        send(&mut write, &response).await?;
    }
}

/// Marge au-delà de la fenêtre : le hub la ferme lui-même à l'échéance.
const PAIR_GRACE: Duration = Duration::from_secs(5);

/// `PairStart` : `Pairing`, puis `PairingDone` ou `PairingEnded` sur la même
/// connexion. Client parti (ou qui écrit) : fenêtre fermée, `false`.
async fn pair<R, W>(hub: &Hub, reader: &mut R, write: &mut W) -> io::Result<bool>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let ticket = match hub.pair_start().await {
        Ok(ticket) => ticket,
        Err(message) => {
            send(write, &ControlResponse::Error { message }).await?;
            return Ok(true);
        }
    };
    let id = ticket.id;
    let opened = ControlResponse::Pairing {
        code: ticket.code,
        uri: ticket.uri,
        fingerprint_short: ticket.fingerprint_short,
        expires_in_secs: ticket.expires_in_secs,
    };
    if let Err(e) = send(write, &opened).await {
        hub.pair_cancel(id).await;
        return Err(e);
    }
    let guard = Duration::from_secs(ticket.expires_in_secs) + PAIR_GRACE;
    let ended = |reason: &str| ControlResponse::PairingEnded {
        reason: reason.into(),
    };
    let last = tokio::select! {
        outcome = ticket.outcome => match outcome {
            Ok(PairOutcome::Done { name }) => ControlResponse::PairingDone { name },
            Ok(PairOutcome::Ended { reason }) => ended(&reason),
            Err(_) => ended(pairing_ended::BRAIN_STOPPING),
        },
        _ = reader.fill_buf() => {
            // Ctrl-C de `jarvis pair` : connexion fermée.
            hub.pair_cancel(id).await;
            return Ok(false);
        }
        () = tokio::time::sleep(guard) => {
            hub.pair_cancel(id).await;
            ended(pairing_ended::TIMED_OUT)
        }
    };
    send(write, &last).await?;
    Ok(true)
}

async fn send<W: AsyncWrite + Unpin>(write: &mut W, response: &ControlResponse) -> io::Result<()> {
    let mut out = serde_json::to_string(response).map_err(io::Error::other)?;
    out.push('\n');
    write.write_all(out.as_bytes()).await
}

async fn respond(shared: &Shared, request: ControlRequest) -> ControlResponse {
    let Shared {
        brain,
        speaker,
        voice,
        board,
        hub,
    } = shared;
    match request {
        ControlRequest::Say { text } => ControlResponse::Reply {
            speech: brain.handle_text(&text).await.speech,
        },
        ControlRequest::Status => status(brain),
        ControlRequest::Announcements { ear } => {
            announcements(board, voice, ear.as_deref().unwrap_or(DEFAULT_EAR)).await
        }
        ControlRequest::EnrollVoice { samples } => enroll_voice(speaker, samples).await,
        ControlRequest::SetMode { mode } => match brain.set_mode(mode) {
            Ok(()) => status(brain),
            Err(e) => ControlResponse::Error {
                message: format!("Impossible d'enregistrer le mode : {e}."),
            },
        },
        // Traité par `handle_client` (réponse en deux temps).
        ControlRequest::PairStart => ControlResponse::Error {
            message: "requête invalide".into(),
        },
        ControlRequest::PairLocal { name } => match hub {
            Some(hub) => hub.pair_local(name).await,
            None => no_network(),
        },
        ControlRequest::ListEars => match hub {
            Some(hub) => ControlResponse::Ears {
                ears: hub.list_ears().await,
            },
            None => no_network(),
        },
        ControlRequest::Unpair { ear } => match hub {
            Some(hub) => hub.unpair(ear).await,
            None => no_network(),
        },
    }
}

/// Temps total de synthèse des annonces d'une relève : l'oreille abandonne une
/// requête à 30 s. Au-delà, les annonces restantes partent sans voix.
const ANNOUNCE_SYNTH_BUDGET: Duration = Duration::from_secs(20);

fn no_network() -> ControlResponse {
    ControlResponse::Error {
        message: NO_NETWORK.into(),
    }
}

/// Annonces de l'oreille `ear`, avec voix et bip.
async fn announcements(board: &AnnouncementBoard, voice: &Voice, ear: &str) -> ControlResponse {
    ControlResponse::Announcements {
        items: voiced(voice, board.take_for(ear)).await,
    }
}

/// Voix et bip des annonces relevées (synthèse bornée à 20 s en tout).
pub(crate) async fn voiced(voice: &Voice, taken: Vec<(String, Outcome)>) -> Vec<Announcement> {
    let deadline = tokio::time::Instant::now() + ANNOUNCE_SYNTH_BUDGET;
    let mut items = Vec::new();
    for (speech, outcome) in taken {
        let audio = tokio::time::timeout_at(deadline, voice.audio_for(&speech))
            .await
            .ok()
            .flatten();
        let sound = match outcome {
            Outcome::Done => Some(Sound::Success),
            Outcome::Failed => Some(Sound::Failure),
            Outcome::Info => None,
        };
        items.push(Announcement {
            speech,
            audio,
            sound,
        });
    }
    items
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
