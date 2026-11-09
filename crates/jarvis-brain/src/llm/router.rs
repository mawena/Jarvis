//! Aiguillage du mode local : commande (outils) ou conversation (réponse libre).
//!
//! Un petit modèle local qui reçoit la liste d'outils en choisit toujours un, même pour
//! « quelle est la capitale du Pérou ». On tranche donc avant, avec une heuristique
//! française sans modèle. Dans le doute, c'est une commande (chemin historique).

use crate::fastpath::normalize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    Command,
    Conversation,
}

/// Verbes d'action et noms de ce que Jarvis pilote : leur présence fait une commande.
const COMMAND_WORDS: &[&str] = &[
    "mets",
    "mettre",
    "met",
    "remets",
    "remettre",
    "lance",
    "lancer",
    "relance",
    "ouvre",
    "ouvrir",
    "monte",
    "monter",
    "baisse",
    "baisser",
    "passe",
    "passer",
    "joue",
    "jouer",
    "reprends",
    "reprendre",
    "pause",
    "stop",
    "stoppe",
    "coupe",
    "couper",
    "augmente",
    "augmenter",
    "diminue",
    "diminuer",
    "demarre",
    "demarrer",
    "active",
    "desactive",
    "ecouter",
    "ecoute",
    "suivant",
    "suivante",
    "precedent",
    "precedente",
    "volume",
    "son",
    "musique",
    "chanson",
    "morceau",
    "titre",
    "piste",
    "lecture",
    "spotify",
    "navigateur",
    "terminal",
    "application",
    "appli",
    "calculatrice",
    "mode",
    // Recherche et vidéos.
    "cherche",
    "chercher",
    "recherche",
    "rechercher",
    "video",
    "videos",
    "youtube",
    "internet",
    "web",
    "resultat",
    "resultats",
    "google",
];

/// Mots qui, en tête de phrase, annoncent une question ou une demande de discussion.
const LEADING_MARKERS: &[&str] = &[
    "quel",
    "quelle",
    "quels",
    "quelles",
    "qui",
    "combien",
    "pourquoi",
    "comment",
    "quand",
    "ou",
    "que",
    "quoi",
    "raconte",
    "explique",
    "definis",
    "definir",
    "traduis",
    "calcule",
    "parle",
    "dis",
    "donne",
    "resume",
    "ecris",
    "cite",
    "conseille",
    "recommande",
];

/// Débuts de phrase de plusieurs mots.
const LEADING_PHRASES: &[&str] = &[
    "c est quoi",
    "qu est ce que",
    "qu est ce qu",
    "est ce que",
    "est ce qu",
    "c est qui",
    "dis moi",
    "parle moi",
    "raconte moi",
];

pub fn classify(text: &str) -> Route {
    let norm = normalize(text);
    let has_command = norm.split(' ').any(|w| COMMAND_WORDS.contains(&w));
    if has_command {
        return Route::Command;
    }
    let first = norm.split(' ').next().unwrap_or("");
    let chat = text.trim_end().ends_with('?')
        || LEADING_MARKERS.contains(&first)
        || LEADING_PHRASES
            .iter()
            .any(|p| norm == *p || norm.starts_with(&format!("{p} ")));
    if chat {
        Route::Conversation
    } else {
        Route::Command
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversations() {
        for p in [
            "quelle est la capitale du Pérou",
            "raconte-moi une blague courte",
            "combien font 12 fois 7",
            "quelle heure est-il",
            "qui a écrit les Misérables",
            "traduis bonjour en anglais",
            "pourquoi le ciel est bleu",
            "comment fait-on une omelette",
            "c'est quoi un trou noir",
            "explique-moi la photosynthèse",
            "dis-moi quelque chose de drôle",
            "parle-moi de Napoléon",
            "définis le mot éphémère",
            "calcule 15 pour cent de 80",
            "est-ce que les baleines sont des poissons",
            "quand a eu lieu la révolution française",
            "où se trouve Lima",
            "quel temps fait-il demain ?",
            "la météo ?",
            "qu'est-ce que la gravité",
            "Jarvis, quelle est la capitale du Japon",
        ] {
            assert_eq!(classify(p), Route::Conversation, "{p}");
        }
    }

    #[test]
    fn commandes() {
        for p in [
            "suivant",
            "mets en pause",
            "reprends",
            "plus fort",
            "baisse le son",
            "c'est quoi cette chanson",
            "passe en mode local",
            "lance VS Code",
            "ouvre Firefox",
            "démarre Spotify",
            "monte le volume",
            "mets un peu plus de son",
            "ouvre-moi le navigateur",
            "je veux écouter autre chose",
            "tu peux baisser un peu le volume",
            "remets la chanson d'avant",
            "stoppe la musique deux secondes",
            "c'est quel morceau là",
            "je n'entends rien, augmente le son",
            "j'aimerais bien avoir la calculatrice",
            "lance-moi le terminal",
            "relance la musique",
            "moins fort",
            "coupe la musique",
            "est-ce que tu peux lancer spotify",
            "est-ce que tu peux remettre la précédente ?",
            "diminue le volume",
            "joue de la musique",
        ] {
            assert_eq!(classify(p), Route::Command, "{p}");
        }
    }

    #[test]
    fn recherche_et_video_sont_des_commandes() {
        for p in [
            "trouve-moi une vidéo de chats",
            "qu'est-ce qu'on dit sur internet de Rust ?",
            "cherche la météo de demain",
            "recherche les horaires du train",
            "ouvre les résultats",
            "lance une vidéo YouTube",
            "regarde sur le web",
            "fais une recherche Google",
        ] {
            assert_eq!(classify(p), Route::Command, "{p}");
        }
    }

    #[test]
    fn dans_le_doute_commande() {
        assert_eq!(classify("hmm"), Route::Command);
        assert_eq!(classify(""), Route::Command);
    }
}
