//! Le pire cas d'une phrase tient dans le délai `Listen` de l'oreille.
//!
//! L'oreille abandonne une phrase après `LISTEN_TIMEOUT` (jarvis-ear, client.rs) :
//! au-delà, Jarvis reste muet. Pire cas côté cerveau, mode par mode :
//!
//! ```text
//!   reconnaissance vocale (marge)           2 s
//! + compréhension par l'IA du mode         20 s en local (Ollama), 15 s en ligne (Claude)
//! + compétence la plus lente               15 s en local (recherche : 4 + 10 + 1)
//! + synthèse vocale                         8 s
//! ```
//!
//! Les délais des compétences sont lus sur les vraies compétences, réglages par
//! défaut. Les constantes privées (Ollama, Claude, synthèse, oreille) sont lues
//! dans leur source : jarvis-brain ne dépend pas de jarvis-ear, et ces constantes
//! ne sont pas publiques.

use jarvis_brain::config::Config;
use jarvis_brain::llm::Understanding;
use jarvis_brain::mode_store::ModeStore;
use jarvis_brain::summarizer::ModeSummarizer;
use jarvis_brain::testing::FakeLlm;
use jarvis_protocol::Mode;
use jarvis_skills::media::MprisPlayers;
use jarvis_skills::search::{SearchSkill, SearxClient};
use jarvis_skills::video::VideoSkill;
use jarvis_skills::{FakeRunner, Skill};
use std::sync::Arc;
use std::time::Duration;

/// Marge de la reconnaissance vocale (Whisper sur une phrase courte).
const STT_MARGIN: Duration = Duration::from_secs(2);

const OLLAMA_SRC: &str = include_str!("../src/llm/ollama.rs");
const CLAUDE_SRC: &str = include_str!("../src/llm/claude.rs");
const TTS_SRC: &str = include_str!("../src/tts/mod.rs");
const EAR_CLIENT_SRC: &str = include_str!("../../jarvis-ear/src/client.rs");

/// Valeur de `const <name>: Duration = Duration::from_secs(N);` dans `src`.
fn const_secs(src: &str, name: &str) -> Duration {
    let prefix = format!("const {name}: Duration = Duration::from_secs(");
    let line = src
        .lines()
        .map(str::trim)
        .find_map(|l| l.strip_prefix(&prefix))
        .unwrap_or_else(|| panic!("constante {name} introuvable"));
    let secs = line
        .split(')')
        .next()
        .and_then(|n| n.trim().parse().ok())
        .unwrap_or_else(|| panic!("constante {name} illisible : {line}"));
    Duration::from_secs(secs)
}

/// Délai le plus long des compétences de recherche et de vidéo, dans `mode`.
fn slowest_skill(mode: Mode) -> Duration {
    let settings = Config::default().search;
    let dir = tempfile::tempdir().unwrap();
    let modes = Arc::new(ModeStore::new(dir.path().join("mode"), mode));
    modes.set(mode).unwrap();
    let llm = || Arc::new(FakeLlm::new(Ok(Understanding::Answer("non".into()))));
    let summarizer = ModeSummarizer::new(llm(), llm(), modes, "Monsieur", &settings);
    let client = Arc::new(SearxClient::new(&settings).unwrap());
    let runner = Arc::new(FakeRunner::new());
    let search = SearchSkill::new(
        client.clone(),
        Arc::new(summarizer),
        runner.clone(),
        settings.clone(),
    );
    let video = VideoSkill::new(client, Arc::new(MprisPlayers::new()), runner, settings);
    let search = search.timeout("web_search").unwrap();
    let video = video.timeout("play_video").unwrap();
    search.max(video)
}

fn worst_case(mode: Mode) -> Duration {
    let llm = match mode {
        Mode::Local => const_secs(OLLAMA_SRC, "TIMEOUT"),
        Mode::Online => const_secs(CLAUDE_SRC, "TIMEOUT"),
    };
    STT_MARGIN + llm + slowest_skill(mode) + const_secs(TTS_SRC, "SYNTH_DEADLINE")
}

#[test]
fn constantes_lues_dans_les_sources() {
    assert_eq!(const_secs(OLLAMA_SRC, "TIMEOUT"), Duration::from_secs(20));
    assert_eq!(
        const_secs(TTS_SRC, "SYNTH_DEADLINE"),
        Duration::from_secs(8)
    );
    assert_eq!(slowest_skill(Mode::Local), Duration::from_secs(15));
}

#[test]
fn pire_cas_local_tient_dans_le_delai_d_ecoute_de_l_oreille() {
    let ear = const_secs(EAR_CLIENT_SRC, "LISTEN_TIMEOUT");
    let worst = worst_case(Mode::Local);
    assert_eq!(worst, Duration::from_secs(45));
    assert!(
        worst <= ear,
        "pire cas {worst:?} > délai de l'oreille {ear:?}"
    );
}

#[test]
fn pire_cas_en_ligne_tient_dans_le_delai_d_ecoute_de_l_oreille() {
    let ear = const_secs(EAR_CLIENT_SRC, "LISTEN_TIMEOUT");
    let worst = worst_case(Mode::Online);
    assert!(
        worst <= ear,
        "pire cas {worst:?} > délai de l'oreille {ear:?}"
    );
}
