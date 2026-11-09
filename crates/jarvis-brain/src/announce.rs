//! Tableau des annonces : notification de bureau tout de suite, voix gardée pour
//! l'oreille la plus récemment active, qui la relève avec son statut.

use crate::brain::Clock;
use jarvis_skills::{Announcer, CommandRunner, Notice, Outcome};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::Notify;
use tracing::{debug, info, warn};

/// Annonces gardées au plus : au-delà, la plus ancienne est oubliée.
pub const MAX_PENDING: usize = 10;

/// Durée de vie d'une annonce non relevée.
pub const LIFETIME: Duration = Duration::from_secs(10 * 60);

/// Oreille cible sans relève depuis ce délai : une autre oreille peut la prendre.
pub const STALE_AFTER: Duration = Duration::from_secs(20);

/// Annonce en attente, avec son heure d'arrivée (gardée si elle est remise en file).
#[derive(Debug, Clone, PartialEq)]
pub struct Pending {
    pub speech: String,
    pub outcome: Outcome,
    at: Instant,
}

#[derive(Default)]
struct State {
    queue: VecDeque<Pending>,
    /// Oreille de la dernière phrase, et quand.
    target: Option<(String, Instant)>,
    /// Dernière relève de chaque oreille.
    polls: HashMap<String, Instant>,
}

/// Annonces en attente. Ne connaît que des noms d'oreille, pas le transport.
pub struct AnnouncementBoard {
    runner: Arc<dyn CommandRunner>,
    clock: Arc<dyn Clock>,
    state: Mutex<State>,
    changed: Notify,
}

impl AnnouncementBoard {
    pub fn new(runner: Arc<dyn CommandRunner>, clock: Arc<dyn Clock>) -> Self {
        Self {
            runner,
            clock,
            state: Mutex::new(State::default()),
            changed: Notify::new(),
        }
    }

    /// Une phrase est arrivée de cette oreille : elle devient la cible.
    pub fn touch(&self, ear: &str) {
        let now = self.clock.now();
        self.state.lock().unwrap().target = Some((ear.to_string(), now));
    }

    /// Annonces à dire par cette oreille (vide si elles reviennent à une autre). Note
    /// aussi la relève.
    pub fn take_for(&self, ear: &str) -> Vec<(String, Outcome)> {
        self.take_pending_for(ear)
            .into_iter()
            .map(|p| (p.speech, p.outcome))
            .collect()
    }

    /// Comme [`Self::take_for`], en gardant l'heure d'arrivée de chaque annonce (pour
    /// [`Self::requeue`]).
    pub fn take_pending_for(&self, ear: &str) -> Vec<Pending> {
        let now = self.clock.now();
        let mut state = self.state.lock().unwrap();
        state.polls.insert(ear.to_string(), now);
        state
            .polls
            .retain(|_, at| now.duration_since(*at) < LIFETIME);
        state.queue.retain(|p| now.duration_since(p.at) < LIFETIME);
        if state.queue.is_empty() || !may_take(&state, ear, now) {
            return Vec::new();
        }
        debug!(
            oreille = ear,
            nombre = state.queue.len(),
            "annonces relevées"
        );
        state.queue.drain(..).collect()
    }

    /// Annonces relevées qui n'ont pas pu partir (oreille partie pendant leur
    /// synthèse) : remises en tête de file, avec leur heure d'arrivée (leur durée de
    /// vie ne repart pas de zéro). Au-delà de [`MAX_PENDING`], les plus anciennes sont
    /// oubliées.
    pub fn requeue(&self, items: Vec<Pending>) {
        if items.is_empty() {
            return;
        }
        let mut state = self.state.lock().unwrap();
        for item in items.into_iter().rev() {
            state.queue.push_front(item);
        }
        while state.queue.len() > MAX_PENDING {
            state.queue.pop_front();
        }
        debug!(nombre = state.queue.len(), "annonces remises en file");
    }

    /// Signalé à chaque nouvelle annonce (envoi immédiat, phase 4).
    pub fn changed(&self) -> &Notify {
        &self.changed
    }

    fn notify_desktop(&self, notice: &Notice) {
        let (urgency, icon) = if notice.urgent {
            ("critical", "dialog-warning")
        } else {
            ("normal", "dialog-information")
        };
        let args = [
            "--app-name=Jarvis".to_string(),
            format!("--urgency={urgency}"),
            format!("--icon={icon}"),
            // Fin des options : un titre qui commence par « - » reste un titre.
            "--".to_string(),
            notice.title.clone(),
            // Le corps est lu comme du balisage (GNOME) : le résumé de l'agent n'y
            // glisse ni lien ni mise en forme.
            escape_markup(&notice.body),
        ];
        if let Err(e) = self.runner.spawn_detached("notify-send", &args) {
            warn!(erreur = %e, "notification de bureau impossible");
        }
    }
}

/// Texte brut pour un corps de notification : `&`, `<` et `>` échappés.
fn escape_markup(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// L'oreille `ear` peut-elle prendre les annonces ?
fn may_take(state: &State, ear: &str, now: Instant) -> bool {
    let Some((target, touched)) = &state.target else {
        // Aucune phrase encore : la première oreille qui relève.
        return true;
    };
    if target == ear {
        return true;
    }
    // Cible muette depuis 20 s (oreille arrêtée) : n'importe quelle oreille.
    let seen = state
        .polls
        .get(target)
        .map_or(*touched, |polled| (*polled).max(*touched));
    now.duration_since(seen) >= STALE_AFTER
}

impl Announcer for AnnouncementBoard {
    fn announce(&self, notice: Notice) {
        info!(titre = %notice.title, urgente = notice.urgent, "annonce");
        self.notify_desktop(&notice);
        if !notice.speak || notice.speech.trim().is_empty() {
            return;
        }
        let now = self.clock.now();
        {
            let mut state = self.state.lock().unwrap();
            if state.queue.len() >= MAX_PENDING {
                warn!("trop d'annonces en attente, la plus ancienne est oubliée");
                state.queue.pop_front();
            }
            state.queue.push_back(Pending {
                speech: notice.speech,
                outcome: notice.outcome,
                at: now,
            });
        }
        self.changed.notify_waiters();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::ManualClock;
    use jarvis_skills::FakeRunner;

    struct Setup {
        board: AnnouncementBoard,
        runner: Arc<FakeRunner>,
        clock: Arc<ManualClock>,
    }

    fn setup() -> Setup {
        let runner = Arc::new(FakeRunner::new());
        let clock = Arc::new(ManualClock::new());
        Setup {
            board: AnnouncementBoard::new(runner.clone(), clock.clone()),
            runner,
            clock,
        }
    }

    fn notice(speech: &str) -> Notice {
        Notice {
            title: "Tâche terminée".into(),
            speech: speech.into(),
            body: "Branche feature/jarvis-tests".into(),
            outcome: Outcome::Done,
            urgent: false,
            speak: true,
        }
    }

    fn speeches(items: Vec<(String, Outcome)>) -> Vec<String> {
        items.into_iter().map(|(s, _)| s).collect()
    }

    #[test]
    fn annonce_notifie_le_bureau() {
        let s = setup();
        s.board.announce(notice("C'est fait."));
        assert_eq!(
            s.runner.calls(),
            [(
                "notify-send".to_string(),
                vec![
                    "--app-name=Jarvis".to_string(),
                    "--urgency=normal".into(),
                    "--icon=dialog-information".into(),
                    "--".into(),
                    "Tâche terminée".into(),
                    "Branche feature/jarvis-tests".into(),
                ]
            )]
        );
    }

    #[test]
    fn corps_de_notification_sans_balisage() {
        let s = setup();
        s.board.announce(Notice {
            title: "Tâche terminée : Jarvis".into(),
            body: "Résumé : <a href=\"https://x.test\">clic</a> & <b>gras</b>".into(),
            ..notice("C'est fait.")
        });
        let (_, args) = &s.runner.calls()[0];
        assert_eq!(args[3], "--");
        assert_eq!(args[4], "Tâche terminée : Jarvis");
        assert_eq!(
            args[5],
            "Résumé : &lt;a href=\"https://x.test\"&gt;clic&lt;/a&gt; &amp; &lt;b&gt;gras&lt;/b&gt;"
        );
        // La phrase dite, elle, reste telle quelle.
        assert_eq!(s.board.take_for("pc")[0].0, "C'est fait.");
    }

    #[test]
    fn annonce_urgente_critique() {
        let s = setup();
        s.board.announce(Notice {
            urgent: true,
            outcome: Outcome::Failed,
            ..notice("Attention.")
        });
        let (_, args) = &s.runner.calls()[0];
        assert!(args.contains(&"--urgency=critical".to_string()), "{args:?}");
        assert!(
            args.contains(&"--icon=dialog-warning".to_string()),
            "{args:?}"
        );
        assert_eq!(
            s.board.take_for("pc"),
            [("Attention.".to_string(), Outcome::Failed)]
        );
    }

    #[test]
    fn annonces_remises_en_tete_avec_leur_age() {
        let s = setup();
        s.board.announce(notice("Première."));
        s.clock.advance(Duration::from_secs(60));
        s.board.announce(notice("Deuxième."));
        let taken = s.board.take_pending_for("pc");
        assert_eq!(taken.len(), 2);
        // Une nouvelle arrive pendant la synthèse, puis l'envoi échoue.
        s.board.announce(notice("Troisième."));
        s.board.requeue(taken);
        assert_eq!(
            speeches(s.board.take_for("pc")),
            ["Première.", "Deuxième.", "Troisième."]
        );
        // L'heure d'arrivée est gardée : la première expire à son heure.
        s.board.announce(notice("Première."));
        let taken = s.board.take_pending_for("pc");
        s.clock.advance(LIFETIME - Duration::from_secs(1));
        s.board.requeue(taken);
        s.clock.advance(Duration::from_secs(1));
        assert!(s.board.take_for("pc").is_empty());
    }

    #[test]
    fn annonces_remises_en_file_bornees() {
        let s = setup();
        for i in 0..MAX_PENDING {
            s.board.announce(notice(&format!("Ancienne {i}.")));
        }
        let taken = s.board.take_pending_for("pc");
        s.board.announce(notice("Nouvelle."));
        s.board.requeue(taken);
        let left = speeches(s.board.take_for("pc"));
        assert_eq!(left.len(), MAX_PENDING);
        // La plus ancienne est oubliée, la nouvelle gardée.
        assert_eq!(left[0], "Ancienne 1.");
        assert_eq!(left.last().unwrap(), "Nouvelle.");
    }

    #[test]
    fn annonce_pour_l_oreille_la_plus_recente() {
        let s = setup();
        s.board.touch("pc");
        s.board.touch("salon");
        s.board.announce(notice("C'est fait."));
        assert!(s.board.take_for("pc").is_empty());
        assert_eq!(speeches(s.board.take_for("salon")), ["C'est fait."]);
    }

    #[test]
    fn annonce_sans_oreille_active_prise_par_la_premiere() {
        let s = setup();
        s.board.announce(notice("C'est fait."));
        assert_eq!(speeches(s.board.take_for("salon")), ["C'est fait."]);
        assert!(s.board.take_for("pc").is_empty());
    }

    #[test]
    fn oreille_absente_annonce_prise_par_une_autre() {
        let s = setup();
        s.board.touch("pc");
        assert!(s.board.take_for("pc").is_empty());
        s.board.announce(notice("C'est fait."));
        // La cible a relevé il y a moins de 20 s : on l'attend.
        s.clock.advance(Duration::from_secs(19));
        assert!(s.board.take_for("salon").is_empty());
        // Plus de relève depuis 20 s : une autre oreille la prend.
        s.clock.advance(Duration::from_secs(1));
        assert_eq!(speeches(s.board.take_for("salon")), ["C'est fait."]);
        assert!(s.board.take_for("pc").is_empty());
    }

    #[test]
    fn annonce_livree_une_seule_fois() {
        let s = setup();
        s.board.announce(notice("Un."));
        s.board.announce(notice("Deux."));
        assert_eq!(speeches(s.board.take_for("pc")), ["Un.", "Deux."]);
        assert!(s.board.take_for("pc").is_empty());
    }

    #[test]
    fn annonce_perimee_oubliee() {
        let s = setup();
        s.board.announce(notice("Vieille."));
        s.clock.advance(LIFETIME - Duration::from_secs(1));
        s.board.announce(notice("Récente."));
        s.clock.advance(Duration::from_secs(1));
        assert_eq!(speeches(s.board.take_for("pc")), ["Récente."]);
    }

    #[test]
    fn file_bornee_garde_les_plus_recentes() {
        let s = setup();
        for i in 0..MAX_PENDING + 2 {
            s.board.announce(notice(&format!("n{i}")));
        }
        let got = speeches(s.board.take_for("pc"));
        assert_eq!(got.len(), MAX_PENDING);
        assert_eq!(got[0], "n2");
    }

    #[test]
    fn notification_seule_sans_voix() {
        let s = setup();
        s.board.announce(Notice {
            speak: false,
            ..notice("Rien à dire.")
        });
        assert_eq!(s.runner.calls().len(), 1);
        assert!(s.board.take_for("pc").is_empty());
    }

    #[tokio::test]
    async fn nouvelle_annonce_signalee() {
        let s = setup();
        let notified = s.board.changed().notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        s.board.announce(notice("C'est fait."));
        tokio::time::timeout(Duration::from_secs(1), notified)
            .await
            .expect("changement signalé");
    }
}
