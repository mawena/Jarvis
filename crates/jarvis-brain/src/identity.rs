//! Qui parle ? (phase 8) : la personne est celle de l'oreille, sauf qu'une voix
//! reconnue par l'empreinte du propriétaire vaut le propriétaire sur n'importe quelle
//! oreille. L'empreinte ne prouve que le propriétaire, jamais un invité.

use crate::people::{EarPerson, People, Speaker, Trust};
use crate::speaker::Verdict;
use jarvis_protocol::EarId;

/// Décision de l'écoute pour une phrase.
#[derive(Debug, Clone, PartialEq)]
pub enum Identity {
    /// Phrase ignorée (voix inconnue sur l'oreille du propriétaire).
    Ignore,
    /// Phrase attribuée à `speaker` ; `short_only` : raccourcis seulement (jamais l'IA).
    Speak { speaker: Speaker, short_only: bool },
}

/// `verdict` : `None` si aucune vérification (désactivée ou installation neuve).
///
/// | Oreille de… | `Match` | `NotEnrolled` / `None` | `SessionShort` | `NoMatch` / `TooShort` |
/// |---|---|---|---|---|
/// | propriétaire | `Recognized` | `Accepted` | `Unverified`, raccourcis seuls | ignoré |
/// | un invité | propriétaire `Recognized` | invité `Unverified` | invité `Unverified`, raccourcis seuls | invité `Unverified` (raccourcis seuls si `TooShort`) |
/// | personne introuvable | propriétaire `Recognized` | sans droit | idem, raccourcis seuls | idem |
pub fn identify(people: &People, ear: &EarId, verdict: Option<Verdict>) -> Identity {
    if matches!(verdict, Some(Verdict::Match(_))) {
        return Identity::Speak {
            speaker: people.owner_speaker(Trust::Recognized),
            short_only: false,
        };
    }
    let short = matches!(verdict, Some(Verdict::SessionShort | Verdict::TooShort));
    match people.person_of(ear) {
        EarPerson::Owner => match verdict {
            None | Some(Verdict::NotEnrolled) => Identity::Speak {
                speaker: people.owner_speaker(Trust::Accepted),
                short_only: false,
            },
            Some(Verdict::SessionShort) => Identity::Speak {
                speaker: people.owner_speaker(Trust::Unverified),
                short_only: true,
            },
            Some(_) => Identity::Ignore,
        },
        EarPerson::Guest(name) => Identity::Speak {
            speaker: people.speaker(&name, Trust::Unverified),
            short_only: short,
        },
        // Aucun droit : raccourcis seuls (pas d'IA pour rien).
        EarPerson::Unknown => Identity::Speak {
            speaker: people.speaker("", Trust::Unverified),
            short_only: true,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn people() -> People {
        let p = People::admin_only("Charles");
        let tools = ["media_next".to_string()];
        p.add_role("famille", vec!["media_*".into()], false, false, &tools)
            .unwrap();
        p.add_person("Léa", "famille").unwrap();
        p.attach("Léa", &EarId::new("chambre")).unwrap();
        p
    }

    fn salon() -> EarId {
        EarId::new("salon")
    }

    fn chambre() -> EarId {
        EarId::new("chambre")
    }

    fn speak(i: Identity) -> (Speaker, bool) {
        match i {
            Identity::Speak {
                speaker,
                short_only,
            } => (speaker, short_only),
            Identity::Ignore => panic!("ignoré"),
        }
    }

    #[test]
    fn identify_oreille_de_charles_match() {
        let (s, short) = speak(identify(&people(), &salon(), Some(Verdict::Match(0.9))));
        assert_eq!(
            (s.person.as_str(), s.trust, short),
            ("Charles", Trust::Recognized, false)
        );
    }

    #[test]
    fn identify_oreille_de_charles_sans_empreinte_accepted() {
        for v in [None, Some(Verdict::NotEnrolled)] {
            let (s, short) = speak(identify(&people(), &salon(), v));
            assert_eq!(
                (s.person.as_str(), s.trust, short),
                ("Charles", Trust::Accepted, false)
            );
        }
    }

    #[test]
    fn identify_oreille_de_charles_session_courte_unverified_raccourcis_seuls() {
        let (s, short) = speak(identify(&people(), &salon(), Some(Verdict::SessionShort)));
        assert_eq!(
            (s.person.as_str(), s.trust, short),
            ("Charles", Trust::Unverified, true)
        );
    }

    #[test]
    fn identify_oreille_de_charles_nomatch_ignore() {
        assert_eq!(
            identify(&people(), &salon(), Some(Verdict::NoMatch(0.1))),
            Identity::Ignore
        );
    }

    #[test]
    fn identify_oreille_de_charles_trop_court_ignore() {
        assert_eq!(
            identify(&people(), &salon(), Some(Verdict::TooShort)),
            Identity::Ignore
        );
    }

    #[test]
    fn identify_oreille_d_un_invite_match_vaut_charles() {
        let (s, short) = speak(identify(&people(), &chambre(), Some(Verdict::Match(0.9))));
        assert_eq!(
            (s.person.as_str(), s.trust, short),
            ("Charles", Trust::Recognized, false)
        );
        assert!(s.rights.all);
    }

    #[test]
    fn identify_oreille_d_un_invite_nomatch_vaut_l_invite_unverified() {
        let (s, short) = speak(identify(&people(), &chambre(), Some(Verdict::NoMatch(0.1))));
        assert_eq!(
            (s.person.as_str(), s.trust, short),
            ("Léa", Trust::Unverified, false)
        );
        assert!(!s.rights.all);
    }

    #[test]
    fn identify_oreille_d_un_invite_trop_court_raccourcis_seuls() {
        for v in [Verdict::TooShort, Verdict::SessionShort] {
            let (s, short) = speak(identify(&people(), &chambre(), Some(v)));
            assert_eq!(
                (s.person.as_str(), s.trust, short),
                ("Léa", Trust::Unverified, true)
            );
        }
    }

    #[test]
    fn identify_oreille_d_un_invite_sans_verification_unverified() {
        for v in [None, Some(Verdict::NotEnrolled)] {
            let (s, short) = speak(identify(&people(), &chambre(), v));
            assert_eq!(
                (s.person.as_str(), s.trust, short),
                ("Léa", Trust::Unverified, false)
            );
        }
    }

    #[test]
    fn identify_oreille_non_rattachee_vaut_charles() {
        let (s, _) = speak(identify(&people(), &EarId::new("ancienne"), None));
        assert_eq!((s.person.as_str(), s.trust), ("Charles", Trust::Accepted));
    }

    #[test]
    fn identify_oreille_de_personne_supprimee_sans_droit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("people.toml");
        std::fs::write(
            &path,
            "attached = [\"chambre\"]\n\n[[person]]\nname = \"Charles\"\nrole = \"admin\"\nowner = true\n",
        )
        .unwrap();
        let p2 = People::open(&path, "Charles").unwrap();
        assert_eq!(p2.person_of(&chambre()), EarPerson::Unknown);
        let (s, _) = speak(identify(&p2, &chambre(), None));
        assert_eq!(s.rights, crate::people::Rights::default());
    }

    #[test]
    fn identify_mode_degrade_match_seul() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("people.toml");
        std::fs::write(&path, "pas du toml [").unwrap();
        let p = People::open_or_degraded(&path, "Charles");
        assert!(p.degraded().is_some());
        let (s, _) = speak(identify(&p, &salon(), None));
        assert_eq!(s.rights, crate::people::Rights::default());
        let (s, _) = speak(identify(&p, &salon(), Some(Verdict::NoMatch(0.1))));
        assert_eq!(s.rights, crate::people::Rights::default());
        let (s, _) = speak(identify(&p, &salon(), Some(Verdict::Match(0.9))));
        assert!(s.rights.all);
        assert_eq!(s.trust, Trust::Recognized);
    }
}
