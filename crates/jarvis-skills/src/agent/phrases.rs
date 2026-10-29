//! Phrases de l'agent : départ, suivi, arrêt.
//!
//! Déclencheur déterministe : un verbe de code et un projet nommé, ou une
//! ouverture explicite. Une phrase courante ne lance jamais l'agent.

use super::projects::ProjectList;
use super::settings::Project;
use super::types::AgentCommand;
use crate::text::fold;
use std::ops::Range;

/// Longueur maximale d'une consigne (caractères).
const MAX_INSTRUCTION: usize = 2000;

/// Politesses retirées en tête de phrase.
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
];

/// Politesses retirées en fin de phrase.
const SUFFIXES: &[&[&str]] = &[
    &["s", "il", "te", "plait"],
    &["s", "il", "vous", "plait"],
    &["stp"],
    &["svp"],
    &["merci", "beaucoup"],
    &["merci"],
];

/// Mots derrière lesquels un « Jarvis » final nomme le projet (« …du projet
/// Jarvis ») ; partout ailleurs, il s'adresse à Jarvis et il est retiré.
const NAMES_FOLLOW: &[&str] = &["projet", "de", "du", "dans", "sur", "au", "pour"];

/// Verbes de code, en tête de phrase.
const CODE_VERBS: &[&[&str]] = &[
    &["finis"],
    &["finir"],
    &["termine"],
    &["terminer"],
    &["acheve"],
    &["achever"],
    &["corrige"],
    &["corriger"],
    &["repare"],
    &["reparer"],
    &["ajoute"],
    &["ajouter"],
    &["ecris"],
    &["ecrire"],
    &["implemente"],
    &["implementer"],
    &["refactorise"],
    &["refactoriser"],
    &["nettoie"],
    &["nettoyer"],
    &["teste"],
    &["tester"],
    &["documente"],
    &["documenter"],
    &["ameliore"],
    &["ameliorer"],
    &["optimise"],
    &["optimiser"],
    &["renomme"],
    &["renommer"],
    &["migre"],
    &["migrer"],
    &["supprime"],
    &["supprimer"],
    &["prepare"],
    &["preparer"],
    &["mets", "a", "jour"],
    &["mettre", "a", "jour"],
    &["fais", "passer"],
    &["faire", "passer"],
    &["lance", "les", "tests"],
    &["lancer", "les", "tests"],
    &["fais", "tourner", "les", "tests"],
    &["faire", "tourner", "les", "tests"],
];

/// Ouvertures explicites, en tête de phrase.
const OPENERS: &[&[&str]] = &[
    &["demande", "a", "l", "agent"],
    &["confie", "a", "l", "agent"],
    &["tache", "de", "code"],
];

/// Formules de suivi, n'importe où dans la phrase.
const STATUS: &[&[&str]] = &[
    &["ou", "en", "est"],
    &["ou", "en", "sont"],
    &["ca", "avance"],
    &["en", "est", "ou"],
    &["en", "sont", "ou"],
    &["quelles", "taches"],
    &["etat", "de", "la", "tache"],
];

/// « La tâche est finie ? » : sujet suivi d'un verbe de fin.
const DONE_SUBJECTS: &[&[&str]] = &[&["la", "tache"], &["les", "taches"], &["l", "agent"]];
const DONE_VERBS: &[&[&str]] = &[
    &["est", "finie"],
    &["est", "fini"],
    &["est", "terminee"],
    &["est", "termine"],
    &["a", "fini"],
    &["a", "termine"],
    &["sont", "finies"],
    &["sont", "terminees"],
    &["ont", "fini"],
    &["ont", "termine"],
];

/// Verbes d'arrêt, en tête de phrase : impératif (tu, vous), infinitif
/// (« tu peux arrêter… », politesse retirée) et « stop ».
const STOP_VERBS: &[&str] = &[
    "arrete",
    "arretes",
    "arretez",
    "arreter",
    "stop",
    "stoppe",
    "stoppez",
    "stopper",
    "interromps",
    "interrompez",
    "interrompre",
    "annule",
    "annulez",
    "annuler",
    "coupe",
    "coupez",
    "couper",
];

/// Ce qu'on arrête : jamais « arrête » seul ni « arrête la musique ».
const STOP_TARGETS: &[&[&str]] = &[&["la", "tache"], &["les", "taches"], &["l", "agent"]];

/// Mots qui ne font pas partie d'un nom de projet entendu.
const NOT_A_NAME: &[&str] = &[
    "a", "au", "aux", "avec", "dans", "de", "des", "du", "en", "et", "la", "le", "les", "mais",
    "ou", "par", "pour", "puis", "qui", "que", "s", "sur",
];

/// Commande de l'agent comprise dans `text` (phrase d'origine), ou `None`.
pub fn parse(text: &str, projects: &ProjectList) -> Option<AgentCommand> {
    let phrase = Phrase::new(text);
    let words: Vec<&str> = phrase.words.iter().map(String::as_str).collect();
    let first = *words.first()?;

    if STOP_VERBS.contains(&first) && STOP_TARGETS.iter().any(|t| contains(&words, t)) {
        let project = phrase.named(projects).map(Named::into_name);
        return Some(AgentCommand::Stop { project });
    }

    let done_question = DONE_SUBJECTS.iter().any(|s| {
        DONE_VERBS
            .iter()
            .any(|v| contains(&words, &[*s, *v].concat()))
    });
    if done_question || STATUS.iter().any(|s| contains(&words, s)) {
        let named = phrase.named(projects);
        let about_tasks = words
            .iter()
            .any(|w| ["tache", "taches", "agent"].contains(w));
        if about_tasks || matches!(named, Some(Named::Known(_))) {
            let project = named.map(Named::into_name);
            return Some(AgentCommand::Status { project });
        }
        return None;
    }

    let opener = OPENERS.iter().any(|o| words.starts_with(o));
    let verb = CODE_VERBS.iter().any(|v| words.starts_with(v));
    if !opener && !verb {
        return None;
    }
    match phrase.named(projects)? {
        Named::Known(p) => Some(AgentCommand::Start {
            project: p.name.clone(),
            instruction: phrase.instruction(),
        }),
        // « Ajoute le projet vacances à ma liste » : pas une tâche de code.
        Named::Unknown(heard) if opener || projects.is_near(&heard) => {
            Some(AgentCommand::UnknownProject { heard })
        }
        Named::Unknown(_) => None,
    }
}

/// `words` contient la suite `seq`.
fn contains(words: &[&str], seq: &[&str]) -> bool {
    words.windows(seq.len()).any(|w| w == seq)
}

/// Projet nommé dans une phrase.
enum Named<'a> {
    Known(&'a Project),
    /// « projet X » inconnu : X tel qu'entendu.
    Unknown(String),
}

impl Named<'_> {
    fn into_name(self) -> String {
        match self {
            Named::Known(p) => p.name.clone(),
            Named::Unknown(heard) => heard,
        }
    }
}

/// Phrase découpée en mots, politesses retirées, avec leur place dans le texte
/// d'origine.
struct Phrase<'a> {
    text: &'a str,
    /// Mots gardés, passés par `fold`.
    words: Vec<String>,
    /// Place de chaque mot gardé dans `text`.
    places: Vec<Range<usize>>,
    /// Début du mot qui suit le dernier gardé (ou fin du texte).
    tail: usize,
}

impl<'a> Phrase<'a> {
    fn new(text: &'a str) -> Self {
        let places = original_words(text);
        let words: Vec<String> = places.iter().map(|p| fold(&text[p.clone()])).collect();
        let refs: Vec<&str> = words.iter().map(String::as_str).collect();
        let (mut start, mut end) = (0, refs.len());
        loop {
            let before = (start, end);
            if let Some(p) = PREFIXES.iter().find(|p| refs[start..end].starts_with(p)) {
                start += p.len();
            }
            if let Some(s) = SUFFIXES.iter().find(|s| refs[start..end].ends_with(s)) {
                end -= s.len();
            }
            // « …, merci Jarvis » : on s'adresse à Jarvis ; « …du projet
            // Jarvis » nomme le projet (sauf derrière une virgule).
            if end > start + 1
                && refs[end - 1] == "jarvis"
                && (!NAMES_FOLLOW.contains(&refs[end - 2])
                    || text[places[end - 2].end..places[end - 1].start].contains(','))
            {
                end -= 1;
            }
            if (start, end) == before {
                break;
            }
        }
        let tail = places.get(end).map_or(text.len(), |p| p.start);
        Phrase {
            text,
            words: words[start..end].to_vec(),
            places: places[start..end].to_vec(),
            tail,
        }
    }

    /// Projet nommé : nom ou alias exact n'importe où, sinon les mots qui
    /// suivent « projet » (recherche floue, ou nom inconnu).
    fn named<'p>(&self, projects: &'p ProjectList) -> Option<Named<'p>> {
        let words: Vec<&str> = self.words.iter().map(String::as_str).collect();
        if let Some((p, _, _)) = projects.find_in(&words) {
            return Some(Named::Known(p));
        }
        let start = words.iter().position(|w| *w == "projet")? + 1;
        let len = words[start..]
            .iter()
            .take(3)
            .take_while(|w| !NOT_A_NAME.contains(w))
            .count();
        if len == 0 {
            return None;
        }
        let heard = &self.text[self.places[start].start..self.places[start + len - 1].end];
        Some(match projects.resolve(heard) {
            Some(p) => Named::Known(p),
            None => Named::Unknown(heard.to_string()),
        })
    }

    /// Consigne : texte d'origine sans politesse ni ponctuation finale, borné.
    fn instruction(&self) -> String {
        let Some(first) = self.places.first() else {
            return String::new();
        };
        let text = self.text[first.start..self.tail]
            .trim_end_matches(|c: char| c.is_whitespace() || ",.;:!?…".contains(c));
        text.chars().take(MAX_INSTRUCTION).collect()
    }
}

/// Mots d'origine : suites de caractères alphanumériques (même découpage que
/// `fold`).
fn original_words(text: &str) -> Vec<Range<usize>> {
    let mut words = Vec::new();
    let mut start = None;
    for (i, c) in text.char_indices() {
        if c.is_alphanumeric() {
            start.get_or_insert(i);
        } else if let Some(s) = start.take() {
            words.push(s..i);
        }
    }
    if let Some(s) = start {
        words.push(s..text.len());
    }
    words
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::projects::tests::connus;

    fn depart(project: &str, instruction: &str) -> Option<AgentCommand> {
        Some(AgentCommand::Start {
            project: project.into(),
            instruction: instruction.into(),
        })
    }

    #[test]
    fn finis_les_tests_du_projet_jarvis() {
        assert_eq!(
            parse("Jarvis, finis les tests du projet Jarvis.", &connus()),
            depart("Jarvis", "finis les tests du projet Jarvis")
        );
    }

    #[test]
    fn projet_nomme_jarvis_reconnu_en_fin_de_phrase() {
        let l = connus();
        assert_eq!(
            parse("ajoute un test au projet Jarvis", &l),
            depart("Jarvis", "ajoute un test au projet Jarvis")
        );
        assert_eq!(
            parse("corrige les tests du projet Jarvis s'il te plaît", &l),
            depart("Jarvis", "corrige les tests du projet Jarvis")
        );
        assert_eq!(
            parse("finis les tests du projet Jarvis, merci Jarvis", &l),
            depart("Jarvis", "finis les tests du projet Jarvis")
        );
        assert_eq!(
            parse("Jarvis, ajoute un test au projet Jarvis stp Jarvis", &l),
            depart("Jarvis", "ajoute un test au projet Jarvis")
        );
        assert_eq!(
            parse("corrige le bug de l'assistant", &l),
            depart("Jarvis", "corrige le bug de l'assistant")
        );
        // « …, Jarvis » en fin de phrase : on s'adresse à Jarvis.
        assert_eq!(
            parse("Finis les tests de Mawena, Jarvis", &l),
            depart("Mawena", "Finis les tests de Mawena")
        );
        assert_eq!(
            parse("Finis les tests de Mawena merci Jarvis", &l),
            depart("Mawena", "Finis les tests de Mawena")
        );
        // Sans « projet » ni alias, « Jarvis » ne nomme pas le projet.
        assert_eq!(parse("corrige les tests de Jarvis", &l), None);
    }

    #[test]
    fn jarvis_en_fin_de_phrase_n_est_pas_le_projet() {
        let l = connus();
        for phrase in [
            "Ajoute du lait à la liste merci Jarvis",
            "ajoute du lait à la liste, Jarvis",
            "ajoute du lait à la liste",
            "supprime le minuteur Jarvis",
            "supprime l'alarme",
            "supprime l'alarme de demain, merci Jarvis",
            "teste le micro Jarvis",
            "teste le micro",
            "mets à jour Jarvis",
            "mets à jour le système",
            "mets à jour le système s'il te plaît Jarvis",
            "prépare un café",
            "prépare un café stp Jarvis",
            "lance Spotify",
            "lance Spotify Jarvis",
            "ouvre le projet Jarvis",
            "cherche la météo de demain Jarvis",
            "cherche le projet Jarvis sur internet",
            "où en est mon colis, merci Jarvis",
            "où en est mon colis Jarvis",
            "ça avance Jarvis ?",
            "termine la playlist Jarvis",
            "corrige l'orthographe de ce message Jarvis",
            "nettoie le bureau, Jarvis",
            "renomme le fichier rapport Jarvis",
            "finis la phrase Jarvis",
            "écris un message à Kofi merci Jarvis",
            "améliore le son Jarvis",
            "répare la connexion wifi Jarvis",
            "ajoute un rendez-vous pour Jarvis",
            "arrête la musique, merci Jarvis",
            "stop Jarvis",
            "Jarvis Jarvis",
            "merci Jarvis",
        ] {
            assert_eq!(parse(phrase, &l), None, "{phrase}");
        }
    }

    #[test]
    fn corrige_le_bug_dans_mawena() {
        let l = connus();
        assert_eq!(
            parse("corrige le bug dans Mawena", &l),
            depart("Mawena", "corrige le bug dans Mawena")
        );
        assert_eq!(
            parse(
                "Jarvis, tu peux mettre à jour la doc du projet mawenna ?",
                &l
            ),
            depart("Mawena", "mettre à jour la doc du projet mawenna")
        );
        assert_eq!(
            parse("lance les tests de l'assistant", &l),
            depart("Jarvis", "lance les tests de l'assistant")
        );
        assert_eq!(
            parse(
                "Demande à l'agent de relire le module audio du projet Jarvis",
                &l
            ),
            depart(
                "Jarvis",
                "Demande à l'agent de relire le module audio du projet Jarvis"
            )
        );
    }

    #[test]
    fn consigne_gardee_avec_accents() {
        assert_eq!(
            parse(
                "Jarvis, s'il te plaît, améliore l'écran « Réglages » du projet Mawena, merci.",
                &connus()
            ),
            depart("Mawena", "améliore l'écran « Réglages » du projet Mawena")
        );
        let long = format!("corrige {} dans Mawena", "é".repeat(3000));
        let Some(AgentCommand::Start { instruction, .. }) = parse(&long, &connus()) else {
            panic!("pas une tâche");
        };
        assert_eq!(instruction.chars().count(), 2000);
    }

    fn inconnu(heard: &str) -> Option<AgentCommand> {
        Some(AgentCommand::UnknownProject {
            heard: heard.into(),
        })
    }

    #[test]
    fn projet_inconnu_dit_le_nom_entendu() {
        let l = connus();
        // Après une ouverture explicite.
        assert_eq!(
            parse(
                "demande à l'agent de corriger le bug du projet Zorglub dans le parseur",
                &l
            ),
            inconnu("Zorglub")
        );
        assert_eq!(
            parse("tâche de code : finis les tests du projet Tam Tam", &l),
            inconnu("Tam Tam")
        );
        // Nom proche d'un projet connu, sans le désigner sûrement.
        assert_eq!(
            parse("corrige le bug du projet Jarvoss", &l),
            inconnu("Jarvoss")
        );
        let ambigus = ProjectList::new(vec![
            crate::agent::projects::tests::projet("Mawena App", &[]),
            crate::agent::projects::tests::projet("Mawena Api", &[]),
        ]);
        assert_eq!(
            parse("finis les tests du projet mawena", &ambigus),
            inconnu("mawena")
        );
    }

    #[test]
    fn projet_inconnu_sans_ouverture_laisse_l_ia_repondre() {
        let l = connus();
        assert_eq!(parse("Ajoute le projet vacances à ma liste", &l), None);
        assert_eq!(parse("termine le projet maison", &l), None);
        assert_eq!(
            parse("corrige le bug du projet Zorglub dans le parseur", &l),
            None
        );
    }

    #[test]
    fn verbe_sans_projet_pas_une_tache() {
        let l = connus();
        assert_eq!(parse("corrige le bug", &l), None);
        assert_eq!(parse("ajoute du lait à la liste", &l), None);
        assert_eq!(parse("finis les tests du projet", &l), None);
        assert_eq!(parse("demande à l'agent de corriger le bug", &l), None);
    }

    #[test]
    fn lance_jarvis_n_est_pas_une_tache() {
        let l = connus();
        assert_eq!(parse("lance Jarvis", &l), None);
        assert_eq!(parse("Jarvis, lance Mawena", &l), None);
        assert_eq!(parse("Jarvis", &l), None);
        assert_eq!(parse("", &l), None);
    }

    #[test]
    fn ouvre_le_projet_n_est_pas_une_tache() {
        let l = connus();
        assert_eq!(parse("ouvre le projet Jarvis", &l), None);
        assert_eq!(parse("parle-moi du projet Mawena", &l), None);
    }

    #[test]
    fn ou_en_est_la_tache() {
        let l = connus();
        let statut = Some(AgentCommand::Status { project: None });
        assert_eq!(parse("Jarvis, où en est la tâche ?", &l), statut);
        assert_eq!(parse("ça avance, l'agent ?", &l), statut);
        assert_eq!(parse("quelles tâches tournent", &l), statut);
        assert_eq!(parse("état de la tâche", &l), statut);
        assert_eq!(parse("ça en est où, la tâche", &l), statut);
        assert_eq!(parse("elle en est où, la tâche ?", &l), statut);
        assert_eq!(parse("on en est où avec l'agent ?", &l), statut);
        assert_eq!(parse("la tâche est finie ?", &l), statut);
        assert_eq!(parse("est-ce que l'agent a fini ?", &l), statut);
        assert_eq!(parse("l'agent a terminé ?", &l), statut);
        assert_eq!(parse("les tâches sont terminées, Jarvis ?", &l), statut);
        assert_eq!(parse("où en est la météo", &l), None);
        assert_eq!(parse("le lave-linge a fini ?", &l), None);
        assert_eq!(parse("le film est fini ?", &l), None);
    }

    #[test]
    fn ou_en_est_la_tache_de_mawena() {
        let l = connus();
        let statut = Some(AgentCommand::Status {
            project: Some("Mawena".into()),
        });
        assert_eq!(parse("où en est la tâche de Mawena ?", &l), statut);
        assert_eq!(parse("ça avance sur le projet mawenna ?", &l), statut);
    }

    #[test]
    fn arrete_la_tache() {
        let l = connus();
        let arret = Some(AgentCommand::Stop { project: None });
        assert_eq!(parse("Jarvis, arrête la tâche", &l), arret);
        assert_eq!(parse("stoppe l'agent", &l), arret);
        assert_eq!(parse("annule les tâches, Jarvis", &l), arret);
        // « Jarvis » final s'adresse à Jarvis.
        assert_eq!(parse("arrête la tâche Jarvis", &l), arret);
        for phrase in [
            "tu peux arrêter la tâche ?",
            "peux-tu stopper l'agent",
            "stop la tâche",
            "arrêtez la tâche",
            "arrêter la tâche",
            "est-ce que vous pouvez interrompre la tâche ?",
            "coupez l'agent s'il vous plaît",
        ] {
            assert_eq!(parse(phrase, &l), arret, "{phrase}");
        }
        assert_eq!(
            parse("arrête la tâche du projet Jarvis", &l),
            Some(AgentCommand::Stop {
                project: Some("Jarvis".into())
            })
        );
        assert_eq!(
            parse("interromps la tâche sur Mawena", &l),
            Some(AgentCommand::Stop {
                project: Some("Mawena".into())
            })
        );
    }

    #[test]
    fn arrete_seul_n_est_pas_l_agent() {
        let l = connus();
        assert_eq!(parse("arrête", &l), None);
        assert_eq!(parse("stop", &l), None);
        assert_eq!(parse("arrêtez", &l), None);
        assert_eq!(parse("Jarvis, arrête la musique", &l), None);
        assert_eq!(parse("stop la musique", &l), None);
        assert_eq!(parse("tu peux arrêter la musique ?", &l), None);
        assert_eq!(parse("coupe le son", &l), None);
    }

    #[derive(serde::Deserialize)]
    struct Jeu {
        phrase: Vec<Reference>,
    }

    #[derive(serde::Deserialize)]
    struct Reference {
        text: String,
    }

    #[test]
    fn aucune_phrase_de_reference_n_est_une_tache() {
        let chemin = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../jarvis-brain/tests/fixtures/phrases.toml"
        );
        let jeu: Jeu = toml::from_str(&std::fs::read_to_string(chemin).unwrap()).unwrap();
        assert!(jeu.phrase.len() > 20);
        let l = connus();
        for r in &jeu.phrase {
            assert_eq!(parse(&r.text, &l), None, "{}", r.text);
        }
    }
}
