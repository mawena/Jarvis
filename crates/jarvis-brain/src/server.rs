//! Serveur du socket de contrôle : une ligne JSON par requête, une par réponse.

use crate::brain::Brain;
use crate::listen::ListenService;
use crate::speaker::SpeakerGate;
use anyhow::{Context, bail};
use jarvis_protocol::{ControlRequest, ControlResponse, decode_pcm};
use std::io::ErrorKind;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
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

/// Comme [`serve`], avec la vérification du locuteur (`EnrollVoice`).
pub async fn serve_with_speaker(
    brain: Arc<Brain>,
    listen: Arc<ListenService>,
    speaker: Option<Arc<SpeakerGate>>,
    socket: &Path,
) -> anyhow::Result<()> {
    let listener = bind(socket).await?;
    info!(socket = %socket.display(), mode = %brain.mode(), "cerveau prêt");
    loop {
        let (stream, _) = listener.accept().await.context("acceptation d'un client")?;
        let (brain, listen, speaker) = (brain.clone(), listen.clone(), speaker.clone());
        tokio::spawn(async move {
            if let Err(e) = handle_client(brain, listen, speaker, stream).await {
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

async fn handle_client(
    brain: Arc<Brain>,
    listen: Arc<ListenService>,
    speaker: Option<Arc<SpeakerGate>>,
    stream: UnixStream,
) -> std::io::Result<()> {
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    while let Some(line) = lines.next_line().await? {
        let response = match serde_json::from_str::<ControlRequest>(&line) {
            Ok(request) => respond(&brain, &listen, speaker.as_ref(), request).await,
            Err(_) => ControlResponse::Error {
                message: "requête invalide".into(),
            },
        };
        let mut out = serde_json::to_string(&response).map_err(std::io::Error::other)?;
        out.push('\n');
        write.write_all(out.as_bytes()).await?;
    }
    Ok(())
}

async fn respond(
    brain: &Brain,
    listen: &ListenService,
    speaker: Option<&Arc<SpeakerGate>>,
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
        ControlRequest::Listen { audio, muted } => match decode_pcm(&audio) {
            Ok(samples) => listen.listen(&samples, muted).await,
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

async fn enroll_voice(speaker: Option<&Arc<SpeakerGate>>, samples: Vec<String>) -> ControlResponse {
    let error = |message: &str| ControlResponse::Error {
        message: message.into(),
    };
    let Some(gate) = speaker else {
        return error("L'empreinte vocale n'est pas activée.");
    };
    let mut audio = Vec::with_capacity(samples.len());
    for s in &samples {
        match decode_pcm(s) {
            Ok(pcm) => audio.push(
                pcm.iter()
                    .map(|&v| f32::from(v) / 32768.0)
                    .collect::<Vec<f32>>(),
            ),
            Err(_) => return error("audio invalide"),
        }
    }
    let gate = gate.clone();
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
