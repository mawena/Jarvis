use jarvis_protocol::{ControlRequest, ControlResponse, EarId, EarSummary, pairing_ended};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::Path;
use std::process::{Command, Output};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

#[test]
fn cerveau_absent_message_clair() {
    let dir = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_jarvis"))
        .arg("status")
        .env("XDG_RUNTIME_DIR", dir.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(
        String::from_utf8_lossy(&out.stderr).trim(),
        "Le cerveau de Jarvis n'est pas lancé (jarvis-brain)."
    );
}

#[test]
fn dossier_du_socket_ouvert_aux_autres_refuse() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("jarvis")).unwrap();
    std::fs::set_permissions(
        dir.path().join("jarvis"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    let _listener = UnixListener::bind(dir.path().join("jarvis/brain.sock")).unwrap();
    let out = jarvis(dir.path(), &["status"]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("peu sûr"), "{stderr}");
}

/// Faux cerveau sur `$XDG_RUNTIME_DIR/jarvis/brain.sock` : une connexion, une
/// requête, puis chaque réponse après son délai. Rend la requête reçue.
fn faux_cerveau(
    dir: &Path,
    replies: Vec<(Duration, ControlResponse)>,
) -> JoinHandle<ControlRequest> {
    std::fs::create_dir_all(dir.join("jarvis")).unwrap();
    // Comme le vrai cerveau : dossier fermé aux autres.
    std::fs::set_permissions(dir.join("jarvis"), std::fs::Permissions::from_mode(0o700)).unwrap();
    let listener = UnixListener::bind(dir.join("jarvis/brain.sock")).unwrap();
    std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let mut stream = stream;
        for (delay, reply) in replies {
            std::thread::sleep(delay);
            let mut json = serde_json::to_string(&reply).unwrap();
            json.push('\n');
            stream.write_all(json.as_bytes()).unwrap();
        }
        serde_json::from_str(&line).unwrap()
    })
}

fn jarvis(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_jarvis"))
        .args(args)
        .env("XDG_RUNTIME_DIR", dir)
        .output()
        .unwrap()
}

fn fenetre() -> ControlResponse {
    ControlResponse::Pairing {
        code: "482913".into(),
        uri: "jarvis://pair?v=1&id=abc&h=192.168.1.20&p=7341&fp=xyz&c=482913".into(),
        fingerprint_short: "A1B2-C3D4-E5F6-0718".into(),
        expires_in_secs: 120,
    }
}

#[test]
fn pair_affiche_code_et_attend_la_fin() {
    let dir = tempfile::tempdir().unwrap();
    let brain = faux_cerveau(
        dir.path(),
        vec![
            (Duration::ZERO, fenetre()),
            (
                Duration::from_millis(300),
                ControlResponse::PairingDone {
                    name: "Pixel 4 XL".into(),
                },
            ),
        ],
    );
    let started = Instant::now();
    let out = jarvis(dir.path(), &["pair"]);
    assert!(
        started.elapsed() >= Duration::from_millis(300),
        "n'a pas attendu"
    );
    assert_eq!(brain.join().unwrap(), ControlRequest::PairStart);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{stdout}");
    assert!(stdout.contains("Code : 482913"), "{stdout}");
    assert!(stdout.contains("█"), "code en gros et QR code : {stdout}");
    assert!(stdout.contains("jarvis://pair?v=1&id=abc&h=192.168.1.20&p=7341&fp=xyz&c=482913"));
    assert!(stdout.contains("A1B2-C3D4-E5F6-0718"));
    assert!(stdout.contains("2 min 00 s"), "{stdout}");
    assert!(
        stdout.contains("Oreille « Pixel 4 XL » appairée."),
        "{stdout}"
    );
}

#[test]
fn pair_fenetre_expiree_code_1() {
    let dir = tempfile::tempdir().unwrap();
    let brain = faux_cerveau(
        dir.path(),
        vec![
            (Duration::ZERO, fenetre()),
            (
                Duration::from_millis(50),
                ControlResponse::PairingEnded {
                    reason: pairing_ended::TIMED_OUT.into(),
                },
            ),
        ],
    );
    let out = jarvis(dir.path(), &["pair"]);
    brain.join().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&out.stderr).trim(),
        "Fenêtre d'appairage fermée : délai écoulé."
    );
}

#[test]
fn pair_raisons_du_cerveau_bien_ponctuees() {
    for (reason, shown) in [
        (
            pairing_ended::TOO_MANY_ATTEMPTS.to_string(),
            "Fenêtre d'appairage fermée : trop d'essais.",
        ),
        (
            pairing_ended::REPLACED.to_string(),
            "Fenêtre d'appairage fermée : une autre fenêtre d'appairage a été ouverte.",
        ),
        (
            pairing_ended::BRAIN_STOPPING.to_string(),
            "Fenêtre d'appairage fermée : le cerveau s'arrête.",
        ),
        (
            pairing_ended::not_saved("disque plein"),
            "Fenêtre d'appairage fermée : enregistrement impossible (disque plein).",
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let brain = faux_cerveau(
            dir.path(),
            vec![
                (Duration::ZERO, fenetre()),
                (
                    Duration::from_millis(50),
                    ControlResponse::PairingEnded { reason },
                ),
            ],
        );
        let out = jarvis(dir.path(), &["pair"]);
        brain.join().unwrap();
        assert_eq!(out.status.code(), Some(1));
        assert_eq!(String::from_utf8_lossy(&out.stderr).trim(), shown);
    }
}

#[test]
fn pair_connexion_coupee_code_1() {
    let dir = tempfile::tempdir().unwrap();
    let brain = faux_cerveau(dir.path(), vec![(Duration::ZERO, fenetre())]);
    let out = jarvis(dir.path(), &["pair"]);
    brain.join().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("connexion"));
}

#[test]
fn ears_liste_les_oreilles() {
    let dir = tempfile::tempdir().unwrap();
    let ear = |id: &str, name: &str, platform: &str, connected, local| EarSummary {
        id: EarId::new(id),
        name: name.into(),
        platform: platform.into(),
        connected,
        local,
    };
    let brain = faux_cerveau(
        dir.path(),
        vec![(
            Duration::ZERO,
            ControlResponse::Ears {
                ears: vec![
                    ear("a1", "PC", "linux", true, true),
                    ear("b2", "Pixel 4 XL", "android", false, false),
                ],
            },
        )],
    );
    let out = jarvis(dir.path(), &["ears"]);
    assert_eq!(brain.join().unwrap(), ControlRequest::ListEars);
    assert_eq!(out.status.code(), Some(0));
    let lines: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.trim_end().to_string())
        .collect();
    assert_eq!(
        lines,
        [
            "Nom         Plateforme  Connectée  Locale",
            "PC          linux       oui        oui",
            "Pixel 4 XL  android     non        non",
        ]
    );
}

#[test]
fn ears_aucune_oreille() {
    let dir = tempfile::tempdir().unwrap();
    let brain = faux_cerveau(
        dir.path(),
        vec![(Duration::ZERO, ControlResponse::Ears { ears: vec![] })],
    );
    let out = jarvis(dir.path(), &["ears"]);
    brain.join().unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "Aucune oreille appairée."
    );
}

#[test]
fn unpair_oreille_inconnue_code_1() {
    let dir = tempfile::tempdir().unwrap();
    let brain = faux_cerveau(
        dir.path(),
        vec![(
            Duration::ZERO,
            ControlResponse::Error {
                message: "Aucune oreille « Pixel 4 XL ».".into(),
            },
        )],
    );
    let out = jarvis(dir.path(), &["unpair", "Pixel", "4", "XL"]);
    assert_eq!(
        brain.join().unwrap(),
        ControlRequest::Unpair {
            ear: "Pixel 4 XL".into()
        }
    );
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&out.stderr).trim(),
        "Aucune oreille « Pixel 4 XL »."
    );
}

#[test]
fn pair_cerveau_absent_code_2() {
    let dir = tempfile::tempdir().unwrap();
    let out = jarvis(dir.path(), &["pair"]);
    assert_eq!(out.status.code(), Some(2));
}
