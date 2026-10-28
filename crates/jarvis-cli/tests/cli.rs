use std::process::Command;

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
