//! Vue de l'état pour la notification : le texte est rédigé ici, en Rust, jamais en
//! Kotlin.

use std::sync::Mutex;

use jarvis_ear_core::MicState;
use jarvis_ear_core::session::{BrainLink, EarStatus};
use jarvis_protocol::Mode;

use crate::{BrainView, EarView, MicView};

/// Vue de l'état de la session ; `blocked` : Android livre des zéros exacts depuis
/// plus de 3 s (sans effet si le micro est fermé ou indisponible).
pub fn view_of(status: &EarStatus, blocked: bool) -> EarView {
    let mic = match status.mic {
        MicState::HardMuted => MicView::HardMuted,
        MicState::Unavailable => MicView::Unavailable,
        _ if blocked => MicView::Blocked,
        MicState::Muted => MicView::Muted,
        MicState::Open => MicView::Open,
    };
    let brain = match &status.brain {
        BrainLink::Connected { mode, .. } => BrainView::Connected {
            online: *mode == Mode::Online,
        },
        BrainLink::Connecting => BrainView::Connecting,
        BrainLink::Unreachable => BrainView::Unreachable,
        BrainLink::Refused => BrainView::Refused,
    };
    let mic_text = match mic {
        MicView::HardMuted => "Micro coupé (coupure totale)",
        MicView::Unavailable => "Micro indisponible",
        MicView::Blocked => "Micro bloqué par Android : touchez pour relancer",
        MicView::Muted => "Écoute coupée",
        MicView::Open => "À l'écoute",
    };
    let brain_text = match brain {
        BrainView::Connected { online: false } => "mode local",
        BrainView::Connected { online: true } => "mode en ligne",
        BrainView::Connecting => "connexion au PC…",
        BrainView::Unreachable => "PC injoignable",
        BrainView::Refused => "appairage refusé : réappairez depuis l'appli",
    };
    EarView {
        mic,
        brain,
        text: format!("{mic_text} · {brain_text}"),
        attention: matches!(mic, MicView::Blocked | MicView::Unavailable)
            || matches!(brain, BrainView::Unreachable | BrainView::Refused),
        last_heard: status.last_heard.clone(),
    }
}

/// Retient la dernière vue envoyée à Kotlin : on ne renvoie que si `(mic, brain, text)`
/// change (la dernière phrase entendue ne s'affiche pas dans la notification).
#[derive(Default)]
pub struct Notifier {
    sent: Mutex<Option<EarView>>,
}

impl Notifier {
    /// Appelle `send(view)` si la vue diffère de la précédente. Le verrou est tenu
    /// pendant l'appel : l'ordre des envois est celui des vues.
    pub fn notify(&self, view: &EarView, send: impl FnOnce(&EarView)) -> bool {
        let mut sent = self.sent.lock().unwrap_or_else(|e| e.into_inner());
        let same = sent.as_ref().is_some_and(|old| {
            (old.mic, old.brain, &old.text) == (view.mic, view.brain, &view.text)
        });
        if same {
            return false;
        }
        *sent = Some(view.clone());
        send(view);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(mic: MicState, brain: BrainLink) -> EarStatus {
        EarStatus {
            mic,
            capturing: false,
            brain,
            last_heard: None,
        }
    }

    fn connected(mode: Mode) -> BrainLink {
        BrainLink::Connected {
            mode,
            local_model: "llama3.2:3b".into(),
        }
    }

    #[test]
    fn vue_a_l_ecoute_mode_local() {
        let v = view_of(&status(MicState::Open, connected(Mode::Local)), false);
        assert_eq!(v.text, "À l'écoute · mode local");
        assert_eq!(v.mic, MicView::Open);
        assert_eq!(v.brain, BrainView::Connected { online: false });
        assert!(!v.attention);
    }

    #[test]
    fn vue_a_l_ecoute_mode_en_ligne() {
        let v = view_of(&status(MicState::Open, connected(Mode::Online)), false);
        assert_eq!(v.text, "À l'écoute · mode en ligne");
        assert_eq!(v.brain, BrainView::Connected { online: true });
    }

    #[test]
    fn vue_ecoute_coupee_pc_injoignable() {
        let v = view_of(&status(MicState::Muted, BrainLink::Unreachable), false);
        assert_eq!(v.text, "Écoute coupée · PC injoignable");
        assert!(v.attention);
    }

    #[test]
    fn vue_coupure_totale() {
        let v = view_of(&status(MicState::HardMuted, connected(Mode::Local)), false);
        assert_eq!(v.text, "Micro coupé (coupure totale) · mode local");
        assert_eq!(v.mic, MicView::HardMuted);
        assert!(!v.attention, "coupure voulue : rien à faire");
        // Un micro fermé n'est pas « bloqué ».
        let v = view_of(&status(MicState::HardMuted, connected(Mode::Local)), true);
        assert_eq!(v.mic, MicView::HardMuted);
    }

    #[test]
    fn vue_micro_indisponible() {
        let v = view_of(&status(MicState::Unavailable, BrainLink::Connecting), true);
        assert_eq!(v.text, "Micro indisponible · connexion au PC…");
        assert_eq!(v.mic, MicView::Unavailable);
        assert!(v.attention);
    }

    #[test]
    fn vue_micro_bloque_demande_attention() {
        let v = view_of(&status(MicState::Open, connected(Mode::Local)), true);
        assert_eq!(v.mic, MicView::Blocked);
        assert_eq!(
            v.text,
            "Micro bloqué par Android : touchez pour relancer · mode local"
        );
        assert!(v.attention);
    }

    #[test]
    fn vue_appairage_refuse() {
        let v = view_of(&status(MicState::Open, BrainLink::Refused), false);
        assert_eq!(
            v.text,
            "À l'écoute · appairage refusé : réappairez depuis l'appli"
        );
        assert!(v.attention);
    }

    #[test]
    fn statut_envoye_seulement_quand_la_vue_change() {
        let notifier = Notifier::default();
        let mut sent = Vec::new();
        let mut status = status(MicState::Open, connected(Mode::Local));
        let push = |s: &EarStatus, sent: &mut Vec<String>| {
            notifier.notify(&view_of(s, false), |v| sent.push(v.text.clone()))
        };
        assert!(push(&status, &mut sent));
        // Capture en cours et dernière phrase : pas dans la vue, pas de clignotement.
        status.capturing = true;
        status.last_heard = Some("bonjour".into());
        assert!(!push(&status, &mut sent));
        status.capturing = false;
        assert!(!push(&status, &mut sent));
        status.mic = MicState::Muted;
        assert!(push(&status, &mut sent));
        assert!(!push(&status, &mut sent));
        assert_eq!(
            sent,
            ["À l'écoute · mode local", "Écoute coupée · mode local"]
        );
    }
}
