//! Entrées-sorties audio de la session, propres à chaque plateforme : capture du micro
//! ([`Capture`], `cpal` sur le PC) et sortie des sons ([`Player`]).

use tokio::sync::{mpsc, watch};

/// Fréquence des bips si la sortie est inconnue.
pub const FALLBACK_RATE: u32 = 48_000;

/// Sortie sonore (remplaçable dans les tests).
pub trait Player: Send {
    /// Fréquence conseillée pour générer les bips.
    fn sample_rate(&self) -> u32;
    /// Ajoute un son mono à la file ; il sera joué après ceux déjà en file.
    fn play(&mut self, samples: Vec<f32>, sample_rate: u32);
    /// Coupe tout de suite le son en cours et vide la file.
    fn stop(&mut self);
    /// `true` tant qu'un son est en file ou en cours de lecture.
    fn playing(&self) -> watch::Receiver<bool>;

    fn is_playing(&self) -> bool {
        *self.playing().borrow()
    }
}

/// Aucune sortie : les sons sont ignorés.
pub struct SilentPlayer {
    playing: watch::Sender<bool>,
}

impl SilentPlayer {
    pub fn new() -> Self {
        Self {
            playing: watch::channel(false).0,
        }
    }
}

impl Default for SilentPlayer {
    fn default() -> Self {
        Self::new()
    }
}

impl Player for SilentPlayer {
    fn sample_rate(&self) -> u32 {
        FALLBACK_RATE
    }
    fn play(&mut self, _: Vec<f32>, _: u32) {}
    fn stop(&mut self) {}
    fn playing(&self) -> watch::Receiver<bool> {
        self.playing.subscribe()
    }
}

/// Ce que la capture transmet à la session ; `S` : tampon d'échantillons de la
/// plateforme (recyclé sur le PC, simple `Vec<f32>` ailleurs).
#[derive(Debug, PartialEq)]
pub enum CaptureEvent<S> {
    /// Le flux est ouvert avec ce format ; les `Samples` suivants sont entrelacés ainsi.
    Format { rate: u32, channels: u16 },
    /// Échantillons entrelacés au format du périphérique.
    Samples(S),
    /// Le micro est absent ou perdu.
    Unavailable,
}

/// Source audio de la session (remplaçable dans les tests).
pub trait Capture: Send {
    /// Tampon d'échantillons transmis ; rendu à la capture quand la session le libère.
    type Samples: std::ops::Deref<Target = [f32]> + Send + 'static;
    /// Ouvre le flux et envoie les événements dans `tx`.
    fn start(&mut self, tx: mpsc::Sender<CaptureEvent<Self::Samples>>) -> anyhow::Result<()>;
    /// Ferme le flux (coupure totale).
    fn stop(&mut self);
}
