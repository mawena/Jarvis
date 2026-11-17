//! Icône de la zone de notification : statut du micro et menu de contrôle.

use jarvis_ear_core::MicState;
use jarvis_ear_core::session::{BrainLink, EarCommand as TrayCommand, EarStatus as TrayState};
use jarvis_protocol::Mode;
use ksni::menu::{MenuItem, RadioGroup, RadioItem, StandardItem, SubMenu};
use std::sync::OnceLock;
use tokio::sync::mpsc::UnboundedSender;

pub struct JarvisTray {
    pub state: TrayState,
    pub commands: UnboundedSender<TrayCommand>,
}

/// Mode correspondant à l'index du groupe radio (0 = local, 1 = en ligne).
fn mode_for_index(index: usize) -> Mode {
    if index == 0 {
        Mode::Local
    } else {
        Mode::Online
    }
}

fn brain_unreachable(state: &TrayState) -> bool {
    !matches!(state.brain, BrainLink::Connected { .. })
}

/// À signaler : PC injoignable ou appairage refusé (pas pendant une connexion).
fn needs_attention(state: &TrayState) -> bool {
    matches!(state.brain, BrainLink::Unreachable | BrainLink::Refused)
        || state.mic == MicState::Unavailable
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
    match state.brain {
        BrainLink::Unreachable => return "Jarvis — PC injoignable".into(),
        BrainLink::Connecting => return "Jarvis — connexion au PC…".into(),
        BrainLink::Refused => {
            return "Jarvis — appairage refusé : relancez jarvis-ear pair".into();
        }
        BrainLink::Connected { .. } => {}
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

/// Décalage de l'heure locale (secondes), lu une fois par `date +%z` (`+0100`).
/// Inconnu : heure UTC.
fn local_offset() -> i64 {
    static OFFSET: OnceLock<i64> = OnceLock::new();
    *OFFSET.get_or_init(|| {
        std::process::Command::new("date")
            .arg("+%z")
            .output()
            .ok()
            .and_then(|o| parse_offset(String::from_utf8_lossy(&o.stdout).trim()))
            .unwrap_or(0)
    })
}

/// `+0130` → 5400 s, `-0500` → -18000 s.
fn parse_offset(text: &str) -> Option<i64> {
    let (sign, digits) = match text.split_at_checked(1)? {
        ("+", d) => (1, d),
        ("-", d) => (-1, d),
        _ => return None,
    };
    if digits.len() != 4 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let hours: i64 = digits[..2].parse().ok()?;
    let minutes: i64 = digits[2..].parse().ok()?;
    Some(sign * (hours * 3600 + minutes * 60))
}

/// Heure `HH:MM` d'un instant Unix, avec le décalage donné.
fn clock(unix: u64, offset: i64) -> String {
    let secs = (unix as i64 + offset).rem_euclid(86_400);
    format!("{:02}:{:02}", secs / 3600, secs % 3600 / 60)
}

/// Texte affiché tel quel dans un menu (`_` y marque sinon un raccourci clavier).
fn literal(text: &str) -> String {
    text.replace('_', "__")
}

fn plural(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

/// Résumé court des oreilles et des tâches (infobulle) ; vide hors connexion.
pub fn activity(state: &TrayState) -> Option<String> {
    if brain_unreachable(state) {
        return None;
    }
    let connected = state.ears.iter().filter(|e| e.connected).count();
    let mut text = plural(connected, "oreille connectée", "oreilles connectées");
    match state.tasks.as_slice() {
        [] => {}
        [task] => text.push_str(&format!(" · agent sur {}", task.project)),
        tasks => text.push_str(&format!(" · {} tâches d'agent", tasks.len())),
    }
    Some(text)
}

/// Lignes du sous-menu « Statut détaillé » (`offset` : décalage de l'heure locale).
pub fn detail_lines(state: &TrayState, offset: i64) -> Vec<String> {
    let mut lines = Vec::new();
    if brain_unreachable(state) {
        lines.push("Cerveau injoignable".into());
    } else {
        if state.ears.is_empty() {
            lines.push("Aucune oreille appairée".into());
        }
        for ear in &state.ears {
            let who = ear
                .person
                .as_deref()
                .map(|p| format!(", {p}"))
                .unwrap_or_default();
            let link = if ear.connected {
                "connectée"
            } else {
                "déconnectée"
            };
            lines.push(format!("Oreille {}{who} : {link}", ear.name));
        }
        if state.tasks.is_empty() {
            lines.push("Aucune tâche d'agent en cours".into());
        }
        for task in &state.tasks {
            lines.push(format!(
                "Agent : {}, depuis {}",
                task.project,
                clock(task.started_at, offset)
            ));
        }
    }
    if let Some(text) = &state.last_heard {
        lines.push(format!("Dernière phrase : {text}"));
    }
    lines
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

#[cfg(test)]
pub fn menu_labels(state: &TrayState) -> Vec<String> {
    let mut labels = vec![
        summary(state),
        listen_label(state).into(),
        hard_mute_label(state).into(),
        "Mode local".into(),
        "Mode en ligne".into(),
        "Statut détaillé".into(),
    ];
    labels.extend(detail_lines(state, 0).into_iter().map(|l| format!("  {l}")));
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

    fn watcher_online(&self) {
        tracing::info!("zone de notification disponible : icône affichée");
    }

    /// Sans zone de notification (pas encore là au démarrage de la session, ou GNOME Shell
    /// qui redémarre), l'oreille continue et l'icône revient quand la zone réapparaît.
    fn watcher_offline(&self, reason: ksni::OfflineReason) -> bool {
        tracing::warn!("zone de notification absente, icône en attente : {reason:?}");
        true
    }

    fn icon_name(&self) -> String {
        icon_for(&self.state).into()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            icon_name: icon_for(&self.state).into(),
            icon_pixmap: Vec::new(),
            title: "Jarvis".into(),
            description: match activity(&self.state) {
                Some(more) => format!("{}\n{more}", summary(&self.state)),
                None => summary(&self.state),
            },
        }
    }

    fn status(&self) -> ksni::Status {
        if needs_attention(&self.state) {
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
            _ => (0, false),
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
                    t.send(TrayCommand::SetMode(mode_for_index(index)));
                }),
                options: vec![radio("Mode local"), radio("Mode en ligne")],
            }
            .into(),
        );
        let details = detail_lines(state, local_offset())
            .iter()
            .map(|line| entry(&literal(line), false, |_| {}))
            .collect();
        items.push(
            SubMenu {
                label: "Statut détaillé".into(),
                submenu: details,
                ..Default::default()
            }
            .into(),
        );
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
    use jarvis_protocol::EarId;
    use jarvis_protocol::ear::{EarSummary, TaskSummary};

    fn connected(mic: MicState, capturing: bool) -> TrayState {
        TrayState {
            mic,
            capturing,
            brain: BrainLink::Connected {
                mode: Mode::Local,
                local_model: "llama3.2:3b".into(),
            },
            last_heard: None,
            ears: vec![ear("PC", true, true), ear("Pixel", false, false)],
            tasks: Vec::new(),
        }
    }

    fn ear(name: &str, connected: bool, local: bool) -> EarSummary {
        EarSummary {
            id: EarId::new(name),
            name: name.into(),
            platform: "linux".into(),
            connected,
            local,
            person: Some("Charles".into()),
        }
    }

    fn task(project: &str, started_at: u64) -> TaskSummary {
        TaskSummary {
            id: format!("t-{project}"),
            project: project.into(),
            started_at,
        }
    }

    #[test]
    fn decalage_horaire_lu() {
        assert_eq!(parse_offset("+0100"), Some(3600));
        assert_eq!(parse_offset("+0530"), Some(19_800));
        assert_eq!(parse_offset("-0500"), Some(-18_000));
        assert_eq!(parse_offset("0100"), None);
        assert_eq!(parse_offset("+01"), None);
        assert_eq!(parse_offset(""), None);
    }

    #[test]
    fn heure_locale_de_depart() {
        // 2026-11-17 13:32:10 UTC.
        let t = 1_794_922_330;
        assert_eq!(clock(t, 0), "13:32");
        assert_eq!(clock(t, 3600), "14:32");
        assert_eq!(clock(t, -14 * 3600), "23:32");
    }

    #[test]
    fn statut_detaille_oreilles_et_taches() {
        let mut state = connected(MicState::Open, false);
        state.tasks = vec![task("Jarvis", 1_794_922_330)];
        state.last_heard = Some("suivant".into());
        assert_eq!(
            detail_lines(&state, 3600),
            [
                "Oreille PC, Charles : connectée",
                "Oreille Pixel, Charles : déconnectée",
                "Agent : Jarvis, depuis 14:32",
                "Dernière phrase : suivant",
            ]
        );
        assert_eq!(
            activity(&state).unwrap(),
            "1 oreille connectée · agent sur Jarvis"
        );
        state.tasks.push(task("Mawena", 1_794_922_330));
        assert_eq!(
            activity(&state).unwrap(),
            "1 oreille connectée · 2 tâches d'agent"
        );
    }

    #[test]
    fn statut_detaille_sans_tache_ni_cerveau() {
        let mut state = connected(MicState::Open, false);
        state.ears.clear();
        assert_eq!(
            detail_lines(&state, 0),
            ["Aucune oreille appairée", "Aucune tâche d'agent en cours"]
        );
        assert_eq!(activity(&state).unwrap(), "0 oreilles connectées");
        state.brain = BrainLink::Unreachable;
        assert_eq!(detail_lines(&state, 0), ["Cerveau injoignable"]);
        assert_eq!(activity(&state), None);
    }

    #[test]
    fn soulignes_affiches_tels_quels() {
        assert_eq!(literal("Agent : mon_projet"), "Agent : mon__projet");
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
        assert_eq!(summary(&s), "Jarvis — PC injoignable");
        let (tray, _rx) = tray_of(s.clone());
        assert!(matches!(
            ksni::Tray::status(&tray),
            ksni::Status::NeedsAttention
        ));
        s.mic = MicState::HardMuted;
        assert_eq!(icon_for(&s), "microphone-disabled-symbolic");
    }

    #[test]
    fn icone_pc_injoignable() {
        let mut s = connected(MicState::Open, false);
        for (link, text, attention) in [
            (BrainLink::Unreachable, "Jarvis — PC injoignable", true),
            (BrainLink::Connecting, "Jarvis — connexion au PC…", false),
            (
                BrainLink::Refused,
                "Jarvis — appairage refusé : relancez jarvis-ear pair",
                true,
            ),
        ] {
            s.brain = link;
            assert_eq!(icon_for(&s), "network-offline");
            assert_eq!(summary(&s), text);
            let (tray, _rx) = tray_of(s.clone());
            assert_eq!(
                matches!(ksni::Tray::status(&tray), ksni::Status::NeedsAttention),
                attention,
                "{text}"
            );
        }
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

    #[test]
    fn index_radio_vers_mode() {
        let (mut tray, mut rx) = tray_of(connected(MicState::Open, false));
        let group = ksni::Tray::menu(&tray)
            .into_iter()
            .find_map(|m| match m {
                MenuItem::RadioGroup(g) => Some(g),
                _ => None,
            })
            .expect("groupe radio");
        (group.select)(&mut tray, 1);
        (group.select)(&mut tray, 0);
        assert_eq!(rx.try_recv().unwrap(), TrayCommand::SetMode(Mode::Online));
        assert_eq!(rx.try_recv().unwrap(), TrayCommand::SetMode(Mode::Local));
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
