use jarvis_brain::brain::Brain;
use jarvis_brain::listen::ListenService;
use jarvis_brain::llm::Understanding;
use jarvis_brain::mode_store::ModeStore;
use jarvis_brain::server::serve;
use jarvis_brain::testing::{FakeDucker, FakeLlm, FakeSkill, FakeTranscriber};
use jarvis_protocol::{ControlRequest, ControlResponse, Mode, encode_pcm};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

fn brain(dir: &Path) -> Arc<Brain> {
    let llm = || Arc::new(FakeLlm::new(Ok(Understanding::Answer("x".into()))));
    Arc::new(
        Brain::new(
            vec![Arc::new(FakeSkill::new(&["media_next"]))],
            llm(),
            llm(),
            Arc::new(ModeStore::new(dir.join("mode"), Mode::Local)),
        )
        .with_local_model("qwen-test"),
    )
}

fn listen(b: &Arc<Brain>) -> Arc<ListenService> {
    Arc::new(ListenService::new(
        b.clone(),
        Arc::new(FakeTranscriber::new("suivant")),
        Arc::new(FakeDucker::new()),
    ))
}

async fn start(dir: &Path) -> PathBuf {
    let socket = dir.join("run/brain.sock");
    let (b, s) = (brain(dir), socket.clone());
    let l = listen(&b);
    tokio::spawn(async move { serve(b, l, &s).await });
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
            local_model: "qwen-test".into()
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
async fn requete_trop_grande_refusee_et_connexion_fermee() {
    use jarvis_brain::server::MAX_REQUEST_BYTES;
    let dir = tempfile::tempdir().unwrap();
    let socket = start(dir.path()).await;
    let mut c = Client::connect(&socket).await;
    let big = vec![b'a'; MAX_REQUEST_BYTES + 1];
    c.0.get_mut().write_all(&big).await.unwrap();
    let mut answer = String::new();
    c.0.read_line(&mut answer).await.unwrap();
    assert_eq!(
        serde_json::from_str::<ControlResponse>(&answer).unwrap(),
        ControlResponse::Error {
            message: "requête trop grande".into()
        }
    );
    answer.clear();
    assert_eq!(c.0.read_line(&mut answer).await.unwrap(), 0);
    // Le serveur continue d'accepter d'autres clients.
    assert!(matches!(
        Client::connect(&socket)
            .await
            .send(ControlRequest::Status)
            .await,
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
    let b = brain(dir.path());
    let err = serve(b.clone(), listen(&b), &socket).await.unwrap_err();
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

#[tokio::test]
async fn server_listen_et_status_local_model() {
    let dir = tempfile::tempdir().unwrap();
    let socket = start(dir.path()).await;
    let mut c = Client::connect(&socket).await;
    assert!(matches!(
        c.send(ControlRequest::Wake).await,
        ControlResponse::Status { .. }
    ));
    let r = c
        .send(ControlRequest::Listen {
            audio: encode_pcm(&[1, 2, 3]),
            muted: false,
        })
        .await;
    assert_eq!(
        r,
        ControlResponse::Heard {
            text: "suivant".into(),
            speech: "ok".into(),
            ear_action: None
        }
    );
    let r = c
        .send(ControlRequest::Listen {
            audio: "!!".into(),
            muted: false,
        })
        .await;
    assert!(matches!(r, ControlResponse::Error { .. }));
    match c.send(ControlRequest::Status).await {
        ControlResponse::Status { local_model, .. } => assert_eq!(local_model, "qwen-test"),
        other => panic!("{other:?}"),
    }
}

/// Une seconde d'audio (i16 16 kHz) dont le premier échantillon désigne l'empreinte factice.
fn second(id: i16) -> Vec<i16> {
    let mut a = vec![0i16; 16_000];
    a[0] = id;
    a
}

#[tokio::test]
async fn enroll_voice_modele_introuvable() {
    use jarvis_brain::server::{SpeakerState, serve_with_state};
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("run/brain.sock");
    let b = brain(dir.path());
    let l = listen(&b);
    let s = socket.clone();
    tokio::spawn(async move {
        serve_with_state(b, l, SpeakerState::Unavailable { enrolled: true }, &s).await
    });
    for _ in 0..100 {
        if UnixStream::connect(&socket).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let r = Client::connect(&socket)
        .await
        .send(ControlRequest::EnrollVoice {
            samples: vec![encode_pcm(&second(0))],
        })
        .await;
    assert_eq!(
        r,
        ControlResponse::Error {
            message: "Empreinte vocale indisponible : modèle introuvable (voir scripts/download-models.sh speaker).".into()
        }
    );
}

#[tokio::test]
async fn enroll_voice_par_le_socket() {
    use jarvis_brain::server::serve_with_speaker;
    use jarvis_brain::speaker::SpeakerGate;
    use jarvis_brain::testing::FakeVerifier;

    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("run/brain.sock");
    let b = brain(dir.path());
    let l = listen(&b);
    let print = dir.path().join("voiceprint/charles.json");
    let gate = Arc::new(SpeakerGate::new(
        Arc::new(FakeVerifier::new(vec![vec![1.0, 0.0]])),
        print.clone(),
        0.55,
        0.65,
    ));
    let s = socket.clone();
    tokio::spawn(async move { serve_with_speaker(b, l, Some(gate), &s).await });
    for _ in 0..100 {
        if UnixStream::connect(&socket).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let mut c = Client::connect(&socket).await;
    let r = c
        .send(ControlRequest::EnrollVoice {
            // Un échantillon de moins d'1 s est ignoré.
            samples: vec![
                encode_pcm(&second(0)),
                encode_pcm(&second(0)),
                encode_pcm(&[0, 5]),
            ],
        })
        .await;
    assert_eq!(
        r,
        ControlResponse::Reply {
            speech: "Empreinte vocale enregistrée (2 échantillons).".into()
        }
    );
    assert!(print.is_file());
    // Aucun échantillon valide : erreur ; trop d'échantillons : erreur.
    let r = c
        .send(ControlRequest::EnrollVoice {
            samples: vec![encode_pcm(&[0, 5])],
        })
        .await;
    assert!(matches!(r, ControlResponse::Error { .. }), "{r:?}");
    let r = c
        .send(ControlRequest::EnrollVoice {
            samples: vec![encode_pcm(&second(0)); 21],
        })
        .await;
    assert!(matches!(r, ControlResponse::Error { .. }), "{r:?}");
    let r = c
        .send(ControlRequest::EnrollVoice {
            samples: vec!["!!".into()],
        })
        .await;
    assert!(matches!(r, ControlResponse::Error { .. }));
    // Sans vérification du locuteur : erreur claire.
    let socket2 = start(dir.path().join("b").as_path()).await;
    let r = Client::connect(&socket2)
        .await
        .send(ControlRequest::EnrollVoice { samples: vec![] })
        .await;
    assert!(matches!(r, ControlResponse::Error { .. }));
}
