//! Client du socket de contrôle du cerveau : une connexion par requête.

use std::io::ErrorKind;
use std::path::Path;
use std::time::Duration;

use jarvis_protocol::{ControlRequest, ControlResponse};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

/// Délai maximal d'une requête (sauf `Listen`).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Délai maximal d'une phrase (`Listen`) : le pire cas du cerveau, plus une marge.
///
/// ```text
///   reconnaissance vocale (marge)           2 s
/// + choix de l'outil par Ollama            20 s  (llm/ollama.rs, TIMEOUT)
/// + compétence la plus lente : recherche   15 s  (SearXNG 4 s + résumé local 10 s + 1 s)
/// + synthèse vocale                         8 s  (tts/mod.rs, SYNTH_DEADLINE)
/// = 45 s, arrondi à 50 s
/// ```
///
/// Vérifié côté cerveau par le test `budget_ecoute` de jarvis-brain.
const LISTEN_TIMEOUT: Duration = Duration::from_secs(50);

/// Délai accordé à `req`.
fn timeout_for(req: &ControlRequest) -> Duration {
    match req {
        ControlRequest::Listen { .. } => LISTEN_TIMEOUT,
        _ => REQUEST_TIMEOUT,
    }
}

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
    tokio::time::timeout(timeout_for(req), exchange(socket, req))
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

    fn listen() -> ControlRequest {
        ControlRequest::Listen {
            audio: String::new(),
            muted: false,
            follow_up: false,
        }
    }

    #[test]
    fn ecoute_a_son_propre_delai_plus_long() {
        assert_eq!(timeout_for(&listen()), LISTEN_TIMEOUT);
        assert_eq!(LISTEN_TIMEOUT, Duration::from_secs(50));
        assert!(LISTEN_TIMEOUT > REQUEST_TIMEOUT);
    }

    #[test]
    fn autres_requetes_gardent_trente_secondes() {
        for req in [
            ControlRequest::Status,
            ControlRequest::Wake,
            ControlRequest::Say { text: "x".into() },
            ControlRequest::EnrollVoice { samples: vec![] },
        ] {
            assert_eq!(timeout_for(&req), Duration::from_secs(30), "{req:?}");
        }
    }

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
