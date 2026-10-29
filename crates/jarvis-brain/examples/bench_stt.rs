//! Mesure le temps de transcription de Whisper sur des fichiers WAV 16 kHz mono.
//!
//! cargo run --release -p jarvis-brain --example bench_stt -- <modèle.bin> <threads> <fichier.wav>...
//!
//! Variables facultatives :
//!   AUDIO_CTX=auto  fenêtre de l'encodeur ajustée à la longueur de l'audio (sinon 30 s) ;
//!   AUDIO_CTX=<n>   fenêtre fixe de n trames (1 500 = 30 s) ;
//!   FLASH=1         attention « flash » de whisper.cpp ;
//!   NO_FALLBACK=1   pas de nouvel essai à température plus haute quand le décodage hésite ;
//!   REPEAT=<n>      répète chaque fichier n fois ;
//!   PROMPT=<texte>  indice donné à Whisper (« Jarvis. » par défaut).

use std::time::Instant;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let usage = "usage : bench_stt <modèle.bin> <threads> <fichier.wav>...";
    let model = args.next().ok_or_else(|| anyhow::anyhow!(usage))?;
    let threads: i32 = args.next().ok_or_else(|| anyhow::anyhow!(usage))?.parse()?;
    let files: Vec<String> = args.collect();
    anyhow::ensure!(!files.is_empty(), usage);
    let audio_ctx = std::env::var("AUDIO_CTX").ok();
    let flash = std::env::var("FLASH").is_ok_and(|v| v == "1");
    let no_fallback = std::env::var("NO_FALLBACK").is_ok_and(|v| v == "1");
    let prompt = std::env::var("PROMPT").unwrap_or_else(|_| "Jarvis.".into());
    let repeat: usize = std::env::var("REPEAT").map_or(Ok(1), |v| v.parse())?;
    let files: Vec<String> = files
        .iter()
        .flat_map(|f| std::iter::repeat_n(f.clone(), repeat))
        .collect();

    let mut cparams = WhisperContextParameters::default();
    cparams.flash_attn(flash);
    let ctx =
        WhisperContext::new_with_params(&model, cparams).map_err(|e| anyhow::anyhow!("{e:?}"))?;

    let mut total = 0.0;
    for file in &files {
        let mut reader = hound::WavReader::open(file)?;
        let spec = reader.spec();
        anyhow::ensure!(
            spec.sample_rate == 16_000 && spec.channels == 1,
            "{file} : 16 kHz mono attendu"
        );
        let audio: Vec<f32> = reader
            .samples::<i16>()
            .map(|s| s.map(|s| s as f32 / i16::MAX as f32))
            .collect::<Result<_, _>>()?;
        let seconds = audio.len() as f32 / 16_000.0;

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
        params.set_initial_prompt(&prompt);
        if no_fallback {
            params.set_temperature_inc(0.0);
            params.set_max_tokens(128); // comme le cerveau
        }
        let ctx_frames = match audio_ctx.as_deref() {
            Some("auto") => Some(auto_audio_ctx(audio.len())),
            Some(n) => Some(n.parse()?),
            None => None,
        };
        if let Some(n) = ctx_frames {
            params.set_audio_ctx(n);
        }

        let t = Instant::now();
        let mut state = ctx.create_state().map_err(|e| anyhow::anyhow!("{e:?}"))?;
        state
            .full(params, &audio)
            .map_err(|e| anyhow::anyhow!("{e:?}"))?;
        let mut text = String::new();
        for seg in state.as_iter() {
            text.push_str(&seg.to_str_lossy().map_err(|e| anyhow::anyhow!("{e:?}"))?);
        }
        let secs = t.elapsed().as_secs_f32();
        total += secs;
        println!(
            "{secs:.2} s pour {seconds:.1} s d'audio (fenêtre {}) : {}",
            ctx_frames.map_or("30 s".into(), |n| format!("{n}")),
            text.trim()
        );
    }
    println!("moyenne : {:.2} s", total / files.len() as f32);
    Ok(())
}

/// 50 trames par seconde d'audio, plus une marge, sans dépasser 30 s (1 500 trames).
fn auto_audio_ctx(samples: usize) -> i32 {
    let frames = samples.div_ceil(320) as i32; // 16 000 / 50
    (frames + 128).min(1_500)
}
