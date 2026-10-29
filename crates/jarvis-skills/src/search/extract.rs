//! Réponse tirée des résultats seuls, sans IA : le repli du résumé.

use super::SearchPage;

/// Réponse courte tirée de la page. `None` : la page est vide.
///
/// Ordre : réponse directe, première phrase de l'encadré, première phrase de
/// l'extrait du premier résultat, puis son titre.
pub fn extract_answer(page: &SearchPage) -> Option<String> {
    fn filled(t: &Option<String>) -> Option<&str> {
        t.as_deref().map(str::trim).filter(|t| !t.is_empty())
    }
    if let Some(answer) = filled(&page.answer) {
        return Some(answer.to_string());
    }
    if let Some(infobox) = filled(&page.infobox) {
        return Some(first_sentence(infobox).to_string());
    }
    let hit = page.hits.first()?;
    let sentence = first_sentence(&hit.snippet).trim_end_matches(['.', '…', ' ', ',', ';', ':']);
    if sentence.is_empty() {
        return Some(hit.title.clone());
    }
    Some(format!("D'après {} : {sentence}.", hit.site))
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
        assert_eq!(extract_answer(&page).as_deref(), Some("Il fera beau."));
        page.answer = Some("  ".into());
        assert_eq!(
            extract_answer(&page).as_deref(),
            Some("Lomé est la capitale du Togo.")
        );
        page.infobox = None;
        assert_eq!(
            extract_answer(&page).as_deref(),
            Some("D'après meteo : Demain, 31 degrés au soleil.")
        );
        page.hits[0].snippet = " ".into();
        assert_eq!(extract_answer(&page).as_deref(), Some("Météo Lomé"));
        page.hits.clear();
        assert_eq!(extract_answer(&page), None);
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
