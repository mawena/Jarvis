//! Empreinte vocale : vérifie que la phrase entendue est bien celle de Charles.
//! Seule l'empreinte (un vecteur) est conservée, jamais le son.

pub mod sherpa;

use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use tracing::{info, warn};

const SAMPLE_RATE: usize = 16_000;
/// Au-delà : seuil normal. Entre 1 s et 2 s : seuil strict. En dessous : trop court.
const NORMAL_MIN: usize = 2 * SAMPLE_RATE;
const STRICT_MIN: usize = SAMPLE_RATE;
/// Durée pendant laquelle une phrase très courte reste acceptée après une voix reconnue.
const SESSION_WINDOW: Duration = Duration::from_secs(180);

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Verdict {
    /// Voix reconnue (similarité cosinus). Une phrase très courte acceptée grâce à la
    /// session renvoie `Match(1.0)` sans renouveler la fenêtre.
    Match(f32),
    /// Voix inconnue : la phrase doit être ignorée.
    NoMatch(f32),
    /// Moins d'une seconde, hors session : refusée.
    TooShort,
    /// Aucune empreinte enregistrée : la phrase est acceptée.
    NotEnrolled,
}

pub trait SpeakerVerifier: Send + Sync {
    /// Empreinte de l'audio (f32, 16 kHz mono).
    fn embed(&self, audio: &[f32]) -> anyhow::Result<Vec<f32>>;
}

/// Empreinte enregistrée (JSON, 0600).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VoicePrint {
    pub embedding: Vec<f32>,
    pub samples: usize,
    /// Secondes depuis l'epoch Unix.
    pub created: String,
}

pub struct SpeakerGate {
    verifier: Arc<dyn SpeakerVerifier>,
    print: RwLock<Option<VoicePrint>>,
    threshold: f32,
    strict_threshold: f32,
    session: Mutex<Option<Instant>>,
    path: PathBuf,
    warned: AtomicBool,
}

fn normalize(v: &mut [f32]) {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 0.0 {
        v.iter_mut().for_each(|x| *x /= n);
    }
}

/// Vecteur exploitable : non vide, fini, de norme non nulle.
fn is_usable(v: &[f32]) -> bool {
    !v.is_empty() && v.iter().all(|x| x.is_finite()) && v.iter().any(|&x| x != 0.0)
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na * nb)
    }
}

impl SpeakerGate {
    /// Charge l'empreinte existante dans `path` (absente ou illisible : non enregistré).
    pub fn new(
        verifier: Arc<dyn SpeakerVerifier>,
        path: PathBuf,
        threshold: f32,
        strict_threshold: f32,
    ) -> Self {
        let print = match std::fs::read_to_string(&path) {
            Ok(t) => match serde_json::from_str::<VoicePrint>(&t) {
                Ok(p) if is_usable(&p.embedding) => Some(p),
                Ok(_) => {
                    warn!(
                        chemin = %path.display(),
                        "empreinte vocale invalide (vide, non finie ou nulle), ignorée"
                    );
                    None
                }
                Err(e) => {
                    warn!(chemin = %path.display(), erreur = %e, "empreinte vocale illisible, ignorée");
                    None
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                warn!(chemin = %path.display(), erreur = %e, "empreinte vocale illisible, ignorée");
                None
            }
        };
        Self {
            verifier,
            print: RwLock::new(print),
            threshold,
            strict_threshold,
            session: Mutex::new(None),
            path,
            warned: AtomicBool::new(false),
        }
    }

    pub fn default_path() -> PathBuf {
        match directories::ProjectDirs::from("", "", "jarvis") {
            Some(dirs) => dirs.data_dir().join("voiceprint/charles.json"),
            None => PathBuf::from(".local/share/jarvis/voiceprint/charles.json"),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn is_enrolled(&self) -> bool {
        self.print.read().unwrap().is_some()
    }

    /// Enregistre l'empreinte : moyenne des empreintes normalisées, renormalisée.
    /// Renvoie le nombre d'échantillons utilisés.
    pub fn enroll(&self, samples: &[Vec<f32>]) -> anyhow::Result<usize> {
        anyhow::ensure!(!samples.is_empty(), "aucun échantillon");
        let mut mean: Vec<f32> = Vec::new();
        for audio in samples {
            let mut e = self.verifier.embed(audio)?;
            anyhow::ensure!(
                is_usable(&e),
                "empreinte invalide (valeurs non finies ou nulle)"
            );
            normalize(&mut e);
            if mean.is_empty() {
                mean = vec![0.0; e.len()];
            }
            anyhow::ensure!(e.len() == mean.len(), "dimensions d'empreinte incohérentes");
            mean.iter_mut().zip(&e).for_each(|(m, x)| *m += x);
        }
        normalize(&mut mean);
        anyhow::ensure!(
            is_usable(&mean),
            "empreinte invalide (valeurs non finies ou norme nulle)"
        );
        let print = VoicePrint {
            embedding: mean,
            samples: samples.len(),
            created: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs().to_string())
                .unwrap_or_default(),
        };
        self.save(&print)?;
        *self.print.write().unwrap() = Some(print);
        *self.session.lock().unwrap() = None;
        Ok(samples.len())
    }

    fn save(&self, print: &VoicePrint) -> anyhow::Result<()> {
        anyhow::ensure!(
            is_usable(&print.embedding),
            "empreinte invalide, non enregistrée"
        );
        let parent = self
            .path
            .parent()
            .context("chemin d'empreinte sans dossier")?;
        std::fs::create_dir_all(parent)
            .with_context(|| format!("création de {}", parent.display()))?;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
            .with_context(|| format!("permissions de {}", parent.display()))?;
        let json = serde_json::to_string(print)?;
        // Écriture atomique : fichier temporaire 0600 dans le même dossier, puis `rename`.
        // Nom unique par écriture (processus + compteur) : deux enregistrements simultanés
        // ne peuvent pas se renommer mutuellement leur fichier temporaire.
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let tmp = parent.join(format!(".charles.json.{}.{n}.tmp", std::process::id()));
        let _ = std::fs::remove_file(&tmp);
        let written = (|| -> std::io::Result<()> {
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&tmp)?;
            f.write_all(json.as_bytes())?;
            f.sync_all()?;
            std::fs::rename(&tmp, &self.path)
        })();
        if let Err(e) = written {
            let _ = std::fs::remove_file(&tmp);
            return Err(e).with_context(|| format!("écriture de {}", self.path.display()));
        }
        Ok(())
    }

    pub fn check(&self, audio: &[f32]) -> Verdict {
        // Copie de l'empreinte : le verrou n'est pas tenu pendant l'inférence.
        let reference = self
            .print
            .read()
            .unwrap()
            .as_ref()
            .map(|p| p.embedding.clone());
        let Some(reference) = reference else {
            if !self.warned.swap(true, Ordering::Relaxed) {
                warn!("aucune empreinte vocale enregistrée : toutes les voix sont acceptées");
            }
            return Verdict::NotEnrolled;
        };
        if audio.len() < STRICT_MIN {
            let in_session = self
                .session
                .lock()
                .unwrap()
                .is_some_and(|t| t.elapsed() <= SESSION_WINDOW);
            return if in_session {
                Verdict::Match(1.0)
            } else {
                Verdict::TooShort
            };
        }
        let threshold = if audio.len() >= NORMAL_MIN {
            self.threshold
        } else {
            self.strict_threshold
        };
        let score = match self.verifier.embed(audio) {
            Ok(e) => cosine(&e, &reference),
            Err(e) => {
                warn!(erreur = %e, "empreinte de la phrase impossible");
                return Verdict::NoMatch(0.0);
            }
        };
        if score >= threshold {
            *self.session.lock().unwrap() = Some(Instant::now());
            info!(score = format!("{score:.2}"), "voix reconnue");
            Verdict::Match(score)
        } else {
            Verdict::NoMatch(score)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FakeVerifier;

    /// Audio factice dont le premier échantillon désigne l'empreinte de `FakeVerifier`.
    fn audio(id: usize, secs: f32) -> Vec<f32> {
        let mut a = vec![0.0; (secs * SAMPLE_RATE as f32) as usize];
        a[0] = id as f32;
        a
    }

    fn gate(dir: &Path, vectors: Vec<Vec<f32>>) -> SpeakerGate {
        SpeakerGate::new(
            Arc::new(FakeVerifier::new(vectors)),
            dir.join("voiceprint/charles.json"),
            0.55,
            0.65,
        )
    }

    /// 0 = Charles ; 1 = proche (cos 0.6) ; 2 = loin (cos 0.2) ; 3 = autre Charles.
    fn vectors() -> Vec<Vec<f32>> {
        vec![
            vec![1.0, 0.0],
            vec![0.6, 0.8],
            vec![0.2, 0.98],
            vec![0.9, 0.1],
        ]
    }

    fn enrolled(dir: &Path) -> SpeakerGate {
        let g = gate(dir, vectors());
        g.enroll(&[audio(0, 2.0)]).unwrap();
        g
    }

    #[test]
    fn enrolement_moyenne_normalisee() {
        let d = tempfile::tempdir().unwrap();
        let g = gate(d.path(), vec![vec![3.0, 0.0], vec![0.0, 5.0]]);
        assert_eq!(g.enroll(&[audio(0, 2.0), audio(1, 2.0)]).unwrap(), 2);
        let p = g.print.read().unwrap().clone().unwrap();
        let expected = std::f32::consts::FRAC_1_SQRT_2;
        assert!((p.embedding[0] - expected).abs() < 1e-5);
        assert!((p.embedding[1] - expected).abs() < 1e-5);
        assert_eq!(p.samples, 2);
        assert!(g.enroll(&[]).is_err());
    }

    #[test]
    fn empreinte_fichier_0600_sans_son() {
        let d = tempfile::tempdir().unwrap();
        let g = enrolled(d.path());
        let mode = std::fs::metadata(g.path()).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(g.path()).unwrap()).unwrap();
        let mut keys: Vec<_> = json.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(keys, ["created", "embedding", "samples"]);
        // Rechargée au redémarrage.
        let again = gate(d.path(), vectors());
        assert!(again.is_enrolled());
    }

    #[test]
    fn empreinte_non_finie_ou_nulle_refusee() {
        let d = tempfile::tempdir().unwrap();
        let g = gate(
            d.path(),
            vec![
                vec![f32::NAN, 1.0],
                vec![0.0, 0.0],
                vec![f32::INFINITY, 0.0],
            ],
        );
        for id in 0..3 {
            assert!(g.enroll(&[audio(id, 2.0)]).is_err(), "id {id}");
        }
        assert!(!g.is_enrolled());
        assert!(!g.path().exists());
        let bad = VoicePrint {
            embedding: vec![f32::NAN],
            samples: 1,
            created: String::new(),
        };
        assert!(g.save(&bad).is_err());
    }

    #[test]
    fn fichier_invalide_ignore_au_chargement() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("voiceprint/charles.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        for contenu in [
            "pas du json",
            r#"{"embedding":[null,1.0],"samples":1,"created":"0"}"#,
            r#"{"embedding":[],"samples":1,"created":"0"}"#,
            r#"{"embedding":[0.0,0.0],"samples":1,"created":"0"}"#,
        ] {
            std::fs::write(&path, contenu).unwrap();
            let g = gate(d.path(), vectors());
            assert!(!g.is_enrolled(), "{contenu}");
            assert_eq!(g.check(&audio(2, 3.0)), Verdict::NotEnrolled);
        }
    }

    #[test]
    fn ecriture_atomique_dossier_0700() {
        let d = tempfile::tempdir().unwrap();
        let g = enrolled(d.path());
        let dir = g.path().parent().unwrap();
        let mode = std::fs::metadata(dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
        // Aucun fichier temporaire ne reste, et un réenrôlement remplace le fichier.
        g.enroll(&[audio(3, 2.0)]).unwrap();
        let names: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(names, ["charles.json"]);
        let mode = std::fs::metadata(g.path()).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn voix_reconnue() {
        let d = tempfile::tempdir().unwrap();
        let g = enrolled(d.path());
        assert!(matches!(g.check(&audio(3, 3.0)), Verdict::Match(s) if s > 0.9));
    }

    #[test]
    fn voix_inconnue_rejetee() {
        let d = tempfile::tempdir().unwrap();
        let g = enrolled(d.path());
        assert!(matches!(g.check(&audio(2, 3.0)), Verdict::NoMatch(s) if s < 0.55));
    }

    #[test]
    fn phrase_courte_acceptee_dans_la_session() {
        let d = tempfile::tempdir().unwrap();
        let g = enrolled(d.path());
        assert!(matches!(g.check(&audio(0, 3.0)), Verdict::Match(_)));
        assert!(matches!(g.check(&audio(2, 0.5)), Verdict::Match(_)));
    }

    #[test]
    fn phrase_courte_rejetee_hors_session() {
        let d = tempfile::tempdir().unwrap();
        let g = enrolled(d.path());
        assert_eq!(g.check(&audio(0, 0.5)), Verdict::TooShort);
        // Session expirée.
        *g.session.lock().unwrap() = Some(Instant::now() - SESSION_WINDOW - Duration::from_secs(1));
        assert_eq!(g.check(&audio(0, 0.5)), Verdict::TooShort);
    }

    #[test]
    fn seuil_strict_entre_une_et_deux_secondes() {
        let d = tempfile::tempdir().unwrap();
        let g = enrolled(d.path());
        // cos 0.6 : passe le seuil normal (0.55) mais pas le strict (0.65).
        assert!(matches!(g.check(&audio(1, 3.0)), Verdict::Match(_)));
        assert!(matches!(g.check(&audio(1, 1.5)), Verdict::NoMatch(_)));
        assert!(matches!(g.check(&audio(3, 1.5)), Verdict::Match(_)));
    }

    #[test]
    fn sans_empreinte_acceptee() {
        let d = tempfile::tempdir().unwrap();
        let g = gate(d.path(), vectors());
        assert_eq!(g.check(&audio(2, 3.0)), Verdict::NotEnrolled);
        assert_eq!(g.check(&audio(2, 0.2)), Verdict::NotEnrolled);
    }

    #[test]
    #[ignore = "nécessite le modèle CAM++ (download-models.sh speaker)"]
    fn sherpa_embedding_reel() {
        let cfg = crate::config::SpeakerConfig::default();
        let v = sherpa::SherpaVerifier::load(&cfg).expect("modèle (download-models.sh speaker)");
        // Bruit pseudo-aléatoire de 3 s : on ne mesure que la latence.
        let mut x = 12345u32;
        let noise: Vec<f32> = (0..3 * SAMPLE_RATE)
            .map(|_| {
                x = x.wrapping_mul(1664525).wrapping_add(1013904223);
                ((x >> 16) as f32 / 32768.0 - 1.0) * 0.1
            })
            .collect();
        let t = Instant::now();
        let e = v.embed(&noise).unwrap();
        println!("embedding {} dims en {:?}", e.len(), t.elapsed());
        // whisper-rs lié dans le même binaire : on touche aussi à son API.
        println!("whisper : {}", whisper_rs::print_system_info());
        assert!(!e.is_empty());
    }
}
