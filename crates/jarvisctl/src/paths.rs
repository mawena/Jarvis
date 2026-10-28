use std::path::{Path, PathBuf};

pub const SERVICE: &str = "jarvis-brain.service";
pub const BINARIES: [&str; 3] = ["jarvis-brain", "jarvis", "jarvisctl"];
pub const DEFAULT_MODEL: &str = "llama3.2:3b";

#[derive(Debug, Clone)]
pub struct Paths {
    pub bin_dir: PathBuf,
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
