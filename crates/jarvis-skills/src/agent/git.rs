//! Garde-fous git : arbre propre, worktree, branche, vérification après coup (tâche 3).
//!
//! Fonctions synchrones (à appeler dans `spawn_blocking`), toujours `git -C <dossier>`.
//! Rien n'est jamais poussé ni fusionné ; la copie de travail de Charles n'est pas touchée.
//!
//! Chaque git de Jarvis passe par `timeout` (tout le groupe est tué au délai),
//! sans hooks ni fsmonitor du dépôt, sans demande de mot de passe. Dans le worktree
//! d'une tâche, le dossier git est celui relevé à sa création : un fichier `.git`
//! réécrit par l'agent n'est jamais suivi.

use crate::agent::types::{TaskId, has_wake_word};
use crate::runner::{CommandOutput, CommandRunner};
use crate::text::fold;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Adresse de push qui ne mène nulle part : aucun transport ne la connaît.
const NO_PUSH_URL: &str = "jarvis-push-interdit://";

/// Variables retirées de l'environnement de l'agent (identifiants, demandes de mot de passe).
const REMOVED_VARS: &[&str] = &[
    "SSH_AUTH_SOCK",
    "GH_TOKEN",
    "GITHUB_TOKEN",
    "GIT_ASKPASS",
    "SSH_ASKPASS",
    "GIT_SSH",
    "GIT_CONFIG_PARAMETERS",
];

/// Délai d'une commande git de Jarvis.
pub const GIT_TIMEOUT: Duration = Duration::from_secs(30);
/// Délai du commit de fin (signature comprise).
pub const COMMIT_TIMEOUT: Duration = Duration::from_secs(60);
/// Après le délai : `TERM` au groupe, puis `KILL` après ce répit.
const KILL_AFTER: &str = "--kill-after=5";

/// Options de chaque git de Jarvis : ni hooks ni fsmonitor venus du dépôt.
pub const SAFE_CONFIG: &[&str] = &[
    "-c",
    "core.fsmonitor=false",
    "-c",
    "core.hooksPath=/dev/null",
];
/// Environnement de chaque git de Jarvis : aucune demande qui bloque.
const QUIET_ENV: &[&str] = &["GIT_TERMINAL_PROMPT=0", "SSH_ASKPASS_REQUIRE=never"];

/// Mots retirés du nom de branche.
const STOP_WORDS: &[&str] = &[
    "a", "au", "aux", "avec", "c", "ce", "ces", "cet", "cette", "d", "dans", "de", "des", "du",
    "elle", "en", "est", "et", "il", "j", "je", "l", "la", "le", "les", "leur", "leurs", "m", "ma",
    "me", "merci", "mes", "mon", "n", "ne", "nos", "notre", "ou", "par", "pas", "plait", "pour",
    "projet", "qu", "que", "qui", "s", "sa", "se", "ses", "son", "stp", "sur", "svp", "t", "ta",
    "te", "tes", "ton", "tu", "un", "une", "vos", "votre", "y",
];

/// Pourquoi une tâche n'est pas lancée.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    NotARepo,
    Dirty {
        files: usize,
    },
    NoBase(String),
    /// Git a échoué (détail pour le journal).
    Git(String),
}

impl Refusal {
    /// Phrase dite à Charles.
    pub fn phrase(&self, project: &crate::agent::types::SpokenName) -> String {
        match self {
            Refusal::NotARepo => format!(
                "Le dossier {} n'est pas un dépôt git : je ne lance rien.",
                project.of_project()
            ),
            Refusal::Dirty { .. } => format!(
                "{} a des modifications non enregistrées : je ne lance rien.",
                project.the_project_capitalized()
            ),
            Refusal::NoBase(base) => format!(
                "{} n'a pas de branche {base} : je ne lance rien.",
                project.the_project_capitalized()
            ),
            Refusal::Git(_) => format!(
                "Git a échoué sur {} : je ne lance rien.",
                project.the_project()
            ),
        }
    }
}

/// Noms git flow du dépôt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlowNames {
    /// Branche de départ (develop).
    pub base: String,
    /// Préfixe des branches de fonctionnalité (`feature/`).
    pub prefix: String,
    /// Branches git flow principales (base, develop, main).
    pub protected: Vec<String>,
}

/// Worktree d'une tâche et son dossier git, relevé à la création.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskTree {
    pub worktree: PathBuf,
    pub git_dir: PathBuf,
}

/// Arguments de `timeout … env … git <options sûres> [extra] -C dir args…`.
fn command(dir: &Path, limit: Duration, extra: &[String], args: &[&str]) -> Vec<String> {
    let mut all = vec![
        KILL_AFTER.to_string(),
        format!("{:.3}s", limit.as_secs_f64()),
        "env".to_string(),
    ];
    all.extend(QUIET_ENV.iter().map(|v| v.to_string()));
    all.push("git".into());
    all.extend(SAFE_CONFIG.iter().map(|v| v.to_string()));
    all.extend(extra.iter().cloned());
    all.push("-C".into());
    all.push(dir.display().to_string());
    all.extend(args.iter().map(|a| a.to_string()));
    all
}

/// Lance git dans `dir` (dépôt principal), borné par `GIT_TIMEOUT`.
fn git(r: &dyn CommandRunner, dir: &Path, args: &[&str]) -> std::io::Result<CommandOutput> {
    r.run("timeout", &command(dir, GIT_TIMEOUT, &[], args))
}

/// Lance git dans le worktree d'une tâche, avec son dossier git fixé.
fn tree_git(
    r: &dyn CommandRunner,
    tree: &TaskTree,
    limit: Duration,
    args: &[&str],
) -> std::io::Result<CommandOutput> {
    let extra = [
        format!("--git-dir={}", tree.git_dir.display()),
        format!("--work-tree={}", tree.worktree.display()),
    ];
    r.run("timeout", &command(&tree.worktree, limit, &extra, args))
}

/// Sortie si la commande réussit, sans espaces finaux.
fn ok_out(out: std::io::Result<CommandOutput>) -> Option<String> {
    match out {
        Ok(out) if out.success => Some(out.stdout.trim_end().to_string()),
        _ => None,
    }
}

/// Sortie de git si la commande réussit, sans espaces finaux.
fn git_ok(r: &dyn CommandRunner, dir: &Path, args: &[&str]) -> Option<String> {
    ok_out(git(r, dir, args))
}

/// Valeur de config non vide.
fn config(r: &dyn CommandRunner, repo: &Path, key: &str) -> Option<String> {
    git_ok(r, repo, &["config", "--get", key]).filter(|v| !v.is_empty())
}

/// Lignes non vides.
fn lines(text: &str) -> impl Iterator<Item = &str> {
    text.lines().map(str::trim).filter(|l| !l.is_empty())
}

/// Noms lus dans `gitflow.*` ; `base_override` (réglage du projet) passe avant.
pub fn flow_names(r: &dyn CommandRunner, repo: &Path, base_override: Option<&str>) -> FlowNames {
    let develop = config(r, repo, "gitflow.branch.develop").unwrap_or_else(|| "develop".into());
    let prefix = config(r, repo, "gitflow.prefix.feature").unwrap_or_else(|| "feature/".into());
    let base = base_override
        .filter(|b| !b.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| develop.clone());
    let masters = match config(r, repo, "gitflow.branch.master") {
        Some(m) => vec![m],
        None => vec!["main".into(), "master".into()],
    };
    let mut protected: Vec<String> = Vec::new();
    for b in std::iter::once(base.clone())
        .chain(std::iter::once(develop))
        .chain(masters)
    {
        if !protected.contains(&b) {
            protected.push(b);
        }
    }
    FlowNames {
        base,
        prefix,
        protected,
    }
}

/// Refuse un dossier hors dépôt, ou un dépôt aux modifications suivies non
/// enregistrées. Les fichiers non suivis sont ignorés.
pub fn check_clean(r: &dyn CommandRunner, repo: &Path) -> Result<(), Refusal> {
    let inside = git(r, repo, &["rev-parse", "--is-inside-work-tree"])
        .map_err(|e| Refusal::Git(format!("git introuvable : {e}")))?;
    if !inside.success || inside.stdout.trim() == "false" {
        return Err(Refusal::NotARepo);
    }
    let status = git(r, repo, &["status", "--porcelain", "--untracked-files=no"])
        .map_err(|e| Refusal::Git(format!("git introuvable : {e}")))?;
    if !status.success {
        return Err(Refusal::Git("git status a échoué".into()));
    }
    match lines(&status.stdout).count() {
        0 => Ok(()),
        files => Err(Refusal::Dirty { files }),
    }
}

/// Nom de branche : `jarvis-` + 4 mots au plus de la consigne, sans accents,
/// sans mots vides ni nom du projet, 40 caractères au plus.
pub fn slug(instruction: &str, project: &str) -> String {
    const MAX: usize = 40;
    let project_words: Vec<String> = fold(project).split(' ').map(ascii_word).collect();
    let mut out = String::from("jarvis");
    let mut count = 0;
    for word in fold(instruction).split(' ').map(ascii_word) {
        if count == 4 {
            break;
        }
        if word.is_empty() || STOP_WORDS.contains(&word.as_str()) || project_words.contains(&word) {
            continue;
        }
        if out.len() + 1 + word.len() > MAX {
            // Premier mot trop long : coupé ; sinon on s'arrête là.
            if count == 0 {
                out.push('-');
                out.push_str(&word[..MAX - out.len()]);
            }
            break;
        }
        out.push('-');
        out.push_str(&word);
        count += 1;
    }
    if out == "jarvis" {
        out.push_str("-tache");
    }
    out
}

/// Mot en ASCII seulement (`œ` → `oe`, le reste hors ASCII est retiré).
fn ascii_word(word: &str) -> String {
    let mut out = String::new();
    for c in word.chars() {
        match c {
            'œ' => out.push_str("oe"),
            'æ' => out.push_str("ae"),
            'ñ' => out.push('n'),
            c if c.is_ascii_alphanumeric() => out.push(c),
            _ => {}
        }
    }
    out
}

/// Worktree prêt pour l'agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prepared {
    pub branch: String,
    pub worktree: PathBuf,
    /// Dossier git du worktree, relevé avant que l'agent n'y touche.
    pub git_dir: PathBuf,
    /// SHA de la base au départ.
    pub base_sha: String,
}

impl Prepared {
    pub fn tree(&self) -> TaskTree {
        TaskTree {
            worktree: self.worktree.clone(),
            git_dir: self.git_dir.clone(),
        }
    }
}

/// Crée la branche `<prefix><slug>` (suffixe `-2`, `-3`… si elle existe) depuis
/// la base, dans un worktree `<worktree_dir>/<projet>-<slug>-<id8>`. La copie de
/// travail du dépôt ne change pas.
pub fn prepare(
    r: &dyn CommandRunner,
    repo: &Path,
    names: &FlowNames,
    slug: &str,
    worktree_dir: &Path,
    id: &TaskId,
) -> Result<Prepared, Refusal> {
    let base_ref = format!("{}^{{commit}}", names.base);
    let base_sha = git_ok(r, repo, &["rev-parse", "--verify", "--quiet", &base_ref])
        .filter(|s| !s.is_empty())
        .ok_or_else(|| Refusal::NoBase(names.base.clone()))?;

    let mut name = slug.to_string();
    let mut n = 1;
    loop {
        let full = format!("refs/heads/{}{name}", names.prefix);
        let exists = git(r, repo, &["rev-parse", "--verify", "--quiet", &full])
            .map_err(|e| Refusal::Git(format!("git introuvable : {e}")))?
            .success;
        if !exists {
            break;
        }
        n += 1;
        if n > 99 {
            return Err(Refusal::Git("aucun nom de branche libre".into()));
        }
        name = format!("{slug}-{n}");
    }
    let branch = format!("{}{name}", names.prefix);

    let project = repo
        .file_name()
        .map(|f| fold(&f.to_string_lossy()).replace(' ', "-"))
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| "projet".into());
    let id8: String = id
        .as_str()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(8)
        .collect();
    let worktree = worktree_dir.join(format!("{project}-{name}-{id8}"));
    std::fs::create_dir_all(worktree_dir)
        .map_err(|e| Refusal::Git(format!("dossier des worktrees : {e}")))?;

    let wt = worktree.display().to_string();
    let out = git(
        r,
        repo,
        &["worktree", "add", "-q", "-b", &branch, &wt, &base_sha],
    )
    .map_err(|e| Refusal::Git(format!("git introuvable : {e}")))?;
    if !out.success {
        return Err(Refusal::Git("git worktree add a échoué".into()));
    }
    let git_dir = git_ok(r, &worktree, &["rev-parse", "--absolute-git-dir"])
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| Refusal::Git("dossier git du worktree introuvable".into()))?;
    Ok(Prepared {
        branch,
        worktree,
        git_dir,
        base_sha,
    })
}

/// Retire un worktree tout juste créé et sa branche, si elle n'a pas bougé de
/// la base (tâche annulée avant son départ).
pub fn discard(r: &dyn CommandRunner, repo: &Path, prepared: &Prepared) -> Result<(), String> {
    let wt = prepared.worktree.display().to_string();
    let full = format!("refs/heads/{}", prepared.branch);
    for (args, what) in [
        (
            &["worktree", "remove", "--force", wt.as_str()][..],
            "git worktree remove",
        ),
        (
            &[
                "update-ref",
                "-d",
                full.as_str(),
                prepared.base_sha.as_str(),
            ][..],
            "git update-ref -d",
        ),
    ] {
        let out = git(r, repo, args).map_err(|e| format!("git introuvable : {e}"))?;
        if !out.success {
            return Err(format!("{what} a échoué"));
        }
    }
    Ok(())
}

/// Références relevées avant le lancement.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RefSnapshot {
    /// `refs/heads/**`, `refs/tags/**` et `refs/remotes/**` → SHA.
    pub refs: BTreeMap<String, String>,
}

/// Toutes les branches, tags et références distantes ; `None` si git échoue.
pub fn snapshot(r: &dyn CommandRunner, repo: &Path) -> Option<RefSnapshot> {
    let out = git_ok(
        r,
        repo,
        &[
            "for-each-ref",
            "--format=%(refname) %(objectname)",
            "refs/heads/",
            "refs/tags/",
            "refs/remotes/",
        ],
    )?;
    let refs = lines(&out)
        .filter_map(|l| l.split_once(' '))
        .map(|(name, sha)| (name.to_string(), sha.to_string()))
        .collect();
    Some(RefSnapshot { refs })
}

/// Garde-fou enfreint pendant la tâche.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Violation {
    /// Branche, tag ou branche distante créé, déplacé ou supprimé (nom complet).
    RefChanged(String),
    /// Le worktree n'est plus sur sa branche (`None` : HEAD détachée).
    BranchChanged {
        expected: String,
        actual: Option<String>,
    },
    /// La base n'est plus un ancêtre de la branche.
    BaseLost,
    /// Le commit de Jarvis n'a pas la forme attendue (parent, branche).
    CommitUnexpected,
}

impl Violation {
    /// Sujet dit pour une référence changée (« la branche develop »).
    pub fn ref_subject(&self, task_branch: &str) -> Option<String> {
        let Violation::RefChanged(full) = self else {
            return None;
        };
        if let Some(b) = full.strip_prefix("refs/heads/") {
            return Some(match speakable(b) {
                Some(b) => format!("la branche {b}"),
                None => "une branche d'une autre tâche".into(),
            });
        }
        if let Some(t) = full.strip_prefix("refs/tags/") {
            return Some(match speakable(t) {
                Some(t) => format!("le tag {t}"),
                None => "un tag".into(),
            });
        }
        let short = full.strip_prefix("refs/remotes/").unwrap_or(full);
        // Branche de la tâche sur un remote : poussée, traitée à part.
        if short.split_once('/').is_some_and(|(_, b)| b == task_branch) {
            return None;
        }
        Some(match speakable(short) {
            Some(s) => format!("la branche distante {s}"),
            None => "une branche distante".into(),
        })
    }

    /// Phrase courte, sans majuscule ni point (« Attention : <phrase>. »),
    /// jamais avec le mot de réveil.
    pub fn phrase(&self, task_branch: &str) -> String {
        if let Some(subject) = self.ref_subject(task_branch) {
            return format!("{subject} a changé pendant la tâche");
        }
        match self {
            Violation::RefChanged(full) => {
                let short = full.strip_prefix("refs/remotes/").unwrap_or(full);
                let remote = short.split_once('/').map_or(short, |(r, _)| r);
                match speakable(remote) {
                    Some(remote) => format!("la branche de la tâche a été poussée sur {remote}"),
                    None => "la branche de la tâche a été poussée".into(),
                }
            }
            Violation::BranchChanged {
                actual: Some(a), ..
            } => match speakable(a) {
                Some(a) => format!("l'agent a quitté la branche de la tâche pour {a}"),
                None => "l'agent a quitté la branche de la tâche".into(),
            },
            Violation::BranchChanged { actual: None, .. } => {
                "l'agent a quitté la branche de la tâche".into()
            }
            Violation::BaseLost => "la branche de la tâche ne part plus de sa base".into(),
            Violation::CommitUnexpected => {
                "le commit de fin n'est pas celui que j'attendais".into()
            }
        }
    }

    /// Détail écrit (notification) : nom complet de la référence.
    pub fn detail(&self) -> Option<String> {
        match self {
            Violation::RefChanged(full) => Some(full.clone()),
            Violation::BranchChanged {
                actual: Some(a), ..
            } => Some(format!("HEAD : {a}")),
            _ => None,
        }
    }
}

/// Nom dicible : `None` s'il contient le mot de réveil.
fn speakable(name: &str) -> Option<&str> {
    (!has_wake_word(name)).then_some(name)
}

/// Vérification après coup : toute branche, tag ou branche distante changé (sauf
/// la branche de la tâche), branche du worktree, base toujours ancêtre. `Err` si
/// la vérification n'a pas pu se faire.
pub fn verify(
    r: &dyn CommandRunner,
    repo: &Path,
    tree: &TaskTree,
    before: &RefSnapshot,
    branch: &str,
    base_sha: &str,
) -> Result<Vec<Violation>, String> {
    let after = snapshot(r, repo).ok_or("relecture des références impossible")?;
    let own = format!("refs/heads/{branch}");
    let names: BTreeSet<&String> = before.refs.keys().chain(after.refs.keys()).collect();
    let mut out: Vec<Violation> = names
        .into_iter()
        .filter(|name| **name != own && before.refs.get(*name) != after.refs.get(*name))
        .map(|name| Violation::RefChanged(name.clone()))
        .collect();
    let actual = ok_out(tree_git(
        r,
        tree,
        GIT_TIMEOUT,
        &["symbolic-ref", "--quiet", "--short", "HEAD"],
    ))
    .filter(|s| !s.is_empty());
    if actual.as_deref() != Some(branch) {
        out.push(Violation::BranchChanged {
            expected: branch.to_string(),
            actual,
        });
    }
    let ancestor = tree_git(
        r,
        tree,
        GIT_TIMEOUT,
        &["merge-base", "--is-ancestor", base_sha, "HEAD"],
    )
    .map(|o| o.success)
    .unwrap_or(false);
    if !ancestor {
        out.push(Violation::BaseLost);
    }
    Ok(out)
}

/// Environnement git de l'agent.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitEnv {
    /// Variables posées.
    pub set: Vec<(String, String)>,
    /// Variables retirées.
    pub remove: Vec<String>,
}

/// Environnement qui empêche tout push, indépendamment des permissions de
/// Claude Code : chaque remote pousse vers une adresse sans transport (les
/// `pushurl` existantes y sont réécrites), aucun identifiant https ni ssh.
pub fn no_push_env(r: &dyn CommandRunner, repo: &Path) -> GitEnv {
    let mut config: Vec<(String, String)> = Vec::new();
    let remotes = git_ok(r, repo, &["remote"]).unwrap_or_default();
    let remotes: BTreeSet<&str> = lines(&remotes).collect();
    for remote in remotes {
        let key = format!("remote.{remote}.pushurl");
        // `pushurl` est une liste : une valeur ajoutée ne remplace pas les
        // anciennes, qui sont donc réécrites vers l'adresse sans transport.
        if let Some(existing) = git_ok(r, repo, &["config", "--get-all", &key]) {
            for url in lines(&existing) {
                if url != NO_PUSH_URL {
                    config.push((format!("url.{NO_PUSH_URL}.insteadOf"), url.to_string()));
                }
            }
        }
        config.push((key, NO_PUSH_URL.to_string()));
    }
    // Valeur vide : vide la liste des assistants d'identifiants.
    config.push(("credential.helper".into(), String::new()));

    let mut set = vec![("GIT_CONFIG_COUNT".to_string(), config.len().to_string())];
    for (i, (k, v)) in config.into_iter().enumerate() {
        set.push((format!("GIT_CONFIG_KEY_{i}"), k));
        set.push((format!("GIT_CONFIG_VALUE_{i}"), v));
    }
    set.push(("GIT_SSH_COMMAND".into(), "false".into()));
    set.push(("GIT_TERMINAL_PROMPT".into(), "0".into()));
    GitEnv {
        set,
        remove: REMOVED_VARS.iter().map(|v| v.to_string()).collect(),
    }
}

/// Fichiers changés depuis la base (enregistrés ou non) et nouveaux fichiers non
/// ignorés, triés. `Err` si git échoue.
pub fn changed_files(
    r: &dyn CommandRunner,
    tree: &TaskTree,
    base_sha: &str,
) -> Result<Vec<String>, String> {
    let mut files = BTreeSet::new();
    for args in [
        &["diff", "--name-only", "-z", base_sha][..],
        &["ls-files", "-z", "--others", "--exclude-standard"][..],
    ] {
        match tree_git(r, tree, GIT_TIMEOUT, args) {
            Ok(out) if out.success => files.extend(
                out.stdout
                    .split('\0')
                    .filter(|f| !f.is_empty())
                    .map(str::to_string),
            ),
            _ => return Err(format!("git {} a échoué", args[0])),
        }
    }
    Ok(files.into_iter().collect())
}

/// Un seul commit de tout le travail, avec la config git de Charles (identité et
/// signature comprises), sans hooks ni ligne d'attribution. Le commit est borné
/// par `limit`. Renvoie le SHA de HEAD avant le commit.
pub fn commit_all(
    r: &dyn CommandRunner,
    tree: &TaskTree,
    message: &str,
    limit: Duration,
) -> Result<String, String> {
    let parent = ok_out(tree_git(
        r,
        tree,
        GIT_TIMEOUT,
        &["rev-parse", "--verify", "--quiet", "HEAD"],
    ))
    .filter(|s| !s.is_empty())
    .ok_or("HEAD introuvable")?;
    for (args, what, limit) in [
        (&["add", "-A"][..], "git add", GIT_TIMEOUT),
        (
            &["commit", "-q", "--no-verify", "-m", message][..],
            "git commit",
            limit,
        ),
    ] {
        let out = tree_git(r, tree, limit, args).map_err(|e| format!("git introuvable : {e}"))?;
        if !out.success {
            return Err(format!("{what} a échoué"));
        }
    }
    Ok(parent)
}

/// Après le commit de Jarvis : la branche de la tâche porte un nouveau commit
/// dont le seul parent est `parent`, et le worktree est toujours dessus.
pub fn check_commit(
    r: &dyn CommandRunner,
    tree: &TaskTree,
    branch: &str,
    parent: &str,
) -> Vec<Violation> {
    let mut out = Vec::new();
    let full = format!("refs/heads/{branch}");
    let parents = ok_out(tree_git(
        r,
        tree,
        GIT_TIMEOUT,
        &["rev-list", "--parents", "-n", "1", &full],
    ))
    .unwrap_or_default();
    let shas: Vec<&str> = parents.split_whitespace().collect();
    if !matches!(shas.as_slice(), [new, p] if *new != parent && *p == parent) {
        out.push(Violation::CommitUnexpected);
    }
    let actual = ok_out(tree_git(
        r,
        tree,
        GIT_TIMEOUT,
        &["symbolic-ref", "--quiet", "--short", "HEAD"],
    ))
    .filter(|s| !s.is_empty());
    if actual.as_deref() != Some(branch) {
        out.push(Violation::BranchChanged {
            expected: branch.to_string(),
            actual,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::testing::{git, temp_repo};
    use crate::agent::types::SpokenName;
    use crate::runner::{CommandOutput, FakeRunner, SystemRunner};
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    const R: SystemRunner = SystemRunner;

    fn id() -> TaskId {
        TaskId("0123abcd-4567-89ef".into())
    }

    fn named(name: &str) -> SpokenName {
        SpokenName::Name(name.into())
    }

    /// Script exécutable `path` (`#!/bin/sh` + `body`).
    fn script(path: &Path, body: &str) {
        std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn snap(repo: &Path) -> RefSnapshot {
        snapshot(&R, repo).expect("relevé des références")
    }

    fn check(repo: &Path, p: &Prepared, before: &RefSnapshot) -> Vec<Violation> {
        verify(&R, repo, &p.tree(), before, &p.branch, &p.base_sha).unwrap()
    }

    /// Dépôt temporaire et son worktree de tâche.
    fn prepared() -> (tempfile::TempDir, PathBuf, Prepared) {
        let (dir, repo) = temp_repo();
        let names = flow_names(&R, &repo, None);
        let p = prepare(
            &R,
            &repo,
            &names,
            "jarvis-finis-tests",
            &dir.path().join("worktrees"),
            &id(),
        )
        .unwrap();
        (dir, repo, p)
    }

    /// `git -C dir args…` avec l'environnement de l'agent ; vrai si git réussit.
    fn git_with_env(dir: &Path, env: &GitEnv, args: &[&str]) -> bool {
        let mut cmd = Command::new("git");
        cmd.arg("-C").arg(dir).args(args);
        for (k, v) in &env.set {
            cmd.env(k, v);
        }
        for k in &env.remove {
            cmd.env_remove(k);
        }
        cmd.stdin(std::process::Stdio::null())
            .output()
            .expect("git introuvable")
            .status
            .success()
    }

    /// Dépôt nu local, dans un dossier temporaire, ajouté comme `origin`.
    fn local_remote(dir: &Path, repo: &Path) -> PathBuf {
        let bare = dir.join("distant.git");
        let out = Command::new("git")
            .args(["init", "-q", "--bare", "-b", "develop"])
            .arg(&bare)
            .output()
            .unwrap();
        assert!(out.status.success());
        git(repo, &["remote", "add", "origin", bare.to_str().unwrap()]);
        bare
    }

    fn branch_exists(repo: &Path, branch: &str) -> bool {
        Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["rev-parse", "--verify", "--quiet"])
            .arg(format!("refs/heads/{branch}"))
            .output()
            .unwrap()
            .status
            .success()
    }

    #[test]
    fn arbre_propre_accepte() {
        let (_dir, repo) = temp_repo();
        assert_eq!(check_clean(&R, &repo), Ok(()));
    }

    #[test]
    fn arbre_modifie_refuse() {
        let (_dir, repo) = temp_repo();
        std::fs::write(repo.join("lib.rs"), "pub fn un() -> u32 { 2 }\n").unwrap();
        std::fs::write(repo.join("README.md"), "changé\n").unwrap();
        let refusal = check_clean(&R, &repo).unwrap_err();
        assert_eq!(refusal, Refusal::Dirty { files: 2 });
        assert_eq!(
            refusal.phrase(&named("Mawena")),
            "Le projet Mawena a des modifications non enregistrées : je ne lance rien."
        );
        assert_eq!(
            refusal.phrase(&SpokenName::Article("l'assistant".into())),
            "L'assistant a des modifications non enregistrées : je ne lance rien."
        );
    }

    #[test]
    fn fichier_indexe_refuse() {
        let (_dir, repo) = temp_repo();
        std::fs::write(repo.join("nouveau.rs"), "// nouveau\n").unwrap();
        git(&repo, &["add", "nouveau.rs"]);
        assert_eq!(check_clean(&R, &repo), Err(Refusal::Dirty { files: 1 }));
    }

    #[test]
    fn fichier_non_suivi_ignore() {
        let (_dir, repo) = temp_repo();
        std::fs::write(repo.join("brouillon.txt"), "notes\n").unwrap();
        assert_eq!(check_clean(&R, &repo), Ok(()));
    }

    #[test]
    fn pas_un_depot_refuse() {
        let dir = tempfile::tempdir().unwrap();
        let refusal = check_clean(&R, dir.path()).unwrap_err();
        assert_eq!(refusal, Refusal::NotARepo);
        assert_eq!(
            refusal.phrase(&named("Mawena")),
            "Le dossier du projet Mawena n'est pas un dépôt git : je ne lance rien."
        );
        let absent = dir.path().join("absent");
        assert_eq!(check_clean(&R, &absent), Err(Refusal::NotARepo));
    }

    #[test]
    fn base_develop_absente_refusee() {
        let (dir, repo) = temp_repo();
        let names = flow_names(&R, &repo, Some("inexistante"));
        let worktrees = dir.path().join("worktrees");
        let refusal = prepare(&R, &repo, &names, "jarvis-x", &worktrees, &id()).unwrap_err();
        assert_eq!(refusal, Refusal::NoBase("inexistante".into()));
        assert!(refusal.phrase(&named("Mawena")).contains("inexistante"));
        assert!(!worktrees.exists() || std::fs::read_dir(&worktrees).unwrap().count() == 0);
        assert_eq!(git(&repo, &["branch", "--list", "feature/*"]), "");
    }

    #[test]
    fn noms_git_flow_lus_dans_la_config() {
        let (_dir, repo) = temp_repo();
        git(&repo, &["config", "gitflow.branch.develop", "dev"]);
        git(&repo, &["config", "gitflow.branch.master", "prod"]);
        git(&repo, &["config", "gitflow.prefix.feature", "feat/"]);
        assert_eq!(
            flow_names(&R, &repo, None),
            FlowNames {
                base: "dev".into(),
                prefix: "feat/".into(),
                protected: vec!["dev".into(), "prod".into()],
            }
        );
        // Base imposée par le projet : la branche develop reste protégée.
        let n = flow_names(&R, &repo, Some("integration"));
        assert_eq!(n.base, "integration");
        assert_eq!(n.protected, ["integration", "dev", "prod"]);
    }

    #[test]
    fn noms_par_defaut_sans_git_flow() {
        let (_dir, repo) = temp_repo();
        git(&repo, &["config", "--remove-section", "gitflow.branch"]);
        git(&repo, &["config", "--remove-section", "gitflow.prefix"]);
        assert_eq!(
            flow_names(&R, &repo, None),
            FlowNames {
                base: "develop".into(),
                prefix: "feature/".into(),
                protected: vec!["develop".into(), "main".into(), "master".into()],
            }
        );
    }

    #[test]
    fn slug_court_sans_accents() {
        assert_eq!(
            slug("Finis les tests du projet Jarvis", "Jarvis"),
            "jarvis-finis-tests"
        );
        let long = slug(
            "Corrige l'élément « Ça » dans le cerveau, et améliore la mémoire du système",
            "Mawena",
        );
        assert_eq!(long, "jarvis-corrige-element-ca-cerveau");
        let tres_long = slug(
            "Refactorisation interminablement compliquée extraordinairement",
            "X",
        );
        assert!(tres_long.len() <= 40, "{tres_long}");
        assert!(
            tres_long.starts_with("jarvis-refactorisation"),
            "{tres_long}"
        );
        assert!(!tres_long.ends_with('-'));
        assert_eq!(
            slug("du projet Jarvis, s'il te plaît", "Jarvis"),
            "jarvis-tache"
        );
        assert_eq!(slug("", "Jarvis"), "jarvis-tache");
        assert_eq!(slug("Ajoute 日本 le cœur", "Jarvis"), "jarvis-ajoute-coeur");
    }

    #[test]
    fn worktree_cree_sur_branche_feature() {
        let (dir, repo, p) = prepared();
        assert_eq!(p.branch, "feature/jarvis-finis-tests");
        assert_eq!(
            p.worktree,
            dir.path()
                .join("worktrees")
                .join("projet-jarvis-finis-tests-0123abcd")
        );
        assert_eq!(p.base_sha, git(&repo, &["rev-parse", "develop"]));
        assert_eq!(
            git(&p.worktree, &["branch", "--show-current"]),
            "feature/jarvis-finis-tests"
        );
        assert_eq!(git(&p.worktree, &["rev-parse", "HEAD"]), p.base_sha);
        assert!(p.worktree.join("lib.rs").exists());
    }

    #[test]
    fn branche_existante_suffixe() {
        let (dir, repo) = temp_repo();
        git(&repo, &["branch", "feature/jarvis-x"]);
        git(&repo, &["branch", "feature/jarvis-x-2"]);
        let names = flow_names(&R, &repo, None);
        let p = prepare(&R, &repo, &names, "jarvis-x", &dir.path().join("w"), &id()).unwrap();
        assert_eq!(p.branch, "feature/jarvis-x-3");
        assert!(p.worktree.ends_with("projet-jarvis-x-3-0123abcd"));
    }

    #[test]
    fn copie_de_travail_de_charles_intacte() {
        let (_dir, repo) = temp_repo();
        std::fs::write(repo.join("brouillon.txt"), "notes\n").unwrap();
        let head = git(&repo, &["rev-parse", "HEAD"]);
        let dir2 = tempfile::tempdir().unwrap();
        let names = flow_names(&R, &repo, None);
        let p = prepare(&R, &repo, &names, "jarvis-y", dir2.path(), &id()).unwrap();
        std::fs::write(p.worktree.join("lib.rs"), "changé\n").unwrap();
        assert_eq!(git(&repo, &["branch", "--show-current"]), "develop");
        assert_eq!(git(&repo, &["rev-parse", "HEAD"]), head);
        assert_eq!(git(&repo, &["status", "--porcelain"]), "?? brouillon.txt");
        assert_eq!(
            std::fs::read_to_string(repo.join("lib.rs")).unwrap(),
            "pub fn un() -> u32 {\n    1\n}\n"
        );
    }

    #[test]
    fn push_bloque_par_l_environnement() {
        let (dir, repo) = temp_repo();
        let bare = local_remote(dir.path(), &repo);
        let names = flow_names(&R, &repo, None);
        let p = prepare(&R, &repo, &names, "jarvis-p", &dir.path().join("w"), &id()).unwrap();
        let env = no_push_env(&R, &repo);
        assert!(
            env.set
                .contains(&("GIT_SSH_COMMAND".into(), "false".into()))
        );
        assert!(
            env.set
                .contains(&("GIT_TERMINAL_PROMPT".into(), "0".into()))
        );
        for k in ["SSH_AUTH_SOCK", "GH_TOKEN", "GITHUB_TOKEN", "GIT_ASKPASS"] {
            assert!(env.remove.iter().any(|r| r == k), "{k}");
        }
        for args in [
            vec!["push", "origin", p.branch.as_str()],
            vec!["push", "-u", "origin", "HEAD"],
            vec!["push", "origin", "develop:develop"],
            vec!["push", "--all"],
        ] {
            assert!(!git_with_env(&p.worktree, &env, &args), "{args:?}");
        }
        assert!(!branch_exists(&bare, &p.branch));
        assert!(!branch_exists(&bare, "develop"));
        // Témoin : sans l'environnement, le même push passe.
        assert!(git_with_env(
            &p.worktree,
            &GitEnv::default(),
            &["push", "-q", "origin", &p.branch]
        ));
        assert!(branch_exists(&bare, &p.branch));
    }

    #[test]
    fn pushurl_existante_bloquee() {
        let (dir, repo) = temp_repo();
        let bare = local_remote(dir.path(), &repo);
        git(
            &repo,
            &["config", "remote.origin.pushurl", bare.to_str().unwrap()],
        );
        let env = no_push_env(&R, &repo);
        assert!(!git_with_env(&repo, &env, &["push", "origin", "develop"]));
        assert!(!branch_exists(&bare, "develop"));
    }

    #[test]
    fn push_ssh_explicite_bloque() {
        let (_dir, repo) = temp_repo();
        let env = no_push_env(&R, &repo);
        for url in [
            "ssh://git@jarvis.invalid/projet.git",
            "git@jarvis.invalid:projet.git",
        ] {
            assert!(
                !git_with_env(&repo, &env, &["push", url, "develop"]),
                "{url}"
            );
        }
    }

    #[test]
    fn environnement_neutralise_les_identifiants() {
        let r = FakeRunner::new();
        r.respond(
            "timeout",
            CommandOutput {
                success: true,
                stdout: "origin\namont\n".into(),
            },
        );
        let env = no_push_env(&r, Path::new("/depot"));
        let get = |k: &str| {
            env.set
                .iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.as_str())
        };
        let count: usize = get("GIT_CONFIG_COUNT").unwrap().parse().unwrap();
        let pairs: Vec<(String, String)> = (0..count)
            .map(|i| {
                (
                    get(&format!("GIT_CONFIG_KEY_{i}")).unwrap().to_string(),
                    get(&format!("GIT_CONFIG_VALUE_{i}")).unwrap().to_string(),
                )
            })
            .collect();
        for remote in ["origin", "amont"] {
            assert!(pairs.contains(&(
                format!("remote.{remote}.pushurl"),
                "jarvis-push-interdit://".into()
            )));
        }
        assert!(pairs.contains(&("credential.helper".into(), String::new())));
    }

    #[test]
    fn violation_develop_deplacee_signalee() {
        let (_dir, repo, p) = prepared();
        let before = snap(&repo);
        std::fs::write(p.worktree.join("lib.rs"), "changé\n").unwrap();
        git(&p.worktree, &["commit", "-qam", "travail"]);
        let head = git(&p.worktree, &["rev-parse", "HEAD"]);
        git(&repo, &["update-ref", "refs/heads/develop", &head]);
        let v = check(&repo, &p, &before);
        assert_eq!(v, vec![Violation::RefChanged("refs/heads/develop".into())]);
        assert_eq!(
            v[0].phrase(&p.branch),
            "la branche develop a changé pendant la tâche"
        );
        assert_eq!(v[0].detail().as_deref(), Some("refs/heads/develop"));
    }

    #[test]
    fn violation_branche_poussee_signalee() {
        let (dir, repo) = temp_repo();
        let bare = local_remote(dir.path(), &repo);
        let names = flow_names(&R, &repo, None);
        let p = prepare(&R, &repo, &names, "jarvis-p", &dir.path().join("w"), &id()).unwrap();
        let before = snap(&repo);
        git(&p.worktree, &["push", "-q", "origin", &p.branch]);
        assert!(branch_exists(&bare, &p.branch));
        let v = check(&repo, &p, &before);
        assert_eq!(
            v,
            vec![Violation::RefChanged(
                "refs/remotes/origin/feature/jarvis-p".into()
            )]
        );
        // Jamais le mot de réveil dans la phrase dite.
        assert_eq!(
            v[0].phrase(&p.branch),
            "la branche de la tâche a été poussée sur origin"
        );
    }

    #[test]
    fn toutes_les_branches_et_tags_surveilles() {
        let (_dir, repo, p) = prepared();
        git(&repo, &["branch", "release/x"]);
        git(&repo, &["branch", "feature/jarvis-ancienne"]);
        git(&repo, &["branch", "hotfix/vieux"]);
        let before = snap(&repo);
        assert!(before.refs.contains_key("refs/heads/release/x"));
        // Travail normal sur la branche de la tâche : rien.
        std::fs::write(p.worktree.join("lib.rs"), "changé\n").unwrap();
        git(&p.worktree, &["commit", "-qam", "travail"]);
        assert_eq!(check(&repo, &p, &before), vec![]);
        // Branche release déplacée, tag créé, branches supprimées.
        let head = git(&p.worktree, &["rev-parse", "HEAD"]);
        git(&repo, &["update-ref", "refs/heads/release/x", &head]);
        git(&repo, &["tag", "v9.9", &head]);
        git(&repo, &["branch", "-D", "hotfix/vieux"]);
        git(&repo, &["branch", "-D", "feature/jarvis-ancienne"]);
        let v = check(&repo, &p, &before);
        assert_eq!(
            v,
            vec![
                Violation::RefChanged("refs/heads/feature/jarvis-ancienne".into()),
                Violation::RefChanged("refs/heads/hotfix/vieux".into()),
                Violation::RefChanged("refs/heads/release/x".into()),
                Violation::RefChanged("refs/tags/v9.9".into()),
            ]
        );
        let phrases: Vec<String> = v.iter().map(|x| x.phrase(&p.branch)).collect();
        assert_eq!(
            phrases,
            [
                "une branche d'une autre tâche a changé pendant la tâche",
                "la branche hotfix/vieux a changé pendant la tâche",
                "la branche release/x a changé pendant la tâche",
                "le tag v9.9 a changé pendant la tâche",
            ]
        );
        for phrase in &phrases {
            assert!(!has_wake_word(phrase), "{phrase}");
        }
    }

    #[test]
    fn violation_changement_de_branche() {
        let (_dir, repo, p) = prepared();
        let before = snap(&repo);
        git(&p.worktree, &["switch", "-q", "--orphan", "ailleurs"]);
        let v = check(&repo, &p, &before);
        assert!(
            v.contains(&Violation::BranchChanged {
                expected: p.branch.clone(),
                actual: Some("ailleurs".into()),
            }),
            "{v:?}"
        );
        assert!(v.contains(&Violation::BaseLost), "{v:?}");
        for x in &v {
            assert!(!x.phrase(&p.branch).is_empty());
            assert!(!has_wake_word(&x.phrase(&p.branch)));
        }
    }

    #[test]
    fn verification_impossible_signalee() {
        let (_dir, repo, p) = prepared();
        let before = snap(&repo);
        let r = FakeRunner::new();
        r.respond(
            "timeout",
            CommandOutput {
                success: false,
                stdout: String::new(),
            },
        );
        assert!(verify(&r, &repo, &p.tree(), &before, &p.branch, &p.base_sha).is_err());
        assert!(changed_files(&r, &p.tree(), &p.base_sha).is_err());
        assert!(snapshot(&r, &repo).is_none());
    }

    #[test]
    fn aucune_violation_apres_travail_normal() {
        let (dir, repo) = temp_repo();
        local_remote(dir.path(), &repo);
        let names = flow_names(&R, &repo, None);
        let p = prepare(&R, &repo, &names, "jarvis-n", &dir.path().join("w"), &id()).unwrap();
        let before = snap(&repo);
        std::fs::write(p.worktree.join("lib.rs"), "changé\n").unwrap();
        std::fs::write(p.worktree.join("neuf.rs"), "// neuf\n").unwrap();
        let parent = commit_all(&R, &p.tree(), "wip(jarvis): essai", COMMIT_TIMEOUT).unwrap();
        assert_eq!(parent, p.base_sha);
        assert_eq!(check_commit(&R, &p.tree(), &p.branch, &parent), vec![]);
        assert_eq!(check(&repo, &p, &before), vec![]);
    }

    #[test]
    fn fichiers_modifies_comptes() {
        let (_dir, _repo, p) = prepared();
        let tree = p.tree();
        assert!(changed_files(&R, &tree, &p.base_sha).unwrap().is_empty());
        std::fs::write(p.worktree.join("lib.rs"), "changé\n").unwrap();
        git(&p.worktree, &["commit", "-qam", "un"]);
        std::fs::write(p.worktree.join("README.md"), "changé\n").unwrap();
        std::fs::create_dir(p.worktree.join("src")).unwrap();
        std::fs::write(p.worktree.join("src/neuf.rs"), "// neuf\n").unwrap();
        std::fs::write(p.worktree.join(".gitignore"), "cible/\n").unwrap();
        std::fs::create_dir(p.worktree.join("cible")).unwrap();
        std::fs::write(p.worktree.join("cible/ignoré.o"), "x").unwrap();
        assert_eq!(
            changed_files(&R, &tree, &p.base_sha).unwrap(),
            [".gitignore", "README.md", "lib.rs", "src/neuf.rs"]
        );
    }

    #[test]
    fn commit_final_sans_attribution() {
        let (_dir, repo, p) = prepared();
        std::fs::write(p.worktree.join("lib.rs"), "changé\n").unwrap();
        std::fs::write(p.worktree.join("neuf.rs"), "// neuf\n").unwrap();
        commit_all(
            &R,
            &p.tree(),
            "wip(jarvis): finis les tests",
            COMMIT_TIMEOUT,
        )
        .unwrap();
        let body = git(&p.worktree, &["log", "-1", "--format=%B"]);
        assert_eq!(body, "wip(jarvis): finis les tests");
        assert!(!body.to_lowercase().contains("co-authored-by"));
        assert_eq!(
            git(&p.worktree, &["log", "-1", "--format=%an <%ae>"]),
            "Test Jarvis <test@jarvis.invalid>"
        );
        assert_eq!(git(&p.worktree, &["status", "--porcelain"]), "");
        assert_eq!(
            git(&p.worktree, &["rev-parse", "HEAD~1"]),
            git(&repo, &["rev-parse", "develop"])
        );
        // Rien à enregistrer : erreur dite, pas de commit vide.
        assert!(commit_all(&R, &p.tree(), "vide", COMMIT_TIMEOUT).is_err());
    }

    #[test]
    fn commit_final_sans_hooks_ni_fsmonitor() {
        let (dir, repo, p) = prepared();
        let marker = dir.path().join("hook-lance");
        let mark = format!("echo \"$0\" >> '{}'", marker.display());
        // Hooks suivis par le dépôt (comme .husky), écrits par l'agent.
        let hooks = p.worktree.join(".githooks");
        std::fs::create_dir(&hooks).unwrap();
        for hook in [
            "pre-commit",
            "prepare-commit-msg",
            "commit-msg",
            "post-commit",
            "post-index-change",
        ] {
            script(&hooks.join(hook), &mark);
        }
        git(&repo, &["config", "core.hooksPath", ".githooks"]);
        let fsmonitor = dir.path().join("fsmonitor.sh");
        script(&fsmonitor, &mark);
        git(
            &repo,
            &["config", "core.fsmonitor", fsmonitor.to_str().unwrap()],
        );
        std::fs::write(p.worktree.join("lib.rs"), "changé\n").unwrap();
        let tree = p.tree();
        changed_files(&R, &tree, &p.base_sha).unwrap();
        commit_all(&R, &tree, "wip(jarvis): un", COMMIT_TIMEOUT).unwrap();
        let ran = || std::fs::read_to_string(&marker).unwrap_or_default();
        assert_eq!(ran(), "");

        // Hook planté dans le dossier git du dépôt principal.
        git(&repo, &["config", "--unset", "core.hooksPath"]);
        git(&repo, &["config", "--unset", "core.fsmonitor"]);
        script(&repo.join(".git/hooks/pre-commit"), &mark);
        std::fs::write(p.worktree.join("lib.rs"), "encore\n").unwrap();
        commit_all(&R, &tree, "wip(jarvis): deux", COMMIT_TIMEOUT).unwrap();
        assert_eq!(ran(), "");

        // Témoin : un commit ordinaire lance bien le hook.
        std::fs::write(p.worktree.join("lib.rs"), "témoin\n").unwrap();
        git(&p.worktree, &["commit", "-qam", "témoin"]);
        assert!(ran().contains("pre-commit"), "{}", ran());
    }

    #[test]
    fn dossier_git_fixe_au_depart() {
        let (dir, repo, p) = prepared();
        assert!(
            p.git_dir.starts_with(repo.join(".git/worktrees")),
            "{}",
            p.git_dir.display()
        );
        // L'agent réécrit `.git` vers un faux dépôt.
        let faux = dir.path().join("faux.git");
        let out = Command::new("git")
            .args(["init", "-q", "--bare"])
            .arg(&faux)
            .output()
            .unwrap();
        assert!(out.status.success());
        std::fs::write(
            p.worktree.join(".git"),
            format!("gitdir: {}\n", faux.display()),
        )
        .unwrap();
        std::fs::write(p.worktree.join("lib.rs"), "changé\n").unwrap();
        let parent = commit_all(&R, &p.tree(), "wip(jarvis): vrai", COMMIT_TIMEOUT).unwrap();
        assert_eq!(parent, p.base_sha);
        assert_eq!(
            git(&repo, &["log", "-1", "--format=%s", &p.branch]),
            "wip(jarvis): vrai"
        );
        assert_eq!(check_commit(&R, &p.tree(), &p.branch, &parent), vec![]);
    }

    #[test]
    fn commit_verifie_par_son_parent() {
        let (_dir, repo, p) = prepared();
        let tree = p.tree();
        std::fs::write(p.worktree.join("lib.rs"), "changé\n").unwrap();
        let parent = commit_all(&R, &tree, "wip(jarvis): un", COMMIT_TIMEOUT).unwrap();
        assert_eq!(check_commit(&R, &tree, &p.branch, &parent), vec![]);
        // Pas de nouveau commit : inattendu.
        let head = git(&p.worktree, &["rev-parse", "HEAD"]);
        assert_eq!(
            check_commit(&R, &tree, &p.branch, &head),
            vec![Violation::CommitUnexpected]
        );
        // Un commit de plus glissé par-dessus : inattendu.
        std::fs::write(p.worktree.join("lib.rs"), "glissé\n").unwrap();
        git(&p.worktree, &["commit", "-qam", "glissé"]);
        assert_eq!(
            check_commit(&R, &tree, &p.branch, &parent),
            vec![Violation::CommitUnexpected]
        );
        // Worktree sorti de sa branche.
        git(&p.worktree, &["switch", "-q", "--detach"]);
        let v = check_commit(&R, &tree, &p.branch, &head);
        assert!(
            v.contains(&Violation::BranchChanged {
                expected: p.branch.clone(),
                actual: None
            }),
            "{v:?}"
        );
        // Branche déplacée ensuite : la vérification refaite la voit.
        let before = snap(&repo);
        git(&repo, &["update-ref", "refs/heads/main", &head]);
        assert!(
            check(&repo, &p, &before).contains(&Violation::RefChanged("refs/heads/main".into()))
        );
    }

    #[test]
    fn commit_borne_par_le_delai() {
        let (dir, repo, p) = prepared();
        // Signature qui ne rend jamais la main (demande de phrase de passe…).
        let pids = dir.path().join("pids");
        let signer = dir.path().join("signe.sh");
        script(
            &signer,
            &format!(
                "echo $$ >> '{0}'\nsleep 1000 &\necho $! >> '{0}'\nwait",
                pids.display()
            ),
        );
        for (k, v) in [
            ("commit.gpgsign", "true"),
            ("gpg.format", "ssh"),
            ("user.signingkey", "/dev/null"),
            ("gpg.ssh.program", signer.to_str().unwrap()),
        ] {
            git(&repo, &["config", k, v]);
        }
        std::fs::write(p.worktree.join("lib.rs"), "changé\n").unwrap();
        let begin = std::time::Instant::now();
        let r = commit_all(&R, &p.tree(), "wip(jarvis): x", Duration::from_millis(500));
        assert_eq!(r, Err("git commit a échoué".into()));
        assert!(
            begin.elapsed() < Duration::from_secs(5),
            "{:?}",
            begin.elapsed()
        );
        // Aucun processus laissé derrière.
        let pids = std::fs::read_to_string(&pids).unwrap();
        assert_eq!(pids.lines().count(), 2, "{pids}");
        assert_dead(&pids);
        assert_eq!(git(&repo, &["rev-parse", &p.branch]), p.base_sha);
    }

    /// Chaque pid (un par ligne) disparaît (le temps d'être récolté).
    fn assert_dead(pids: &str) {
        for pid in pids.lines() {
            let alive = || {
                Command::new("kill")
                    .args(["-0", pid])
                    .stderr(std::process::Stdio::null())
                    .status()
                    .unwrap()
                    .success()
            };
            let mut tries = 0;
            while alive() && tries < 300 {
                std::thread::sleep(Duration::from_millis(10));
                tries += 1;
            }
            assert!(!alive(), "processus {pid} encore là");
        }
    }

    #[test]
    fn annulation_retire_worktree_et_branche() {
        let (_dir, repo, p) = prepared();
        discard(&R, &repo, &p).unwrap();
        assert!(!p.worktree.exists());
        assert!(!branch_exists(&repo, &p.branch));
        assert_eq!(git(&repo, &["worktree", "list"]).lines().count(), 1);
    }

    #[test]
    fn commandes_toujours_bornees_sans_hooks() {
        let r = FakeRunner::new();
        r.respond(
            "timeout",
            CommandOutput {
                success: true,
                stdout: "0123\n".into(),
            },
        );
        check_clean(&r, Path::new("/depot")).unwrap_err();
        let tree = TaskTree {
            worktree: "/w".into(),
            git_dir: "/depot/.git/worktrees/w".into(),
        };
        commit_all(&r, &tree, "message", COMMIT_TIMEOUT).unwrap();
        let calls = r.calls();
        assert!(!calls.is_empty());
        for (program, args) in &calls {
            assert_eq!(program, "timeout");
            assert_eq!(args[0], "--kill-after=5");
            assert_eq!(
                args[2..10],
                [
                    "env",
                    "GIT_TERMINAL_PROMPT=0",
                    "SSH_ASKPASS_REQUIRE=never",
                    "git",
                    "-c",
                    "core.fsmonitor=false",
                    "-c",
                    "core.hooksPath=/dev/null",
                ]
            );
            assert!(args.iter().any(|a| a == "-C"));
            assert!(!args.iter().any(|a| a == "--no-gpg-sign"));
        }
        let tail = |a: &[String], n: usize| a[a.len() - n..].to_vec();
        assert!(calls.iter().any(|(_, a)| a[1] == "30.000s"
            && tail(a, 3) == ["status", "--porcelain", "--untracked-files=no"]));
        let commit = calls
            .iter()
            .find(|(_, a)| a.contains(&"commit".to_string()))
            .unwrap();
        assert_eq!(commit.1[1], "60.000s");
        assert_eq!(
            tail(&commit.1, 5),
            ["commit", "-q", "--no-verify", "-m", "message"]
        );
        assert!(
            commit
                .1
                .contains(&"--git-dir=/depot/.git/worktrees/w".to_string())
        );
        assert!(commit.1.contains(&"--work-tree=/w".to_string()));
    }
}
