//! Raccourcis rapides : phrases courantes reconnues sans IA.

use crate::intent::Intent;
use serde_json::json;

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

/// Minuscules, accents retirés, ponctuation en espaces, espaces fusionnés,
/// mot de tête `jarvis` retiré.
pub fn normalize(text: &str) -> String {
    let cleaned: String = text
        .to_lowercase()
        .chars()
        .map(strip_accent)
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect();
    let mut words: Vec<&str> = cleaned.split_whitespace().collect();
    if words.first() == Some(&"jarvis") {
        words.remove(0);
    }
    words.join(" ")
}

fn exact(phrase: &str) -> Option<Intent> {
    let simple = |tool: &str| Some(Intent::new(tool, json!({})));
    let mode = |m: &str| Some(Intent::new("set_mode", json!({ "mode": m })));
    match phrase {
        "suivant" | "suivante" | "chanson suivante" | "musique suivante" | "next" => {
            simple("media_next")
        }
        "precedent" | "precedente" | "chanson precedente" | "musique precedente" => {
            simple("media_previous")
        }
        "pause" | "stop" | "mets en pause" => simple("media_pause"),
        "reprends" | "reprend" | "lecture" | "play" | "relance la musique" => simple("media_play"),
        "plus fort" | "monte le son" | "monte le volume" => simple("volume_up"),
        "moins fort" | "baisse le son" | "baisse le volume" => simple("volume_down"),
        "qu est ce qui passe" | "c est quoi cette chanson" => simple("media_now_playing"),
        "passe en mode local" | "mode local" => mode("local"),
        "passe en mode en ligne" | "mode en ligne" | "passe en ligne" => mode("online"),
        "qu est ce que tu fais" | "statut" => simple("status"),
        _ => None,
    }
}

fn launch(phrase: &str) -> Option<Intent> {
    let (verb, rest) = phrase.split_once(' ')?;
    if !matches!(verb, "lance" | "ouvre" | "demarre") || rest.is_empty() {
        return None;
    }
    if rest == "la video"
        || rest == "une video"
        || rest.starts_with("la video ")
        || rest.starts_with("une video ")
    {
        return None;
    }
    let name = rest
        .split(' ')
        .skip_while(|w| {
            matches!(
                *w,
                "moi" | "le" | "la" | "les" | "l" | "un" | "une" | "mon" | "ma" | "mes"
            )
        })
        .collect::<Vec<_>>()
        .join(" ");
    if name.is_empty() {
        return None;
    }
    Some(Intent::new("app_launch", json!({ "name": name })))
}

/// Reconnaît une phrase courante sans passer par l'IA.
pub fn match_fast(text: &str) -> Option<Intent> {
    let phrase = normalize(text);
    exact(&phrase).or_else(|| launch(&phrase))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn intent(tool: &str, args: serde_json::Value) -> Option<Intent> {
        Some(Intent::new(tool, args))
    }

    #[test]
    fn raccourcis_media() {
        let cases: Vec<(&str, &str, serde_json::Value)> = vec![
            ("suivant", "media_next", json!({})),
            ("suivante", "media_next", json!({})),
            ("chanson suivante", "media_next", json!({})),
            ("musique suivante", "media_next", json!({})),
            ("next", "media_next", json!({})),
            ("precedent", "media_previous", json!({})),
            ("precedente", "media_previous", json!({})),
            ("chanson precedente", "media_previous", json!({})),
            ("musique precedente", "media_previous", json!({})),
            ("pause", "media_pause", json!({})),
            ("stop", "media_pause", json!({})),
            ("mets en pause", "media_pause", json!({})),
            ("reprends", "media_play", json!({})),
            ("reprend", "media_play", json!({})),
            ("lecture", "media_play", json!({})),
            ("play", "media_play", json!({})),
            ("relance la musique", "media_play", json!({})),
            ("plus fort", "volume_up", json!({})),
            ("monte le son", "volume_up", json!({})),
            ("monte le volume", "volume_up", json!({})),
            ("moins fort", "volume_down", json!({})),
            ("baisse le son", "volume_down", json!({})),
            ("baisse le volume", "volume_down", json!({})),
            ("qu'est-ce qui passe", "media_now_playing", json!({})),
            ("c'est quoi cette chanson", "media_now_playing", json!({})),
            ("passe en mode local", "set_mode", json!({"mode": "local"})),
            ("mode local", "set_mode", json!({"mode": "local"})),
            (
                "passe en mode en ligne",
                "set_mode",
                json!({"mode": "online"}),
            ),
            ("mode en ligne", "set_mode", json!({"mode": "online"})),
            ("passe en ligne", "set_mode", json!({"mode": "online"})),
            ("qu'est-ce que tu fais", "status", json!({})),
            ("statut", "status", json!({})),
        ];
        for (phrase, tool, args) in cases {
            assert_eq!(match_fast(phrase), intent(tool, args), "phrase: {phrase}");
        }
    }

    #[test]
    fn normalise_et_tolere_le_bruit() {
        for phrase in ["Jarvis, Précédent !", "  jarvis   precedent", "PRÉCÉDENT."] {
            assert_eq!(
                match_fast(phrase),
                intent("media_previous", json!({})),
                "phrase: {phrase}"
            );
        }
        assert_eq!(normalize("Jarvis, Où êtes-vous ?"), "ou etes vous");
    }

    #[test]
    fn lance_extrait_le_nom() {
        assert_eq!(
            match_fast("Lance VS Code"),
            intent("app_launch", json!({"name": "vs code"}))
        );
        assert_eq!(
            match_fast("ouvre spotify s'il te plait"),
            intent("app_launch", json!({"name": "spotify s il te plait"}))
        );
        assert_eq!(
            match_fast("démarre Firefox"),
            intent("app_launch", json!({"name": "firefox"}))
        );
    }

    #[test]
    fn lance_retire_articles_et_pronoms() {
        for (phrase, name) in [
            ("ouvre-moi le navigateur", "navigateur"),
            ("lance le terminal", "terminal"),
            ("ouvre la calculatrice", "calculatrice"),
            ("lance-moi le terminal", "terminal"),
            ("ouvre mon l'éditeur", "editeur"),
        ] {
            assert_eq!(
                match_fast(phrase),
                intent("app_launch", json!({ "name": name })),
                "phrase: {phrase}"
            );
        }
        assert_eq!(match_fast("ouvre le"), None);
        assert_eq!(match_fast("lance moi la"), None);
    }

    #[test]
    fn video_laissee_a_l_ia() {
        assert_eq!(match_fast("lance la vidéo de Squeezie"), None);
        assert_eq!(match_fast("ouvre une video de chats"), None);
        assert_eq!(match_fast("demarre la video"), None);
    }

    #[test]
    fn phrase_libre_non_reconnue() {
        assert_eq!(match_fast("quel temps fait-il demain"), None);
        assert_eq!(match_fast("lance"), None);
        assert_eq!(match_fast(""), None);
        assert_eq!(match_fast("jarvis"), None);
    }
}
