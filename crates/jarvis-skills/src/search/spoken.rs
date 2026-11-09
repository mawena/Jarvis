//! Texte prêt à être dit : sans markdown, sans adresse web, sans émoji, court.

/// Nettoie `text` pour la voix et le coupe à `max_chars` caractères au plus.
///
/// Coupe à la dernière fin de phrase, sinon au dernier mot suivi de « … ».
pub fn for_voice(text: &str, max_chars: usize) -> String {
    let text = text
        .replace("°C", " degrés")
        .replace('°', " degrés")
        .replace('%', " pour cent");
    let clean: String = text
        .split_whitespace()
        .filter_map(without_url)
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .filter(|c| !matches!(c, '*' | '_' | '#' | '`') && !is_emoji(*c))
        .collect();
    let clean = clean.split_whitespace().collect::<Vec<_>>().join(" ");
    cut(&clean, max_chars)
}

/// Le mot sans son adresse web ; `None` s'il n'en reste rien.
fn without_url(word: &str) -> Option<&str> {
    let start = ["http://", "https://", "www."]
        .iter()
        .filter_map(|p| word.find(p))
        .min();
    let kept = match start {
        Some(i) => word[..i].trim_end_matches(['(', '<', '[']),
        None => word,
    };
    (!kept.is_empty()).then_some(kept)
}

fn is_emoji(c: char) -> bool {
    matches!(c as u32,
        0x1F000..=0x1FAFF // émojis, drapeaux, pictogrammes
        | 0x2600..=0x27BF // symboles divers, dingbats
        | 0x2B00..=0x2BFF // étoiles, flèches
        | 0xFE0F | 0x200D) // variante émoji, liant
}

fn cut(text: &str, max_chars: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max_chars {
        return text.to_string();
    }
    // Dernière fin de phrase (suivie d'un espace) qui tient.
    let sentence_end = (0..max_chars).rev().find(|&i| {
        matches!(chars[i], '.' | '!' | '?' | '…') && chars.get(i + 1).is_none_or(|c| *c == ' ')
    });
    if let Some(i) = sentence_end {
        return chars[..=i].iter().collect();
    }
    // Sinon le dernier mot entier, avec la place pour « … ».
    let room = max_chars.saturating_sub(1);
    let end = (1..=room).rev().find(|&i| chars[i] == ' ').unwrap_or(room);
    let head: String = chars[..end].iter().collect();
    format!("{}…", head.trim_end_matches([' ', ',', ';', ':']))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voix_sans_markdown_ni_url() {
        let dit = for_voice(
            "**Demain**, il fera _beau_ à Lomé 🌞☀️ (voir https://meteo.fr/lome) # `ok` \
             [source](https://x.fr) www.exemple.fr",
            300,
        );
        assert_eq!(dit, "Demain, il fera beau à Lomé (voir ok [source]");
        for interdit in ["*", "_", "#", "`", "http", "www", "🌞", "☀"] {
            assert!(!dit.contains(interdit), "{interdit} dans {dit}");
        }
    }

    #[test]
    fn voix_coupee_a_la_phrase() {
        let texte = "Il fera 3.5 fois plus chaud. Les vents seront faibles toute la journée.";
        assert_eq!(for_voice(texte, 40), "Il fera 3.5 fois plus chaud.");
        assert_eq!(for_voice(texte, 300), texte);
        // Pas de fin de phrase : au dernier mot, suivi de « … ».
        let long = "Les vents seront faibles, toute la journée sur la côte";
        let dit = for_voice(long, 26);
        assert_eq!(dit, "Les vents seront faibles…");
        assert!(dit.chars().count() <= 26);
        // Un seul mot trop long : coupé net.
        assert_eq!(for_voice("anticonstitutionnellement", 6), "antic…");
    }

    #[test]
    fn voix_unites_en_toutes_lettres() {
        assert_eq!(
            for_voice("Il fera 31°C, humidité 80 %, ressenti 33°.", 300),
            "Il fera 31 degrés, humidité 80 pour cent, ressenti 33 degrés."
        );
    }
}
