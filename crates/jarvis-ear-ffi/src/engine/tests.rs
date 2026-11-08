//! Tests du moteur : faux hôte Kotlin, faux lien, faux détecteur (rien de réel).

use super::*;
use jarvis_ear_core::net::{LinkDown, LinkState, Pairing};
use jarvis_ear_core::session::sounds;
use jarvis_protocol::ear::{BrainMessage, EarMessage, StatusInfo};
use jarvis_protocol::{
    Announcement, ControlResponse, EarId, Fingerprint, Mode, Sound, SpeechAudio, encode_pcm,
};

use crate::{BrainView, MicView};

#[derive(Debug, Clone, PartialEq)]
enum Call {
    StartCapture,
    StopCapture,
    Play {
        len: usize,
        rate: u32,
        generation: u64,
        first: i16,
    },
    StopPlayback,
    Status(EarView),
}

#[derive(Default)]
struct FakeHost {
    calls: Mutex<Vec<Call>>,
}

impl FakeHost {
    fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }
    fn count(&self, wanted: &Call) -> usize {
        self.calls().iter().filter(|c| *c == wanted).count()
    }
    fn plays(&self) -> Vec<Call> {
        self.calls()
            .into_iter()
            .filter(|c| matches!(c, Call::Play { .. }))
            .collect()
    }
    fn last_status(&self) -> Option<EarView> {
        self.calls().into_iter().rev().find_map(|c| match c {
            Call::Status(view) => Some(view),
            _ => None,
        })
    }
    fn statuses(&self) -> usize {
        self.calls()
            .iter()
            .filter(|c| matches!(c, Call::Status(_)))
            .count()
    }
}

impl EarHost for FakeHost {
    fn start_capture(&self) {
        self.calls.lock().unwrap().push(Call::StartCapture);
    }
    fn stop_capture(&self) {
        self.calls.lock().unwrap().push(Call::StopCapture);
    }
    fn play_pcm(&self, pcm_le: Vec<u8>, sample_rate: u32, generation: u64) {
        let first = i16::from_le_bytes([pcm_le[0], pcm_le[1]]);
        self.calls.lock().unwrap().push(Call::Play {
            len: pcm_le.len() / 2,
            rate: sample_rate,
            generation,
            first,
        });
    }
    fn stop_playback(&self) {
        self.calls.lock().unwrap().push(Call::StopPlayback);
    }
    fn status_changed(&self, view: EarView) {
        self.calls.lock().unwrap().push(Call::Status(view));
    }
}

/// Détecteur qui se déclenche toutes les `every` trames de 160 échantillons et compte
/// ce qu'il voit.
struct FakeWake {
    every: usize,
    frame: usize,
    seen: Arc<AtomicUsize>,
}

impl FakeWake {
    fn new(every: usize) -> (Self, Arc<AtomicUsize>) {
        let seen = Arc::new(AtomicUsize::new(0));
        (
            FakeWake {
                every,
                frame: 0,
                seen: seen.clone(),
            },
            seen,
        )
    }
    fn never() -> Self {
        Self::new(usize::MAX).0
    }
}

impl WakeDetector for FakeWake {
    fn frame_len(&self) -> usize {
        160
    }
    fn process(&mut self, frame: &[f32]) -> Option<f32> {
        self.seen.fetch_add(frame.len(), Ordering::SeqCst);
        self.frame += 1;
        self.frame.is_multiple_of(self.every).then_some(0.9)
    }
}

struct FakeLink {
    sent: Mutex<Vec<EarMessage>>,
    events: mpsc::Sender<LinkEvent>,
    state: watch::Sender<LinkState>,
    closed: AtomicBool,
}

impl FakeLink {
    fn reply(&self, message: BrainMessage) {
        self.events
            .try_send(LinkEvent::Message(message))
            .expect("événement transmis");
    }
}

#[async_trait::async_trait]
impl Link for FakeLink {
    fn send(&self, message: EarMessage) -> Result<(), LinkDown> {
        self.sent.lock().unwrap().push(message.clone());
        if let EarMessage::Wake { utterance, .. } = message {
            self.reply(BrainMessage::Elected { utterance });
        }
        Ok(())
    }
    fn send_audio(&self, _: u32, _: &[i16]) -> Result<(), LinkDown> {
        Ok(())
    }
    fn nudge(&self) {}
    fn state(&self) -> watch::Receiver<LinkState> {
        self.state.subscribe()
    }
    async fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }
}

fn status_info(mode: Mode) -> StatusInfo {
    StatusInfo {
        mode,
        version: "test".into(),
        local_model: "llama3.2:3b".into(),
        ears: vec![],
    }
}

struct Bench {
    engine: Arc<EarEngine>,
    host: Arc<FakeHost>,
    link: Arc<FakeLink>,
    dir: tempfile::TempDir,
}

fn settings(dir: &std::path::Path) -> EarSettings {
    EarSettings {
        data_dir: dir.to_string_lossy().into_owned(),
        name: "Pixel essai".into(),
        output_rate: 44_100,
        wake_threshold: 0.5,
    }
}

fn link_pair() -> (Arc<FakeLink>, mpsc::Receiver<LinkEvent>) {
    let (events_tx, events) = mpsc::channel(64);
    let link = Arc::new(FakeLink {
        sent: Mutex::new(Vec::new()),
        events: events_tx,
        state: watch::channel(LinkState::Connected(status_info(Mode::Local))).0,
        closed: AtomicBool::new(false),
    });
    (link, events)
}

fn bench_in(dir: tempfile::TempDir, wake: impl WakeDetector + 'static) -> Bench {
    let host = Arc::new(FakeHost::default());
    let (link, events) = link_pair();
    let engine = EarEngine::with_parts(
        settings(dir.path()),
        host.clone(),
        link.clone(),
        events,
        wake,
    );
    Bench {
        engine,
        host,
        link,
        dir,
    }
}

fn bench(wake: impl WakeDetector + 'static) -> Bench {
    bench_in(tempfile::tempdir().unwrap(), wake)
}

/// Moteur démarré, micro ouvert côté Kotlin (16 kHz mono annoncé).
fn started(wake: impl WakeDetector + 'static) -> Bench {
    let b = bench(wake);
    b.engine.start().unwrap();
    wait("micro ouvert", || b.host.count(&Call::StartCapture) == 1);
    b.engine.audio_format(16_000, 1);
    b
}

fn wait(what: &str, cond: impl Fn() -> bool) {
    for _ in 0..500 {
        if cond() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("délai dépassé : {what}");
}

/// Un bloc de 20 ms à 16 kHz, valeur constante.
fn block(value: i16) -> Vec<u8> {
    std::iter::repeat_n(value.to_le_bytes(), 320)
        .flatten()
        .collect()
}

fn voice() -> SpeechAudio {
    SpeechAudio {
        pcm: encode_pcm(&[1_000; 2_205]),
        sample_rate: 22_050,
    }
}

fn announce(link: &FakeLink) {
    link.reply(BrainMessage::Response {
        utterance: None,
        response: ControlResponse::Announcements {
            items: vec![Announcement {
                speech: "La tâche est finie.".into(),
                audio: Some(voice()),
                sound: Some(Sound::Success),
            }],
        },
    });
}

fn is_playing(b: &Bench) -> bool {
    *b.engine.shared.playing.borrow()
}

// --- son -----------------------------------------------------------------------

#[test]
fn trois_secondes_de_zeros_micro_bloque() {
    let b = started(FakeWake::never());
    wait("vue initiale", || b.host.last_status().is_some());
    // 2,8 s de zéros : pas encore.
    for _ in 0..140 {
        b.engine.push_audio(block(0));
    }
    assert_eq!(b.engine.view().mic, MicView::Open);
    // Les 3 s sont dépassées.
    for _ in 0..20 {
        b.engine.push_audio(block(0));
    }
    let view = b.engine.view();
    assert_eq!(view.mic, MicView::Blocked);
    assert!(view.attention);
    let sent = b.host.last_status().unwrap();
    assert_eq!(sent.mic, MicView::Blocked);
    assert!(
        sent.text.starts_with("Micro bloqué par Android"),
        "{}",
        sent.text
    );
}

#[test]
fn vrai_son_debloque() {
    let b = started(FakeWake::never());
    for _ in 0..160 {
        b.engine.push_audio(block(0));
    }
    assert_eq!(b.engine.view().mic, MicView::Blocked);
    b.engine.push_audio(block(500));
    assert_eq!(b.engine.view().mic, MicView::Open);
    let sent = b.host.last_status().unwrap();
    assert_eq!(sent.mic, MicView::Open);
    assert!(sent.text.starts_with("À l'écoute"), "{}", sent.text);
    // Les zéros isolés d'un vrai micro ne cumulent pas.
    for _ in 0..400 {
        let mut pcm = block(300);
        pcm[0] = 0;
        pcm[1] = 0;
        b.engine.push_audio(pcm);
    }
    assert_eq!(b.engine.view().mic, MicView::Open);
}

#[test]
fn octets_impairs_ignores() {
    let (wake, seen) = FakeWake::new(usize::MAX);
    let b = started(wake);
    b.engine.push_audio(vec![7]);
    b.engine.push_audio(Vec::new());
    // 320 échantillons et un octet de trop : 2 trames de 160, rien de plus.
    let mut pcm = block(1_000);
    pcm.push(9);
    b.engine.push_audio(pcm);
    wait("son vu", || seen.load(Ordering::SeqCst) >= 160);
    std::thread::sleep(Duration::from_millis(100));
    let seen = seen.load(Ordering::SeqCst);
    assert!(seen <= 320, "{seen}");
    // L'octet de trop est écarté à la conversion.
    assert_eq!(crate::enroll::pcm_to_f32(&[1, 0, 2]).len(), 1);
}

#[test]
fn push_audio_hors_service_ignore() {
    let b = bench(FakeWake::never());
    // Avant `start`.
    b.engine.audio_format(16_000, 1);
    b.engine.push_audio(block(1_000));
    b.engine.capture_unavailable();
    b.engine.playback_idle(3);
    b.engine.mute();
    b.engine.hard_mute();
    b.engine.reopen();
    assert!(b.host.calls().is_empty(), "{:?}", b.host.calls());
    // Après `stop`.
    b.engine.start().unwrap();
    wait("micro ouvert", || b.host.count(&Call::StartCapture) == 1);
    b.engine.stop();
    b.engine.audio_format(16_000, 1);
    b.engine.push_audio(block(0));
    b.engine.push_audio(vec![1]);
    b.engine.capture_unavailable();
    b.engine.mute();
    assert_eq!(b.engine.view().mic, MicView::Open);
}

#[test]
fn format_48k_reechantillonne_en_16k() {
    let (wake, seen) = FakeWake::new(usize::MAX);
    let b = bench(wake);
    b.engine.start().unwrap();
    wait("micro ouvert", || b.host.count(&Call::StartCapture) == 1);
    b.engine.audio_format(48_000, 1);
    // 1 s à 48 kHz, blocs de 20 ms.
    for _ in 0..50 {
        let pcm: Vec<u8> = std::iter::repeat_n(1_000i16.to_le_bytes(), 960)
            .flatten()
            .collect();
        b.engine.push_audio(pcm);
    }
    wait("son vu", || seen.load(Ordering::SeqCst) >= 15_000);
    std::thread::sleep(Duration::from_millis(100));
    let seen = seen.load(Ordering::SeqCst);
    assert!((15_000..=16_100).contains(&seen), "{seen}");
}

#[test]
fn fin_d_une_ancienne_lecture_ignoree() {
    let b = started(FakeWake::never());
    announce(&b.link);
    wait("deux sons joués", || b.host.plays().len() == 2);
    let generations: Vec<u64> = b
        .host
        .plays()
        .iter()
        .map(|c| match c {
            Call::Play { generation, .. } => *generation,
            _ => unreachable!(),
        })
        .collect();
    assert_eq!(generations, [1, 2]);
    assert!(is_playing(&b));
    // La fin du bip (génération 1) arrive alors que la voix (2) est en cours.
    b.engine.playback_idle(1);
    assert!(is_playing(&b), "la voix de Jarvis parle encore");
    b.engine.playback_idle(0);
    assert!(is_playing(&b));
}

#[test]
fn fin_de_la_derniere_lecture_repasse_au_repos() {
    let b = started(FakeWake::never());
    announce(&b.link);
    wait("deux sons joués", || b.host.plays().len() == 2);
    b.engine.playback_idle(2);
    assert!(!is_playing(&b));
}

#[test]
fn bip_de_reveil_envoye_a_la_frequence_de_sortie() {
    let b = started(FakeWake::new(10).0);
    for _ in 0..10 {
        b.engine.push_audio(block(1_000));
    }
    let beep = sounds::wake(44_100);
    wait("bip de réveil", || {
        b.host.plays().first()
            == Some(&Call::Play {
                len: beep.len(),
                rate: 44_100,
                generation: 1,
                first: f32_to_i16(beep[0]),
            })
    });
    assert!(
        b.link
            .sent
            .lock()
            .unwrap()
            .iter()
            .any(|m| matches!(m, EarMessage::Wake { .. }))
    );
}

#[test]
fn voix_de_jarvis_transmise_a_sa_frequence() {
    let b = started(FakeWake::never());
    announce(&b.link);
    wait("voix jouée", || {
        b.host.plays().contains(&Call::Play {
            len: 2_205,
            rate: 22_050,
            generation: 2,
            first: 1_000,
        })
    });
}

// --- cycle ---------------------------------------------------------------------

fn pairing() -> Pairing {
    Pairing {
        brain_id: "b1".into(),
        brain_name: "PC de Charles".into(),
        ear_id: EarId::new("e1"),
        token: "jeton".into(),
        fingerprint: Fingerprint::of_der(b"cert"),
        addresses: vec!["127.0.0.1:7341".parse().unwrap()],
        manual: None,
    }
}

fn no_host() -> Arc<dyn EarHost> {
    Arc::new(FakeHost::default())
}

#[test]
fn moteur_sans_appairage_not_paired() {
    let dir = tempfile::tempdir().unwrap();
    let err = EarEngine::new(settings(dir.path()), no_host())
        .err()
        .unwrap();
    assert!(matches!(err, EarError::NotPaired), "{err:?}");
}

#[test]
fn moteur_sans_mot_d_eveil_no_wakeword() {
    let dir = tempfile::tempdir().unwrap();
    store_of(dir.path()).save(&pairing()).unwrap();
    let err = EarEngine::new(settings(dir.path()), no_host())
        .err()
        .unwrap();
    assert!(matches!(err, EarError::NoWakeword), "{err:?}");
}

#[test]
fn start_deux_fois_un_seul_lien() {
    let host = Arc::new(FakeHost::default());
    let dir = tempfile::tempdir().unwrap();
    let (link, events) = link_pair();
    let created = Arc::new(AtomicUsize::new(0));
    let counter = created.clone();
    let engine = EarEngine::build(
        settings(dir.path()),
        host.clone(),
        Box::new(FakeWake::never()),
        Box::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
            (link, events)
        }),
    );
    engine.start().unwrap();
    engine.start().unwrap();
    wait("micro ouvert", || host.count(&Call::StartCapture) >= 1);
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(created.load(Ordering::SeqCst), 1);
    assert_eq!(host.count(&Call::StartCapture), 1);
    engine.stop();
}

#[test]
fn stop_idempotent_et_borne() {
    let b = started(FakeWake::never());
    let begin = std::time::Instant::now();
    b.engine.stop();
    b.engine.stop();
    assert!(begin.elapsed() < STOP_TIMEOUT, "{:?}", begin.elapsed());
    assert!(b.link.closed.load(Ordering::SeqCst), "lien fermé");
    assert_eq!(b.host.count(&Call::StopCapture), 1);
    // Un moteur arrêté ne repart pas : il faut en créer un autre.
    assert!(matches!(b.engine.start(), Err(EarError::Internal { .. })));
    // Arrêt avant démarrage : sans effet ni attente.
    bench(FakeWake::never()).engine.stop();
}

#[test]
fn coupure_totale_ferme_le_micro() {
    let b = started(FakeWake::never());
    b.engine.hard_mute();
    wait("micro fermé", || b.host.count(&Call::StopCapture) == 1);
    wait("vue coupure totale", || {
        b.host
            .last_status()
            .is_some_and(|v| v.mic == MicView::HardMuted)
    });
    assert_eq!(b.engine.view().mic, MicView::HardMuted);
    // Le son qui traîne ensuite est ignoré.
    b.engine.push_audio(block(0));
}

#[test]
fn reouverture_rouvre_le_micro() {
    let b = started(FakeWake::never());
    b.engine.hard_mute();
    wait("micro fermé", || b.host.count(&Call::StopCapture) == 1);
    b.engine.reopen();
    wait("micro rouvert", || b.host.count(&Call::StartCapture) == 2);
    wait("vue à l'écoute", || {
        b.host.last_status().is_some_and(|v| v.mic == MicView::Open)
    });
}

#[test]
fn ecoute_coupee_retenue_au_redemarrage() {
    let b = started(FakeWake::never());
    b.engine.mute();
    wait("écoute coupée", || b.engine.view().mic == MicView::Muted);
    let muted_file = b.dir.path().join("muted");
    wait("fichier écrit", || muted_file.exists());
    b.engine.stop();
    // Nouveau moteur sur le même dossier.
    let again = bench_in(b.dir, FakeWake::never());
    assert_eq!(again.engine.view().mic, MicView::Muted);
    again.engine.start().unwrap();
    wait("vue au démarrage", || {
        again
            .host
            .last_status()
            .is_some_and(|v| v.text.starts_with("Écoute coupée"))
    });
    again.engine.unmute();
    wait("écoute rétablie", || {
        !again.dir.path().join("muted").exists()
    });
}

#[test]
fn vue_envoyee_au_demarrage_et_seulement_quand_elle_change() {
    let b = started(FakeWake::never());
    wait("vue initiale", || b.host.statuses() >= 1);
    wait("lien établi", || {
        b.host
            .last_status()
            .is_some_and(|v| v.brain == BrainView::Connected { online: false })
    });
    let before = b.host.statuses();
    // Un son qui passe ne change pas la vue.
    for _ in 0..20 {
        b.engine.push_audio(block(1_000));
    }
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(b.host.statuses(), before);
}
