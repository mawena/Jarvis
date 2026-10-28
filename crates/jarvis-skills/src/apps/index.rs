//! Index of installed applications read from `.desktop` files.

use fuzzy_matcher::FuzzyMatcher;
use fuzzy_matcher::skim::SkimMatcherV2;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const MIN_SCORE: i64 = 50;
const FILLERS: [&str; 4] = ["s il te plait", "stp", "l application", "le jeu"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppEntry {
    /// File name without `.desktop`.
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    pub wm_class: Option<String>,
}

#[derive(Debug, Default)]
pub struct AppIndex {
    pub entries: Vec<AppEntry>,
}

impl AppIndex {
    pub fn default_dirs() -> Vec<PathBuf> {
        let mut dirs = Vec::new();
        if let Some(home) = std::env::var_os("HOME") {
            let home = PathBuf::from(home);
            dirs.push(home.join(".local/share/applications"));
            dirs.push(home.join(".local/share/flatpak/exports/share/applications"));
        }
        for d in [
            "/var/lib/flatpak/exports/share/applications",
            "/var/lib/snapd/desktop/applications",
            "/usr/local/share/applications",
            "/usr/share/applications",
        ] {
            dirs.push(PathBuf::from(d));
        }
        dirs
    }

    /// Load entries. Missing directories are skipped; on a duplicate id the first directory wins.
    pub fn load(dirs: &[PathBuf]) -> AppIndex {
        let mut entries: Vec<AppEntry> = Vec::new();
        for dir in dirs {
            let Ok(read) = std::fs::read_dir(dir) else {
                continue;
            };
            let mut paths: Vec<PathBuf> = read
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "desktop"))
                .collect();
            paths.sort();
            for path in paths {
                if let Some(entry) = parse_entry(&path)
                    && !entries.iter().any(|e| e.id == entry.id)
                {
                    entries.push(entry);
                }
            }
        }
        AppIndex { entries }
    }

    /// Resolve a spoken name: alias, then exact match, then fuzzy match.
    pub fn find(&self, query: &str, aliases: &HashMap<String, String>) -> Option<&AppEntry> {
        let query = query.trim().to_lowercase();
        if let Some(target) = aliases.get(&query) {
            let target = target.to_lowercase();
            if let Some(e) = self.entries.iter().find(|e| e.id.to_lowercase() == target) {
                return Some(e);
            }
        }
        let cleaned = strip_fillers(&query);
        if cleaned.is_empty() {
            return None;
        }
        if let Some(e) = self
            .entries
            .iter()
            .find(|e| e.name.to_lowercase() == cleaned || e.id.to_lowercase() == cleaned)
        {
            return Some(e);
        }
        let matcher = SkimMatcherV2::default();
        // Names first; ids only when no name reaches the threshold.
        best_fuzzy(&self.entries, &matcher, &cleaned, |e| &e.name)
            .or_else(|| best_fuzzy(&self.entries, &matcher, &cleaned, |e| &e.id))
    }
}

/// Best fuzzy match on one field: highest score, then shortest field, then index order.
fn best_fuzzy<'a>(
    entries: &'a [AppEntry],
    matcher: &SkimMatcherV2,
    query: &str,
    field: impl Fn(&AppEntry) -> &str,
) -> Option<&'a AppEntry> {
    let mut best: Option<(i64, usize, &AppEntry)> = None;
    for e in entries {
        let text = field(e).to_lowercase();
        let Some(score) = matcher.fuzzy_match(&text, query) else {
            continue;
        };
        if score < MIN_SCORE {
            continue;
        }
        let len = text.chars().count();
        let better = match best {
            None => true,
            Some((bs, bl, _)) => score > bs || (score == bs && len < bl),
        };
        if better {
            best = Some((score, len, e));
        }
    }
    best.map(|(_, _, e)| e)
}

/// Remove ignored filler phrases (whole words only) from a normalized query.
fn strip_fillers(query: &str) -> String {
    let mut s = format!(" {query} ");
    for f in FILLERS {
        let pat = format!(" {f} ");
        // Twice: adjacent occurrences share their separating space.
        s = s.replace(&pat, " ").replace(&pat, " ");
    }
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Parse the `[Desktop Entry]` group by hand (non-localized keys only).
fn parse_entry(path: &Path) -> Option<AppEntry> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut in_group = false;
    let mut kv: HashMap<&str, &str> = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_group = line == "[Desktop Entry]";
        } else if in_group && let Some((k, v)) = line.split_once('=') {
            kv.entry(k.trim()).or_insert(v.trim());
        }
    }
    if kv.get("Type").copied() != Some("Application")
        || kv.get("NoDisplay").copied() == Some("true")
        || kv.get("Hidden").copied() == Some("true")
        || !kv.contains_key("Exec")
    {
        return None;
    }
    Some(AppEntry {
        id: path.file_stem()?.to_string_lossy().into_owned(),
        name: kv.get("Name")?.to_string(),
        path: path.to_path_buf(),
        wm_class: kv
            .get("StartupWMClass")
            .filter(|v| !v.is_empty())
            .map(|v| v.to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flou() -> AppIndex {
        AppIndex::load(&[
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/applications_flou")
        ])
    }

    #[test]
    fn nom_prioritaire_sur_id() {
        let idx = flou();
        let e = idx.find("chrome", &HashMap::new()).unwrap();
        assert_eq!(e.id, "google-chrome");
    }

    #[test]
    fn seuil_flou_epingle() {
        assert!(flou().find("xyzzy", &HashMap::new()).is_none());
    }
}
