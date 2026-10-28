//! Icône de la zone de notification : statut du micro et menu de contrôle.

use jarvis_ear_core::MicState;
use jarvis_protocol::Mode;
use ksni::menu::{MenuItem, RadioGroup, RadioItem, StandardItem};
use tokio::sync::mpsc::UnboundedSender;

/// Liaison avec le cerveau.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrainLink {
    Connected { mode: Mode, local_model: String },
    Unreachable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayState {
    pub mic: MicState,
    pub capturing: bool,
    pub brain: BrainLink,
    pub last_heard: Option<String>,
}

/// Commandes émises par le menu de l'icône.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCommand {
    Mute,
    Unmute,
    HardMute,
    Reopen,
    SetMode(Mode),
    Quit,
}

pub struct JarvisTray {
    pub state: TrayState,
    pub commands: UnboundedSender<TrayCommand>,
}

fn brain_unreachable(state: &TrayState) -> bool {
    state.brain == BrainLink::Unreachable
}

pub fn icon_for(state: &TrayState) -> &'static str {
    if state.mic == MicState::HardMuted {
        "microphone-disabled-symbolic"
    } else if brain_unreachable(state) {
        "network-offline"
    } else if state.mic == MicState::Unavailable {
        "dialog-warning"
    } else if state.mic == MicState::Muted {
        "microphone-sensitivity-muted"
    } else if state.capturing {
        "media-record"
    } else {
        "audio-input-microphone"
    }
}

pub fn summary(state: &TrayState) -> String {
    if state.mic == MicState::HardMuted {
        return "Jarvis — micro coupé".into();
    }
    if brain_unreachable(state) {
        return "Jarvis — cerveau injoignable".into();
    }
    match state.mic {
        MicState::Unavailable => "Jarvis — micro indisponible".into(),
        MicState::Muted => "Jarvis — écoute coupée".into(),
        _ => match &state.brain {
            BrainLink::Connected {
                mode: Mode::Local,
                local_model,
            } => format!("Jarvis — à l'écoute · mode local ({local_model})"),
            _ => "Jarvis — à l'écoute · mode en ligne".into(),
        },
    }
}

fn listen_label(state: &TrayState) -> &'static str {
    if state.mic == MicState::Muted {
        "Réactiver l'écoute"
    } else {
        "Couper l'écoute"
    }
}

fn hard_mute_label(state: &TrayState) -> &'static str {
    if state.mic == MicState::HardMuted {
        "Rouvrir le micro"
    } else {
        "Coupure totale du micro"
    }
}

pub fn menu_labels(state: &TrayState) -> Vec<String> {
    let mut labels = vec![
        summary(state),
        listen_label(state).into(),
        hard_mute_label(state).into(),
        "Mode local".into(),
        "Mode en ligne".into(),
    ];
    if let Some(text) = &state.last_heard {
        labels.push(format!("Dernière phrase : {text}"));
    }
    labels.push("Quitter l'oreille".into());
    labels
}

impl JarvisTray {
    fn send(&self, command: TrayCommand) {
        let _ = self.commands.send(command);
    }
}

fn entry(
    label: &str,
    enabled: bool,
    activate: impl Fn(&mut JarvisTray) + Send + 'static,
) -> MenuItem<JarvisTray> {
    StandardItem {
        label: label.into(),
        enabled,
        activate: Box::new(activate),
        ..Default::default()
    }
    .into()
}

impl ksni::Tray for JarvisTray {
    fn id(&self) -> String {
        "jarvis".into()
    }

    fn title(&self) -> String {
        "Jarvis".into()
    }

    fn icon_name(&self) -> String {
        icon_for(&self.state).into()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            icon_name: icon_for(&self.state).into(),
            icon_pixmap: Vec::new(),
            title: "Jarvis".into(),
            description: summary(&self.state),
        }
    }

    fn status(&self) -> ksni::Status {
        if brain_unreachable(&self.state) || self.state.mic == MicState::Unavailable {
            ksni::Status::NeedsAttention
        } else {
            ksni::Status::Active
        }
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let state = &self.state;
        let hard_muted = state.mic == MicState::HardMuted;
        let muted = state.mic == MicState::Muted;
        let mut items = vec![
            entry(&summary(state), false, |_| {}),
            entry(listen_label(state), !hard_muted, move |t| {
                t.send(if muted {
                    TrayCommand::Unmute
                } else {
                    TrayCommand::Mute
                });
            }),
            entry(hard_mute_label(state), true, move |t| {
                t.send(if hard_muted {
                    TrayCommand::Reopen
                } else {
                    TrayCommand::HardMute
                });
            }),
        ];
        let (selected, enabled) = match &state.brain {
            BrainLink::Connected {
                mode: Mode::Local, ..
            } => (0, true),
            BrainLink::Connected {
                mode: Mode::Online, ..
            } => (1, true),
            BrainLink::Unreachable => (0, false),
        };
        let radio = |label: &str| RadioItem {
            label: label.into(),
            enabled,
            ..Default::default()
        };
        items.push(
            RadioGroup {
                selected,
                select: Box::new(|t: &mut JarvisTray, index| {
                    t.send(TrayCommand::SetMode(if index == 0 {
                        Mode::Local
                    } else {
                        Mode::Online
                    }));
                }),
                options: vec![radio("Mode local"), radio("Mode en ligne")],
            }
            .into(),
        );
        if let Some(text) = &state.last_heard {
            items.push(entry(&format!("Dernière phrase : {text}"), false, |_| {}));
        }
        items.push(MenuItem::Separator);
        items.push(entry("Quitter l'oreille", true, |t| {
            t.send(TrayCommand::Quit)
        }));
        items
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connected(mic: MicState, capturing: bool) -> TrayState {
        TrayState {
            mic,
            capturing,
            brain: BrainLink::Connected {
                mode: Mode::Local,
                local_model: "llama3.2:3b".into(),
            },
            last_heard: None,
        }
    }

    fn tray_of(
        state: TrayState,
    ) -> (
        JarvisTray,
        tokio::sync::mpsc::UnboundedReceiver<TrayCommand>,
    ) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        (
            JarvisTray {
                state,
                commands: tx,
            },
            rx,
        )
    }

    #[test]
    fn icone_selon_etat() {
        assert_eq!(
            icon_for(&connected(MicState::Open, false)),
            "audio-input-microphone"
        );
        assert_eq!(icon_for(&connected(MicState::Open, true)), "media-record");
        assert_eq!(
            icon_for(&connected(MicState::Muted, false)),
            "microphone-sensitivity-muted"
        );
        assert_eq!(
            icon_for(&connected(MicState::HardMuted, false)),
            "microphone-disabled-symbolic"
        );
        assert_eq!(
            icon_for(&connected(MicState::Unavailable, false)),
            "dialog-warning"
        );
        let mut s = connected(MicState::Open, false);
        s.brain = BrainLink::Unreachable;
        assert_eq!(icon_for(&s), "network-offline");
    }

    #[test]
    fn resume_mode_local() {
        assert_eq!(
            summary(&connected(MicState::Open, false)),
            "Jarvis — à l'écoute · mode local (llama3.2:3b)"
        );
        assert_eq!(
            summary(&connected(MicState::Muted, false)),
            "Jarvis — écoute coupée"
        );
        assert_eq!(
            summary(&connected(MicState::HardMuted, false)),
            "Jarvis — micro coupé"
        );
        assert_eq!(
            summary(&connected(MicState::Unavailable, false)),
            "Jarvis — micro indisponible"
        );
    }

    #[test]
    fn cerveau_injoignable_prioritaire() {
        let mut s = connected(MicState::Muted, true);
        s.brain = BrainLink::Unreachable;
        assert_eq!(icon_for(&s), "network-offline");
        assert_eq!(summary(&s), "Jarvis — cerveau injoignable");
        let (tray, _rx) = tray_of(s.clone());
        assert!(matches!(
            ksni::Tray::status(&tray),
            ksni::Status::NeedsAttention
        ));
        s.mic = MicState::HardMuted;
        assert_eq!(icon_for(&s), "microphone-disabled-symbolic");
    }

    #[test]
    fn menu_ecoute_coupee() {
        let labels = menu_labels(&connected(MicState::Muted, false));
        assert!(labels.contains(&"Réactiver l'écoute".to_string()));
        let labels = menu_labels(&connected(MicState::HardMuted, false));
        assert!(labels.contains(&"Rouvrir le micro".to_string()));
    }

    #[test]
    fn menu_mode_desactive_sans_cerveau() {
        let mut s = connected(MicState::Open, false);
        s.brain = BrainLink::Unreachable;
        let (tray, _rx) = tray_of(s);
        let group = ksni::Tray::menu(&tray)
            .into_iter()
            .find_map(|m| match m {
                MenuItem::RadioGroup(g) => Some(g),
                _ => None,
            })
            .expect("groupe radio");
        assert!(group.options.iter().all(|o| !o.enabled));
    }

    #[test]
    fn action_menu_envoie_commande() {
        let (mut tray, mut rx) = tray_of(connected(MicState::Open, false));
        let item = ksni::Tray::menu(&tray)
            .into_iter()
            .find_map(|m| match m {
                MenuItem::Standard(s) if s.label == "Couper l'écoute" => Some(s),
                _ => None,
            })
            .expect("entrée");
        (item.activate)(&mut tray);
        assert_eq!(rx.try_recv().unwrap(), TrayCommand::Mute);
    }

    /// Vérifie l'enregistrement réel sur le bus de session (5 s).
    #[tokio::test]
    #[ignore = "nécessite un bus de session et l'extension AppIndicator"]
    async fn enregistrement_reel() {
        use ksni::TrayMethods;
        let (tray, _rx) = tray_of(connected(MicState::Open, false));
        let handle = tray.spawn().await.expect("icône enregistrée");
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        handle.shutdown().await;
    }
}
