//! Compétence `agent` branchée dans le cerveau : faux `claude`, dépôts temporaires,
//! jamais le vrai `claude`, jamais de vraie notification.

use jarvis_brain::announce::AnnouncementBoard;
use jarvis_brain::brain::{Brain, NEEDS_KNOWN_VOICE, SystemClock};
use jarvis_brain::listen::ListenService;
use jarvis_brain::llm::Understanding;
use jarvis_brain::mode_store::ModeStore;
use jarvis_brain::server::{SpeakerState, serve_with_board};
use jarvis_brain::speaker::{SpeakerGate, SpeakerVerifier};
use jarvis_brain::testing::{FakeDucker, FakeLlm, FakeTranscriber};
use jarvis_brain::tts::Voice;
use jarvis_protocol::{ControlRequest, ControlResponse, EarId, Mode};
use jarvis_skills::agent::registry::Registry;
use jarvis_skills::agent::testing::{fake_claude, git, result_line, shell_quote, temp_repo};
use jarvis_skills::agent::{AgentSettings, AgentSkill, Project};
use jarvis_skills::{Announcer, FakeRunner, RecordingAnnouncer, SystemRunner};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

const STARTED: &str = "Bien. Je confie cela à Claude, sur une branche à part de l'assistant. \
                       Je vous préviens à la fin.";

struct Setup {
    _repo: TempDir,
    repo: PathBuf,
    work: TempDir,
    brain: Arc<Brain>,
    local: Arc<FakeLlm>,
}

impl Setup {
    fn claude_ran(&self) -> bool {
        self.work.path().join("claude/args").exists()
    }

    fn worktrees(&self) -> usize {
        std::fs::read_dir(self.work.path().join("worktrees")).map_or(0, |d| d.count())
    }
}

/// Cerveau avec la seule compétence `agent` (projet « Jarvis » = dépôt temporaire).
fn setup(body: &str, announcer: Arc<dyn Announcer>, llm: Understanding) -> Setup {
    let (dir, repo) = temp_repo();
    let work = tempfile::tempdir().unwrap();
    let claude_dir = work.path().join("claude");
    std::fs::create_dir(&claude_dir).unwrap();
    let settings = AgentSettings {
        claude_path: fake_claude(&claude_dir, body),
        worktree_dir: work.path().join("worktrees"),
        cargo_target_dir: work.path().join("target"),
        extra_path: Vec::new(),
        projects: vec![Project {
            name: "Jarvis".into(),
            path: repo.clone(),
            aliases: vec!["l'assistant".into()],
            base: None,
            extra_allowed_tools: Vec::new(),
        }],
        ..AgentSettings::default()
    };
    let registry = Registry::open(work.path().join("tasks")).unwrap();
    let skill = AgentSkill::new(settings, Arc::new(SystemRunner), announcer, registry);
    let local = Arc::new(FakeLlm::new(Ok(llm.clone())));
    let online = Arc::new(FakeLlm::new(Ok(llm)));
    let brain = Arc::new(Brain::new(
        vec![Arc::new(skill)],
        local.clone(),
        online,
        Arc::new(ModeStore::new(work.path().join("mode"), Mode::Local)),
    ));
    Setup {
        _repo: dir,
        repo,
        work,
        brain,
        local,
    }
}

fn answer(text: &str) -> Understanding {
    Understanding::Answer(text.into())
}

/// Agent qui modifie un fichier et réussit.
fn successful_agent() -> String {
    let result = result_line(true, "RÉSUMÉ : J'ai fini les tests.");
    format!(
        "echo 'pub fn deux() -> u32 {{ 2 }}' >> lib.rs\nprintf '%s\\n' {}",
        shell_quote(&result)
    )
}

async fn connect(socket: &Path) -> BufReader<UnixStream> {
    for _ in 0..100 {
        if let Ok(s) = UnixStream::connect(socket).await {
            return BufReader::new(s);
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("le serveur ne démarre pas");
}

async fn send(c: &mut BufReader<UnixStream>, req: ControlRequest) -> ControlResponse {
    let line = serde_json::to_string(&req).unwrap();
    c.get_mut()
        .write_all(format!("{line}\n").as_bytes())
        .await
        .unwrap();
    let mut answer = String::new();
    tokio::time::timeout(std::time::Duration::from_secs(10), c.read_line(&mut answer))
        .await
        .expect("pas de réponse du serveur")
        .unwrap();
    serde_json::from_str(&answer).unwrap()
}

/// Attend que `cond` soit vraie (10 s au plus).
async fn until(mut cond: impl FnMut() -> bool) {
    for _ in 0..1000 {
        if cond() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("condition jamais remplie");
}

#[tokio::test]
async fn jarvis_say_lance_l_agent() {
    let notify = Arc::new(FakeRunner::new());
    let board = Arc::new(AnnouncementBoard::new(
        notify.clone(),
        Arc::new(SystemClock),
    ));
    let s = setup(&successful_agent(), board.clone(), answer("x"));
    let develop = git(&s.repo, &["rev-parse", "develop"]);
    let socket = s.work.path().join("brain.sock");
    let listen = Arc::new(ListenService::new(
        s.brain.clone(),
        Arc::new(FakeTranscriber::new("")),
        Arc::new(FakeDucker::new()),
    ));
    let (brain, path) = (s.brain.clone(), socket.clone());
    tokio::spawn(async move {
        serve_with_board(
            brain,
            listen,
            SpeakerState::Disabled,
            Voice::none(),
            board,
            &path,
        )
        .await
    });
    let mut c = connect(&socket).await;
    let r = send(
        &mut c,
        ControlRequest::Say {
            text: "Jarvis, finis les tests du projet Jarvis.".into(),
        },
    )
    .await;
    assert_eq!(
        r,
        ControlResponse::Reply {
            speech: STARTED.into()
        }
    );
    assert_eq!(s.local.calls(), 0, "l'IA n'est pas consultée");

    // La fin est relevée par l'oreille, et notifiée sur le bureau.
    let mut items = Vec::new();
    for _ in 0..500 {
        match send(&mut c, ControlRequest::Announcements { ear: None }).await {
            ControlResponse::Announcements { items: got } if !got.is_empty() => {
                items = got;
                break;
            }
            ControlResponse::Announcements { .. } => {}
            other => panic!("{other:?}"),
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(items.len(), 1, "aucune annonce de fin");
    assert_eq!(
        items[0].speech,
        "La tâche sur l'assistant est terminée : J'ai fini les tests. Un fichier modifié."
    );
    let calls = notify.calls();
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(calls[0].0, "notify-send");
    assert!(calls[0].1.contains(&"Tâche terminée : Jarvis".to_string()));
    // Travail enregistré sur sa branche ; develop et la copie de Charles intactes.
    assert_eq!(
        git(
            &s.repo,
            &["log", "-1", "--format=%s", "feature/jarvis-finis-tests"]
        ),
        "wip(jarvis): finis les tests du projet Jarvis"
    );
    assert_eq!(git(&s.repo, &["rev-parse", "develop"]), develop);
    assert_eq!(git(&s.repo, &["branch", "--show-current"]), "develop");
    assert_eq!(git(&s.repo, &["status", "--porcelain"]), "");
}

#[tokio::test]
async fn changer_de_mode_n_arrete_pas_les_taches() {
    let announcer = Arc::new(RecordingAnnouncer::new());
    let s = setup("sleep 30", announcer.clone(), answer("x"));
    let b = &s.brain;
    assert_eq!(
        b.handle_text("finis les tests du projet Jarvis")
            .await
            .speech,
        STARTED
    );
    b.set_mode(Mode::Online).unwrap();
    let r = b.handle_text("Jarvis, où en est la tâche ?").await;
    assert!(
        r.speech
            .starts_with("La tâche sur l'assistant tourne depuis"),
        "{}",
        r.speech
    );
    b.set_mode(Mode::Local).unwrap();
    let r = b.handle_text("où en est la tâche ?").await;
    assert!(r.speech.contains("tourne depuis"), "{}", r.speech);
    assert!(announcer.notices().is_empty());
    assert_eq!(
        b.handle_text("arrête la tâche").await.speech,
        "J'arrête la tâche sur l'assistant. Le travail reste sur sa branche."
    );
    until(|| !announcer.notices().is_empty()).await;
    assert!(!announcer.notices()[0].speak);
}

#[tokio::test]
async fn outils_agent_jamais_proposes_a_l_ia() {
    // Même si l'IA inventait l'outil, rien ne serait lancé.
    let invented = Understanding::Tool(jarvis_brain::intent::Intent::new(
        "agent_start",
        serde_json::json!({"project": "Jarvis", "instruction": "corrige le bug"}),
    ));
    let s = setup(
        &successful_agent(),
        Arc::new(RecordingAnnouncer::new()),
        invented,
    );
    assert!(
        s.brain
            .tools()
            .iter()
            .all(|t| !t.name.starts_with("agent_"))
    );
    // Verbe de code sans projet : l'IA répond, sans l'outil.
    let r = s.brain.handle_text("corrige le bug").await;
    assert_eq!(s.local.calls(), 1);
    assert!(
        s.local
            .last_tools()
            .iter()
            .all(|t| !t.starts_with("agent_"))
    );
    assert_ne!(r.speech, STARTED);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!s.claude_ran());
    assert_eq!(s.worktrees(), 0);
    assert_eq!(
        git(&s.repo, &["branch", "--format=%(refname:short)"]),
        "develop\nmain"
    );
}

/// Toujours la même empreinte : toute voix est celle de Charles.
struct SameVoice;

impl SpeakerVerifier for SameVoice {
    fn embed(&self, _audio: &[f32]) -> anyhow::Result<Vec<f32>> {
        Ok(vec![1.0, 0.0])
    }
}

/// Vérification prête ; `enrolled` : empreinte de Charles enregistrée.
fn gate(dir: &Path, enrolled: bool) -> SpeakerState {
    let g = SpeakerGate::new(Arc::new(SameVoice), dir.join("charles.json"), 0.55, 0.65);
    if enrolled {
        g.enroll(&[vec![0.0; 16_000]]).unwrap();
    }
    SpeakerState::Ready(Arc::new(g))
}

/// La phrase de départ dite à l'oreille, avec cette vérification du locuteur.
async fn heard(s: &Setup, speaker: SpeakerState) -> String {
    let listen = ListenService::new(
        s.brain.clone(),
        Arc::new(FakeTranscriber::new(
            "Jarvis, finis les tests du projet Jarvis",
        )),
        Arc::new(FakeDucker::new()),
    )
    .with_speaker_state(speaker);
    match listen
        .listen(&EarId::new("test"), &[0; 3 * 16_000], false, false)
        .await
    {
        ControlResponse::Heard { speech, .. } => speech,
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn voix_sans_empreinte_ne_lance_pas_l_agent() {
    let voices = tempfile::tempdir().unwrap();
    for speaker in [
        gate(voices.path(), false),
        SpeakerState::Unavailable { enrolled: false },
    ] {
        let s = setup(
            &successful_agent(),
            Arc::new(RecordingAnnouncer::new()),
            answer("x"),
        );
        assert_eq!(heard(&s, speaker).await, NEEDS_KNOWN_VOICE);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!s.claude_ran());
        assert_eq!(s.worktrees(), 0);
        assert_eq!(s.local.calls(), 0, "pas de repli sur l'IA");
    }
}

#[tokio::test]
async fn verification_desactivee_ne_lance_pas_l_agent() {
    let s = setup(
        &successful_agent(),
        Arc::new(RecordingAnnouncer::new()),
        answer("Réponse de l'IA."),
    );
    assert_eq!(heard(&s, SpeakerState::Disabled).await, NEEDS_KNOWN_VOICE);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!s.claude_ran());
    assert_eq!(s.worktrees(), 0);
    // Le reste marche comme avant : l'IA répond.
    let listen = ListenService::new(
        s.brain.clone(),
        Arc::new(FakeTranscriber::new("quel temps fait-il")),
        Arc::new(FakeDucker::new()),
    );
    match listen
        .listen(&EarId::new("test"), &[0; 3 * 16_000], false, false)
        .await
    {
        ControlResponse::Heard { speech, .. } => assert_eq!(speech, "Réponse de l'IA."),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn voix_reconnue_lance_l_agent() {
    let voices = tempfile::tempdir().unwrap();
    let announcer = Arc::new(RecordingAnnouncer::new());
    let s = setup(&successful_agent(), announcer.clone(), answer("x"));
    assert_eq!(heard(&s, gate(voices.path(), true)).await, STARTED);
    until(|| !announcer.notices().is_empty()).await;
    assert!(s.claude_ran());
}

#[tokio::test]
async fn texte_tape_lance_l_agent() {
    let announcer = Arc::new(RecordingAnnouncer::new());
    let s = setup(&successful_agent(), announcer.clone(), answer("x"));
    assert_eq!(
        s.brain
            .handle_text("finis les tests du projet Jarvis")
            .await
            .speech,
        STARTED
    );
    until(|| !announcer.notices().is_empty()).await;
    assert!(s.claude_ran());
}

#[tokio::test]
async fn voix_non_verifiee_ne_lance_pas_l_agent() {
    let s = setup(
        &successful_agent(),
        Arc::new(RecordingAnnouncer::new()),
        answer("x"),
    );
    for phrase in [
        "Jarvis, finis les tests du projet Jarvis",
        "demande à l'agent de corriger le projet Jarvis",
        "arrête la tâche",
    ] {
        assert_eq!(
            s.brain.answer_fast(&EarId::new("test"), phrase).await,
            None,
            "{phrase}"
        );
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!s.claude_ran());
    assert_eq!(s.worktrees(), 0);
    assert_eq!(s.local.calls(), 0);
}
