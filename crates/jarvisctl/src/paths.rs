use std::path::{Path, PathBuf};

pub const SERVICE: &str = "jarvis-brain.service";
pub const EAR_SERVICE: &str = "jarvis-ear.service";
/// Modèle du mot d'éveil, relatif au dossier de données : sans lui, l'unité de l'oreille
/// ne démarre pas (`ConditionPathExists`).
pub const WAKEWORD_FILE: &str = "wakeword/jarvis.rpw";
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
/// Dossier de la voix de Jarvis (un `.onnx` et `tokens.txt`).
pub const VOICE_DIR: &str = "vits-piper-fr_FR-upmc-medium";
pub const DEFAULT_MODEL: &str = "llama3.2:3b";
/// Image SearXNG épinglée : la mettre à jour = changer cette constante (et `compose.yml`).
pub const SEARXNG_IMAGE: &str = "docker.io/searxng/searxng:2026.10.28-c360868e5";
/// Adresse de SearXNG, sur la machine seulement (8080 est déjà pris).
pub const SEARXNG_ADDR: &str = "127.0.0.1:8888";
pub const SEARXNG_CONTAINER: &str = "jarvis-searxng";
pub const SEARXNG_PROJECT: &str = "jarvis-searxng";

#[derive(Debug, Clone)]
pub struct Paths {
    pub home: PathBuf,
    pub bin_dir: PathBuf,
    pub lib_dir: PathBuf,
    pub cargo_bin: PathBuf,
    pub unit_path: PathBuf,
    pub ear_unit_path: PathBuf,
    pub config_dir: PathBuf,
    pub config_file: PathBuf,
    pub env_file: PathBuf,
    pub install_file: PathBuf,
    pub state_dir: PathBuf,
    pub data_dir: PathBuf,
    /// Fichiers de SearXNG (`compose.yml`, `settings.yml`, `.env`).
    pub searx_dir: PathBuf,
}

impl Paths {
    pub fn from_home(home: &Path) -> Self {
        let config_dir = home.join(".config/jarvis");
        let units = home.join(".config/systemd/user");
        Self {
            home: home.to_path_buf(),
            bin_dir: home.join(".local/bin"),
            lib_dir: home.join(".local/lib/jarvis"),
            cargo_bin: home.join(".cargo/bin"),
            unit_path: units.join(SERVICE),
            ear_unit_path: units.join(EAR_SERVICE),
            config_file: config_dir.join("config.toml"),
            searx_dir: config_dir.join("searxng"),
            env_file: config_dir.join("env"),
            install_file: config_dir.join("install.toml"),
            config_dir,
            state_dir: home.join(".local/state/jarvis"),
            data_dir: home.join(".local/share/jarvis"),
        }
    }

    /// Le modèle du mot d'éveil existe : la condition de démarrage de l'oreille est remplie.
    pub fn wakeword_enrolled(&self) -> bool {
        self.data_dir.join(WAKEWORD_FILE).is_file()
    }
}

/// Service géré par `jarvisctl`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Service {
    /// Le cerveau (jarvis-brain.service)
    #[value(name = "cerveau", alias = "brain")]
    Brain,
    /// L'oreille du PC (jarvis-ear.service)
    #[value(name = "oreille", alias = "ear")]
    Ear,
}

impl Service {
    /// Ordre de démarrage : le cerveau, puis l'oreille.
    pub const ALL: [Service; 2] = [Service::Brain, Service::Ear];

    pub fn unit(self) -> &'static str {
        match self {
            Service::Brain => SERVICE,
            Service::Ear => EAR_SERVICE,
        }
    }

    pub fn unit_path(self, paths: &Paths) -> &Path {
        match self {
            Service::Brain => &paths.unit_path,
            Service::Ear => &paths.ear_unit_path,
        }
    }

    /// Contenu exact de l'unité.
    pub fn unit_content(self) -> String {
        match self {
            Service::Brain => crate::ops::brain_unit(),
            Service::Ear => crate::ops::ear_unit(),
        }
    }
}
