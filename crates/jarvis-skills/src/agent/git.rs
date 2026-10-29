//! Garde-fous git : arbre propre, worktree, branche, vérification après coup (tâche 3).
//!
//! Fonctions synchrones (à appeler dans `spawn_blocking`), toujours `git -C <dossier>`.
//! Rien n'est jamais poussé ni fusionné ; la copie de travail de Charles n'est pas touchée.

use crate::agent::types::TaskId;
use crate::runner::{CommandOutput, CommandRunner};
use crate::text::fold;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

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
    pub fn phrase(&self, project: &str) -> String {
        match self {
            Refusal::NotARepo => {
                format!("Le dossier du projet {project} n'est pas un dépôt git : je ne lance rien.")
            }
            Refusal::Dirty { .. } => format!(
                "Le projet {project} a des modifications non enregistrées : je ne lance rien."
            ),
            Refusal::NoBase(base) => {
                format!("Le projet {project} n'a pas de branche {base} : je ne lance rien.")
            }
            Refusal::Git(_) => {
                format!("Git a échoué sur le projet {project} : je ne lance rien.")
            }
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
    /// Branches que l'agent ne doit jamais déplacer.
    pub protected: Vec<String>,
}

/// Lance `git -C dir args…`.
fn git(r: &dyn CommandRunner, dir: &Path, args: &[&str]) -> std::io::Result<CommandOutput> {
    let mut all = vec!["-C".to_string(), dir.display().to_string()];
    all.extend(args.iter().map(|a| a.to_string()));
    r.run("git", &all)
}

/// Sortie de git si la commande réussit, sans espaces finaux.
fn git_ok(r: &dyn CommandRunner, dir: &Path, args: &[&str]) -> Option<String> {
    match git(r, dir, args) {
        Ok(out) if out.success => Some(out.stdout.trim_end().to_string()),
        _ => None,
    }
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
    /// SHA de la base au départ.
    pub base_sha: String,
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
    Ok(Prepared {
        branch,
        worktree,
        base_sha,
    })
}

/// Références relevées avant le lancement.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RefSnapshot {
    /// Branche protégée → SHA (absente : `None`).
    pub protected: BTreeMap<String, Option<String>>,
    /// `refs/remotes/**` → SHA.
    pub remotes: BTreeMap<String, String>,
}

/// SHA des branches protégées et références distantes.
pub fn snapshot(r: &dyn CommandRunner, repo: &Path, protected: &[String]) -> RefSnapshot {
    let protected = protected
        .iter()
        .map(|b| {
            let full = format!("refs/heads/{b}");
            let sha = git_ok(r, repo, &["rev-parse", "--verify", "--quiet", &full])
                .filter(|s| !s.is_empty());
            (b.clone(), sha)
        })
        .collect();
    let remotes = git_ok(
        r,
        repo,
        &[
            "for-each-ref",
            "--format=%(refname) %(objectname)",
            "refs/remotes/",
        ],
    )
    .map(|out| {
        lines(&out)
            .filter_map(|l| l.split_once(' '))
            .map(|(name, sha)| (name.to_string(), sha.to_string()))
            .collect()
    })
    .unwrap_or_default();
    RefSnapshot { protected, remotes }
}

/// Garde-fou enfreint pendant la tâche.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Violation {
    /// Branche protégée déplacée, créée ou supprimée.
    ProtectedMoved(String),
    /// Référence distante nouvelle ou déplacée (`origin/…`) : poussée.
    Pushed(String),
    /// Le worktree n'est plus sur sa branche (`None` : HEAD détachée).
    BranchChanged {
        expected: String,
        actual: Option<String>,
    },
    /// La base n'est plus un ancêtre de la branche.
    BaseLost,
}

impl Violation {
    /// Phrase courte, sans majuscule ni point (« Attention : <phrase>. »).
    pub fn phrase(&self) -> String {
        match self {
            Violation::ProtectedMoved(b) => format!("la branche {b} a été modifiée"),
            Violation::Pushed(r) => format!("la branche {r} a été poussée"),
            Violation::BranchChanged {
                expected,
                actual: Some(a),
            } => format!("l'agent a quitté la branche {expected} pour {a}"),
            Violation::BranchChanged {
                expected,
                actual: None,
            } => format!("l'agent a quitté la branche {expected}"),
            Violation::BaseLost => "la branche de la tâche ne part plus de sa base".into(),
        }
    }
}

/// Vérification après coup : branches protégées, références distantes, branche
/// du worktree, base toujours ancêtre.
pub fn verify(
    r: &dyn CommandRunner,
    repo: &Path,
    worktree: &Path,
    before: &RefSnapshot,
    branch: &str,
    base_sha: &str,
) -> Vec<Violation> {
    let protected: Vec<String> = before.protected.keys().cloned().collect();
    let after = snapshot(r, repo, &protected);
    let mut out = Vec::new();
    for (b, sha) in &before.protected {
        if after.protected.get(b) != Some(sha) {
            out.push(Violation::ProtectedMoved(b.clone()));
        }
    }
    for (name, sha) in &after.remotes {
        if before.remotes.get(name) != Some(sha) {
            let short = name.strip_prefix("refs/remotes/").unwrap_or(name);
            out.push(Violation::Pushed(short.to_string()));
        }
    }
    let actual = git_ok(r, worktree, &["symbolic-ref", "--quiet", "--short", "HEAD"])
        .filter(|s| !s.is_empty());
    if actual.as_deref() != Some(branch) {
        out.push(Violation::BranchChanged {
            expected: branch.to_string(),
            actual,
        });
    }
    let ancestor = git(
        r,
        worktree,
        &["merge-base", "--is-ancestor", base_sha, "HEAD"],
    )
    .map(|o| o.success)
    .unwrap_or(false);
    if !ancestor {
        out.push(Violation::BaseLost);
    }
    out
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
/// ignorés, triés.
pub fn changed_files(r: &dyn CommandRunner, worktree: &Path, base_sha: &str) -> Vec<String> {
    let mut files = BTreeSet::new();
    for args in [
        &["diff", "--name-only", "-z", base_sha][..],
        &["ls-files", "-z", "--others", "--exclude-standard"][..],
    ] {
        if let Ok(out) = git(r, worktree, args)
            && out.success
        {
            files.extend(
                out.stdout
                    .split('\0')
                    .filter(|f| !f.is_empty())
                    .map(str::to_string),
            );
        }
    }
    files.into_iter().collect()
}

/// Un seul commit de tout le travail, avec la config git du dépôt (identité et
/// signature comprises), sans ligne d'attribution.
pub fn commit_all(r: &dyn CommandRunner, worktree: &Path, message: &str) -> Result<(), String> {
    for (args, what) in [
        (&["add", "-A"][..], "git add"),
        (&["commit", "-q", "-m", message][..], "git commit"),
    ] {
        let out = git(r, worktree, args).map_err(|e| format!("git introuvable : {e}"))?;
        if !out.success {
            return Err(format!("{what} a échoué"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::testing::{git, temp_repo};
    use crate::runner::{CommandOutput, FakeRunner, SystemRunner};
    use std::process::Command;

    const R: SystemRunner = SystemRunner;

    fn id() -> TaskId {
        TaskId("0123abcd-4567-89ef".into())
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
            refusal.phrase("Jarvis"),
            "Le projet Jarvis a des modifications non enregistrées : je ne lance rien."
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
        assert!(refusal.phrase("Mawena").contains("Mawena"));
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
        assert!(refusal.phrase("Jarvis").contains("inexistante"));
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
            "git",
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
        let names = flow_names(&R, &repo, None);
        let before = snapshot(&R, &repo, &names.protected);
        std::fs::write(p.worktree.join("lib.rs"), "changé\n").unwrap();
        git(&p.worktree, &["commit", "-qam", "travail"]);
        let head = git(&p.worktree, &["rev-parse", "HEAD"]);
        git(&repo, &["update-ref", "refs/heads/develop", &head]);
        let v = verify(&R, &repo, &p.worktree, &before, &p.branch, &p.base_sha);
        assert_eq!(v, vec![Violation::ProtectedMoved("develop".into())]);
        assert_eq!(v[0].phrase(), "la branche develop a été modifiée");
    }

    #[test]
    fn violation_branche_poussee_signalee() {
        let (dir, repo) = temp_repo();
        let bare = local_remote(dir.path(), &repo);
        let names = flow_names(&R, &repo, None);
        let p = prepare(&R, &repo, &names, "jarvis-p", &dir.path().join("w"), &id()).unwrap();
        let before = snapshot(&R, &repo, &names.protected);
        git(&p.worktree, &["push", "-q", "origin", &p.branch]);
        assert!(branch_exists(&bare, &p.branch));
        let v = verify(&R, &repo, &p.worktree, &before, &p.branch, &p.base_sha);
        assert_eq!(v, vec![Violation::Pushed("origin/feature/jarvis-p".into())]);
        assert!(v[0].phrase().contains("origin/feature/jarvis-p"));
    }

    #[test]
    fn violation_changement_de_branche() {
        let (_dir, repo, p) = prepared();
        let names = flow_names(&R, &repo, None);
        let before = snapshot(&R, &repo, &names.protected);
        git(&p.worktree, &["switch", "-q", "--orphan", "ailleurs"]);
        let v = verify(&R, &repo, &p.worktree, &before, &p.branch, &p.base_sha);
        assert!(
            v.contains(&Violation::BranchChanged {
                expected: p.branch.clone(),
                actual: Some("ailleurs".into()),
            }),
            "{v:?}"
        );
        assert!(v.contains(&Violation::BaseLost), "{v:?}");
        for x in &v {
            assert!(!x.phrase().is_empty());
        }
    }

    #[test]
    fn aucune_violation_apres_travail_normal() {
        let (dir, repo) = temp_repo();
        local_remote(dir.path(), &repo);
        let names = flow_names(&R, &repo, None);
        let p = prepare(&R, &repo, &names, "jarvis-n", &dir.path().join("w"), &id()).unwrap();
        let before = snapshot(&R, &repo, &names.protected);
        std::fs::write(p.worktree.join("lib.rs"), "changé\n").unwrap();
        std::fs::write(p.worktree.join("neuf.rs"), "// neuf\n").unwrap();
        commit_all(&R, &p.worktree, "wip(jarvis): essai").unwrap();
        assert_eq!(
            verify(&R, &repo, &p.worktree, &before, &p.branch, &p.base_sha),
            vec![]
        );
    }

    #[test]
    fn fichiers_modifies_comptes() {
        let (_dir, _repo, p) = prepared();
        assert!(changed_files(&R, &p.worktree, &p.base_sha).is_empty());
        std::fs::write(p.worktree.join("lib.rs"), "changé\n").unwrap();
        git(&p.worktree, &["commit", "-qam", "un"]);
        std::fs::write(p.worktree.join("README.md"), "changé\n").unwrap();
        std::fs::create_dir(p.worktree.join("src")).unwrap();
        std::fs::write(p.worktree.join("src/neuf.rs"), "// neuf\n").unwrap();
        std::fs::write(p.worktree.join(".gitignore"), "cible/\n").unwrap();
        std::fs::create_dir(p.worktree.join("cible")).unwrap();
        std::fs::write(p.worktree.join("cible/ignoré.o"), "x").unwrap();
        assert_eq!(
            changed_files(&R, &p.worktree, &p.base_sha),
            [".gitignore", "README.md", "lib.rs", "src/neuf.rs"]
        );
    }

    #[test]
    fn commit_final_sans_attribution() {
        let (_dir, repo, p) = prepared();
        std::fs::write(p.worktree.join("lib.rs"), "changé\n").unwrap();
        std::fs::write(p.worktree.join("neuf.rs"), "// neuf\n").unwrap();
        commit_all(&R, &p.worktree, "wip(jarvis): finis les tests").unwrap();
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
        assert!(commit_all(&R, &p.worktree, "vide").is_err());
    }

    #[test]
    fn commandes_toujours_avec_dossier() {
        let r = FakeRunner::new();
        check_clean(&r, Path::new("/depot")).unwrap();
        commit_all(&r, Path::new("/w"), "message").unwrap();
        let calls = r.calls();
        assert!(!calls.is_empty());
        for (program, args) in &calls {
            assert_eq!(program, "git");
            assert_eq!(args[0], "-C");
            assert!(
                !args
                    .iter()
                    .any(|a| a == "--no-gpg-sign" || a == "--no-verify")
            );
        }
        assert!(
            calls
                .iter()
                .any(|(_, a)| a[2..] == ["status", "--porcelain", "--untracked-files=no"])
        );
        assert!(
            calls
                .iter()
                .any(|(_, a)| a[2..] == ["commit", "-q", "-m", "message"])
        );
    }
}
