//! Outils de texte partagés par les compétences.

/// Lettre sans accent (même table que les raccourcis du cerveau).
fn strip_accent(c: char) -> char {
    match c {
        'é' | 'è' | 'ê' | 'ë' => 'e',
        'à' | 'â' | 'ä' => 'a',
        'ç' => 'c',
        'ô' | 'ö' => 'o',
        'î' | 'ï' => 'i',
        'ù' | 'û' | 'ü' => 'u',
        other => other,
    }
}

/// Forme de comparaison : minuscules, accents retirés, ponctuation en espaces,
/// espaces fusionnés. Ne retire ni politesse ni « jarvis ».
pub fn fold(text: &str) -> String {
    let cleaned: String = text
        .to_lowercase()
        .chars()
        .map(strip_accent)
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect();
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fold_retire_accents_et_ponctuation() {
        assert_eq!(
            fold("  Où en est la TÂCHE, s'il te plaît ?! "),
            "ou en est la tache s il te plait"
        );
        assert_eq!(fold("Corrige l'élément « Ça »."), "corrige l element ca");
        assert_eq!(fold(""), "");
        assert_eq!(fold(" ,;! "), "");
    }

    #[test]
    fn fold_garde_jarvis() {
        assert_eq!(
            fold("Jarvis, finis les tests du projet Jarvis."),
            "jarvis finis les tests du projet jarvis"
        );
    }
}
