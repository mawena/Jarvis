//! Serveur WebSocket TLS des oreilles et hub : vrai TLS (identité du cerveau dans un
//! dossier temporaire), `127.0.0.1` port 0, client TLS épinglé de `jarvis-ear-core`.

use futures_util::{SinkExt, StreamExt};
use jarvis_brain::announce::AnnouncementBoard;
use jarvis_brain::brain::{Brain, SystemClock};
use jarvis_brain::config::NetworkConfig;
use jarvis_brain::ears::EarRegistry;
use jarvis_brain::listen::ListenService;
use jarvis_brain::llm::Understanding;
use jarvis_brain::mode_store::ModeStore;
use jarvis_brain::net::{self, Hub, HubSettings, Limits, NetworkPaths, Services};
use jarvis_brain::server::{SpeakerState, serve_with_hub};
use jarvis_brain::testing::{FakeDucker, FakeLlm, FakeSkill, FakeSynthesizer, FakeTranscriber};
use jarvis_brain::tls::BrainIdentity;
use jarvis_brain::tts::{Speech, Voice};
use jarvis_ear_core::net::tls::{TlsConnector, pinned_client_config};
use jarvis_ear_core::net::{
    AsyncStream, Connector, EarLink, LinkConfig, LinkState, PairError, Pairing, PairingStore,
    pair_with_code,
};
use jarvis_protocol::ear::{BrainMessage, EarMessage, PROTOCOL_VERSION, RejectReason};
use jarvis_protocol::frame::encode_mic_frame;
use jarvis_protocol::pairing_uri::PairingUri;
use jarvis_protocol::{
    ControlRequest, ControlResponse, EarId, Fingerprint, Mode, SpeechAudio, encode_pcm,
};
use jarvis_skills::{Announcer, FakeRunner, Notice, Outcome};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, UnixStream};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;

type Ws = WebSocketStream<Box<dyn AsyncStream>>;

/// Cerveau complet sur `127.0.0.1:0` (fenêtre de l'arbitre : 50 ms) et son socket Unix.
struct Rig {
    _dir: tempfile::TempDir,
    addr: SocketAddr,
    fingerprint: Fingerprint,
    hub: Hub,
    ducker: Arc<FakeDucker>,
    stt: Arc<FakeTranscriber>,
    skill: Arc<FakeSkill>,
    board: Arc<AnnouncementBoard>,
    socket: PathBuf,
    /// Oreille appairée d'avance : (id, jeton).
    salon: (EarId, String),
}

fn limits() -> Limits {
    Limits {
        handshake_timeout: Duration::from_millis(300),
        ..Limits::default()
    }
}

async fn rig() -> Rig {
    rig_with(Duration::ZERO, net::hub::PROCESS_TIMEOUT).await
}

/// `skill_delay` : durée de la compétence « suivant » ; `process_timeout` : traitement
/// d'une phrase au plus.
async fn rig_with(skill_delay: Duration, process_timeout: Duration) -> Rig {
    rig_full(skill_delay, process_timeout, Voice::none()).await
}

/// Comme [`rig_with`], avec la voix des réponses et des annonces.
async fn rig_full(skill_delay: Duration, process_timeout: Duration, voice: Voice) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let identity = BrainIdentity::load_or_create(&dir.path().join("tls")).unwrap();
    let mut registry = EarRegistry::load(&dir.path().join("ears.toml")).unwrap();
    let (salon, token) = registry.add("Salon", "linux", false).unwrap();

    let skill = Arc::new(
        FakeSkill::new(&["media_next"])
            .with_shortcut("suivant", "media_next", serde_json::json!({}))
            .with_delay(skill_delay),
    );
    let llm = || Arc::new(FakeLlm::new(Ok(Understanding::Answer("x".into()))));
    let modes = Arc::new(ModeStore::new(dir.path().join("mode"), Mode::Local));
    let changes = modes.subscribe();
    let brain = Arc::new(
        Brain::new(vec![skill.clone()], llm(), llm(), modes).with_local_model("qwen-test"),
    );
    let stt = Arc::new(FakeTranscriber::new("suivant"));
    let ducker = Arc::new(FakeDucker::new());
    let listen = Arc::new(ListenService::new(
        brain.clone(),
        stt.clone(),
        ducker.clone(),
    ));
    let board = Arc::new(AnnouncementBoard::new(
        Arc::new(FakeRunner::new()),
        Arc::new(SystemClock),
    ));
    let board_handle = board.clone();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let services = Services {
        brain: brain.clone(),
        listen: listen.clone(),
        voice,
        board: board.clone(),
        modes: Some(changes),
        voice_tag: "test".into(),
    };
    let settings = HubSettings {
        brain_id: identity.brain_id.clone(),
        brain_name: "Bureau".into(),
        port: addr.port(),
        fingerprint: identity.fingerprint,
        hosts: vec!["192.168.1.20".parse().unwrap()],
        window: Duration::from_millis(50),
        pairing_ttl: Duration::from_secs(120),
        process_timeout,
    };
    let hub = Hub::spawn(settings, registry, services, None);
    let tls = identity.server_config().unwrap();
    tokio::spawn(net::serve_with_limits(listener, tls, hub.clone(), limits()));

    let socket = dir.path().join("run/brain.sock");
    let (h, s) = (hub.clone(), socket.clone());
    tokio::spawn(async move {
        serve_with_hub(
            brain,
            listen,
            SpeakerState::Disabled,
            Voice::none(),
            board,
            Some(h),
            &s,
        )
        .await
    });
    wait_socket(&socket).await;
    Rig {
        _dir: dir,
        addr,
        fingerprint: identity.fingerprint,
        hub,
        ducker,
        stt,
        skill,
        board: board_handle,
        socket,
        salon: (salon.id, token),
    }
}

async fn wait_socket(socket: &Path) {
    for _ in 0..100 {
        if UnixStream::connect(socket).await.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("le socket Unix ne s'ouvre pas");
}

/// Client du socket Unix (une ligne JSON par requête et par réponse).
struct Cli(BufReader<UnixStream>);

impl Cli {
    async fn connect(socket: &Path) -> Cli {
        Cli(BufReader::new(UnixStream::connect(socket).await.unwrap()))
    }

    async fn send(&mut self, request: ControlRequest) -> ControlResponse {
        let line = serde_json::to_string(&request).unwrap() + "\n";
        self.0.get_mut().write_all(line.as_bytes()).await.unwrap();
        self.read().await
    }

    async fn read(&mut self) -> ControlResponse {
        let mut answer = String::new();
        tokio::time::timeout(Duration::from_secs(5), self.0.read_line(&mut answer))
            .await
            .expect("pas de réponse du cerveau")
            .unwrap();
        serde_json::from_str(&answer).unwrap()
    }
}

async fn stream_to(addr: SocketAddr, fingerprint: &Fingerprint) -> Box<dyn AsyncStream> {
    TlsConnector::new(pinned_client_config(fingerprint))
        .connect(addr)
        .await
        .unwrap()
}

async fn connect(rig: &Rig) -> Ws {
    let stream = stream_to(rig.addr, &rig.fingerprint).await;
    let (ws, _) = tokio_tungstenite::client_async("wss://jarvis-brain/", stream)
        .await
        .unwrap();
    ws
}

async fn send(ws: &mut Ws, message: &EarMessage) {
    ws.send(Message::text(serde_json::to_string(message).unwrap()))
        .await
        .unwrap();
}

#[derive(Debug, PartialEq)]
enum Next {
    Msg(BrainMessage),
    Closed,
    Silent,
}

/// Prochain message texte du cerveau (pings ignorés), dans `wait`.
async fn next_within(ws: &mut Ws, wait: Duration) -> Next {
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        match tokio::time::timeout_at(deadline, ws.next()).await {
            Err(_) => return Next::Silent,
            Ok(None) | Ok(Some(Err(_))) | Ok(Some(Ok(Message::Close(_)))) => return Next::Closed,
            Ok(Some(Ok(Message::Text(text)))) => {
                return Next::Msg(serde_json::from_str(text.as_str()).unwrap());
            }
            Ok(Some(Ok(_))) => {}
        }
    }
}

async fn next(ws: &mut Ws) -> BrainMessage {
    match next_within(ws, Duration::from_secs(5)).await {
        Next::Msg(m) => m,
        other => panic!("message attendu, reçu {other:?}"),
    }
}

/// La connexion se ferme (après d'éventuels messages d'état).
async fn closed(ws: &mut Ws) {
    loop {
        match next_within(ws, Duration::from_secs(5)).await {
            Next::Closed => return,
            Next::Msg(BrainMessage::Status(_)) => {}
            other => panic!("fermeture attendue, reçu {other:?}"),
        }
    }
}

fn hello(id: &EarId, token: &str, offline: Option<&str>) -> EarMessage {
    EarMessage::Hello {
        version: PROTOCOL_VERSION,
        ear_id: id.clone(),
        token: token.into(),
        offline_phrase: offline.map(String::from),
    }
}

/// Connexion authentifiée : `Welcome` puis `Status` lus.
async fn session(rig: &Rig, id: &EarId, token: &str) -> Ws {
    let mut ws = connect(rig).await;
    send(&mut ws, &hello(id, token, None)).await;
    let BrainMessage::Welcome { brain_name, .. } = next(&mut ws).await else {
        panic!("welcome attendu")
    };
    assert_eq!(brain_name, "Bureau");
    assert!(matches!(next(&mut ws).await, BrainMessage::Status(_)));
    ws
}

async fn salon(rig: &Rig) -> Ws {
    let (id, token) = rig.salon.clone();
    session(rig, &id, &token).await
}

/// Seconde oreille appairée par code.
async fn pair_ear(rig: &Rig, name: &str) -> (EarId, String) {
    let mut cli = Cli::connect(&rig.socket).await;
    let ControlResponse::Pairing { code, .. } = cli.send(ControlRequest::PairStart).await else {
        panic!("fenêtre d'appairage attendue")
    };
    let stream = stream_to(rig.addr, &rig.fingerprint).await;
    let paired = pair_with_code(stream, &code, name, "linux").await.unwrap();
    assert_eq!(
        cli.read().await,
        ControlResponse::PairingDone { name: name.into() }
    );
    (paired.ear_id, paired.token)
}

fn wake(utterance: u32, score: f32) -> EarMessage {
    EarMessage::Wake {
        utterance,
        score,
        level_db: -30.0,
        muted: false,
    }
}

fn end(utterance: u32) -> EarMessage {
    EarMessage::ListenEnd {
        utterance,
        muted: false,
        no_speech: false,
        cut: None,
    }
}

async fn audio(ws: &mut Ws, utterance: u32, samples: usize) {
    let frame = encode_mic_frame(utterance, &vec![100i16; samples]);
    ws.send(Message::binary(frame)).await.unwrap();
}

async fn eventually(what: &str, mut ok: impl FnMut() -> bool) {
    for _ in 0..200 {
        if ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("jamais vrai : {what}");
}

#[tokio::test]
async fn oreille_inconnue_refusee_avant_tout_audio() {
    let rig = rig().await;
    // Du son avant tout hello : connexion fermée, rien n'est traité.
    let mut ws = connect(&rig).await;
    audio(&mut ws, 1, 1_600).await;
    assert_eq!(
        next_within(&mut ws, Duration::from_secs(3)).await,
        Next::Closed
    );
    // Un réveil sans hello : fermée aussi.
    let mut ws = connect(&rig).await;
    send(&mut ws, &wake(1, 0.9)).await;
    assert_eq!(
        next_within(&mut ws, Duration::from_secs(3)).await,
        Next::Closed
    );
    // Oreille inconnue : refus explicite puis fermeture.
    let mut ws = connect(&rig).await;
    let stranger = EarId::new("00112233445566778899aabbccddeeff");
    send(&mut ws, &hello(&stranger, "n'importe quoi", None)).await;
    assert_eq!(
        next(&mut ws).await,
        BrainMessage::Rejected {
            reason: RejectReason::UnknownEar
        }
    );
    assert_eq!(
        next_within(&mut ws, Duration::from_secs(3)).await,
        Next::Closed
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(rig.ducker.events().is_empty());
    assert_eq!(rig.stt.calls(), 0);
}

#[tokio::test]
async fn jeton_faux_refuse() {
    let rig = rig().await;
    let mut ws = connect(&rig).await;
    send(&mut ws, &hello(&rig.salon.0, "jeton-faux", None)).await;
    assert_eq!(
        next(&mut ws).await,
        BrainMessage::Rejected {
            reason: RejectReason::BadToken
        }
    );
    assert_eq!(
        next_within(&mut ws, Duration::from_secs(3)).await,
        Next::Closed
    );
}

#[tokio::test]
async fn version_inconnue_refusee() {
    let rig = rig().await;
    let mut ws = connect(&rig).await;
    let (id, token) = rig.salon.clone();
    send(
        &mut ws,
        &EarMessage::Hello {
            version: 2,
            ear_id: id,
            token,
            offline_phrase: None,
        },
    )
    .await;
    assert_eq!(
        next(&mut ws).await,
        BrainMessage::Rejected {
            reason: RejectReason::Version { supported: vec![1] }
        }
    );
    assert_eq!(
        next_within(&mut ws, Duration::from_secs(3)).await,
        Next::Closed
    );
}

#[tokio::test]
async fn hello_trop_lent_ferme() {
    let rig = rig().await;
    // WebSocket ouvert, rien envoyé : fermé passé le délai (300 ms ici).
    let mut ws = connect(&rig).await;
    let start = std::time::Instant::now();
    assert_eq!(
        next_within(&mut ws, Duration::from_secs(3)).await,
        Next::Closed
    );
    assert!(start.elapsed() < Duration::from_secs(2));
    // TCP sans TLS : fermé aussi.
    let mut tcp = tokio::net::TcpStream::connect(rig.addr).await.unwrap();
    let mut buf = [0u8; 16];
    let read = tokio::time::timeout(
        Duration::from_secs(3),
        tokio::io::AsyncReadExt::read(&mut tcp, &mut buf),
    )
    .await
    .expect("connexion jamais fermée");
    assert!(matches!(read, Ok(0) | Err(_)));
}

#[tokio::test]
async fn appairage_par_code_puis_connexion() {
    let rig = rig().await;
    let mut cli = Cli::connect(&rig.socket).await;
    let ControlResponse::Pairing {
        code,
        uri,
        fingerprint_short,
        expires_in_secs,
    } = cli.send(ControlRequest::PairStart).await
    else {
        panic!("fenêtre d'appairage attendue")
    };
    assert_eq!(fingerprint_short, rig.fingerprint.short());
    assert_eq!(expires_in_secs, 120);
    let parsed = PairingUri::parse(&uri).unwrap();
    assert_eq!(parsed.code, code);
    assert_eq!(parsed.port, rig.addr.port());
    assert_eq!(parsed.fingerprint, rig.fingerprint);
    assert_eq!(
        parsed.hosts,
        vec!["192.168.1.20".parse::<std::net::IpAddr>().unwrap()]
    );

    let stream = stream_to(rig.addr, &rig.fingerprint).await;
    let paired = pair_with_code(stream, &code, "Pixel 4 XL", "android")
        .await
        .unwrap();
    assert_eq!(paired.brain_name, "Bureau");
    assert_eq!(
        cli.read().await,
        ControlResponse::PairingDone {
            name: "Pixel 4 XL".into()
        }
    );
    // Code à usage unique.
    let stream = stream_to(rig.addr, &rig.fingerprint).await;
    assert!(matches!(
        pair_with_code(stream, &code, "Autre", "android").await,
        Err(PairError::Rejected(RejectReason::PairingClosed))
    ));
    // L'oreille revient avec hello.
    let _ws = session(&rig, &paired.ear_id, &paired.token).await;
    let mut cli = Cli::connect(&rig.socket).await;
    let ControlResponse::Ears { ears } = cli.send(ControlRequest::ListEars).await else {
        panic!("liste attendue")
    };
    let pixel = ears.iter().find(|e| e.id == paired.ear_id).unwrap();
    assert_eq!(
        (pixel.name.as_str(), pixel.platform.as_str()),
        ("Pixel 4 XL", "android")
    );
    assert!(pixel.connected && !pixel.local);
    assert!(!ears.iter().find(|e| e.name == "Salon").unwrap().connected);
}

#[tokio::test]
async fn appairage_hors_fenetre_refuse() {
    let rig = rig().await;
    let stream = stream_to(rig.addr, &rig.fingerprint).await;
    assert!(matches!(
        pair_with_code(stream, "123456", "Pixel", "android").await,
        Err(PairError::Rejected(RejectReason::PairingClosed))
    ));
    // Fenêtre ouverte, code faux : essais décomptés.
    let mut cli = Cli::connect(&rig.socket).await;
    let ControlResponse::Pairing { code, .. } = cli.send(ControlRequest::PairStart).await else {
        panic!("fenêtre d'appairage attendue")
    };
    let wrong = if code == "000000" { "111111" } else { "000000" };
    let stream = stream_to(rig.addr, &rig.fingerprint).await;
    assert!(matches!(
        pair_with_code(stream, wrong, "Pixel", "android").await,
        Err(PairError::Rejected(RejectReason::WrongCode {
            attempts_left: 2
        }))
    ));
    // `jarvis pair` interrompu : la fenêtre se ferme.
    drop(cli);
    tokio::time::sleep(Duration::from_millis(100)).await;
    let stream = stream_to(rig.addr, &rig.fingerprint).await;
    assert!(matches!(
        pair_with_code(stream, &code, "Pixel", "android").await,
        Err(PairError::Rejected(RejectReason::PairingClosed))
    ));
}

#[tokio::test]
async fn une_machine_seule_ne_ferme_pas_la_fenetre() {
    let rig = rig().await;
    let mut cli = Cli::connect(&rig.socket).await;
    let ControlResponse::Pairing { code, .. } = cli.send(ControlRequest::PairStart).await else {
        panic!("fenêtre d'appairage attendue")
    };
    let wrong = if code == "000000" { "111111" } else { "000000" };
    let intrus: std::net::IpAddr = "192.168.1.66".parse().unwrap();
    let try_from = |code: &str, from: std::net::IpAddr| {
        rig.hub
            .pair(code.into(), "Intrus".into(), "linux".into(), from)
    };
    for left in [2, 1, 0] {
        assert_eq!(
            try_from(wrong, intrus).await,
            BrainMessage::Rejected {
                reason: RejectReason::WrongCode {
                    attempts_left: left
                }
            }
        );
    }
    // Essais épuisés : même le bon code est refusé depuis cette adresse.
    assert_eq!(
        try_from(&code, intrus).await,
        BrainMessage::Rejected {
            reason: RejectReason::WrongCode { attempts_left: 0 }
        }
    );
    // La fenêtre reste ouverte : l'oreille attendue s'appaire, par le vrai serveur.
    let stream = stream_to(rig.addr, &rig.fingerprint).await;
    pair_with_code(stream, &code, "Pixel", "android")
        .await
        .unwrap();
    assert_eq!(
        cli.read().await,
        ControlResponse::PairingDone {
            name: "Pixel".into()
        }
    );
}

#[tokio::test]
async fn vingt_codes_faux_ferment_la_fenetre() {
    let rig = rig().await;
    let mut cli = Cli::connect(&rig.socket).await;
    let ControlResponse::Pairing { code, .. } = cli.send(ControlRequest::PairStart).await else {
        panic!("fenêtre d'appairage attendue")
    };
    let wrong = if code == "000000" { "111111" } else { "000000" };
    for n in 0..20u8 {
        let from = std::net::IpAddr::from([10, 0, 0, n / 3]);
        let answer = rig
            .hub
            .pair(wrong.into(), "Intrus".into(), "linux".into(), from)
            .await;
        assert!(
            matches!(
                answer,
                BrainMessage::Rejected {
                    reason: RejectReason::WrongCode { .. }
                }
            ),
            "{answer:?}"
        );
    }
    assert_eq!(
        cli.read().await,
        ControlResponse::PairingEnded {
            reason: "Trop d'essais.".into()
        }
    );
    let stream = stream_to(rig.addr, &rig.fingerprint).await;
    assert!(matches!(
        pair_with_code(stream, &code, "Pixel", "android").await,
        Err(PairError::Rejected(RejectReason::PairingClosed))
    ));
}

#[tokio::test]
async fn appairage_local_par_le_socket() {
    let rig = rig().await;
    let mut cli = Cli::connect(&rig.socket).await;
    let ControlResponse::PairedLocal {
        ear_id,
        token,
        port,
        fingerprint,
        brain_id,
        brain_name,
    } = cli
        .send(ControlRequest::PairLocal { name: "PC".into() })
        .await
    else {
        panic!("appairage local attendu")
    };
    assert_eq!((port, fingerprint), (rig.addr.port(), rig.fingerprint));
    // Le vrai lien de l'oreille se connecte avec ce qu'il a reçu.
    let dir = tempfile::tempdir().unwrap();
    let store = PairingStore::new(dir.path().join("ear"));
    let pairing = Pairing {
        brain_id,
        brain_name,
        ear_id: ear_id.clone(),
        token: token.clone(),
        fingerprint,
        addresses: vec![SocketAddr::from(([127, 0, 0, 1], port))],
        manual: None,
    };
    let connector = Arc::new(TlsConnector::new(pinned_client_config(&fingerprint)));
    let (link, _events) = EarLink::spawn(LinkConfig::new(pairing, store, connector, None));
    let mut state = link.state();
    tokio::time::timeout(
        Duration::from_secs(5),
        state.wait_for(|s| matches!(s, LinkState::Connected(_))),
    )
    .await
    .expect("lien jamais connecté")
    .unwrap();
    let LinkState::Connected(status) = state.borrow().clone() else {
        unreachable!()
    };
    let me = status.ears.iter().find(|e| e.id == ear_id).unwrap();
    assert!(me.local && me.connected && me.name == "PC");

    // Réappairage local : nouveau jeton, l'ancien ne vaut plus rien.
    let ControlResponse::PairedLocal { ear_id: new_id, .. } = cli
        .send(ControlRequest::PairLocal { name: "PC".into() })
        .await
    else {
        panic!("appairage local attendu")
    };
    assert_ne!(new_id, ear_id);
    let mut ws = connect(&rig).await;
    send(&mut ws, &hello(&ear_id, &token, None)).await;
    assert_eq!(
        next(&mut ws).await,
        BrainMessage::Rejected {
            reason: RejectReason::UnknownEar
        }
    );
}

#[tokio::test]
async fn jeton_de_l_oreille_du_pc_seulement_par_la_boucle_locale() {
    let rig = rig().await;
    let mut cli = Cli::connect(&rig.socket).await;
    let ControlResponse::PairedLocal { ear_id, token, .. } = cli
        .send(ControlRequest::PairLocal { name: "PC".into() })
        .await
    else {
        panic!("appairage local attendu")
    };
    // L'oreille du PC connectée par la boucle locale.
    let mut pc = session(&rig, &ear_id, &token).await;
    let hello_from = |from: &str| {
        let (tx, _rx) = tokio::sync::mpsc::channel(64);
        rig.hub.hello(
            ear_id.clone(),
            token.clone(),
            None,
            tx,
            from.parse().unwrap(),
        )
    };
    // Son jeton, venu d'une autre machine : refusé…
    assert!(matches!(
        hello_from("192.168.1.66").await,
        net::hub::HelloOutcome::Rejected(RejectReason::BadToken)
    ));
    // … sans fermer la vraie session.
    send(&mut pc, &EarMessage::Status).await;
    assert!(matches!(
        next(&mut pc).await,
        BrainMessage::Response {
            utterance: None,
            response: ControlResponse::Status { .. }
        }
    ));
    // IPv4 dans IPv6 : la boucle locale reste reconnue.
    assert!(matches!(
        hello_from("::ffff:127.0.0.1").await,
        net::hub::HelloOutcome::Accepted { .. }
    ));
    // Une oreille d'une autre machine se connecte de n'importe où.
    let (tx, _rx) = tokio::sync::mpsc::channel(64);
    let (salon, salon_token) = rig.salon.clone();
    assert!(matches!(
        rig.hub
            .hello(
                salon,
                salon_token,
                None,
                tx,
                "192.168.1.66".parse().unwrap()
            )
            .await,
        net::hub::HelloOutcome::Accepted { .. }
    ));
}

#[tokio::test]
async fn reconnexion_remplace_l_ancienne_session() {
    let rig = rig().await;
    let mut old = salon(&rig).await;
    let mut new = salon(&rig).await;
    closed(&mut old).await;
    send(&mut new, &EarMessage::Status).await;
    assert!(matches!(
        next(&mut new).await,
        BrainMessage::Response {
            utterance: None,
            response: ControlResponse::Status { .. }
        }
    ));
    let mut cli = Cli::connect(&rig.socket).await;
    let ControlResponse::Ears { ears } = cli.send(ControlRequest::ListEars).await else {
        panic!("liste attendue")
    };
    assert!(ears[0].connected);
}

#[tokio::test]
async fn trame_trop_grande_ferme_la_session() {
    let rig = rig().await;
    // Plus de 16 000 échantillons dans une trame.
    let mut ws = salon(&rig).await;
    audio(&mut ws, 1, 16_001).await;
    closed(&mut ws).await;
    // Message de plus d'1 Mio.
    let mut ws = salon(&rig).await;
    let _ = ws.send(Message::binary(vec![1u8; 1024 * 1024 + 16])).await;
    closed(&mut ws).await;
    assert_eq!(rig.stt.calls(), 0);
}

#[tokio::test]
async fn phrase_trop_longue_abandonnee_et_son_retabli() {
    let rig = rig().await;
    let mut ws = salon(&rig).await;
    send(&mut ws, &wake(1, 0.9)).await;
    assert_eq!(next(&mut ws).await, BrainMessage::Elected { utterance: 1 });
    // 14 s de son : au-delà des 13 s permises.
    for _ in 0..14 {
        audio(&mut ws, 1, 16_000).await;
    }
    assert_eq!(next(&mut ws).await, BrainMessage::Stop { utterance: 1 });
    eventually("son rétabli", || {
        rig.ducker.events() == ["duck", "restore"]
    })
    .await;
    // La fin qui arrive ensuite n'est pas traitée.
    send(&mut ws, &end(1)).await;
    assert_eq!(
        next_within(&mut ws, Duration::from_millis(300)).await,
        Next::Silent
    );
    assert_eq!(rig.stt.calls(), 0);
    // Une nouvelle phrase passe.
    send(&mut ws, &wake(2, 0.9)).await;
    assert_eq!(next(&mut ws).await, BrainMessage::Elected { utterance: 2 });
}

#[tokio::test]
async fn reponse_envoyee_a_l_oreille_gagnante() {
    let rig = rig().await;
    let (cuisine, token) = pair_ear(&rig, "Cuisine").await;
    let mut a = salon(&rig).await;
    let mut b = session(&rig, &cuisine, &token).await;
    // `a` a reçu l'état annonçant `b`.
    assert!(matches!(next(&mut a).await, BrainMessage::Status(_)));
    send(&mut a, &wake(7, 0.9)).await;
    send(&mut b, &wake(3, 0.5)).await;
    audio(&mut a, 7, 1_600).await;
    audio(&mut b, 3, 1_600).await;
    assert_eq!(next(&mut a).await, BrainMessage::Elected { utterance: 7 });
    assert_eq!(next(&mut b).await, BrainMessage::Stop { utterance: 3 });
    send(&mut b, &end(3)).await;
    audio(&mut a, 7, 16_000).await;
    send(&mut a, &end(7)).await;
    let BrainMessage::Response {
        utterance: Some(7),
        response: ControlResponse::Heard { text, .. },
    } = next(&mut a).await
    else {
        panic!("réponse attendue")
    };
    assert_eq!(text, "suivant");
    assert_eq!(
        next_within(&mut b, Duration::from_millis(300)).await,
        Next::Silent
    );
    assert_eq!(rig.stt.calls(), 1);
    assert_eq!(rig.skill.calls().len(), 1);
    assert_eq!(rig.ducker.events().first(), Some(&"duck"));
    assert!(rig.ducker.events().contains(&"restore"));
}

#[tokio::test]
async fn traitement_trop_long_arrete_pour_de_bon() {
    // Compétence de 1,5 s, traitement permis : 200 ms.
    let rig = rig_with(Duration::from_millis(1_500), Duration::from_millis(200)).await;
    let mut ws = salon(&rig).await;
    send(&mut ws, &wake(1, 0.9)).await;
    assert_eq!(next(&mut ws).await, BrainMessage::Elected { utterance: 1 });
    audio(&mut ws, 1, 1_600).await;
    send(&mut ws, &end(1)).await;
    let BrainMessage::Response {
        utterance: Some(1),
        response: ControlResponse::Error { message },
    } = next(&mut ws).await
    else {
        panic!("erreur attendue")
    };
    assert_eq!(message, "Traitement trop long.");
    eventually("compétence lancée", || rig.skill.calls().len() == 1).await;
    // Son rétabli, et la phrase suivante passe tout de suite.
    assert_eq!(rig.ducker.events().last(), Some(&"restore"));
    send(&mut ws, &wake(2, 0.9)).await;
    assert_eq!(next(&mut ws).await, BrainMessage::Elected { utterance: 2 });
    // Le traitement abandonné ne finit jamais derrière : ni compétence, ni réponse.
    tokio::time::sleep(Duration::from_millis(1_800)).await;
    assert_eq!(rig.skill.finished(), 0);
    assert_eq!(
        next_within(&mut ws, Duration::from_millis(100)).await,
        Next::Silent
    );
}

#[tokio::test]
async fn phrase_close_avant_la_reponse() {
    let rig = rig().await;
    let mut ws = salon(&rig).await;
    send(&mut ws, &wake(1, 0.9)).await;
    assert_eq!(next(&mut ws).await, BrainMessage::Elected { utterance: 1 });
    audio(&mut ws, 1, 1_600).await;
    send(&mut ws, &end(1)).await;
    assert!(matches!(
        next(&mut ws).await,
        BrainMessage::Response {
            utterance: Some(1),
            ..
        }
    ));
    // Dès la réponse reçue, une réponse sans mot d'éveil est acceptée…
    send(&mut ws, &EarMessage::ListenStart { utterance: 2 }).await;
    assert_eq!(next(&mut ws).await, BrainMessage::Elected { utterance: 2 });
    // … et un réveil pendant celle-ci reçoit stop.
    send(&mut ws, &wake(3, 0.9)).await;
    assert_eq!(next(&mut ws).await, BrainMessage::Stop { utterance: 3 });
    send(
        &mut ws,
        &EarMessage::ListenEnd {
            utterance: 2,
            muted: false,
            no_speech: true,
            cut: None,
        },
    )
    .await;
    assert!(matches!(
        next(&mut ws).await,
        BrainMessage::Response {
            utterance: Some(2),
            ..
        }
    ));
    // Rien dit : pas de transcription.
    assert_eq!(rig.stt.calls(), 1);
    send(&mut ws, &wake(4, 0.9)).await;
    assert_eq!(next(&mut ws).await, BrainMessage::Elected { utterance: 4 });
}

#[tokio::test]
async fn annonce_poussee_a_l_oreille_connectee() {
    let rig = rig().await;
    let mut ws = salon(&rig).await;
    rig.board.announce(Notice {
        title: "Agent".into(),
        speech: "Tâche finie.".into(),
        body: String::new(),
        outcome: Outcome::Done,
        urgent: false,
        speak: true,
    });
    let BrainMessage::Response {
        utterance: None,
        response: ControlResponse::Announcements { items },
    } = next(&mut ws).await
    else {
        panic!("annonces attendues")
    };
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].speech, "Tâche finie.");
    assert_eq!(items[0].sound, Some(jarvis_protocol::Sound::Success));
    // Relevées une seule fois.
    assert_eq!(
        next_within(&mut ws, Duration::from_millis(300)).await,
        Next::Silent
    );
}

#[tokio::test]
async fn annonce_gardee_si_l_oreille_part_pendant_sa_synthese() {
    let synth = FakeSynthesizer::new(Speech {
        samples: vec![1_000; 1_600],
        sample_rate: 16_000,
    })
    .with_delay(Duration::from_millis(800));
    let rig = rig_full(
        Duration::ZERO,
        net::hub::PROCESS_TIMEOUT,
        Voice::new(Arc::new(synth)),
    )
    .await;
    let mut ws = salon(&rig).await;
    rig.board.announce(Notice {
        title: "Agent".into(),
        speech: "Tâche finie.".into(),
        body: String::new(),
        outcome: Outcome::Done,
        urgent: false,
        speak: true,
    });
    // Partie pendant la synthèse de l'annonce.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let _ = ws.close(None).await;
    drop(ws);
    tokio::time::sleep(Duration::from_secs(1)).await;
    // De retour : l'annonce n'est pas perdue (relève suivante, dans les 5 s).
    let mut ws = salon(&rig).await;
    let BrainMessage::Response {
        utterance: None,
        response: ControlResponse::Announcements { items },
    } = (match next_within(&mut ws, Duration::from_secs(8)).await {
        Next::Msg(m) => m,
        other => panic!("annonces attendues, reçu {other:?}"),
    })
    else {
        panic!("annonces attendues")
    };
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].speech, "Tâche finie.");
    assert!(items[0].audio.is_some());
}

#[tokio::test]
async fn changement_de_mode_diffuse() {
    let rig = rig().await;
    let (cuisine, token) = pair_ear(&rig, "Cuisine").await;
    let mut a = salon(&rig).await;
    let mut b = session(&rig, &cuisine, &token).await;
    assert!(matches!(next(&mut a).await, BrainMessage::Status(_)));
    let mode_of = |m: BrainMessage| match m {
        BrainMessage::Status(s) => s.mode,
        other => panic!("état attendu, reçu {other:?}"),
    };
    // Par le CLI.
    let mut cli = Cli::connect(&rig.socket).await;
    cli.send(ControlRequest::SetMode { mode: Mode::Online })
        .await;
    assert_eq!(mode_of(next(&mut a).await), Mode::Online);
    assert_eq!(mode_of(next(&mut b).await), Mode::Online);
    // Par une oreille : réponse à elle, état à toutes.
    send(&mut a, &EarMessage::SetMode { mode: Mode::Local }).await;
    assert!(matches!(
        next(&mut a).await,
        BrainMessage::Response {
            utterance: None,
            response: ControlResponse::Status {
                mode: Mode::Local,
                ..
            }
        }
    ));
    assert_eq!(mode_of(next(&mut a).await), Mode::Local);
    assert_eq!(mode_of(next(&mut b).await), Mode::Local);
}

#[tokio::test]
async fn mode_en_ligne_seulement_depuis_l_oreille_du_pc() {
    let rig = rig().await;
    let mut salon = salon(&rig).await;
    // Une oreille d'une autre machine : refusé, le mode ne change pas.
    send(&mut salon, &EarMessage::SetMode { mode: Mode::Online }).await;
    let BrainMessage::Response {
        utterance: None,
        response: ControlResponse::Error { message },
    } = next(&mut salon).await
    else {
        panic!("erreur attendue")
    };
    assert!(message.contains("PC"), "{message}");
    assert_eq!(
        next_within(&mut salon, Duration::from_millis(300)).await,
        Next::Silent
    );
    let mut cli = Cli::connect(&rig.socket).await;
    assert!(matches!(
        cli.send(ControlRequest::Status).await,
        ControlResponse::Status {
            mode: Mode::Local,
            ..
        }
    ));
    // L'oreille du PC (son icône) : accepté.
    let ControlResponse::PairedLocal { ear_id, token, .. } = cli
        .send(ControlRequest::PairLocal { name: "PC".into() })
        .await
    else {
        panic!("appairage local attendu")
    };
    let mut pc = session(&rig, &ear_id, &token).await;
    send(&mut pc, &EarMessage::SetMode { mode: Mode::Online }).await;
    assert!(matches!(
        next(&mut pc).await,
        BrainMessage::Response {
            utterance: None,
            response: ControlResponse::Status {
                mode: Mode::Online,
                ..
            }
        }
    ));
}

#[tokio::test]
async fn retrait_deconnecte_l_oreille() {
    let rig = rig().await;
    let mut ws = salon(&rig).await;
    let mut cli = Cli::connect(&rig.socket).await;
    assert_eq!(
        cli.send(ControlRequest::Unpair {
            ear: "Salon".into()
        })
        .await,
        ControlResponse::Reply {
            speech: "Oreille « Salon » retirée.".into()
        }
    );
    closed(&mut ws).await;
    let (id, token) = rig.salon.clone();
    let mut ws = connect(&rig).await;
    send(&mut ws, &hello(&id, &token, None)).await;
    assert_eq!(
        next(&mut ws).await,
        BrainMessage::Rejected {
            reason: RejectReason::UnknownEar
        }
    );
    assert!(matches!(
        cli.send(ControlRequest::Unpair {
            ear: "Salon".into()
        })
        .await,
        ControlResponse::Error { .. }
    ));
}

fn phrase() -> SpeechAudio {
    SpeechAudio {
        pcm: encode_pcm(&[1, 2, 3]),
        sample_rate: 22_050,
    }
}

#[tokio::test]
async fn phrase_hors_ligne_envoyee_si_absente() {
    let rig = rig().await;
    // Oreille déjà connectée quand la phrase devient prête.
    let mut early = salon(&rig).await;
    rig.hub.set_offline_phrase("abc".into(), phrase()).await;
    assert_eq!(
        next(&mut early).await,
        BrainMessage::OfflinePhrase {
            id: "abc".into(),
            audio: phrase()
        }
    );
    // Oreille qui arrive avec une autre version.
    let (id, token) = rig.salon.clone();
    let mut ws = connect(&rig).await;
    send(&mut ws, &hello(&id, &token, Some("vieille"))).await;
    assert!(matches!(next(&mut ws).await, BrainMessage::Welcome { .. }));
    assert!(matches!(next(&mut ws).await, BrainMessage::Status(_)));
    assert_eq!(
        next(&mut ws).await,
        BrainMessage::OfflinePhrase {
            id: "abc".into(),
            audio: phrase()
        }
    );
}

#[tokio::test]
async fn phrase_hors_ligne_non_renvoyee_si_a_jour() {
    let rig = rig().await;
    rig.hub.set_offline_phrase("abc".into(), phrase()).await;
    let (id, token) = rig.salon.clone();
    let mut ws = connect(&rig).await;
    send(&mut ws, &hello(&id, &token, Some("abc"))).await;
    assert!(matches!(next(&mut ws).await, BrainMessage::Welcome { .. }));
    assert!(matches!(next(&mut ws).await, BrainMessage::Status(_)));
    send(&mut ws, &EarMessage::Status).await;
    assert!(matches!(
        next(&mut ws).await,
        BrainMessage::Response {
            utterance: None,
            ..
        }
    ));
}

/// Cerveau sans réseau : le socket Unix répond, l'appairage dit pourquoi.
async fn unix_only(dir: &Path) -> PathBuf {
    let llm = || Arc::new(FakeLlm::new(Ok(Understanding::Answer("x".into()))));
    let brain = Arc::new(Brain::new(
        vec![],
        llm(),
        llm(),
        Arc::new(ModeStore::new(dir.join("mode"), Mode::Local)),
    ));
    let listen = Arc::new(ListenService::new(
        brain.clone(),
        Arc::new(FakeTranscriber::new("x")),
        Arc::new(FakeDucker::new()),
    ));
    let board = Arc::new(AnnouncementBoard::new(
        Arc::new(FakeRunner::new()),
        Arc::new(SystemClock),
    ));
    let socket = dir.join("run/brain.sock");
    let s = socket.clone();
    tokio::spawn(async move {
        serve_with_hub(
            brain,
            listen,
            SpeakerState::Disabled,
            Voice::none(),
            board,
            None,
            &s,
        )
        .await
    });
    wait_socket(&socket).await;
    socket
}

fn services(dir: &Path) -> Services {
    let llm = || Arc::new(FakeLlm::new(Ok(Understanding::Answer("x".into()))));
    let brain = Arc::new(Brain::new(
        vec![],
        llm(),
        llm(),
        Arc::new(ModeStore::new(dir.join("mode"), Mode::Local)),
    ));
    Services {
        listen: Arc::new(ListenService::new(
            brain.clone(),
            Arc::new(FakeTranscriber::new("x")),
            Arc::new(FakeDucker::new()),
        )),
        brain,
        voice: Voice::none(),
        board: Arc::new(AnnouncementBoard::new(
            Arc::new(FakeRunner::new()),
            Arc::new(SystemClock),
        )),
        modes: None,
        voice_tag: String::new(),
    }
}

#[tokio::test]
async fn reseau_desactive_le_cerveau_demarre() {
    let dir = tempfile::tempdir().unwrap();
    let paths = NetworkPaths {
        tls_dir: dir.path().join("tls"),
        ears_file: dir.path().join("ears.toml"),
    };
    let local = |listen: &str| NetworkConfig {
        listen: listen.into(),
        ..NetworkConfig::default()
    };
    // Désactivé : rien n'est créé.
    let off = NetworkConfig {
        enabled: false,
        ..local("127.0.0.1:0")
    };
    assert!(
        net::start(&off, &paths, services(dir.path()), false)
            .await
            .is_none()
    );
    assert!(!paths.tls_dir.exists());
    // Port pris.
    let taken = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let busy = local(&taken.local_addr().unwrap().to_string());
    assert!(
        net::start(&busy, &paths, services(dir.path()), false)
            .await
            .is_none()
    );
    // Adresse invalide.
    assert!(
        net::start(&local("partout"), &paths, services(dir.path()), false)
            .await
            .is_none()
    );
    // Identité TLS abîmée (clé disparue).
    std::fs::remove_file(paths.tls_dir.join("brain.key.der")).unwrap();
    assert!(
        net::start(&local("127.0.0.1:0"), &paths, services(dir.path()), false)
            .await
            .is_none()
    );
    // Le cerveau répond quand même sur le socket Unix.
    let socket = unix_only(dir.path()).await;
    let mut cli = Cli::connect(&socket).await;
    assert!(matches!(
        cli.send(ControlRequest::Status).await,
        ControlResponse::Status { .. }
    ));
    for request in [
        ControlRequest::PairStart,
        ControlRequest::PairLocal { name: "PC".into() },
        ControlRequest::ListEars,
    ] {
        let ControlResponse::Error { message } = cli.send(request).await else {
            panic!("erreur attendue")
        };
        assert!(message.contains("désactivé"), "{message}");
    }
    // Identité saine : le réseau démarre.
    let fresh = tempfile::tempdir().unwrap();
    let paths = NetworkPaths {
        tls_dir: fresh.path().join("tls"),
        ears_file: fresh.path().join("ears.toml"),
    };
    let network = net::start(&local("127.0.0.1:0"), &paths, services(fresh.path()), false)
        .await
        .expect("réseau démarré");
    assert_ne!(network.port, 0);
    network.shutdown().await;
}
