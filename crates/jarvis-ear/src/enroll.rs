//! Enrôlement : enregistre « Jarvis » plusieurs fois et construit `jarvis.rpw` (les WAV
//! des prises « Jarvis » sont les seuls sons conservés sur disque), puis fait lire
//! quelques phrases pour l'empreinte vocale. Les phrases restent en mémoire : elles
//! partent vers le cerveau local et ne sont jamais écrites sur disque.

use std::io::{BufRead, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, anyhow};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use jarvis_ear_core::SAMPLE_RATE;
use jarvis_ear_core::resample::Downmixer;
use jarvis_ear_core::resample::f32_to_i16;
use jarvis_ear_core::rustpotter::build_reference;
use jarvis_protocol::{ControlRequest, ControlResponse, encode_pcm};

use tokio::signal::unix::{SignalKind, signal};

use crate::app::{Brain, SocketBrain};
use crate::client::ClientError;
use crate::service::{EarPause, RealSystemctl};

/// Pause entre l'appui sur Entrée et le début de la prise (le bruit de la touche,
/// puis le temps de se préparer, ne coupent pas le début de « Jarvis »).
const START_DELAY: Duration = Duration::from_millis(800);

/// Durée de chaque prise (secondes).
const TAKE_SECONDS: f32 = 1.5;

/// Durée de chaque phrase de l'empreinte vocale (secondes).
const PHRASE_SECONDS: f32 = 3.5;

/// Phrases à lire pour l'empreinte vocale.
pub const PHRASES: [&str; 6] = [
    "Jarvis, mets la musique suivante",
    "Quelle est la météo demain ?",
    "Jarvis, lance Visual Studio Code",
    "Baisse un peu le son s'il te plaît",
    "Raconte-moi une blague",
    "Jarvis, coupe l'écoute",
];

/// Dossier `~/.local/share/jarvis/wakeword`.
pub fn wakeword_dir() -> anyhow::Result<PathBuf> {
    let base = directories::BaseDirs::new().context("dossier personnel introuvable")?;
    Ok(base.data_dir().join("jarvis").join("wakeword"))
}

/// Nom de fichier de la prise `index` (à partir de 1).
pub fn sample_file_name(index: usize) -> String {
    format!("jarvis-{index:02}.wav")
}

/// Nombre d'échantillons mono 16 kHz d'une prise.
#[cfg(test)]
pub fn samples_per_take() -> usize {
    (SAMPLE_RATE as f32 * TAKE_SECONDS) as usize
}

/// Vrai pour un nom de la forme `jarvis-NN.wav` (deux chiffres).
fn is_take_file(name: &str) -> bool {
    name.strip_prefix("jarvis-")
        .and_then(|n| n.strip_suffix(".wav"))
        .is_some_and(|n| n.len() == 2 && n.bytes().all(|b| b.is_ascii_digit()))
}

/// Supprime les anciennes prises `jarvis-NN.wav` (un `--count` plus petit ne doit pas
/// laisser d'anciens échantillons), et elles seules. Renvoie le nombre de fichiers supprimés.
pub fn clear_samples(dir: &Path) -> anyhow::Result<usize> {
    let mut removed = 0;
    for path in list_samples(dir)? {
        let old_take = path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(is_take_file);
        if old_take {
            std::fs::remove_file(&path)?;
            removed += 1;
        }
    }
    Ok(removed)
}

/// Crée le dossier (et ses parents) en 0700 ; un dossier existant passe aussi en 0700.
fn create_private_dir(path: &Path) -> std::io::Result<()> {
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
}

/// Écrit une prise en WAV 16 bits mono 16 kHz, lisible par moi seul (0600).
pub fn write_wav(path: &Path, samples: &[f32]) -> anyhow::Result<()> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    // `mode` ne vaut qu'à la création : un ancien fichier garderait ses droits.
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    let mut writer = hound::WavWriter::new(std::io::BufWriter::new(file), spec)?;
    for s in samples {
        writer.write_sample((s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)?;
    }
    writer.finalize()?;
    Ok(())
}

/// Liste triée des WAV d'échantillons d'un dossier.
pub fn list_samples(dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "wav"))
        .collect();
    files.sort();
    Ok(files)
}

/// Construit `jarvis.rpw` (0600) à partir des WAV du dossier d'échantillons.
pub fn build_model(samples_dir: &Path, output: &Path) -> anyhow::Result<usize> {
    let files = list_samples(samples_dir)?;
    build_reference(&files, output)?;
    std::fs::set_permissions(output, std::fs::Permissions::from_mode(0o600))?;
    Ok(files.len())
}

/// Enregistre `seconds` secondes au format natif du micro (renvoyées en mono 16 kHz).
fn record_audio(seconds: f32) -> anyhow::Result<Vec<f32>> {
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or_else(|| anyhow!("aucun micro disponible"))?;
    let supported = device.default_input_config()?;
    let channels = supported.channels() as usize;
    let rate = supported.sample_rate();
    let config = supported.config();
    let wanted = (rate as f32 * seconds) as usize * channels;
    // Capacité réservée d'avance : le callback n'alloue pas.
    let buffer = Arc::new(Mutex::new(Vec::<f32>::with_capacity(wanted + 16_384)));
    let sink = buffer.clone();
    let on_err = |e| eprintln!("erreur micro : {e}");

    macro_rules! stream {
        ($t:ty) => {
            device.build_input_stream(
                config,
                move |data: &[$t], _| {
                    if let Ok(mut b) = sink.try_lock() {
                        let room = wanted.saturating_sub(b.len());
                        b.extend(
                            data.iter()
                                .take(room)
                                .map(|s| cpal::Sample::to_sample::<f32>(*s)),
                        );
                    }
                },
                on_err,
                None,
            )?
        };
    }
    let stream = match supported.sample_format() {
        cpal::SampleFormat::F32 => stream!(f32),
        cpal::SampleFormat::I16 => stream!(i16),
        cpal::SampleFormat::I32 => stream!(i32),
        cpal::SampleFormat::U16 => stream!(u16),
        other => return Err(anyhow!("format de micro non géré : {other:?}")),
    };
    stream.play()?;
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while buffer.lock().map_err(|_| anyhow!("verrou"))?.len() < wanted {
        if std::time::Instant::now() > deadline {
            return Err(anyhow!("le micro ne délivre pas d'audio"));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(stream);
    let raw = buffer.lock().map_err(|_| anyhow!("verrou"))?.clone();
    Ok(Downmixer::new(rate, channels as u16).push(&raw))
}

/// Interaction avec la personne qui enregistre (remplaçable dans les tests).
pub trait Session {
    /// Affiche une ligne.
    fn say(&mut self, line: &str);
    /// Attend que la personne soit prête (Entrée), puis laisse un court délai.
    fn wait_ready(&mut self, prompt: &str) -> anyhow::Result<()>;
    /// Enregistre `seconds` secondes : mono 16 kHz.
    fn record(&mut self, seconds: f32) -> anyhow::Result<Vec<f32>>;
}

/// Session réelle : terminal et vrai micro.
struct Terminal;

impl Session for Terminal {
    fn say(&mut self, line: &str) {
        println!("{line}");
    }

    fn wait_ready(&mut self, prompt: &str) -> anyhow::Result<()> {
        print!("{prompt}");
        std::io::stdout().flush()?;
        std::io::stdin().lock().read_line(&mut String::new())?;
        std::thread::sleep(START_DELAY);
        print!("parle… ");
        std::io::stdout().flush()?;
        Ok(())
    }

    fn record(&mut self, seconds: f32) -> anyhow::Result<Vec<f32>> {
        let audio = record_audio(seconds)?;
        println!("ok");
        Ok(audio)
    }
}

/// Convertit des enregistrements mono 16 kHz en échantillons du protocole
/// (i16 little-endian en base64).
pub fn encode_samples(takes: &[Vec<f32>]) -> Vec<String> {
    takes
        .iter()
        .map(|t| {
            let pcm: Vec<i16> = t.iter().map(|s| f32_to_i16(*s)).collect();
            encode_pcm(&pcm)
        })
        .collect()
}

/// Envoie les échantillons au cerveau pour construire l'empreinte vocale ; renvoie le
/// message du cerveau, ou une erreur claire.
pub async fn send_voice(brain: &dyn Brain, takes: &[Vec<f32>]) -> anyhow::Result<String> {
    let samples = encode_samples(takes);
    match brain.request(ControlRequest::EnrollVoice { samples }).await {
        Ok(ControlResponse::Reply { speech }) => Ok(speech),
        Ok(ControlResponse::Error { message }) => {
            Err(anyhow!("le cerveau refuse l'empreinte vocale : {message}"))
        }
        Ok(other) => Err(anyhow!("réponse inattendue du cerveau : {other:?}")),
        Err(ClientError::BrainUnreachable) => Err(anyhow!(
            "cerveau injoignable : l'empreinte vocale n'a pas été enregistrée. \
             Démarre le cerveau (service jarvis-brain) puis relance `jarvis-ear enroll --voice-only`."
        )),
        Err(e) => Err(anyhow!("empreinte vocale non transmise : {e}")),
    }
}

/// Déroulé complet de l'enrôlement.
pub async fn run_with(
    session: &mut dyn Session,
    brain: &dyn Brain,
    dir: &Path,
    count: usize,
    voice_only: bool,
) -> anyhow::Result<()> {
    let mut jarvis_takes: Vec<Vec<f32>> = Vec::new();
    if !voice_only {
        if count < 3 {
            return Err(anyhow!("il faut au moins 3 prises (8 recommandées)"));
        }
        // Les prises sont des enregistrements de ma voix : dossiers en 0700.
        let samples_dir = dir.join("samples");
        create_private_dir(dir)?;
        create_private_dir(&samples_dir)?;
        let removed = clear_samples(&samples_dir)?;
        if removed > 0 {
            session.say(&format!("{removed} ancienne(s) prise(s) supprimée(s)."));
        }
        session.say(&format!(
            "Enrôlement du mot d'éveil : dis « Jarvis » à chaque prise ({count} au total)."
        ));
        session.say(&format!(
            "Les WAV sont conservés dans {} pour reconstruire le modèle.",
            samples_dir.display()
        ));
        for i in 1..=count {
            session.wait_ready(&format!(
                "Prise {i}/{count} : appuie sur Entrée puis dis « Jarvis »… "
            ))?;
            let take = session.record(TAKE_SECONDS)?;
            write_wav(&samples_dir.join(sample_file_name(i)), &take)?;
            jarvis_takes.push(take);
        }
        let output = dir.join("jarvis.rpw");
        let n = build_model(&samples_dir, &output)?;
        session.say(&format!(
            "{} créé à partir de {n} échantillons.",
            output.display()
        ));
    }

    session.say("Empreinte vocale : lis chaque phrase à voix naturelle (3 à 4 secondes).");
    session.say(
        "Ces enregistrements ne sont pas gardés sur disque : ils partent vers le cerveau local.",
    );
    let mut voice: Vec<Vec<f32>> = Vec::new();
    for (i, phrase) in PHRASES.iter().enumerate() {
        session.say(&format!(
            "Phrase {}/{} : « {phrase} »",
            i + 1,
            PHRASES.len()
        ));
        session.wait_ready("appuie sur Entrée puis lis la phrase… ")?;
        voice.push(session.record(PHRASE_SECONDS)?);
    }
    // Les prises « Jarvis » (1,5 s) complètent l'empreinte pour les phrases courtes.
    voice.extend(jarvis_takes);
    let answer = send_voice(brain, &voice).await?;
    session.say(&format!("Empreinte vocale : {answer}"));
    Ok(())
}

/// Point d'entrée de `jarvis-ear enroll`.
pub fn run(count: usize, voice_only: bool) -> anyhow::Result<()> {
    if !voice_only && count < 3 {
        return Err(anyhow!("il faut au moins 3 prises (8 recommandées)"));
    }
    let dir = wakeword_dir()?;
    let brain = SocketBrain {
        socket: jarvis_protocol::socket_path(),
    };
    // Un fil de travail : la tâche des signaux tourne pendant que le fil principal attend
    // Entrée ou le micro.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()?;
    // Le service de l'oreille entendrait les prises : arrêté pendant l'enrôlement, relancé
    // à la fin (aussi après une erreur, ou par la tâche des signaux sur Ctrl-C).
    let pause = Arc::new(EarPause::begin(Arc::new(RealSystemctl), &mut |l| {
        println!("{l}")
    })?);
    let result = runtime.block_on(async {
        // Les gestionnaires sont posés ici, avant la première prise.
        let mut interrupt = signal(SignalKind::interrupt())?;
        let mut terminate = signal(SignalKind::terminate())?;
        let mut hangup = signal(SignalKind::hangup())?;
        let on_signal = pause.clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = interrupt.recv() => {}
                _ = terminate.recv() => {}
                _ = hangup.recv() => {}
            }
            println!();
            on_signal.resume_and_report();
            std::process::exit(130);
        });
        run_with(&mut Terminal, &brain, &dir, count, voice_only).await
    });
    // Réussite ou erreur : relance tout de suite (la tâche des signaux garde une copie du
    // garde, qui ne serait détruit qu'avec le runtime). Une panique passe par `Drop`.
    pause.resume_and_report();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nom_de_fichier_des_prises() {
        assert_eq!(sample_file_name(1), "jarvis-01.wav");
        assert_eq!(sample_file_name(12), "jarvis-12.wav");
    }

    #[test]
    fn nombre_d_echantillons_par_prise() {
        assert_eq!(samples_per_take(), 24_000);
    }

    #[test]
    fn anciennes_prises_supprimees() {
        let dir = tempfile::tempdir().unwrap();
        for i in 1..=5 {
            write_wav(&dir.path().join(sample_file_name(i)), &[0.0; 16]).unwrap();
        }
        write_wav(&dir.path().join("autre.wav"), &[0.0; 16]).unwrap();
        assert_eq!(clear_samples(dir.path()).unwrap(), 5);
        let left = list_samples(dir.path()).unwrap();
        assert_eq!(left.len(), 1);
        assert!(left[0].ends_with("autre.wav"));
    }

    #[test]
    fn construction_du_modele_depuis_des_wav() {
        let dir = tempfile::tempdir().unwrap();
        let samples = dir.path().join("samples");
        std::fs::create_dir_all(&samples).unwrap();
        for i in 1..=3 {
            let f = 300.0 + 50.0 * i as f32;
            let take: Vec<f32> = (0..samples_per_take())
                .map(|n| {
                    let t = n as f32 / SAMPLE_RATE as f32;
                    (t * f * std::f32::consts::TAU).sin() * (t * 2.0).sin().abs() * 0.5
                })
                .collect();
            write_wav(&samples.join(sample_file_name(i)), &take).unwrap();
        }
        let rpw = dir.path().join("jarvis.rpw");
        assert_eq!(build_model(&samples, &rpw).unwrap(), 3);
        assert!(rpw.metadata().unwrap().len() > 0);
    }

    #[test]
    fn seules_les_prises_a_deux_chiffres_sont_supprimees() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["jarvis-01.wav", "jarvis-12.wav"] {
            write_wav(&dir.path().join(name), &[0.0; 16]).unwrap();
        }
        for name in [
            "jarvis-extra.wav",
            "jarvis-1.wav",
            "jarvis-001.wav",
            "jarvis-ab.wav",
        ] {
            write_wav(&dir.path().join(name), &[0.0; 16]).unwrap();
        }
        assert_eq!(clear_samples(dir.path()).unwrap(), 2);
        assert_eq!(list_samples(dir.path()).unwrap().len(), 4);
    }

    /// Session simulée : sinusoïdes pour les prises, durée demandée respectée.
    #[derive(Default)]
    struct FakeSession {
        said: Vec<String>,
        durations: Vec<f32>,
    }
    impl Session for FakeSession {
        fn say(&mut self, line: &str) {
            self.said.push(line.to_string());
        }
        fn wait_ready(&mut self, _: &str) -> anyhow::Result<()> {
            Ok(())
        }
        fn record(&mut self, seconds: f32) -> anyhow::Result<Vec<f32>> {
            let index = self.durations.len();
            self.durations.push(seconds);
            let f = 300.0 + 40.0 * index as f32;
            Ok((0..(SAMPLE_RATE as f32 * seconds) as usize)
                .map(|n| {
                    let t = n as f32 / SAMPLE_RATE as f32;
                    (t * f * std::f32::consts::TAU).sin() * (t * 2.0).sin().abs() * 0.5
                })
                .collect())
        }
    }

    struct FakeBrain {
        requests: std::sync::Mutex<Vec<ControlRequest>>,
        answer: fn() -> Result<ControlResponse, ClientError>,
    }
    #[async_trait::async_trait]
    impl Brain for FakeBrain {
        async fn request(&self, req: ControlRequest) -> Result<ControlResponse, ClientError> {
            self.requests.lock().unwrap().push(req);
            (self.answer)()
        }
    }
    fn brain(answer: fn() -> Result<ControlResponse, ClientError>) -> FakeBrain {
        FakeBrain {
            requests: Default::default(),
            answer,
        }
    }
    fn enrolled() -> Result<ControlResponse, ClientError> {
        Ok(ControlResponse::Reply {
            speech: "Empreinte enregistrée.".into(),
        })
    }

    fn all_files(dir: &Path) -> Vec<String> {
        fn walk(dir: &Path, out: &mut Vec<String>) {
            for e in std::fs::read_dir(dir).unwrap() {
                let p = e.unwrap().path();
                if p.is_dir() {
                    walk(&p, out);
                } else {
                    out.push(p.file_name().unwrap().to_string_lossy().into_owned());
                }
            }
        }
        let mut out = Vec::new();
        walk(dir, &mut out);
        out.sort();
        out
    }

    #[tokio::test]
    async fn echantillons_envoyes_en_i16_base64_a_16_khz_sans_fichier_de_phrase() {
        let dir = tempfile::tempdir().unwrap();
        let (mut session, brain) = (FakeSession::default(), brain(enrolled));
        run_with(&mut session, &brain, dir.path(), 3, false)
            .await
            .unwrap();

        let requests = brain.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        let ControlRequest::EnrollVoice { samples } = &requests[0] else {
            panic!("{requests:?}")
        };
        // 6 phrases de 3,5 s, puis les 3 prises « Jarvis » de 1,5 s.
        assert_eq!(samples.len(), PHRASES.len() + 3);
        for (i, s) in samples.iter().enumerate() {
            let pcm = jarvis_protocol::decode_pcm(s).unwrap();
            let seconds = if i < PHRASES.len() { 3.5 } else { 1.5 };
            assert_eq!(pcm.len(), (16_000.0 * seconds) as usize, "échantillon {i}");
            assert!(pcm.iter().any(|v| *v != 0));
        }
        assert!(
            session
                .said
                .iter()
                .any(|l| l.contains("Empreinte enregistrée"))
        );
        // Sur disque : les prises « Jarvis » et le modèle, aucune phrase.
        assert_eq!(
            all_files(dir.path()),
            vec![
                "jarvis-01.wav",
                "jarvis-02.wav",
                "jarvis-03.wav",
                "jarvis.rpw"
            ]
        );
    }

    fn mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[tokio::test]
    async fn prises_et_modele_lisibles_par_moi_seul() {
        let base = tempfile::tempdir().unwrap();
        let dir = base.path().join("wakeword");
        let (mut session, brain) = (FakeSession::default(), brain(enrolled));
        run_with(&mut session, &brain, &dir, 3, false)
            .await
            .unwrap();
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(&dir.join("samples")), 0o700);
        for i in 1..=3 {
            assert_eq!(mode(&dir.join("samples").join(sample_file_name(i))), 0o600);
        }
        assert_eq!(mode(&dir.join("jarvis.rpw")), 0o600);
    }

    #[tokio::test]
    async fn dossier_et_modele_existants_rendus_prives() {
        use std::os::unix::fs::PermissionsExt;
        let base = tempfile::tempdir().unwrap();
        let dir = base.path().join("wakeword");
        let samples = dir.join("samples");
        std::fs::create_dir_all(&samples).unwrap();
        let rpw = dir.join("jarvis.rpw");
        std::fs::write(&rpw, "ancien").unwrap();
        for p in [&dir, &samples] {
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        std::fs::set_permissions(&rpw, std::fs::Permissions::from_mode(0o644)).unwrap();
        let (mut session, brain) = (FakeSession::default(), brain(enrolled));
        run_with(&mut session, &brain, &dir, 3, false)
            .await
            .unwrap();
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(&samples), 0o700);
        assert_eq!(mode(&rpw), 0o600);
    }

    #[test]
    fn prise_reecrite_rendue_privee() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(sample_file_name(1));
        std::fs::write(&path, "ancien").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        write_wav(&path, &[0.0; 16]).unwrap();
        assert_eq!(mode(&path), 0o600);
        assert_eq!(hound::WavReader::open(&path).unwrap().len(), 16);
    }

    #[tokio::test]
    async fn voix_seule_n_envoie_que_les_six_phrases_et_n_ecrit_rien() {
        let dir = tempfile::tempdir().unwrap();
        let (mut session, brain) = (FakeSession::default(), brain(enrolled));
        run_with(&mut session, &brain, dir.path(), 8, true)
            .await
            .unwrap();
        let requests = brain.requests.lock().unwrap();
        let ControlRequest::EnrollVoice { samples } = &requests[0] else {
            panic!("{requests:?}")
        };
        assert_eq!(samples.len(), PHRASES.len());
        assert_eq!(session.durations, vec![3.5; 6]);
        assert!(all_files(dir.path()).is_empty());
    }

    #[tokio::test]
    async fn cerveau_injoignable_message_clair_modele_construit() {
        let dir = tempfile::tempdir().unwrap();
        let (mut session, brain) = (
            FakeSession::default(),
            brain(|| Err(ClientError::BrainUnreachable)),
        );
        let err = run_with(&mut session, &brain, dir.path(), 3, false)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("cerveau injoignable"), "{err}");
        assert!(err.contains("--voice-only"), "{err}");
        // Le modèle du mot d'éveil est quand même construit.
        assert!(dir.path().join("jarvis.rpw").exists());
    }
}
