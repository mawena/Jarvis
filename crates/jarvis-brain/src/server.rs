//! Serveur du socket de contrôle : une ligne JSON par requête, une par réponse.

use crate::brain::Brain;
use anyhow::{Context, bail};
use jarvis_protocol::{ControlRequest, ControlResponse};
use std::io::ErrorKind;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tracing::{debug, info, warn};

/// Écoute sur `socket` jusqu'à une erreur d'acceptation. Chaque client a sa tâche.
pub async fn serve(brain: Arc<Brain>, socket: &Path) -> anyhow::Result<()> {
    let listener = bind(socket).await?;
    info!(socket = %socket.display(), mode = %brain.mode(), "cerveau prêt");
    loop {
        let (stream, _) = listener.accept().await.context("acceptation d'un client")?;
        let brain = brain.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_client(brain, stream).await {
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

async fn handle_client(brain: Arc<Brain>, stream: UnixStream) -> std::io::Result<()> {
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    while let Some(line) = lines.next_line().await? {
        let response = match serde_json::from_str::<ControlRequest>(&line) {
            Ok(request) => respond(&brain, request).await,
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

async fn respond(brain: &Brain, request: ControlRequest) -> ControlResponse {
    match request {
        ControlRequest::Say { text } => ControlResponse::Reply {
            speech: brain.handle_text(&text).await.speech,
        },
        ControlRequest::Status => status(brain),
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
    }
}
