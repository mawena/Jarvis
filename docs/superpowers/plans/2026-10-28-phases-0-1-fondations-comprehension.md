# Jarvis, phases 0 et 1 (fondations et compréhension par le texte) : plan d'implémentation

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal :** à la fin de ce plan, `jarvis say "suivant"` change la musique et `jarvis say "lance VS Code"` ouvre VS Code, en passant par un cerveau qui tourne en permanence, avec les modes local (Ollama) et en ligne (Claude) et un switch persistant. La voix arrive dans le plan suivant (phase 2).

**Architecture :** un workspace Cargo. `jarvis-brain` est un service qui écoute sur un socket Unix et orchestre : raccourcis rapides → sinon IA (locale ou en ligne) → compétence. `jarvis-skills` contient les compétences derrière un trait `Skill`. `jarvis-protocol` définit les messages partagés. `jarvis-cli` (binaire `jarvis`) parle au cerveau. Les oreilles (phase 2) utiliseront le même `Brain::handle_text`.

**Tech Stack :** Rust édition 2024, tokio 1.54, serde 1 / serde_json 1, toml 1.1, clap 4.6, reqwest 0.13 (json, rustls), zbus 6.1, freedesktop-desktop-entry 0.8, fuzzy-matcher 0.3, tracing 0.1 / tracing-subscriber 0.3, thiserror 2, anyhow 1, async-trait 0.1, directories 6. Tests : wiremock 0.6, tempfile 3.

**Spec :** [docs/superpowers/specs/2026-10-28-jarvis-design.md](../specs/2026-10-28-jarvis-design.md)

## Global Constraints

- Langue des réponses de Jarvis : français, phrases brèves, vouvoiement.
- Configuration : `~/.config/jarvis/config.toml` (TOML). Mode persistant dans `~/.local/state/jarvis/mode`.
- Mode local : la compréhension n'appelle **que** Ollama. Aucune requête vers l'API Claude quand le mode est `local`.
- Si l'IA en ligne est injoignable, Jarvis le dit et propose le mode local ; il ne bascule **jamais** tout seul.
- Le texte des phrases est journalisé (`tracing`, niveau `info`) ; jamais de son.
- Modèles par défaut : local `qwen2.5:3b` sur `http://127.0.0.1:11434` ; en ligne `claude-haiku-5-5`, clé lue dans la variable d'environnement `ANTHROPIC_API_KEY`.
- git flow : phase 0 sur `feature/fondations`, phase 1 sur `feature/comprehension-texte`, chacune terminée par `git flow feature finish`. Commits au nom de Charles uniquement, **aucune** ligne `Co-Authored-By` ni autre attribution.
- `ROADMAP.md` : cocher la tâche correspondante **dès** qu'elle est finie, dans le même commit que le code.
- Avant chaque `feature finish` : `scripts/check.sh` doit passer (`cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`).

## Review Focus

1. **Phrases « sales » venant de la reconnaissance vocale** : majuscules, ponctuation, « Jarvis, » en tête, accents manquants (« precedent »), espaces doublés. Le raccourci doit quand même marcher. → test `normalise_et_tolere_le_bruit` (tâche 4).
2. **Aucun lecteur multimédia ouvert** : « suivant » doit répondre « Aucun lecteur n'est ouvert. », pas une erreur D-Bus. → test `sans_lecteur_reponse_claire` (tâche 5).
3. **Application introuvable** : « lance zorglub » → « Je ne trouve pas l'application zorglub. », sans rien lancer. → test `appli_introuvable` (tâche 6).
4. **L'IA invente un outil ou des arguments invalides** : réponse « Je ne sais pas faire ça. », pas de panique. → test `outil_inconnu_reponse_polie` (tâche 8).
5. **Ollama arrêté en mode local** (connexion refusée) : « L'IA locale ne répond pas. Ollama est-il lancé ? ». → test `ollama_injoignable` (tâche 8).

---

## Phase 0 : fondations (branche `feature/fondations`)

Démarrage : `git flow feature start fondations` depuis `develop`.

### Task 1 : workspace Cargo et protocole partagé

**Files :**
- Create : `Cargo.toml` (workspace, `resolver = "3"`, dépendances partagées dans `[workspace.dependencies]`), `.gitignore` (`/target`), `scripts/check.sh` (exécutable), `README.md` (5 lignes : ce qu'est Jarvis, lien vers la spec et `ROADMAP.md`).
- Create : `crates/jarvis-protocol/Cargo.toml`, `crates/jarvis-protocol/src/lib.rs`
- Create (squelettes vides qui compilent) : `crates/jarvis-skills/`, `crates/jarvis-brain/` (lib + bin `jarvis-brain`), `crates/jarvis-cli/` (bin `jarvis`).

**Interfaces :**
- Produces (dans `jarvis_protocol`) :
  - `enum Mode { Local, Online }` : serde `rename_all = "lowercase"`, `Display` (« local » / « en ligne »), `FromStr` acceptant `local`, `online`, `en-ligne`.
  - `enum ControlRequest { Say { text: String }, Status, SetMode { mode: Mode } }` : serde `tag = "type"`, `rename_all = "snake_case"`.
  - `enum ControlResponse { Reply { speech: String }, Status { mode: Mode, version: String }, Error { message: String } }` : même représentation.
  - `fn socket_path() -> PathBuf` : `$XDG_RUNTIME_DIR/jarvis/brain.sock`, sinon `/tmp/jarvis-<uid>/brain.sock`.
  - Format sur le socket : une ligne JSON par message (`\n`).

- [ ] **Step 1 : écrire les tests** dans `crates/jarvis-protocol/src/lib.rs` (`#[cfg(test)]`) :
  - `say_serialise` : `serde_json::to_string(&ControlRequest::Say{text:"suivant".into()})` == `{"type":"say","text":"suivant"}`.
  - `set_mode_serialise` : `SetMode{mode: Mode::Online}` == `{"type":"set_mode","mode":"online"}`.
  - `aller_retour_reponse` : chaque variante de `ControlResponse` survit à `to_string` puis `from_str`.
  - `mode_depuis_texte` : `"en-ligne".parse::<Mode>()` == `Ok(Mode::Online)`, `"bof".parse::<Mode>()` est une erreur.
  - `socket_sous_xdg_runtime` : avec `XDG_RUNTIME_DIR=/run/user/1000`, `socket_path()` == `/run/user/1000/jarvis/brain.sock`.
- [ ] **Step 2 :** `cargo test -p jarvis-protocol` → échec de compilation (types absents).
- [ ] **Step 3 :** implémenter les types et `socket_path` ; `scripts/check.sh` lance les trois commandes de vérification (`set -euo pipefail`).
- [ ] **Step 4 :** `scripts/check.sh` → tout passe.
- [ ] **Step 5 : commit** `feat(protocol): workspace Cargo et messages de contrôle` (cocher la tâche dans `ROADMAP.md`).

### Task 2 : configuration, mode persistant, journaux

**Files :**
- Create : `crates/jarvis-brain/src/config.rs`, `crates/jarvis-brain/src/mode_store.rs`, `crates/jarvis-brain/src/logging.rs`
- Modify : `crates/jarvis-brain/src/lib.rs` (modules publics)

**Interfaces :**
- Consumes : `jarvis_protocol::Mode`.
- Produces :
  - `struct Config { mode: Mode, address: String, local: LocalLlmConfig, online: OnlineLlmConfig, apps: AppsConfig }` (`Deserialize`, `Default`, champs manquants → valeurs par défaut via `#[serde(default)]`).
    - `LocalLlmConfig { url: String = "http://127.0.0.1:11434", model: String = "qwen2.5:3b" }`
    - `OnlineLlmConfig { url: String = "https://api.anthropic.com", model: String = "claude-haiku-5-5", api_key_env: String = "ANTHROPIC_API_KEY" }`
    - `AppsConfig { aliases: HashMap<String, String> }`, par défaut `{"vs code": "code", "vscode": "code"}` ; les alias de l'utilisateur s'ajoutent aux alias par défaut.
    - `address: String = "Monsieur"` (nom par lequel Jarvis s'adresse à l'utilisateur).
  - `fn Config::load(path: &Path) -> anyhow::Result<Config>` : fichier absent → `Config::default()` ; TOML invalide → erreur qui cite le chemin et la ligne.
  - `fn Config::default_path() -> PathBuf` → `~/.config/jarvis/config.toml` (crate `directories`).
  - `struct ModeStore { path: PathBuf, initial: Mode }` ; `fn ModeStore::new(path: PathBuf, initial: Mode) -> Self` ; `fn get(&self) -> Mode` (fichier absent ou illisible → `initial`) ; `fn set(&self, mode: Mode) -> io::Result<()>` (crée les dossiers parents) ; `fn ModeStore::default_path() -> PathBuf` → `~/.local/state/jarvis/mode`.
  - `fn init_logging()` : `tracing-subscriber` avec `EnvFilter`, `info` par défaut, surcharge via `RUST_LOG`.

- [ ] **Step 1 : tests** (`tempfile` pour les chemins) :
  - `config_absente_donne_defauts` : `Config::load` sur un chemin inexistant → `mode == Local`, `local.model == "qwen2.5:3b"`, `online.model == "claude-haiku-5-5"`.
  - `config_partielle` : fichier contenant seulement `mode = "online"` → `mode == Online`, le reste par défaut.
  - `alias_utilisateur_fusionnes` : `[apps.aliases] musique = "spotify"` → contient `musique` **et** `vs code`.
  - `config_invalide_cite_le_chemin` : `mode = 3` → l'erreur contient le chemin du fichier.
  - `mode_persiste` : `set(Online)` puis un **nouveau** `ModeStore` sur le même chemin → `get() == Online`.
  - `mode_fichier_corrompu` : fichier contenant `n'importe quoi` → `get()` renvoie `initial`.
- [ ] **Step 2 :** `cargo test -p jarvis-brain` → échec.
- [ ] **Step 3 :** implémenter. `ModeStore` écrit le mot `local` ou `online` (une ligne).
- [ ] **Step 4 :** `cargo test -p jarvis-brain` → passe.
- [ ] **Step 5 : commit** `feat(brain): configuration TOML et mode persistant`.

### Task 3 : trait `Skill` et exécuteur de commandes

**Files :**
- Create : `crates/jarvis-skills/src/lib.rs`, `crates/jarvis-skills/src/runner.rs`

**Interfaces :**
- Produces :
  - `enum Risk { Direct, Risky }` (phase 1 : tout est `Direct`, la confirmation arrive en phase 3).
  - `struct ToolSpec { name: String, description: String, parameters: serde_json::Value /* schéma JSON objet */, risk: Risk }`
  - `struct Reply { speech: String }` avec `Reply::new(impl Into<String>)`.
  - `enum SkillError { UnknownTool(String), BadArgs(String), Failed(String) }` (thiserror).
  - `#[async_trait] trait Skill: Send + Sync { fn tools(&self) -> Vec<ToolSpec>; async fn call(&self, tool: &str, args: serde_json::Value) -> Result<Reply, SkillError>; }`
  - `trait CommandRunner: Send + Sync { fn run(&self, program: &str, args: &[String]) -> std::io::Result<CommandOutput>; fn spawn_detached(&self, program: &str, args: &[String]) -> std::io::Result<()>; }`, `struct CommandOutput { success: bool, stdout: String }`.
  - `struct SystemRunner` (implémentation réelle avec `std::process::Command` ; `spawn_detached` détache le processus, sans attendre).
  - `struct FakeRunner` (exposée sous `#[cfg(any(test, feature = "test-util"))]`) : enregistre chaque appel dans un `Mutex<Vec<(String, Vec<String>)>>`, renvoie des réponses programmées par nom de programme (`fn respond(&self, program: &str, output: CommandOutput)`), par défaut `success: true, stdout: ""`.

- [ ] **Step 1 : tests** : `fake_runner_enregistre_les_appels` (deux appels → `calls()` dans l'ordre), `fake_runner_reponse_programmee` (`respond("wmctrl", échec)` → `run("wmctrl", …)` renvoie `success == false`), `system_runner_execute_true` (`run("true", &[])` → `success`).
- [ ] **Step 2 :** `cargo test -p jarvis-skills` → échec.
- [ ] **Step 3 :** implémenter.
- [ ] **Step 4 :** `scripts/check.sh` → passe.
- [ ] **Step 5 : commit** `feat(skills): trait Skill et exécuteur de commandes`, puis `git flow feature finish fondations`.

---

## Phase 1 : compréhension et premières compétences (branche `feature/comprehension-texte`)

Démarrage : `git flow feature start comprehension-texte`.

### Task 4 : raccourcis rapides

**Files :**
- Create : `crates/jarvis-brain/src/fastpath.rs`, `crates/jarvis-brain/src/intent.rs`

**Interfaces :**
- Produces :
  - `struct Intent { tool: String, args: serde_json::Value }` (`PartialEq`, `Debug`) : dans `intent.rs`, partagé avec la tâche 7.
  - `fn normalize(text: &str) -> String` : minuscules, accents retirés (é→e, è→e, ê→e, à→a, ç→c, ô→o, î→i, ù→u, û→u), ponctuation → espace, espaces fusionnés, mot de tête `jarvis` retiré.
  - `fn match_fast(text: &str) -> Option<Intent>` (appelle `normalize`).
- Table des raccourcis (phrase normalisée **entière**, sauf `lance/ouvre/demarre`) :

| Phrases | Outil | Arguments |
|---|---|---|
| `suivant`, `suivante`, `chanson suivante`, `musique suivante`, `next` | `media_next` | `{}` |
| `precedent`, `precedente`, `chanson precedente`, `musique precedente` | `media_previous` | `{}` |
| `pause`, `stop`, `mets en pause` | `media_pause` | `{}` |
| `reprends`, `reprend`, `lecture`, `play`, `relance la musique` | `media_play` | `{}` |
| `plus fort`, `monte le son`, `monte le volume` | `volume_up` | `{}` |
| `moins fort`, `baisse le son`, `baisse le volume` | `volume_down` | `{}` |
| `qu est ce qui passe`, `c est quoi cette chanson` | `media_now_playing` | `{}` |
| `passe en mode local`, `mode local` | `set_mode` | `{"mode":"local"}` |
| `passe en mode en ligne`, `mode en ligne`, `passe en ligne` | `set_mode` | `{"mode":"online"}` |
| `qu est ce que tu fais`, `statut` | `status` | `{}` |
| `lance X`, `ouvre X`, `demarre X` (X non vide, sauf `la video …` / `une video …`, laissées à l'IA) | `app_launch` | `{"name": X}` |

- [ ] **Step 1 : tests** (table de cas avec `#[test]` paramétré à la main) :
  - `raccourcis_media` : chaque phrase de la table → l'`Intent` attendu.
  - `normalise_et_tolere_le_bruit` *(Review Focus 1)* : `"Jarvis, Précédent !"`, `"  jarvis   precedent"`, `"PRÉCÉDENT."` → `media_previous`.
  - `lance_extrait_le_nom` : `"Lance VS Code"` → `app_launch {"name":"vs code"}` ; `"ouvre spotify s'il te plait"` → `{"name":"spotify s il te plait"}` (le nettoyage des politesses est laissé à la recherche floue de la tâche 6).
  - `video_laissee_a_l_ia` : `"lance la vidéo de Squeezie"` → `None`.
  - `phrase_libre_non_reconnue` : `"quel temps fait-il demain"` → `None` ; `"lance"` seul → `None`.
- [ ] **Step 2 :** `cargo test -p jarvis-brain fastpath` → échec.
- [ ] **Step 3 :** implémenter avec une correspondance exacte sur la phrase normalisée (`HashMap<&str, Intent>`) plus un test de préfixe pour `lance/ouvre/demarre`.
- [ ] **Step 4 :** test → passe.
- [ ] **Step 5 : commit** `feat(brain): raccourcis rapides sans IA`.

### Task 5 : compétence médias (MPRIS et volume)

**Files :**
- Create : `crates/jarvis-skills/src/media/mod.rs` (la compétence), `crates/jarvis-skills/src/media/mpris.rs` (accès D-Bus réel)

**Interfaces :**
- Consumes : `Skill`, `ToolSpec`, `Reply`, `CommandRunner` (tâche 3).
- Produces :
  - `#[async_trait] trait MediaPlayers: Send + Sync { async fn players(&self) -> anyhow::Result<Vec<PlayerInfo>>; async fn command(&self, bus_name: &str, cmd: PlayerCommand) -> anyhow::Result<()>; }`
  - `struct PlayerInfo { bus_name: String, identity: String, status: PlaybackStatus, title: Option<String>, artist: Option<String> }`, `enum PlaybackStatus { Playing, Paused, Stopped }`, `enum PlayerCommand { Next, Previous, Play, Pause }`.
  - `struct MprisPlayers` : implémentation zbus sur le bus de session (noms `org.mpris.MediaPlayer2.*`, interface `org.mpris.MediaPlayer2.Player`, propriétés `PlaybackStatus`, `Metadata` (`xesam:title`, `xesam:artist`), `Identity` sur `org.mpris.MediaPlayer2`).
  - `struct MediaSkill { players: Arc<dyn MediaPlayers>, runner: Arc<dyn CommandRunner> }`, `fn MediaSkill::new(...)`.
  - Outils : `media_next`, `media_previous`, `media_play`, `media_pause`, `media_now_playing`, `volume_up`, `volume_down` (tous sans paramètre).
- Règles :
  - Lecteur ciblé : le premier `Playing`, sinon le premier `Paused`, sinon le premier de la liste.
  - Volume : `wpctl set-volume @DEFAULT_AUDIO_SINK@ 10%+` (ou `10%-`), avec `-l 1.0` pour ne pas dépasser 100 %.
  - Réponses : next/previous/play/pause → `""` (succès silencieux, un son le remplacera en phase 3) ; volume → `""` ; now_playing → « C'est {titre} de {artiste}. » (ou « C'est {titre}. » sans artiste, ou « Je ne sais pas ce qui passe sur {identity}. » sans métadonnées).

- [ ] **Step 1 : tests** avec un `FakePlayers` local au module de test :
  - `cible_le_lecteur_en_lecture` : VLC `Paused` + Spotify `Playing` → `media_next` envoie `Next` à Spotify.
  - `sinon_le_lecteur_en_pause` : VLC `Stopped` + mpv `Paused` → `media_play` envoie `Play` à mpv.
  - `sans_lecteur_reponse_claire` *(Review Focus 2)* : aucune entrée → `Reply.speech == "Aucun lecteur n'est ouvert."`, aucune commande envoyée.
  - `volume_appelle_wpctl` : `volume_up` → `FakeRunner` a reçu `wpctl ["set-volume","-l","1.0","@DEFAULT_AUDIO_SINK@","10%+"]`.
  - `quoi_qui_passe` : Spotify `Playing`, titre « Hello », artiste « Adele » → « C'est Hello de Adele. ».
  - `outil_inconnu` : `call("media_explode", …)` → `SkillError::UnknownTool`.
- [ ] **Step 2 :** `cargo test -p jarvis-skills media` → échec.
- [ ] **Step 3 :** implémenter `MediaSkill`, puis `MprisPlayers`.
- [ ] **Step 4 :** test → passe. Vérification manuelle : un test `#[ignore] mpris_reel_liste_les_lecteurs` affiche les lecteurs avec Spotify ouvert (`cargo test -p jarvis-skills -- --ignored mpris_reel`).
- [ ] **Step 5 : commit** `feat(skills): contrôle des médias via MPRIS et volume PipeWire`.

### Task 6 : compétence applications

**Files :**
- Create : `crates/jarvis-skills/src/apps/mod.rs` (compétence), `crates/jarvis-skills/src/apps/index.rs` (index des `.desktop`)
- Test fixtures : `crates/jarvis-skills/tests/fixtures/applications/{spotify_spotify.desktop, code.desktop, org.gnome.Nautilus.desktop, hidden.desktop}` (copies minimales : `Name`, `Exec`, `StartupWMClass` quand présent ; `hidden.desktop` a `NoDisplay=true`).

**Interfaces :**
- Consumes : `Skill`, `CommandRunner`, `FakeRunner`.
- Produces :
  - `struct AppEntry { id: String /* nom du fichier sans .desktop */, name: String, path: PathBuf, wm_class: Option<String> }`
  - `struct AppIndex { entries: Vec<AppEntry> }` ; `fn AppIndex::load(dirs: &[PathBuf]) -> AppIndex` (ignore les dossiers absents, `NoDisplay=true`, `Hidden=true`, `Type` ≠ `Application` ; un même `id` vu deux fois : le premier dossier gagne) ; `fn AppIndex::default_dirs() -> Vec<PathBuf>` = `~/.local/share/applications`, `~/.local/share/flatpak/exports/share/applications`, `/var/lib/flatpak/exports/share/applications`, `/var/lib/snapd/desktop/applications`, `/usr/local/share/applications`, `/usr/share/applications` (dans cet ordre).
  - `fn AppIndex::find(&self, query: &str, aliases: &HashMap<String, String>) -> Option<&AppEntry>` : (1) alias exact → recherche de l'`id` cible ; (2) correspondance exacte, insensible à la casse, sur `name` ou `id` ; (3) meilleur score `SkimMatcherV2` sur `name` puis `id`, en ignorant les mots `s il te plait`, `stp`, `l application`, `le jeu` ; score minimal 50, sinon `None`.
  - `struct AppsSkill { index: AppIndex, aliases: HashMap<String,String>, runner: Arc<dyn CommandRunner> }` ; outil `app_launch { name: string }`.
- Comportement de `app_launch` :
  1. introuvable → « Je ne trouve pas l'application {name}. » ;
  2. si `wm_class` connu : `wmctrl -x -a <wm_class>` ; succès → « {Name} est au premier plan. » ;
  3. sinon `spawn_detached("gio", ["launch", <path>])` → « Je lance {Name}. ».

- [ ] **Step 1 : tests** :
  - `index_ignore_les_cachees` : `load([fixtures])` → 3 entrées, pas `hidden`.
  - `alias_vs_code` : `find("vs code", {"vs code":"code"})` → `id == "code"`.
  - `recherche_floue` : `find("spotify s il te plait", {})` → `spotify_spotify` ; `find("nautilus", {})` → `org.gnome.Nautilus`.
  - `appli_introuvable` *(Review Focus 3)* : `call("app_launch", {"name":"zorglub"})` → « Je ne trouve pas l'application zorglub. », `FakeRunner` sans aucun appel.
  - `deja_ouverte_mise_au_premier_plan` : `wmctrl` répond succès → réponse « Spotify est au premier plan. », aucun `gio`.
  - `pas_ouverte_lancee` : `wmctrl` répond échec → `spawn_detached("gio", ["launch", ".../spotify_spotify.desktop"])`, réponse « Je lance Spotify. ».
  - `args_invalides` : `{"nom": "x"}` → `SkillError::BadArgs`.
- [ ] **Step 2 :** `cargo test -p jarvis-skills apps` → échec.
- [ ] **Step 3 :** implémenter (`freedesktop-desktop-entry` pour lire les fichiers, `fuzzy-matcher` pour le score).
- [ ] **Step 4 :** test → passe.
- [ ] **Step 5 : commit** `feat(skills): lancement et mise au premier plan des applications`.

### Task 7 : backends d'IA (Ollama et Claude)

**Files :**
- Create : `crates/jarvis-brain/src/llm/mod.rs` (trait et prompt), `crates/jarvis-brain/src/llm/ollama.rs`, `crates/jarvis-brain/src/llm/claude.rs`

**Interfaces :**
- Consumes : `ToolSpec` (tâche 3), `Intent` (tâche 4), `LocalLlmConfig`, `OnlineLlmConfig` (tâche 2).
- Produces :
  - `enum Understanding { Tool(Intent), Answer(String) }`
  - `enum LlmError { Unreachable(String), BadResponse(String), MissingApiKey(String) }` (thiserror).
  - `#[async_trait] trait LlmBackend: Send + Sync { async fn understand(&self, text: &str, tools: &[ToolSpec]) -> Result<Understanding, LlmError>; }`
  - `fn system_prompt(address: &str) -> String` : « Tu es Jarvis, l'assistant vocal de {address}. Tu réponds en français, en une ou deux phrases, en vouvoyant. Si la demande correspond à un outil, appelle cet outil. Sinon, réponds directement. »
  - `struct OllamaBackend { client: reqwest::Client, url: String, model: String, address: String }` : `POST {url}/api/chat`, corps `{model, stream:false, messages:[{role:"system",…},{role:"user",content:text}], tools:[{type:"function", function:{name, description, parameters}}]}` ; réponse `message.tool_calls[0].function.{name, arguments}` → `Tool`, sinon `message.content` → `Answer`. Connexion refusée ou délai > 20 s → `Unreachable`.
  - `struct ClaudeBackend { client, url, model, api_key_env, address }` : `POST {url}/v1/messages`, en-têtes `x-api-key`, `anthropic-version: 2023-06-01`, `content-type: application/json` ; corps `{model, max_tokens: 300, system, messages:[{role:"user",content:text}], tools:[{name, description, input_schema: parameters}]}` ; premier bloc `type == "tool_use"` → `Tool(name, input)`, sinon concaténation des blocs `text` → `Answer`. Variable d'environnement absente → `MissingApiKey` **sans** requête. Délai > 15 s ou erreur réseau → `Unreachable`.

- [ ] **Step 1 : tests** avec `wiremock` (un `MockServer` par test, `url` pointé dessus) :
  - `ollama_appel_outil` : réponse `{"message":{"role":"assistant","content":"","tool_calls":[{"function":{"name":"app_launch","arguments":{"name":"firefox"}}}]}}` → `Tool(Intent{tool:"app_launch", args:{"name":"firefox"}})` ; le corps reçu contient `"stream":false` et l'outil.
  - `ollama_reponse_texte` : `content: "Il est midi."`, pas d'outil → `Answer("Il est midi.")`.
  - `ollama_injoignable_erreur` : `url = "http://127.0.0.1:1"` → `Unreachable`.
  - `claude_appel_outil` : réponse `{"content":[{"type":"tool_use","id":"t1","name":"media_next","input":{}}],"stop_reason":"tool_use"}` → `Tool(media_next)` ; requête reçue avec `x-api-key: test-key` et `anthropic-version: 2023-06-01`.
  - `claude_texte` : `[{"type":"text","text":"Bonjour."}]` → `Answer("Bonjour.")`.
  - `claude_sans_cle_ne_contacte_rien` : variable absente → `MissingApiKey`, le `MockServer` n'a reçu **aucune** requête.
  - `claude_500` : HTTP 500 → `Unreachable`.
- [ ] **Step 2 :** `cargo test -p jarvis-brain llm` → échec.
- [ ] **Step 3 :** implémenter (reqwest avec `json` et `rustls`, sans OpenSSL).
- [ ] **Step 4 :** test → passe.
- [ ] **Step 5 : commit** `feat(brain): compréhension par Ollama et par Claude`.

### Task 8 : orchestrateur `Brain` et compétence système

**Files :**
- Create : `crates/jarvis-brain/src/brain.rs`, `crates/jarvis-brain/src/system_skill.rs`

**Interfaces :**
- Consumes : tout ce qui précède.
- Produces :
  - `struct Brain { skills: Vec<Arc<dyn Skill>>, local: Arc<dyn LlmBackend>, online: Arc<dyn LlmBackend>, modes: Arc<ModeStore> }`
  - `fn Brain::new(skills: Vec<Arc<dyn Skill>>, local: Arc<dyn LlmBackend>, online: Arc<dyn LlmBackend>, modes: Arc<ModeStore>) -> Brain` (ajoute `SystemSkill` lui-même).
  - `async fn Brain::handle_text(&self, text: &str) -> Reply` (ne renvoie jamais d'erreur : tout échec devient une phrase).
  - `fn Brain::mode(&self) -> Mode`, `fn Brain::set_mode(&self, mode: Mode) -> io::Result<()>`.
  - `SystemSkill` : outils `set_mode { mode: "local" | "online" }` → « Mode local activé. » / « Mode en ligne activé. » ; `status` → « Je suis en mode {mode}. Tout va bien. ».
- Déroulé de `handle_text` : log `info!(phrase = text)` → `match_fast` → sinon backend du mode **actuel** (`modes.get()` à chaque phrase) avec la liste de tous les outils → si `Tool`, trouver la compétence qui déclare cet outil et l'appeler ; si `Answer`, la renvoyer.
- Phrases d'échec (exactes) :
  - outil inconnu ou `SkillError::UnknownTool`/`BadArgs` → « Je ne sais pas faire ça. »
  - `SkillError::Failed` → « Ça n'a pas marché : {détail}. »
  - local `Unreachable` → « L'IA locale ne répond pas. Ollama est-il lancé ? »
  - en ligne `Unreachable` → « Je n'arrive pas à joindre l'IA en ligne. Dites « passe en mode local » pour continuer sans elle. »
  - `MissingApiKey(var)` → « La clé de l'IA en ligne est absente : définissez {var}. »
  - `BadResponse` → « Je n'ai pas compris la réponse de l'IA. »

- [ ] **Step 1 : tests** avec un `FakeLlm` (réponse programmée + compteur d'appels) et une `FakeSkill` :
  - `raccourci_sans_ia` : « suivant » → `FakeSkill` reçoit `media_next`, les **deux** `FakeLlm` ont 0 appel.
  - `mode_local_n_appelle_que_le_local` : mode `Local`, « quel temps fait-il » → local 1 appel, en ligne **0**.
  - `mode_en_ligne` : après `set_mode(Online)`, même phrase → en ligne 1 appel, local 0.
  - `switch_vocal_persiste` : « passe en mode en ligne » → « Mode en ligne activé. » et un `ModeStore` neuf sur le même fichier lit `Online`.
  - `outil_inconnu_reponse_polie` *(Review Focus 4)* : le `FakeLlm` renvoie `Tool("lance_fusee", {})` → « Je ne sais pas faire ça. ».
  - `ollama_injoignable` *(Review Focus 5)* : local renvoie `Unreachable` → phrase exacte ci-dessus, et le mode reste `Local`.
  - `en_ligne_injoignable_ne_bascule_pas` : en ligne `Unreachable` → phrase exacte, `mode()` reste `Online`, local 0 appel.
- [ ] **Step 2 :** `cargo test -p jarvis-brain brain` → échec.
- [ ] **Step 3 :** implémenter.
- [ ] **Step 4 :** test → passe.
- [ ] **Step 5 : commit** `feat(brain): orchestrateur et changement de mode`.

### Task 9 : service du cerveau et outil `jarvis`

**Files :**
- Create : `crates/jarvis-brain/src/server.rs`, `crates/jarvis-brain/src/main.rs`
- Create : `crates/jarvis-cli/src/main.rs`

**Interfaces :**
- Consumes : `Brain`, `Config`, `ModeStore`, `socket_path`, `ControlRequest`/`ControlResponse`.
- Produces :
  - `async fn serve(brain: Arc<Brain>, socket: &Path) -> anyhow::Result<()>` : supprime un socket orphelin, crée le dossier parent en `0700`, accepte plusieurs clients, une ligne JSON → une ligne JSON. Ligne illisible → `Error { message: "requête invalide" }`, la connexion reste ouverte.
  - `jarvis-brain` (`main.rs`) : `init_logging`, `Config::load(default_path)`, `ModeStore::new(default_path, config.mode)`, construit `MediaSkill(MprisPlayers, SystemRunner)`, `AppsSkill(AppIndex::load(default_dirs()), config.apps.aliases, SystemRunner)`, `OllamaBackend`, `ClaudeBackend`, puis `serve`. Arrêt propre sur `SIGINT`/`SIGTERM` (socket supprimé).
  - `jarvis` (clap) : `jarvis say <texte…>`, `jarvis status`, `jarvis mode <local|online>`. Affiche `speech` (ou rien si vide, code 0), ou `message` sur stderr avec code 1. Socket absent ou connexion refusée → « Le cerveau de Jarvis n'est pas lancé (jarvis-brain). » sur stderr, code 2.

- [ ] **Step 1 : tests** (`crates/jarvis-brain/tests/server.rs`, socket dans un `tempdir`, `Brain` construit avec des faux) :
  - `say_renvoie_la_reponse` : envoi `{"type":"say","text":"suivant"}` → `{"type":"reply","speech":…}`.
  - `status_et_set_mode` : `set_mode online` puis `status` → `mode == online`.
  - `ligne_invalide_n_interrompt_pas` : `pas du json` → `Error`, puis un `status` sur la **même** connexion répond.
  - `socket_orphelin_remplace` : un fichier existe déjà au chemin → `serve` démarre quand même.
  - CLI (`crates/jarvis-cli/tests/cli.rs`, via `std::process::Command` sur le binaire) : `cerveau_absent_message_clair` avec `XDG_RUNTIME_DIR` pointé sur un dossier vide → code 2 et le message exact.
- [ ] **Step 2 :** `cargo test -p jarvis-brain --test server` et `cargo test -p jarvis-cli` → échec.
- [ ] **Step 3 :** implémenter.
- [ ] **Step 4 :** `scripts/check.sh` → passe.
- [ ] **Step 5 : commit** `feat: service jarvis-brain et outil jarvis`.

### Task 10 : jeu de phrases de référence, IA locale, essai réel

**Files :**
- Create : `crates/jarvis-brain/tests/fixtures/phrases.toml` (au moins 30 entrées `[[phrase]] text = "…" tool = "…" args = { … }`, dont 10 qui **ne** passent **pas** par les raccourcis, par exemple « mets un peu plus de son », « ouvre-moi le navigateur », « je veux écouter autre chose »).
- Create : `crates/jarvis-brain/tests/phrases.rs`
- Create : `docs/ia-locale.md` (installation d'Ollama, modèle choisi et mesures).

**Interfaces :**
- Consumes : `match_fast`, `OllamaBackend`, `ClaudeBackend`, les `tools()` réels de `MediaSkill`, `AppsSkill` et `SystemSkill`.

- [ ] **Step 1 : tests** :
  - `raccourcis_couvrent_leurs_phrases` (toujours lancé) : chaque phrase marquée `fast = true` donne l'`Intent` attendu via `match_fast`.
  - `ollama_jeu_de_phrases` et `claude_jeu_de_phrases` (`#[ignore]`, réseau réel) : pour chaque phrase, outil obtenu == outil attendu ; affiche le taux de réussite et le temps médian ; échoue sous 80 % de réussite.
- [ ] **Step 2 :** `cargo test -p jarvis-brain --test phrases` → `raccourcis_couvrent_leurs_phrases` passe (les tâches 4 à 8 sont faites) ; lancer les deux tests ignorés : ils échouent tant qu'Ollama n'est pas installé.
- [ ] **Step 3 : installer l'IA locale** : `curl -fsSL https://ollama.com/install.sh | sh`, puis `ollama pull qwen2.5:3b` et `ollama pull llama3.2:3b`. Lancer `ollama_jeu_de_phrases` avec chacun (`JARVIS_TEST_MODEL=<modèle>`). Garder le meilleur (réussite, puis temps médian) comme défaut dans `LocalLlmConfig` et noter les chiffres dans `docs/ia-locale.md`.
- [ ] **Step 4 : essai réel** : `cargo run -p jarvis-brain` dans un terminal ; dans un autre, avec Spotify en lecture :
  - `cargo run -p jarvis-cli -- say "suivant"` → la chanson change en moins de 1 s ;
  - `… say "lance VS Code"` → VS Code s'ouvre ou passe au premier plan ;
  - `… say "mets un peu plus de son"` en mode local → le volume monte ;
  - `… mode online`, puis `… say "c'est quoi la capitale du Pérou"` → réponse texte ; avec `ANTHROPIC_API_KEY` absente → la phrase « clé absente ».
  Noter les résultats dans `docs/ia-locale.md`.
- [ ] **Step 5 : commit** `test: jeu de phrases de référence et choix du modèle local`, `scripts/check.sh`, puis `git flow feature finish comprehension-texte`.
