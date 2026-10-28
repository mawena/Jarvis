//! Personnalité de Jarvis : un majordome discret, qui vouvoie et nomme son maître
//! avec parcimonie.
//!
//! Les compétences gardent des phrases neutres ; seul le cerveau ajoute le nom
//! d'adresse, et seulement à quelques phrases choisies (questions de confirmation,
//! échecs, refus), jamais plus d'une fois par phrase.

/// Ton de Jarvis, tiré de la configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Persona {
    /// Nom par lequel Jarvis s'adresse à l'utilisateur (« Monsieur »). Vide : jamais nommé.
    pub address: String,
}

impl Default for Persona {
    fn default() -> Self {
        Self {
            address: "Monsieur".to_string(),
        }
    }
}

impl Persona {
    pub fn new(address: impl Into<String>) -> Self {
        Self {
            address: address.into(),
        }
    }

    /// Ajoute le nom d'adresse avant la ponctuation finale :
    /// « Il n'y a rien à annuler. » → « Il n'y a rien à annuler, Monsieur. »,
    /// « Je mets le PC en veille ? » → « Je mets le PC en veille, Monsieur ? ».
    pub fn addressed(&self, phrase: &str) -> String {
        let address = self.address.trim();
        if address.is_empty() {
            return phrase.to_string();
        }
        let body = phrase.trim_end();
        let cut = body.trim_end_matches(['.', '?', '!', '…']).len();
        let (text, end) = body.split_at(cut);
        let text = text.trim_end();
        let end = end.trim();
        // Ponctuation haute (« ? », « ! ») précédée d'une espace, à la française.
        let sep = if end.starts_with(['?', '!']) { " " } else { "" };
        let end = if end.is_empty() { "." } else { end };
        format!("{text}, {address}{sep}{end}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adresse_inseree_avant_la_ponctuation() {
        let p = Persona::default();
        assert_eq!(
            p.addressed("Il n'y a rien à annuler."),
            "Il n'y a rien à annuler, Monsieur."
        );
        assert_eq!(
            p.addressed("Je mets le PC en veille ?"),
            "Je mets le PC en veille, Monsieur ?"
        );
        assert_eq!(p.addressed("Je lance"), "Je lance, Monsieur.");
        assert_eq!(
            Persona::new("Charles").addressed("C'est fait !"),
            "C'est fait, Charles !"
        );
    }

    #[test]
    fn adresse_vide_phrase_inchangee() {
        let p = Persona::new("  ");
        assert_eq!(p.addressed("Bien."), "Bien.");
    }
}
