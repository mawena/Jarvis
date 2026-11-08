//! Personnes et rôles de bout en bout : vrai cerveau (vrai TLS, `127.0.0.1` port 0, vrai
//! socket Unix), vraies oreilles (`EarLink` de `jarvis-ear-core`), annuaire à deux
//! personnes (Charles `admin`, Léa rôle `famille`) et vérification de la voix à faux
//! vérificateur. Ni micro, ni son, ni service, ni vrai `claude` : l'agent de code est une
//! doublure qui compte ses appels. Chaque attente a un délai, une attente trompée fait
//! échouer le test au lieu de le suspendre.
//!
//! Voix (échantillon 1 du son) : 0 = Charles, 20 000 = inconnue. Phrase (échantillon 0) :
//! lue par le faux transcripteur.

use futures_util::{SinkExt, StreamExt};
use jarvis_brain::announce::AnnouncementBoard;
use jarvis_brain::brain::{Brain, REFUSED, SystemClock};
use jarvis_brain::ears::EarRegistry;
use jarvis_brain::listen::ListenService;
use jarvis_brain::llm::Understanding;
use jarvis_brain::mode_store::ModeStore;
use jarvis_brain::net::{self, Hub, HubSettings, Limits, Services};
use jarvis_brain::people::{EarPerson, People};
use jarvis_brain::server::{SpeakerState, serve_with_hub};
use jarvis_brain::speaker::{SpeakerGate, SpeakerVerifier};
use jarvis_brain::stt::Transcriber;
use jarvis_brain::testing::{FakeDucker, FakeLlm, FakeSkill};
use jarvis_brain::tls::BrainIdentity;
use jarvis_brain::tts::Voice;
use jarvis_ear_core::net::tls::{TlsConnector, pinned_client_config};
use jarvis_ear_core::net::{
    AsyncStream, Connector, EarLink, LinkConfig, LinkDown, LinkEvent, Pairing, PairingStore,
};
use jarvis_protocol::ear::{BrainMessage, EarMessage, PROTOCOL_VERSION};
use jarvis_protocol::{ControlRequest, ControlResponse, EarAction, EarId, Fingerprint, Mode};
use jarvis_skills::{FakeRunner, Reply, Risk, Skill, SkillError, ToolSpec};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, UnixStream};
use tokio::sync::mpsc;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;

/// Délai généreux pour tout ce qui doit arriver.
const WAIT: Duration = Duration::from_secs(10);
/// Délai pour constater qu'il n'arrive rien.
const QUIET: Duration = Duration::from_millis(500);
/// Paquet de son de l'oreille : 100 ms à 16 kHz.
const PACKET: usize = 1_600;
/// 1,2 s de son : assez long pour que la voix soit vérifiée.
const PACKETS: usize = 12;

// Phrases : valeur du premier échantillon du son.
const SUIVANT: i16 = 1_000;
const VEILLE: i16 = 2_000;
const OUI: i16 = 3_000;
const MUSIQUE: i16 = 4_000;
const AGENT: i16 = 5_000;

/// Qui a parlé (échantillon 1 du son).
#[derive(Clone, Copy)]
enum Voix {
    Charles,
    Inconnue,
}

/// Transcripteur factice : le premier échantillon dit la phrase.
#[derive(Default)]
struct Phrases {
    calls: AtomicUsize,
}

#[async_trait::async_trait]
impl Transcriber for Phrases {
    async fn transcribe(&self, audio: &[f32]) -> anyhow::Result<String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let code = audio.first().map_or(0, |s| (s * 32_768.0).round() as i16);
        Ok(match code {
            SUIVANT => "suivant",
            VEILLE => "mets le PC en veille",
            OUI => "oui",
            MUSIQUE => "lance la musique",
            AGENT => "confie une tâche à l'agent",
            _ => "",
        }
        .to_string())
    }
}

/// Faux vérificateur : la voix de Charles a l'empreinte (1, 0), toute autre (0, 1).
struct ParEchantillon;

impl SpeakerVerifier for ParEchantillon {
    fn embed(&self, audio: &[f32]) -> anyhow::Result<Vec<f32>> {
        Ok(if audio.get(1).copied().unwrap_or(0.0) < 0.5 {
            vec![1.0, 0.0]
        } else {
            vec![0.0, 1.0]
        })
    }
}

/// Compétence risquée (mise en veille) : demande un « oui », compte ses exécutions.
#[derive(Default)]
struct Suspend {
    calls: AtomicUsize,
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

/// Noms des outils du cerveau d'essai (validation des rôles).
const TOOLS: [&str; 6] = [
    "media_next",
    "media_play",
    "app_launch",
    "agent_start",
    "suspend",
    "set_mode",
];

type Oreille = (EarId, String);

struct Rig {
    dir: tempfile::TempDir,
    addr: SocketAddr,
    fingerprint: Fingerprint,
    brain_id: String,
    people: Arc<People>,
    people_file: PathBuf,
    stt: Arc<Phrases>,
    /// `media_next` (raccourci « suivant »).
    media: Arc<FakeSkill>,
    /// `media_play` : « lance la musique ».
    musique: Arc<FakeSkill>,
    /// Doublure de l'agent de code : `agent_start`, raccourci « confie une tâche à l'agent ».
    agent: Arc<FakeSkill>,
    suspend: Arc<Suspend>,
    /// À Charles, rattachée à lui.
    salon: Oreille,
    /// À Léa.
    chambre: Oreille,
    /// Appairée avant la phase 8 : jamais rattachée.
    ancienne: Oreille,
}

impl Rig {
    fn socket(&self) -> PathBuf {
        self.dir.path().join("run/brain.sock")
    }

    fn medias(&self) -> usize {
        self.media.calls().len()
    }

    fn musiques(&self) -> usize {
        self.musique.calls().len()
    }

    fn agents(&self) -> usize {
        self.agent.calls().len()
    }

    fn veilles(&self) -> usize {
        self.suspend.calls.load(Ordering::SeqCst)
    }
}

async fn rig() -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let identity = BrainIdentity::load_or_create(&dir.path().join("tls")).unwrap();
    let ears_file = dir.path().join("ears.toml");
    let mut registry = EarRegistry::load(&ears_file).unwrap();
    let (salon, salon_token) = registry.add("Salon", "linux", false).unwrap();
    let (chambre, chambre_token) = registry.add("Chambre", "android", false).unwrap();
    let (ancienne, ancienne_token) = registry.add("Ancienne", "linux", false).unwrap();

    let media = Arc::new(
        FakeSkill::new(&["media_next"]).with_shortcut("suivant", "media_next", serde_json::json!({})),
    );
    let musique = Arc::new(FakeSkill::new(&["media_play"]));
    let apps = Arc::new(FakeSkill::new(&["app_launch"]));
    let agent = Arc::new(
        FakeSkill::new(&["agent_start"])
            .with_shortcut("confie une tâche à l'agent", "agent_start", serde_json::json!({}))
            .with_llm_tools(false),
    );
    let suspend = Arc::new(Suspend::default());
    let llm = || Arc::new(FakeLlm::new(Ok(Understanding::Answer("x".into()))));
    let modes = Arc::new(ModeStore::new(dir.path().join("mode"), Mode::Local));
    let changes = modes.subscribe();

    // L'annuaire : Charles `admin` (créé par `open`), Léa rôle `famille`.
    let people_file = dir.path().join("people/people.toml");
    let people = Arc::new(People::open(&people_file, "Charles").unwrap());
    let skills: Vec<Arc<dyn Skill>> = vec![
        media.clone(),
        musique.clone(),
        apps,
        agent.clone(),
        suspend.clone(),
    ];
    let brain = Arc::new(Brain::new(skills, llm(), llm(), modes).with_people(people.clone()));
    people
        .add_role(
            "famille",
            vec!["media_*".into(), "app_launch".into()],
            false,
            false,
            &brain.tool_names(),
        )
        .unwrap();
    people.add_person("Léa", "famille").unwrap();
    people.attach("Charles", &salon.id).unwrap();
    people.attach("Léa", &chambre.id).unwrap();

    // Empreinte vocale de Charles : (1, 0).
    let gate = SpeakerGate::new(
        Arc::new(ParEchantillon),
        dir.path().join("voiceprint/charles.json"),
        0.55,
        0.65,
    );
    gate.enroll(&[vec![0.0; 16_000]]).unwrap();

    let stt = Arc::new(Phrases::default());
    let listen = Arc::new(
        ListenService::new(brain.clone(), stt.clone(), Arc::new(FakeDucker::new()))
            .with_speaker(Some(Arc::new(gate))),
    );
    let board = Arc::new(AnnouncementBoard::new(
        Arc::new(FakeRunner::new()),
        Arc::new(SystemClock),
    ));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
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
        window: Duration::from_millis(50),
        pairing_ttl: Duration::from_secs(120),
        process_timeout: net::hub::PROCESS_TIMEOUT,
    };
    let hub = Hub::spawn(settings, registry, services, None);
    let tls = identity.server_config().unwrap();
    tokio::spawn(net::serve_with_limits(
        listener,
        tls,
        hub.clone(),
        Limits::default(),
    ));
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
    for _ in 0..250 {
        if UnixStream::connect(&socket).await.is_ok() {
            return Rig {
                addr,
                fingerprint: identity.fingerprint,
                brain_id: identity.brain_id,
                people,
                people_file,
                stt,
                media,
                musique,
                agent,
                suspend,
                salon: (salon.id, salon_token),
                chambre: (chambre.id, chambre_token),
                ancienne: (ancienne.id, ancienne_token),
                dir,
            };
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("le socket Unix ne s'ouvre pas");
}

/// Client du socket Unix (une ligne JSON par requête et par réponse).
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

impl Ear {
    async fn connect(rig: &Rig, (ear_id, token): &Oreille) -> Ear {
        let pairing = Pairing {
            brain_id: rig.brain_id.clone(),
            brain_name: "Bureau".into(),
            ear_id: ear_id.clone(),
            token: token.clone(),
            fingerprint: rig.fingerprint,
            addresses: vec![rig.addr],
            manual: None,
        };
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
        let mut ear = Ear {
            link,
            events,
            _dir: dir,
        };
        loop {
            match tokio::time::timeout(WAIT, ear.events.recv()).await {
                Ok(Some(LinkEvent::Connected(_))) => return ear,
                Ok(Some(_)) => {}
                other => panic!("connexion attendue, reçu {other:?}"),
            }
        }
    }

    /// Prochain message du cerveau (hors `Status`), dans `wait`.
    async fn next_within(&mut self, wait: Duration) -> Option<BrainMessage> {
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            match tokio::time::timeout_at(deadline, self.events.recv()).await {
                Err(_) => return None,
                Ok(None) => panic!("lien de l'oreille fermé"),
                Ok(Some(LinkEvent::Message(BrainMessage::Status(_)))) => {}
                Ok(Some(LinkEvent::Message(m))) => return Some(m),
                Ok(Some(_)) => {}
            }
        }
    }

    async fn msg(&mut self) -> BrainMessage {
        self.next_within(WAIT)
            .await
            .expect("message du cerveau attendu")
    }

    /// Rien du cerveau (hors `Status`) pendant `QUIET`.
    async fn silent(&mut self) {
        if let Some(m) = self.next_within(QUIET).await {
            panic!("rien attendu, reçu {m:?}");
        }
    }

    fn say(&self, utterance: u32, phrase: i16, voix: Voix) -> Result<(), LinkDown> {
        let mut samples = vec![phrase; PACKET * PACKETS];
        samples[1] = match voix {
            Voix::Charles => 0,
            Voix::Inconnue => 20_000,
        };
        self.link.send_audio(utterance, &samples)?;
        self.link.send(EarMessage::ListenEnd {
            utterance,
            muted: false,
            no_speech: false,
            cut: None,
        })
    }

    /// « Jarvis » puis une phrase : la réponse du cerveau.
    async fn phrase(&mut self, utterance: u32, phrase: i16, voix: Voix) -> Reponse {
        self.link
            .send(EarMessage::Wake {
                utterance,
                score: 0.9,
                level_db: -30.0,
                muted: false,
            })
            .unwrap();
        assert_eq!(self.msg().await, BrainMessage::Elected { utterance });
        self.say(utterance, phrase, voix).unwrap();
        self.response(utterance).await
    }

    /// Réponse sans mot d'éveil (`AwaitReply`) : la réponse du cerveau.
    async fn reponse_sans_reveil(&mut self, utterance: u32, phrase: i16, voix: Voix) -> Reponse {
        self.link
            .send(EarMessage::ListenStart { utterance })
            .unwrap();
        assert_eq!(self.msg().await, BrainMessage::Elected { utterance });
        self.say(utterance, phrase, voix).unwrap();
        self.response(utterance).await
    }

    async fn response(&mut self, utterance: u32) -> Reponse {
        match self.msg().await {
            BrainMessage::Response {
                utterance: Some(u),
                response:
                    ControlResponse::Heard {
                        text,
                        speech,
                        ear_action,
                        ..
                    },
            } if u == utterance => Reponse {
                text,
                speech,
                ear_action,
            },
            other => panic!("réponse à la phrase {utterance} attendue, reçu {other:?}"),
        }
    }
}

struct Reponse {
    text: String,
    speech: String,
    ear_action: Option<EarAction>,
}

/// Veille demandée : la question est posée (`AwaitReply`).
async fn demande_veille(ear: &mut Ear, utterance: u32, voix: Voix) {
    let r = ear.phrase(utterance, VEILLE, voix).await;
    assert_eq!(r.text, "mets le PC en veille");
    assert_eq!(r.speech, "Je mets le PC en veille, Monsieur ?");
    assert_eq!(r.ear_action, Some(EarAction::AwaitReply));
}

#[tokio::test]
async fn oreille_de_lea_peut_lancer_une_musique() {
    let rig = rig().await;
    let mut chambre = Ear::connect(&rig, &rig.chambre).await;
    // Voix inconnue de l'empreinte, sur l'oreille de Léa : c'est Léa, elle a le droit.
    let r = chambre.phrase(1, MUSIQUE, Voix::Inconnue).await;
    assert_eq!(r.text, "lance la musique");
    assert_ne!(r.speech, REFUSED);
    assert_eq!(rig.musiques(), 1);
    assert_eq!(rig.musique.calls()[0].0, "media_play");
}

#[tokio::test]
async fn oreille_de_lea_veille_refusee_et_rien_execute() {
    let rig = rig().await;
    let mut chambre = Ear::connect(&rig, &rig.chambre).await;
    let r = chambre.phrase(1, VEILLE, Voix::Inconnue).await;
    assert_eq!(r.speech, REFUSED);
    assert_eq!(r.ear_action, None, "aucune question posée à Léa");
    // Aucune action en attente : un « oui » juste après ne fait rien.
    let r = chambre.reponse_sans_reveil(2, OUI, Voix::Inconnue).await;
    assert_ne!(r.speech, "Je mets le PC en veille.");
    assert_eq!(rig.veilles(), 0);
    // Même avec le droit d'outil et de confirmer, une voix non vérifiée n'obtient jamais
    // une action risquée.
    rig.people
        .edit_role(
            "famille",
            Some(vec!["*".into()]),
            Some(true),
            Some(true),
            &TOOLS.map(String::from),
        )
        .unwrap();
    let r = chambre.phrase(3, VEILLE, Voix::Inconnue).await;
    assert_eq!(r.ear_action, None);
    assert_ne!(r.speech, "Je mets le PC en veille, Monsieur ?");
    assert_eq!(rig.veilles(), 0);
}

#[tokio::test]
async fn agent_refuse_sur_l_oreille_de_lea() {
    let rig = rig().await;
    let mut chambre = Ear::connect(&rig, &rig.chambre).await;
    let r = chambre.phrase(1, AGENT, Voix::Inconnue).await;
    assert_eq!(r.speech, REFUSED);
    assert_eq!(rig.agents(), 0);
    // Rôle tout-puissant sauf l'identité : sa voix n'est pas reconnue, l'agent reste fermé.
    rig.people
        .edit_role(
            "famille",
            Some(vec!["*".into()]),
            Some(true),
            Some(true),
            &TOOLS.map(String::from),
        )
        .unwrap();
    let r = chambre.phrase(2, AGENT, Voix::Inconnue).await;
    assert_eq!(r.ear_action, None);
    assert_eq!(rig.agents(), 0);
    // Témoin : la doublure fonctionne, la voix reconnue de Charles la lance.
    let mut salon = Ear::connect(&rig, &rig.salon).await;
    salon.phrase(1, AGENT, Voix::Charles).await;
    assert_eq!(rig.agents(), 1);
}

#[tokio::test]
async fn voix_de_charles_sur_l_oreille_de_lea_vaut_charles() {
    let rig = rig().await;
    let mut chambre = Ear::connect(&rig, &rig.chambre).await;
    // L'action risquée est demandée puis confirmée, sur l'oreille de Léa.
    demande_veille(&mut chambre, 1, Voix::Charles).await;
    assert_eq!(rig.veilles(), 0);
    let r = chambre.reponse_sans_reveil(2, OUI, Voix::Charles).await;
    assert_eq!(r.speech, "Je mets le PC en veille.");
    assert_eq!(rig.veilles(), 1);
    // L'agent aussi.
    chambre.phrase(3, AGENT, Voix::Charles).await;
    assert_eq!(rig.agents(), 1);
}

#[tokio::test]
async fn voix_inconnue_sur_l_oreille_de_charles_ignoree_comme_avant() {
    let rig = rig().await;
    let mut salon = Ear::connect(&rig, &rig.salon).await;
    let r = salon.phrase(1, SUIVANT, Voix::Inconnue).await;
    assert_eq!((r.text.as_str(), r.speech.as_str()), ("", ""));
    // Ignorée avant toute transcription et toute action.
    assert_eq!(rig.stt.calls.load(Ordering::SeqCst), 0);
    assert_eq!(rig.medias(), 0);
    // Témoin : la voix de Charles passe sur la même oreille.
    let r = salon.phrase(2, SUIVANT, Voix::Charles).await;
    assert_eq!(r.text, "suivant");
    assert_eq!(rig.medias(), 1);
}

#[tokio::test]
async fn oui_de_lea_ne_confirme_pas_une_action_de_charles() {
    let rig = rig().await;
    let mut salon = Ear::connect(&rig, &rig.salon).await;
    let mut chambre = Ear::connect(&rig, &rig.chambre).await;
    demande_veille(&mut salon, 1, Voix::Charles).await;
    // « Oui » de Léa, entendu par l'autre oreille : rien.
    let r = chambre.reponse_sans_reveil(1, OUI, Voix::Inconnue).await;
    assert_eq!(r.speech, "");
    assert_eq!(rig.veilles(), 0);
    salon.silent().await;
    // Par sécurité l'attente est effacée : même le « oui » de Charles ne fait plus rien.
    let r = salon.reponse_sans_reveil(2, OUI, Voix::Charles).await;
    assert_ne!(r.speech, "Je mets le PC en veille.");
    assert_eq!(rig.veilles(), 0);
    // Une nouvelle demande de Charles se confirme normalement.
    demande_veille(&mut salon, 3, Voix::Charles).await;
    let r = salon.reponse_sans_reveil(4, OUI, Voix::Charles).await;
    assert_eq!(r.speech, "Je mets le PC en veille.");
    assert_eq!(rig.veilles(), 1);
}

#[tokio::test]
async fn gestion_par_le_socket_effective_a_la_phrase_suivante() {
    let rig = rig().await;
    let mut cli = Cli::connect(&rig).await;
    let mut chambre = Ear::connect(&rig, &rig.chambre).await;
    chambre.phrase(1, MUSIQUE, Voix::Inconnue).await;
    assert_eq!(rig.musiques(), 1);
    // Le rôle perd la musique : sans redémarrage, refusé à la phrase suivante.
    let r = cli
        .send(ControlRequest::RoleEdit {
            name: "famille".into(),
            tools: Some(vec!["app_launch".into()]),
            confirm: None,
            agent: None,
        })
        .await;
    assert!(matches!(r, ControlResponse::Reply { .. }), "{r:?}");
    let r = chambre.phrase(2, MUSIQUE, Voix::Inconnue).await;
    assert_eq!(r.speech, REFUSED);
    assert_eq!(rig.musiques(), 1);
    // Il la retrouve.
    let r = cli
        .send(ControlRequest::RoleEdit {
            name: "famille".into(),
            tools: Some(vec!["media_*".into()]),
            confirm: None,
            agent: None,
        })
        .await;
    assert!(matches!(r, ControlResponse::Reply { .. }), "{r:?}");
    chambre.phrase(3, MUSIQUE, Voix::Inconnue).await;
    assert_eq!(rig.musiques(), 2);
    // L'oreille de Léa passe à Charles : c'est désormais une oreille de Charles, où une
    // voix inconnue est ignorée (jamais les droits de l'administrateur pour autant).
    let r = cli
        .send(ControlRequest::PersonAttach {
            name: "Charles".into(),
            ear: "Chambre".into(),
        })
        .await;
    assert!(matches!(r, ControlResponse::Reply { .. }), "{r:?}");
    assert_eq!(rig.people.person_of(&rig.chambre.0), EarPerson::Owner);
    let calls = rig.stt.calls.load(Ordering::SeqCst);
    let r = chambre.phrase(4, MUSIQUE, Voix::Inconnue).await;
    assert_eq!(r.text, "");
    assert_eq!(rig.musiques(), 2);
    assert_eq!(rig.stt.calls.load(Ordering::SeqCst), calls);
    let r = chambre.phrase(5, VEILLE, Voix::Inconnue).await;
    assert_eq!(r.text, "");
    assert_eq!(rig.veilles(), 0);
}

type Ws = WebSocketStream<Box<dyn AsyncStream>>;

#[tokio::test]
async fn message_de_gestion_envoye_par_une_oreille_ignore() {
    let rig = rig().await;
    let stream = TlsConnector::new(pinned_client_config(&rig.fingerprint))
        .connect(rig.addr)
        .await
        .unwrap();
    let (mut ws, _): (Ws, _) = tokio_tungstenite::client_async("wss://jarvis-brain/", stream)
        .await
        .unwrap();
    let (id, token) = rig.chambre.clone();
    let hello = EarMessage::Hello {
        version: PROTOCOL_VERSION,
        ear_id: id,
        token,
        offline_phrase: None,
    };
    ws.send(Message::text(serde_json::to_string(&hello).unwrap()))
        .await
        .unwrap();
    // Messages de gestion que seul le socket Unix connaît : ignorés.
    for raw in [
        r#"{"type":"person_add","name":"Mallory","role":"admin"}"#,
        r#"{"type":"person_role","name":"Léa","role":"admin"}"#,
        r#"{"type":"role_edit","name":"famille","tools":["*"],"confirm":true,"agent":true}"#,
    ] {
        ws.send(Message::text(raw)).await.unwrap();
    }
    // Un `Status` demandé après : la réponse prouve que les messages précédents, traités
    // dans l'ordre sur la même connexion, l'ont été.
    ws.send(Message::text(
        serde_json::to_string(&EarMessage::Status).unwrap(),
    ))
    .await
    .unwrap();
    loop {
        let message = tokio::time::timeout(WAIT, ws.next())
            .await
            .expect("pas de réponse du cerveau")
            .expect("connexion fermée")
            .unwrap();
        if let Message::Text(text) = message
            && matches!(
                serde_json::from_str::<BrainMessage>(text.as_str()),
                Ok(BrainMessage::Response {
                    utterance: None,
                    response: ControlResponse::Status { .. }
                })
            )
        {
            break;
        }
    }
    let (people, roles) = (rig.people.people_summary(&|_| None, false), rig.people.roles_summary());
    let names: Vec<_> = people.iter().map(|p| p.name.clone()).collect();
    assert_eq!(names.len(), 2, "{names:?}");
    assert!(!names.iter().any(|n| n == "Mallory"), "{names:?}");
    let lea = people.iter().find(|p| p.name == "Léa").unwrap();
    assert_eq!(lea.role, "famille");
    let famille = roles.iter().find(|r| r.name == "famille").unwrap();
    assert!(!famille.confirm && !famille.agent);
    let file = std::fs::read_to_string(&rig.people_file).unwrap();
    assert!(!file.contains("Mallory"), "{file}");
}

#[tokio::test]
async fn oreille_non_rattachee_vaut_charles_apres_migration() {
    let rig = rig().await;
    // L'annuaire a été créé par `open` : l'ancienne oreille n'y est écrite nulle part.
    let file = std::fs::read_to_string(&rig.people_file).unwrap();
    assert!(!file.contains(rig.ancienne.0.as_str()), "{file}");
    assert_eq!(rig.people.person_of(&rig.ancienne.0), EarPerson::Owner);
    let mut ancienne = Ear::connect(&rig, &rig.ancienne).await;
    // Comme l'oreille de Charles : voix inconnue ignorée…
    let r = ancienne.phrase(1, SUIVANT, Voix::Inconnue).await;
    assert_eq!(r.text, "");
    assert_eq!(rig.medias(), 0);
    // … sa voix reconnue fait tout, y compris l'action risquée et l'agent.
    demande_veille(&mut ancienne, 2, Voix::Charles).await;
    let r = ancienne.reponse_sans_reveil(3, OUI, Voix::Charles).await;
    assert_eq!(r.speech, "Je mets le PC en veille.");
    assert_eq!(rig.veilles(), 1);
    ancienne.phrase(4, AGENT, Voix::Charles).await;
    assert_eq!(rig.agents(), 1);
}
