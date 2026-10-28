//! Configuration `~/.config/jarvis/ear.toml` (tout est optionnel).

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

    /// Chemin par défaut : `~/.config/jarvis/ear.toml`.
    pub fn default_path() -> anyhow::Result<PathBuf> {
        let base = directories::BaseDirs::new().context("dossier personnel introuvable")?;
        Ok(base.config_dir().join("jarvis").join("ear.toml"))
    }
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
            "wakeword_path = \"/x/jarvis.rpw\"\nthreshold = 0.6\ndevice = \"USB Mic\"\n",
        )
        .unwrap();
        assert_eq!(c.wakeword_path, Some(PathBuf::from("/x/jarvis.rpw")));
        assert_eq!(c.threshold, Some(0.6));
        assert_eq!(c.device.as_deref(), Some("USB Mic"));
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
