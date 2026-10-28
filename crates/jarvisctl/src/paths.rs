use std::path::{Path, PathBuf};

pub const SERVICE: &str = "jarvis-brain.service";
pub const BINARIES: [&str; 4] = ["jarvis-brain", "jarvis", "jarvisctl", "jarvis-ear"];
/// Bibliothèques partagées de sherpa-onnx (le binaire du cerveau les cherche dans
/// `$ORIGIN/../lib/jarvis`, soit `~/.local/lib/jarvis`).
pub const SHARED_LIBS: [&str; 3] = [
    "libsherpa-onnx-c-api.so",
    "libsherpa-onnx-cxx-api.so",
    "libonnxruntime.so",
];
pub const WHISPER_MODEL: &str = "ggml-small-q5_1.bin";
pub const SPEAKER_MODEL: &str = "3dspeaker_speech_campplus_sv_en_voxceleb_16k.onnx";
pub const DEFAULT_MODEL: &str = "llama3.2:3b";

#[derive(Debug, Clone)]
pub struct Paths {
    pub bin_dir: PathBuf,
    pub lib_dir: PathBuf,
    pub cargo_bin: PathBuf,
    pub unit_path: PathBuf,
    pub config_dir: PathBuf,
    pub config_file: PathBuf,
    pub env_file: PathBuf,
    pub install_file: PathBuf,
    pub state_dir: PathBuf,
    pub data_dir: PathBuf,
}

impl Paths {
    pub fn from_home(home: &Path) -> Self {
        let config_dir = home.join(".config/jarvis");
        Self {
            bin_dir: home.join(".local/bin"),
            lib_dir: home.join(".local/lib/jarvis"),
            cargo_bin: home.join(".cargo/bin"),
            unit_path: home.join(".config/systemd/user").join(SERVICE),
            config_file: config_dir.join("config.toml"),
            env_file: config_dir.join("env"),
            install_file: config_dir.join("install.toml"),
            config_dir,
            state_dir: home.join(".local/state/jarvis"),
            data_dir: home.join(".local/share/jarvis"),
        }
    }
}
