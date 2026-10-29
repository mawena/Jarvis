//! Projets connus : nom, alias, recherche floue.

use super::settings::Project;
use crate::text::fold;
use fuzzy_matcher::FuzzyMatcher;
use fuzzy_matcher::skim::SkimMatcherV2;

/// Score flou minimal (même seuil que les applications).
const MIN_SCORE: i64 = 50;

/// Mot d'éveil : un projet de ce nom n'est nommé que par « projet Jarvis »
/// (ou un alias), jamais par « …, merci Jarvis ».
const WAKE_WORD: &str = "jarvis";

/// Liste des projets connus de l'agent.
#[derive(Debug, Clone, Default)]
pub struct ProjectList {
    projects: Vec<Project>,
}

impl ProjectList {
    pub fn new(projects: Vec<Project>) -> Self {
        Self { projects }
    }

    /// Noms des projets, dans l'ordre de la configuration.
    pub fn names(&self) -> Vec<&str> {
        self.projects.iter().map(|p| p.name.as_str()).collect()
    }

    /// Projet désigné par un nom entendu : nom ou alias exact, puis recherche
    /// floue. Deux projets presque aussi proches : `None` (ambigu).
    pub fn resolve(&self, spoken: &str) -> Option<&Project> {
        let query = fold(spoken);
        if query.is_empty() {
            return None;
        }
        if let Some(p) = self.projects.iter().find(|p| keys(p).any(|k| k == query)) {
            return Some(p);
        }
        if query.chars().filter(|c| *c != ' ').count() < 3 {
            return None;
        }
        let matcher = SkimMatcherV2::default();
        // Meilleur score de chaque projet, puis nom le plus court.
        let mut scores: Vec<(i64, usize, &Project)> = self
            .projects
            .iter()
            .filter_map(|p| {
                let score = keys(p).filter_map(|k| fuzzy(&matcher, &k, &query)).max()?;
                (score >= MIN_SCORE).then(|| (score, p.name.chars().count(), p))
            })
            .collect();
        scores.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        match scores.as_slice() {
            [] => None,
            [(_, _, p)] => Some(p),
            [(best, _, p), (second, _, _), ..] => (*second * 10 < *best * 9).then_some(*p),
        }
    }

    /// Le nom entendu ressemble à un projet connu sans le désigner sûrement
    /// (ambigu, ou à une ou deux lettres près).
    pub fn is_near(&self, spoken: &str) -> bool {
        let query = fold(spoken);
        if query.chars().filter(|c| *c != ' ').count() < 3 {
            return false;
        }
        let matcher = SkimMatcherV2::default();
        self.projects.iter().flat_map(keys).any(|k| {
            fuzzy(&matcher, &k, &query).is_some_and(|s| s >= MIN_SCORE)
                || distance(&k, &query) <= k.chars().count() / 3
        })
    }

    /// Premier groupe de mots (déjà passés par `fold`) égal à un nom ou un
    /// alias : le projet, l'indice du premier mot et l'indice qui suit le
    /// dernier. Un nom égal au mot d'éveil ne compte que juste après « projet ».
    pub fn find_in(&self, words: &[&str]) -> Option<(&Project, usize, usize)> {
        for start in 0..words.len() {
            let mut best: Option<(&Project, usize)> = None;
            let after_projet = start > 0 && words[start - 1] == "projet";
            for p in &self.projects {
                for key in keys(p) {
                    if key == WAKE_WORD && !after_projet {
                        continue;
                    }
                    let key: Vec<&str> = key.split(' ').collect();
                    let end = start + key.len();
                    let longer = best.is_none_or(|(_, e)| end > e);
                    if end <= words.len() && words[start..end] == key[..] && longer {
                        best = Some((p, end));
                    }
                }
            }
            if let Some((p, end)) = best {
                return Some((p, start, end));
            }
        }
        None
    }
}

/// Nom et alias d'un projet, passés par `fold` (vides retirés).
fn keys(p: &Project) -> impl Iterator<Item = String> + '_ {
    std::iter::once(&p.name)
        .chain(&p.aliases)
        .map(|k| fold(k))
        .filter(|k| !k.is_empty())
}

/// Score flou dans les deux sens : un nom entendu peut avoir une lettre de trop
/// (« mawenna ») ou de moins (« jarvi »).
fn fuzzy(matcher: &SkimMatcherV2, key: &str, query: &str) -> Option<i64> {
    let a = matcher.fuzzy_match(key, query);
    let b = matcher.fuzzy_match(query, key);
    a.max(b)
}

/// Distance d'édition (Levenshtein) entre deux textes.
fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cur = row[j + 1];
            row[j + 1] = if ca == *cb {
                prev
            } else {
                1 + prev.min(cur).min(row[j])
            };
            prev = cur;
        }
    }
    row[b.len()]
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::path::PathBuf;

    pub(crate) fn projet(name: &str, aliases: &[&str]) -> Project {
        Project {
            name: name.into(),
            path: PathBuf::from(format!("/tmp/{name}")),
            aliases: aliases.iter().map(|a| a.to_string()).collect(),
            base: None,
            extra_allowed_tools: Vec::new(),
        }
    }

    /// Jarvis et Mawena, comme dans la configuration de Charles.
    pub(crate) fn connus() -> ProjectList {
        ProjectList::new(vec![
            projet("Jarvis", &["l'assistant"]),
            projet("Mawena", &[]),
        ])
    }

    fn nom(p: Option<&Project>) -> Option<&str> {
        p.map(|p| p.name.as_str())
    }

    #[test]
    fn projet_exact_par_nom() {
        let l = connus();
        assert_eq!(nom(l.resolve("Jarvis")), Some("Jarvis"));
        assert_eq!(nom(l.resolve(" mawena. ")), Some("Mawena"));
        assert_eq!(l.names(), ["Jarvis", "Mawena"]);
    }

    #[test]
    fn projet_par_alias() {
        let l = connus();
        assert_eq!(nom(l.resolve("L'Assistant")), Some("Jarvis"));
        let mots = ["ameliore", "l", "assistant", "vite"];
        let (p, debut, fin) = l.find_in(&mots).unwrap();
        assert_eq!((p.name.as_str(), debut, fin), ("Jarvis", 1, 3));
    }

    #[test]
    fn projet_flou() {
        let l = connus();
        assert_eq!(nom(l.resolve("mawenna")), Some("Mawena"));
        assert_eq!(nom(l.resolve("Jarvi")), Some("Jarvis"));
        assert_eq!(nom(l.resolve("Mawéna")), Some("Mawena"));
    }

    #[test]
    fn projet_ambigu_refuse() {
        let l = ProjectList::new(vec![projet("Mawena App", &[]), projet("Mawena Api", &[])]);
        assert_eq!(l.resolve("mawena"), None);
        assert_eq!(nom(l.resolve("mawena api")), Some("Mawena Api"));
    }

    #[test]
    fn projet_inconnu() {
        let l = connus();
        assert_eq!(l.resolve("Zorglub"), None);
        assert_eq!(l.resolve(""), None);
        assert_eq!(l.resolve("a"), None);
        assert_eq!(l.find_in(&["corrige", "le", "bug"]), None);
    }

    #[test]
    fn nom_jarvis_seulement_apres_projet() {
        let l = connus();
        assert_eq!(l.find_in(&["teste", "le", "micro", "jarvis"]), None);
        assert_eq!(l.find_in(&["les", "tests", "de", "jarvis"]), None);
        let mots = ["finis", "le", "projet", "jarvis"];
        let (p, debut, fin) = l.find_in(&mots).unwrap();
        assert_eq!((p.name.as_str(), debut, fin), ("Jarvis", 3, 4));
        // L'alias compte partout, les autres noms aussi.
        assert!(l.find_in(&["corrige", "l", "assistant"]).is_some());
        assert!(l.find_in(&["corrige", "mawena"]).is_some());
    }

    #[test]
    fn nom_proche_d_un_projet() {
        let l = connus();
        assert!(l.is_near("Jarvoss"));
        assert!(l.is_near("mawenna"));
        assert!(!l.is_near("vacances"));
        assert!(!l.is_near("maison"));
        assert!(!l.is_near("Zorglub"));
        assert!(!l.is_near("ab"));
        let ambigus = ProjectList::new(vec![projet("Mawena App", &[]), projet("Mawena Api", &[])]);
        assert!(ambigus.is_near("mawena"));
        assert_eq!(distance("jarvis", "jarvoss"), 2);
        assert_eq!(distance("", "abc"), 3);
    }
}
