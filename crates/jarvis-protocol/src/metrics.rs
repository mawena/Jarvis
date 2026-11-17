//! Mesures des critères de réussite (spec §9) : une ligne JSON par phrase traitée,
//! écrite par le cerveau dans `~/.local/state/jarvis/mesures.jsonl`, résumée par
//! `jarvisctl mesures`. Ni son ni empreinte : l'heure, l'oreille, la durée, la route,
//! l'issue et le texte (déjà journalisé).

use serde::{Deserialize, Serialize};

/// Chemin compris d'une phrase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Route {
    /// Raccourci du cerveau (« suivant », « lance VS Code »).
    Raccourci,
    /// Raccourci propre à une compétence (agent de code…).
    Competence,
    IaLocale,
    IaEnLigne,
    /// Pas de compréhension (rien entendu, voix ignorée, « répète »…).
    Aucune,
}

impl Route {
    pub const ALL: [Route; 5] = [
        Route::Raccourci,
        Route::Competence,
        Route::IaLocale,
        Route::IaEnLigne,
        Route::Aucune,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Route::Raccourci => "raccourci",
            Route::Competence => "compétence",
            Route::IaLocale => "IA locale",
            Route::IaEnLigne => "IA en ligne",
            Route::Aucune => "aucune",
        }
    }

    /// Seuil de la spec §9 (millisecondes, mesuré sur le 90e centile).
    pub fn target_ms(self) -> Option<u64> {
        match self {
            Route::Raccourci | Route::Competence => Some(2_000),
            Route::IaLocale => Some(6_000),
            Route::IaEnLigne => Some(4_000),
            Route::Aucune => None,
        }
    }
}

/// Issue d'une phrase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// Jarvis a répondu ou agi.
    Comprise,
    /// Réveil sans phrase : fausse détection probable du mot d'éveil.
    Vide,
    /// Phrase entendue mais écartée sans réponse (autre voix…).
    Ignoree,
    Erreur,
}

/// Une phrase traitée.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhraseRecord {
    /// Heure de fin de traitement (millisecondes Unix).
    pub at_ms: u64,
    /// Nom de l'oreille.
    pub ear: String,
    /// Fin de la phrase → réponse prête à jouer (millisecondes).
    pub ms: u64,
    pub route: Route,
    pub outcome: Outcome,
    #[serde(default)]
    pub text: String,
}

/// Lignes lisibles d'un fichier de mesures (les lignes abîmées sont sautées).
pub fn parse_lines(text: &str) -> Vec<PhraseRecord> {
    text.lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

/// Durées d'une route.
#[derive(Debug, Clone, PartialEq)]
pub struct RouteStats {
    pub route: Route,
    pub count: usize,
    pub median_ms: u64,
    pub p90_ms: u64,
}

impl RouteStats {
    /// Seuil tenu (90e centile sous le seuil) ; `None` sans seuil.
    pub fn met(&self) -> Option<bool> {
        self.route.target_ms().map(|t| self.p90_ms < t)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Summary {
    pub phrases: usize,
    /// Durée couverte, en jours (au moins 1).
    pub days: f64,
    /// Routes vues, phrases comprises seulement, dans l'ordre de [`Route::ALL`].
    pub routes: Vec<RouteStats>,
    pub empty: usize,
    pub ignored: usize,
    pub errors: usize,
    /// Même phrase comprise sur deux oreilles à moins de [`DOUBLE_WINDOW_MS`].
    pub doubles: usize,
}

impl Summary {
    /// Fausses détections probables par jour (réveils sans phrase).
    pub fn false_wakes_per_day(&self) -> f64 {
        self.empty as f64 / self.days
    }
}

/// Fenêtre d'une double exécution.
pub const DOUBLE_WINDOW_MS: u64 = 3_000;

/// Centile `p` (0 à 100), rang le plus proche ; liste triée non vide.
fn percentile(sorted: &[u64], p: usize) -> u64 {
    let rank = (p * sorted.len()).div_ceil(100).max(1);
    sorted[rank - 1]
}

fn same_text(a: &str, b: &str) -> bool {
    let norm = |s: &str| s.trim().to_lowercase();
    !a.trim().is_empty() && norm(a) == norm(b)
}

pub fn summarize(records: &[PhraseRecord]) -> Summary {
    let first = records.iter().map(|r| r.at_ms).min().unwrap_or(0);
    let last = records.iter().map(|r| r.at_ms).max().unwrap_or(0);
    let days = ((last - first) as f64 / 86_400_000.0).max(1.0);
    let count = |o: Outcome| records.iter().filter(|r| r.outcome == o).count();
    let understood: Vec<&PhraseRecord> = records
        .iter()
        .filter(|r| r.outcome == Outcome::Comprise)
        .collect();
    let routes = Route::ALL
        .into_iter()
        .filter_map(|route| {
            let mut ms: Vec<u64> = understood
                .iter()
                .filter(|r| r.route == route)
                .map(|r| r.ms)
                .collect();
            if ms.is_empty() {
                return None;
            }
            ms.sort_unstable();
            Some(RouteStats {
                route,
                count: ms.len(),
                median_ms: percentile(&ms, 50),
                p90_ms: percentile(&ms, 90),
            })
        })
        .collect();
    let mut by_time = understood.clone();
    by_time.sort_by_key(|r| r.at_ms);
    let mut paired = vec![false; by_time.len()];
    let mut doubles = 0;
    for i in 0..by_time.len() {
        if paired[i] {
            continue;
        }
        for j in i + 1..by_time.len() {
            if by_time[j].at_ms - by_time[i].at_ms > DOUBLE_WINDOW_MS {
                break;
            }
            if !paired[j]
                && by_time[j].ear != by_time[i].ear
                && same_text(&by_time[i].text, &by_time[j].text)
            {
                paired[i] = true;
                paired[j] = true;
                doubles += 1;
                break;
            }
        }
    }
    Summary {
        phrases: records.len(),
        days,
        routes,
        empty: count(Outcome::Vide),
        ignored: count(Outcome::Ignoree),
        errors: count(Outcome::Erreur),
        doubles,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(
        at_ms: u64,
        ear: &str,
        ms: u64,
        route: Route,
        outcome: Outcome,
        text: &str,
    ) -> PhraseRecord {
        PhraseRecord {
            at_ms,
            ear: ear.into(),
            ms,
            route,
            outcome,
            text: text.into(),
        }
    }

    #[test]
    fn ligne_json_stable() {
        let r = rec(5, "PC", 640, Route::IaEnLigne, Outcome::Comprise, "météo");
        let line = serde_json::to_string(&r).unwrap();
        assert_eq!(
            line,
            r#"{"at_ms":5,"ear":"PC","ms":640,"route":"ia_en_ligne","outcome":"comprise","text":"météo"}"#
        );
        assert_eq!(
            parse_lines(&format!("{line}\nabîmée\n\n{line}")),
            [r.clone(), r]
        );
    }

    #[test]
    fn latences_par_route() {
        let mut records: Vec<PhraseRecord> = (1..=10)
            .map(|i| {
                rec(
                    i * 60_000,
                    "PC",
                    i * 100,
                    Route::Raccourci,
                    Outcome::Comprise,
                    "suivant",
                )
            })
            .collect();
        records.push(rec(
            0,
            "PC",
            7_000,
            Route::IaLocale,
            Outcome::Comprise,
            "météo",
        ));
        // Pas comprise : hors des latences.
        records.push(rec(
            0,
            "PC",
            9_000,
            Route::Raccourci,
            Outcome::Erreur,
            "suivant",
        ));
        let s = summarize(&records);
        assert_eq!(s.routes.len(), 2);
        let fast = &s.routes[0];
        assert_eq!((fast.route, fast.count), (Route::Raccourci, 10));
        assert_eq!((fast.median_ms, fast.p90_ms), (500, 900));
        assert_eq!(fast.met(), Some(true));
        assert_eq!(s.routes[1].route, Route::IaLocale);
        assert_eq!(s.routes[1].met(), Some(false));
        assert_eq!(s.errors, 1);
    }

    #[test]
    fn fausses_detections_par_jour() {
        let day = 86_400_000;
        let records = [
            rec(0, "PC", 100, Route::Aucune, Outcome::Vide, ""),
            rec(day, "PC", 100, Route::Aucune, Outcome::Vide, ""),
            rec(
                2 * day,
                "PC",
                100,
                Route::Aucune,
                Outcome::Ignoree,
                "bonjour",
            ),
            rec(4 * day, "PC", 100, Route::Aucune, Outcome::Vide, ""),
        ];
        let s = summarize(&records);
        assert_eq!((s.empty, s.ignored), (3, 1));
        assert_eq!(s.days, 4.0);
        assert_eq!(s.false_wakes_per_day(), 0.75);
        // Moins d'un jour : compté comme un jour.
        assert_eq!(summarize(&records[..1]).days, 1.0);
    }

    #[test]
    fn doubles_executions() {
        let records = [
            rec(
                10_000,
                "PC",
                500,
                Route::Raccourci,
                Outcome::Comprise,
                "Suivant",
            ),
            rec(
                10_400,
                "Pixel",
                500,
                Route::Raccourci,
                Outcome::Comprise,
                "suivant ",
            ),
            // Même oreille : une phrase redite, pas un doublon.
            rec(
                11_000,
                "PC",
                500,
                Route::Raccourci,
                Outcome::Comprise,
                "suivant",
            ),
            // Trop tard.
            rec(
                20_000,
                "Pixel",
                500,
                Route::Raccourci,
                Outcome::Comprise,
                "suivant",
            ),
            // Vides : jamais des doublons.
            rec(30_000, "PC", 500, Route::Aucune, Outcome::Vide, ""),
            rec(30_100, "Pixel", 500, Route::Aucune, Outcome::Vide, ""),
        ];
        assert_eq!(summarize(&records).doubles, 1);
    }

    #[test]
    fn fichier_vide() {
        let s = summarize(&[]);
        assert_eq!((s.phrases, s.doubles, s.routes.len()), (0, 0, 0));
    }
}
