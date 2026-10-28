//! Raccourcis rapides : phrases courantes reconnues sans IA.

use crate::intent::Intent;
use jarvis_protocol::EarAction;
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

/// Formules de politesse retirées en tête de phrase (mots normalisés).
const PREFIXES: &[&[&str]] = &[
    &["jarvis"],
    &["s", "il", "te", "plait"],
    &["s", "il", "vous", "plait"],
    &["stp"],
    &["svp"],
    &["merci"],
    &["est", "ce", "que", "tu", "peux"],
    &["est", "ce", "que", "vous", "pouvez"],
    &["tu", "peux"],
    &["vous", "pouvez"],
    &["peux", "tu"],
    &["pouvez", "vous"],
    &["pourrais", "tu"],
    &["pourriez", "vous"],
    &["vas", "y"],
    &["allez", "y"],
    &["allez"],
];

/// Formules de politesse retirées en fin de phrase.
const SUFFIXES: &[&[&str]] = &[
    &["jarvis"],
    &["s", "il", "te", "plait"],
    &["s", "il", "vous", "plait"],
    &["stp"],
    &["svp"],
    &["merci"],
    &["vas", "y"],
    &["allez", "y"],
];

/// Infinitif ou vouvoiement en tête de phrase vers l'impératif tutoyé
/// (« mettre pause » → « mets pause »).
fn imperatif(verbe: &str) -> &str {
    match verbe {
        "mettre" => "mets",
        "remettre" => "remets",
        "lancer" => "lance",
        "relancer" => "relance",
        "ouvrir" => "ouvre",
        "demarrer" => "demarre",
        "arreter" => "arrete",
        "stopper" => "stoppe",
        "couper" => "coupe",
        "monter" => "monte",
        "baisser" => "baisse",
        "augmenter" => "augmente",
        "diminuer" => "diminue",
        "passer" => "passe",
        "zapper" => "zappe",
        "reprendre" | "reprenez" => "reprends",
        "continuer" | "continuez" => "continue",
        "jouer" => "joue",
        "eteindre" | "eteignez" => "eteins",
        "redemarrer" | "redemarrez" => "redemarre",
        "annuler" | "annulez" => "annule",
        "repeter" | "repetez" => "repete",
        other => other,
    }
}

fn strip_politesse(mut words: Vec<&str>) -> Vec<&str> {
    loop {
        let before = words.len();
        if let Some(p) = PREFIXES.iter().find(|p| words.starts_with(p)) {
            words.drain(..p.len());
        }
        if let Some(s) = SUFFIXES.iter().find(|s| words.ends_with(s)) {
            words.truncate(words.len() - s.len());
        }
        if words.len() == before {
            return words;
        }
    }
}

/// Minuscules, accents retirés, ponctuation en espaces.
fn clean(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .map(strip_accent)
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect()
}

/// Minuscules, accents retirés, ponctuation en espaces, espaces fusionnés,
/// politesses retirées (début et fin), verbe initial ramené à l'impératif.
pub fn normalize(text: &str) -> String {
    let cleaned = clean(text);
    let mut words = strip_politesse(cleaned.split_whitespace().collect());
    if let Some(first) = words.first_mut() {
        *first = imperatif(first);
    }
    words.join(" ")
}

/// « chanson suivante », « morceau précédent »… : nom de média + adjectif.
fn piste(phrase: &str) -> Option<Intent> {
    let (nom, adj) = phrase.split_once(' ')?;
    if !matches!(
        nom,
        "chanson" | "musique" | "piste" | "morceau" | "titre" | "video"
    ) {
        return None;
    }
    match adj {
        "suivant" | "suivante" => Some(Intent::new("media_next", json!({}))),
        "precedent" | "precedente" => Some(Intent::new("media_previous", json!({}))),
        _ => None,
    }
}

fn exact(phrase: &str) -> Option<Intent> {
    let simple = |tool: &str| Some(Intent::new(tool, json!({})));
    let mode = |m: &str| Some(Intent::new("set_mode", json!({ "mode": m })));
    match phrase {
        // media_play
        "reprends" | "reprend" | "continue" | "relance" | "relance la musique"
        | "remets la musique" | "remets en route" | "lecture" | "play" | "joue"
        | "lance la lecture" | "lance la musique" | "mets la musique" => simple("media_play"),
        // media_pause
        "pause"
        | "mets pause"
        | "mets en pause"
        | "met en pause"
        | "stop"
        | "stoppe"
        | "arrete"
        | "arrete la musique"
        | "coupe la musique"
        | "mets la musique en pause" => simple("media_pause"),
        // media_next
        "suivant"
        | "suivante"
        | "la suivante"
        | "passe"
        | "passe a la suivante"
        | "passe au suivant"
        | "next"
        | "skip"
        | "zappe" => simple("media_next"),
        // media_previous
        "precedent"
        | "precedente"
        | "la precedente"
        | "reviens en arriere"
        | "retour"
        | "remets la precedente" => simple("media_previous"),
        // volume_up
        "plus fort" | "mets plus fort" | "plus de son" | "monte le son" | "monte le volume"
        | "augmente le son" | "augmente le volume" => simple("volume_up"),
        // volume_down
        "moins fort" | "mets moins fort" | "moins de son" | "baisse le son"
        | "baisse le volume" | "diminue le son" | "diminue le volume" => simple("volume_down"),
        // media_now_playing
        "qu est ce qui passe"
        | "c est quoi cette chanson"
        | "c est quoi ce morceau"
        | "quelle est cette chanson"
        | "qu est ce qu on ecoute" => simple("media_now_playing"),
        // set_mode
        "passe en mode local" | "mode local" => mode("local"),
        "passe en mode en ligne" | "mode en ligne" | "passe en ligne" => mode("online"),
        // status
        "qu est ce que tu fais" | "statut" => simple("status"),
        // alimentation (toujours confirmée)
        "mets le pc en veille" | "mets l ordinateur en veille" | "mets en veille" => {
            simple("suspend")
        }
        "redemarre le pc" | "redemarre l ordinateur" => simple("reboot"),
        "eteins le pc" | "eteins l ordinateur" => simple("shutdown"),
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
    exact(&phrase)
        .or_else(|| piste(&phrase))
        .or_else(|| launch(&phrase))
}

/// Commandes traitées par le cerveau lui-même.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrainCommand {
    /// Efface l'action en attente de confirmation.
    Cancel,
    /// Redit la dernière réponse, sans rien réexécuter.
    Repeat,
}

pub fn match_brain_command(text: &str) -> Option<BrainCommand> {
    match normalize(text).as_str() {
        "annule" | "annule ca" | "annule tout" | "laisse tomber" => Some(BrainCommand::Cancel),
        "repete" | "repete ca" | "redis le" | "qu est ce que tu as dit" | "tu as dit quoi" => {
            Some(BrainCommand::Repeat)
        }
        _ => None,
    }
}

/// Phrase reconnue sans IA (raccourci ou commande du cerveau).
pub fn is_shortcut(text: &str) -> bool {
    match_fast(text).is_some() || match_brain_command(text).is_some()
}

/// Réponse à une question de confirmation : `Some(true)` pour oui, `Some(false)` pour
/// non, `None` pour toute autre phrase.
pub fn match_confirmation(text: &str) -> Option<bool> {
    let cleaned = clean(text);
    let mut words: Vec<&str> = cleaned.split_whitespace().collect();
    // Pas `strip_politesse` : « vas-y » et « allez-y » sont ici des réponses.
    loop {
        let before = words.len();
        if words.first() == Some(&"jarvis") {
            words.remove(0);
        }
        if let Some(s) = SUFFIXES
            .iter()
            .filter(|s| !matches!(s.first(), Some(&"vas" | &"allez")))
            .find(|s| words.ends_with(s))
        {
            words.truncate(words.len() - s.len());
        }
        if words.len() == before {
            break;
        }
    }
    match words.join(" ").as_str() {
        "oui" | "ouais" | "oui oui" | "ok" | "okay" | "vas y" | "allez y" | "oui vas y"
        | "oui allez y" | "confirme" | "je confirme" | "oui je confirme" | "oui confirme"
        | "d accord" | "oui d accord" | "fais le" | "faites le" | "oui fais le" => Some(true),
        "non" | "non non" | "annule" | "non annule" | "laisse tomber" | "non laisse tomber"
        | "surtout pas" | "non surtout pas" | "pas maintenant" | "non pas maintenant" => {
            Some(false)
        }
        _ => None,
    }
}

/// Commandes d'écoute (couper ou réactiver le micro), après `normalize`.
pub fn match_ear_command(text: &str) -> Option<EarAction> {
    match normalize(text).as_str() {
        "coupe l ecoute" | "coupe le micro" | "arrete d ecouter" | "mute" => Some(EarAction::Mute),
        "reactive l ecoute" | "reactive le micro" | "reprends l ecoute" | "ecoute a nouveau" => {
            Some(EarAction::Unmute)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commandes_ecoute_reconnues() {
        for t in [
            "coupe l'écoute",
            "Coupe le micro !",
            "arrête d'écouter",
            "mute",
            "Jarvis, coupe l'écoute.",
        ] {
            assert_eq!(match_ear_command(t), Some(EarAction::Mute), "{t}");
        }
        for t in [
            "réactive l'écoute",
            "réactive le micro",
            "reprends l'écoute",
            "écoute à nouveau",
            "Jarvis, réactive l'écoute s'il te plaît",
        ] {
            assert_eq!(match_ear_command(t), Some(EarAction::Unmute), "{t}");
        }
        assert_eq!(match_ear_command("chanson suivante"), None);
        assert_eq!(match_ear_command(""), None);
    }
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

    fn verifie(tool: &str, phrases: &[&str]) {
        for phrase in phrases {
            assert_eq!(
                match_fast(phrase),
                intent(tool, json!({})),
                "phrase: {phrase}"
            );
        }
    }

    #[test]
    fn variantes_media_play() {
        verifie(
            "media_play",
            &[
                "reprendre",
                "reprends",
                "reprenez",
                "reprend",
                "continue",
                "continuer",
                "continuez",
                "relance",
                "relancer",
                "relance la musique",
                "remets la musique",
                "lecture",
                "play",
                "joue",
                "lance la lecture",
                "remets en route",
            ],
        );
    }

    #[test]
    fn variantes_media_pause() {
        verifie(
            "media_pause",
            &[
                "pause",
                "mets pause",
                "mets en pause",
                "met en pause",
                "mettre en pause",
                "stop",
                "stoppe",
                "arrête",
                "arrêter",
                "arrête la musique",
                "coupe la musique",
                "mets la musique en pause",
            ],
        );
    }

    #[test]
    fn variantes_media_next() {
        verifie(
            "media_next",
            &[
                "suivant",
                "suivante",
                "chanson suivante",
                "musique suivante",
                "piste suivante",
                "morceau suivant",
                "titre suivant",
                "vidéo suivante",
                "passe",
                "passe à la suivante",
                "passe au suivant",
                "la suivante",
                "next",
                "skip",
                "zappe",
            ],
        );
    }

    #[test]
    fn variantes_media_previous() {
        verifie(
            "media_previous",
            &[
                "précédent",
                "précédente",
                "chanson précédente",
                "musique précédente",
                "piste précédente",
                "morceau précédent",
                "titre précédent",
                "vidéo précédente",
                "reviens en arrière",
                "retour",
                "la précédente",
                "remets la précédente",
            ],
        );
    }

    #[test]
    fn variantes_volume() {
        verifie(
            "volume_up",
            &[
                "plus fort",
                "monte le son",
                "monte le volume",
                "monter le volume",
                "augmente le son",
                "augmente le volume",
                "mets plus fort",
                "plus de son",
            ],
        );
        verifie(
            "volume_down",
            &[
                "moins fort",
                "baisse le son",
                "baisse le volume",
                "baisser le son",
                "diminue le son",
                "diminue le volume",
                "moins de son",
                "mets moins fort",
            ],
        );
    }

    #[test]
    fn variantes_now_playing() {
        verifie(
            "media_now_playing",
            &[
                "qu'est-ce qui passe",
                "c'est quoi cette chanson",
                "c'est quoi ce morceau",
                "quelle est cette chanson",
                "qu'est-ce qu'on écoute",
            ],
        );
    }

    #[test]
    fn politesse_retiree() {
        assert_eq!(
            match_fast("Jarvis, tu peux reprendre s'il te plaît"),
            intent("media_play", json!({}))
        );
        assert_eq!(
            match_fast("peux-tu mettre pause merci"),
            intent("media_pause", json!({}))
        );
        assert_eq!(
            match_fast("est-ce que tu peux lancer spotify"),
            intent("app_launch", json!({"name": "spotify"}))
        );
        assert_eq!(
            match_fast("tu peux ouvrir firefox s'il te plaît"),
            intent("app_launch", json!({"name": "firefox"}))
        );
        assert_eq!(
            match_fast("merci jarvis vas-y passe à la suivante stp"),
            intent("media_next", json!({}))
        );
    }

    #[test]
    fn micro_n_est_pas_pause() {
        for phrase in ["coupe le micro", "coupe l'écoute", "arrête d'écouter"] {
            assert_eq!(match_fast(phrase), None, "phrase: {phrase}");
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
            intent("app_launch", json!({"name": "spotify"}))
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
    fn raccourcis_alimentation() {
        verifie(
            "suspend",
            &[
                "mets le PC en veille",
                "Jarvis, mets le PC en veille.",
                "mettre l'ordinateur en veille",
                "mets en veille",
            ],
        );
        verifie(
            "reboot",
            &[
                "redémarre le PC",
                "redémarrer l'ordinateur",
                "redémarrez le PC",
            ],
        );
        verifie(
            "shutdown",
            &["éteins le PC", "éteindre l'ordinateur", "éteignez le PC"],
        );
        // « redémarre » n'est pas « démarre » (lancer une appli).
        assert_eq!(match_fast("redémarre spotify"), None);
    }

    #[test]
    fn commandes_du_cerveau() {
        for t in [
            "annule",
            "Jarvis, annule.",
            "annule ça",
            "laisse tomber",
            "annuler",
        ] {
            assert_eq!(match_brain_command(t), Some(BrainCommand::Cancel), "{t}");
        }
        for t in [
            "répète",
            "Tu peux répéter ?",
            "répète s'il te plaît",
            "répéter",
        ] {
            assert_eq!(match_brain_command(t), Some(BrainCommand::Repeat), "{t}");
        }
        assert_eq!(match_brain_command("suivant"), None);
        assert!(is_shortcut("annule") && is_shortcut("suivant"));
        assert!(!is_shortcut("quel temps fait-il"));
    }

    #[test]
    fn reponses_de_confirmation() {
        for t in [
            "oui",
            "Oui.",
            "Oui !",
            "vas-y",
            "Vas-y.",
            "allez-y",
            "oui, vas-y",
            "confirme",
            "je confirme",
            "d'accord",
            "fais-le",
            "oui merci",
            "Jarvis, oui",
            "oui s'il te plaît",
        ] {
            assert_eq!(match_confirmation(t), Some(true), "{t}");
        }
        for t in [
            "non",
            "Non.",
            "non merci",
            "annule",
            "laisse tomber",
            "non, annule",
        ] {
            assert_eq!(match_confirmation(t), Some(false), "{t}");
        }
        for t in [
            "",
            "merci",
            "pas d'accord",
            "mets pause",
            "oui mais non",
            "jarvis",
        ] {
            assert_eq!(match_confirmation(t), None, "{t}");
        }
    }

    #[test]
    fn phrase_libre_non_reconnue() {
        assert_eq!(match_fast("quel temps fait-il demain"), None);
        assert_eq!(match_fast("lance"), None);
        assert_eq!(match_fast(""), None);
        assert_eq!(match_fast("jarvis"), None);
    }
}
