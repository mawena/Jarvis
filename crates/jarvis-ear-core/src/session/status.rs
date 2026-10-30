//! État publié par la session (icône du PC, notification d'Android) et commandes
//! qu'elle reçoit (menu de l'icône, boutons de la notification).

use jarvis_protocol::Mode;

use crate::MicState;

/// Liaison avec le cerveau (poussée par le lien réseau).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrainLink {
    Connected {
        mode: Mode,
        local_model: String,
    },
    /// Première tentative, ou reconnexion après une coupure.
    Connecting,
    Unreachable,
    /// Jeton ou oreille refusés (ou version incompatible) : il faut réappairer.
    Refused,
}

/// État de l'oreille, publié à chaque changement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EarStatus {
    pub mic: MicState,
    pub capturing: bool,
    pub brain: BrainLink,
    pub last_heard: Option<String>,
}

/// Commandes reçues par la session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EarCommand {
    Mute,
    Unmute,
    HardMute,
    Reopen,
    SetMode(Mode),
    Quit,
}
