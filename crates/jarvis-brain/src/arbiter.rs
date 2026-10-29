//! Arbitre des oreilles : une seule oreille gagne par phrase. Logique pure, sans
//! entrée-sortie ; l'heure (`now`) est passée en argument.
//!
//! Une phrase à la fois : le premier réveil ouvre une fenêtre ; à sa fin, une seule
//! oreille est élue, les autres reçoivent `Stop`. La phrase reste en cours jusqu'à
//! [`Arbiter::finished`] (ou son abandon) : tout autre réveil reçoit `Stop`.

use jarvis_protocol::EarId;
use std::time::{Duration, Instant};

/// Fenêtre de regroupement des réveils, comptée à l'arrivée au cerveau.
pub const DEFAULT_WINDOW: Duration = Duration::from_millis(300);

/// Réveil d'une oreille, avec ses réglages (`ears.toml`).
#[derive(Debug, Clone, PartialEq)]
pub struct WakeReport {
    pub ear: EarId,
    pub utterance: u32,
    pub score: f32,
    pub level_db: f32,
    pub muted: bool,
    pub bias: f32,
    pub duck: bool,
}

/// `score + 0.5 × clamp((level_db + 60) / 60, 0, 1) + bias`.
pub fn rank(report: &WakeReport) -> f32 {
    let level = ((report.level_db + 60.0) / 60.0).clamp(0.0, 1.0);
    report.score + 0.5 * level + report.bias
}

/// Rang comparable : une valeur invalide (NaN) ne gagne jamais.
fn sort_key(report: &WakeReport) -> f32 {
    let r = rank(report);
    if r.is_nan() { f32::NEG_INFINITY } else { r }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArbiterAction {
    /// Baisser le son (une fois par phrase).
    Duck,
    Elect {
        ear: EarId,
        utterance: u32,
    },
    Stop {
        ear: EarId,
        utterance: u32,
    },
    /// Rétablir le son.
    Restore,
}

/// Que faire de la fin d'une phrase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndVerdict {
    /// La gagnante : à transcrire.
    Process,
    /// Avant la décision : garder l'audio, rappeler `ended` après `Elect`.
    Wait,
    /// Une perdante, ou une phrase inconnue.
    Ignore,
}

enum State {
    /// Aucune phrase en cours.
    Idle,
    /// Fenêtre ouverte : réveils dans l'ordre d'arrivée.
    Window {
        deadline: Instant,
        candidates: Vec<WakeReport>,
        ducked: bool,
    },
    /// Phrase élue (ou réponse sans mot d'éveil). `processing` : fin reçue, en
    /// traitement jusqu'à `finished`.
    Phrase {
        ear: EarId,
        utterance: u32,
        processing: bool,
    },
}

pub struct Arbiter {
    window: Duration,
    state: State,
}

impl Arbiter {
    pub fn new(window: Duration) -> Self {
        Self {
            window,
            state: State::Idle,
        }
    }

    pub fn wake(&mut self, report: WakeReport, now: Instant) -> Vec<ArbiterAction> {
        // Fenêtre échue mais pas encore tranchée : la décision tombe d'abord.
        let mut actions = self.tick(now);
        match &mut self.state {
            State::Idle => {
                let ducked = report.duck;
                if ducked {
                    actions.push(ArbiterAction::Duck);
                }
                self.state = State::Window {
                    deadline: now + self.window,
                    candidates: vec![report],
                    ducked,
                };
            }
            State::Window {
                candidates, ducked, ..
            } => {
                if let Some(c) = candidates.iter().find(|c| c.ear == report.ear) {
                    // Même phrase en double : rien ; autre phrase de la même oreille : arrêtée.
                    if c.utterance != report.utterance {
                        actions.push(stop(report));
                    }
                } else {
                    if report.duck && !*ducked {
                        *ducked = true;
                        actions.push(ArbiterAction::Duck);
                    }
                    candidates.push(report);
                }
            }
            State::Phrase { ear, utterance, .. } => {
                if *ear != report.ear || *utterance != report.utterance {
                    actions.push(stop(report));
                }
            }
        }
        actions
    }

    /// Fin de la fenêtre en cours, s'il y en a une.
    pub fn deadline(&self) -> Option<Instant> {
        match &self.state {
            State::Window { deadline, .. } => Some(*deadline),
            _ => None,
        }
    }

    /// À appeler à l'échéance de [`Self::deadline`] : élit la gagnante.
    pub fn tick(&mut self, now: Instant) -> Vec<ArbiterAction> {
        let State::Window { deadline, .. } = &self.state else {
            return Vec::new();
        };
        if now < *deadline {
            return Vec::new();
        }
        let State::Window { candidates, .. } = std::mem::replace(&mut self.state, State::Idle)
        else {
            unreachable!()
        };
        // Ouverte avant coupée, puis meilleur rang, puis première arrivée.
        let mut best = 0;
        for (i, c) in candidates.iter().enumerate().skip(1) {
            let b = &candidates[best];
            let better = match (c.muted, b.muted) {
                (false, true) => true,
                (true, false) => false,
                _ => sort_key(c) > sort_key(b),
            };
            if better {
                best = i;
            }
        }
        let mut actions = Vec::with_capacity(candidates.len());
        let mut losers = Vec::with_capacity(candidates.len().saturating_sub(1));
        for (i, c) in candidates.into_iter().enumerate() {
            if i == best {
                self.state = State::Phrase {
                    ear: c.ear.clone(),
                    utterance: c.utterance,
                    processing: false,
                };
                actions.push(ArbiterAction::Elect {
                    ear: c.ear,
                    utterance: c.utterance,
                });
            } else {
                losers.push(stop(c));
            }
        }
        actions.extend(losers);
        actions
    }

    pub fn ended(&mut self, ear: &EarId, utterance: u32) -> EndVerdict {
        match &mut self.state {
            State::Window { candidates, .. }
                if candidates
                    .iter()
                    .any(|c| &c.ear == ear && c.utterance == utterance) =>
            {
                EndVerdict::Wait
            }
            State::Phrase {
                ear: e,
                utterance: u,
                processing,
            } if e == ear && *u == utterance => {
                *processing = true;
                EndVerdict::Process
            }
            _ => EndVerdict::Ignore,
        }
    }

    /// Capture sans mot d'éveil (`ListenStart`) : acceptée si aucune phrase n'est en cours.
    pub fn follow_up(&mut self, ear: &EarId, utterance: u32) -> bool {
        if !matches!(self.state, State::Idle) {
            return false;
        }
        self.state = State::Phrase {
            ear: ear.clone(),
            utterance,
            processing: false,
        };
        true
    }

    /// Phrase traitée : le groupe est fermé.
    pub fn finished(&mut self, ear: &EarId, utterance: u32) {
        if self.is_current(ear, utterance) {
            self.state = State::Idle;
        }
    }

    /// Oreille partie. Gagnante en pleine capture, ou dernière de la fenêtre :
    /// son rétabli, groupe fermé. En traitement, la phrase va jusqu'au bout
    /// (`listen` rétablit le son) : `finished` fermera le groupe.
    pub fn disconnected(&mut self, ear: &EarId) -> Vec<ArbiterAction> {
        match &mut self.state {
            State::Idle => Vec::new(),
            State::Window { candidates, .. } => {
                candidates.retain(|c| &c.ear != ear);
                if candidates.is_empty() {
                    self.state = State::Idle;
                    vec![ArbiterAction::Restore]
                } else {
                    Vec::new()
                }
            }
            State::Phrase {
                ear: e, processing, ..
            } => {
                if e == ear && !*processing {
                    self.state = State::Idle;
                    vec![ArbiterAction::Restore]
                } else {
                    Vec::new()
                }
            }
        }
    }

    /// Phrase abandonnée (trop longue…) : son rétabli, groupe fermé.
    pub fn cancelled(&mut self, ear: &EarId, utterance: u32) -> Vec<ArbiterAction> {
        match &mut self.state {
            State::Window { candidates, .. } => {
                let before = candidates.len();
                candidates.retain(|c| !(&c.ear == ear && c.utterance == utterance));
                if before != candidates.len() && candidates.is_empty() {
                    self.state = State::Idle;
                    vec![ArbiterAction::Restore]
                } else {
                    Vec::new()
                }
            }
            State::Phrase {
                ear: e,
                utterance: u,
                processing: false,
            } if e == ear && *u == utterance => {
                self.state = State::Idle;
                vec![ArbiterAction::Restore]
            }
            _ => Vec::new(),
        }
    }

    fn is_current(&self, ear: &EarId, utterance: u32) -> bool {
        matches!(&self.state, State::Phrase { ear: e, utterance: u, .. } if e == ear && *u == utterance)
    }
}

fn stop(report: WakeReport) -> ArbiterAction {
    ArbiterAction::Stop {
        ear: report.ear,
        utterance: report.utterance,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn id(name: &str) -> EarId {
        EarId::new(name)
    }

    /// Réveil ouvert, à baisser, niveau moyen.
    fn report(ear: &str, utterance: u32, score: f32) -> WakeReport {
        WakeReport {
            ear: id(ear),
            utterance,
            score,
            level_db: -30.0,
            muted: false,
            bias: 0.0,
            duck: true,
        }
    }

    fn elect(ear: &str, utterance: u32) -> ArbiterAction {
        ArbiterAction::Elect {
            ear: id(ear),
            utterance,
        }
    }

    fn stop(ear: &str, utterance: u32) -> ArbiterAction {
        ArbiterAction::Stop {
            ear: id(ear),
            utterance,
        }
    }

    fn arbiter() -> Arbiter {
        Arbiter::new(DEFAULT_WINDOW)
    }

    /// Réveils arrivés tous à `t0`, puis fin de la fenêtre : actions de la décision.
    fn decide(reports: Vec<WakeReport>) -> Vec<ArbiterAction> {
        let mut a = arbiter();
        let t0 = Instant::now();
        for r in reports {
            a.wake(r, t0);
        }
        a.tick(t0 + DEFAULT_WINDOW)
    }

    #[test]
    fn rang_selon_la_formule() {
        let mut r = report("a", 1, 0.8);
        assert!((rank(&r) - (0.8 + 0.25)).abs() < 1e-6);
        r.level_db = 10.0;
        r.bias = 0.1;
        assert!((rank(&r) - (0.8 + 0.5 + 0.1)).abs() < 1e-6);
        r.level_db = -90.0;
        assert!((rank(&r) - (0.8 + 0.1)).abs() < 1e-6);
    }

    #[test]
    fn une_seule_oreille_elue_a_la_fin_de_la_fenetre() {
        let mut a = arbiter();
        let t0 = Instant::now();
        assert_eq!(a.wake(report("a", 1, 0.9), t0), vec![ArbiterAction::Duck]);
        assert_eq!(a.deadline(), Some(t0 + DEFAULT_WINDOW));
        assert!(a.wake(report("b", 7, 0.5), t0 + ms(100)).is_empty());
        // Avant la fin : rien.
        assert!(a.tick(t0 + ms(299)).is_empty());
        assert_eq!(
            a.tick(t0 + DEFAULT_WINDOW),
            vec![elect("a", 1), stop("b", 7)]
        );
        assert_eq!(a.deadline(), None);
        // Décision prise une seule fois.
        assert!(a.tick(t0 + ms(400)).is_empty());
        // Seule à la fin de la fenêtre : élue aussi.
        assert_eq!(decide(vec![report("c", 2, 0.1)]), vec![elect("c", 2)]);
    }

    #[test]
    fn meilleur_score_gagne() {
        assert_eq!(
            decide(vec![report("a", 1, 0.6), report("b", 1, 0.9)]),
            vec![elect("b", 1), stop("a", 1)]
        );
    }

    #[test]
    fn oreille_plus_forte_gagne_a_confiance_egale() {
        let mut fort = report("b", 1, 0.8);
        fort.level_db = -10.0;
        assert_eq!(
            decide(vec![report("a", 1, 0.8), fort]),
            vec![elect("b", 1), stop("a", 1)]
        );
    }

    #[test]
    fn egalite_premiere_arrivee_gagne() {
        assert_eq!(
            decide(vec![report("b", 1, 0.8), report("a", 1, 0.8)]),
            vec![elect("b", 1), stop("a", 1)]
        );
    }

    #[test]
    fn biais_de_l_oreille_compte() {
        let mut favorite = report("b", 1, 0.7);
        favorite.bias = 0.2;
        assert_eq!(
            decide(vec![report("a", 1, 0.8), favorite]),
            vec![elect("b", 1), stop("a", 1)]
        );
    }

    #[test]
    fn oreille_coupee_perd_contre_oreille_ouverte() {
        let mut coupee = report("a", 1, 0.99);
        coupee.muted = true;
        coupee.level_db = 0.0;
        assert_eq!(
            decide(vec![coupee.clone(), report("b", 1, 0.3)]),
            vec![elect("b", 1), stop("a", 1)]
        );
        // Seule, elle gagne (pour entendre « écoute-moi »).
        assert_eq!(decide(vec![coupee]), vec![elect("a", 1)]);
    }

    #[test]
    fn score_invalide_ne_gagne_jamais() {
        let mut nan = report("a", 1, f32::NAN);
        nan.level_db = f32::NAN;
        assert_eq!(
            decide(vec![nan, report("b", 1, 0.1)]),
            vec![elect("b", 1), stop("a", 1)]
        );
    }

    #[test]
    fn reveil_tardif_recoit_stop() {
        let mut a = arbiter();
        let t0 = Instant::now();
        a.wake(report("a", 1, 0.5), t0);
        // Arrivé après la fenêtre, avant le `tick` : la décision tombe d'abord.
        assert_eq!(
            a.wake(report("b", 3, 0.99), t0 + ms(301)),
            vec![elect("a", 1), stop("b", 3)]
        );
    }

    #[test]
    fn reveil_pendant_une_phrase_recoit_stop() {
        let mut a = arbiter();
        let t0 = Instant::now();
        a.wake(report("a", 1, 0.5), t0);
        a.tick(t0 + DEFAULT_WINDOW);
        assert_eq!(
            a.wake(report("b", 2, 0.9), t0 + ms(2000)),
            vec![stop("b", 2)]
        );
        // Pendant le traitement aussi, même de la gagnante.
        assert_eq!(a.ended(&id("a"), 1), EndVerdict::Process);
        assert_eq!(
            a.wake(report("a", 2, 0.9), t0 + ms(4000)),
            vec![stop("a", 2)]
        );
        // Phrase finie : un nouveau groupe s'ouvre.
        a.finished(&id("a"), 1);
        assert_eq!(
            a.wake(report("b", 3, 0.9), t0 + ms(5000)),
            vec![ArbiterAction::Duck]
        );
    }

    #[test]
    fn reveil_en_double_ignore() {
        let mut a = arbiter();
        let t0 = Instant::now();
        a.wake(report("a", 1, 0.5), t0);
        assert!(a.wake(report("a", 1, 0.5), t0 + ms(10)).is_empty());
        // Une seconde phrase de la même oreille dans la fenêtre : arrêtée.
        assert_eq!(a.wake(report("a", 2, 0.9), t0 + ms(20)), vec![stop("a", 2)]);
        assert_eq!(a.tick(t0 + DEFAULT_WINDOW), vec![elect("a", 1)]);
        assert!(a.wake(report("a", 1, 0.5), t0 + ms(400)).is_empty());
    }

    #[test]
    fn fin_de_la_perdante_ignoree() {
        let mut a = arbiter();
        let t0 = Instant::now();
        a.wake(report("a", 1, 0.9), t0);
        a.wake(report("b", 1, 0.1), t0);
        a.tick(t0 + DEFAULT_WINDOW);
        assert_eq!(a.ended(&id("b"), 1), EndVerdict::Ignore);
        assert_eq!(a.ended(&id("a"), 1), EndVerdict::Process);
        // Phrase inconnue, ou aucun groupe.
        assert_eq!(a.ended(&id("a"), 9), EndVerdict::Ignore);
        a.finished(&id("a"), 1);
        assert_eq!(a.ended(&id("a"), 1), EndVerdict::Ignore);
    }

    #[test]
    fn fin_avant_decision_attend() {
        let mut a = arbiter();
        let t0 = Instant::now();
        a.wake(report("a", 1, 0.9), t0);
        a.wake(report("b", 1, 0.1), t0);
        assert_eq!(a.ended(&id("a"), 1), EndVerdict::Wait);
        assert_eq!(a.ended(&id("b"), 1), EndVerdict::Wait);
        assert_eq!(a.ended(&id("c"), 1), EndVerdict::Ignore);
        a.tick(t0 + DEFAULT_WINDOW);
        assert_eq!(a.ended(&id("a"), 1), EndVerdict::Process);
        assert_eq!(a.ended(&id("b"), 1), EndVerdict::Ignore);
    }

    #[test]
    fn deconnexion_de_la_gagnante_retablit_le_son() {
        let mut a = arbiter();
        let t0 = Instant::now();
        a.wake(report("a", 1, 0.9), t0);
        a.wake(report("b", 1, 0.1), t0);
        a.tick(t0 + DEFAULT_WINDOW);
        // Perdante partie : rien.
        assert!(a.disconnected(&id("b")).is_empty());
        assert_eq!(a.disconnected(&id("a")), vec![ArbiterAction::Restore]);
        // Groupe fermé : sa fin est ignorée, un nouveau réveil ouvre un groupe.
        assert_eq!(a.ended(&id("a"), 1), EndVerdict::Ignore);
        assert_eq!(
            a.wake(report("b", 2, 0.5), t0 + ms(1000)),
            vec![ArbiterAction::Duck]
        );
    }

    #[test]
    fn deconnexion_pendant_la_fenetre() {
        let mut a = arbiter();
        let t0 = Instant::now();
        a.wake(report("a", 1, 0.9), t0);
        a.wake(report("b", 1, 0.1), t0);
        assert!(a.disconnected(&id("a")).is_empty());
        assert_eq!(a.tick(t0 + DEFAULT_WINDOW), vec![elect("b", 1)]);
        // Toutes parties avant la décision : son rétabli, groupe fermé.
        let mut a = arbiter();
        a.wake(report("a", 1, 0.9), t0);
        assert_eq!(a.disconnected(&id("a")), vec![ArbiterAction::Restore]);
        assert_eq!(a.deadline(), None);
        assert!(a.tick(t0 + DEFAULT_WINDOW).is_empty());
    }

    #[test]
    fn deconnexion_pendant_le_traitement_garde_la_phrase() {
        let mut a = arbiter();
        let t0 = Instant::now();
        a.wake(report("a", 1, 0.9), t0);
        a.tick(t0 + DEFAULT_WINDOW);
        assert_eq!(a.ended(&id("a"), 1), EndVerdict::Process);
        // `listen` rétablit déjà le son ; la phrase reste en cours jusqu'à `finished`.
        assert!(a.disconnected(&id("a")).is_empty());
        assert_eq!(
            a.wake(report("b", 1, 0.9), t0 + ms(1000)),
            vec![stop("b", 1)]
        );
        a.finished(&id("a"), 1);
        assert_eq!(
            a.wake(report("b", 2, 0.9), t0 + ms(2000)),
            vec![ArbiterAction::Duck]
        );
    }

    #[test]
    fn phrase_abandonnee_retablit_le_son() {
        let mut a = arbiter();
        let t0 = Instant::now();
        a.wake(report("a", 1, 0.9), t0);
        a.wake(report("b", 1, 0.1), t0);
        a.tick(t0 + DEFAULT_WINDOW);
        // Autre phrase que la gagnante : rien.
        assert!(a.cancelled(&id("b"), 1).is_empty());
        assert!(a.cancelled(&id("a"), 2).is_empty());
        assert_eq!(a.cancelled(&id("a"), 1), vec![ArbiterAction::Restore]);
        assert_eq!(a.ended(&id("a"), 1), EndVerdict::Ignore);
        assert!(a.follow_up(&id("b"), 5));
    }

    #[test]
    fn baisse_du_son_une_seule_fois_par_phrase() {
        let mut a = arbiter();
        let t0 = Instant::now();
        assert_eq!(a.wake(report("a", 1, 0.9), t0), vec![ArbiterAction::Duck]);
        assert!(a.wake(report("b", 1, 0.9), t0 + ms(50)).is_empty());
        assert!(a.wake(report("c", 1, 0.9), t0 + ms(60)).is_empty());
        // Phrase suivante : de nouveau.
        a.tick(t0 + DEFAULT_WINDOW);
        a.ended(&id("a"), 1);
        a.finished(&id("a"), 1);
        assert_eq!(
            a.wake(report("b", 2, 0.9), t0 + ms(3000)),
            vec![ArbiterAction::Duck]
        );
    }

    #[test]
    fn pas_de_baisse_si_l_oreille_ne_le_demande_pas() {
        let mut a = arbiter();
        let t0 = Instant::now();
        let mut loin = report("a", 1, 0.9);
        loin.duck = false;
        assert!(a.wake(loin.clone(), t0).is_empty());
        // Une oreille du groupe qui le demande : baisse, une fois.
        assert_eq!(
            a.wake(report("b", 1, 0.5), t0 + ms(10)),
            vec![ArbiterAction::Duck]
        );
        assert!(a.wake(report("c", 1, 0.5), t0 + ms(20)).is_empty());
        // Réveil tardif : jamais de baisse.
        let mut a = arbiter();
        a.wake(loin, t0);
        assert_eq!(
            a.wake(report("b", 1, 0.9), t0 + ms(400)),
            vec![elect("a", 1), stop("b", 1)]
        );
    }

    #[test]
    fn reponse_sans_mot_d_eveil_acceptee_hors_phrase() {
        let mut a = arbiter();
        let t0 = Instant::now();
        assert!(a.follow_up(&id("a"), 4));
        // Elle devient la phrase en cours.
        assert_eq!(a.ended(&id("a"), 4), EndVerdict::Process);
        assert_eq!(a.wake(report("b", 1, 0.9), t0), vec![stop("b", 1)]);
        assert!(!a.follow_up(&id("b"), 2));
        a.finished(&id("a"), 4);
        // Refusée pendant une fenêtre ouverte, puis pendant la phrase élue.
        a.wake(report("b", 3, 0.9), t0 + ms(1000));
        assert!(!a.follow_up(&id("a"), 5));
        a.tick(t0 + ms(1300));
        assert!(!a.follow_up(&id("a"), 5));
        a.finished(&id("b"), 3);
        assert!(a.follow_up(&id("a"), 5));
        // Perdue en route : son rétabli.
        assert_eq!(a.disconnected(&id("a")), vec![ArbiterAction::Restore]);
    }
}
