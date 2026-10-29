//! Client du socket de contrôle du cerveau (une connexion par requête). L'oreille ne
//! s'en sert plus que pour l'appairage local (`PairLocal`) et l'enrôlement vocal
//! (`EnrollVoice`) ; les phrases et les annonces passent par le réseau (voir
//! [`crate::link`]).

use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::Duration;

use jarvis_protocol::{ControlRequest, ControlResponse};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

/// Délai maximal d'une requête.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Attente maximale de la réponse à une phrase : le pire cas du cerveau, plus une marge.
///
/// ```text
///   reconnaissance vocale (marge)           2 s
/// + choix de l'outil par Ollama            20 s  (llm/ollama.rs, TIMEOUT)
/// + compétence la plus lente : recherche   15 s  (SearXNG 4 s + résumé local 10 s + 1 s)
/// + synthèse vocale                         8 s  (tts/mod.rs, SYNTH_DEADLINE)
/// = 45 s, arrondi à 50 s
/// ```
///
/// Vérifié côté cerveau par le test `budget_ecoute` de jarvis-brain (qui lit cette ligne).
const LISTEN_TIMEOUT: Duration = Duration::from_secs(50);

/// Au-delà, la réponse attendue n'est plus attendue (les annonces peuvent passer).
pub const RESPONSE_TIMEOUT: Duration = LISTEN_TIMEOUT;

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("cerveau injoignable")]
    BrainUnreachable,
    #[error("le cerveau n'a pas répondu à temps")]
    Timeout,
    #[error("erreur de communication : {0}")]
    Io(#[from] std::io::Error),
    #[error("réponse illisible : {0}")]
    Protocol(String),
}

/// Socket de contrôle du cerveau (remplaçable dans les tests).
#[async_trait::async_trait]
pub trait Brain: Send + Sync + 'static {
    async fn request(&self, req: ControlRequest) -> Result<ControlResponse, ClientError>;
}

/// Cerveau joint par son socket de contrôle.
pub struct SocketBrain {
    pub socket: PathBuf,
}

#[async_trait::async_trait]
impl Brain for SocketBrain {
    async fn request(&self, req: ControlRequest) -> Result<ControlResponse, ClientError> {
        request(&self.socket, &req).await
    }
}

/// Envoie `req` au cerveau et lit la réponse.
pub async fn request(socket: &Path, req: &ControlRequest) -> Result<ControlResponse, ClientError> {
    tokio::time::timeout(REQUEST_TIMEOUT, exchange(socket, req))
        .await
        .map_err(|_| ClientError::Timeout)?
}

async fn exchange(socket: &Path, req: &ControlRequest) -> Result<ControlResponse, ClientError> {
    // Dossier à soi et fermé aux autres : sinon, ce qui répond pourrait ne pas être le
    // cerveau (et l'appairage local lui ferait confiance).
    match jarvis_protocol::check_socket_dir(socket) {
        Ok(()) => {}
        Err(e) if e.kind() == ErrorKind::NotFound => return Err(ClientError::BrainUnreachable),
        Err(e) => return Err(e.into()),
    }
    let stream = match UnixStream::connect(socket).await {
        Ok(s) => s,
        Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::ConnectionRefused) => {
            return Err(ClientError::BrainUnreachable);
        }
        Err(e) => return Err(e.into()),
    };
    let (read, mut write) = stream.into_split();
    let mut line = serde_json::to_string(req).map_err(|e| ClientError::Protocol(e.to_string()))?;
    line.push('\n');
    write.write_all(line.as_bytes()).await?;
    let mut lines = BufReader::new(read).lines();
    let answer = lines
        .next_line()
        .await?
        .ok_or_else(|| ClientError::Protocol("connexion fermée sans réponse".into()))?;
    // Pas le texte dans l'erreur : il peut contenir un jeton (`PairedLocal`).
    serde_json::from_str(&answer).map_err(|e| ClientError::Protocol(format!("{:?}", e.classify())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use tokio::net::UnixListener;

    /// Dossier fermé aux autres, comme celui du vrai cerveau.
    fn private_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        dir
    }

    #[test]
    fn attente_de_la_reponse_cinquante_secondes() {
        assert_eq!(RESPONSE_TIMEOUT, Duration::from_secs(50));
        assert!(RESPONSE_TIMEOUT > REQUEST_TIMEOUT);
    }

    #[tokio::test]
    async fn socket_absent_cerveau_injoignable() {
        let dir = private_dir();
        let err = request(&dir.path().join("absent.sock"), &ControlRequest::Status)
            .await
            .unwrap_err();
        assert!(matches!(err, ClientError::BrainUnreachable), "{err:?}");
    }

    #[tokio::test]
    async fn dossier_du_socket_ouvert_aux_autres_refuse() {
        let dir = private_dir();
        let path = dir.path().join("b.sock");
        let _listener = UnixListener::bind(&path).unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        let err = request(&path, &ControlRequest::PairLocal { name: "PC".into() })
            .await
            .unwrap_err();
        assert!(
            matches!(&err, ClientError::Io(e) if e.kind() == ErrorKind::PermissionDenied),
            "{err:?}"
        );
        assert!(err.to_string().contains("peu sûr"), "{err}");
    }

    #[tokio::test]
    async fn aller_retour_sur_un_faux_serveur() {
        let dir = private_dir();
        let path = dir.path().join("b.sock");
        let listener = UnixListener::bind(&path).unwrap();
        tokio::spawn(async move {
            let (s, _) = listener.accept().await.unwrap();
            let (r, mut w) = s.into_split();
            let line = BufReader::new(r)
                .lines()
                .next_line()
                .await
                .unwrap()
                .unwrap();
            assert_eq!(line, r#"{"type":"pair_local","name":"PC"}"#);
            w.write_all(b"{\"type\":\"reply\",\"speech\":\"ok\"}\n")
                .await
                .unwrap();
        });
        let resp = request(&path, &ControlRequest::PairLocal { name: "PC".into() })
            .await
            .unwrap();
        assert_eq!(
            resp,
            ControlResponse::Reply {
                speech: "ok".into()
            }
        );
    }
}
