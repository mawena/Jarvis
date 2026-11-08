//! Façade de l'oreille de Jarvis pour Android, exposée à Kotlin par UniFFI.
//!
//! Toute la logique reste en Rust (session commune de `jarvis-ear-core`) : Kotlin
//! pousse le son du micro (`push_audio`), joue ce que le moteur lui donne
//! (`EarHost::play_pcm`) et affiche l'état déjà rédigé (`EarHost::status_changed`).
//!
//! Le son traverse la frontière en **octets** PCM 16 bits little-endian (`Vec<u8>`,
//! `ByteArray` en Kotlin) : un `Vec<i16>` deviendrait une `List<Short>`, une boîte
//! par échantillon.
//!
//! Surface figée par la tâche 3 de la phase 5 ; le moteur est dans `engine`.

use std::path::Path;

mod engine;
mod enroll;
mod logging;
mod pairing;
mod view;

pub use engine::EarEngine;
pub use view::view_of;

uniffi::setup_scaffolding!();

/// Réglages du moteur, donnés par l'appli.
#[derive(Debug, Clone, uniffi::Record)]
pub struct EarSettings {
    /// Dossier privé de l'appli (`context.filesDir`).
    pub data_dir: String,
    /// Nom de l'oreille annoncé au cerveau (« Pixel 4 XL »).
    pub name: String,
    /// Fréquence native de la sortie audio du téléphone (Hz).
    pub output_rate: u32,
    /// Seuil du mot d'éveil (0,30 à 0,80).
    pub wake_threshold: f32,
}

/// Erreurs rendues à Kotlin ; le message s'affiche tel quel.
///
/// Erreur « plate » : Kotlin reçoit la variante (`EarException.TooQuiet`…) et, comme
/// `message`, le texte ci-dessous rédigé en Rust (sans quoi UniFFI rendrait
/// `message=…`, et un champ `message` heurterait celui de `Throwable`).
#[derive(Debug, thiserror::Error, uniffi::Error)]
#[uniffi(flat_error)]
pub enum EarError {
    #[error("oreille non appairée : appairez-la au PC depuis l'appli")]
    NotPaired,
    #[error("mot d'éveil non enregistré : enregistrez « Jarvis » depuis l'appli")]
    NoWakeword,
    #[error("{message}")]
    InvalidUri { message: String },
    #[error("appairage refusé par le PC : {message}")]
    Refused { message: String },
    #[error("{message}")]
    Unreachable { message: String },
    #[error("prise trop faible : rapprochez-vous et recommencez")]
    TooQuiet,
    #[error("{message}")]
    Io { message: String },
    #[error("erreur interne : {message}")]
    Internal { message: String },
}

/// État du micro vu par la notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum MicView {
    /// À l'écoute.
    Open,
    /// Écoute coupée (mot d'éveil ignoré, micro ouvert).
    Muted,
    /// Coupure totale : micro fermé.
    HardMuted,
    /// Micro indisponible.
    Unavailable,
    /// Android livre des zéros exacts (service démarré hors premier plan, micro
    /// coupé dans les réglages rapides) : toucher pour relancer.
    Blocked,
}

/// Lien avec le cerveau vu par la notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum BrainView {
    Connected { online: bool },
    Connecting,
    Unreachable,
    Refused,
}

/// État complet, avec le texte de la notification déjà rédigé.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct EarView {
    pub mic: MicView,
    pub brain: BrainView,
    /// Texte de la notification (« À l'écoute · mode local »).
    pub text: String,
    /// Vrai si l'utilisateur doit agir (micro bloqué ou indisponible, PC injoignable,
    /// appairage refusé).
    pub attention: bool,
    /// Dernière phrase entendue, s'il y en a une.
    pub last_heard: Option<String>,
}

/// Entrées-sorties Android, implémentées en Kotlin. **Aucune méthode ne doit
/// bloquer** : Kotlin délègue à ses propres fils.
#[uniffi::export(foreign)]
pub trait EarHost: Send + Sync {
    /// Ouvrir le micro, puis appeler `audio_format` et `push_audio`.
    fn start_capture(&self);
    /// Fermer le micro.
    fn stop_capture(&self);
    /// Jouer du PCM 16 bits little-endian mono ; appeler `playback_idle(generation)`
    /// quand tout est joué.
    fn play_pcm(&self, pcm_le: Vec<u8>, sample_rate: u32, generation: u64);
    /// Couper la lecture en cours et vider la file.
    fn stop_playback(&self);
    /// L'état a changé : mettre la notification à jour.
    fn status_changed(&self, view: EarView);
}

/// « jarvis-ear-ffi <version> ».
#[uniffi::export]
pub fn library_version() -> String {
    format!("jarvis-ear-ffi {}", env!("CARGO_PKG_VERSION"))
}

/// Journal Rust vers `logcat` (étiquette `JarvisRust`) ; sans effet hors Android.
/// Un second appel ne fait rien.
#[uniffi::export]
pub fn init_logging() {
    logging::init();
}

/// Nom du cerveau appairé, s'il y en a un.
#[uniffi::export]
pub fn paired_brain(data_dir: String) -> Option<String> {
    pairing::paired_brain(Path::new(&data_dir))
}

/// Appaire l'oreille par l'adresse `jarvis://pair?…` (QR code de `jarvis pair`).
/// Bloquante (à appeler hors du fil principal) ; rend le nom du cerveau.
#[uniffi::export]
pub fn pair_with_uri(data_dir: String, uri: String, name: String) -> Result<String, EarError> {
    pairing::pair_real(Path::new(&data_dir), &uri, &name)
}

/// Oublie l'appairage (garde le mot d'éveil).
#[uniffi::export]
pub fn unpair(data_dir: String) -> Result<(), EarError> {
    pairing::unpair(Path::new(&data_dir))
}

/// Vrai si le modèle du mot d'éveil existe.
#[uniffi::export]
pub fn has_wakeword(data_dir: String) -> bool {
    enroll::model_path(Path::new(&data_dir)).exists()
}

/// Efface les prises d'un enregistrement précédent.
#[uniffi::export]
pub fn enroll_reset(data_dir: String) -> Result<(), EarError> {
    enroll::reset(Path::new(&data_dir))
}

/// Enregistre la prise `index` (PCM 16 bits little-endian) ; `TooQuiet` si inaudible.
#[uniffi::export]
pub fn enroll_take(
    data_dir: String,
    index: u32,
    pcm_le: Vec<u8>,
    sample_rate: u32,
) -> Result<(), EarError> {
    enroll::take(Path::new(&data_dir), index, &pcm_le, sample_rate)
}

/// Construit le modèle du mot d'éveil ; rend le nombre de prises utilisées.
#[uniffi::export]
pub fn enroll_finish(data_dir: String) -> Result<u32, EarError> {
    enroll::finish(Path::new(&data_dir))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_de_la_bibliotheque() {
        let version = library_version();
        assert!(version.starts_with("jarvis-ear-ffi "), "{version}");
        assert!(version.ends_with(env!("CARGO_PKG_VERSION")), "{version}");
    }

    #[test]
    fn init_logging_deux_fois_sans_panique() {
        init_logging();
        init_logging();
    }

    #[test]
    fn erreurs_lisibles_en_francais() {
        let detail = || "détail".to_string();
        let errors = [
            EarError::NotPaired,
            EarError::NoWakeword,
            EarError::InvalidUri { message: detail() },
            EarError::Refused { message: detail() },
            EarError::Unreachable { message: detail() },
            EarError::TooQuiet,
            EarError::Io { message: detail() },
            EarError::Internal { message: detail() },
        ];
        for error in errors {
            let text = error.to_string();
            assert!(!text.trim().is_empty(), "{error:?}");
            assert!(!text.contains("Error"), "{text}");
        }
    }
}
