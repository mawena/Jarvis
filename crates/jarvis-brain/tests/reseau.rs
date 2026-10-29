//! Oreilles en réseau de bout en bout : vrai cerveau (`net::serve`, vrai TLS,
//! `127.0.0.1`, fenêtre de l'arbitre de 300 ms) dans son propre runtime, comme un
//! processus à part ; fausses oreilles = vrais `EarLink` de `jarvis-ear-core`, qui
//! envoient un réveil puis un son scripté par paquets de 100 ms, comme l'oreille réelle.
//! Faux transcripteur (le premier échantillon du son dit quelle phrase a été dite),
//! fausses compétences, fausse baisse du son : ni Whisper, ni micro, ni son.

use jarvis_brain::announce::AnnouncementBoard;
use jarvis_brain::brain::{Brain, SystemClock};
use jarvis_brain::ears::EarRegistry;
use jarvis_brain::listen::ListenService;
use jarvis_brain::llm::Understanding;
use jarvis_brain::mode_store::ModeStore;
use jarvis_brain::net::{self, Hub, HubSettings, Limits, Services};
use jarvis_brain::server::{SpeakerState, serve_with_hub};
use jarvis_brain::stt::Transcriber;
use jarvis_brain::testing::{FakeDucker, FakeLlm, FakeSkill};
use jarvis_brain::tls::BrainIdentity;
use jarvis_brain::tts::Voice;
use jarvis_ear_core::net::tls::{TlsConnector, pinned_client_config};
use jarvis_ear_core::net::{
    ELECTION_TIMEOUT, EarLink, LinkConfig, LinkDown, LinkEvent, LinkState, Pairing, PairingStore,
    pair_with_code,
};
use jarvis_protocol::ear::{BrainMessage, EarMessage, RejectReason};
use jarvis_protocol::pairing_uri::PairingUri;
use jarvis_protocol::{ControlRequest, ControlResponse, EarAction, EarId, Fingerprint, Mode};
use jarvis_skills::{Announcer, Notice, Outcome, Reply, Risk, Skill, SkillError, ToolSpec};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, UnixStream};
use tokio::sync::{mpsc, oneshot};

/// Délai généreux pour tout ce qui doit arriver.
const WAIT: Duration = Duration::from_secs(10);
/// Fenêtre réelle de l'arbitre.
const WINDOW: Duration = Duration::from_millis(300);
/// Paquet de son de l'oreille : 100 ms à 16 kHz.
const PACKET: usize = 1_600;

// Phrases : valeur des échantillons du son envoyé (lue par le faux transcripteur).
const SUIVANT: i16 = 1_000;
const VEILLE: i16 = 2_000;
const OUI: i16 = 3_000;

/// Transcripteur factice : le premier échantillon dit la phrase. Compte ses appels.
#[derive(Default)]
struct Phrases {
    calls: AtomicUsize,
}

impl Phrases {
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl Transcriber for Phrases {
    async fn transcribe(&self, audio: &[f32]) -> anyhow::Result<String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        // i16 / 32 768 : exact en f32.
        let code = audio.first().map_or(0, |s| (s * 32_768.0).round() as i16);
        Ok(match code {
            SUIVANT => "suivant",
            VEILLE => "mets le PC en veille",
            OUI => "oui",
            _ => "",
        }
        .to_string())
    }
}

/// Compétence risquée (mise en veille) : demande toujours un « oui ». Compte ses
/// exécutions, sans rien lancer.
#[derive(Default)]
struct Suspend {
    calls: AtomicUsize,
}

impl Suspend {
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl Skill for Suspend {
    fn tools(&self) -> Vec<ToolSpec> {
        vec![ToolSpec {
            name: "suspend".into(),
            description: "Met le PC en veille.".into(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
            risk: Risk::Risky,
        }]
    }

    fn confirmation(&self, _tool: &str, _args: &serde_json::Value) -> Option<String> {
        Some("Je mets le PC en veille ?".into())
    }

    async fn call(&self, _tool: &str, _args: serde_json::Value) -> Result<Reply, SkillError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(Reply::new("Je mets le PC en veille."))
    }
}

/// Le « système » autour du cerveau : il survit à un redémarrage du cerveau.
#[derive(Clone)]
struct Fakes {
    stt: Arc<Phrases>,
    ducker: Arc<FakeDucker>,
    media: Arc<FakeSkill>,
    suspend: Arc<Suspend>,
    /// Fenêtre de l'arbitre.
    window: Duration,
}

/// Cerveau en marche, dans son propre runtime (arrêté comme un processus tué).
struct BrainProc {
    runtime: Option<tokio::runtime::Runtime>,
    addr: SocketAddr,
    board: Arc<AnnouncementBoard>,
}

impl BrainProc {
    /// Arrêt brutal : toutes les tâches et toutes les connexions tombent.
    async fn kill(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            tokio::task::spawn_blocking(move || runtime.shutdown_timeout(Duration::from_secs(5)))
                .await
                .unwrap();
        }
    }
}

impl Drop for BrainProc {
    fn drop(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

/// Banc d'essai : dossier du cerveau (identité TLS, registre), deux oreilles appairées
/// d'avance (« Salon », « Cuisine »).
struct Rig {
    dir: tempfile::TempDir,
    fakes: Fakes,
    fingerprint: Fingerprint,
    brain_id: String,
    salon: (EarId, String),
    cuisine: (EarId, String),
    brain: BrainProc,
}

impl Rig {
    fn socket(&self) -> PathBuf {
        self.dir.path().join("run/brain.sock")
    }

    fn addr(&self) -> SocketAddr {
        self.brain.addr
    }

    fn ducker(&self) -> Vec<&'static str> {
        self.fakes.ducker.events()
    }

    async fn restart(&mut self) {
        let addr = self.brain.addr;
        self.brain.kill().await;
        self.brain = start_brain(self.dir.path(), &self.fakes, addr).await;
    }
}

async fn rig() -> Rig {
    rig_with(Duration::ZERO, WINDOW).await
}

/// `skill_delay` : durée de la compétence « suivant » (traitement long) ; `window` :
/// fenêtre de l'arbitre.
async fn rig_with(skill_delay: Duration, window: Duration) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let identity = BrainIdentity::load_or_create(&dir.path().join("tls")).unwrap();
    let mut registry = EarRegistry::load(&dir.path().join("ears.toml")).unwrap();
    let (salon, salon_token) = registry.add("Salon", "linux", false).unwrap();
    let (cuisine, cuisine_token) = registry.add("Cuisine", "android", false).unwrap();
    drop(registry);
    let fakes = Fakes {
        stt: Arc::new(Phrases::default()),
        ducker: Arc::new(FakeDucker::new()),
        media: Arc::new(
            FakeSkill::new(&["media_next"])
                .with_shortcut("suivant", "media_next", serde_json::json!({}))
                .with_delay(skill_delay),
        ),
        suspend: Arc::new(Suspend::default()),
        window,
    };
    let brain = start_brain(dir.path(), &fakes, "127.0.0.1:0".parse().unwrap()).await;
    Rig {
        fingerprint: identity.fingerprint,
        brain_id: identity.brain_id,
        salon: (salon.id, salon_token),
        cuisine: (cuisine.id, cuisine_token),
        fakes,
        brain,
        dir,
    }
}

/// Démarre le cerveau sur `bind` (port 0 : port libre), avec l'identité et le registre
/// du dossier.
async fn start_brain(dir: &Path, fakes: &Fakes, bind: SocketAddr) -> BrainProc {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let (ready_tx, ready) = oneshot::channel();
    let (dir_owned, fakes) = (dir.to_path_buf(), fakes.clone());
    runtime.spawn(async move {
        let dir = dir_owned;
        let identity = BrainIdentity::load_or_create(&dir.join("tls")).unwrap();
        let registry = EarRegistry::load(&dir.join("ears.toml")).unwrap();
        // Le port vient d'être libéré par le cerveau précédent : quelques essais.
        let mut listener = None;
        for _ in 0..100 {
            match TcpListener::bind(bind).await {
                Ok(l) => {
                    listener = Some(l);
                    break;
                }
                Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
            }
        }
        let listener = listener.expect("port des oreilles libre");
        let addr = listener.local_addr().unwrap();

        let llm = || Arc::new(FakeLlm::new(Ok(Understanding::Answer("x".into()))));
        let modes = Arc::new(ModeStore::new(dir.join("mode"), Mode::Local));
        let changes = modes.subscribe();
        let skills: Vec<Arc<dyn Skill>> = vec![fakes.media.clone(), fakes.suspend.clone()];
        let brain = Arc::new(Brain::new(skills, llm(), llm(), modes));
        let listen = Arc::new(ListenService::new(
            brain.clone(),
            fakes.stt.clone(),
            fakes.ducker.clone(),
        ));
        let board = Arc::new(AnnouncementBoard::new(
            Arc::new(jarvis_skills::FakeRunner::new()),
            Arc::new(SystemClock),
        ));
        let services = Services {
            brain: brain.clone(),
            listen: listen.clone(),
            voice: Voice::none(),
            board: board.clone(),
            modes: Some(changes),
            voice_tag: "test".into(),
        };
        let settings = HubSettings {
            brain_id: identity.brain_id.clone(),
            brain_name: "Bureau".into(),
            port: addr.port(),
            fingerprint: identity.fingerprint,
            hosts: vec!["127.0.0.1".parse().unwrap()],
            window: fakes.window,
            pairing_ttl: Duration::from_secs(120),
        };
        let hub = Hub::spawn(settings, registry, services, None);
        let tls = identity.server_config().unwrap();
        tokio::spawn(net::serve_with_limits(
            listener,
            tls,
            hub.clone(),
            Limits::default(),
        ));
        let (h, b, socket) = (hub.clone(), board.clone(), dir.join("run/brain.sock"));
        tokio::spawn(async move {
            serve_with_hub(
                brain,
                listen,
                SpeakerState::Disabled,
                Voice::none(),
                b,
                Some(h),
                &socket,
            )
            .await
        });
        let _ = ready_tx.send((addr, board));
    });
    let (addr, board) = tokio::time::timeout(WAIT, ready)
        .await
        .expect("cerveau jamais prêt")
        .unwrap();
    wait_socket(&dir.join("run/brain.sock")).await;
    BrainProc {
        runtime: Some(runtime),
        addr,
        board,
    }
}

async fn wait_socket(socket: &Path) {
    for _ in 0..250 {
        if UnixStream::connect(socket).await.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("le socket Unix ne s'ouvre pas");
}

/// Client du socket Unix (`jarvis`, une ligne JSON par requête et par réponse).
struct Cli(BufReader<UnixStream>);

impl Cli {
    async fn connect(rig: &Rig) -> Cli {
        Cli(BufReader::new(
            UnixStream::connect(rig.socket()).await.unwrap(),
        ))
    }

    async fn send(&mut self, request: ControlRequest) -> ControlResponse {
        let line = serde_json::to_string(&request).unwrap() + "\n";
        self.0.get_mut().write_all(line.as_bytes()).await.unwrap();
        self.read().await
    }

    async fn read(&mut self) -> ControlResponse {
        let mut answer = String::new();
        tokio::time::timeout(WAIT, self.0.read_line(&mut answer))
            .await
            .expect("pas de réponse du cerveau")
            .unwrap();
        serde_json::from_str(&answer).unwrap()
    }
}

/// Fausse oreille : un vrai `EarLink`, ses événements lus en continu.
struct Ear {
    link: EarLink,
    events: mpsc::UnboundedReceiver<LinkEvent>,
    _dir: tempfile::TempDir,
}

/// Prochain événement du lien (les `Status` diffusés sont ignorés).
#[derive(Debug)]
enum Got {
    Msg(BrainMessage),
    Connected,
    Disconnected,
    Nothing,
}

impl Ear {
    fn spawn(pairing: Pairing) -> Ear {
        let dir = tempfile::tempdir().unwrap();
        let store = PairingStore::new(dir.path().join("ear"));
        store.save(&pairing).unwrap();
        let connector = Arc::new(TlsConnector::new(pinned_client_config(
            &pairing.fingerprint,
        )));
        let (link, mut raw) = EarLink::spawn(LinkConfig::new(pairing, store, connector, None));
        let (tx, events) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Some(event) = raw.recv().await {
                if tx.send(event).is_err() {
                    return;
                }
            }
        });
        Ear {
            link,
            events,
            _dir: dir,
        }
    }

    async fn next_within(&mut self, wait: Duration) -> Got {
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            match tokio::time::timeout_at(deadline, self.events.recv()).await {
                Err(_) | Ok(None) => return Got::Nothing,
                Ok(Some(LinkEvent::Message(BrainMessage::Status(_)))) => {}
                Ok(Some(LinkEvent::Message(m))) => return Got::Msg(m),
                Ok(Some(LinkEvent::Connected(_))) => return Got::Connected,
                Ok(Some(LinkEvent::Disconnected)) => return Got::Disconnected,
            }
        }
    }

    /// Prochain message du cerveau (hors `Status`).
    async fn msg(&mut self) -> BrainMessage {
        match self.next_within(WAIT).await {
            Got::Msg(m) => m,
            other => panic!("message attendu, reçu {other:?}"),
        }
    }

    /// Rien du cerveau (hors `Status`) pendant `wait`.
    async fn silent(&mut self, wait: Duration) {
        match self.next_within(wait).await {
            Got::Nothing => {}
            other => panic!("rien attendu, reçu {other:?}"),
        }
    }

    async fn connected(&mut self) {
        match self.next_within(WAIT).await {
            Got::Connected => {}
            other => panic!("connexion attendue, reçu {other:?}"),
        }
    }

    async fn disconnected(&mut self) {
        loop {
            match self.next_within(WAIT).await {
                Got::Disconnected => return,
                Got::Msg(_) => {}
                other => panic!("déconnexion attendue, reçu {other:?}"),
            }
        }
    }

    async fn state(&self, what: &str, ok: impl Fn(&LinkState) -> bool) {
        let mut state = self.link.state();
        tokio::time::timeout(WAIT, state.wait_for(|s| ok(s)))
            .await
            .unwrap_or_else(|_| panic!("état jamais atteint : {what}"))
            .unwrap();
    }

    /// « Jarvis » entendu.
    fn wake(&self, utterance: u32, score: f32, level_db: f32) -> Result<(), LinkDown> {
        self.link.send(EarMessage::Wake {
            utterance,
            score,
            level_db,
            muted: false,
        })
    }

    /// `packets` paquets de 100 ms de la phrase `phrase`.
    fn speak(&self, utterance: u32, phrase: i16, packets: usize) -> Result<(), LinkDown> {
        for _ in 0..packets {
            self.link.send_audio(utterance, &[phrase; PACKET])?;
        }
        Ok(())
    }

    fn end(&self, utterance: u32) -> Result<(), LinkDown> {
        self.link.send(EarMessage::ListenEnd {
            utterance,
            muted: false,
            no_speech: false,
            cut: None,
        })
    }

    /// Fin de phrase vide : l'oreille abandonne la phrase (pas d'élection à temps…).
    fn give_up(&self, utterance: u32) -> Result<(), LinkDown> {
        self.link.send(EarMessage::ListenEnd {
            utterance,
            muted: false,
            no_speech: true,
            cut: None,
        })
    }

    fn listen_start(&self, utterance: u32) -> Result<(), LinkDown> {
        self.link.send(EarMessage::ListenStart { utterance })
    }
}

fn pairing(rig: &Rig, (ear_id, token): &(EarId, String)) -> Pairing {
    Pairing {
        brain_id: rig.brain_id.clone(),
        brain_name: "Bureau".into(),
        ear_id: ear_id.clone(),
        token: token.clone(),
        fingerprint: rig.fingerprint,
        addresses: vec![rig.addr()],
        manual: None,
    }
}

/// Oreille connectée (bienvenue reçue).
async fn ear(rig: &Rig, who: &(EarId, String)) -> Ear {
    let mut ear = Ear::spawn(pairing(rig, who));
    ear.connected().await;
    ear
}

async fn two_ears(rig: &Rig) -> (Ear, Ear) {
    (ear(rig, &rig.salon).await, ear(rig, &rig.cuisine).await)
}

fn heard_text(message: &BrainMessage, utterance: u32) -> String {
    match message {
        BrainMessage::Response {
            utterance: Some(u),
            response: ControlResponse::Heard { text, .. },
        } if *u == utterance => text.clone(),
        other => panic!("réponse à la phrase {utterance} attendue, reçu {other:?}"),
    }
}

fn speech_of(message: &BrainMessage) -> String {
    match message {
        BrainMessage::Response {
            response: ControlResponse::Heard { speech, .. },
            ..
        } => speech.clone(),
        other => panic!("réponse attendue, reçu {other:?}"),
    }
}

async fn eventually(what: &str, ok: impl Fn() -> bool) {
    let deadline = Instant::now() + WAIT;
    while Instant::now() < deadline {
        if ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("jamais vrai : {what}");
}

/// Les deux oreilles entendent la même phrase. `(gagnante, perdante)` : chacune avec
/// son numéro de phrase. La gagnante est élue, la perdante arrêtée ; toutes deux
/// envoient leur son et leur fin (la perdante a pu finir avant son `stop`).
async fn same_phrase(
    winner: (&mut Ear, u32, f32),
    loser: (&mut Ear, u32, f32),
) -> (BrainMessage, BrainMessage) {
    let (w, wu, ws) = winner;
    let (l, lu, ls) = loser;
    w.wake(wu, ws, -30.0).unwrap();
    l.wake(lu, ls, -30.0).unwrap();
    for _ in 0..3 {
        w.speak(wu, SUIVANT, 1).unwrap();
        l.speak(lu, SUIVANT, 1).unwrap();
    }
    let (elected, stopped) = (w.msg().await, l.msg().await);
    w.speak(wu, SUIVANT, 5).unwrap();
    // Perdante : son et fin déjà en route quand le `stop` est arrivé.
    let _ = l.speak(lu, SUIVANT, 5);
    let _ = l.end(lu);
    w.end(wu).unwrap();
    (elected, stopped)
}

#[tokio::test]
async fn deux_oreilles_une_seule_execution() {
    let rig = rig().await;
    let (mut salon, mut cuisine) = two_ears(&rig).await;
    // Trois phrases entendues des deux côtés ; la gagnante change.
    for round in 1..=3u32 {
        let (s, c) = (round, 100 + round);
        let salon_wins = round != 2;
        let (elected, stopped) = if salon_wins {
            same_phrase((&mut salon, s, 0.9), (&mut cuisine, c, 0.6)).await
        } else {
            same_phrase((&mut cuisine, c, 0.9), (&mut salon, s, 0.6)).await
        };
        let (wu, lu) = if salon_wins { (s, c) } else { (c, s) };
        assert_eq!(elected, BrainMessage::Elected { utterance: wu });
        assert_eq!(stopped, BrainMessage::Stop { utterance: lu });
        let (winner, loser) = if salon_wins {
            (&mut salon, &mut cuisine)
        } else {
            (&mut cuisine, &mut salon)
        };
        assert_eq!(heard_text(&winner.msg().await, wu), "suivant");
        loser.silent(Duration::from_millis(500)).await;
        // Une seule transcription, une seule exécution par phrase.
        assert_eq!(rig.fakes.stt.calls(), round as usize);
        assert_eq!(rig.fakes.media.calls().len(), round as usize);
    }
    // Une baisse et un rétablissement du son par phrase, jamais deux.
    assert_eq!(rig.ducker(), ["duck", "restore"].repeat(3));
}

#[tokio::test]
async fn la_perdante_recoit_stop() {
    let rig = rig().await;
    let (mut salon, mut cuisine) = two_ears(&rig).await;
    let start = Instant::now();
    salon.wake(1, 0.9, -30.0).unwrap();
    cuisine.wake(1, 0.5, -30.0).unwrap();
    cuisine.speak(1, SUIVANT, 3).unwrap();
    assert_eq!(cuisine.msg().await, BrainMessage::Stop { utterance: 1 });
    // Bien avant que l'oreille ne tienne le PC pour endormi.
    assert!(start.elapsed() < ELECTION_TIMEOUT, "{:?}", start.elapsed());
    assert_eq!(salon.msg().await, BrainMessage::Elected { utterance: 1 });
    // Son, fin et demande d'écoute de la perdante : ignorés, rien n'est joué.
    let _ = cuisine.speak(1, OUI, 10);
    let _ = cuisine.end(1);
    cuisine.listen_start(2).unwrap();
    assert_eq!(cuisine.msg().await, BrainMessage::Stop { utterance: 2 });
    salon.speak(1, SUIVANT, 5).unwrap();
    salon.end(1).unwrap();
    assert_eq!(heard_text(&salon.msg().await, 1), "suivant");
    cuisine.silent(Duration::from_millis(500)).await;
    assert_eq!(rig.fakes.stt.calls(), 1);
    assert_eq!(rig.fakes.media.calls().len(), 1);
}

#[tokio::test]
async fn la_plus_forte_gagne() {
    // La perdante se réveille la première : fenêtre d'une seconde pour que les deux
    // réveils y tombent toujours, même machine chargée.
    let rig = rig_with(Duration::ZERO, Duration::from_secs(1)).await;
    let (mut salon, mut cuisine) = two_ears(&rig).await;
    // Même score : la plus proche (son le plus fort) gagne.
    salon.wake(1, 0.8, -45.0).unwrap();
    cuisine.wake(1, 0.8, -15.0).unwrap();
    assert_eq!(cuisine.msg().await, BrainMessage::Elected { utterance: 1 });
    assert_eq!(salon.msg().await, BrainMessage::Stop { utterance: 1 });
    cuisine.speak(1, SUIVANT, 3).unwrap();
    cuisine.end(1).unwrap();
    heard_text(&cuisine.msg().await, 1);
    // Même niveau : le meilleur score gagne.
    salon.wake(2, 0.9, -30.0).unwrap();
    cuisine.wake(2, 0.6, -30.0).unwrap();
    assert_eq!(salon.msg().await, BrainMessage::Elected { utterance: 2 });
    assert_eq!(cuisine.msg().await, BrainMessage::Stop { utterance: 2 });
}

#[tokio::test]
async fn reponse_jouee_sur_la_gagnante_seulement() {
    let rig = rig().await;
    let (mut salon, mut cuisine) = two_ears(&rig).await;
    let (elected, stopped) = same_phrase((&mut cuisine, 4, 0.9), (&mut salon, 9, 0.5)).await;
    assert_eq!(elected, BrainMessage::Elected { utterance: 4 });
    assert_eq!(stopped, BrainMessage::Stop { utterance: 9 });
    let response = cuisine.msg().await;
    assert_eq!(heard_text(&response, 4), "suivant");
    salon.silent(Duration::from_secs(1)).await;
}

#[tokio::test]
async fn reveil_tardif_d_une_oreille_wifi_recoit_stop() {
    let rig = rig().await;
    let (mut salon, mut cuisine) = two_ears(&rig).await;
    salon.wake(1, 0.6, -30.0).unwrap();
    salon.speak(1, SUIVANT, 3).unwrap();
    // Réveil de la même phrase, arrivé 600 ms plus tard (Wi-Fi lent) : fenêtre fermée.
    tokio::time::sleep(Duration::from_millis(600)).await;
    let late = Instant::now();
    cuisine.wake(1, 0.95, -10.0).unwrap();
    cuisine.speak(1, SUIVANT, 3).unwrap();
    assert_eq!(cuisine.msg().await, BrainMessage::Stop { utterance: 1 });
    assert!(late.elapsed() < ELECTION_TIMEOUT, "{:?}", late.elapsed());
    assert_eq!(salon.msg().await, BrainMessage::Elected { utterance: 1 });
    let _ = cuisine.end(1);
    salon.speak(1, SUIVANT, 3).unwrap();
    salon.end(1).unwrap();
    assert_eq!(heard_text(&salon.msg().await, 1), "suivant");
    cuisine.silent(Duration::from_millis(500)).await;
    assert_eq!(rig.fakes.media.calls().len(), 1);
}

/// « Mets le PC en veille » sur `ear` (phrase `utterance`) : la question est posée.
async fn ask_suspend(ear: &mut Ear, utterance: u32) {
    ear.wake(utterance, 0.9, -30.0).unwrap();
    assert_eq!(ear.msg().await, BrainMessage::Elected { utterance });
    ear.speak(utterance, VEILLE, 8).unwrap();
    ear.end(utterance).unwrap();
    let BrainMessage::Response {
        utterance: Some(u),
        response: ControlResponse::Heard {
            text, ear_action, ..
        },
    } = ear.msg().await
    else {
        panic!("question attendue")
    };
    assert_eq!(u, utterance);
    assert_eq!(text, "mets le PC en veille");
    assert_eq!(ear_action, Some(EarAction::AwaitReply));
}

/// Réponse sans mot d'éveil (`listen_start`) : « oui ».
async fn reply_yes(ear: &mut Ear, utterance: u32) -> BrainMessage {
    ear.listen_start(utterance).unwrap();
    assert_eq!(ear.msg().await, BrainMessage::Elected { utterance });
    ear.speak(utterance, OUI, 4).unwrap();
    ear.end(utterance).unwrap();
    ear.msg().await
}

#[tokio::test]
async fn confirmation_sur_la_meme_oreille() {
    let rig = rig().await;
    let (mut salon, _cuisine) = two_ears(&rig).await;
    ask_suspend(&mut salon, 1).await;
    assert_eq!(rig.fakes.suspend.calls(), 0);
    // La réponse est acceptée dès la question reçue (le groupe est clos avant).
    let answer = reply_yes(&mut salon, 2).await;
    assert_eq!(heard_text(&answer, 2), "oui");
    assert_eq!(speech_of(&answer), "Je mets le PC en veille.");
    assert_eq!(rig.fakes.suspend.calls(), 1);
}

#[tokio::test]
async fn confirmation_d_une_autre_oreille_ignoree() {
    let rig = rig().await;
    let (mut salon, mut cuisine) = two_ears(&rig).await;
    ask_suspend(&mut salon, 1).await;
    // « Oui » entendu par la cuisine (réponse sans mot d'éveil) : rien.
    let answer = reply_yes(&mut cuisine, 1).await;
    assert_eq!(speech_of(&answer), "");
    // Par sécurité, l'attente est effacée : le « oui » du salon ne fait plus rien.
    let answer = reply_yes(&mut salon, 2).await;
    assert_eq!(speech_of(&answer), "Il n'y a rien à confirmer.");
    // « Oui » dit à la cuisine après « Jarvis » : nouvelle phrase, rien non plus.
    ask_suspend(&mut salon, 3).await;
    cuisine.wake(2, 0.9, -30.0).unwrap();
    assert_eq!(cuisine.msg().await, BrainMessage::Elected { utterance: 2 });
    cuisine.speak(2, OUI, 4).unwrap();
    cuisine.end(2).unwrap();
    heard_text(&cuisine.msg().await, 2);
    reply_yes(&mut salon, 4).await;
    assert_eq!(rig.fakes.suspend.calls(), 0);
}

#[tokio::test]
async fn musique_retablie_si_la_gagnante_se_deconnecte() {
    let rig = rig().await;
    let (mut salon, mut cuisine) = two_ears(&rig).await;
    // Gagnante partie en pleine capture.
    salon.wake(1, 0.9, -30.0).unwrap();
    cuisine.wake(1, 0.5, -30.0).unwrap();
    assert_eq!(salon.msg().await, BrainMessage::Elected { utterance: 1 });
    assert_eq!(cuisine.msg().await, BrainMessage::Stop { utterance: 1 });
    salon.speak(1, SUIVANT, 5).unwrap();
    drop(salon);
    eventually("son rétabli", || rig.ducker() == ["duck", "restore"]).await;
    assert_eq!(rig.fakes.stt.calls(), 0);
    // Le groupe est clos : une nouvelle phrase passe.
    cuisine.wake(2, 0.9, -30.0).unwrap();
    assert_eq!(cuisine.msg().await, BrainMessage::Elected { utterance: 2 });
    cuisine.speak(2, SUIVANT, 3).unwrap();
    cuisine.end(2).unwrap();
    heard_text(&cuisine.msg().await, 2);
    assert_eq!(rig.ducker(), ["duck", "restore", "duck", "restore"]);
    // Seule oreille de la fenêtre, partie avant l'élection : son rétabli aussi.
    let mut salon = ear(&rig, &rig.salon).await;
    salon.wake(3, 0.9, -30.0).unwrap();
    salon.link.close().await;
    eventually("son rétabli", || {
        rig.ducker() == ["duck", "restore", "duck", "restore", "duck", "restore"]
    })
    .await;
    cuisine.wake(3, 0.9, -30.0).unwrap();
    assert_eq!(cuisine.msg().await, BrainMessage::Elected { utterance: 3 });
    assert_eq!(rig.fakes.stt.calls(), 1);
    let _ = &mut salon;
}

#[tokio::test]
async fn oreille_inconnue_refusee() {
    let rig = rig().await;
    // Identifiant inconnu.
    let stranger = (
        EarId::new("00112233445566778899aabbccddeeff"),
        "x".to_string(),
    );
    let unknown = Ear::spawn(pairing(&rig, &stranger));
    unknown
        .state("refusée", |s| {
            *s == LinkState::Rejected(RejectReason::UnknownEar)
        })
        .await;
    assert_eq!(unknown.wake(1, 0.9, -30.0), Err(LinkDown));
    assert_eq!(
        unknown.link.send_audio(1, &[SUIVANT; PACKET]),
        Err(LinkDown)
    );
    // Bon identifiant, jeton faux.
    let forged = (rig.salon.0.clone(), "jeton-faux".to_string());
    let forged = Ear::spawn(pairing(&rig, &forged));
    forged
        .state("refusée", |s| {
            *s == LinkState::Rejected(RejectReason::BadToken)
        })
        .await;
    assert_eq!(forged.wake(1, 0.9, -30.0), Err(LinkDown));
    // Bon jeton, mais un autre certificat que celui épinglé : jamais connectée.
    let mut wrong = pairing(&rig, &rig.salon);
    wrong.fingerprint = Fingerprint::of_der(b"un autre cerveau");
    let impostor = Ear::spawn(wrong);
    impostor
        .state("injoignable", |s| *s == LinkState::Unreachable)
        .await;
    assert_eq!(impostor.wake(1, 0.9, -30.0), Err(LinkDown));
    // Rien n'a été baissé, ni transcrit.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(rig.ducker().is_empty());
    assert_eq!(rig.fakes.stt.calls(), 0);
}

#[tokio::test]
async fn jeton_revoque_deconnecte() {
    let rig = rig().await;
    let mut salon = ear(&rig, &rig.salon).await;
    let mut cli = Cli::connect(&rig).await;
    assert_eq!(
        cli.send(ControlRequest::Unpair {
            ear: "Salon".into()
        })
        .await,
        ControlResponse::Reply {
            speech: "Oreille « Salon » retirée.".into()
        }
    );
    salon.disconnected().await;
    salon
        .state("refusée", |s| {
            *s == LinkState::Rejected(RejectReason::UnknownEar)
        })
        .await;
    assert_eq!(salon.wake(1, 0.9, -30.0), Err(LinkDown));
    assert!(rig.ducker().is_empty());
}

#[tokio::test]
async fn cerveau_coupe_puis_relance_reconnexion() {
    let mut rig = rig().await;
    let mut salon = ear(&rig, &rig.salon).await;
    rig.brain.kill().await;
    salon.disconnected().await;
    // PC endormi : le réveil échoue tout de suite (la phrase hors ligne est jouée
    // aussitôt), rien n'est envoyé ni gardé.
    let asleep = Instant::now();
    assert_eq!(salon.wake(1, 0.9, -30.0), Err(LinkDown));
    assert_eq!(salon.link.send_audio(1, &[SUIVANT; PACKET]), Err(LinkDown));
    assert!(asleep.elapsed() < Duration::from_millis(100));
    // Cerveau relancé sur le même port (même identité) : l'oreille revient seule.
    rig.restart().await;
    match salon.next_within(Duration::from_secs(30)).await {
        Got::Connected => {}
        other => panic!("reconnexion attendue, reçu {other:?}"),
    }
    salon.wake(2, 0.9, -30.0).unwrap();
    assert_eq!(salon.msg().await, BrainMessage::Elected { utterance: 2 });
    salon.speak(2, SUIVANT, 3).unwrap();
    salon.end(2).unwrap();
    assert_eq!(heard_text(&salon.msg().await, 2), "suivant");
    // Le réveil refusé pendant la coupure n'a rien laissé derrière lui.
    assert_eq!(rig.fakes.stt.calls(), 1);
    assert_eq!(rig.ducker(), ["duck", "restore"]);
}

#[tokio::test]
async fn appairage_par_code_de_bout_en_bout() {
    let rig = rig().await;
    let mut cli = Cli::connect(&rig).await;
    let ControlResponse::Pairing {
        code,
        uri,
        fingerprint_short,
        ..
    } = cli.send(ControlRequest::PairStart).await
    else {
        panic!("fenêtre d'appairage attendue")
    };
    // Côté oreille : l'URI (QR code) donne l'adresse et l'empreinte à épingler.
    let parsed = PairingUri::parse(&uri).unwrap();
    assert_eq!(parsed.fingerprint.short(), fingerprint_short);
    let addr = SocketAddr::new(parsed.hosts[0], parsed.port);
    let connector = TlsConnector::new(pinned_client_config(&parsed.fingerprint));
    let stream = jarvis_ear_core::net::Connector::connect(&connector, addr)
        .await
        .unwrap();
    let paired = pair_with_code(stream, &code, "Chambre", "linux")
        .await
        .unwrap();
    assert_eq!(
        cli.read().await,
        ControlResponse::PairingDone {
            name: "Chambre".into()
        }
    );
    // L'oreille revient avec `hello` et parle comme les autres.
    let mut chambre = Ear::spawn(Pairing {
        brain_id: paired.brain_id,
        brain_name: paired.brain_name,
        ear_id: paired.ear_id.clone(),
        token: paired.token,
        fingerprint: parsed.fingerprint,
        addresses: vec![addr],
        manual: None,
    });
    chambre.connected().await;
    let ControlResponse::Ears { ears } = Cli::connect(&rig)
        .await
        .send(ControlRequest::ListEars)
        .await
    else {
        panic!("liste attendue")
    };
    let me = ears.iter().find(|e| e.id == paired.ear_id).unwrap();
    assert!(me.connected && me.name == "Chambre" && !me.local);
    chambre.wake(1, 0.9, -30.0).unwrap();
    assert_eq!(chambre.msg().await, BrainMessage::Elected { utterance: 1 });
}

fn notice(speech: &str) -> Notice {
    Notice {
        title: "Agent".into(),
        speech: speech.into(),
        body: String::new(),
        outcome: Outcome::Done,
        urgent: false,
        speak: true,
    }
}

fn announcements(message: BrainMessage) -> Vec<String> {
    match message {
        BrainMessage::Response {
            utterance: None,
            response: ControlResponse::Announcements { items },
        } => items.into_iter().map(|a| a.speech).collect(),
        other => panic!("annonces attendues, reçu {other:?}"),
    }
}

#[tokio::test]
async fn annonce_arrive_une_seule_fois_a_l_oreille_de_la_derniere_phrase() {
    let rig = rig().await;
    let (mut salon, mut cuisine) = two_ears(&rig).await;
    // Dernière phrase dite au salon : les annonces y vont.
    salon.wake(1, 0.9, -30.0).unwrap();
    assert_eq!(salon.msg().await, BrainMessage::Elected { utterance: 1 });
    salon.speak(1, SUIVANT, 3).unwrap();
    salon.end(1).unwrap();
    heard_text(&salon.msg().await, 1);
    rig.brain.board.announce(notice("Tâche finie."));
    assert_eq!(announcements(salon.msg().await), ["Tâche finie."]);
    // Une seule fois, même après une relève périodique (5 s) ; jamais à la cuisine.
    salon.silent(Duration::from_secs(6)).await;
    cuisine.silent(Duration::from_millis(100)).await;
    // L'ancienne relève par le socket Unix (sous un nom d'oreille) n'en a plus.
    let mut cli = Cli::connect(&rig).await;
    for ear in [Some("pc".to_string()), Some("Salon".to_string()), None] {
        assert_eq!(
            cli.send(ControlRequest::Announcements { ear }).await,
            ControlResponse::Announcements { items: vec![] }
        );
    }
}

#[tokio::test]
async fn annonce_retenue_pendant_une_phrase() {
    let rig = rig().await;
    let mut salon = ear(&rig, &rig.salon).await;
    salon.wake(1, 0.9, -30.0).unwrap();
    assert_eq!(salon.msg().await, BrainMessage::Elected { utterance: 1 });
    salon.speak(1, SUIVANT, 3).unwrap();
    // Annonce pendant la capture : pas tout de suite.
    rig.brain.board.announce(notice("Tâche finie."));
    salon.silent(Duration::from_secs(1)).await;
    salon.end(1).unwrap();
    // D'abord la réponse, puis l'annonce (relève suivante, dans les 5 s).
    heard_text(&salon.msg().await, 1);
    assert_eq!(announcements(salon.msg().await), ["Tâche finie."]);
}

#[tokio::test]
async fn pc_eveille_repond_avant_le_delai_de_l_oreille() {
    // Compétence lente (3 s) : la phrase du salon est longue à traiter.
    let rig = rig_with(Duration::from_secs(3), WINDOW).await;
    let (mut salon, mut cuisine) = two_ears(&rig).await;
    let start = Instant::now();
    salon.wake(1, 0.9, -30.0).unwrap();
    assert_eq!(salon.msg().await, BrainMessage::Elected { utterance: 1 });
    // L'élection tombe à la fin de la fenêtre, bien avant les 2 s de l'oreille.
    let elected_in = start.elapsed();
    assert!(
        elected_in >= WINDOW - Duration::from_millis(10),
        "{elected_in:?}"
    );
    assert!(elected_in < ELECTION_TIMEOUT, "{elected_in:?}");
    salon.speak(1, SUIVANT, 3).unwrap();
    salon.end(1).unwrap();
    eventually("compétence lancée", || rig.fakes.media.calls().len() == 1).await;
    // Pendant le traitement : réveil et réponse d'une autre oreille arrêtés aussitôt.
    let start = Instant::now();
    cuisine.wake(1, 0.9, -30.0).unwrap();
    assert_eq!(cuisine.msg().await, BrainMessage::Stop { utterance: 1 });
    cuisine.listen_start(2).unwrap();
    assert_eq!(cuisine.msg().await, BrainMessage::Stop { utterance: 2 });
    assert!(start.elapsed() < ELECTION_TIMEOUT, "{:?}", start.elapsed());
    heard_text(&salon.msg().await, 1);
    // Phrase finie : la cuisine passe.
    cuisine.wake(3, 0.9, -30.0).unwrap();
    assert_eq!(cuisine.msg().await, BrainMessage::Elected { utterance: 3 });
}

#[tokio::test]
async fn oreille_qui_abandonne_sans_prevenir_son_retabli() {
    // L'oreille n'a pas reçu son élection dans ses 2 s (PC tenu pour endormi) : elle a
    // abandonné la phrase sans fin de phrase, la connexion restant ouverte (élection
    // retardée par le réseau, PC sorti de veille avec le réveil encore en route…).
    let rig = rig().await;
    let (mut salon, mut cuisine) = two_ears(&rig).await;
    let start = Instant::now();
    salon.wake(1, 0.9, -30.0).unwrap();
    salon.speak(1, SUIVANT, 3).unwrap();
    assert_eq!(salon.msg().await, BrainMessage::Elected { utterance: 1 });
    // Plus de son : la phrase est close et la musique rétablie bien avant les 15 s du
    // délai de capture.
    assert_eq!(salon.msg().await, BrainMessage::Stop { utterance: 1 });
    eventually("son rétabli", || rig.ducker() == ["duck", "restore"]).await;
    assert!(
        start.elapsed() < Duration::from_secs(8),
        "{:?}",
        start.elapsed()
    );
    assert_eq!(rig.fakes.stt.calls(), 0);
    cuisine.wake(1, 0.9, -30.0).unwrap();
    assert_eq!(cuisine.msg().await, BrainMessage::Elected { utterance: 1 });
}

#[tokio::test]
async fn capture_qui_continue_n_est_pas_abandonnee() {
    // Son reçu en continu (paquets de 100 ms en temps réel) pendant plusieurs secondes :
    // la phrase va jusqu'au bout.
    let rig = rig().await;
    let mut salon = ear(&rig, &rig.salon).await;
    salon.wake(1, 0.9, -30.0).unwrap();
    assert_eq!(salon.msg().await, BrainMessage::Elected { utterance: 1 });
    for _ in 0..50 {
        salon.speak(1, SUIVANT, 1).unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    salon.end(1).unwrap();
    assert_eq!(heard_text(&salon.msg().await, 1), "suivant");
}

#[tokio::test]
async fn gagnante_partie_pendant_la_fenetre_l_autre_est_elue() {
    // Fenêtre d'une seconde : les deux réveils sont arrivés bien avant le départ.
    let rig = rig_with(Duration::ZERO, Duration::from_secs(1)).await;
    let (salon, mut cuisine) = two_ears(&rig).await;
    cuisine.wake(1, 0.5, -40.0).unwrap();
    salon.wake(1, 0.95, -10.0).unwrap();
    cuisine.speak(1, SUIVANT, 2).unwrap();
    salon.speak(1, SUIVANT, 2).unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    salon.link.close().await;
    drop(salon);
    // La meilleure est partie avant la fin de la fenêtre : la cuisine est élue.
    assert_eq!(cuisine.msg().await, BrainMessage::Elected { utterance: 1 });
    cuisine.speak(1, SUIVANT, 3).unwrap();
    cuisine.end(1).unwrap();
    assert_eq!(heard_text(&cuisine.msg().await, 1), "suivant");
    // Une seule baisse du son pour la phrase, rétablie une fois.
    assert_eq!(rig.ducker(), ["duck", "restore"]);
}

#[tokio::test]
async fn fin_de_phrase_avant_l_election() {
    // Phrase très courte : les deux fins arrivent avant la fin de la fenêtre (d'une
    // seconde ici : la perdante se réveille la première).
    let rig = rig_with(Duration::ZERO, Duration::from_secs(1)).await;
    let (mut salon, mut cuisine) = two_ears(&rig).await;
    salon.wake(5, 0.5, -30.0).unwrap();
    cuisine.wake(8, 0.9, -30.0).unwrap();
    salon.speak(5, SUIVANT, 2).unwrap();
    cuisine.speak(8, SUIVANT, 2).unwrap();
    salon.end(5).unwrap();
    cuisine.end(8).unwrap();
    assert_eq!(cuisine.msg().await, BrainMessage::Elected { utterance: 8 });
    assert_eq!(salon.msg().await, BrainMessage::Stop { utterance: 5 });
    assert_eq!(heard_text(&cuisine.msg().await, 8), "suivant");
    salon.silent(Duration::from_millis(500)).await;
    assert_eq!(rig.fakes.stt.calls(), 1);
    assert_eq!(rig.fakes.media.calls().len(), 1);
}

#[tokio::test]
async fn abandon_par_l_oreille_apres_l_election_son_retabli_rien_d_execute() {
    // L'élection est arrivée trop tard : l'oreille a dit « le PC est endormi » et
    // envoyé une fin de phrase vide.
    let rig = rig().await;
    let mut salon = ear(&rig, &rig.salon).await;
    salon.wake(1, 0.9, -30.0).unwrap();
    salon.speak(1, SUIVANT, 3).unwrap();
    assert_eq!(salon.msg().await, BrainMessage::Elected { utterance: 1 });
    salon.give_up(1).unwrap();
    assert_eq!(heard_text(&salon.msg().await, 1), "");
    assert_eq!(rig.ducker(), ["duck", "restore"]);
    assert_eq!(rig.fakes.stt.calls(), 0);
    assert!(rig.fakes.media.calls().is_empty());
}

#[tokio::test]
async fn abandon_d_une_phrase_deja_lancee_ignore() {
    // Phrase finie et lancée, puis abandonnée par l'oreille : elle va jusqu'au bout,
    // une seule fois, et sa réponse reste la sienne.
    let rig = rig_with(Duration::from_millis(500), WINDOW).await;
    let mut salon = ear(&rig, &rig.salon).await;
    salon.wake(1, 0.9, -30.0).unwrap();
    assert_eq!(salon.msg().await, BrainMessage::Elected { utterance: 1 });
    salon.speak(1, SUIVANT, 3).unwrap();
    salon.end(1).unwrap();
    eventually("compétence lancée", || rig.fakes.media.calls().len() == 1).await;
    salon.give_up(1).unwrap();
    assert_eq!(heard_text(&salon.msg().await, 1), "suivant");
    salon.silent(Duration::from_millis(500)).await;
    assert_eq!(rig.fakes.stt.calls(), 1);
    assert_eq!(rig.fakes.media.calls().len(), 1);
}
