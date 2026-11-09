//! Réponse tirée des résultats seuls, sans IA : le repli du résumé.

use super::SearchPage;
use crate::text::fold;

/// Résultats regardés pour choisir l'extrait.
const EXTRACT_HITS: usize = 5;

/// Mots trop courants pour dire si une phrase répond à la question.
const COMMON_WORDS: [&str; 24] = [
    "les",
    "des",
    "une",
    "est",
    "sur",
    "par",
    "pour",
    "dans",
    "avec",
    "qui",
    "que",
    "quoi",
    "quel",
    "quelle",
    "quels",
    "quelles",
    "cherche",
    "recherche",
    "trouve",
    "moi",
    "infos",
    "internet",
    "web",
    "ligne",
];

/// Réponse courte tirée de la page. `None` : la page est vide.
///
/// Ordre : réponse directe, première phrase de l'encadré, puis, parmi les
/// extraits des premiers résultats, la phrase qui reprend le plus de mots de la
/// question (à égalité, la plus haute dans la page). Si aucune ne reprend un
/// mot de la question : la première phrase du premier extrait, puis son titre.
pub fn extract_answer(query: &str, page: &SearchPage) -> Option<String> {
    fn filled(t: &Option<String>) -> Option<&str> {
        t.as_deref().map(str::trim).filter(|t| !t.is_empty())
    }
    if let Some(answer) = filled(&page.answer) {
        return Some(answer.to_string());
    }
    if let Some(infobox) = filled(&page.infobox) {
        return Some(first_sentence(infobox).to_string());
    }
    let first = page.hits.first()?;
    let words = key_words(query);
    let mut best: Option<(usize, &str, &str)> = None;
    for hit in page.hits.iter().take(EXTRACT_HITS) {
        for sentence in sentences(&hit.snippet) {
            let score = overlap(&words, sentence);
            if score > 0 && best.is_none_or(|(b, _, _)| score > b) {
                best = Some((score, hit.site.as_str(), sentence));
            }
        }
    }
    let (site, sentence) = match best {
        Some((_, site, sentence)) => (site, sentence),
        None => (first.site.as_str(), first_sentence(&first.snippet)),
    };
    let sentence = sentence.trim_end_matches(['.', '…', ' ', ',', ';', ':']);
    if sentence.is_empty() {
        return Some(first.title.clone());
    }
    Some(format!("D'après {site} : {sentence}."))
}

/// Mots de la question qui comptent : trois lettres au moins, hors mots courants.
fn key_words(query: &str) -> Vec<String> {
    let mut words: Vec<String> = fold(query)
        .split(' ')
        .filter(|w| w.chars().count() >= 3 && !COMMON_WORDS.contains(w))
        .map(str::to_string)
        .collect();
    words.dedup();
    words
}

/// Nombre de mots de la question présents dans `sentence`.
fn overlap(words: &[String], sentence: &str) -> usize {
    let folded = fold(sentence);
    let present: Vec<&str> = folded.split(' ').collect();
    words
        .iter()
        .filter(|w| present.contains(&w.as_str()))
        .count()
}

/// Les phrases de `text`, dans l'ordre (coupées comme `first_sentence`).
fn sentences(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = text.trim();
    while !rest.is_empty() {
        let sentence = first_sentence(rest);
        if sentence.is_empty() {
            break;
        }
        out.push(sentence);
        let start = rest.find(sentence).unwrap_or(0) + sentence.len();
        rest = rest[start..].trim_start();
    }
    out
}

/// Première phrase de `text`, ou tout le texte s'il n'y a pas de fin de phrase.
///
/// Une fin de phrase est suivie d'un espace puis d'autre chose qu'une minuscule
/// ou un chiffre : « 3.5 » et « 29 oct. 2026 » ne coupent pas.
pub fn first_sentence(text: &str) -> &str {
    let text = text.trim();
    for (i, c) in text.char_indices() {
        if !matches!(c, '.' | '!' | '?' | '…') {
            continue;
        }
        let end = i + c.len_utf8();
        let rest = &text[end..];
        let next = rest.trim_start().chars().next();
        let spaced = rest.starts_with(char::is_whitespace);
        let ends = match next {
            None => true,
            Some(n) => spaced && !n.is_lowercase() && !n.is_ascii_digit(),
        };
        if ends {
            return &text[..end];
        }
    }
    text
}

/// `text` coupé à `max` caractères au plus, sur un mot, suivi de « … ».
pub fn cut_on_word(text: &str, max: usize) -> String {
    let text = text.trim();
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max {
        return text.to_string();
    }
    if max == 0 {
        return String::new();
    }
    // La place pour « … ».
    let room = max - 1;
    let end = if chars[room].is_whitespace() {
        room
    } else {
        chars[..room]
            .iter()
            .rposition(|c| c.is_whitespace())
            .unwrap_or(room)
    };
    let head: String = chars[..end].iter().collect();
    format!("{}…", head.trim_end_matches([' ', ',', ';', ':', '.']))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::SearchHit;

    fn hit(site: &str, title: &str, snippet: &str) -> SearchHit {
        SearchHit {
            title: title.into(),
            url: format!("https://{site}.fr/"),
            snippet: snippet.into(),
            site: site.into(),
        }
    }

    #[test]
    fn extrait_ordre_de_priorite() {
        let mut page = SearchPage {
            hits: vec![hit(
                "meteo",
                "Météo Lomé",
                "Demain, 31 degrés au soleil. Vent faible.",
            )],
            answer: Some("Il fera beau.".into()),
            infobox: Some(
                "Lomé est la capitale du Togo. Elle compte 2 millions d'habitants.".into(),
            ),
        };
        assert_eq!(
            extract_answer("meteo lome", &page).as_deref(),
            Some("Il fera beau.")
        );
        page.answer = Some("  ".into());
        assert_eq!(
            extract_answer("meteo lome", &page).as_deref(),
            Some("Lomé est la capitale du Togo.")
        );
        page.infobox = None;
        assert_eq!(
            extract_answer("meteo lome", &page).as_deref(),
            Some("D'après meteo : Demain, 31 degrés au soleil.")
        );
        page.hits[0].snippet = " ".into();
        assert_eq!(
            extract_answer("meteo lome", &page).as_deref(),
            Some("Météo Lomé")
        );
        page.hits.clear();
        assert_eq!(extract_answer("meteo lome", &page), None);
    }

    #[test]
    fn extrait_qui_repond_a_la_question() {
        // Cas réel (essai 6.7) : le premier extrait parle d'autre chose.
        let page = SearchPage {
            hits: vec![
                hit(
                    "meteofrance",
                    "METEO LOMÉ par Météo-France",
                    "Le changement climatique a-t-il un impact sur la fréquence et l'intensité \
                     des tempêtes ? Et les tornades ?",
                ),
                hit(
                    "meteoart",
                    "Météo demain à Lome",
                    "Il fera chaud demain à Lome. La température pourra atteindre 28°C.",
                ),
                hit(
                    "lachainemeteo",
                    "Meteo Lomé",
                    "Aujourd'hui à Lomé, il fera beau. Les températures varieront.",
                ),
            ],
            answer: None,
            infobox: None,
        };
        assert_eq!(
            extract_answer("la météo de demain à Lomé", &page).as_deref(),
            Some("D'après meteoart : Il fera chaud demain à Lome.")
        );
    }

    #[test]
    fn extrait_sans_mot_de_la_question_reste_le_premier() {
        let page = SearchPage {
            hits: vec![
                hit("a", "Titre A", "Rien à voir. Toujours rien."),
                hit("b", "Titre B", "Autre sujet."),
            ],
            answer: None,
            infobox: None,
        };
        assert_eq!(
            extract_answer("prix du pain à Kara", &page).as_deref(),
            Some("D'après a : Rien à voir.")
        );
    }

    #[test]
    fn mots_de_la_question() {
        assert_eq!(
            key_words("Cherche la météo de demain à Lomé"),
            vec!["meteo", "demain", "lome"]
        );
        assert_eq!(
            sentences("Un. Deux trois ! Quatre"),
            vec!["Un.", "Deux trois !", "Quatre"]
        );
    }

    #[test]
    fn premiere_phrase_sans_fausse_fin() {
        assert_eq!(
            first_sentence("Il fait 3.5 degrés. Puis 4."),
            "Il fait 3.5 degrés."
        );
        // Date abrégée : pas une fin de phrase.
        assert_eq!(
            first_sentence("29 oct. 2026 — Pluie le matin. Soleil ensuite."),
            "29 oct. 2026 — Pluie le matin."
        );
        assert_eq!(first_sentence(" Sans point "), "Sans point");
        assert_eq!(first_sentence("Quoi ? Rien."), "Quoi ?");
    }

    #[test]
    fn extrait_coupe_sur_un_mot() {
        assert_eq!(cut_on_word("court", 10), "court");
        assert_eq!(
            cut_on_word("Les vents seront faibles, toute la journée", 26),
            "Les vents seront faibles…"
        );
        let coupe = cut_on_word("un deux trois", 9);
        assert_eq!(coupe, "un deux…");
        assert!(coupe.chars().count() <= 9);
        assert_eq!(cut_on_word("anticonstitutionnellement", 6), "antic…");
        assert_eq!(cut_on_word("abc", 0), "");
    }
}
