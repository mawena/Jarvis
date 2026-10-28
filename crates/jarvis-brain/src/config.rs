use anyhow::Context;
use jarvis_protocol::Mode;
use serde::{Deserialize, Deserializer};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct Config {
    pub mode: Mode,
    /// Nom par lequel Jarvis s'adresse à l'utilisateur.
    pub address: String,
    pub local: LocalLlmConfig,
    pub online: OnlineLlmConfig,
    pub apps: AppsConfig,
    pub stt: SttConfig,
    pub speaker: SpeakerConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            mode: Mode::Local,
            address: "Monsieur".to_string(),
            local: LocalLlmConfig::default(),
            online: OnlineLlmConfig::default(),
            apps: AppsConfig::default(),
            stt: SttConfig::default(),
            speaker: SpeakerConfig::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct LocalLlmConfig {
    pub url: String,
    pub model: String,
    /// Durée pendant laquelle Ollama garde le modèle en mémoire après un appel.
    pub keep_alive: String,
}

impl Default for LocalLlmConfig {
    fn default() -> Self {
        Self {
            url: "http://127.0.0.1:11434".to_string(),
            model: "llama3.2:3b".to_string(),
            keep_alive: "30m".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct OnlineLlmConfig {
    pub url: String,
    pub model: String,
    pub api_key_env: String,
}

impl Default for OnlineLlmConfig {
    fn default() -> Self {
        Self {
            url: "https://api.anthropic.com".to_string(),
            model: "claude-haiku-5-5".to_string(),
            api_key_env: "ANTHROPIC_API_KEY".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AppsConfig {
    pub aliases: HashMap<String, String>,
}

impl Default for AppsConfig {
    fn default() -> Self {
        Self {
            aliases: HashMap::from_iter(
                [
                    ("vs code", "com.microsoft.VSCode"),
                    ("vscode", "com.microsoft.VSCode"),
                    ("navigateur", "firefox_firefox"),
                    ("fichiers", "org.gnome.Nautilus"),
                    ("terminal", "org.gnome.Terminal"),
                    ("calculatrice", "org.gnome.Calculator"),
                    ("parametres", "org.gnome.Settings"),
                ]
                .map(|(k, v)| (k.to_string(), v.to_string())),
            ),
        }
    }
}

impl<'de> Deserialize<'de> for AppsConfig {
    /// Les alias de l'utilisateur s'ajoutent aux alias par défaut.
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Raw {
            aliases: HashMap<String, String>,
        }
        let raw = Raw::deserialize(d)?;
        let mut apps = AppsConfig::default();
        apps.aliases.extend(raw.aliases);
        Ok(apps)
    }
}

/// Reconnaissance vocale locale (Whisper).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct SttConfig {
    /// Modèle ggml de whisper.cpp (`~/` accepté en tête de chemin).
    pub model_path: PathBuf,
    pub threads: i32,
}

impl Default for SttConfig {
    fn default() -> Self {
        let model_path = match directories::ProjectDirs::from("", "", "jarvis") {
            Some(dirs) => dirs.data_dir().join("models/ggml-small-q5_1.bin"),
            None => PathBuf::from(".local/share/jarvis/models/ggml-small-q5_1.bin"),
        };
        Self {
            model_path,
            threads: 6,
        }
    }
}

/// Empreinte vocale (vérification du locuteur).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct SpeakerConfig {
    pub enabled: bool,
    /// Modèle CAM++ (`~/` accepté en tête de chemin).
    pub model_path: PathBuf,
    /// Seuil de similarité cosinus pour une phrase d'au moins 2 s.
    pub threshold: f32,
    /// Seuil pour une phrase de 1 à 2 s.
    pub strict_threshold: f32,
    pub threads: i32,
}

impl Default for SpeakerConfig {
    fn default() -> Self {
        let model_path = match directories::ProjectDirs::from("", "", "jarvis") {
            Some(dirs) => dirs
                .data_dir()
                .join("models/3dspeaker_speech_campplus_sv_en_voxceleb_16k.onnx"),
            None => PathBuf::from(
                ".local/share/jarvis/models/3dspeaker_speech_campplus_sv_en_voxceleb_16k.onnx",
            ),
        };
        Self {
            enabled: true,
            model_path,
            threshold: 0.55,
            strict_threshold: 0.65,
            threads: 2,
        }
    }
}

/// Remplace un `~` en tête de chemin par `$HOME`.
pub fn expand_home(p: &Path) -> PathBuf {
    if let Ok(rest) = p.strip_prefix("~")
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    p.to_path_buf()
}

impl Config {
    /// Fichier absent : valeurs par défaut. TOML invalide : erreur citant le chemin.
    pub fn load(path: &Path) -> anyhow::Result<Config> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Config::default()),
            Err(e) => {
                return Err(e).with_context(|| format!("lecture de {}", path.display()));
            }
        };
        toml::from_str(&text)
            .map_err(|e| anyhow::anyhow!("configuration invalide dans {} :\n{}", path.display(), e))
    }

    /// `~/.config/jarvis/config.toml`
    pub fn default_path() -> PathBuf {
        match directories::ProjectDirs::from("", "", "jarvis") {
            Some(dirs) => dirs.config_dir().join("config.toml"),
            None => PathBuf::from(".config/jarvis/config.toml"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jarvis_protocol::Mode;
    use std::path::PathBuf;

    fn write(content: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, content).unwrap();
        (dir, path)
    }

    #[test]
    fn config_absente_donne_defauts() {
        let dir = tempfile::tempdir().unwrap();
        let c = Config::load(&dir.path().join("absent.toml")).unwrap();
        assert_eq!(c.mode, Mode::Local);
        assert_eq!(c.local.model, "llama3.2:3b");
        assert_eq!(c.local.keep_alive, "30m");
        assert_eq!(c.online.model, "claude-haiku-5-5");
        assert_eq!(c.address, "Monsieur");
    }

    #[test]
    fn config_partielle() {
        let (_d, p) = write("mode = \"online\"\n");
        let c = Config::load(&p).unwrap();
        assert_eq!(c.mode, Mode::Online);
        assert_eq!(c.local, LocalLlmConfig::default());
        assert_eq!(c.online, OnlineLlmConfig::default());
    }

    #[test]
    fn alias_utilisateur_fusionnes() {
        let (_d, p) = write("[apps.aliases]\nmusique = \"spotify\"\n");
        let c = Config::load(&p).unwrap();
        assert_eq!(c.apps.aliases.get("musique").unwrap(), "spotify");
        assert_eq!(
            c.apps.aliases.get("vs code").unwrap(),
            "com.microsoft.VSCode"
        );
        assert_eq!(c.apps.aliases.get("navigateur").unwrap(), "firefox_firefox");
        assert_eq!(
            c.apps.aliases.get("terminal").unwrap(),
            "org.gnome.Terminal"
        );
    }

    #[test]
    fn stt_config_defauts() {
        let c = Config::load(Path::new("/inexistant/config.toml")).unwrap();
        assert_eq!(c.stt, SttConfig::default());
        assert_eq!(c.stt.threads, 6);
        assert!(
            c.stt
                .model_path
                .ends_with("jarvis/models/ggml-small-q5_1.bin"),
            "{}",
            c.stt.model_path.display()
        );
        let (_d, p) = write("[stt]\nthreads = 4\n");
        let c = Config::load(&p).unwrap();
        assert_eq!(c.stt.threads, 4);
        assert_eq!(c.stt.model_path, SttConfig::default().model_path);
    }

    #[test]
    fn speaker_config_defauts() {
        let c = Config::load(Path::new("/inexistant/config.toml")).unwrap();
        assert!(c.speaker.enabled);
        assert_eq!(c.speaker.threshold, 0.55);
        assert_eq!(c.speaker.strict_threshold, 0.65);
        let (_d, p) = write("[speaker]\nenabled = false\nthreshold = 0.5\n");
        let c = Config::load(&p).unwrap();
        assert!(!c.speaker.enabled);
        assert_eq!(c.speaker.threshold, 0.5);
        assert_eq!(c.speaker.strict_threshold, 0.65);
    }

    #[test]
    fn config_invalide_cite_le_chemin() {
        let (_d, p) = write("mode = 3\n");
        let err = Config::load(&p).unwrap_err().to_string();
        assert!(err.contains(&p.display().to_string()), "{err}");
    }
}
