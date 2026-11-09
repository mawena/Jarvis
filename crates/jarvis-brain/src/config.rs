use anyhow::Context;
use jarvis_protocol::Mode;
use jarvis_skills::agent::AgentSettings;
use jarvis_skills::search::SearchSettings;
use serde::{Deserialize, Deserializer};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct Config {
    pub mode: Mode,
    /// Nom par lequel Jarvis s'adresse à l'utilisateur.
    pub address: String,
    /// Une action réussie par un raccourci est confirmée par un bip, sans phrase.
    pub sound_only_for_success: bool,
    pub local: LocalLlmConfig,
    pub online: OnlineLlmConfig,
    pub apps: AppsConfig,
    pub stt: SttConfig,
    pub speaker: SpeakerConfig,
    pub tts: TtsConfig,
    /// Recherche sur internet (section `[search]`).
    pub search: SearchSettings,
    /// Agent de code (section `[agent]`).
    pub agent: AgentSettings,
    /// Oreilles en réseau (section `[network]`).
    pub network: NetworkConfig,
    /// Personnes et rôles (section `[people]`).
    pub people: PeopleConfig,
}

/// Annuaire des personnes : réglages à la création du fichier.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct PeopleConfig {
    /// Nom du propriétaire (rôle `admin`), utilisé seulement à la création de
    /// `people.toml`.
    pub owner: String,
}

impl Default for PeopleConfig {
    fn default() -> Self {
        Self {
            owner: "Charles".to_string(),
        }
    }
}

/// Serveur des oreilles (WebSocket sur TLS) et annonce mDNS.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct NetworkConfig {
    pub enabled: bool,
    /// Adresse d'écoute `ip:port` (toutes les interfaces par défaut).
    pub listen: String,
    /// Nom annoncé ; vide : nom de la machine.
    pub name: String,
    /// Fenêtre de regroupement des réveils, en millisecondes.
    pub arbiter_window_ms: u64,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            listen: format!("0.0.0.0:{}", jarvis_protocol::ear::DEFAULT_PORT),
            name: String::new(),
            arbiter_window_ms: 300,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            mode: Mode::Local,
            address: "Monsieur".to_string(),
            sound_only_for_success: false,
            local: LocalLlmConfig::default(),
            online: OnlineLlmConfig::default(),
            apps: AppsConfig::default(),
            stt: SttConfig::default(),
            speaker: SpeakerConfig::default(),
            tts: TtsConfig::default(),
            search: SearchSettings::default(),
            agent: AgentSettings::default(),
            network: NetworkConfig::default(),
            people: PeopleConfig::default(),
        }
    }
}

/// Serveur qui fait tourner le modèle local.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LocalProvider {
    /// LM Studio (API compatible OpenAI, port 1234) : le serveur par défaut.
    LmStudio,
    /// Ollama (port 11434) : conservé en option.
    Ollama,
}

impl LocalProvider {
    pub fn default_url(self) -> &'static str {
        match self {
            Self::LmStudio => "http://127.0.0.1:1234",
            Self::Ollama => "http://127.0.0.1:11434",
        }
    }

    pub fn default_model(self) -> &'static str {
        match self {
            Self::LmStudio => DEFAULT_LMSTUDIO_MODEL,
            Self::Ollama => "llama3.2:3b",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::LmStudio => "LM Studio",
            Self::Ollama => "Ollama",
        }
    }
}

/// Identifiant du modèle local par défaut dans LM Studio (Qwen3.5 4B).
pub const DEFAULT_LMSTUDIO_MODEL: &str = "qwen3.5-4b";

/// Section `[local]`. `url` et `model` absents prennent la valeur par défaut du `provider`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(from = "RawLocalConfig")]
pub struct LocalLlmConfig {
    pub provider: LocalProvider,
    pub url: String,
    pub model: String,
    /// Durée pendant laquelle le serveur garde le modèle en mémoire après un appel
    /// (`keep_alive` d'Ollama, `ttl` de LM Studio) : « 30m », « 1h », « 90s ».
    pub keep_alive: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawLocalConfig {
    provider: Option<LocalProvider>,
    url: Option<String>,
    model: Option<String>,
    keep_alive: Option<String>,
}

impl From<RawLocalConfig> for LocalLlmConfig {
    fn from(raw: RawLocalConfig) -> Self {
        let provider = raw.provider.unwrap_or(LocalProvider::LmStudio);
        Self {
            provider,
            url: raw
                .url
                .unwrap_or_else(|| provider.default_url().to_string()),
            model: raw
                .model
                .unwrap_or_else(|| provider.default_model().to_string()),
            keep_alive: raw.keep_alive.unwrap_or_else(|| "30m".to_string()),
        }
    }
}

impl Default for LocalLlmConfig {
    fn default() -> Self {
        RawLocalConfig::default().into()
    }
}

/// « 30m », « 1h », « 90s » ou un nombre de secondes → secondes. None si illisible ou négatif
/// (Ollama accepte -1 pour « jamais »).
pub fn keep_alive_secs(text: &str) -> Option<u64> {
    let t = text.trim();
    let (digits, factor) = match t.chars().last()? {
        'h' => (&t[..t.len() - 1], 3600),
        'm' => (&t[..t.len() - 1], 60),
        's' => (&t[..t.len() - 1], 1),
        _ => (t, 1),
    };
    digits.trim().parse::<u64>().ok().map(|n| n * factor)
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
    /// Mots difficiles donnés à Whisper comme indice (noms d'applications, de jeux…),
    /// écrits comme ils doivent sortir : `["GiGit", "Celluloid"]`. Garder la liste courte.
    pub vocabulary: Vec<String>,
    /// Fenêtre minimale de l'encodeur, en trames (50 par seconde, 1 500 au plus). Plus
    /// grande : reconnaissance plus sûre mais plus lente (512 ≈ 1,9 s, 1 500 ≈ 7 s).
    pub min_audio_ctx: i32,
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
            vocabulary: Vec::new(),
            min_audio_ctx: 512,
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

/// Voix de Jarvis (Piper, via sherpa-onnx).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct TtsConfig {
    pub enabled: bool,
    /// Dossier de la voix : un `.onnx` et `tokens.txt` (`~/` accepté en tête de chemin).
    pub model_dir: PathBuf,
    /// Locuteur de la voix (1 = « pierre »).
    pub speaker: i32,
    /// 1.0 = vitesse normale ; plus grand = plus rapide.
    pub speed: f32,
    pub threads: i32,
}

impl Default for TtsConfig {
    fn default() -> Self {
        let model_dir = match directories::ProjectDirs::from("", "", "jarvis") {
            Some(dirs) => dirs.data_dir().join("models/vits-piper-fr_FR-upmc-medium"),
            None => PathBuf::from(".local/share/jarvis/models/vits-piper-fr_FR-upmc-medium"),
        };
        Self {
            enabled: true,
            model_dir,
            speaker: 1,
            speed: 1.0,
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
    use crate::persona::Persona;
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
        assert_eq!(c.local.provider, LocalProvider::LmStudio);
        assert_eq!(c.local.url, "http://127.0.0.1:1234");
        assert_eq!(c.local.model, "qwen3.5-4b");
        assert_eq!(c.local.keep_alive, "30m");
        assert_eq!(c.online.model, "claude-haiku-5-5");
        assert_eq!(c.address, "Monsieur");
    }

    #[test]
    fn provider_ollama_reprend_ses_valeurs() {
        let (_d, p) = write("[local]\nprovider = \"ollama\"\n");
        let c = Config::load(&p).unwrap();
        assert_eq!(c.local.provider, LocalProvider::Ollama);
        assert_eq!(c.local.url, "http://127.0.0.1:11434");
        assert_eq!(c.local.model, "llama3.2:3b");
    }

    #[test]
    fn local_url_et_modele_explicites_gagnent() {
        let (_d, p) = write("[local]\nurl = \"http://x:1\"\nmodel = \"autre\"\n");
        let c = Config::load(&p).unwrap();
        assert_eq!(c.local.provider, LocalProvider::LmStudio);
        assert_eq!(c.local.url, "http://x:1");
        assert_eq!(c.local.model, "autre");
    }

    #[test]
    fn provider_inconnu_refuse() {
        let (_d, p) = write("[local]\nprovider = \"truc\"\n");
        assert!(Config::load(&p).is_err());
    }

    #[test]
    fn duree_de_garde_en_secondes() {
        assert_eq!(keep_alive_secs("30m"), Some(1800));
        assert_eq!(keep_alive_secs("1h"), Some(3600));
        assert_eq!(keep_alive_secs("90s"), Some(90));
        assert_eq!(keep_alive_secs("45"), Some(45));
        assert_eq!(keep_alive_secs("-1"), None);
        assert_eq!(keep_alive_secs("bof"), None);
        assert_eq!(keep_alive_secs(""), None);
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
        assert!(c.stt.vocabulary.is_empty());
        assert_eq!(c.stt.min_audio_ctx, 512);
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
    fn tts_config_defauts() {
        let c = Config::load(Path::new("/inexistant/config.toml")).unwrap();
        assert!(c.tts.enabled);
        assert_eq!(c.tts.speaker, 1);
        assert_eq!(c.tts.speed, 1.0);
        assert_eq!(c.tts.threads, 2);
        assert!(
            c.tts
                .model_dir
                .ends_with("jarvis/models/vits-piper-fr_FR-upmc-medium"),
            "{}",
            c.tts.model_dir.display()
        );
        let (_d, p) = write("[tts]\nenabled = false\nspeed = 1.2\n");
        let c = Config::load(&p).unwrap();
        assert!(!c.tts.enabled);
        assert_eq!(c.tts.speed, 1.2);
        assert_eq!(c.tts.speaker, 1);
        assert_eq!(c.tts.model_dir, TtsConfig::default().model_dir);
    }

    #[test]
    fn bip_seul_pour_reussite_desactive_par_defaut() {
        let c = Config::load(Path::new("/inexistant/config.toml")).unwrap();
        assert!(!c.sound_only_for_success);
        let (_d, p) = write("address = \"Charles\"\nsound_only_for_success = true\n");
        let c = Config::load(&p).unwrap();
        assert!(c.sound_only_for_success);
        let persona = Persona::from(&c);
        assert_eq!(persona.address, "Charles");
        assert!(persona.sound_only_for_success);
    }

    #[test]
    fn config_search_defauts() {
        let c = Config::load(Path::new("/inexistant/config.toml")).unwrap();
        assert_eq!(c.search, SearchSettings::default());
        assert_eq!(c.search.url, "http://127.0.0.1:8888");
        assert_eq!(c.search.local_summary_timeout_ms, 10_000);
        assert_eq!(c.search.online_summary_timeout_ms, 5000);
        assert_eq!(c.search.summary_max_tokens, 80);
        assert!(c.search.summarize);
    }

    #[test]
    fn config_search_partielle() {
        let (_d, p) = write(
            "[search]\nurl = \"http://127.0.0.1:9999\"\nsummarize = false\n\
             video_engines = [\"youtube\", \"peertube\"]\n",
        );
        let c = Config::load(&p).unwrap();
        assert_eq!(c.search.url, "http://127.0.0.1:9999");
        assert!(!c.search.summarize);
        assert_eq!(c.search.video_engines, ["youtube", "peertube"]);
        let defaut = SearchSettings::default();
        assert_eq!(c.search.max_results, defaut.max_results);
        assert_eq!(c.search.language, "fr-FR");
        assert_eq!(c.search.local_summary_model, "");
        assert_eq!(c.local, LocalLlmConfig::default());
    }

    #[test]
    fn config_agent_defauts() {
        let c = Config::load(Path::new("/inexistant/config.toml")).unwrap();
        let a = &c.agent;
        assert_eq!(*a, AgentSettings::default());
        assert!(a.enabled);
        assert_eq!(a.claude_path, PathBuf::from("~/.local/bin/claude"));
        assert_eq!((a.max_minutes, a.idle_minutes, a.max_tasks), (45, 30, 2));
        assert_eq!(a.model, "");
        assert_eq!(a.max_budget_usd, 0.0);
        assert!(!a.pass_api_key);
        assert!(a.commit_at_end);
        assert_eq!(
            a.worktree_dir,
            PathBuf::from("~/.local/share/jarvis/worktrees")
        );
        assert_eq!(
            a.cargo_target_dir,
            PathBuf::from("~/.cache/jarvis/agent-target")
        );
        assert_eq!(
            a.extra_path,
            [PathBuf::from("~/.cargo/bin"), PathBuf::from("~/.local/bin")]
        );
        assert!(a.extra_allowed_tools.is_empty());
        assert_eq!(a.projects.len(), 1);
        assert_eq!(a.projects[0].name, "Jarvis");
        assert_eq!(a.projects[0].aliases, ["l'assistant"]);
        assert_eq!(a.projects[0].base, None);
    }

    #[test]
    fn config_projets_lus() {
        let (_d, p) = write(
            "[agent]\nmax_tasks = 1\npass_api_key = true\n\
             extra_allowed_tools = [\"Bash(make*)\"]\n\
             [[agent.projects]]\nname = \"Jarvis\"\npath = \"~/Work/Mawena/Projects/Jarvis\"\n\
             aliases = [\"l'assistant\"]\n\
             [[agent.projects]]\nname = \"Mawena\"\npath = \"/srv/mawena\"\nbase = \"dev\"\n\
             extra_allowed_tools = [\"Bash(npm test*)\"]\n",
        );
        let c = Config::load(&p).unwrap();
        let a = &c.agent;
        assert_eq!(a.max_tasks, 1);
        assert!(a.pass_api_key);
        assert_eq!(a.max_minutes, 45);
        assert_eq!(a.extra_allowed_tools, ["Bash(make*)"]);
        let names: Vec<&str> = a.projects.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["Jarvis", "Mawena"]);
        let mawena = &a.projects[1];
        assert_eq!(mawena.path, PathBuf::from("/srv/mawena"));
        assert_eq!(mawena.base.as_deref(), Some("dev"));
        assert!(mawena.aliases.is_empty());
        assert_eq!(mawena.extra_allowed_tools, ["Bash(npm test*)"]);
        // `~/` développé à la demande.
        if let Some(home) = std::env::var_os("HOME") {
            let r = a.resolved();
            assert_eq!(
                r.projects[0].path,
                PathBuf::from(home).join("Work/Mawena/Projects/Jarvis")
            );
            assert_eq!(r.projects[1].path, PathBuf::from("/srv/mawena"));
        }
        assert_eq!(c.search, SearchSettings::default());
    }

    #[test]
    fn config_people_defaut_et_proprietaire() {
        let c = Config::default();
        assert_eq!(c.people.owner, "Charles");
        let (_d, p) = write("[people]\nowner = \"Léa\"\n");
        assert_eq!(Config::load(&p).unwrap().people.owner, "Léa");
        let (_d, p) = write("[network]\nenabled = false\n");
        assert_eq!(Config::load(&p).unwrap().people.owner, "Charles");
    }

    #[test]
    fn config_network_defauts_et_partielle() {
        let dir = tempfile::tempdir().unwrap();
        let c = Config::load(&dir.path().join("absent.toml")).unwrap();
        assert!(c.network.enabled);
        assert_eq!(c.network.listen, "0.0.0.0:7341");
        assert_eq!(c.network.name, "");
        assert_eq!(c.network.arbiter_window_ms, 300);
        let (_d, p) = write("[network]\nenabled = false\nname = \"Bureau\"\n");
        let c = Config::load(&p).unwrap();
        assert!(!c.network.enabled);
        assert_eq!(c.network.name, "Bureau");
        assert_eq!(c.network.listen, "0.0.0.0:7341");
    }

    #[test]
    fn config_invalide_cite_le_chemin() {
        let (_d, p) = write("mode = 3\n");
        let err = Config::load(&p).unwrap_err().to_string();
        assert!(err.contains(&p.display().to_string()), "{err}");
    }
}
