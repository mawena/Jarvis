//! Reconnaissance vocale locale avec whisper.cpp (via `whisper-rs`).

use super::Transcriber;
use crate::config::SttConfig;
use anyhow::Context;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

/// En dessous de 100 ms (1 600 échantillons), il n'y a rien à transcrire.
const MIN_SAMPLES: usize = 1_600;
/// Fenêtre complète de l'encodeur : 1 500 trames = 30 s (50 trames par seconde).
const FULL_AUDIO_CTX: i32 = 1_500;
/// Marge ajoutée à la fenêtre réduite, pour ne pas couper la fin de la phrase.
const AUDIO_CTX_MARGIN: i32 = 128;
/// Garde-fou contre les boucles de répétition (une commande tient en bien moins).
const MAX_TOKENS: i32 = 128;

pub struct WhisperTranscriber {
    ctx: Arc<WhisperContext>,
    threads: i32,
}

impl WhisperTranscriber {
    /// Charge le modèle en mémoire. Peut prendre plusieurs secondes : à appeler
    /// une seule fois, au démarrage, puis à réutiliser pour chaque phrase.
    pub fn load(cfg: &SttConfig) -> anyhow::Result<Self> {
        // Les journaux de whisper.cpp et de ggml passent par `tracing` (sans effet si répété).
        whisper_rs::install_logging_hooks();
        let path = expand_home(&cfg.model_path);
        if !path.is_file() {
            anyhow::bail!(
                "modèle Whisper introuvable : {} (lancez scripts/download-models.sh)",
                path.display()
            );
        }
        let mut params = WhisperContextParameters::default();
        params.flash_attn(true);
        let ctx = WhisperContext::new_with_params(&path, params)
            .map_err(|e| anyhow::anyhow!("{e:?}"))
            .with_context(|| format!("chargement du modèle Whisper {}", path.display()))?;
        Ok(Self {
            ctx: Arc::new(ctx),
            threads: cfg.threads.max(1),
        })
    }
}

fn is_too_short(audio: &[f32]) -> bool {
    audio.len() < MIN_SAMPLES
}

/// Whisper encode toujours 30 s d'audio ; pour une phrase courte, on réduit la
/// fenêtre à sa longueur réelle (plus une marge). Mesuré sur le PC de Charles
/// avec `small` : 0,9 à 2 s au lieu de 6 à 9 s (exemple `bench_stt`).
fn audio_ctx(samples: usize) -> i32 {
    let frames = i32::try_from(samples.div_ceil(320)).unwrap_or(FULL_AUDIO_CTX); // 16 000 / 50
    frames.saturating_add(AUDIO_CTX_MARGIN).min(FULL_AUDIO_CTX)
}

fn expand_home(p: &Path) -> PathBuf {
    if let Ok(rest) = p.strip_prefix("~")
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    p.to_path_buf()
}

#[async_trait::async_trait]
impl Transcriber for WhisperTranscriber {
    async fn transcribe(&self, audio: &[f32]) -> anyhow::Result<String> {
        if is_too_short(audio) {
            return Ok(String::new());
        }
        let ctx = Arc::clone(&self.ctx);
        let threads = self.threads;
        let audio = audio.to_vec();
        tokio::task::spawn_blocking(move || {
            let mut state = ctx
                .create_state()
                .map_err(|e| anyhow::anyhow!("état Whisper : {e:?}"))?;
            let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
            params.set_n_threads(threads);
            params.set_language(Some("fr"));
            params.set_translate(false);
            params.set_print_special(false);
            params.set_print_progress(false);
            params.set_print_realtime(false);
            params.set_print_timestamps(false);
            params.set_no_timestamps(true);
            params.set_no_context(true);
            params.set_single_segment(true);
            params.set_initial_prompt("Jarvis.");
            params.set_audio_ctx(audio_ctx(audio.len()));
            // Pas de nouvel essai à température plus haute : avec la fenêtre réduite,
            // ces essais faisaient monter certaines phrases à 7-12 s.
            params.set_temperature_inc(0.0);
            params.set_max_tokens(MAX_TOKENS);
            state
                .full(params, &audio)
                .map_err(|e| anyhow::anyhow!("transcription Whisper : {e:?}"))?;
            let mut text = String::new();
            for seg in state.as_iter() {
                text.push_str(&seg.to_str_lossy().map_err(|e| anyhow::anyhow!("{e:?}"))?);
            }
            Ok(text.trim().to_string())
        })
        .await
        .context("tâche de transcription interrompue")?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn audio_vide_ou_trop_court_ignore() {
        assert!(is_too_short(&[]));
        assert!(is_too_short(&vec![0.0; MIN_SAMPLES - 1]));
        assert!(!is_too_short(&vec![0.0; MIN_SAMPLES]));
    }

    #[test]
    fn fenetre_ajustee_a_la_longueur() {
        assert_eq!(audio_ctx(16_000), 50 + AUDIO_CTX_MARGIN);
        assert_eq!(audio_ctx(16_001), 51 + AUDIO_CTX_MARGIN);
        assert_eq!(audio_ctx(16_000 * 30), FULL_AUDIO_CTX);
        assert_eq!(audio_ctx(usize::MAX), FULL_AUDIO_CTX);
    }

    #[test]
    fn modele_absent_message_clair() {
        let cfg = SttConfig {
            model_path: PathBuf::from("/inexistant/ggml-absent.bin"),
            threads: 2,
        };
        let err = WhisperTranscriber::load(&cfg).err().expect("doit échouer");
        let msg = err.to_string();
        assert!(msg.contains("/inexistant/ggml-absent.bin"), "{msg}");
        assert!(msg.contains("scripts/download-models.sh"), "{msg}");
    }

    /// `JARVIS_TEST_WAV=/chemin/phrase.wav [JARVIS_TEST_MODEL=...] cargo test -p jarvis-brain
    /// whisper_transcrit_un_wav -- --ignored --nocapture` (WAV 16 kHz mono).
    #[tokio::test]
    #[ignore = "nécessite le modèle Whisper et JARVIS_TEST_WAV"]
    async fn whisper_transcrit_un_wav() {
        let wav = std::env::var("JARVIS_TEST_WAV").expect("JARVIS_TEST_WAV non défini");
        let mut cfg = SttConfig::default();
        if let Ok(m) = std::env::var("JARVIS_TEST_MODEL") {
            cfg.model_path = PathBuf::from(m);
        }
        let mut reader = hound::WavReader::open(&wav).unwrap();
        let spec = reader.spec();
        assert_eq!(
            (spec.sample_rate, spec.channels),
            (16_000, 1),
            "WAV 16 kHz mono requis"
        );
        let audio: Vec<f32> = match spec.sample_format {
            hound::SampleFormat::Int => reader
                .samples::<i16>()
                .map(|s| s.unwrap() as f32 / 32768.0)
                .collect(),
            hound::SampleFormat::Float => reader.samples::<f32>().map(|s| s.unwrap()).collect(),
        };
        let t0 = std::time::Instant::now();
        let stt = WhisperTranscriber::load(&cfg).unwrap();
        println!("chargement : {:?}", t0.elapsed());
        let t1 = std::time::Instant::now();
        let text = stt.transcribe(&audio).await.unwrap();
        println!(
            "transcription ({:.1} s d'audio) en {:?} : « {text} »",
            audio.len() as f32 / 16_000.0,
            t1.elapsed()
        );
        assert!(!text.is_empty());
    }
}
