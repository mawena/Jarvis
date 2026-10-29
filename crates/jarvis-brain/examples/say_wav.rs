//! Écrit une phrase dite par la voix de Jarvis dans un WAV 16 kHz mono, sans rien jouer.
//! Sert à essayer la reconnaissance vocale (`bench_stt`) sans micro.
//!
//! cargo run --release -p jarvis-brain --example say_wav -- <sortie.wav> "<phrase>"
//!
//! NATIVE=1 garde le débit de la voix (22 050 Hz) au lieu de 16 kHz.

use jarvis_brain::config::TtsConfig;
use jarvis_brain::tts::Synthesizer;
use jarvis_brain::tts::piper::PiperSynthesizer;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let usage = "usage : say_wav <sortie.wav> \"<phrase>\"";
    let out = args.next().ok_or_else(|| anyhow::anyhow!(usage))?;
    let text = args.next().ok_or_else(|| anyhow::anyhow!(usage))?;
    let voice = PiperSynthesizer::load(&TtsConfig::default())?;
    let speech = voice.synthesize(&text)?;
    // NATIVE=1 : garde le débit de la voix (pour rééchantillonner ailleurs, ex. ffmpeg).
    let native = std::env::var("NATIVE").is_ok_and(|v| v == "1");
    let rate = if native { speech.sample_rate } else { 16_000 };
    let samples = resample(&speech.samples, speech.sample_rate, rate);
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(&out, spec)?;
    // Un peu de silence autour, comme une vraie phrase coupée par la détection de parole.
    let pad = vec![0i16; rate as usize / 4];
    for s in pad.iter().chain(&samples).chain(&pad) {
        writer.write_sample(*s)?;
    }
    writer.finalize()?;
    Ok(())
}

/// Rééchantillonnage linéaire (suffisant pour un essai).
fn resample(input: &[i16], from: u32, to: u32) -> Vec<i16> {
    if from == to || input.is_empty() {
        return input.to_vec();
    }
    let len = (input.len() as u64 * to as u64 / from as u64) as usize;
    (0..len)
        .map(|i| {
            let pos = i as f64 * from as f64 / to as f64;
            let j = pos as usize;
            let frac = pos - j as f64;
            let a = input[j.min(input.len() - 1)] as f64;
            let b = input[(j + 1).min(input.len() - 1)] as f64;
            (a + (b - a) * frac) as i16
        })
        .collect()
}
