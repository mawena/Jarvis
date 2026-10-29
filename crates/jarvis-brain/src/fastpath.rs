//! Raccourcis rapides : phrases courantes reconnues sans IA.

use crate::intent::Intent;
use jarvis_protocol::EarAction;
use serde_json::json;
use std::ops::Range;

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
        "chercher" => "cherche",
        "rechercher" => "recherche",
        "trouver" => "trouve",
        "regarder" => "regarde",
        "afficher" => "affiche",
        "montrer" => "montre",
        other => other,
    }
}

/// Bornes des mots gardés une fois les politesses retirées (début et fin).
fn strip_politesse(words: &[&str]) -> Range<usize> {
    let (mut debut, mut fin) = (0, words.len());
    loop {
        let avant = (debut, fin);
        if let Some(p) = PREFIXES.iter().find(|p| words[debut..fin].starts_with(p)) {
            debut += p.len();
        }
        if let Some(s) = SUFFIXES.iter().find(|s| words[debut..fin].ends_with(s)) {
            fin -= s.len();
        }
        if (debut, fin) == avant {
            return debut..fin;
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

/// Mots d'origine : suites de caractères alphanumériques (même découpage que
/// `clean`), repérées par leur position dans le texte.
fn mots_d_origine(text: &str) -> Vec<Range<usize>> {
    let mut mots = Vec::new();
    let mut debut = None;
    for (i, c) in text.char_indices() {
        if c.is_alphanumeric() {
            debut.get_or_insert(i);
        } else if let Some(d) = debut.take() {
            mots.push(d..i);
        }
    }
    if let Some(d) = debut {
        mots.push(d..text.len());
    }
    mots
}

/// Phrase analysée : mots normalisés (politesses retirées, verbe initial à
/// l'impératif) et leur place dans le texte d'origine.
struct Analyse<'a> {
    texte: &'a str,
    mots: Vec<String>,
    places: Vec<Range<usize>>,
}

impl<'a> Analyse<'a> {
    fn new(texte: &'a str) -> Self {
        let places = mots_d_origine(texte);
        let mots: Vec<String> = places
            .iter()
            .map(|p| clean(&texte[p.clone()]).split_whitespace().collect())
            .collect();
        let refs: Vec<&str> = mots.iter().map(String::as_str).collect();
        let gardes = strip_politesse(&refs);
        let mut mots = mots[gardes.clone()].to_vec();
        if let Some(first) = mots.first_mut() {
            *first = imperatif(first).to_string();
        }
        Self {
            texte,
            mots,
            places: places[gardes].to_vec(),
        }
    }

    fn mots(&self) -> Vec<&str> {
        self.mots.iter().map(String::as_str).collect()
    }

    fn normalisee(&self) -> String {
        self.mots.join(" ")
    }

    /// Texte d'origine (accents, majuscules et apostrophes gardés) des mots
    /// `mots`, du premier au dernier ; `None` si la tranche est vide.
    fn origine(&self, mots: Range<usize>) -> Option<&'a str> {
        if mots.is_empty() {
            return None;
        }
        let debut = self.places[mots.start].start;
        let fin = self.places[mots.end - 1].end;
        Some(&self.texte[debut..fin])
    }
}

/// Minuscules, accents retirés, ponctuation en espaces, espaces fusionnés,
/// politesses retirées (début et fin), verbe initial ramené à l'impératif.
pub fn normalize(text: &str) -> String {
    Analyse::new(text).normalisee()
}

/// Indice qui suit `mots[i..]` si l'une des `suites` y commence.
fn apres(mots: &[&str], i: usize, suites: &[&[&str]]) -> Option<usize> {
    let reste = mots.get(i..)?;
    suites
        .iter()
        .find(|s| reste.starts_with(s))
        .map(|s| i + s.len())
}

/// Comme `apres`, mais la suite est facultative.
fn facultatif(mots: &[&str], i: usize, suites: &[&[&str]]) -> usize {
    apres(mots, i, suites).unwrap_or(i)
}

const MOI: &[&[&str]] = &[&["moi"]];

/// Verbes de « lance la vidéo de… » (« cherche la vidéo de… » compris).
const VERBES_VIDEO: &[&[&str]] = &[
    &["lance"],
    &["mets"],
    &["joue"],
    &["montre", "moi"],
    &["cherche"],
];

/// « lance la vidéo de X », « lance X sur YouTube », « mets la bande-annonce de X ».
fn video(a: &Analyse) -> Option<Intent> {
    let mots = a.mots();
    // « … sur YouTube » en fin de phrase : retiré de la requête.
    let sur_youtube = mots.ends_with(&["sur", "youtube"]);
    let fin = if sur_youtube {
        mots.len() - 2
    } else {
        mots.len()
    };
    let m = &mots[..fin];
    let query = if let Some(i) = apres(m, 0, VERBES_VIDEO)
        .and_then(|i| apres(m, i, &[&["la"], &["une"]]))
        .and_then(|i| apres(m, i, &[&["video"]]))
    {
        let i = facultatif(m, i, &[&["de"], &["du"], &["des"], &["d"], &["sur"]]);
        // « mets la vidéo en pause », « en plein écran »… : pas une recherche.
        if m.get(i) == Some(&"en") {
            return None;
        }
        a.origine(i..fin)?.to_string()
    } else if let Some(i) = apres(m, 0, &[&["lance"], &["mets"]])
        .and_then(|i| apres(m, i, &[&["la"], &["une"]]))
        .and_then(|i| apres(m, i, &[&["bande", "annonce"]]))
    {
        format!("bande-annonce {}", a.origine(i..fin)?)
    } else if sur_youtube {
        let i = apres(m, 0, &[&["lance"], &["mets"], &["joue"]])?;
        a.origine(i..fin)?.to_string()
    } else {
        return None;
    };
    Some(Intent::new("play_video", json!({ "query": query })))
}

/// Rang d'un résultat dit à l'ordinal (« deuxième », « 2e »…), de 1 à 5.
fn ordinal(mot: &str) -> Option<u64> {
    match mot {
        "premier" | "1er" => Some(1),
        "deuxieme" | "2e" | "2eme" | "second" => Some(2),
        "troisieme" | "3e" | "3eme" => Some(3),
        "quatrieme" | "4e" | "4eme" => Some(4),
        "cinquieme" | "5e" | "5eme" => Some(5),
        _ => None,
    }
}

/// Numéro d'un résultat dit en chiffres ou en lettres, de 1 à 5.
fn numero(mot: &str) -> Option<u64> {
    match mot {
        "1" | "un" => Some(1),
        "2" | "deux" => Some(2),
        "3" | "trois" => Some(3),
        "4" | "quatre" => Some(4),
        "5" | "cinq" => Some(5),
        _ => None,
    }
}

/// « ouvre les résultats », « ouvre le deuxième résultat », « ouvre le résultat 3 ».
fn ouvrir_resultats(m: &[&str]) -> Option<Intent> {
    let i = apres(m, 0, &[&["ouvre"], &["montre"], &["affiche"]])?;
    let i = facultatif(m, i, MOI);
    let index = match &m[i..] {
        ["les", "resultats" | "liens"] => None,
        ["la", "recherche"] if m[0] == "ouvre" => None,
        ["le", rang, "resultat" | "lien"] => Some(ordinal(rang)?),
        ["le", "resultat" | "lien", n] | ["le", "resultat" | "lien", "numero", n] => {
            Some(numero(n)?)
        }
        _ => return None,
    };
    let args = match index {
        Some(n) => json!({ "index": n }),
        None => json!({}),
    };
    Some(Intent::new("open_results", args))
}

const VERBES_RECHERCHE: &[&[&str]] = &[&["cherche"], &["recherche"], &["google"]];

/// « … sur internet » en fin de phrase.
const FIN_INTERNET: &[&[&str]] = &[&["sur", "internet"], &["sur", "le", "web"]];

/// « cherche [moi] [sur internet] X », « fais une recherche sur X »,
/// « trouve-moi des infos sur X », « X sur internet ».
fn recherche(a: &Analyse) -> Option<Intent> {
    let mots = a.mots();
    let en_fin = FIN_INTERNET.iter().find(|s| mots.ends_with(s));
    let fin = mots.len() - en_fin.map_or(0, |s| s.len());
    let m = &mots[..fin];
    let debut = if let Some(i) = apres(m, 0, VERBES_RECHERCHE) {
        let i = facultatif(m, i, MOI);
        let i = facultatif(
            m,
            i,
            &[
                &["sur", "internet"],
                &["sur", "le", "web"],
                &["en", "ligne"],
            ],
        );
        facultatif(
            m,
            i,
            &[&["des", "infos", "sur"], &["des", "informations", "sur"]],
        )
    } else if let Some(i) = apres(m, 0, &[&["fais"]]) {
        let i = facultatif(m, i, MOI);
        apres(m, i, &[&["une", "recherche", "sur"]])?
    } else if let Some(i) = apres(m, 0, &[&["trouve"]]) {
        let i = facultatif(m, i, MOI);
        apres(
            m,
            i,
            &[&["des", "infos", "sur"], &["des", "informations", "sur"]],
        )
        .or_else(|| en_fin.map(|_| i))?
    } else if en_fin.is_some() {
        // « X sur internet » : sauf après un verbe de lancement (inchangé).
        if apres(
            m,
            0,
            &[&["lance"], &["ouvre"], &["demarre"], &["mets"], &["joue"]],
        )
        .is_some()
        {
            return None;
        }
        let i = facultatif(m, 0, &[&["regarde"]]);
        facultatif(m, i, MOI)
    } else {
        return None;
    };
    let query = a.origine(debut..fin)?;
    Some(Intent::new("web_search", json!({ "query": query })))
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
    // « ouvre les résultats de tout à l'heure », « lance la bande-annonce » :
    // jamais une appli, laissé à l'IA.
    if name.starts_with("bande annonce")
        || name
            .split(' ')
            .any(|w| matches!(w, "resultat" | "resultats" | "lien" | "liens"))
    {
        return None;
    }
    Some(Intent::new("app_launch", json!({ "name": name })))
}

/// Reconnaît une phrase courante sans passer par l'IA.
pub fn match_fast(text: &str) -> Option<Intent> {
    let analyse = Analyse::new(text);
    let phrase = analyse.normalisee();
    exact(&phrase)
        .or_else(|| piste(&phrase))
        .or_else(|| video(&analyse))
        .or_else(|| ouvrir_resultats(&analyse.mots()))
        .or_else(|| recherche(&analyse))
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
    fn video_sans_sujet_laissee_a_l_ia() {
        assert_eq!(match_fast("lance la vidéo"), None);
        assert_eq!(match_fast("ouvre une video de chats"), None);
        assert_eq!(match_fast("demarre la video"), None);
        assert_eq!(match_fast("lance la bande-annonce"), None);
        assert_eq!(match_fast("mets la vidéo en pause"), None);
    }

    fn recherche(query: &str) -> Option<Intent> {
        intent("web_search", json!({ "query": query }))
    }

    fn video(query: &str) -> Option<Intent> {
        intent("play_video", json!({ "query": query }))
    }

    #[test]
    fn recherche_raccourcis() {
        for (phrase, query) in [
            ("cherche la météo de demain", "la météo de demain"),
            ("Jarvis, cherche les horaires du TER", "les horaires du TER"),
            ("recherche Rust 2024", "Rust 2024"),
            ("google Mbappé", "Mbappé"),
            (
                "cherche-moi l'adresse de la mairie",
                "l'adresse de la mairie",
            ),
            (
                "cherche sur internet la recette du fufu",
                "la recette du fufu",
            ),
            ("recherche sur le web le prix du cacao", "le prix du cacao"),
            (
                "cherche en ligne les résultats du match",
                "les résultats du match",
            ),
            (
                "tu peux chercher le cours du franc CFA ?",
                "le cours du franc CFA",
            ),
            ("fais une recherche sur les volcans", "les volcans"),
            ("fais-moi une recherche sur Kofi Annan", "Kofi Annan"),
            (
                "trouve-moi des infos sur la Coupe d'Afrique",
                "la Coupe d'Afrique",
            ),
            ("trouve moi des informations sur Lomé", "Lomé"),
            ("rechercher le film Dune", "le film Dune"),
        ] {
            assert_eq!(match_fast(phrase), recherche(query), "phrase: {phrase}");
        }
    }

    #[test]
    fn requete_garde_accents_et_majuscules() {
        assert_eq!(
            match_fast("Jarvis, cherche la météo à Lomé s'il te plaît"),
            recherche("la météo à Lomé")
        );
        assert_eq!(
            match_fast("Lance la vidéo de Stromae, Jarvis !"),
            video("Stromae")
        );
    }

    #[test]
    fn cherche_seul_pas_de_raccourci() {
        for phrase in [
            "cherche",
            "Jarvis, cherche.",
            "cherche-moi",
            "recherche",
            "cherche sur internet",
            "fais une recherche",
            "fais une recherche sur",
            "trouve-moi des infos",
            "google",
            "sur internet",
        ] {
            assert_eq!(match_fast(phrase), None, "phrase: {phrase}");
        }
    }

    #[test]
    fn recherche_sur_internet_en_fin_retire() {
        for (phrase, query) in [
            (
                "cherche la météo de demain sur internet",
                "la météo de demain",
            ),
            (
                "recherche le score du match sur le web",
                "le score du match",
            ),
            (
                "regarde les horaires du train sur internet",
                "les horaires du train",
            ),
            (
                "le prix de l'essence à Kara sur internet",
                "le prix de l'essence à Kara",
            ),
            ("cherche-moi Lomé sur internet s'il te plaît", "Lomé"),
        ] {
            assert_eq!(match_fast(phrase), recherche(query), "phrase: {phrase}");
        }
    }

    #[test]
    fn video_raccourcis() {
        for (phrase, query) in [
            ("lance la vidéo de Squeezie", "Squeezie"),
            ("mets une vidéo de chats", "chats"),
            ("joue la vidéo du discours", "discours"),
            ("lance une vidéo des Avengers", "Avengers"),
            ("mets la vidéo d'Aya Nakamura", "Aya Nakamura"),
            ("lance une vidéo sur les pyramides", "les pyramides"),
            ("montre-moi une vidéo de pandas", "pandas"),
            ("lance la vidéo Gangnam Style", "Gangnam Style"),
            (
                "cherche une vidéo de cuisine togolaise",
                "cuisine togolaise",
            ),
            ("cherche la vidéo du match", "match"),
            ("lance Stromae sur YouTube", "Stromae"),
            ("mets Daft Punk sur youtube", "Daft Punk"),
            ("joue Bohemian Rhapsody sur YouTube", "Bohemian Rhapsody"),
            ("lance la bande-annonce de Dune", "bande-annonce de Dune"),
            ("mets la bande annonce de Dune", "bande-annonce de Dune"),
            (
                "tu peux lancer la vidéo de Squeezie s'il te plaît",
                "Squeezie",
            ),
        ] {
            assert_eq!(match_fast(phrase), video(query), "phrase: {phrase}");
        }
    }

    #[test]
    fn video_avant_lancement_d_appli() {
        assert_eq!(match_fast("lance Stromae sur YouTube"), video("Stromae"));
        assert_eq!(match_fast("lance la vidéo de VS Code"), video("VS Code"));
        assert_eq!(
            match_fast("lance VS Code"),
            intent("app_launch", json!({"name": "vs code"}))
        );
    }

    #[test]
    fn ouvre_les_resultats_raccourcis() {
        for phrase in [
            "ouvre les résultats",
            "Jarvis, ouvre les résultats s'il te plaît",
            "ouvre les liens",
            "montre les résultats",
            "montre-moi les résultats",
            "affiche les résultats",
            "affiche les liens",
            "ouvre la recherche",
            "tu peux ouvrir les résultats ?",
        ] {
            assert_eq!(
                match_fast(phrase),
                intent("open_results", json!({})),
                "phrase: {phrase}"
            );
        }
        // Jamais une appli nommée « résultats ».
        assert_eq!(match_fast("ouvre les résultats de tout à l'heure"), None);
    }

    #[test]
    fn ouvre_le_resultat_par_ordinal() {
        for (phrase, index) in [
            ("ouvre le premier résultat", 1),
            ("ouvre le 1er résultat", 1),
            ("ouvre le deuxième résultat", 2),
            ("ouvre le 2e lien", 2),
            ("ouvre le 2ème résultat", 2),
            ("ouvre le second lien", 2),
            ("ouvre le troisième résultat", 3),
            ("ouvre le 3e résultat", 3),
            ("ouvre le quatrième lien", 4),
            ("ouvre le cinquième résultat", 5),
            ("ouvre le résultat 2", 2),
            ("ouvre le résultat numéro 3", 3),
            ("ouvre le résultat numéro deux", 2),
            ("ouvre le résultat un", 1),
            ("ouvre le résultat cinq", 5),
            ("Jarvis, ouvre le deuxième résultat s'il te plaît", 2),
        ] {
            assert_eq!(
                match_fast(phrase),
                intent("open_results", json!({ "index": index })),
                "phrase: {phrase}"
            );
        }
        for phrase in [
            "ouvre le sixième résultat",
            "ouvre le résultat 7",
            "ouvre le résultat",
        ] {
            assert_eq!(match_fast(phrase), None, "phrase: {phrase}");
        }
    }

    #[test]
    fn raccourcis_existants_inchanges() {
        assert_eq!(
            match_fast("lance VS Code"),
            intent("app_launch", json!({"name": "vs code"}))
        );
        assert_eq!(
            match_fast("ouvre Firefox"),
            intent("app_launch", json!({"name": "firefox"}))
        );
        assert_eq!(
            match_fast("c'est quoi cette chanson"),
            intent("media_now_playing", json!({}))
        );
        assert_eq!(match_fast("passe"), intent("media_next", json!({})));
        assert_eq!(
            match_fast("vidéo suivante"),
            intent("media_next", json!({}))
        );
        assert_eq!(
            match_fast("mets la musique"),
            intent("media_play", json!({}))
        );
        assert_eq!(
            match_fast("lance la musique"),
            intent("media_play", json!({}))
        );
        assert_eq!(
            match_fast("mets le PC en veille"),
            intent("suspend", json!({}))
        );
        assert_eq!(match_fast("c'est quoi un trou noir"), None);
        assert_eq!(match_fast("qui est Kofi Annan"), None);
        assert_eq!(match_fast("regarde"), None);
        assert_eq!(match_fast("montre-moi"), None);
        assert_eq!(match_brain_command("annule"), Some(BrainCommand::Cancel));
        assert_eq!(match_brain_command("répète"), Some(BrainCommand::Repeat));
        assert_eq!(match_confirmation("oui"), Some(true));
        assert_eq!(match_confirmation("non"), Some(false));
        assert_eq!(match_ear_command("coupe le micro"), Some(EarAction::Mute));
        assert_eq!(
            normalize("Jarvis, cherche-moi la météo"),
            "cherche moi la meteo"
        );
        assert_eq!(normalize("chercher la météo"), "cherche la meteo");
    }

    #[test]
    fn nouveaux_imperatifs() {
        for (infinitif, imperatif) in [
            ("chercher", "cherche"),
            ("rechercher", "recherche"),
            ("trouver", "trouve"),
            ("regarder", "regarde"),
            ("afficher", "affiche"),
            ("montrer", "montre"),
        ] {
            assert_eq!(normalize(infinitif), imperatif);
        }
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
