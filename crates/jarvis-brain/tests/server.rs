use jarvis_brain::brain::Brain;
use jarvis_brain::llm::Understanding;
use jarvis_brain::mode_store::ModeStore;
use jarvis_brain::server::serve;
use jarvis_brain::testing::{FakeLlm, FakeSkill};
use jarvis_protocol::{ControlRequest, ControlResponse, Mode};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

fn brain(dir: &Path) -> Arc<Brain> {
    let llm = || Arc::new(FakeLlm::new(Ok(Understanding::Answer("x".into()))));
    Arc::new(Brain::new(
        vec![Arc::new(FakeSkill::new(&["media_next"]))],
        llm(),
        llm(),
        Arc::new(ModeStore::new(dir.join("mode"), Mode::Local)),
    ))
}

async fn start(dir: &Path) -> PathBuf {
    let socket = dir.join("run/brain.sock");
    let (b, s) = (brain(dir), socket.clone());
    tokio::spawn(async move { serve(b, &s).await });
    for _ in 0..100 {
        if UnixStream::connect(&socket).await.is_ok() {
            return socket;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("le serveur ne démarre pas");
}

struct Client(BufReader<UnixStream>);

impl Client {
    async fn connect(socket: &Path) -> Client {
        Client(BufReader::new(UnixStream::connect(socket).await.unwrap()))
    }

    async fn raw(&mut self, line: &str) -> ControlResponse {
        self.0
            .get_mut()
            .write_all(format!("{line}\n").as_bytes())
            .await
            .unwrap();
        let mut answer = String::new();
        self.0.read_line(&mut answer).await.unwrap();
        serde_json::from_str(&answer).unwrap()
    }

    async fn send(&mut self, req: ControlRequest) -> ControlResponse {
        self.raw(&serde_json::to_string(&req).unwrap()).await
    }
}

#[tokio::test]
async fn say_renvoie_la_reponse() {
    let dir = tempfile::tempdir().unwrap();
    let socket = start(dir.path()).await;
    let mut c = Client::connect(&socket).await;
    let r = c.raw(r#"{"type":"say","text":"suivant"}"#).await;
    assert_eq!(
        r,
        ControlResponse::Reply {
            speech: "ok".into()
        }
    );
}

#[tokio::test]
async fn status_et_set_mode() {
    let dir = tempfile::tempdir().unwrap();
    let socket = start(dir.path()).await;
    let mut c = Client::connect(&socket).await;
    c.send(ControlRequest::SetMode { mode: Mode::Online }).await;
    let r = c.send(ControlRequest::Status).await;
    assert_eq!(
        r,
        ControlResponse::Status {
            mode: Mode::Online,
            version: env!("CARGO_PKG_VERSION").into(),
            local_model: String::new()
        }
    );
}

#[tokio::test]
async fn ligne_invalide_n_interrompt_pas() {
    let dir = tempfile::tempdir().unwrap();
    let socket = start(dir.path()).await;
    let mut c = Client::connect(&socket).await;
    assert_eq!(
        c.raw("pas du json").await,
        ControlResponse::Error {
            message: "requête invalide".into()
        }
    );
    assert!(matches!(
        c.send(ControlRequest::Status).await,
        ControlResponse::Status { .. }
    ));
}

#[tokio::test]
async fn socket_orphelin_remplace() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("run")).unwrap();
    std::fs::write(dir.path().join("run/brain.sock"), b"vieux").unwrap();
    let socket = start(dir.path()).await;
    let mut c = Client::connect(&socket).await;
    assert!(matches!(
        c.send(ControlRequest::Status).await,
        ControlResponse::Status { .. }
    ));
}

#[tokio::test]
async fn cerveau_deja_lance_refuse() {
    let dir = tempfile::tempdir().unwrap();
    let socket = start(dir.path()).await;
    let err = serve(brain(dir.path()), &socket).await.unwrap_err();
    assert_eq!(err.to_string(), "Un cerveau Jarvis tourne déjà.");
}

#[tokio::test]
async fn permissions_du_socket() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let socket = start(dir.path()).await;
    let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&socket), 0o600);
    assert_eq!(mode(socket.parent().unwrap()), 0o700);
}
