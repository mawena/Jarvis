# Jarvis, phase 7 (agent de code) : plan d'implémentation

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal :** « Jarvis, finis les tests du projet Jarvis » lance aussitôt, sans confirmation et dans les deux modes, Claude Code en arrière-plan sur une branche `feature/jarvis-…` créée pour l'occasion dans un worktree à part ; Jarvis annonce le départ, répond à « où en est la tâche ? » et à « arrête la tâche », puis annonce la fin sur l'oreille la plus récemment active, avec une notification de bureau. Rien n'est poussé ni fusionné ; un projet qui a des modifications non enregistrées est refusé.

**Architecture :** une compétence `agent` dans `jarvis-skills` (projets connus, phrases, garde-fous git, lancement et supervision de `claude -p`, registre des tâches sur disque). Le cerveau ne gagne que deux points génériques du trait `Skill` (`shortcut` : raccourci propre à une compétence, essayé après `match_fast` et avant l'IA ; `llm_tools` : outils proposés à l'IA) et un **tableau d'annonces** (`AnnouncementBoard`) : la compétence y dépose la fin d'une tâche ; le cerveau lance `notify-send` et garde l'annonce pour l'oreille la plus récemment active, qui la relève avec son statut (toutes les 5 s, requête `Announcements`). En phase 4, la connexion persistante remplacera la relève par un envoi immédiat sur la même API (`touch`, `take_for`, `changed`).

**Tech Stack :** Claude Code 2.1.312 (`~/.local/bin/claude`, `-p`, `--output-format stream-json`), git 2.43 (worktrees, `GIT_CONFIG_COUNT`), git-flow AVH 1.12.3 (noms lus dans `gitflow.*`), `tokio::process` (groupe de processus), `fuzzy-matcher` 0.3 (déjà là), `notify-send` (présent). Aucune nouvelle crate (seulement `directories`, `tempfile` et `toml` du workspace ajoutés à `jarvis-skills`).

**Spec :** [docs/superpowers/specs/2026-10-28-jarvis-design.md](../specs/2026-10-28-jarvis-design.md) (§4, §5.4, §5.6, §5.7, §6.4, §9)

## Global Constraints

- Lancement **direct, sans confirmation, dans les deux modes** (§4, §5.6) : outils `Risk::Direct`. Seulement par une voix vérifiée de Charles (`answer_text`) ou du texte tapé (`jarvis say`) ; jamais par `answer_fast` (voix non vérifiée) ; jamais choisi par l'IA : déclencheur déterministe (voir « Décisions »).
- **Seule compétence qui envoie du contenu à une IA en ligne en mode local** (§5.4, §6.4) : la consigne et le code du projet partent chez Claude Code. Le suivi et les résumés n'utilisent aucune IA (calculés à partir du flux de l'agent). La phrase de départ dit « je confie cela à Claude » : la sortie vers l'IA en ligne est dite à voix haute.
- Garde-fous (§6.4), chacun couvert par des tests :
  1. Branche `<gitflow.prefix.feature>jarvis-<slug>` créée depuis `<gitflow.branch.develop>` dans un **worktree** séparé (`~/.local/share/jarvis/worktrees/`) ; jamais `develop` ni `main` ; la copie de travail de Charles n'est jamais touchée.
  2. Rien de poussé ni fusionné : trois couches indépendantes (permissions de Claude Code, environnement git du processus, vérification après coup).
  3. Refus si le dépôt principal a des modifications suivies non enregistrées (`git status --porcelain --untracked-files=no` non vide). Rien n'est créé dans ce cas.
- L'agent ne fait aucun commit. À la fin d'une tâche **réussie**, Jarvis fait un seul commit dans le worktree avec l'identité et la signature git de Charles (jamais `--no-gpg-sign`), sans ligne d'attribution. Signature ou commit impossible : travail laissé dans le worktree, et dit.
- Le cerveau ne bloque jamais : tout git passe par `spawn_blocking` ; `agent_start` répond en moins de 5 s (délai des compétences) ; le travail tourne dans des tâches tokio.
- Journal brut de chaque tâche : `~/.local/state/jarvis/tasks/<id>.log` (dossier 0700, fichiers 0600, 20 Mo au plus). La consigne est journalisée en `info` (comme les phrases).
- **Tests : jamais le vrai `claude`** (faux script shell dans un dossier temporaire), dépôts git temporaires (`git init -b develop`, `commit.gpgsign=false` et identité en config locale), aucun réseau, `notify-send` uniquement via `FakeRunner`. Pas de micro, pas de son, pas de redémarrage du service.
- Les règles des phases précédentes restent valables (git flow, commits au nom de Charles sans attribution, `ROADMAP.md` coché dès qu'une tâche est validée, `scripts/check.sh` vert, jamais `cargo update` sans `-p`, ne pas redémarrer le service ni ouvrir le micro sans Charles).

## Review Focus

1. **Aucun chemin vers un push ou une fusion** → `push_bloque_par_l_environnement`, `push_ssh_explicite_bloque` (tâche 3), `arguments_interdisent_push_et_commit` (tâche 4), `violation_develop_deplacee_signalee` (tâche 3), `fin_avec_violation_annonce_urgente` (tâche 6).
2. **Arbre modifié → rien n'est lancé** (ni worktree, ni branche, ni processus) → `arbre_modifie_refuse` (tâche 3), `projet_modifie_refuse_sans_rien_creer` (tâche 6).
3. **Déclencheur** : une voix non vérifiée, une phrase courante ou « lance Jarvis » ne lancent jamais l'agent → tests des tâches 1, 2 et 6 (`aucune_phrase_de_reference_n_est_une_tache`).
4. **Processus** : arrêt et délais tuent tout le groupe (y compris un `cargo test` lancé par l'agent), aucun zombie, journal complet → tests de la tâche 4.
5. **Compatibilité** : ancien `Listen` sans `ear` ; ancien cerveau qui répond `Error` à `Announcements` (l'oreille cesse de demander) → tests des tâches 1 et 5.
6. **Phase 4 planifiée en parallèle** : seuls ajouts au protocole, `Listen.ear` et `Announcements` ; l'API du tableau ne dépend pas du transport.

---

Démarrage : `git flow feature start agent-de-code` depuis `develop`.

**Ordre et parallélisme :** tâche 1 seule ; puis tâches 2, 3, 4 et 5 **en parallèle** (4 agents, fichiers disjoints) ; puis tâches 6 et 7 **en parallèle** ; puis tâche 8 avec Charles.

| tâche | dépend de | fichiers (propriété exclusive) |
|---|---|---|
| 1 socle | — | protocole, `jarvis-skills/src/{lib.rs,text.rs,announce.rs}`, `agent/{mod.rs,types.rs,settings.rs,testing.rs}` + fichiers vides des autres modules, `jarvis-brain/src/{brain.rs,config.rs,server.rs,testing.rs}`, constructions de `Listen` ailleurs |
| 2 projets, phrases, registre | 1 | `agent/{projects.rs,phrases.rs,registry.rs}` |
| 3 garde-fous git | 1 | `agent/git.rs` |
| 4 lancement, supervision | 1 | `agent/{claude.rs,supervisor.rs}` |
| 5 annonces | 1 | `jarvis-brain/src/{announce.rs,lib.rs,server.rs,tts/mod.rs}`, `jarvis-ear/src/{app.rs,config.rs,main.rs}` |
| 6 compétence assemblée | 2-5 | `agent/mod.rs` (corps), `jarvis-brain/src/main.rs`, `jarvis-brain/tests/agent.rs` |
| 7 outils et docs | 2-5 | `jarvisctl/src/ops.rs`, `docs/agent.md`, `docs/installation.md` |
| 8 essai réel | 6, 7 | `ROADMAP.md` |

### Task 1 : socle (protocole, trait `Skill`, types de l'agent)

**Files :** Modify `crates/jarvis-protocol/src/lib.rs`, `crates/jarvis-skills/{Cargo.toml,src/lib.rs}`, `crates/jarvis-brain/src/{brain.rs,config.rs,server.rs,testing.rs}`, toutes les constructions de `ControlRequest::Listen { .. }` (oreille, CLI, tests : `grep -rn "ControlRequest::Listen {"`) ; Create `crates/jarvis-skills/src/{text.rs,announce.rs}`, `crates/jarvis-skills/src/agent/{mod.rs,types.rs,settings.rs,testing.rs}` et des fichiers ne contenant qu'un commentaire de module : `agent/{projects.rs,phrases.rs,registry.rs,git.rs,claude.rs,supervisor.rs}`.

**Interfaces :**
- Protocole (nouveaux champs `#[serde(default)]`) :
  - `ControlRequest::Listen` gagne `ear: Option<String>` (nom de l'oreille ; absent = `DEFAULT_EAR`).
  - `ControlRequest::Announcements { ear: Option<String> }` → `ControlResponse::Announcements { items: Vec<Announcement> }`.
  - `struct Announcement { speech: String, audio: Option<SpeechAudio>, sound: Option<Sound> }`.
  - `pub const DEFAULT_EAR: &str = "pc";`
- `jarvis_skills` :
  - `pub struct Shortcut { pub tool: String, pub args: serde_json::Value }`.
  - `Skill::shortcut(&self, text: &str) -> Option<Shortcut>` (défaut `None`) : `text` est la phrase **d'origine** (casse, accents) ; la compétence la normalise elle-même.
  - `Skill::llm_tools(&self) -> Vec<ToolSpec>` (défaut `self.tools()`).
  - `text::fold(&str) -> String` : minuscules, accents retirés, ponctuation en espaces, espaces fusionnés (même règle que `fastpath::clean`, **sans** retirer les politesses ni « jarvis »).
  - `announce` : `struct Notice { title: String, speech: String, body: String, outcome: Outcome, urgent: bool, speak: bool }` ; `trait Announcer: Send + Sync { fn announce(&self, notice: Notice); }` ; `NullAnnouncer` ; `RecordingAnnouncer` (feature `test-util`, `notices()`).
- `agent/types.rs` : `TaskId(String)` ; `enum TaskState { Running, Succeeded, Failed, Stopped, TimedOut, Interrupted }` ; `struct TaskRecord { id, project, instruction, branch, worktree: PathBuf, base, base_sha, started_at: u64, ended_at: Option<u64>, state, summary: Option<String>, files_changed: Option<usize>, violations: Vec<String>, cost_usd: Option<f64>, log: PathBuf }` (serde) ; `struct Progress { last_action: Option<String>, last_text: Option<String>, result: Option<RunResult>, lines: usize }` ; `struct RunResult { is_error: bool, subtype: String, text: String, cost_usd: Option<f64>, num_turns: Option<u32> }` ; `enum EndReason { Exited, Stopped, TimedOut, Idle }` ; `enum AgentCommand { Start { project: String, instruction: String }, Status { project: Option<String> }, Stop { project: Option<String> }, UnknownProject { heard: String } }`.
- `agent/settings.rs` (serde, `#[serde(default)]`, `~/` développé par `resolved()`) :

  ```toml
  [agent]
  enabled = true
  claude_path = "~/.local/bin/claude"
  max_minutes = 45          # durée maximale d'une tâche
  idle_minutes = 10         # sans aucune sortie : bloquée, arrêtée
  max_tasks = 2             # en même temps ; une seule par projet
  model = ""                # vide : modèle par défaut de Claude Code
  max_budget_usd = 0.0      # 0 : sans plafond
  pass_api_key = false      # ANTHROPIC_API_KEY retirée : l'agent utilise le compte de Charles
  commit_at_end = true
  worktree_dir = "~/.local/share/jarvis/worktrees"
  cargo_target_dir = "~/.cache/jarvis/agent-target"   # + /<projet>
  extra_path = ["~/.cargo/bin", "~/.local/bin"]
  extra_allowed_tools = []  # ajoutés à la liste par défaut ; la liste interdite ne se réduit pas

  [[agent.projects]]
  name = "Jarvis"
  path = "~/Work/Mawena/Projects/Jarvis"
  aliases = ["l'assistant"]
  # base = "develop"        # sinon gitflow.branch.develop, sinon develop
  # extra_allowed_tools = ["Bash(npm test*)"]
  ```
  `DEFAULT_ALLOWED_TOOLS`, `DENIED_TOOLS` : voir « Décisions ».
- `agent/testing.rs` (feature `test-util` ; `tempfile` devient dépendance optionnelle : `test-util = ["dep:tempfile"]`) : `temp_repo() -> (TempDir, PathBuf)` (`git init -b develop`, identité et `commit.gpgsign=false` locales, `gitflow.*` posés, un commit) ; `git(repo, &[..]) -> String` ; `fake_claude(dir, body) -> PathBuf` (script `#!/bin/sh` exécutable qui écrit `"$@"`, `env` et l'entrée standard dans `dir/{args,env,stdin}` puis exécute `body`) ; lignes de flux : `init_line(session)`, `text_line(t)`, `tool_use_line(name, input)`, `result_line(ok, text)`.
- Cerveau :
  - `Brain::tools()` utilise `llm_tools()` ; `find()` garde `tools()`.
  - `Brain::understand` : après `match_fast` (et son repli `NotFound`), essaie `shortcut(text)` de chaque compétence ; le premier trouvé passe par `run_or_ask` ; `Step::NotFound(m)` → `Reply::failed(m)` **sans** repli sur l'IA ; `fast_path: false` (la phrase de départ n'est jamais remplacée par un bip). `answer_fast` ne consulte jamais `shortcut`.
  - `Config.agent: AgentSettings`.
  - `server.rs` : `Announcements` → `Announcements { items: vec![] }` (rempli en tâche 5) ; `Listen.ear` accepté et ignoré.
  - `testing::FakeSkill::with_shortcut(phrase, tool, args)` et `with_llm_tools(bool)`.

- [ ] Tests : `listen_sans_ear_reste_lisible`, `announcements_aller_retour`, `annonce_sans_audio_lisible`, `fold_retire_accents_et_ponctuation`, `fold_garde_jarvis`, `raccourci_de_competence_apres_les_raccourcis`, `raccourci_de_competence_introuvable_sans_ia` (l'IA n'est pas appelée), `voix_non_verifiee_ignore_les_raccourcis_de_competence`, `outils_hors_ia_non_proposes`, `raccourci_de_competence_jamais_en_bip_seul`, `config_agent_defauts`, `config_projets_lus`.
- [ ] Implémenter ; tout compile (`ear: None` partout) ; `scripts/check.sh` vert ; commit `feat: socle de l'agent de code`.

### Task 2 : projets connus, phrases, registre des tâches (7.2, 7.5)

**Files :** `crates/jarvis-skills/src/agent/{projects.rs,phrases.rs,registry.rs}` (+ `toml` en dev-dependency pour lire le jeu de phrases)

**Interfaces :**
- `ProjectList::new(Vec<Project>)` ; `resolve(&self, spoken: &str) -> Option<&Project>` : `fold`, nom ou alias exact, puis `SkimMatcherV2` (seuil comme `apps::index`, meilleur score puis nom le plus court) ; deux projets différents à moins de 10 % de score → `None` (ambigu) ; `find_in(&self, words: &[&str]) -> Option<(&Project, usize, usize)>` : n-gramme de 1 à 3 mots égal à un nom ou alias ; `names()`.
- `phrases::parse(text: &str, projects: &ProjectList) -> Option<AgentCommand>` (sur `fold(text)`, premier mot « jarvis » retiré, politesses courantes retirées) :
  - **Suivi** : « où en est », « ça avance », « ça en est où », « quelles tâches », « état de la tâche » + « tâche(s) » ou « agent » ou un projet → `Status { project }`.
  - **Arrêt** : premier mot `arrete | stoppe | interromps | annule | coupe` + « la tâche », « les tâches » ou « l'agent » → `Stop { project }`. « arrête » ou « arrête la musique » seuls : `None`.
  - **Départ** : (a) ouverture explicite « demande à l'agent », « confie à l'agent », « tâche de code » ; ou (b) premier verbe de code (`finis termine acheve corrige repare ajoute ecris implemente refactorise nettoie teste documente ameliore optimise renomme migre supprime prepare`, plus « mets à jour », « fais passer », « lance les tests », « fais tourner les tests ») **et** un projet nommé (n-gramme exact n'importe où, ou flou juste après « projet ») → `Start { project: nom, instruction }`. Verbe de code + « projet X » inconnu → `UnknownProject { heard: X }`. Verbe de code sans projet → `None` (l'IA répond normalement, elle n'a pas l'outil).
  - `instruction` : texte d'origine sans « Jarvis, » de tête ni politesse finale, 2 000 caractères au plus.
- `Registry::open(dir) -> io::Result<Registry>` (0700) ; `save(&TaskRecord)` (écriture atomique : fichier temporaire puis renommage, 0600) ; `load_all()` (un fichier illisible est ignoré avec `warn!`) ; `recover(now) -> Vec<TaskRecord>` (`Running` → `Interrupted`, enregistrés) ; `prune(keep)` (enregistrements et journaux les plus anciens terminés, jamais `Running`) ; `log_path(&TaskId)` ; `running_on_disk(dir) -> usize` (pour `jarvisctl`).

- [ ] Tests : `projet_exact_par_nom`, `projet_par_alias`, `projet_flou`, `projet_ambigu_refuse`, `projet_inconnu`, `finis_les_tests_du_projet_jarvis`, `projet_nomme_jarvis_reconnu_en_fin_de_phrase`, `corrige_le_bug_dans_mawena`, `consigne_gardee_avec_accents`, `projet_inconnu_dit_le_nom_entendu`, `verbe_sans_projet_pas_une_tache`, `lance_jarvis_n_est_pas_une_tache`, `ouvre_le_projet_n_est_pas_une_tache`, `ou_en_est_la_tache`, `ou_en_est_la_tache_de_mawena`, `arrete_la_tache`, `arrete_seul_n_est_pas_l_agent`, `aucune_phrase_de_reference_n_est_une_tache` (lit `crates/jarvis-brain/tests/fixtures/phrases.toml`), `registre_aller_retour`, `registre_marque_les_taches_interrompues`, `registre_ignore_un_fichier_corrompu`, `registre_purge_les_anciennes`, `registre_permissions_privees`.

### Task 3 : garde-fous git (7.4)

**Files :** `crates/jarvis-skills/src/agent/git.rs`

**Interfaces** (synchrones, `r: &dyn CommandRunner`, toujours `git -C <dossier>`, appelées dans `spawn_blocking`) :
- `enum Refusal { NotARepo, Dirty { files: usize }, NoBase(String), Git(String) }` + `phrase(&self, project) -> String` (« Le projet Jarvis a des modifications non enregistrées : je ne lance rien. »…).
- `flow_names(r, repo, base_override) -> FlowNames { base, prefix, protected: Vec<String> }` : `gitflow.branch.develop` (défaut `develop`), `gitflow.prefix.feature` (défaut `feature/`), protégées = base + `gitflow.branch.master` (défaut `main` et `master`).
- `check_clean(r, repo) -> Result<(), Refusal>` : `status --porcelain --untracked-files=no`.
- `slug(instruction, project) -> String` : `fold`, mots vides et nom du projet retirés, 4 mots au plus, 40 caractères au plus, préfixe `jarvis-` (vide → `jarvis-tache`).
- `prepare(r, repo, names, slug, worktree_dir, id) -> Result<Prepared { branch, worktree, base_sha }, Refusal>` : `rev-parse --verify <base>`, nom de branche libre (suffixe `-2`, `-3`…), `worktree add -b <branch> <worktree_dir>/<projet>-<slug>-<id8> <base>`.
- `snapshot(r, repo, protected) -> RefSnapshot` : SHA des branches protégées, ensemble de `refs/remotes/**` (`for-each-ref`).
- `verify(r, repo, worktree, before, branch, base_sha) -> Vec<Violation>` : branche protégée déplacée, nouvelle référence distante (poussée), worktree sur une autre branche, `merge-base --is-ancestor base_sha HEAD` faux. `Violation::phrase()`.
- `no_push_env(r, repo) -> GitEnv { set: Vec<(String, String)>, remove: Vec<String> }` : voir « Décisions ».
- `changed_files(r, worktree, base_sha) -> Vec<String>` : `diff --name-only <base_sha>` ∪ `ls-files --others --exclude-standard`.
- `commit_all(r, worktree, message) -> Result<(), String>` : `add -A`, `commit -m <message>` (config de Charles, signature comprise).

- [ ] Tests (dépôts temporaires, vrai git via `SystemRunner` ; `FakeRunner` pour la forme des commandes) : `arbre_propre_accepte`, `arbre_modifie_refuse`, `fichier_indexe_refuse`, `fichier_non_suivi_ignore`, `pas_un_depot_refuse`, `base_develop_absente_refusee`, `noms_git_flow_lus_dans_la_config`, `noms_par_defaut_sans_git_flow`, `slug_court_sans_accents`, `worktree_cree_sur_branche_feature`, `branche_existante_suffixe`, `copie_de_travail_de_charles_intacte`, `push_bloque_par_l_environnement` (remote nu local, `git push` lancé avec l'environnement : échec, branche absente du remote), `push_ssh_explicite_bloque`, `violation_develop_deplacee_signalee`, `violation_branche_poussee_signalee`, `violation_changement_de_branche`, `aucune_violation_apres_travail_normal`, `fichiers_modifies_comptes`, `commit_final_sans_attribution`.

### Task 4 : lancement et supervision de Claude Code (7.3, 7.5)

**Files :** `crates/jarvis-skills/src/agent/{claude.rs,supervisor.rs}`

**Interfaces :**
- `struct Invocation { program: PathBuf, args: Vec<String>, cwd: PathBuf, env_set: Vec<(String, String)>, env_remove: Vec<String>, stdin: String }`.
- `claude::build(settings: &AgentSettings, project: &Project, task: &TaskRecord, git_set: Vec<(String, String)>, git_remove: Vec<String>) -> Invocation` (ligne exacte : « Décisions »).
- `claude::system_prompt(branch: &str) -> String`.
- `struct StreamParser` : `feed(&mut self, line: &str)` ; `progress(&self) -> &Progress` ; `session_id()`. Lignes : `system/init`, `assistant` (texte → `last_text` ; `tool_use` → `last_action = describe_tool(..)`), `result` (→ `RunResult`). Ligne invalide ou inconnue : ignorée.
- `describe_tool(name, input) -> String` : Bash → « lance « cargo test » » (4 premiers mots) ; Edit/Write → « modifie brain.rs » ; Read → « lit … » ; Grep/Glob → « cherche dans le code » ; autre → nom.
- `summary(result_text) -> Option<String>` : ligne `RÉSUMÉ :` (variantes de casse et d'accents), sans markdown, 200 caractères au plus ; repli : dernière phrase du résultat.
- `struct Limits { max_run: Duration, idle: Duration, grace: Duration /* 10 s */ }`.
- `supervisor::spawn(inv, log: &Path, limits) -> io::Result<Running>` ; `Running { pid }` avec `progress() -> Progress` (`watch`), `stopper() -> Stopper` (`stop()` idempotent), `wait(self) -> RunOutcome { reason: EndReason, exit_code: Option<i32>, progress: Progress }`.
- Mise en œuvre : `tokio::process::Command` (`current_dir`, `envs`, `env_remove`, `process_group(0)`, `kill_on_drop(true)`, stdin/stdout/stderr en tubes) ; consigne écrite sur stdin puis fermée ; chaque ligne de stdout → journal, analyseur, `watch` ; stderr → journal préfixé `[stderr] ` ; `select!` sur fin du processus, arrêt demandé, échéance `max_run`, minuteur `idle` remis à zéro à chaque ligne ; arrêt : `kill -s TERM -- -<pid>`, attente `grace`, puis `kill -s KILL -- -<pid>`, puis `wait()`. En-tête du journal : id, programme, arguments (sans la consigne système), heure.

- [ ] Tests (faux scripts, durées réelles courtes) : `arguments_de_lancement`, `arguments_interdisent_push_et_commit`, `consigne_passee_par_l_entree` (jamais dans les arguments), `environnement_sans_cle_ni_agent_ssh`, `cle_api_transmise_si_option`, `chemin_cargo_en_tete_du_path`, `flux_suit_la_derniere_action`, `flux_lit_le_resultat`, `flux_ignore_les_lignes_invalides`, `resume_extrait_de_la_ligne_resume`, `resume_de_repli_sans_ligne`, `resume_sans_markdown_et_borne`, `processus_termine_normalement`, `journal_contient_sortie_et_erreurs`, `arret_tue_tout_le_groupe` (le script lance `sleep 1000 &` et écrit son pid ; après l'arrêt, `kill -0` échoue), `term_ignore_tue_apres_le_delai`, `duree_maximale_depassee`, `silence_trop_long_arrete`, `programme_introuvable_erreur_claire`, `arret_apres_fin_sans_effet`.

### Task 5 : annonces sur l'oreille la plus récemment active (7.6)

**Files :** Create `crates/jarvis-brain/src/announce.rs` ; Modify `crates/jarvis-brain/src/{lib.rs,server.rs,tts/mod.rs}`, `crates/jarvis-ear/src/{app.rs,config.rs,main.rs}`

**Interfaces :**
- `AnnouncementBoard::new(runner: Arc<dyn CommandRunner>, clock: Arc<dyn Clock>)` ; `touch(&self, ear: &str)` (une phrase est arrivée de cette oreille) ; `take_for(&self, ear: &str) -> Vec<(String, Outcome)>` (note aussi la dernière relève) ; `changed(&self)` (`Notify`, pour l'envoi immédiat de la phase 4) ; `impl Announcer` : toujours `notify-send --app-name=Jarvis --urgency=normal|critical --icon=dialog-information|dialog-warning <titre> <corps>` par `spawn_detached` ; si `speak`, mise en file (10 au plus, 10 min de vie).
- Règle : cible = oreille du dernier `touch` ; aucune → la première qui relève ; cible sans relève depuis 20 s → n'importe quelle oreille qui relève. Chaque annonce est livrée une fois.
- `Voice::audio_for(&self, text: &str) -> Option<SpeechAudio>` (même synthèse bornée que `speak`).
- `server.rs` : `serve_with_board(brain, listen, speaker, voice, board, socket)` (les anciens `serve*` passent un tableau neuf) ; `Listen { ear }` → `board.touch(ear ou DEFAULT_EAR)` avant l'écoute ; `Announcements { ear }` → `take_for`, synthèse, bip (`Done` → `Success`, `Failed` → `Failure`, `Info` → aucun).
- Oreille : `ear.toml` gagne `name` (défaut `"pc"`) ; `Listen` envoie `ear: Some(name)` ; à chaque relève de statut réussie, requête `Announcements` (même drapeau « en cours ») ; réponse `Error` (ancien cerveau) → relève abandonnée pour la session (`debug!` une fois) ; annonces en file, jouées au repos (ni capture, ni lecture, ni attente de réponse, ni réveil en cours) avec `play_reply` ; « Jarvis » coupe une annonce comme une réponse ; écoute coupée : l'annonce est jouée quand même (sortie seulement).

- [ ] Tests cerveau : `annonce_notifie_le_bureau`, `annonce_urgente_critique`, `annonce_pour_l_oreille_la_plus_recente`, `annonce_sans_oreille_active_prise_par_la_premiere`, `oreille_absente_annonce_prise_par_une_autre`, `annonce_livree_une_seule_fois`, `annonce_perimee_oubliee`, `notification_seule_sans_voix`, `requete_annonces_avec_voix_et_bip` (`FakeSynthesizer`), `listen_marque_l_oreille_active`.
- [ ] Tests oreille : `annonce_jouee_au_repos`, `annonce_attend_la_fin_de_la_capture`, `annonce_attend_la_fin_de_la_lecture`, `reveil_coupe_l_annonce`, `ancien_cerveau_sans_annonces_ignore`, `listen_envoie_le_nom_de_l_oreille`, `nom_d_oreille_par_defaut`.

### Task 6 : compétence `agent` assemblée (7.3 à 7.6)

**Files :** Modify `crates/jarvis-skills/src/agent/mod.rs`, `crates/jarvis-brain/src/main.rs` ; Create `crates/jarvis-brain/tests/agent.rs`

**Interfaces :**
- `AgentSkill::new(settings, runner: Arc<dyn CommandRunner>, announcer: Arc<dyn Announcer>, registry: Registry)` ; `with_ids(Arc<dyn Fn() -> TaskId + Send + Sync>)` (défaut : `/proc/sys/kernel/random/uuid`) ; `with_now(..)` (heure Unix, tests).
- `tools()` : `agent_start { project, instruction }`, `agent_status { project? }`, `agent_stop { project? }`, tous `Risk::Direct` ; `llm_tools()` → `vec![]` ; `shortcut()` → `phrases::parse` (`UnknownProject` → `agent_start` avec le nom entendu → `NotFound` « Je ne connais pas le projet « X ». Projets connus : Jarvis, Mawena. »).
- `agent_start` : projet → limites (une tâche par projet, `max_tasks`) → `claude_path` exécutable → `spawn_blocking { flow_names, check_clean, snapshot, prepare, no_push_env }` → `registry.save(Running)` → `claude::build` → `supervisor::spawn` (échec : `Failed`, worktree gardé, phrase) → tâche de surveillance → `Reply::done("Bien. Je confie cela à Claude, sur une branche à part du projet Jarvis. Je vous préviens à la fin.")` ; `info!` « consigne envoyée à Claude Code (en ligne) ».
- Surveillance : `wait()` → `spawn_blocking { verify, changed_files, commit_all }` (commit seulement si sortie normale, résultat sans erreur, aucune violation, au moins un fichier, `commit_at_end` ; message `wip(jarvis): <consigne, 60 caractères>`) → registre → `Notice` :
  - réussite : « La tâche sur Jarvis est terminée : <résumé>. 3 fichiers modifiés. » (`Done`) ;
  - échec : « L'agent a échoué sur Jarvis : <raison courte>. » (`Failed`) ;
  - délai : « La tâche sur Jarvis a dépassé 45 minutes : je l'ai arrêtée. » ;
  - arrêt demandé : notification seule (`speak: false`) ;
  - violation : « Attention : <violation>. Vérifiez le projet Jarvis. » (`urgent`, `Failed`) ;
  - corps de la notification : branche, worktree, `claude --resume <id>`, journal.
- `agent_status` : « La tâche sur Jarvis tourne depuis 12 minutes ; dernière action : lance « cargo test ». » (deux tâches au plus, une phrase chacune) ; aucune → « Aucune tâche en cours. » + la dernière terminée (« La dernière, sur Jarvis, est terminée : <résumé>. ») ou interrompue.
- `agent_stop` : par projet, ou la seule en cours ; plusieurs sans projet → « Deux tâches tournent, sur Jarvis et Mawena : dites « arrête la tâche Jarvis », par exemple. » ; réponse « J'arrête la tâche sur Jarvis. Le travail reste sur sa branche. ».
- `main.rs` : `Registry::open(<state>/jarvis/tasks)`, `recover()` → une `Notice` (`speak: false`) par tâche interrompue, `prune(50)` ; `AnnouncementBoard` ; `AgentSkill` ajoutée si `agent.enabled` ; `serve_with_board`.

- [ ] Tests (temp repos + faux `claude` ; `RecordingAnnouncer`) : `demarrage_repond_tout_de_suite`, `projet_modifie_refuse_sans_rien_creer`, `projet_inconnu_dit_les_projets_connus`, `une_tache_par_projet`, `nombre_maximal_de_taches`, `claude_absent_refuse`, `tache_reussie_annoncee_avec_resume_et_fichiers` (commit sur la branche, `develop` inchangée), `tache_echouee_annoncee`, `pas_de_commit_si_echec`, `delai_depasse_annonce`, `arret_sans_annonce_vocale`, `arret_sans_tache`, `arret_ambigu_demande_laquelle`, `statut_sans_tache`, `statut_tache_en_cours`, `statut_apres_fin_dit_la_derniere`, `fin_avec_violation_annonce_urgente` (le faux agent déplace `develop` par `git update-ref`), `taches_interrompues_notifiees_au_demarrage`.
- [ ] Tests cerveau (`tests/agent.rs`) : `jarvis_say_lance_l_agent`, `changer_de_mode_n_arrete_pas_les_taches`, `outils_agent_jamais_proposes_a_l_ia`, `voix_non_verifiee_ne_lance_pas_l_agent`.

### Task 7 : `jarvisctl` et documentation

**Files :** Modify `crates/jarvisctl/src/ops.rs`, `docs/installation.md` ; Create `docs/agent.md`

- `jarvisctl doctor` : « agent de code » (claude trouvé, version ; options requises présentes dans `claude --help` : `--permission-mode`, `--setting-sources`, `--session-id`, `--append-system-prompt`, `--disallowedTools`, `stream-json`) ; projets (nombre, chemins valides, dépôt git, branche de base présente) ; `notify-send` présent.
- `jarvisctl restart|update|stop` : « Attention : 1 tâche d'agent en cours sera interrompue. » si un enregistrement `Running` existe (`Registry::running_on_disk`), sans bloquer.
- `docs/agent.md` : configuration des projets, phrases reconnues, garde-fous, où trouver le travail (worktree, branche, `claude --resume`), comment finir (`git worktree remove <dossier>` puis `git flow feature finish jarvis-…`), journaux, ce qui part en ligne.
- Tests : `doctor_signale_claude_absent`, `doctor_compte_les_projets`, `doctor_signale_option_manquante`, `redemarrage_previent_des_taches_en_cours`.

### Task 8 : essai réel avec Charles

- `jarvisctl update` ; `jarvisctl doctor`.
- Projet d'essai : `~/Work/jarvis-essai` (`cargo new`, `git flow init -d`, un test qui échoue, remote nu local `origin` en `file://`), ajouté à `[[agent.projects]]` ; `jarvisctl restart`.
- **Vérifier d'abord la ligne de commande** (seul fichier à ajuster : `claude.rs`) : `stream-json` exige-t-il `--verbose` ; `--setting-sources project` garde-t-il la connexion au compte ; la syntaxe `Bash(cargo test*)` des règles ; un `git push` demandé est refusé et visible dans le journal.
- Arbre modifié : `jarvis say "finis les tests du projet essai"` → refus, rien de créé.
- Arbre propre : même phrase → départ ; `git worktree list`, branche `feature/jarvis-…`, copie de travail intacte ; « Jarvis, où en est la tâche ? » ; fin annoncée sur l'oreille + notification ; commit signé sur la branche ; `develop` inchangée ; remote nu sans la branche.
- « Corrige le test du projet essai et pousse la branche » → rien de poussé.
- Tâche longue puis « Jarvis, arrête la tâche » → plus aucun `claude` ni `cargo` (`pgrep -a claude`).
- `jarvisctl restart` pendant une tâche → avertissement, puis notification « interrompue ».
- Optionnel, avec l'accord de Charles : « Jarvis, finis les tests du projet Jarvis ».
- Cocher 7.2 à 7.6.

## Décisions

**Déclencheur déterministe.** L'agent part sans confirmation et envoie du code en ligne : un petit modèle local (82-89 % de bons outils) ne doit pas pouvoir le déclencher sur une phrase mal comprise. Il faut donc un verbe de code et un projet connu, ou une ouverture explicite. `llm_tools()` vide : l'IA ne voit jamais l'outil, dans aucun mode. La compétence lit la phrase d'origine, car `normalize` retire « jarvis » en fin de phrase (« …du projet Jarvis » perdrait son nom). Les raccourcis de compétence passent après `match_fast` : « lance Jarvis » reste une appli, et « lance » n'est un verbe de code que dans « lance les tests ».

**Worktree plutôt que `git flow feature start`.** `git flow feature start` change la branche de la copie de travail de Charles (fichiers ouverts dans VS Code, compilation en cours). Un `git worktree add -b <gitflow.prefix.feature>jarvis-<slug> … <gitflow.branch.develop>` crée exactement la même branche, depuis la même base, avec les noms de la configuration git flow : `git flow feature finish jarvis-<slug>` marche ensuite, une fois le worktree retiré. Le refus sur arbre modifié reste : il garantit que la base est bien ce que Charles voit et suit la règle de la spec. Les fichiers non suivis sont ignorés (rien n'est écrasé dans un worktree).

**Ligne de commande de Claude Code.**

    <claude_path> -p \
      --output-format stream-json --verbose \
      --permission-mode dontAsk --permission-prompts none \
      --setting-sources project \
      --settings '{"includeCoAuthoredBy":false,"attribution":{"commit":"","pr":""}}' \
      --allowedTools=<autorisés, séparés par des virgules> \
      --disallowedTools=<interdits> \
      --append-system-prompt "<consignes de Jarvis>" \
      --session-id <id de la tâche> --name "jarvis <projet> <slug>" \
      [--model <agent.model>] [--max-budget-usd <agent.max_budget_usd>]
    # cwd = worktree ; stdin = consigne (puis fermé) ; stdout = flux JSON ; stderr journalisé

- `dontAsk` : tout ce qui n'est pas autorisé est refusé, sans personne pour répondre.
- `--setting-sources project` : les réglages utilisateur de Charles autorisent tout `Bash` et le mode `auto` ; ils ne doivent pas s'appliquer. À vérifier à l'essai : la connexion au compte n'est pas un réglage.
- Consigne sur l'entrée standard : jamais en argument (taille, options variadiques).
- `--session-id` = id de la tâche : `claude --resume <id>` dans le worktree permet à Charles de reprendre.
- Autorisés par défaut : `Read, Edit, Write, MultiEdit, Glob, Grep, TodoWrite, Bash(cargo build*), Bash(cargo check*), Bash(cargo test*), Bash(cargo clippy*), Bash(cargo fmt*), Bash(cargo doc*), Bash(cargo tree*), Bash(scripts/check.sh*), Bash(./scripts/check.sh*), Bash(git status*), Bash(git diff*), Bash(git log*), Bash(git show*), Bash(git blame*), Bash(git ls-files*), Bash(git grep*), Bash(ls*), Bash(rg *), Bash(grep *), Bash(head *), Bash(tail *), Bash(wc *), Bash(mkdir *)` + `extra_allowed_tools`.
- Interdits (fixes, prioritaires) : `Bash(git push*), Bash(git merge*), Bash(git rebase*), Bash(git reset*), Bash(git checkout*), Bash(git switch*), Bash(git branch*), Bash(git commit*), Bash(git worktree*), Bash(git remote*), Bash(git config*), Bash(git flow*), Bash(git tag*), Bash(gh*), Bash(cargo run*), Bash(cargo publish*), Bash(cargo install*), Bash(cargo update*), Bash(cargo login*), WebFetch, WebSearch`. `cargo run` est interdit : dans le projet Jarvis, il ouvrirait le micro.
- Consignes ajoutées (`--append-system-prompt`) : lancé par Jarvis, sans personne pour répondre (ne pose pas de question, dis ce que tu as supposé) ; la consigne vient d'une transcription vocale ; worktree sur la branche `<branche>` ; interdits : pousser, fusionner, changer de branche, committer (Jarvis enregistre à la fin), modifier la config git, toucher hors du dossier, lancer les binaires ou les tests `#[ignore]` ; lance les tests avant de finir ; termine par une seule ligne `RÉSUMÉ : <une phrase en français, 25 mots au plus, lisible à voix haute, sans markdown>`.

**Environnement de l'agent** (deuxième couche contre le push, indépendante des permissions) :
- `GIT_CONFIG_COUNT` / `GIT_CONFIG_KEY_i` / `GIT_CONFIG_VALUE_i` :
  - `remote.<r>.pushurl = jarvis-push-interdit://` pour chaque remote ;
  - `credential.helper =` (vide : aucun identifiant https).
- `GIT_SSH_COMMAND=false` et `GIT_TERMINAL_PROMPT=0`.
- Retirés : `SSH_AUTH_SOCK`, `GH_TOKEN`, `GITHUB_TOKEN`, `GIT_ASKPASS`, `SSH_ASKPASS`, et `ANTHROPIC_API_KEY` sauf si `pass_api_key` (sinon l'agent serait facturé sur l'API du mode en ligne au lieu du compte de Charles).
- `PATH` = `extra_path` + `PATH` hérité : le service systemd n'a pas `~/.cargo/bin`.
- `CARGO_TARGET_DIR` = `<cargo_target_dir>/<projet>` : partagé entre les tâches d'un projet, jamais avec la copie de Charles (pas de verrou commun ; la première compilation est complète).

**Troisième couche, après coup :** branches protégées inchangées, aucune nouvelle référence distante, worktree toujours sur sa branche, base toujours ancêtre. Toute violation est annoncée en urgence.

**Supervision :**
- Groupe de processus propre (`process_group(0)`) : `cargo test` lancé par l'agent meurt avec lui ; `kill` du système plutôt qu'une dépendance `nix`/`libc`.
- 45 min au plus, 10 min sans sortie = bloquée ; 2 tâches au plus, une par projet ; un projet Rust compile beaucoup.
- Le registre survit au redémarrage, pas les processus : systemd tue tout le cgroup du service à l'arrêt, et se raccrocher à un `claude` orphelin serait fragile. Au démarrage, `Running` → `Interrupted` + notification ; le worktree et la branche restent.

**Suivi sans IA :** dernière action et durée tirées du flux `stream-json`. C'est instantané, ça marche dans les deux modes, et rien ne part en ligne pour un simple « où en est la tâche ? ». Le résumé de fin est la ligne `RÉSUMÉ` de l'agent (déjà en ligne) plus le nombre de fichiers, calculé par git.

**Annonce de fin :** la relève par l'oreille (5 s au plus de retard) marche avec le socket Unix actuel sans connexion persistante ; la notification part tout de suite, que l'oreille tourne ou non. Le tableau ne connaît que des noms d'oreille : en phase 4, l'identité vient de l'appairage et `changed()` permet l'envoi immédiat.

## Questions ouvertes pour Charles

1. Commit de fin par Jarvis (signé par ta clé, message `wip(jarvis): …`), ou travail laissé non enregistré dans le worktree ?
2. Fichiers non suivis ignorés par le refus « modifications non enregistrées » : d'accord ?
3. Modèle de l'agent (défaut de Claude Code ou `opus`) et plafond `--max-budget-usd` ? `ANTHROPIC_API_KEY` retirée pour utiliser ton abonnement : d'accord ?
4. 45 min, 10 min sans sortie, 2 tâches : d'accord ? Worktrees dans `~/.local/share/jarvis/worktrees/`, nettoyage à la main ?
5. Jamais de déclenchement par l'IA, même en mode en ligne : il faut nommer le projet. D'accord ?
6. Annonce de fin jouée même quand l'écoute est coupée ?
7. Nom d'oreille par défaut `pc` dans `Listen.ear` : à aligner avec le plan de la phase 4.
