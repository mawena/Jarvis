# Jarvis, phase 8 (personnes et rôles) : plan d'implémentation

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal :** le cerveau connaît des **personnes** et des **rôles** (nom + permissions : outils autorisés, droit de confirmer les actions risquées, permission `agent`), chaque oreille appairée appartient à une personne, et chaque outil est vérifié contre les droits de **qui parle** avant d'être lancé (« Je ne peux pas faire ça pour vous. » sinon). Aujourd'hui une seule personne existe, Charles, rôle `admin` : rien ne change pour lui. Tout se gère par `jarvis person …` et `jarvis role …` (socket Unix, même utilisateur), pas par la voix.

**Architecture :** un **annuaire** `People` (`jarvis-brain/src/people.rs`, fichier `~/.local/share/jarvis/people.toml`, 0600, écriture atomique, partagé en `Arc` entre le cerveau, l'écoute, le hub et le serveur du socket) contient rôles, personnes et rattachement des oreilles. Une fonction **pure** `identity::identify` décide « qui parle » à partir de l'oreille et du verdict de la vérification de voix. Le résultat, un `Speaker` (personne, rôle, niveau de confiance, droits), est passé au `Brain`, dont le **garde-fou d'outil** (`run_or_ask`, `answer_follow_up`) refuse ce que les droits n'autorisent pas, **sans toucher à la liste d'outils envoyée à l'IA** (le cache du modèle local reste chaud). La gestion passe par de nouvelles requêtes du socket de contrôle.

**Tech Stack :** aucune nouvelle crate (`toml`, `serde`, `tracing`, `clap` déjà là). Existant réutilisé : `write_private` (`ears.rs`, écriture atomique 0600), `Verdict` (`speaker`), `Risk` (`jarvis-skills`).

**Spec :** [docs/superpowers/specs/2026-10-28-jarvis-design.md](../specs/2026-10-28-jarvis-design.md) (§2 objectif « Rôles », §5.1, §5.2 `jarvis-cli`, §5.4 compréhension, §5.6 confirmations, §5.8 personnes et rôles, §6.4 permission `agent`)

## Global Constraints

- **Rien ne change pour Charles** : sans `people.toml` (installation d'avant la phase 8), le cerveau crée l'annuaire avec Charles `admin` et toute oreille non rattachée vaut Charles. Tous les tests des phases 2 à 7 (dont `tests/phrases.rs`, `tests/agent.rs`) restent verts **sans modification de leurs attentes**.
- **Droit refusé** : réponse exacte « Je ne peux pas faire ça pour vous. » (spec §5.8), issue `Failed` (bip d'échec), journal `info!` avec personne, rôle, outil. Jamais d'action en attente créée pour un outil refusé.
- **Règles de l'agent de code conservées (phase 7, spec §6.4)** : un outil `agent_*` n'est lancé que par une voix **reconnue** par l'empreinte de Charles (`Trust::Recognized`) ou par du texte tapé (`Trust::Typed`), **et** si le rôle a la permission `agent`. Une voix non vérifiée, un « oui » court, `answer_fast` : jamais.
- **Action risquée (spec §5.6)** : jamais demandée à une voix `Unverified`, jamais exécutée sans le droit `confirm` de la personne qui répond « oui », **sur la même oreille** que la question (règle de la phase 4 conservée). Texte tapé : toujours « demandez-le à voix haute » (règle de la phase 3 conservée).
- **Admin intouchable** : le rôle `admin` est intégré (non stocké, tous les droits, non modifiable, non supprimable) ; la personne propriétaire (`owner = true`, celle de l'empreinte vocale) est `admin`, ne peut être ni supprimée ni changée de rôle. Aucune commande ne peut donc retirer à Charles l'accès à la gestion.
- **Moindre privilège en cas de doute** : personne ou rôle inconnu, fichier `people.toml` illisible ou invalide → aucun droit (sauf voix de Charles reconnue et texte tapé), erreur nette dans le journal et dans `jarvisctl doctor`. Jamais de repli sur « admin » pour une oreille dont la personne a disparu.
- **Gestion par le socket Unix seulement** (0600, même utilisateur) : le protocole des oreilles (`EarMessage`) n'a aucun message de gestion, un message inconnu y est ignoré. Pas de gestion vocale dans cette phase.
- **Fichier** `people.toml` : dossier 0700, fichier 0600, écriture atomique (`write_private`), état en mémoire adopté seulement si l'écriture réussit. Noms de personnes et de rôles : 1 à 40 caractères, sans caractère de contrôle, uniques sans tenir compte de la casse et des accents, `admin` réservé.
- **Journaux** : jamais de son ; nom de personne, rôle et outil journalisés (comme le texte des phrases).
- Français partout (code, commentaires, messages). Édition 2024. Pas de nouvelle dépendance. Jamais `cargo update` sans `-p` (`half` figé à 2.4.1).
- **Machine (règles de Charles)** : `CARGO_TARGET_DIR=/home/charles-gamligo/Work/perso/Jarvis/target` partagé, jamais un autre dossier cible. **Toute commande lourde** (`cargo build`, `cargo test`, `cargo clippy`, `./scripts/check.sh`) passe par la file commune : `file-attente "<nom>" <Go de RAM> <cœurs> -- <commande>` (par défaut `file-attente "p8-tN check" 6 4 -- ./scripts/check.sh`, valeurs à ajuster d'après `file-attente --etat`), jamais deux travaux lourds en même temps (la machine a déjà redémarré). Les agents d'une vague écrivent en parallèle mais leurs compilations se suivent dans la file. Les worktrees de tâches vivent sous `.claude/worktrees/p8-tN` (branche `p8-tN`, fusionnée dans `feature/personnes-roles` après relecture). Deux agents ne modifient jamais le même fichier. Commits fréquents.
- Tests : jamais le micro, jamais de son, jamais les services ni le vrai `claude -p`, aucun accès réseau hors `127.0.0.1`, fichiers dans des dossiers temporaires.
- Règles des phases précédentes : git flow, commits au seul nom de Charles (aucune ligne `Co-Authored-By` ni attribution), `ROADMAP.md` coché dès qu'une tâche est validée, `scripts/check.sh` vert (fmt, clippy `-D warnings`, tests). **En fin de phase** : `git flow feature finish personnes-roles`, puis push de `develop` (et de `main` et des étiquettes si une release suit) ; proposer à Charles la release (majeure, mineure ou correctif) avant de la lancer.

## Review Focus

1. **Un invité n'obtient jamais une action risquée ni l'agent**, même si l'IA invente ou choisit l'outil, même par un raccourci → `outil_choisi_par_l_ia_mais_interdit_refuse`, `agent_refuse_sans_droit_agent`, `invite_ne_peut_pas_mettre_en_veille` (tâche 5), `agent_refuse_sur_l_oreille_de_lea` (tâche 9).
2. **Qui parle** : la voix de Charles reconnue sur l'oreille d'un invité vaut Charles ; une voix inconnue sur l'oreille de Charles reste ignorée comme avant ; une voix inconnue sur l'oreille d'un invité vaut l'invité, sans restriction levée → table complète `identify_*` (tâche 6), `voix_de_charles_sur_l_oreille_de_lea_vaut_charles`, `voix_inconnue_sur_l_oreille_de_charles_ignoree_comme_avant` (tâche 9).
3. **Confirmation** : seul celui qui a le droit `confirm`, d'une voix non `Unverified`, sur la même oreille, confirme ; les droits sont revérifiés **au moment du « oui »** (rôle changé entre-temps) → `confirmation_sans_droit_refusee`, `role_retire_entre_question_et_oui_refuse` (tâche 5).
4. **Verrouillage et fichier abîmé** : impossible de supprimer Charles ou `admin` ; `people.toml` illisible, rôle inconnu ou personne supprimée à la main → moindre privilège, jamais admin → `proprietaire_non_supprimable`, `fichier_illisible_moindre_privilege`, `role_inconnu_aucun_droit` (tâche 2), `personne_supprimee_oreille_sans_droit` (tâche 6).
5. **Compatibilité** : sans `people.toml`, comportement identique à la phase 7 ; ancien `PairStart` sans personne, ancien cerveau sans `PersonList` (le `doctor` ne casse pas), `ears.toml` inchangé → `ancien_pair_start_sans_personne` (tâche 1), `annuaire_cree_avec_charles_admin`, `cerveau_par_defaut_se_comporte_comme_avant` (tâches 2 et 5), `doctor_cerveau_sans_personnes` (tâche 7).

---

Démarrage : la branche `feature/personnes-roles` existe déjà (créée depuis `develop` par `git flow feature start personnes-roles`, worktree `.claude/worktrees/phase8`).

## Décisions

| Question | Décision | Pourquoi |
|---|---|---|
| **Qui parle ?** | La personne est celle de **l'oreille** ; mais une voix **reconnue** par l'empreinte de Charles vaut Charles sur n'importe quelle oreille. Une voix non reconnue sur l'oreille d'un invité vaut l'invité en niveau `Unverified` (ni risqué, ni agent). Sur une oreille de Charles, une voix non reconnue reste **ignorée** (`NoMatch`, `TooShort`) ou limitée aux raccourcis (`SessionShort`), comme aujourd'hui. | Spec §5.8 : « au départ, la personne est déduite de l'appareil ». L'empreinte ne connaît que Charles : elle prouve Charles, jamais un invité. Un invité n'est donc jamais « vérifié » : il ne peut rien de risqué, ce qui est le bon défaut. |
| Empreintes vocales des autres personnes | **Reportées** (« plus tard », spec §5.8). Le droit `confirm` et la permission `agent` d'un rôle d'invité sont stockés et vérifiés, mais n'agissent qu'avec une voix reconnue : ils prendront effet avec les empreintes par personne. | Le coût réel est élevé (enrôlement par personne, `SpeakerGate` multi-empreintes, choix des seuils) et hors du besoin d'aujourd'hui (une seule personne). `identify` est pur : il suffira de lui donner un verdict par personne. |
| Texte tapé (`jarvis say`, socket) | Vaut le propriétaire, rôle `admin`, niveau `Typed` (peut lancer l'agent, comme en phase 7 ; action risquée toujours à demander à voix haute). | Socket 0600 du même utilisateur : preuve d'identité suffisante (déjà admis pour l'appairage). |
| Où vérifier | Dans `Brain::run_or_ask` (avant la question de confirmation) et dans `answer_follow_up` (au « oui »), à partir d'un `Speaker` fourni par l'écoute. Aucun filtrage de la liste d'outils vue par l'IA. | Un seul point de passage pour raccourcis, IA et confirmation. Filtrer la liste changerait le prompt (cache Ollama chaud perdu, précision réglée en phase 1 à 82-89 % non remesurée) ; refuser après coup coûte une phrase, pas la justesse. |
| Format des permissions | `tools = ["media_*", "app_launch", "web_search"]` : noms d'outils ou préfixe `x_*`, `*` pour tout. Drapeaux `confirm` et `agent`. Les outils `agent_*` ne dépendent que de `agent`. Les outils risqués (`suspend`, `reboot`, `shutdown`) exigent l'outil dans la liste **et** `confirm`. | Spec §5.8 : « compétences ou outils autorisés, et droit de confirmer ». Les noms d'outils existent déjà ; pas de nouvelle notion de « groupe ». Une faute de frappe est refusée à la création (motif sans aucun outil). |
| Rattachement oreille → personne | Stocké **dans `people.toml`** (`ears = ["<id>", …]` sous chaque personne), pas dans `ears.toml`. | L'annuaire est autonome et partageable en `Arc` entre écoute, hub et serveur sans passer par le registre du hub ; `ears.toml` et le protocole des oreilles restent inchangés. |
| Oreille sans personne (appairée avant la phase 8) ou oreille locale | **Charles** (le propriétaire). Les oreilles appairées après : rattachées explicitement (`jarvis pair --person <nom>`, par défaut Charles) ; si le rattachement échoue, l'appairage échoue. Personne supprimée à la main : droits nuls. | Migration sans écriture ni redémarrage, et sans changer le comportement d'aujourd'hui. Seul un oreille *non rattachée* retombe sur Charles ; une oreille rattachée à une personne disparue n'a aucun droit. |
| Gestion par la voix | **Non**, CLI seulement. | Spec §5.8 : « pas de commande vocale pour ça au départ », et §5.6 la classe en action risquée. Une phrase mal reconnue ou une IA qui hallucine ne doit pas pouvoir créer un administrateur. À reprendre avec les empreintes par personne. |
| Fichier illisible | Cerveau démarré en **mode dégradé** : voix de Charles reconnue et texte tapé = admin, toutes les oreilles sans droit ; message dans le journal et `doctor` ; écritures de gestion refusées. | Ni panne totale (le cerveau reste utilisable depuis le clavier), ni ouverture à tous. |
| Nom du propriétaire | Réglage `[people] owner = "Charles"` (le nom d'adresse « Monsieur » n'est pas un nom). | Utilisé seulement à la création du fichier. |

Mapping de `identify` (verdict de la vérification → qui parle) :

| Oreille de… | `Match` | `NotEnrolled` ou pas de vérification | `SessionShort` | `NoMatch` / `TooShort` |
|---|---|---|---|---|
| Charles | Charles `Recognized` | Charles `Accepted` (comme aujourd'hui : tout sauf l'agent) | Charles `Unverified`, raccourcis seuls | **ignoré** |
| un invité | Charles `Recognized` | invité `Unverified` | invité `Unverified`, raccourcis seuls | invité `Unverified` (raccourcis seuls si `TooShort`) |
| personne introuvable | Charles `Recognized` | « inconnu » sans droit | idem | idem |

## Ordre et parallélisme

| Vague | Tâches | Dépend de |
|---|---|---|
| 0 | **Tâche 1** socle (protocole, squelettes, câblage du cerveau) | — |
| A (2 agents) | **Tâche 2** annuaire, rôles, droits · **Tâche 3** CLI `jarvis person/role/pair --person` | 1 |
| B (2 agents) | **Tâche 4** hub, serveur du socket, registre, `main` · **Tâche 5** garde-fou du `Brain` | 2 (et 1) |
| C (3 agents) | **Tâche 6** identification et écoute · **Tâche 7** `doctor` · **Tâche 8** documentation et fiche d'essai | 6 : 2, 5 ; 7 : 4 ; 8 : 3, 4 |
| D | **Tâche 9** tests de bout en bout · **Tâche 10** relecture (session principale) | 4, 5, 6 |
| E | **Tâche 11** essai réel avec Charles | tout |

Chaque tâche ne touche **que** ses fichiers. Au plus 4 agents à la fois **sur tout le dépôt**, phase 5 comprise (voir « Avec la phase 5 »).

### Avec la phase 5 (oreille Android, branche `feature/oreille-android`, en parallèle)

La phase 5 a déjà fusionné ses tâches 1 et 2 (session de l'oreille du PC passée dans `jarvis-ear-core`, aucun fichier du cerveau). Sa **tâche 4** (en cours de reprise) change les lignes de journal du score de voix dans `crates/jarvis-brain/src/speaker/mod.rs` et `listen.rs` (oreille et seuil) et ajoute une ligne `doctor` dans `crates/jarvisctl/src/ops.rs`. Sa tâche 1 modifie aussi un commentaire de `crates/jarvis-brain/src/net/hub.rs` (`LISTEN_TIMEOUT`) et sa tâche 9 la documentation (`README.md`, `docs/reseau.md`, `docs/essais/README.md`, `docs/oreille.md`).

| Tâche de la phase 8 | Fichier partagé avec la phase 5 | Conséquence |
|---|---|---|
| 6 (identification, écoute) | `crates/jarvis-brain/src/listen.rs` (le bloc du verdict `Verdict::NoMatch(score) => info!(…)` est précisément celui que la tâche 6 remplace) | **Conflit réel** : lancer la tâche 6 seulement après la fusion de la tâche 4 de la phase 5 dans `develop`, ou rebaser `feature/personnes-roles` dessus et conserver ses champs `oreille` et `seuil` dans les `info!`. `speaker/mod.rs` n'est **pas** modifié par la phase 8 (c'est pourquoi `identify` vit dans un fichier à part). |
| 7 (`doctor`) | `crates/jarvisctl/src/ops.rs` | **Conflit probable** (même zone, fonction `doctor`) : après la fusion de la tâche 4 de la phase 5. |
| 1 (littéraux `EarSummary`) | `crates/jarvisctl/src/ops.rs` (test `doctor_liste_les_oreilles`), `crates/jarvis-brain/src/net/hub.rs` | Retouche mécanique d'une ligne ; rebaser, conflit trivial. |
| 4 | `crates/jarvis-brain/src/net/hub.rs` | Commentaire seulement côté phase 5 : conflit trivial. |
| 8 | `README.md`, `docs/reseau.md`, `docs/essais/README.md` | Sections différentes ; résoudre à la fusion. |
| 2, 3, 5, 9 | aucun | Libres, à lancer tout de suite. |

Ordre conseillé : vagues 0, A et B tout de suite (aucun fichier commun, sauf la ligne triviale de la tâche 1) ; la vague C attend la fusion de la tâche 4 de la phase 5. La file `file-attente` sérialise de toute façon les compilations des deux phases.

---

### Task 1 : socle (protocole, squelette de l'annuaire, câblage minimal) (8.2, 8.5)

**Files :** Modify `crates/jarvis-protocol/src/{lib.rs,ear.rs}` ; Create `crates/jarvis-protocol/src/people.rs` ; Modify `crates/jarvis-brain/src/{lib.rs,brain.rs,server.rs,net/hub.rs}`, `crates/jarvis-cli/src/{main.rs,pair.rs}`, `crates/jarvis-cli/tests/cli.rs`, `crates/jarvisctl/src/ops.rs` (test seulement), `crates/jarvis-brain/tests/{reseau.rs,serveur_oreilles.rs}` (littéraux mécaniques) ; Create `crates/jarvis-brain/src/{people.rs,identity.rs}` (squelettes).

**Interfaces :**
- `jarvis_protocol::people` (nouveau) : `struct PersonSummary { name: String, role: String, owner: bool, ears: Vec<String> /* noms */, voiceprint: bool }` ; `struct RoleSummary { name: String, tools: Vec<String>, confirm: bool, agent: bool, builtin: bool, people: usize }` (serde, `Debug`).
- `ControlRequest` gagne : `PairStart { #[serde(default, skip_serializing_if = "Option::is_none")] person: Option<String> }` (ancien variant unitaire : `{"type":"pair_start"}` reste lisible) · `PersonList` · `PersonAdd { name, role }` · `PersonRemove { name }` · `PersonRole { name, role }` · `PersonAttach { name, ear }` (`ear` : nom, identifiant ou début d'identifiant) · `RoleList` · `RoleAdd { name, tools: Vec<String>, #[serde(default)] confirm: bool, #[serde(default)] agent: bool }` · `RoleEdit { name, tools: Option<Vec<String>>, confirm: Option<bool>, agent: Option<bool> }` · `RoleRemove { name }`.
- `ControlResponse` gagne : `People { people: Vec<PersonSummary>, degraded: Option<String> }` · `Roles { roles: Vec<RoleSummary> }` ; les modifications répondent `Reply { speech }` ou `Error { message }`. Le `Debug` écrit à la main du type reçoit les deux bras.
- `EarSummary` gagne `#[serde(default)] person: Option<String>` (nom de la personne ; `None` côté oreille). Littéraux existants complétés par `person: None`.
- `jarvis_brain::people` (squelette, corps `todo!("tâche 2")` sauf la ligne ci-dessous) et `identity` (squelette, `todo!("tâche 6")`) déclarés dans `lib.rs`. Signatures figées (voir tâches 2 et 6). Seul `People::admin_only(owner: &str) -> People` est **réel** (annuaire en mémoire : Charles `admin`, aucune oreille rattachée) ; il sert de valeur par défaut.
- `Brain` : champ `people: Arc<People>` (défaut `Arc::new(People::admin_only("Charles"))`), `Brain::with_people(self, Arc<People>) -> Brain`, `Brain::people(&self) -> &Arc<People>`. Aucune autre modification du comportement de `brain.rs` dans cette tâche.
- `server.rs` : les nouvelles requêtes reçoivent provisoirement `Error { message: "pas encore disponible" }` (la tâche 4 les implémente). `PairStart { .. }` ignore `person` (idem).
- `jarvis-cli` : `ControlRequest::PairStart { person: None }` ; colonne « Personne » ajoutée à `ears_table` (valeur `-` si `None`).

- [ ] Tests (`jarvis-protocol`) : `ancien_pair_start_sans_personne` (`{"type":"pair_start"}` → `PairStart { person: None }`), `pair_start_avec_personne_aller_retour`, `requetes_de_gestion_aller_retour` (les 9 nouvelles requêtes), `role_edit_champs_absents_restent_none`, `reponses_people_et_roles_aller_retour`, `ear_summary_ancien_sans_person_lisible`, `debug_des_reponses_de_gestion`.
- [ ] Implémenter, corriger les appels et littéraux existants (`grep -rn "EarSummary {\|PairStart" crates`), tout compile.
- [ ] `file-attente "p8-t1 check" 6 4 -- ./scripts/check.sh` vert ; commit `feat: protocole des personnes et des rôles, squelettes de l'annuaire`.

### Task 2 : annuaire, rôles et droits (`people.rs`) (8.2, 8.3)

**Files :** `crates/jarvis-brain/src/people.rs` (corps), `crates/jarvis-brain/src/config.rs` (section `[people]`)

**Interfaces :**
- Config : `struct PeopleConfig { owner: String /* défaut "Charles" */ }` dans `Config.people` (`#[serde(default)]`).
- `enum Trust { Typed, Recognized, Accepted, Unverified }` (`Copy`). `struct Rights { all: bool, tools: Vec<String>, confirm: bool, agent: bool }`. `struct Speaker { person: String, role: String, trust: Trust, rights: Rights }` (`Clone`, `Debug`, `PartialEq`).
- `enum Access { Allowed, Denied }` ; `impl Speaker { fn check(&self, tool: &str, risk: Risk) -> Access ; fn may_confirm(&self, tool: &str, risk: Risk) -> bool }`. Règles de `check` (dans cet ordre) :
  1. `tool` commence par `agent_` → autorisé seulement si `rights.agent` (ou `all`) **et** `trust ∈ {Recognized, Typed}` ;
  2. outil absent de `rights.tools` (motif exact, `préfixe*`, ou `*`) et pas `all` → refusé ;
  3. `risk == Risky` → autorisé seulement si `rights.confirm` (ou `all`) **et** `trust != Unverified` ;
  4. sinon autorisé. `may_confirm` : `rights.confirm`/`all`, `trust ∈ {Recognized, Accepted}`, et `check` autorisé.
- `enum EarPerson { Owner, Guest(String), Unknown }`. `enum PeopleError { Invalid(String), Exists(String), Unknown(String), Protected(String), InUse(String), Degraded, Io(io::Error) }` (messages français via `Display`).
- `struct People` (`RwLock` interne) : `open(path: &Path, owner: &str) -> io::Result<People>` (crée le fichier avec le propriétaire `admin` s'il manque) · `open_or_degraded(path, owner) -> People` (illisible ou invalide → mode dégradé) · `admin_only(owner)` (déjà réel) · `default_path() -> PathBuf` (`~/.local/share/jarvis/people.toml`) · `degraded(&self) -> Option<String>` · `owner_name(&self) -> String` · `person_of(&self, &EarId) -> EarPerson` (non rattachée → `Owner` ; rattachée à une personne disparue → `Unknown`) · `speaker(&self, person: &str, trust: Trust) -> Speaker` (personne ou rôle inconnu → droits nuls) · `owner_speaker(&self, Trust) -> Speaker` · `typed(&self) -> Speaker` (propriétaire, `Trust::Typed`) · `add_person(name, role)` · `remove_person(name)` (refuse le propriétaire ; refuse une personne qui a des oreilles) · `set_role(name, role)` (refuse de changer le propriétaire) · `attach(&self, person: &str, &EarId)` (détache d'abord de l'ancienne) · `detach(&self, &EarId)` · `prune(&self, known: &[EarId])` · `add_role(name, tools, confirm, agent, known_tools: &[String])` · `edit_role(name, Option<tools>, Option<confirm>, Option<agent>, known_tools)` · `remove_role(name)` (refuse `admin` et un rôle utilisé) · `people_summary(&self, ear_names: &dyn Fn(&EarId) -> Option<String>, voiceprint: bool) -> Vec<PersonSummary>` · `roles_summary(&self) -> Vec<RoleSummary>`.
- Validation : `validate_name(&str) -> Result<String, PeopleError>` (1 à 40 caractères après rognage, pas de caractère de contrôle, `admin` réservé pour un rôle) ; unicité par `fold` (minuscules, accents retirés : `jarvis_skills::text::fold`) ; chaque motif de `tools` doit correspondre à au moins un outil de `known_tools` (sinon `Invalid("aucun outil ne correspond à « x »")`) ; `agent_*` ne s'écrit pas dans `tools` (refusé : « utilisez le droit agent »).
- Fichier : `[[role]] name, tools, confirm, agent` ; `[[person]] name, role, owner, ears`. Rôle `admin` jamais écrit ; présent dans le fichier → ignoré avec avertissement. Deux propriétaires, aucun propriétaire, propriétaire non `admin`, nom en double → fichier invalide (mode dégradé).
- Mode dégradé : `owner_speaker` et `typed` valent un admin ; `person_of` rend `Unknown` pour toute oreille ; toute écriture rend `PeopleError::Degraded`.

- [ ] Tests (dossier temporaire) : `annuaire_cree_avec_charles_admin`, `fichier_prive_0600_dossier_0700`, `relu_apres_redemarrage`, `ecriture_atomique_en_cas_d_echec_etat_inchange`, `oreille_non_rattachee_vaut_le_proprietaire`, `oreille_rattachee_a_une_personne`, `attacher_detache_de_l_ancienne_personne`, `personne_supprimee_oreille_sans_droit` (édition à la main puis rechargement), `proprietaire_non_supprimable`, `proprietaire_non_modifiable`, `personne_avec_oreilles_non_supprimable`, `role_admin_non_modifiable_ni_supprimable`, `role_en_service_non_supprimable`, `nom_double_refuse_sans_casse_ni_accents`, `nom_vide_trop_long_ou_de_controle_refuse`, `motif_sans_outil_refuse`, `motif_agent_refuse_dans_tools`, `fichier_illisible_moindre_privilege`, `role_inconnu_aucun_droit`, `deux_proprietaires_fichier_invalide`, `mode_degrade_refuse_les_ecritures`, `prune_retire_les_oreilles_disparues`.
- [ ] Tests des droits (purs, sans fichier) : `admin_tout_sauf_ce_que_la_confiance_interdit`, `admin_unverified_ni_risque_ni_agent`, `admin_accepted_tout_sauf_l_agent` (comme aujourd'hui sans empreinte), `admin_recognized_et_typed_peuvent_l_agent`, `invite_media_seulement`, `motif_prefixe_media_etoile`, `outil_risque_exige_confirm_et_confiance`, `agent_exige_droit_et_voix_reconnue`, `may_confirm_exige_confiance_et_droit`, `invite_avec_confirm_mais_unverified_ne_confirme_pas`.
- [ ] `file-attente "p8-t2 check" 6 4 -- ./scripts/check.sh` vert ; commit `feat: annuaire des personnes et des rôles, droits`.

### Task 3 : CLI `jarvis person`, `jarvis role`, `jarvis pair --person` (8.5)

**Files :** `crates/jarvis-cli/src/{main.rs,people.rs (nouveau)}`, `crates/jarvis-cli/tests/cli.rs`

**Interfaces :**
- `jarvis person list | add <nom> --role <rôle> | remove <nom> | role <nom> <rôle> | attach <nom> <oreille>` ; `jarvis role list | add <nom> [--tools a,b,c] [--confirm] [--agent] | edit <nom> [--tools …] [--confirm | --no-confirm] [--agent | --no-agent] | remove <nom>` ; `jarvis pair [--person <nom>]`. Noms à plusieurs mots acceptés (`num_args = 1..` comme `unpair`).
- `people::people_table(&[PersonSummary]) -> String` (Nom, Rôle, Oreilles, Empreinte vocale ; propriétaire marqué) et `people::roles_table(&[RoleSummary]) -> String` (Nom, Outils, Confirmer, Agent, Personnes ; `admin` : « tout »). Mode dégradé : première ligne d'avertissement. Mêmes largeurs de colonnes que `pair::ears_table`.
- Codes de sortie comme aujourd'hui (0 ; 1 erreur du cerveau ; 2 cerveau absent).

- [ ] Tests (faux cerveau sur socket Unix temporaire, comme `pair_affiche_code_et_attend_la_fin`) : `person_list_affiche_le_tableau`, `person_add_envoie_la_requete`, `person_remove_et_role_envoient_les_requetes`, `person_attach_nom_a_plusieurs_mots`, `role_add_decoupe_les_outils_par_virgule`, `role_edit_sans_option_refuse_par_clap`, `role_edit_confirm_et_no_confirm_exclusifs`, `role_list_affiche_le_tableau`, `erreur_du_cerveau_code_1`, `pair_person_envoie_le_nom`, `ears_montre_la_colonne_personne`, `tableau_personnes_marque_le_proprietaire`.
- [ ] `file-attente "p8-t3 check" 6 4 -- ./scripts/check.sh` vert ; commit `feat: jarvis person, jarvis role et jarvis pair --person`.

### Task 4 : hub, serveur du socket, registre, démarrage (8.2, 8.5)

**Files :** `crates/jarvis-brain/src/{server.rs,main.rs,ears.rs}`, `crates/jarvis-brain/src/net/hub.rs`, `crates/jarvis-brain/tests/serveur_oreilles.rs` (ajouts)

**Interfaces :**
- `ears.rs` : `EarRegistry::find(&self, name_or_id: &str) -> io::Result<Option<EarRecord>>` (même résolution que `remove` : identifiant, nom unique, début d'identifiant ≥ 4 caractères ; ambigu → `InvalidInput`), `remove` réécrit par-dessus.
- `hub.rs` : `Hub::pair(person)` mémorise la personne dans `OpenWindow` ; à l'appairage réussi : `registry.add(..)` puis `people.attach(person, &record.id)` ; échec du rattachement → `registry.remove` de l'oreille créée et fenêtre fermée (`pairing_ended::not_saved`). `pair_local` : l'oreille locale reçoit l'identifiant neuf et est rattachée au propriétaire (l'ancienne est détachée par `prune`). `unpair` : `people.detach(&record.id)`. Les `EarSummary` (liste, état diffusé) portent `person` (`people.person_of(..)` → nom du propriétaire pour une oreille non rattachée, `None` pour `Unknown`). `Hub::find_ear(&str) -> Result<EarSummary, String>` pour `PersonAttach`. L'annuaire vient de `services.brain.people()` (aucun champ ajouté à `Services`).
- `server.rs` : implémente les requêtes de la tâche 1 : `PairStart { person }` (personne inconnue → `Error` **avant** d'ouvrir la fenêtre ; `None` → propriétaire) ; `PersonList` → `People { people, degraded }` (noms d'oreilles par `hub.list_ears()`, `voiceprint` = empreinte du propriétaire enregistrée) ; `PersonAdd/Remove/Role/Attach`, `RoleList/Add/Edit/Remove` → `Reply`/`Error` (message de `PeopleError`). `known_tools` = noms d'outils du cerveau (`Brain::tool_names()`, ajouté en tâche 5 : voir interface). Sans hub, `PersonAttach` rend l'erreur « réseau des oreilles désactivé ». Chaque modification : `info!` (qui, quoi).
- `main.rs` : `People::open_or_degraded(&People::default_path(), &config.people.owner)` partagé avec `Brain::with_people` ; `people.prune(..)` avec les identifiants du registre au démarrage ; mode dégradé → `warn!` explicite.

- [ ] Tests (`ears.rs`) : `find_par_nom_id_et_prefixe`, `find_ambigu_refuse`. Tests (`tests/serveur_oreilles.rs`, vrai TLS et faux socket comme les tests d'appairage) : `appairage_attache_a_la_personne_choisie`, `appairage_par_defaut_au_proprietaire`, `appairage_personne_inconnue_refuse_sans_ouvrir_la_fenetre`, `echec_du_rattachement_defait_l_appairage`, `retrait_detache_l_oreille`, `oreille_locale_reappairee_reste_au_proprietaire`, `ears_liste_la_personne`, `person_add_list_remove_par_le_socket`, `person_attach_par_nom_ou_identifiant`, `person_remove_proprietaire_refuse`, `role_admin_intouchable`, `role_en_service_non_supprimable`, `outil_inconnu_dans_un_role_refuse`, `mode_degrade_signale_dans_person_list`, `changement_de_role_sans_redemarrage`.
- [ ] `file-attente "p8-t4 check" 6 4 -- ./scripts/check.sh` vert ; commit `feat: rattachement des oreilles et gestion des personnes par le socket`.

### Task 5 : garde-fou d'outil du `Brain` (8.4)

**Files :** `crates/jarvis-brain/src/brain.rs` (tests inclus), `crates/jarvis-brain/src/testing.rs` (doublures si besoin)

**Interfaces :**
- `Brain::answer_as(&self, ear: &EarId, text: &str, who: &Speaker) -> Answer` ; `Brain::answer_fast_as(&self, ear: &EarId, text: &str, who: &Speaker) -> Option<Answer>` ; `Brain::answer_follow_up_as(&self, ear: &EarId, text: &str, spoken_at: Instant, voice: ReplyVoice, who: &Speaker) -> Answer` ; `Brain::tool_names(&self) -> Vec<String>` (outils des compétences et du système). `answer_voice(ear, text, verified)`, `answer_text`, `answer_fast`, `answer_follow_up` **gardés** comme enveloppes qui construisent le `Speaker` du propriétaire (`verified` → `Recognized`, sinon `Accepted` ; `answer_fast` → `Unverified` ; `answer_follow_up` : `Verified` → `Recognized`, `Unverified` → `Unverified`). `handle_text` → `people.typed()`.
- `Source::Voice { ear, who }` remplace `verified: bool` ; « voix reconnue » = `who.trust == Recognized`. Garde-fou dans `run_or_ask`, avant la question de confirmation : `who.check(tool, risk)` refusé → `Reply::failed("Je ne peux pas faire ça pour vous.")`, rien en attente, `info!(personne, role, outil, "droit refusé")`. Raccourci de compétence (agent) : le droit est testé **avant** le message `NEEDS_KNOWN_VOICE` (un invité entend le refus, pas « enregistrez votre voix »). Outil « hors IA » choisi par l'IA : refus inchangé (phase 7).
- `answer_follow_up_as` : la confirmation n'exécute que si `who.may_confirm(tool, risk)` au moment du « oui » (sinon refus ci-dessus, attente effacée) ; l'attente garde la personne qui a posé la question pour le journal. Toute la logique de l'oreille unique, du délai, de la question reposée est conservée.
- La liste d'outils proposée à l'IA (`Brain::tools`) **ne change pas**.

- [ ] Tests : `invite_peut_lancer_une_musique`, `invite_ne_peut_pas_mettre_en_veille` (aucune attente créée), `outil_interdit_par_le_role_refuse` (phrase exacte, issue `Failed`), `outil_choisi_par_l_ia_mais_interdit_refuse`, `l_ia_recoit_tous_les_outils_pour_un_invite` (liste identique à celle de l'admin), `agent_refuse_sans_droit_agent` (message de refus, pas `NEEDS_KNOWN_VOICE`), `agent_exige_voix_reconnue_meme_avec_le_droit`, `texte_tape_vaut_admin_et_peut_l_agent`, `texte_tape_risque_toujours_a_voix_haute`, `voix_non_verifiee_admin_jamais_risque` (inchangé), `confirmation_sans_droit_refusee`, `role_retire_entre_question_et_oui_refuse`, `oui_d_une_autre_oreille_n_execute_pas` (repris), `cerveau_par_defaut_se_comporte_comme_avant` ; tous les tests existants de `brain.rs` verts sans changement d'attente.
- [ ] `file-attente "p8-t5 check" 6 4 -- ./scripts/check.sh` vert ; commit `feat: droits vérifiés avant chaque outil`.

### Task 6 : identification de qui parle, écoute (8.4)

**Files :** `crates/jarvis-brain/src/{identity.rs,listen.rs}` (**attendre la fusion de la tâche 4 de la phase 5** : conflit sur `listen.rs`)

**Interfaces :**
- `identity.rs` (pur) : `enum Identity { Ignore, Speak { speaker: Speaker, short_only: bool } }` ; `pub fn identify(people: &People, ear: &EarId, verdict: Option<Verdict>) -> Identity` (`None` : aucune vérification, désactivée ou installation neuve). Table « Décisions » ci-dessus, à la lettre.
- `listen.rs` : `hear` remplace les drapeaux `recognized` / `unverified` par le `Speaker` rendu par `identify` (annuaire pris sur `self.brain.people()`), en **gardant** le calcul du verdict dans `spawn_blocking`, les lignes de journal de la phase 5 (oreille, score, seuil) et le refus total quand `SpeakerState::Unavailable { enrolled: true }`. `short_only` remplace `unverified` : réponses courtes → `answer_fast_as` seulement ; phrase longue d'un invité `Unverified` → `answer_as` (IA permise, droits contrôlés par le `Brain`). Réponse à une question : la voix est `Verified` seulement si le `Speaker` n'est pas `Unverified` ; sinon `short_reply_voice` (vérification stricte contre l'empreinte de Charles) et, si elle passe, le `Speaker` devient `owner_speaker(Recognized)`. `answered` / réponses `heard` inchangées.

- [ ] Tests (`identity.rs`, fausse `People`) : `identify_oreille_de_charles_match`, `identify_oreille_de_charles_sans_empreinte_accepted`, `identify_oreille_de_charles_session_courte_unverified_raccourcis_seuls`, `identify_oreille_de_charles_nomatch_ignore`, `identify_oreille_de_charles_trop_court_ignore`, `identify_oreille_d_un_invite_match_vaut_charles`, `identify_oreille_d_un_invite_nomatch_vaut_l_invite_unverified`, `identify_oreille_d_un_invite_trop_court_raccourcis_seuls`, `identify_oreille_d_un_invite_sans_verification_unverified`, `identify_oreille_non_rattachee_vaut_charles`, `identify_oreille_de_personne_supprimee_sans_droit`, `identify_mode_degrade_match_seul`.
- [ ] Tests (`listen.rs`, `ByFirstSample` existant) : `invite_phrase_longue_inconnue_executee_selon_ses_droits`, `invite_veille_refusee`, `voix_de_charles_sur_l_oreille_d_un_invite_peut_tout`, `oui_court_de_charles_sur_l_oreille_d_un_invite_confirme_sa_propre_action`, `oui_d_un_invite_ne_confirme_rien` ; **tous** les tests d'écoute des phases 2 et 3 et 7 verts sans changement d'attente.
- [ ] `file-attente "p8-t6 check" 6 4 -- ./scripts/check.sh` vert ; commit `feat: qui parle est décidé par l'oreille et par la voix`.

### Task 7 : `jarvisctl doctor` (8.6)

**Files :** `crates/jarvisctl/src/ops.rs` (**après la fusion de la tâche 4 de la phase 5** : même fonction `doctor`)

**Interfaces :** dans la partie réseau/oreilles de `doctor`, via `sys.brain_request(&ControlRequest::PersonList)` :
- `✓ Personnes et rôles : 1 personne (Charles, admin), 1 rôle intégré` ; avec invités : `✓ Personnes : 2 (Charles admin, Léa famille)`.
- `! Annuaire des personnes illisible (<raison>) — mode dégradé : seule votre voix reconnue et le texte tapé ont des droits ; corrigez ou supprimez <chemin>` (`degraded: Some(..)`).
- `! Oreille « X » sans personne reconnue` pour chaque oreille dont la personne a disparu (croisement avec `ListEars` : `person` absent d'une oreille non locale).
- Ancien cerveau (`Error "requête invalide"`) : une ligne d'information `· Cerveau d'avant les personnes et les rôles`, sans avertissement.

- [ ] Tests (faux `System` existant) : `doctor_affiche_les_personnes`, `doctor_signale_l_annuaire_illisible`, `doctor_signale_l_oreille_sans_personne`, `doctor_cerveau_sans_personnes`, `doctor_cerveau_injoignable_ne_dit_rien_des_personnes`.
- [ ] `file-attente "p8-t7 check" 6 4 -- ./scripts/check.sh` vert ; commit `feat: doctor vérifie les personnes et les rôles`.

### Task 8 : documentation et fiche d'essai (8.6)

**Files :** Create `docs/personnes.md`, `docs/essais/essai-8.8-personnes.md` ; Modify `README.md`, `docs/reseau.md` (« Appairer une oreille » : `--person`), `docs/essais/README.md` (ligne du tableau, essai 8, 25 min), `docs/installation.md` (une phrase : `people.toml`)

- `docs/personnes.md` : à quoi ça sert ; Charles `admin` par défaut ; **qui parle** (tableau « Décisions » en mots simples : l'oreille, la voix de Charles qui vaut partout, un invité jamais vérifié) ; créer un rôle (`jarvis role add famille --tools 'media_*,app_launch,web_search,open_results,play_video'`) ; **liste des outils** (`media_next`, `media_previous`, `media_play`, `media_pause`, `media_now_playing`, `volume_up`, `volume_down`, `app_launch`, `web_search`, `open_results`, `play_video`, `suspend`, `reboot`, `shutdown`, `set_mode`, `status`, `agent_start`, `agent_status`, `agent_stop` : à recontrôler par `grep -rn 'name: "' crates/*/src` avant de livrer) ; `confirm` et `agent` expliqués avec leurs limites actuelles (un invité n'est jamais vérifié) ; créer une personne, rattacher une oreille, appairer pour quelqu'un (`jarvis pair --person Léa`) ; le fichier `people.toml` (modifier à la main = redémarrer le cerveau) ; mode dégradé ; pourquoi pas de gestion vocale ; ce qui viendra (empreintes par personne).
- Fiche d'essai, **style de `essai-4.12-reseau.md`** (but, durée, « Avant de commencer », étapes à cases, **Attendu**, bloc « Résultats » à recopier). Étapes : 1 `jarvisctl update` puis `jarvisctl doctor` (ligne personnes) ; 2 `jarvis person list` et `jarvis role list` (Charles `admin`, oreille du PC à Charles) ; 3 créer le rôle `famille` (média, applis, recherche), la personne « Léa » (`jarvis person add Léa --role famille`) ; 4 deuxième oreille de l'essai 4.12 (`jarvis pair --person Léa`, `jarvis-ear --profile essai pair '…'`), `jarvis ears` montre « Léa » ; 5 **Charles sur son oreille** : « Jarvis, quelle heure est-il ? », puis « mets le PC en veille » → question → « **non** » ; 6 **sur l'oreille de Léa** (la deuxième) : « Jarvis, suivant » marche ; « mets le PC en veille » → « Je ne peux pas faire ça pour vous. » et rien ne se passe ; « finis les tests du projet Jarvis » → refus, aucune tâche lancée ; 7 la voix de Charles sur l'oreille de Léa : « mets le PC en veille » → question (sa voix est reconnue) → « **non** » ; 8 changer le rôle à chaud (`jarvis role edit famille --tools 'media_*'`) puis « lance VS Code » sur l'oreille de Léa → refus, sans redémarrage ; 9 `jarvis person remove Léa` refusé tant qu'elle a une oreille, puis `jarvis unpair`, puis suppression ; `jarvis person remove Charles` refusé ; 10 ranger. Jamais « oui » à une mise en veille réelle.
- `README.md` : paragraphe « Personnes et rôles » et lien vers `docs/personnes.md`.

- [ ] Relire chaque commande citée contre le code des tâches 3 et 4 ; `file-attente "p8-t8 check" 6 4 -- ./scripts/check.sh` vert ; commit `docs: personnes et rôles, fiche d'essai 8.8`.

### Task 9 : tests de bout en bout (8.4)

**Files :** Create `crates/jarvis-brain/tests/personnes.rs`

- Montage repris de `tests/reseau.rs` (vrai `net::serve`, vrai TLS, `127.0.0.1:0`, `FakeTranscriber`, `FakeSkill` avec un outil risqué, `FakeDucker`, `FakeSynthesizer`, deux `EarLink` réels) avec un annuaire à deux personnes (Charles `admin`, Léa rôle `famille` : `media_*`, `app_launch`), l'oreille « salon » à Charles, l'oreille « chambre » à Léa, et un `SpeakerGate` à faux vérificateur (`ByFirstSample`) pour reconnaître Charles.
- Tests : `oreille_de_lea_peut_lancer_une_musique`, `oreille_de_lea_veille_refusee_et_rien_execute`, `agent_refuse_sur_l_oreille_de_lea`, `voix_de_charles_sur_l_oreille_de_lea_vaut_charles`, `voix_inconnue_sur_l_oreille_de_charles_ignoree_comme_avant`, `oui_de_lea_ne_confirme_pas_une_action_de_charles` (autre oreille), `gestion_par_le_socket_effective_a_la_phrase_suivante`, `message_de_gestion_envoye_par_une_oreille_ignore` (un `EarMessage` inconnu `person_add` ne crée personne), `oreille_non_rattachee_vaut_charles_apres_migration` (annuaire créé par `open`, ancienne oreille du registre).
- [ ] Tests verts trois fois de suite ; `file-attente "p8-t9 check" 6 4 -- ./scripts/check.sh` vert ; commit `test: personnes et rôles de bout en bout`.

### Task 10 : relecture et corrections (session principale) (8.7)

- Relecture de toute la branche (sécurité d'abord : chemins vers un outil sans passer par `Speaker::check`, `grep -n "run_or_ask\|execute(" crates/jarvis-brain/src` ; fichier abîmé ; réponses de gestion sans fuite), `file-attente "p8 check" 6 4 -- ./scripts/check.sh`, noter les corrections dans `ROADMAP.md` (8.7).
- Fin de phase : `git flow feature finish personnes-roles`, push de `develop` ; proposer la release à Charles.

### Task 11 : essai réel avec Charles (8.8)

- Fiche `docs/essais/essai-8.8-personnes.md` (tâche 8), à faire avec Charles ; sa deuxième oreille est celle de l'essai 4.12 (ou l'oreille Android de la phase 5 si elle est livrée). Tâche à cocher par Charles seulement.

## Questions ouvertes pour Charles (valeur proposée entre crochets)

1. Faut-il faire **confiance à l'oreille** d'une personne (`trust_ear`) pour qu'un invité puisse confirmer une action risquée sans empreinte ? [Non : un invité ne fait jamais d'action risquée tant que les empreintes par personne n'existent pas.]
2. Nom du propriétaire créé à la migration. [« Charles », réglage `[people] owner`.]
3. Appairer un nouvel appareil sans `--person` : [rattaché à Charles.] Alternative plus stricte : refuser sans `--person` dès qu'il existe une autre personne.
4. Les nouveaux outils (futures compétences) sont-ils autorisés aux invités par défaut ? [Non : liste blanche explicite.]
5. Le rôle `famille` d'exemple doit-il être créé d'office ? [Non, seulement dans la documentation.]
