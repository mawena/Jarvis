//! `jarvisctl mesures` : résumé de `mesures.jsonl` face aux critères de réussite (spec §9).

use crate::paths::Paths;
use anyhow::Result;
use jarvis_protocol::metrics::{PhraseRecord, parse_lines, summarize};
use std::fs;
use std::io::Write;

/// Fichier écrit par le cerveau.
pub const FILE: &str = "mesures.jsonl";

/// Durée en secondes, virgule décimale : `0,6 s`.
fn secs(ms: u64) -> String {
    format!("{:.1} s", ms as f64 / 1000.0).replace('.', ",")
}

fn number(x: f64) -> String {
    format!("{x:.1}").replace('.', ",")
}

fn mark(ok: bool) -> &'static str {
    if ok { "✓" } else { "✗" }
}

/// Mesures lues (ancien fichier puis courant), filtrées.
pub fn load(paths: &Paths, since_ms: Option<u64>, phrase: Option<&str>) -> Vec<PhraseRecord> {
    let current = paths.state_dir.join(FILE);
    let old = current.with_extension("jsonl.1");
    let needle = phrase.map(str::to_lowercase);
    [old, current]
        .iter()
        .filter_map(|p| fs::read_to_string(p).ok())
        .flat_map(|t| parse_lines(&t))
        .filter(|r| since_ms.is_none_or(|s| r.at_ms >= s))
        .filter(|r| {
            needle
                .as_deref()
                .is_none_or(|n| r.text.to_lowercase().contains(n))
        })
        .collect()
}

pub fn report(
    paths: &Paths,
    days: Option<u32>,
    phrase: Option<&str>,
    now_ms: u64,
    out: &mut dyn Write,
) -> Result<i32> {
    let since = days.map(|d| now_ms.saturating_sub(u64::from(d) * 86_400_000));
    let records = load(paths, since, phrase);
    if records.is_empty() {
        writeln!(
            out,
            "Aucune mesure dans {} : le cerveau en écrit une par phrase entendue.",
            paths.state_dir.join(FILE).display()
        )?;
        return Ok(0);
    }
    let s = summarize(&records);
    writeln!(
        out,
        "{} phrase(s) sur {} jour(s){}",
        s.phrases,
        number(s.days),
        phrase
            .map(|p| format!(", contenant « {p} »"))
            .unwrap_or_default()
    )?;
    writeln!(out)?;
    writeln!(
        out,
        "{:<12} {:>8} {:>8} {:>8}  objectif (90 %)",
        "route", "phrases", "médiane", "90 %"
    )?;
    for r in &s.routes {
        let target = match (r.route.target_ms(), r.met()) {
            (Some(t), Some(ok)) => format!("< {}  {}", secs(t), mark(ok)),
            _ => "—".into(),
        };
        writeln!(
            out,
            "{:<12} {:>8} {:>8} {:>8}  {target}",
            r.route.label(),
            r.count,
            secs(r.median_ms),
            secs(r.p90_ms)
        )?;
    }
    writeln!(out)?;
    let per_day = s.false_wakes_per_day();
    writeln!(
        out,
        "Réveils sans phrase (fausses détections probables) : {} ({} par jour)  objectif < 1 par jour  {}",
        s.empty,
        number(per_day),
        mark(per_day < 1.0)
    )?;
    writeln!(
        out,
        "Phrases écartées (autre voix, phrase courte non vérifiée) : {}",
        s.ignored
    )?;
    writeln!(out, "Erreurs de traitement : {}", s.errors)?;
    writeln!(
        out,
        "Doubles exécutions (même phrase sur deux oreilles à moins de 3 s) : {}  objectif 0  {}",
        s.doubles,
        mark(s.doubles == 0)
    )?;
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use jarvis_protocol::metrics::{Outcome, Route};

    fn line(at_ms: u64, ear: &str, ms: u64, route: &str, outcome: &str, text: &str) -> String {
        format!(
            r#"{{"at_ms":{at_ms},"ear":"{ear}","ms":{ms},"route":"{route}","outcome":"{outcome}","text":"{text}"}}"#
        ) + "\n"
    }

    fn paths_with(lines: &str, old: &str) -> (tempfile::TempDir, Paths) {
        let home = tempfile::tempdir().unwrap();
        let paths = Paths::from_home(home.path());
        fs::create_dir_all(&paths.state_dir).unwrap();
        fs::write(paths.state_dir.join(FILE), lines).unwrap();
        if !old.is_empty() {
            fs::write(paths.state_dir.join("mesures.jsonl.1"), old).unwrap();
        }
        (home, paths)
    }

    fn run(paths: &Paths, days: Option<u32>, phrase: Option<&str>, now: u64) -> String {
        let mut out = Vec::new();
        report(paths, days, phrase, now, &mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn sans_fichier() {
        let home = tempfile::tempdir().unwrap();
        let paths = Paths::from_home(home.path());
        assert!(run(&paths, None, None, 0).starts_with("Aucune mesure"));
    }

    #[test]
    fn resume_face_aux_objectifs() {
        let text = line(1_000, "PC", 600, "raccourci", "comprise", "suivant")
            + &line(2_000, "PC", 7_200, "ia_locale", "comprise", "météo")
            + &line(3_000, "PC", 100, "aucune", "vide", "")
            + &line(3_500, "Pixel", 100, "aucune", "ignoree", "")
            + "ligne abîmée\n";
        let old = line(500, "PC", 900, "raccourci", "comprise", "pause");
        let (_h, paths) = paths_with(&text, &old);
        let o = run(&paths, None, None, 10_000);
        assert!(o.starts_with("5 phrase(s) sur 1,0 jour(s)\n"), "{o}");
        assert!(
            o.contains("raccourci           2    0,6 s    0,9 s  < 2,0 s  ✓"),
            "{o}"
        );
        assert!(
            o.contains("IA locale           1    7,2 s    7,2 s  < 6,0 s  ✗"),
            "{o}"
        );
        assert!(o.contains("Réveils sans phrase (fausses détections probables) : 1 (1,0 par jour)  objectif < 1 par jour  ✗"), "{o}");
        assert!(
            o.contains("Phrases écartées (autre voix, phrase courte non vérifiée) : 1"),
            "{o}"
        );
        assert!(o.contains("Doubles exécutions (même phrase sur deux oreilles à moins de 3 s) : 0  objectif 0  ✓"), "{o}");
    }

    #[test]
    fn filtres_jours_et_phrase() {
        let day = 86_400_000;
        let text = line(0, "PC", 600, "raccourci", "comprise", "Suivant")
            + &line(9 * day, "PC", 700, "raccourci", "comprise", "suivant")
            + &line(9 * day, "PC", 800, "raccourci", "comprise", "lance VS Code");
        let (_h, paths) = paths_with(&text, "");
        let recent = load(&paths, Some(8 * day), None);
        assert_eq!(recent.len(), 2);
        let next = load(&paths, None, Some("SUIVANT"));
        assert_eq!(next.len(), 2);
        assert!(
            next.iter()
                .all(|r| r.route == Route::Raccourci && r.outcome == Outcome::Comprise)
        );
        let o = run(&paths, Some(2), Some("suivant"), 10 * day);
        assert!(
            o.starts_with("1 phrase(s) sur 1,0 jour(s), contenant « suivant »"),
            "{o}"
        );
    }
}
