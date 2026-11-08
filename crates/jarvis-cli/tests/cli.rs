use jarvis_protocol::{
    ControlRequest, ControlResponse, EarId, EarSummary, PersonSummary, RoleSummary, pairing_ended,
};
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
    assert_eq!(
        brain.join().unwrap(),
        ControlRequest::PairStart { person: None }
    );
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
        person: None,
    };
    let brain = faux_cerveau(
        dir.path(),
        vec![(
            Duration::ZERO,
            ControlResponse::Ears {
                ears: vec![
                    ear("a1b2c3d4e5f60718", "PC", "linux", true, true),
                    ear("0f1e2d3c4b5a6978", "Pixel 4 XL", "android", false, false),
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
            "Id        Nom         Plateforme  Connectée  Locale  Personne",
            "a1b2c3d4  PC          linux       oui        oui     -",
            "0f1e2d3c  Pixel 4 XL  android     non        non     -",
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

/// Lance `jarvis <args>` contre un faux cerveau qui répond une fois ; rend la requête et la sortie.
fn essai(args: &[&str], reply: ControlResponse) -> (ControlRequest, Output) {
    let dir = tempfile::tempdir().unwrap();
    let brain = faux_cerveau(dir.path(), vec![(Duration::ZERO, reply)]);
    let out = jarvis(dir.path(), args);
    (brain.join().unwrap(), out)
}

fn ok() -> ControlResponse {
    ControlResponse::Reply {
        speech: "Fait.".into(),
    }
}

fn lignes(out: &Output) -> Vec<String> {
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.trim_end().to_string())
        .collect()
}

#[test]
fn person_list_affiche_le_tableau() {
    let personne = |name: &str, role: &str, owner, ears: &[&str], voiceprint| PersonSummary {
        name: name.into(),
        role: role.into(),
        owner,
        ears: ears.iter().map(|e| e.to_string()).collect(),
        voiceprint,
    };
    let (req, out) = essai(
        &["person", "list"],
        ControlResponse::People {
            people: vec![
                personne("Charles", "admin", true, &["PC", "Pixel 4 XL"], true),
                personne("Léa", "famille", false, &[], false),
            ],
            degraded: None,
        },
    );
    assert_eq!(req, ControlRequest::PersonList);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        lignes(&out),
        [
            "Nom                    Rôle     Oreilles         Empreinte vocale",
            "Charles (propriétaire)  admin    PC, Pixel 4 XL   oui",
            "Léa                    famille  -                non",
        ]
    );
}

#[test]
fn person_add_envoie_la_requete() {
    let (req, out) = essai(
        &["person", "add", "Marie", "Claire", "--role", "famille"],
        ok(),
    );
    assert_eq!(
        req,
        ControlRequest::PersonAdd {
            name: "Marie Claire".into(),
            role: "famille".into()
        }
    );
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(lignes(&out), ["Fait."]);
}

#[test]
fn person_remove_et_role_envoient_les_requetes() {
    let (req, _) = essai(&["person", "remove", "Marie", "Claire"], ok());
    assert_eq!(
        req,
        ControlRequest::PersonRemove {
            name: "Marie Claire".into()
        }
    );
    let (req, _) = essai(&["person", "role", "Marie", "Claire", "admin"], ok());
    assert_eq!(
        req,
        ControlRequest::PersonRole {
            name: "Marie Claire".into(),
            role: "admin".into()
        }
    );
}

#[test]
fn person_attach_nom_a_plusieurs_mots() {
    let (req, out) = essai(
        &["person", "attach", "Marie Claire", "Pixel", "4", "XL"],
        ok(),
    );
    assert_eq!(
        req,
        ControlRequest::PersonAttach {
            name: "Marie Claire".into(),
            ear: "Pixel 4 XL".into()
        }
    );
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn role_add_decoupe_les_outils_par_virgule() {
    let (req, _) = essai(
        &[
            "role",
            "add",
            "famille",
            "--tools",
            "media_*, app_launch,web_search",
            "--agent",
        ],
        ok(),
    );
    assert_eq!(
        req,
        ControlRequest::RoleAdd {
            name: "famille".into(),
            tools: vec!["media_*".into(), "app_launch".into(), "web_search".into()],
            confirm: false,
            agent: true,
        }
    );
}

#[test]
fn role_edit_sans_option_refuse_par_clap() {
    let dir = tempfile::tempdir().unwrap();
    let out = jarvis(dir.path(), &["role", "edit", "famille"]);
    assert_eq!(out.status.code(), Some(2));
    // Refusé avant toute connexion : pas de « cerveau absent ».
    assert!(!String::from_utf8_lossy(&out.stderr).contains("pas lancé"));
}

#[test]
fn role_edit_confirm_et_no_confirm_exclusifs() {
    let dir = tempfile::tempdir().unwrap();
    let out = jarvis(
        dir.path(),
        &["role", "edit", "famille", "--confirm", "--no-confirm"],
    );
    assert_eq!(out.status.code(), Some(2));
    let (req, _) = essai(
        &[
            "role",
            "edit",
            "famille",
            "--no-confirm",
            "--tools",
            "media_*",
        ],
        ok(),
    );
    assert_eq!(
        req,
        ControlRequest::RoleEdit {
            name: "famille".into(),
            tools: Some(vec!["media_*".into()]),
            confirm: Some(false),
            agent: None,
        }
    );
}

#[test]
fn role_list_affiche_le_tableau() {
    let (req, out) = essai(
        &["role", "list"],
        ControlResponse::Roles {
            roles: vec![
                RoleSummary {
                    name: "admin".into(),
                    tools: vec![],
                    confirm: false,
                    agent: true,
                    builtin: true,
                    people: 1,
                },
                RoleSummary {
                    name: "famille".into(),
                    tools: vec!["media_*".into(), "app_launch".into()],
                    confirm: true,
                    agent: false,
                    builtin: false,
                    people: 2,
                },
            ],
        },
    );
    assert_eq!(req, ControlRequest::RoleList);
    assert_eq!(
        lignes(&out),
        [
            "Nom      Outils                Confirmer  Agent  Personnes",
            "admin    tout                  non        oui    1",
            "famille  media_*, app_launch   oui        non    2",
        ]
    );
}

#[test]
fn erreur_du_cerveau_code_1() {
    let (_, out) = essai(
        &["person", "remove", "Charles"],
        ControlResponse::Error {
            message: "Charles est le propriétaire.".into(),
        },
    );
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&out.stderr).trim(),
        "Charles est le propriétaire."
    );
}

#[test]
fn pair_person_envoie_le_nom() {
    let dir = tempfile::tempdir().unwrap();
    let brain = faux_cerveau(
        dir.path(),
        vec![(
            Duration::ZERO,
            ControlResponse::PairingEnded {
                reason: pairing_ended::CANCELLED.into(),
            },
        )],
    );
    jarvis(dir.path(), &["pair", "--person", "Marie", "Claire"]);
    assert_eq!(
        brain.join().unwrap(),
        ControlRequest::PairStart {
            person: Some("Marie Claire".into())
        }
    );
}

#[test]
fn ears_montre_la_colonne_personne() {
    let (_, out) = essai(
        &["ears"],
        ControlResponse::Ears {
            ears: vec![EarSummary {
                id: EarId::new("a1b2c3d4e5f60718"),
                name: "PC".into(),
                platform: "linux".into(),
                connected: true,
                local: true,
                person: Some("Charles".into()),
            }],
        },
    );
    assert_eq!(
        lignes(&out),
        [
            "Id        Nom  Plateforme  Connectée  Locale  Personne",
            "a1b2c3d4  PC   linux       oui        oui     Charles",
        ]
    );
}
