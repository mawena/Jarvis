//! Configuration `~/.config/jarvis/ear.toml` (tout est optionnel) et emplacement des
//! fichiers de l'oreille, séparés par profil (`--profile`).

use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::Deserialize;

#[derive(Debug, Default, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EarConfig {
    /// Modèle du mot d'éveil (défaut : `~/.local/share/jarvis/wakeword/jarvis.rpw`).
    pub wakeword_path: Option<PathBuf>,
    /// Seuil de détection du mot d'éveil (0 à 1).
    pub threshold: Option<f32>,
    /// Nom du périphérique d'entrée (défaut : le micro par défaut du système).
    pub device: Option<String>,
    /// Nom de cette oreille : annonces (défaut : `pc`) et appairage par `jarvis-ear
    /// pair` (défaut : nom de la machine).
    pub name: Option<String>,
    /// Adresse du cerveau (`hôte[:port]`, port 7341 par défaut), essayée avant les
    /// adresses connues et la découverte mDNS.
    pub brain_address: Option<String>,
}

impl EarConfig {
    pub fn parse(text: &str) -> anyhow::Result<Self> {
        toml::from_str(text).context("ear.toml invalide")
    }

    /// Charge le fichier ; absent = configuration par défaut.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::parse(&text).with_context(|| path.display().to_string()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("lecture de {}", path.display())),
        }
    }

    /// Nom de l'oreille ; absent ou vide : `pc`.
    pub fn name(&self) -> String {
        match self.name.as_deref().map(str::trim) {
            Some(name) if !name.is_empty() => name.to_string(),
            _ => jarvis_protocol::DEFAULT_EAR.to_string(),
        }
    }

    /// Nom donné au cerveau à l'appairage : `name` s'il est donné, sinon le nom de la
    /// machine.
    pub fn pair_name(&self) -> String {
        match self.name.as_deref().map(str::trim) {
            Some(name) if !name.is_empty() => name.to_string(),
            _ => hostname().unwrap_or_else(|| "oreille".into()),
        }
    }

    /// Adresse du cerveau donnée à la main (vide : aucune).
    pub fn brain_address(&self) -> Option<String> {
        self.brain_address
            .as_deref()
            .map(str::trim)
            .filter(|a| !a.is_empty())
            .map(str::to_string)
    }
}

fn hostname() -> Option<String> {
    let name = std::fs::read_to_string("/proc/sys/kernel/hostname")
        .or_else(|_| std::fs::read_to_string("/etc/hostname"))
        .ok()?;
    let name = name.trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// Fichiers d'une oreille. Avec un profil, tout est séparé : deux oreilles possibles
/// sur le même PC (essai).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    /// `~/.config/jarvis/ear[-<profil>].toml`
    pub config: PathBuf,
    /// `~/.local/share/jarvis/ear[-<profil>]/` (appairage, phrase hors ligne ; 0700)
    pub pairing_dir: PathBuf,
    /// `~/.local/state/jarvis/ear[-<profil>]-muted` : présent tant que l'écoute est coupée
    pub mute_file: PathBuf,
}

impl Paths {
    pub fn new(
        config_dir: &Path,
        data_dir: &Path,
        state_dir: &Path,
        profile: Option<&str>,
    ) -> Self {
        let stem = match profile {
            Some(profile) => format!("ear-{profile}"),
            None => "ear".to_string(),
        };
        Paths {
            config: config_dir.join("jarvis").join(format!("{stem}.toml")),
            pairing_dir: data_dir.join("jarvis").join(&stem),
            mute_file: state_dir.join("jarvis").join(format!("{stem}-muted")),
        }
    }

    /// Dossiers de l'utilisateur ; profil vérifié.
    pub fn for_profile(profile: Option<&str>) -> anyhow::Result<Self> {
        if let Some(profile) = profile {
            check_profile(profile)?;
        }
        let base = directories::BaseDirs::new().context("dossier personnel introuvable")?;
        let state = base.state_dir().unwrap_or_else(|| base.data_local_dir());
        Ok(Self::new(
            base.config_dir(),
            base.data_local_dir(),
            state,
            profile,
        ))
    }
}

/// Nom de profil : lettres, chiffres, `-` et `_` (il entre dans des noms de fichiers).
pub fn check_profile(profile: &str) -> anyhow::Result<()> {
    let ok = !profile.is_empty()
        && profile.len() <= 32
        && profile
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    anyhow::ensure!(
        ok,
        "profil « {profile} » invalide : lettres, chiffres, - et _ seulement"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configuration_vide_par_defaut() {
        assert_eq!(EarConfig::parse("").unwrap(), EarConfig::default());
    }

    #[test]
    fn configuration_complete() {
        let c = EarConfig::parse(
            "wakeword_path = \"/x/jarvis.rpw\"\nthreshold = 0.6\ndevice = \"USB Mic\"\nname = \"salon\"\nbrain_address = \"192.168.1.20\"\n",
        )
        .unwrap();
        assert_eq!(c.name(), "salon");
        assert_eq!(c.pair_name(), "salon");
        assert_eq!(c.brain_address().as_deref(), Some("192.168.1.20"));
        assert_eq!(c.wakeword_path, Some(PathBuf::from("/x/jarvis.rpw")));
        assert_eq!(c.threshold, Some(0.6));
        assert_eq!(c.device.as_deref(), Some("USB Mic"));
    }

    #[test]
    fn nom_d_oreille_par_defaut() {
        assert_eq!(EarConfig::default().name(), "pc");
        assert_eq!(EarConfig::parse("name = \"  \"").unwrap().name(), "pc");
    }

    #[test]
    fn adresse_vide_ignoree_et_nom_de_machine_par_defaut() {
        let c = EarConfig::parse("brain_address = \" \"").unwrap();
        assert_eq!(c.brain_address(), None);
        assert!(!c.pair_name().is_empty());
    }

    #[test]
    fn profil_separe_les_fichiers() {
        let (config, data, state) = (Path::new("/c"), Path::new("/d"), Path::new("/s"));
        let main = Paths::new(config, data, state, None);
        assert_eq!(main.config, Path::new("/c/jarvis/ear.toml"));
        assert_eq!(main.pairing_dir, Path::new("/d/jarvis/ear"));
        assert_eq!(main.mute_file, Path::new("/s/jarvis/ear-muted"));
        let trial = Paths::new(config, data, state, Some("essai"));
        assert_eq!(trial.config, Path::new("/c/jarvis/ear-essai.toml"));
        assert_eq!(trial.pairing_dir, Path::new("/d/jarvis/ear-essai"));
        assert_eq!(trial.mute_file, Path::new("/s/jarvis/ear-essai-muted"));
        assert!(check_profile("essai_2-b").is_ok());
        for bad in ["", "../x", "a b", "é"] {
            assert!(check_profile(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn cle_inconnue_refusee() {
        assert!(EarConfig::parse("seuil = 1").is_err());
    }

    #[test]
    fn fichier_absent_donne_le_defaut() {
        let dir = tempfile::tempdir().unwrap();
        let c = EarConfig::load(&dir.path().join("ear.toml")).unwrap();
        assert_eq!(c, EarConfig::default());
    }
}
