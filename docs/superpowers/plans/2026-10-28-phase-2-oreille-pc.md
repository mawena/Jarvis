# Jarvis, phase 2 (oreille du PC, écoute permanente) : plan d'implémentation

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal :** Charles dit « Jarvis, suivant » près du PC et la musique change ; une icône dans la zone de notification montre le statut et permet de couper / réactiver l'écoute et de changer de mode.

**Architecture :** une nouvelle crate `jarvis-ear-core` (logique pure : rééchantillonnage, machine d'états d'écoute, détecteur de mot d'éveil, détection de parole) et un binaire `jarvis-ear` (capture micro `cpal`, icône `ksni`, client du socket du cerveau). L'oreille ne fait **pas** la reconnaissance vocale : après « Jarvis », elle envoie le son de la phrase au cerveau (`ControlRequest::Listen`), qui transcrit avec Whisper puis appelle `Brain::handle_text`. C'est le découpage de la spec (§5.3) ; en phase 4 le même message passera sur WebSocket au lieu du socket Unix.

**Tech Stack :** cpal 0.18.2, rustpotter 3.0.2, earshot 1.2.2, whisper-rs 0.16.0 (whisper.cpp 1.8.3), ksni 0.3.7 (feature tokio), base64 0.22. Existant : tokio, serde, zbus, tracing, clap.

**Spec :** [docs/superpowers/specs/2026-10-28-jarvis-design.md](../specs/2026-10-28-jarvis-design.md) (§5.3, §5.7, §5.7 bis)
**Recherche :** versions et API vérifiées le 2026-10-28 (cpal 0.18 prend la config par valeur ; rustpotter : noms exacts `process_*` à vérifier sur docs.rs ; whisper-rs : `WhisperContext::new_with_params`, `create_state`, `FullParams::new(SamplingStrategy::Greedy { best_of: 1 })`, `set_language(Some("fr"))`, `state.full(params, &audio)` ; ksni : `TrayMethods::spawn`, `handle.update`).

## Global Constraints

- **Prérequis système (Charles, une fois) :** `sudo apt install libasound2-dev libclang-dev`. Sans eux, `cpal` et `whisper-rs` ne compilent pas.
- Micro : capter au format par défaut du périphérique (ici 48 kHz, 2 canaux, s32), puis mono + 16 kHz dans `jarvis-ear-core`. Ne jamais allouer ni bloquer dans le callback `cpal` : `try_send` vers un canal borné, trames perdues comptées et journalisées.
- Aucun son n'est enregistré sur disque, et aucun son ne quitte l'oreille avant le mot d'éveil. Après le mot d'éveil, seul le son de la phrase part, vers le cerveau local.
- Reconnaissance vocale **locale dans les deux modes** (Whisper), langue `fr`, exécutée dans `tokio::task::spawn_blocking`, 6 threads.
- Modèles dans `~/.local/share/jarvis/models/` : `ggml-small-q5_1.bin` (défaut) et `ggml-base-q5_1.bin` (rapide). Mot d'éveil : `~/.local/share/jarvis/wakeword/jarvis.rpw`.
- Fin de phrase : 800 ms de silence après de la parole ; aucune parole 4 s après « Jarvis » → abandon ; durée max 10 s ; pré-enregistrement de 300 ms avant le début de parole.
- Trois états de micro : **ouvert** ; **écoute coupée** (le mot d'éveil reste détecté ; la phrase suivante est envoyée marquée `muted`, et le cerveau n'en retient que « réactive l'écoute ») ; **coupure totale** (flux `cpal` fermé, seul le menu de l'icône le rouvre).
- Le texte entendu est journalisé (`info`), jamais le son. Les phrases entendues pendant l'écoute coupée ne sont **pas** journalisées, sauf la réactivation.
- Les règles des phases 0-1 restent valables (git flow, commits au nom de Charles sans attribution, `ROADMAP.md` coché dès qu'une tâche est validée, `scripts/check.sh` vert).

## Review Focus

1. **Pas de micro / micro débranché** pendant l'écoute : l'oreille ne plante pas ; l'icône passe en « Micro indisponible » et réessaie toutes les 5 s. → test `capture_absente_etat_indisponible` (tâche 7, sur la logique de reconnexion avec un faux périphérique).
2. **Cerveau arrêté** : l'oreille continue d'écouter, l'icône passe en « Cerveau injoignable », et après « Jarvis » un message est journalisé sans planter. → test `cerveau_absent_icone_injoignable` (tâche 7).
3. **« Jarvis » suivi de rien** : abandon après 4 s, volume rétabli. → tests `reveil_sans_parole_abandon` (tâche 2) et `listen_vide_retablit_volume` (tâche 5).
4. **Phrase interminable (TV, musique)** : coupée à 10 s, envoyée quand même. → test `phrase_coupee_a_dix_secondes` (tâche 2).
5. **Écoute coupée** : toute phrase autre que la réactivation est ignorée sans action ni journalisation. → test `ecoute_coupee_ignore_le_reste` (tâche 5).

---

Démarrage : `git flow feature start oreille-pc` depuis `develop`.

### Task 1 : socle commun (protocole, crates, traits)

**Files :**
- Modify : `Cargo.toml` (workspace : `crates/jarvis-ear-core`, `crates/jarvis-ear` ; deps cpal 0.18, rustpotter 3.0, earshot 1.2, whisper-rs 0.16, ksni 0.3 avec `tokio`, base64 0.22)
- Modify : `crates/jarvis-protocol/src/lib.rs`
- Create : `crates/jarvis-ear-core/src/lib.rs`, `crates/jarvis-ear/src/main.rs` (squelette), `crates/jarvis-brain/src/stt/mod.rs`
- Modify : `crates/jarvis-brain/src/testing.rs`

**Interfaces :**
- Produces (protocole, rétrocompatible avec le CLI existant) :
  - `ControlRequest::Wake` (« Jarvis » entendu → le cerveau baisse le son).
  - `ControlRequest::Listen { audio: String, muted: bool }` : `audio` = échantillons i16 little-endian, 16 kHz mono, en base64 standard ; vide si rien n'a été dit. Rétablit toujours le son.
  - `ControlResponse::Heard { text: String, speech: String, ear_action: Option<EarAction> }`, `enum EarAction { Mute, Unmute }` (serde `snake_case`).
  - `ControlResponse::Status` gagne `local_model: String` (`#[serde(default)]`).
  - `fn encode_pcm(samples: &[i16]) -> String`, `fn decode_pcm(b64: &str) -> Result<Vec<i16>, ProtocolError>`.
- Produces (`jarvis_ear_core`) :
  - `pub const SAMPLE_RATE: u32 = 16_000;`
  - `trait WakeDetector: Send { fn frame_len(&self) -> usize; fn process(&mut self, frame: &[f32]) -> Option<f32>; }` (renvoie le score si « Jarvis » est détecté).
  - `trait Vad: Send { fn frame_len(&self) -> usize /* 256 */; fn is_speech(&mut self, frame: &[f32]) -> bool; }`
  - `enum MicState { Open, Muted, HardMuted, Unavailable }`.
- Produces (`jarvis_brain::stt`) : `#[async_trait] trait Transcriber: Send + Sync { async fn transcribe(&self, audio: &[f32]) -> anyhow::Result<String>; }` ; `testing::FakeTranscriber` (texte programmé, compte les appels).

- [ ] **Step 1 : tests** : `wake_serialise` (`{"type":"wake"}`), `listen_aller_retour`, `heard_avec_action` (`ear_action: "mute"`), `status_sans_local_model_reste_lisible` (ancien JSON sans le champ → `local_model == ""`), `pcm_aller_retour` (`[0, 1, -1, i16::MAX, i16::MIN]`), `pcm_base64_invalide`.
- [ ] **Step 2 :** `cargo test -p jarvis-protocol` → échec.
- [ ] **Step 3 :** implémenter ; squelettes qui compilent (le binaire `jarvis-ear` affiche son usage).
- [ ] **Step 4 :** `scripts/check.sh` → vert (y compris la compilation de whisper-rs et cpal).
- [ ] **Step 5 : commit** `feat: protocole d'écoute et socle de l'oreille`.

### Task 2 : rééchantillonnage et machine d'états d'écoute (`jarvis-ear-core`)

**Files :** Create `crates/jarvis-ear-core/src/resample.rs`, `crates/jarvis-ear-core/src/listener.rs`

**Interfaces :**
- Consumes : `WakeDetector`, `Vad`, `SAMPLE_RATE` (tâche 1).
- Produces :
  - `struct Downmixer` ; `fn Downmixer::new(input_rate: u32, channels: u16) -> Self` ; `fn push(&mut self, interleaved: &[f32]) -> Vec<f32>` (mono 16 kHz). 48 kHz → filtre passe-bas puis décimation par 3 ; autre fréquence → interpolation linéaire. Gère les morceaux de taille quelconque (état conservé entre appels).
  - `fn i32_to_f32(s: i32) -> f32`, `fn i16_to_f32(s: i16) -> f32`, `fn f32_to_i16(s: f32) -> i16` (saturation).
  - `struct Listener<W: WakeDetector, V: Vad>` ; `fn new(wake: W, vad: V) -> Self` ; `fn push(&mut self, mono16k: &[f32]) -> Vec<ListenEvent>`.
  - `enum ListenEvent { Wake { score: f32 }, Utterance(Vec<f32>), NoSpeech }`.
- Règles : en attente, seul le détecteur tourne ; après `Wake`, accumulation avec 300 ms de pré-enregistrement à partir du début de parole ; fin après 800 ms de silence suivant de la parole → `Utterance` ; pas de parole en 4 s → `NoSpeech` ; 10 s atteintes → `Utterance` immédiate. Pendant la capture d'une phrase, le détecteur de mot d'éveil n'est pas consulté.

- [ ] **Step 1 : tests** (faux détecteur déclenché à une trame donnée, faux VAD piloté par une table de trames parlées/silencieuses) : `downmix_48k_stereo_vers_16k` (1 s de 48k stéréo → 16 000 échantillons ± 1, un sinus de 440 Hz garde son amplitude à ± 10 %), `downmix_morceaux_irreguliers` (même sortie que d'un seul bloc), `conversion_saturee`, `reveil_puis_phrase` (Wake puis Utterance de la bonne durée, pré-enregistrement inclus), `reveil_sans_parole_abandon` *(RF 3)*, `phrase_coupee_a_dix_secondes` *(RF 4)*, `pas_de_reveil_pendant_la_phrase`.
- [ ] **Step 2 → 4 :** échec, implémentation, `cargo test -p jarvis-ear-core` vert.
- [ ] **Step 5 : commit** `feat(ear): rééchantillonnage et machine d'états d'écoute`.

### Task 3 : reconnaissance vocale Whisper (cerveau)

**Files :** Create `crates/jarvis-brain/src/stt/whisper.rs`, `scripts/download-models.sh` ; Modify `crates/jarvis-brain/src/config.rs`, `docs/ia-locale.md` (section Whisper)

**Interfaces :**
- Consumes : `Transcriber` (tâche 1).
- Produces :
  - `SttConfig { model_path: PathBuf = "~/.local/share/jarvis/models/ggml-small-q5_1.bin" (résolu via directories), threads: i32 = 6 }`, champ `Config.stt`.
  - `struct WhisperTranscriber` ; `fn WhisperTranscriber::load(cfg: &SttConfig) -> anyhow::Result<Self>` (erreur claire citant le chemin et `scripts/download-models.sh` si le fichier manque) ; `Transcriber` via `spawn_blocking`, `Greedy { best_of: 1 }`, langue `fr`, sans traduction, sans horodatage, `set_no_context(true)`, `set_single_segment(true)`, `set_initial_prompt("Jarvis.")` ; segments concaténés et `trim()`.
  - `scripts/download-models.sh [small|base|all]` : `curl -L --fail` depuis `https://huggingface.co/ggerganov/whisper.cpp/resolve/main/<fichier>` vers `~/.local/share/jarvis/models/`, vérifie la taille (small-q5_1 : 190 085 487 octets ; base-q5_1 : 59 707 625), ne retélécharge pas un fichier complet.
- [ ] **Step 1 : tests** : `stt_config_defauts`, `modele_absent_message_clair` ; `#[ignore] whisper_transcrit_un_wav` : lit `JARVIS_TEST_WAV` (16 kHz mono) et affiche texte + durée.
- [ ] **Step 2 → 4 :** échec, implémentation, vert ; lancer le script (`small` et `base`), enregistrer « Jarvis, mets la musique suivante » avec `arecord -f S16_LE -r 16000 -c 1 -d 4 /tmp/jv-test.wav` **seulement si Charles le demande** ; sinon générer un WAV de test avec `espeak-ng -v fr` s'il est installé, puis mesurer base et small (durée de transcription) dans `docs/ia-locale.md`.
- [ ] **Step 5 : commit** `feat(brain): reconnaissance vocale Whisper locale`.

### Task 4 : icône de la zone de notification (`jarvis-ear`)

**Files :** Create `crates/jarvis-ear/src/tray.rs`

**Interfaces :**
- Consumes : `MicState`, `jarvis_protocol::Mode`.
- Produces :
  - `struct TrayState { mic: MicState, capturing: bool, brain: BrainLink /* Connected { mode: Mode, local_model: String } | Unreachable */, last_heard: Option<String> }`.
  - `enum TrayCommand { Mute, Unmute, HardMute, Reopen, SetMode(Mode), Quit }` envoyés par un `tokio::sync::mpsc::UnboundedSender<TrayCommand>` stocké dans `JarvisTray`.
  - `struct JarvisTray { state: TrayState, commands: UnboundedSender<TrayCommand> }` implémentant `ksni::Tray` : `id() = "jarvis"`, `title()`, `icon_name()`, `tool_tip()`, `status()`, `menu()`.
  - Fonctions pures testables : `fn icon_for(state: &TrayState) -> &'static str`, `fn summary(state: &TrayState) -> String`, `fn menu_labels(state: &TrayState) -> Vec<String>`.
- Correspondances (exactes) :
  - icônes : ouvert → `audio-input-microphone` ; capture en cours → `media-record` ; écoute coupée → `microphone-sensitivity-muted` ; coupure totale → `microphone-disabled-symbolic` ; micro indisponible → `dialog-warning` ; cerveau injoignable → `network-offline` (prioritaire sauf coupure totale) ; `status()` = `NeedsAttention` si cerveau injoignable ou micro indisponible, sinon `Active`.
  - résumé : « Jarvis — à l'écoute · mode local (llama3.2:3b) », « Jarvis — écoute coupée », « Jarvis — micro coupé », « Jarvis — micro indisponible », « Jarvis — cerveau injoignable ».
  - menu : ligne de statut (désactivée) ; « Couper l'écoute » ou « Réactiver l'écoute » ; « Coupure totale du micro » ou « Rouvrir le micro » ; groupe radio « Mode local » / « Mode en ligne » (désactivé si cerveau injoignable) ; « Dernière phrase : … » (désactivée, si présente) ; séparateur ; « Quitter l'oreille ».
- [ ] **Step 1 : tests** : `icone_selon_etat` (les 6 cas), `resume_mode_local`, `cerveau_injoignable_prioritaire`, `menu_ecoute_coupee` (contient « Réactiver l'écoute »), `menu_mode_desactive_sans_cerveau`, `action_menu_envoie_commande` (activer l'entrée « Couper l'écoute » → `TrayCommand::Mute` reçu).
- [ ] **Step 2 → 4 :** échec, implémentation, vert.
- [ ] **Step 5 : commit** `feat(ear): icône de notification avec statut et menu`.

### Task 5 : écoute côté cerveau (Wake, Listen, commandes d'écoute, baisse du son)

**Files :** Create `crates/jarvis-brain/src/ducking.rs`, `crates/jarvis-brain/src/listen.rs` ; Modify `crates/jarvis-brain/src/server.rs`, `crates/jarvis-brain/src/fastpath.rs`, `crates/jarvis-brain/src/main.rs`

**Interfaces :**
- Consumes : `Transcriber`, `FakeTranscriber`, `Brain::handle_text`, protocole (tâche 1).
- Produces :
  - `fn fastpath::match_ear_command(text: &str) -> Option<EarAction>` : Mute ← « coupe l'écoute », « coupe le micro », « arrête d'écouter », « mute » ; Unmute ← « réactive l'écoute », « réactive le micro », « reprends l'écoute », « écoute à nouveau » (après `normalize`).
  - `#[async_trait] trait Ducker: Send + Sync { async fn duck(&self); async fn restore(&self); }` ; `WpctlDucker` : lit `wpctl get-volume @DEFAULT_AUDIO_SINK@`, passe à 30 % de la valeur, `restore` remet la valeur lue (sans effet si rien n'a été baissé ; deux `duck` successifs ne perdent pas la valeur d'origine) ; `testing::FakeDucker`.
  - `struct ListenService { brain: Arc<Brain>, stt: Arc<dyn Transcriber>, ducker: Arc<dyn Ducker> }` ; `async fn wake(&self)` ; `async fn listen(&self, audio: &[i16], muted: bool) -> ControlResponse`.
  - Règles de `listen` : toujours `restore` d'abord ; audio vide → `Heard { text: "", speech: "", ear_action: None }` ; transcription vide → idem ; `muted` → seule une commande Unmute est retenue (`speech` « Écoute réactivée. », `Some(Unmute)`), tout le reste → réponse vide, sans journaliser le texte ; non muet + Mute → « Écoute coupée. », `Some(Mute)` ; sinon `speech = brain.handle_text(text)`. Échec de transcription → `speech` « Je n'ai pas compris. ».
  - `serve` reçoit un `Arc<ListenService>` en plus ; `Status` renseigne `local_model`.
  - `main.rs` : branche `ListenService` avec un `NoTranscriber` provisoire (répond « La reconnaissance vocale n'est pas installée. ») et `WpctlDucker` ; la tâche 7 remplace `NoTranscriber` par `WhisperTranscriber` (créé en parallèle par la tâche 3).
- [ ] **Step 1 : tests** : `commandes_ecoute_reconnues`, `listen_transcrit_puis_agit` (FakeTranscriber « suivant » → FakeSkill reçoit `media_next`), `listen_vide_retablit_volume` *(RF 3)*, `wake_baisse_le_son`, `ecoute_coupee_ignore_le_reste` *(RF 5)*, `ecoute_coupee_accepte_reactivation`, `coupe_l_ecoute_renvoie_mute`, `ducker_ne_perd_pas_le_volume` (deux `duck` puis `restore` → valeur d'origine, avec `FakeRunner`), `server_listen_et_status_local_model` (test d'intégration socket).
- [ ] **Step 2 → 4 :** échec, implémentation, vert.
- [ ] **Step 5 : commit** `feat(brain): écoute, commandes d'écoute et baisse du son`.

### Task 6 : mot d'éveil et détection de parole réels

**Files :** Create `crates/jarvis-ear-core/src/rustpotter.rs`, `crates/jarvis-ear-core/src/earshot.rs`, `crates/jarvis-ear/src/enroll.rs`, `docs/oreille.md`

**Interfaces :**
- Consumes : `WakeDetector`, `Vad` (tâche 1).
- Produces :
  - `struct RustpotterDetector` ; `fn load(path: &Path, threshold: f32) -> anyhow::Result<Self>` (seuil par défaut 0.5, configurable) ; implémente `WakeDetector` sur des trames f32 16 kHz (vérifier sur docs.rs/rustpotter 3.0.2 les noms exacts de `process_*` et le format attendu).
  - `struct EarshotVad` (earshot `Detector::default()`, seuil 0.5, trames de 256) implémentant `Vad`.
  - `jarvis-ear enroll [--count 8]` : enregistre `count` fois « Jarvis » (invite dans le terminal, 1,5 s par prise, via `cpal` + `Downmixer`), écrit les WAV dans `~/.local/share/jarvis/wakeword/samples/`, puis construit `jarvis.rpw` avec l'API de rustpotter (référence à partir des échantillons). Si l'API ne le permet pas, documenter dans `docs/oreille.md` la voie `cargo install rustpotter-cli` + `rustpotter-cli record` / `build`, et `enroll` se limite à enregistrer les WAV.
  - `docs/oreille.md` : prérequis apt, téléchargement des modèles, enrôlement de la voix, lancement de l'oreille, réglage du seuil.
- [ ] **Step 1 : tests** : `earshot_silence_n_est_pas_parole` (zéros), `earshot_trame_de_256`, `rustpotter_fichier_absent_message_clair` ; `#[ignore] rustpotter_detecte_les_echantillons` (rejoue les WAV enregistrés : chacun déclenche).
- [ ] **Step 2 → 4 :** échec, implémentation, vert.
- [ ] **Step 5 : commit** `feat(ear): mot d'éveil rustpotter, détection de parole et enrôlement`.

### Task 7 : binaire `jarvis-ear` (assemblage)

**Files :** Create `crates/jarvis-ear/src/capture.rs`, `crates/jarvis-ear/src/client.rs`, `crates/jarvis-ear/src/app.rs` ; Modify `crates/jarvis-ear/src/main.rs`

**Interfaces :**
- Consumes : tout ce qui précède.
- Produces :
  - `capture` : thread dédié qui possède le `cpal::Stream` (pas `Send`) ; `fn start(tx: mpsc::Sender<Vec<f32>>) -> anyhow::Result<CaptureHandle>` ; `CaptureHandle::stop()` (ferme le flux = coupure totale) ; périphérique par défaut, ou `ear.toml` `device = "…"` ; perte du périphérique → événement `Unavailable`, nouvel essai toutes les 5 s.
  - `client` : `async fn request(socket: &Path, req: &ControlRequest) -> Result<ControlResponse, ClientError>` (une connexion par requête, délai 30 s) ; `ClientError::BrainUnreachable` si le socket est absent ou refuse.
  - `app` : boucle principale : trames → `Downmixer` → `Listener` ; `Wake` → `Request::Wake` (sans attendre la réponse pour ne pas bloquer l'écoute) + `capturing = true` ; `Utterance` → `Listen{audio, muted}` ; `NoSpeech` → `Listen{"", muted}` ; `Heard.ear_action` appliqué à l'état ; commandes de l'icône appliquées ; `Status` interrogé toutes les 5 s pour l'icône (mode, modèle, joignable).
  - Configuration `~/.config/jarvis/ear.toml` : `wakeword_path`, `threshold`, `device` (tout optionnel).
  - `jarvis-ear` sans argument = `run` ; `jarvis-ear enroll`.
  - Côté cerveau (`crates/jarvis-brain/src/main.rs`) : charger `WhisperTranscriber` au démarrage ; si le modèle manque, garder `NoTranscriber` et journaliser une fois le chemin attendu et `scripts/download-models.sh`.
- [ ] **Step 1 : tests** (sur `app` avec faux capteur / faux client) : `cerveau_absent_icone_injoignable` *(RF 2)*, `capture_absente_etat_indisponible` *(RF 1)*, `action_mute_appliquee`, `coupure_totale_arrete_la_capture`, `reveil_envoie_wake_puis_listen`.
- [ ] **Step 2 → 4 :** échec, implémentation, `scripts/check.sh` vert.
- [ ] **Step 5 : commit** `feat(ear): oreille du PC avec icône de notification`.

### Task 8 : essai réel et réglages (avec Charles)

- [ ] `scripts/download-models.sh all` ; `jarvis-ear enroll` (Charles enregistre sa voix).
- [ ] Lancer `jarvis-brain` puis `jarvis-ear` ; vérifier : icône visible, « Jarvis, suivant » change la musique (mesurer le délai entre la fin de la phrase et le changement), « Jarvis, coupe l'écoute » puis « Jarvis, réactive l'écoute », coupure totale et réouverture depuis le menu, changement de mode depuis le menu.
- [ ] Choisir `small` ou `base` selon la latence mesurée (objectif spec : < 1 s pour « suivant » ; noter l'écart réel).
- [ ] Journée de mesure des fausses détections (objectif < 1 / jour) ; ajuster `threshold`.
- [ ] Fin : `scripts/check.sh`, `git flow feature finish oreille-pc`.

## Vagues d'exécution (agents en parallèle)

- Vague 1 : tâche 1 (socle partagé, seule).
- Vague 2 (parallèle, worktrees) : tâches 2, 3, 4, 5, 6 — fichiers disjoints, interfaces fixées par la tâche 1.
- Vague 3 : tâche 7, puis tâche 8 avec Charles.
