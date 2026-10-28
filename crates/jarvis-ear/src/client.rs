//! Client du socket de contrôle du cerveau : une connexion par requête.

use std::io::ErrorKind;
use std::path::Path;
use std::time::Duration;

use jarvis_protocol::{ControlRequest, ControlResponse};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

/// Délai maximal d'une requête.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

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

/// Envoie `req` au cerveau et lit la réponse.
pub async fn request(socket: &Path, req: &ControlRequest) -> Result<ControlResponse, ClientError> {
    tokio::time::timeout(REQUEST_TIMEOUT, exchange(socket, req))
        .await
        .map_err(|_| ClientError::Timeout)?
}

async fn exchange(socket: &Path, req: &ControlRequest) -> Result<ControlResponse, ClientError> {
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
    serde_json::from_str(&answer).map_err(|e| ClientError::Protocol(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::UnixListener;

    #[tokio::test]
    async fn socket_absent_cerveau_injoignable() {
        let dir = tempfile::tempdir().unwrap();
        let err = request(&dir.path().join("absent.sock"), &ControlRequest::Status)
            .await
            .unwrap_err();
        assert!(matches!(err, ClientError::BrainUnreachable), "{err:?}");
    }

    #[tokio::test]
    async fn aller_retour_sur_un_faux_serveur() {
        let dir = tempfile::tempdir().unwrap();
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
            assert_eq!(line, r#"{"type":"wake"}"#);
            w.write_all(b"{\"type\":\"reply\",\"speech\":\"ok\"}\n")
                .await
                .unwrap();
        });
        let resp = request(&path, &ControlRequest::Wake).await.unwrap();
        assert_eq!(
            resp,
            ControlResponse::Reply {
                speech: "ok".into()
            }
        );
    }
}
