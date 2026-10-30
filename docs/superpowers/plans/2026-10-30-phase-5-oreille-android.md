# Jarvis, phase 5 (oreille Android, Pixel 4 XL) : plan d'implémentation

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal :** le Pixel 4 XL devient une oreille de Jarvis à part entière : une petite appli Kotlin écoute « Jarvis » en continu, écran éteint, dans un service au premier plan de type micro ; elle s'appaire en scannant le QR code de `jarvis pair` ; le mot d'éveil est enregistré sur le téléphone ; la voix de Jarvis est jouée sur le téléphone ; une notification permanente montre l'état (à l'écoute, écoute coupée, micro coupé, PC injoignable, mode local / en ligne) avec « Couper / Réactiver l'écoute » et « Coupure totale » ; l'écoute repart seule au démarrage du téléphone.

**Architecture :** toute la logique reste en Rust. La **machine d'états de l'oreille** (réveil → élection → capture en flux → réponse, annonces, question en attente, « le PC est endormi », écoute coupée, coupure totale), aujourd'hui dans `crates/jarvis-ear/src/app.rs`, **descend dans `jarvis-ear-core`** (module `session`), derrière trois abstractions déjà présentes (`Capture`, `Player`, `Link`) : l'oreille du PC et celle d'Android exécutent exactement le même code, déjà couvert par les 52 tests de l'oreille. Une façade **`jarvis-ear-ffi`** (cdylib, UniFFI 0.32, macros procédurales) expose à Kotlin un objet `EarEngine` : le son du micro entre par `push_audio` (octets PCM 16 bits), la voix et les bips sortent par une interface `EarHost` implémentée en Kotlin (`play_pcm`, `start_capture`…), l'état sort par `status_changed` avec le texte de la notification déjà calculé en Rust. L'appli Kotlin (`apps/android`, sans AndroidX ni Compose) ne fait que les entrées-sorties Android : `AudioRecord`, `AudioTrack`, service, notification, verrous, écrans.

**Tech Stack :** UniFFI 0.32 (`uniffi::setup_scaffolding!`, `#[uniffi::export]`, `#[uniffi::export(foreign)]` pour l'interface implémentée en Kotlin, binaire `uniffi-bindgen` derrière une fonctionnalité), cargo-ndk 4.1.2, NDK 28.2.13676358, cible `aarch64-linux-android` (API 33), Gradle 8.14 (déjà en cache), Android Gradle Plugin 8.11.1 et Kotlin 2.2.20 (déjà en cache), JNA 5.17.0 (`@aar`), zxing-android-embedded 4.3.0 (zxing core 3.4.1), kotlinx-coroutines 1.10.2 (seulement si les liaisons générées l'exigent), JUnit 4.13.2. Existant : tout `jarvis-ear-core` (rustpotter, earshot, tokio, rustls/`ring`, mdns-sd).

**Spec :** [docs/superpowers/specs/2026-10-28-jarvis-design.md](../specs/2026-10-28-jarvis-design.md) (§5.2, §5.3, §5.5, §5.7, §7.1, §9)

## Global Constraints

- **Téléphone :** Pixel 4 XL, Android 13 (API 33, son dernier système), **arm64-v8a seulement**. `minSdk = 33`, `targetSdk = 33` (règles de l'API 34+ sur les services au démarrage hors sujet sur ce téléphone ; les permissions de l'API 34 sont quand même déclarées), `compileSdk = 36` (plateforme installée), `buildToolsVersion = "35.0.0"` (installé), `ndkVersion = "28.2.13676358"`.
- **Machine de compilation :** `ANDROID_HOME=/home/charles-gamligo/Android`, pas d'Android Studio. Gradle tourne avec **JDK 21** (`/usr/lib/jvm/java-21-openjdk-amd64`, sinon 17) : le JDK 23 par défaut n'est pas utilisé pour AGP ; bytecode Java/Kotlin cible 17. Pas de « toolchain » Gradle (elle pourrait télécharger un JDK).
- **Dépôt cargo :** `CARGO_TARGET_DIR=/home/charles-gamligo/Work/Mawena/Projects/Jarvis/target` pour **toute** compilation, y compris dans les worktrees et pour Android (jamais un autre dossier de cible : ~17 Go libres). Une seule ABI. Jamais `cargo update` sans `-p` (`half` reste en 2.4.1) ; l'ajout d'UniFFI ne doit faire qu'**ajouter** des entrées à `Cargo.lock` (`git diff Cargo.lock`).
- **`scripts/check.sh` reste vert et sans Android :** `jarvis-ear-ffi` se compile et se teste sur Linux (cdylib + lib) ; le binaire `uniffi-bindgen` n'est compilé qu'avec `--features bindgen` (`required-features`), donc pas par `check.sh`. La compilation Android et Gradle sont dans `scripts/android-build.sh`, hors de `check.sh`.
- **Tests :** jamais de micro, de son, de service, ni de téléphone. Tests Rust sur Linux (faux `EarHost`, faux lien, faux détecteurs) ; tests Kotlin en JVM pur (`testDebugUnitTest`) sur des classes sans Android ni UniFFI.
- **Téléphone branché (`adb`, numéro 98291FFBA003RN, autorisé) :** seule la session principale lance `adb` (installation, lancement, `logcat`, `getprop`, `dumpsys`) ; rien qui ouvre le micro du téléphone ou joue un son : ça, c'est l'essai de Charles (5.7). Les agents des tâches n'appellent jamais `adb`.
- **Réseau pendant la construction :** Gradle 8.14, AGP 8.11.1, Kotlin 2.2.20, coroutines, JUnit sont en cache ; JNA (`aar`) et zxing seront téléchargés au premier `assembleDebug`, puis `--offline` suffit. Les crates UniFFI viennent de crates.io (premier `cargo build`).
- **Kotlin minimal :** `android.app.Activity` et vues du cadre Android, pas d'AndroidX, d'AppCompat ni de Compose. Aucune logique de Jarvis en Kotlin (élection, réponses, textes d'état : en Rust). Code, commentaires, textes et messages en français.
- **Données de l'oreille :** dans `context.filesDir` (privé à l'appli) : `pairing/` (`PairingStore`, 0700, `pairing.toml` 0600, phrase hors ligne), `wakeword/samples/jarvis-NN.wav` et `wakeword/jarvis.rpw` (0600), fichier `muted`. Le jeton ne sort jamais dans `logcat` (`Debug` masqué, déjà fait dans `Pairing`).
- **Aucun son hors des moments qui suivent « Jarvis »** (anti-objectif §2) : inchangé, c'est le code commun ; les prises d'enrôlement sont les seuls sons écrits sur le téléphone, comme sur le PC.
- **Plateforme annoncée :** `"android"` dans `EarMessage::Pair` (chaîne libre du protocole v1, pas de changement de protocole). Nom par défaut : `Build.MODEL` (« Pixel 4 XL »).
- Règles des phases précédentes : git flow (branche `feature/oreille-android`), commits au nom de Charles **sans aucune attribution**, `ROADMAP.md` coché dès qu'une tâche est fusionnée, `scripts/check.sh` vert avant chaque fusion, au plus **4 agents** en parallèle (worktrees séparés).

## Review Focus

1. **Micro rendu muet par Android** : service démarré hors premier plan (règle « while-in-use » d'Android 11+, qui ne fait pas exception pour `BOOT_COMPLETED`), ou accès au micro coupé dans les réglages rapides : `AudioRecord` livre des zéros exacts et l'oreille paraît marcher sans rien entendre. Attendu : état « micro bloqué » dans la notification après 3 s de zéros exacts, et relance d'un toucher. → `trois_secondes_de_zeros_micro_bloque`, `vrai_son_debloque` (tâche 5), `BootDecisionTest.sansSuperpositionNotificationATapper` (tâche 8).
2. **Fin de lecture périmée** : `playback_idle` d'un son précédent arrive après un nouveau `play_pcm` ; l'oreille se croirait au repos pendant que Jarvis parle (seuil de réveil normal pendant la voix de Jarvis, écoute d'une réponse ouverte trop tôt). Attendu : seule la fin de la dernière génération compte. → `fin_d_une_ancienne_lecture_ignoree` (tâche 5), `PlaybackQueueTest.idleSeulementPourLaDerniereGeneration` (tâche 6).
3. **Double démarrage** (redémarrage « sticky », toucher de la notification, démarrage automatique, écran principal) : deux moteurs, deux connexions, le cerveau remplace sans fin la session de l'oreille. Attendu : un seul moteur, un seul lien. → `start_deux_fois_un_seul_lien` (tâche 5), `ServiceStateTest.startIgnoreSiDejaLance` (tâche 6).
4. **Saisie d'appairage douteuse** : URI collée avec espaces ou retour à la ligne, QR code d'autre chose, téléphone sur la 4G : message clair en français, rien d'enregistré, aucun appairage à moitié. → `uri_avec_espaces_acceptee`, `uri_invalide_rien_enregistre`, `cerveau_injoignable_message_clair` (tâche 5), `PairInputTest` (tâche 7).
5. **Appels hors séquence depuis Kotlin** : `push_audio` avant `start` ou après `stop`, octets impairs, `stop` deux fois, format jamais annoncé : jamais de panique à travers JNA (l'appli planterait). → `push_audio_hors_service_ignore`, `octets_impairs_ignores`, `stop_idempotent_et_borne` (tâche 5).

Hors tests (essai 5.7 seulement) : écran éteint et Wi-Fi en économie d'énergie (réveils retardés au-delà de la fenêtre de 300 ms ou du délai d'élection de 2 s) ; c'est l'objet des verrous de la tâche 6 et des étapes « écran éteint » et « autre pièce » de la fiche d'essai.

---

Démarrage : `git flow feature start oreille-android` depuis `develop` (fait : branche `feature/oreille-android`).

## Ordre et parallélisme

| Vague | Tâches (une par agent, worktree séparé) | Dépend de |
|---|---|---|
| 0 (4 agents) | **Task 1** session commune dans `jarvis-ear-core` · **Task 2** appairage et enrôlement portables · **Task 3** chaîne Android (squelette `jarvis-ear-ffi`, projet Gradle, scripts) · **Task 4** cerveau : score de voix par oreille, `doctor` | — |
| A (4 agents) | **Task 5** façade UniFFI complète (Rust) · **Task 6** service, audio, verrous, notification (Kotlin) · **Task 7** écrans : appairage, mot d'éveil, autorisations (Kotlin) · **Task 8** démarrage automatique (Kotlin) | 5 : 1, 2, 3 ; 6, 7, 8 : 3 |
| B (1 agent) | **Task 9** documentation, fiche d'essai, README | 5 à 8 |
| C (session principale) | **Task 10** relecture, corrections, essai de fumée par `adb` (sans micro) | tout |
| D | **Task 11** essai réel avec Charles (5.7) | tout |

Chaque tâche ne touche **que** ses fichiers. La tâche 3 crée **toute** la surface UniFFI (types, signatures, corps provisoires) et **tous** les composants Android (manifeste complet, classes Kotlin vides avec leurs signatures publiques, ressources de base) : les tâches 5 à 8 compilent contre ce squelette sans se marcher dessus. Chaque tâche Kotlin a son propre fichier de chaînes (`res/values/strings_<tâche>.xml`). Fusion de chaque tâche dans `feature/oreille-android` après relecture, `scripts/check.sh` vert, et pour les tâches 3, 5 à 8 `scripts/android-build.sh --test` vert.

### Task 1 : session commune de l'oreille dans `jarvis-ear-core` (5.2)

Déplacement **sans changement de comportement** : l'oreille du PC doit rester identique.

**Files :** Create `crates/jarvis-ear-core/src/session/{mod.rs,audio.rs,status.rs,sounds.rs,tests.rs}` ; Modify `crates/jarvis-ear-core/src/{lib.rs,net/mod.rs}` ; Modify `crates/jarvis-ear/src/{main.rs,playback.rs,capture.rs,tray.rs,link.rs,client.rs}` ; Delete `crates/jarvis-ear/src/{app.rs,sounds.rs}` ; Modify `crates/jarvis-brain/tests/budget_ecoute.rs` (chemin lu), `crates/jarvis-brain/src/net/hub.rs` (commentaire qui cite `LISTEN_TIMEOUT`)

**Interfaces :**
- Produces (`jarvis_ear_core::session`) :
  - `pub struct EarSession<W: WakeDetector, V: Vad, C: Capture>` = l'actuel `App`, même logique, mêmes constructeurs : `new(listener, capture, link: Arc<dyn Link>, events: mpsc::Receiver<LinkEvent>, commands: mpsc::UnboundedReceiver<EarCommand>, status: watch::Sender<EarStatus>) -> Self`, `with_player(Box<dyn Player>)`, `with_wake_threshold(f32)`, `with_offline_phrase(OfflinePhrase)`, `with_mute_file(PathBuf)`, `async fn run(self) -> anyhow::Result<()>`.
  - Constantes déplacées telles quelles : `ANNOUNCE_RETRY`, `REPLY_WAIT`, `PLAYBACK_WAKE_MARGIN`, `pub type OfflinePhrase`, et **`pub const RESPONSE_TIMEOUT: Duration = Duration::from_secs(50);`** avec le commentaire de budget de `client.rs` (le test `budget_ecoute` lit désormais `const RESPONSE_TIMEOUT` dans `../../jarvis-ear-core/src/session/mod.rs`).
  - `session::audio` : `pub trait Player` (inchangé : `sample_rate`, `play(Vec<f32>, u32)`, `stop`, `playing() -> watch::Receiver<bool>`, `is_playing`), `pub struct SilentPlayer` (fréquence de repli reprise de `playback.rs`) ; `pub enum CaptureEvent<S> { Format { rate: u32, channels: u16 }, Samples(S), Unavailable }` ; `pub trait Capture: Send { type Samples: std::ops::Deref<Target = [f32]> + Send + 'static; fn start(&mut self, tx: mpsc::Sender<CaptureEvent<Self::Samples>>) -> anyhow::Result<()>; fn stop(&mut self); }`.
  - `session::status` : `pub enum BrainLink { Connected { mode: Mode, local_model: String }, Connecting, Unreachable, Refused }`, `pub struct EarStatus { mic: MicState, capturing: bool, brain: BrainLink, last_heard: Option<String> }` (l'actuel `TrayState`), `pub enum EarCommand { Mute, Unmute, HardMute, Reopen, SetMode(Mode), Quit }` (l'actuel `TrayCommand`).
  - `session::sounds` : `wake(rate)`, `success(rate)`, `failure(rate)` (déplacés).
  - `jarvis_ear_core::net::Link` (trait déplacé de `jarvis-ear/src/link.rs`, mêmes méthodes) et `impl Link for EarLink`.
- Côté PC : `CpalCapture` implémente `Capture` avec `type Samples = capture::Samples` (tampon recyclé inchangé) ; `CpalPlayer` implémente le `Player` du cœur ; `tray.rs` affiche `EarStatus` / émet `EarCommand` (alias permis : `use jarvis_ear_core::session::{EarStatus as TrayState, …}`) ; `Supervised` (appairage local par le socket Unix) reste dans `jarvis-ear` et implémente le `Link` du cœur.
- `jarvis-ear-core` ne dépend toujours ni de `cpal`, ni de `ksni`, ni de `directories` (sauf en dev-dependency).

- [ ] Avant de toucher au code : noter le nombre de tests de `cargo test -p jarvis-ear -p jarvis-ear-core` (lignes « test result »).
- [ ] Déplacer les tests de `app.rs` dans `session/tests.rs` avec leur montage (`FakeWake`, `FakeVad`, `FakePlayer`, `FakeCapture` avec `type Samples = Vec<f32>`, `FakeLink`), noms inchangés (`reveil_envoie_wake_puis_le_flux`, `stop_abandonne_la_capture_sans_reponse`, `cerveau_injoignable_dit_pc_endormi`, `annonce_poussee_jouee_une_fois_au_repos`…, les 52).
- [ ] Déplacer le code, adapter `jarvis-ear` ; aucun test supprimé ; seules les lignes `use` et les noms de types changent dans la logique.
- [ ] Vérifier : même nombre total de tests, tous verts ; `cargo tree -p jarvis-ear-core -e normal | grep -E "cpal|ksni|directories"` vide ; `cargo ndk -t arm64-v8a -P 33 check -p jarvis-ear-core` passe (vérifié le 2026-10-30 avant ce plan : 18 s, `ring` et `candle` se compilent avec le NDK 28) ; test `budget_ecoute` vert.
- [ ] `scripts/check.sh` vert ; commit `refactor: session de l'oreille partagée dans jarvis-ear-core`.

### Task 2 : appairage par URI et enrôlement du mot d'éveil portables (5.2, 5.4)

**Files :** Create `crates/jarvis-ear-core/src/{enroll.rs,net/pair.rs}` ; Modify `crates/jarvis-ear-core/{Cargo.toml,src/lib.rs,src/net/mod.rs}` (`hound` en dépendance normale : déjà tiré par rustpotter, aucune nouvelle crate) ; Modify `crates/jarvis-ear/src/{pair.rs,enroll.rs}`

**Interfaces :**
- `jarvis_ear_core::enroll` (déplacés de `jarvis-ear/src/enroll.rs`, comportement identique) : `pub const TAKE_SECONDS: f32 = 1.5`, `sample_file_name(index: usize) -> String`, `clear_samples(dir) -> anyhow::Result<usize>`, `list_samples(dir)`, `write_wav(path, &[f32]) -> anyhow::Result<()>` (0600), `build_model(samples_dir, output) -> anyhow::Result<usize>` (0600), `create_private_dir(path) -> io::Result<()>` (0700).
  Nouveau : `pub fn take_level_ok(samples: &[f32]) -> bool` : vrai si au moins une fenêtre de 100 ms dépasse **−40 dBFS** RMS (prise audible ; utilisé par Android seulement, le PC garde son comportement).
- `jarvis_ear_core::net::pair` (déplacés de `jarvis-ear/src/pair.rs`) : `pub trait PairNet` (inchangé), `pub struct RealNet { pub platform: &'static str }` (au lieu de la constante `PLATFORM`), `pub async fn pair_uri(net: &dyn PairNet, store: &PairingStore, uri: &str, name: &str) -> anyhow::Result<Pairing>` (l'URI est d'abord `trim()`ée, comme aujourd'hui), et en `pub` les aides `pairing_of`, `refusal`, `unreachable`, `resolve`. `pair_code` (questions au terminal) reste dans `jarvis-ear` et utilise ces aides. `jarvis-ear` passe `RealNet { platform: "linux" }`.

- [ ] Déplacer avec eux les tests qui les concernent (prises, modèle, droits 0600/0700, `pair_uri` avec faux `PairNet`) ; ajouter `prise_silencieuse_refusee` (1,5 s de zéros → `false`), `prise_parlee_acceptee` (sinusoïde à −20 dBFS sur 0,3 s → `true`), `prise_tres_faible_refusee` (−50 dBFS → `false`).
- [ ] `jarvis-ear enroll` et `jarvis-ear pair` inchangés pour l'utilisateur ; tous leurs tests verts.
- [ ] `scripts/check.sh` vert ; commit `refactor: appairage par URI et enrôlement partagés dans le cœur`.

### Task 3 : chaîne de compilation Android et squelettes (5.2)

**Files :** Modify `Cargo.toml` (workspace : `uniffi = "0.32"`), `Cargo.lock` (ajouts seulement), `.gitignore` ; Create `crates/jarvis-ear-ffi/{Cargo.toml,uniffi.toml,src/lib.rs,src/bin/uniffi-bindgen.rs}` ; Create `apps/android/{settings.gradle.kts,build.gradle.kts,gradle.properties,gradlew,gradle/wrapper/gradle-wrapper.jar,gradle/wrapper/gradle-wrapper.properties,app/build.gradle.kts,app/src/main/AndroidManifest.xml}`, `apps/android/app/src/main/kotlin/fr/mawena/jarvis/{MainActivity.kt,PairActivity.kt,EnrollActivity.kt,StartActivity.kt,ListenService.kt,AudioIn.kt,AudioOut.kt,EarNotification.kt,BootReceiver.kt,Prefs.kt}`, `apps/android/app/src/main/res/{values/strings.xml,drawable/ic_jarvis.xml,mipmap-anydpi/ic_launcher.xml}` ; Create `scripts/{android-build.sh,android-install.sh}`

**Interfaces :**
- `crates/jarvis-ear-ffi/Cargo.toml` : `[lib] name = "jarvis_ear_ffi", crate-type = ["cdylib", "lib"]` ; dépendances : `uniffi`, `jarvis-ear-core`, `jarvis-protocol`, `tokio`, `tracing`, `tracing-subscriber`, `thiserror`, `anyhow` ; `[features] bindgen = ["uniffi/cli"]` ; `[[bin]] name = "uniffi-bindgen", path = "src/bin/uniffi-bindgen.rs", required-features = ["bindgen"]` (corps : `fn main() { uniffi::uniffi_bindgen_main() }`). `uniffi.toml` : `[bindings.kotlin] package_name = "fr.mawena.jarvis.ffi"`, `cdylib_name = "jarvis_ear_ffi"`.
- **Surface UniFFI complète** (`src/lib.rs`, `uniffi::setup_scaffolding!();`), corps provisoires qui rendent `EarError::Internal { message: "tâche 5" }` ou ne font rien, sauf `library_version()` et `init_logging()` réels :
  - `#[derive(uniffi::Record)] pub struct EarSettings { pub data_dir: String, pub name: String, pub output_rate: u32, pub wake_threshold: f32 }`
  - `#[derive(Debug, thiserror::Error, uniffi::Error)] pub enum EarError { NotPaired, NoWakeword, InvalidUri { message: String }, Refused { message: String }, Unreachable { message: String }, TooQuiet, Io { message: String }, Internal { message: String } }` (messages `#[error("…")]` en français, affichables tels quels).
  - `#[derive(uniffi::Enum)] pub enum MicView { Open, Muted, HardMuted, Unavailable, Blocked }` ; `#[derive(uniffi::Enum)] pub enum BrainView { Connected { online: bool }, Connecting, Unreachable, Refused }` ; `#[derive(uniffi::Record)] pub struct EarView { pub mic: MicView, pub brain: BrainView, pub text: String, pub attention: bool, pub last_heard: Option<String> }`.
  - `#[uniffi::export(foreign)] pub trait EarHost: Send + Sync { fn start_capture(&self); fn stop_capture(&self); fn play_pcm(&self, pcm_le: Vec<u8>, sample_rate: u32, generation: u64); fn stop_playback(&self); fn status_changed(&self, view: EarView); }` — **aucune méthode ne doit bloquer** (Kotlin délègue à ses fils).
  - `#[derive(uniffi::Object)] pub struct EarEngine` ; `#[uniffi::export] impl EarEngine { #[uniffi::constructor] pub fn new(settings: EarSettings, host: Arc<dyn EarHost>) -> Result<Arc<Self>, EarError>; pub fn start(&self) -> Result<(), EarError>; pub fn stop(&self); pub fn audio_format(&self, rate: u32, channels: u16); pub fn push_audio(&self, pcm_le: Vec<u8>); pub fn capture_unavailable(&self); pub fn playback_idle(&self, generation: u64); pub fn mute(&self); pub fn unmute(&self); pub fn hard_mute(&self); pub fn reopen(&self); pub fn view(&self) -> EarView; }`
  - Fonctions libres `#[uniffi::export]` : `library_version() -> String` (« jarvis-ear-ffi <version> »), `init_logging()`, `paired_brain(data_dir: String) -> Option<String>` (nom du cerveau appairé), `pair_with_uri(data_dir: String, uri: String, name: String) -> Result<String, EarError>` (bloquante, rend le nom du cerveau), `unpair(data_dir: String) -> Result<(), EarError>`, `has_wakeword(data_dir: String) -> bool`, `enroll_reset(data_dir: String) -> Result<(), EarError>`, `enroll_take(data_dir: String, index: u32, pcm_le: Vec<u8>, sample_rate: u32) -> Result<(), EarError>`, `enroll_finish(data_dir: String) -> Result<u32, EarError>`.
  - Son en **octets** PCM i16 little-endian (`Vec<u8>` ↔ `ByteArray` en Kotlin, sans boîte par échantillon), pas `Vec<i16>` (`List<Short>`).
- `init_logging()` : une seule fois (`OnceLock`) ; sur Android (`#[cfg(target_os = "android")]`), couche `tracing_subscriber::fmt` écrivant chaque ligne par `__android_log_write(prio, "JarvisRust", msg)` (déclaration `extern "C"` à la main, `#[link(name = "log")]`, aucune nouvelle crate), niveau `info` ; ailleurs : rien.
- **Projet Gradle** `apps/android` : wrapper créé hors ligne avec la distribution en cache (`~/.gradle/wrapper/dists/gradle-8.14-all/*/gradle-8.14/bin/gradle wrapper --gradle-version 8.14 --distribution-type all`, même `distributionUrl` donc rien à télécharger) ; plugins `com.android.application` 8.11.1 et `org.jetbrains.kotlin.android` 2.2.20 ; `namespace` et `applicationId` `fr.mawena.jarvis` ; `defaultConfig { minSdk = 33; targetSdk = 33; ndk { abiFilters += "arm64-v8a" } }` ; `sourceSets["main"].java.srcDir("build/generated/uniffi")`, `sourceSets["main"].jniLibs.srcDir("build/rustJniLibs")` ; Java et Kotlin `jvmTarget` 17 ; dépendances `net.java.dev.jna:jna:5.17.0@aar`, `com.journeyapps:zxing-android-embedded:4.3.0`, `testImplementation("junit:junit:4.13.2")` (+ `kotlinx-coroutines-core:1.10.2` seulement si le code généré l'importe). Paquet `debug` signé par `~/.android/debug.keystore` (existant).
- **Manifeste complet** (les tâches 6 à 8 n'y touchent plus) : permissions `RECORD_AUDIO`, `INTERNET`, `ACCESS_NETWORK_STATE`, `ACCESS_WIFI_STATE`, `CHANGE_WIFI_MULTICAST_STATE`, `WAKE_LOCK`, `FOREGROUND_SERVICE`, `FOREGROUND_SERVICE_MICROPHONE`, `POST_NOTIFICATIONS`, `RECEIVE_BOOT_COMPLETED`, `SYSTEM_ALERT_WINDOW`, `REQUEST_IGNORE_BATTERY_OPTIMIZATIONS`, `CAMERA` ; `<service android:name=".ListenService" android:foregroundServiceType="microphone" android:exported="false"/>` ; `<receiver android:name=".BootReceiver" android:exported="true">` avec `BOOT_COMPLETED` et `MY_PACKAGE_REPLACED` ; activités `MainActivity` (lanceur), `PairActivity`, `EnrollActivity`, `StartActivity` (`@android:style/Theme.Translucent.NoTitleBar`, `excludeFromRecents`, `noHistory`, `exported="false"`).
- **Signatures Kotlin publiques** créées vides ici (Produces pour 6 à 8) : `object Prefs { fun name(ctx): String; fun setName(ctx, String); fun threshold(ctx): Float /* 0.5 */; fun setThreshold(ctx, Float); fun autostart(ctx): Boolean /* true */; fun setAutostart(ctx, Boolean) }` (`SharedPreferences` « jarvis ») ; `class ListenService : Service` avec `companion object { const val ACTION_START, ACTION_STOP, ACTION_MUTE, ACTION_UNMUTE, ACTION_HARD_MUTE, ACTION_REOPEN, ACTION_RESTART; fun start(ctx: Context); fun stop(ctx: Context); fun send(ctx: Context, action: String); fun isRunning(): Boolean; fun currentView(): EarView? }` ; `object EarNotification { const val CHANNEL_LISTEN = "ecoute"; const val CHANNEL_ALERT = "alerte"; const val ID_LISTEN = 1; const val ID_START = 2; fun createChannels(ctx) }`.
- `MainActivity` provisoire : appelle `initLogging()` et affiche `libraryVersion()` ; journalise `Log.i("Jarvis", "bibliothèque chargée : …")`. N'ouvre jamais le micro.
- `scripts/android-build.sh [--test] [--offline]` (bash, `set -euo pipefail`) :
  1. vérifie `ANDROID_HOME` (défaut `~/Android`), le NDK 28.2.13676358 (`ANDROID_NDK_HOME`), `rustup target list --installed | grep aarch64-linux-android`, `cargo ndk --version` ; message clair et code 1 sinon ;
  2. choisit `JAVA_HOME` : `$JARVIS_JAVA_HOME`, sinon JDK 21, sinon 17 ;
  3. `CARGO_TARGET_DIR` : garde la valeur reçue, sinon `<dépôt>/target` ;
  4. `cargo ndk -t arm64-v8a -P 33 -o apps/android/app/build/rustJniLibs build --release -p jarvis-ear-ffi` ;
  5. `cargo run -q -p jarvis-ear-ffi --features bindgen --bin uniffi-bindgen -- generate "$CARGO_TARGET_DIR/aarch64-linux-android/release/libjarvis_ear_ffi.so" --language kotlin --out-dir apps/android/app/build/generated/uniffi --no-format` (sur la bibliothèque non dépouillée : les métadonnées UniFFI y sont) ;
  6. écrit `apps/android/local.properties` (`sdk.dir=…`, ignoré par git) ; `./gradlew assembleDebug` (et `testDebugUnitTest` avec `--test`, `--offline` transmis) ; AGP dépouille la `.so` à l'empaquetage (`ndkVersion` fixé) ;
  7. vérifie l'APK (`unzip -l` : `lib/arm64-v8a/libjarvis_ear_ffi.so` et `lib/arm64-v8a/libjnidispatch.so`, aucune autre ABI) et affiche son chemin et sa taille.
- `scripts/android-install.sh [--no-grant] [--copy-wakeword] [--logs]` : exactement un appareil `adb` autorisé (sinon : comment activer le débogage USB) ; `adb install -r -g <apk>` (`-g` accorde micro, notifications, caméra) ; sauf `--no-grant` : `adb shell appops set fr.mawena.jarvis SYSTEM_ALERT_WINDOW allow` et `adb shell dumpsys deviceidle whitelist +fr.mawena.jarvis` ; `--copy-wakeword` : copie le modèle du PC (`~/.local/share/jarvis/wakeword/jarvis.rpw`) par `adb push` vers `/data/local/tmp/` puis `adb shell run-as fr.mawena.jarvis sh -c 'mkdir -p files/wakeword && cp /data/local/tmp/jarvis.rpw files/wakeword/ && chmod 600 files/wakeword/jarvis.rpw'` puis supprime la copie temporaire (repli si l'enrôlement sur le téléphone déçoit) ; puis `adb shell am start -n fr.mawena.jarvis/.MainActivity` ; `--logs` : `adb logcat -v time -s Jarvis:V JarvisRust:V AndroidRuntime:E`.
- `.gitignore` : `apps/android/.gradle/`, `apps/android/build/`, `apps/android/app/build/`, `apps/android/local.properties`.

- [ ] Test Rust : `version_de_la_bibliotheque` (`library_version()` commence par « jarvis-ear-ffi »), `init_logging_deux_fois_sans_panique`, `erreurs_lisibles_en_francais` (chaque variante d'`EarError` a un message non vide).
- [ ] `git diff Cargo.lock` : seulement des ajouts (UniFFI et ses dépendances) ; `scripts/check.sh` vert.
- [ ] `bash -n scripts/android-*.sh` ; `scripts/android-build.sh --test` produit l'APK (premier passage : réseau pour JNA et zxing) ; noter la taille de l'APK et de la `.so` dans le commit.
- [ ] Commit `build: chaîne Android (UniFFI, Gradle, scripts) et squelettes de l'oreille Android`.
- [ ] **Session principale** (pas l'agent) : `scripts/android-install.sh --no-grant`, puis `adb logcat -d -s Jarvis` montre « bibliothèque chargée : jarvis-ear-ffi … » ; aucune ligne `AndroidRuntime: FATAL`. La chaîne entière est validée avant la vague A.

### Task 4 : cerveau : score de voix par oreille, oreilles Android dans `doctor` (5.8)

**Files :** `crates/jarvis-brain/src/{speaker/mod.rs,listen.rs}` ; `crates/jarvisctl/src/ops.rs`

**Interfaces :**
- Journal du cerveau : les lignes de vérification de la voix portent l'oreille et le seuil : `info!(oreille = %ear, score = "0.62", seuil = "0.55", "voix reconnue")`, `info!(oreille = %ear, score = …, seuil = …, "voix non reconnue")`, `info!(oreille = %ear, "voix non reconnue (phrase trop courte)")`. L'empreinte vocale a été enregistrée sur le micro du PC : l'essai 5.7 lira ces scores pour le Pixel (`jarvisctl logs | grep voix`). Aucun changement de seuil dans cette phase.
- `jarvisctl doctor` : pour chaque oreille non locale appairée mais déconnectée, une ligne d'information (pas un avertissement : un téléphone peut être éteint) `· Oreille « Pixel 4 XL » (android) déconnectée — ouvrez l'appli Jarvis sur le téléphone`.

- [ ] Tests : `journal_du_score_nomme_l_oreille_et_le_seuil` (abonné `tracing_subscriber` de test qui écrit dans un tampon, `tracing::subscriber::with_default`, faux vérificateur existant du module `speaker`), `doctor_signale_l_oreille_android_deconnectee`, `doctor_ne_signale_pas_l_oreille_locale_deconnectee`.
- [ ] `scripts/check.sh` vert ; commit `feat: score de voix par oreille dans le journal, oreilles distantes dans doctor`.

### Task 5 : façade UniFFI complète (`jarvis-ear-ffi`) (5.2, 5.3, 5.6)

**Files :** `crates/jarvis-ear-ffi/src/{lib.rs,engine.rs,host.rs,view.rs,pairing.rs,enroll.rs,logging.rs}` (découpage libre derrière la surface figée par la tâche 3), `crates/jarvis-ear-ffi/Cargo.toml` (dev : `tempfile`, `hound`)

**Interfaces :**
- Consumes : `EarSession`, `Capture`, `Player`, `CaptureEvent`, `EarStatus`, `EarCommand`, `Link`, `sounds` (tâche 1) ; `enroll::*`, `net::pair::{pair_uri, RealNet}` (tâche 2) ; `EarLink`, `LinkConfig`, `PairingStore`, `TlsConnector`, `pinned_client_config`, `MdnsDiscovery`, `RustpotterDetector`, `EarshotVad`, `Listener` (existants).
- Produces : la surface de la tâche 3, avec ces comportements :
  - **Dossiers** sous `data_dir` : `pairing/`, `wakeword/samples/`, `wakeword/jarvis.rpw`, `muted` ; créés 0700.
  - `EarEngine::new` : `NotPaired` sans appairage ; `NoWakeword` sans `jarvis.rpw` ; charge `RustpotterDetector::load(path, wake_threshold)` ; ne lance rien.
  - `start()` : **idempotent** (un second appel ne fait rien) ; fil dédié qui possède un runtime tokio multi-fil à 2 fils ; lance `EarLink::spawn(LinkConfig::new(pairing, store, TlsConnector(pinned_client_config(&fp)), Some(MdnsDiscovery)))` (mDNS indisponible → `None`, avertissement) et `EarSession::new(Listener::new(wake, EarshotVad::new()), FfiCapture, link, …).with_player(FfiPlayer).with_wake_threshold(t).with_offline_phrase(Arc::new(move || store.offline_phrase().map(|(_, a)| a))).with_mute_file(data_dir/muted)` (comme `main.rs` du PC). `stop()` : idempotent, envoie `EarCommand::Quit` et attend la fin au plus **5 s** (la session ferme le lien en 4 s au plus).
  - `FfiCapture` (`type Samples = Vec<f32>`) : `start(tx)` garde `tx` et appelle `host.start_capture()` ; `stop()` oublie `tx` et appelle `host.stop_capture()` (coupure totale = micro réellement fermé côté Android). `audio_format` → `CaptureEvent::Format` ; `push_audio` → octets i16 LE convertis (`i16_to_f32`), dernier octet impair ignoré, `try_send` (file pleine : paquet perdu, un seul avertissement) ; `capture_unavailable` → `CaptureEvent::Unavailable` ; sans `tx` (avant `start`, après `stop`, coupure totale) : ignoré.
  - **Micro bloqué** : `push_audio` compte les échantillons **exactement nuls** consécutifs ; au-delà de **3 s** (au format annoncé) → `MicView::Blocked` ; un échantillon non nul lève l'état.
  - `FfiPlayer` : `sample_rate()` = `output_rate` ; `play(samples, rate)` → génération +1, `playing = true`, `host.play_pcm(octets i16 LE, rate, génération)` ; `stop()` → `playing = false`, `host.stop_playback()` ; `playback_idle(g)` → `playing = false` **seulement si `g` est la dernière génération**.
  - `mute`, `unmute`, `hard_mute`, `reopen` → `EarCommand` correspondante (sans effet si le moteur ne tourne pas).
  - **Vue** : `pub fn view_of(status: &EarStatus, blocked: bool) -> EarView` (pure) ; texte de notification (exact) :
    - micro : `HardMuted` → « Micro coupé (coupure totale) » ; `Unavailable` → « Micro indisponible » ; `blocked` → « Micro bloqué par Android : touchez pour relancer » ; `Muted` → « Écoute coupée » ; `Open` → « À l'écoute » ;
    - cerveau, ajouté après « · » : `Connected { online: false }` → « mode local », `{ online: true }` → « mode en ligne » ; `Connecting` → « connexion au PC… » ; `Unreachable` → « PC injoignable » ; `Refused` → « appairage refusé : réappairez depuis l'appli » ;
    - exemple : « À l'écoute · mode local », « Écoute coupée · PC injoignable » ;
    - `attention = blocked || Unavailable || Unreachable || Refused`.
  - `host.status_changed(view)` appelé au démarrage puis **seulement quand `(mic, brain, text)` change** (pas à chaque capture : `capturing` n'est pas dans la vue, pas de clignotement de la notification).
  - `pair_with_uri` : `pair_uri(&RealNet { platform: "android" }, …)` dans un petit runtime courant ; erreurs : URI illisible → `InvalidUri` (« ce n'est pas une adresse d'appairage de Jarvis ») ; refus du cerveau → `Refused` (texte de `refusal`) ; rien de joignable → `Unreachable` (« PC injoignable : même Wi-Fi que le PC ? `jarvis pair` est-il ouvert ? ») ; rien n'est enregistré en cas d'échec.
  - `unpair` : supprime `pairing/` (appairage et phrase hors ligne), pas le mot d'éveil.
  - Enrôlement : `enroll_reset` = `clear_samples` ; `enroll_take` : convertit, rééchantillonne en 16 kHz si besoin (`resample`), refuse par `TooQuiet` si `!take_level_ok`, sinon `write_wav(samples/jarvis-NN.wav)` ; `enroll_finish` : `build_model` → nombre de prises ; moins de 3 prises → `Io { message: "il faut au moins 3 prises" }`. Kotlin en demande 8, comme `jarvis-ear enroll`.
- Construction interne de test : `EarEngine::with_parts(settings, host, link: Arc<dyn Link>, events, wake: impl WakeDetector + 'static)` (`pub(crate)`), pour les tests sans réseau ni modèle.

- [ ] Tests (faux `EarHost` qui enregistre les appels, faux lien de la tâche 1 recopié en petit, faux détecteur ; tout sur Linux) :
  - vue : `vue_a_l_ecoute_mode_local`, `vue_a_l_ecoute_mode_en_ligne`, `vue_ecoute_coupee_pc_injoignable`, `vue_coupure_totale`, `vue_micro_indisponible`, `vue_micro_bloque_demande_attention`, `vue_appairage_refuse`, `statut_envoye_seulement_quand_la_vue_change` ;
  - son : `trois_secondes_de_zeros_micro_bloque`, `vrai_son_debloque`, `octets_impairs_ignores`, `push_audio_hors_service_ignore`, `format_48k_reechantillonne_en_16k` (le faux détecteur voit 1/3 des échantillons), `fin_d_une_ancienne_lecture_ignoree`, `fin_de_la_derniere_lecture_repasse_au_repos`, `bip_de_reveil_envoye_a_la_frequence_de_sortie`, `voix_de_jarvis_transmise_a_sa_frequence` ;
  - cycle : `moteur_sans_appairage_not_paired`, `moteur_sans_mot_d_eveil_no_wakeword`, `start_deux_fois_un_seul_lien`, `stop_idempotent_et_borne` (< 5 s), `coupure_totale_ferme_le_micro` (`stop_capture` appelé), `reouverture_rouvre_le_micro`, `ecoute_coupee_retenue_au_redemarrage` (fichier `muted`) ;
  - appairage : `uri_avec_espaces_acceptee` (faux `PairNet`), `uri_invalide_rien_enregistre`, `cerveau_injoignable_message_clair` (URI vers `127.0.0.1` et un port fermé), `desappairage_garde_le_mot_d_eveil` ;
  - enrôlement : `prise_trop_faible_refusee`, `huit_prises_construisent_le_modele` (WAV synthétiques comme le test de `rustpotter.rs`), `moins_de_trois_prises_refusees`, `prises_et_modele_prives`.
- [ ] `scripts/check.sh` vert ; `scripts/android-build.sh` vert (les liaisons régénérées compilent avec le Kotlin du squelette) ; commit `feat: moteur de l'oreille Android exposé par UniFFI`.

### Task 6 : service au premier plan, micro, voix, verrous, notification (5.3, 5.6)

**Files :** `apps/android/app/src/main/kotlin/fr/mawena/jarvis/{ListenService.kt,AudioIn.kt,AudioOut.kt,PlaybackQueue.kt,EarNotification.kt,ServiceState.kt}`, `apps/android/app/src/main/res/values/strings_service.xml`, `apps/android/app/src/test/kotlin/fr/mawena/jarvis/{PlaybackQueueTest.kt,ServiceStateTest.kt,NotificationActionsTest.kt}`

**Interfaces :**
- Consumes : `EarEngine`, `EarHost`, `EarSettings`, `EarView`, `MicView`, `EarError` (liaisons générées, tâches 3 et 5), `Prefs`.
- Produces : `ListenService.start/stop/send/isRunning/currentView` (signatures de la tâche 3).
- `ListenService` :
  - `onStartCommand(ACTION_START)` : `startForeground(ID_LISTEN, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE)` **tout de suite** ; un seul `EarEngine` à la fois (`ServiceState` : `START` ignoré si déjà lancé) ; `EarEngine(EarSettings(filesDir.path, Prefs.name, AudioOut.nativeRate(), Prefs.threshold), host)` puis `start()`. `NotPaired` / `NoWakeword` → notification « Ouvrez l'appli Jarvis pour finir la configuration » puis `stopSelf()`. Rend `START_STICKY`.
  - `ACTION_MUTE` / `UNMUTE` / `HARD_MUTE` / `REOPEN` → méthode du moteur ; `ACTION_RESTART` → arrêt complet puis redémarrage du moteur (après un micro bloqué, appelé depuis une activité visible) ; `ACTION_STOP` → `stopForeground(STOP_FOREGROUND_REMOVE)`, `stopSelf()`.
  - Verrous pris au démarrage du moteur, rendus dans `onDestroy` : `PowerManager.PARTIAL_WAKE_LOCK` « jarvis:ecoute » (traitement du son écran éteint), `WifiManager.createWifiLock(WIFI_MODE_FULL_HIGH_PERF, "jarvis")` (le mode « basse latence » ne vaut qu'écran allumé), `WifiManager.createMulticastLock("jarvis-mdns")` avec `setReferenceCounted(false)` (sans lui, Android filtre le multicast et mdns-sd ne voit rien).
  - `onDestroy` : `engine.stop()` (hors fil principal si besoin), `AudioIn.stop()`, `AudioOut.release()`, verrous rendus.
- `AudioIn` (`start_capture` / `stop_capture` de l'hôte, jamais bloquants : fil « jarvis-micro ») : `AudioRecord(MediaRecorder.AudioSource.VOICE_RECOGNITION, 16000, CHANNEL_IN_MONO, ENCODING_PCM_16BIT, max(minBuffer, 6400))` ; si non initialisé : même chose à 48 000 Hz ; sinon `engine.captureUnavailable()`. Puis `engine.audioFormat(rate, 1)` et boucle de lecture par blocs de **20 ms** (`rate / 50` échantillons, `ByteArray`) → `engine.pushAudio(bytes)`. `VOICE_RECOGNITION` : prévu pour la reconnaissance (pas de contrôle de gain agressif) ; l'écho de la voix de Jarvis est géré par le seuil de réveil relevé pendant la lecture (code commun).
- `AudioOut` + `PlaybackQueue` (pure, testée en JVM) : file `(octets, fréquence, génération)` lue par un fil « jarvis-voix » ; `AudioTrack` en flux (`USAGE_ASSISTANT`, `CONTENT_TYPE_SPEECH`, mono 16 bits), recréé si la fréquence change ; focus audio `AUDIOFOCUS_GAIN_TRANSIENT_MAY_DUCK` demandé au premier son, rendu au repos ; quand la file est vide et la tête de lecture a atteint la fin écrite → `engine.playbackIdle(génération du dernier son)`. `stop_playback` → file vidée, `pause()` + `flush()`, focus rendu (pas d'appel à `playbackIdle` : le moteur s'est déjà mis au repos). `nativeRate()` = `AudioTrack.getNativeOutputSampleRate(STREAM_MUSIC)`.
- `EarNotification` : canaux `ecoute` (`IMPORTANCE_LOW`, sans son) et `alerte` (`IMPORTANCE_DEFAULT`) ; notification `setOngoing(true)`, `setOnlyAlertOnce(true)`, icône `ic_jarvis`, titre « Jarvis », texte = `view.text` ; toucher → `MainActivity` (ou `StartActivity` avec `ACTION_RESTART` quand `mic == Blocked`) ; boutons calculés par `actionsFor(muted: Boolean, hardMuted: Boolean): List<Pair<Int /* chaîne */, String /* action */>>` (pure) : écoute ouverte → « Couper l'écoute » (`MUTE`), coupée → « Réactiver l'écoute » (`UNMUTE`) ; toujours « Coupure totale » (`HARD_MUTE`), qui devient « Rouvrir le micro » (`REOPEN`) en coupure totale (et le bouton d'écoute disparaît). `PendingIntent.getService(…, FLAG_IMMUTABLE)` vers `ListenService`. `status_changed` arrive d'un fil Rust : mise à jour postée sur le fil principal.

- [ ] Tests JVM : `PlaybackQueueTest.idleSeulementPourLaDerniereGeneration`, `PlaybackQueueTest.stopVideLaFile`, `PlaybackQueueTest.changementDeFrequenceRecreeLaSortie`, `ServiceStateTest.startIgnoreSiDejaLance`, `ServiceStateTest.restartArreteAvantDeRelancer`, `NotificationActionsTest.ecouteOuverte`, `NotificationActionsTest.ecouteCoupee`, `NotificationActionsTest.coupureTotaleNeProposeQueRouvrir`.
- [ ] `scripts/android-build.sh --test` vert ; commit `feat: service d'écoute Android, voix de Jarvis et notification permanente`.

### Task 7 : écrans : appairage, mot d'éveil, autorisations, réglages (5.4)

**Files :** `apps/android/app/src/main/kotlin/fr/mawena/jarvis/{MainActivity.kt,PairActivity.kt,EnrollActivity.kt,PairInput.kt,EnrollFlow.kt,Permissions.kt}`, `apps/android/app/src/main/res/layout/{activity_main.xml,activity_pair.xml,activity_enroll.xml}`, `apps/android/app/src/main/res/values/strings_screens.xml`, `apps/android/app/src/test/kotlin/fr/mawena/jarvis/{PairInputTest.kt,EnrollFlowTest.kt}`

**Interfaces :**
- Consumes : `pairWithUri`, `pairedBrain`, `unpair`, `hasWakeword`, `enrollReset`, `enrollTake`, `enrollFinish`, `EarError` (tâche 5) ; `ListenService.*`, `Prefs` (tâche 3).
- `MainActivity` : liste d'étapes avec leur état et un bouton chacune : 1. autorisations (micro et notifications par `requestPermissions`) ; 2. appairage (« Appairé à « <cerveau> » » / « Appairer ») ; 3. mot d'éveil (« Prêt » / « Enregistrer ») ; 4. batterie (`ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS` avec `package:fr.mawena.jarvis`, si `!isIgnoringBatteryOptimizations`) ; 5. démarrage seul (`Settings.ACTION_MANAGE_OVERLAY_PERMISSION`, expliqué : « pour que l'écoute reparte seule quand le téléphone redémarre ») ; puis « Démarrer l'écoute » / « Arrêter l'écoute », case « Démarrer avec le téléphone » (`Prefs.autostart`), nom de l'oreille, seuil du mot d'éveil (0,30 à 0,80, appliqué au prochain démarrage), « Désappairer ». `onResume` : si le service tourne et `currentView()?.mic == Blocked` → `ListenService.send(ACTION_RESTART)` (l'activité est visible : Android rend le micro).
- `PairActivity` : « Scanner le QR code » (`IntentIntegrator(this).setPrompt("QR code de jarvis pair").setBeepEnabled(false).setOrientationLocked(false).initiateScan()`, résultat dans `onActivityResult` ; la bibliothèque demande elle-même l'accès à la caméra) ; champ « … ou collez l'adresse jarvis://pair?… » ; champ du nom (défaut `Build.MODEL`). `pairWithUri` sur un fil d'arrière-plan ; succès : « Appairé à « <cerveau> » », retour ; échec : message d'`EarError` affiché tel quel. Si le service tournait, il est relancé après l'appairage.
- `EnrollActivity` : arrête l'écoute pendant l'enrôlement (et la relance après) ; `enrollReset` ; 8 prises : bouton « Prise n / 8 » → 800 ms d'attente (« Préparez-vous… ») → « Dites « Jarvis » » pendant 1,5 s (`AudioRecord` 16 kHz comme `AudioIn`, fermé aussitôt) → `enrollTake` ; `TooQuiet` → « Trop faible : rapprochez-vous et recommencez » (même numéro) ; après la 8e : `enrollFinish` → « Mot d'éveil prêt (8 prises) ».
- `PairInput.clean(raw: String): String?` (pure) : garde la première ligne non vide, retire espaces et guillemets autour ; `null` si elle ne commence pas par `jarvis://pair?`. `EnrollFlow` (pure) : numéro de la prise, `accepted()`, `tooQuiet()`, `done` à 8.

- [ ] Tests JVM : `PairInputTest.uriAvecEspacesEtRetourALaLigne`, `PairInputTest.uriEntreGuillemets`, `PairInputTest.autreTexteRefuse`, `EnrollFlowTest.huitPrisesPuisFini`, `EnrollFlowTest.priseTropFaibleRefaiteSousLeMemeNumero`.
- [ ] `scripts/android-build.sh --test` vert ; commit `feat: écrans d'appairage, d'enregistrement du mot d'éveil et d'autorisations`.

### Task 8 : démarrage automatique (5.5)

**Files :** `apps/android/app/src/main/kotlin/fr/mawena/jarvis/{BootReceiver.kt,StartActivity.kt,BootDecision.kt}`, `apps/android/app/src/main/res/values/strings_boot.xml`, `apps/android/app/src/test/kotlin/fr/mawena/jarvis/BootDecisionTest.kt`

**Règle Android 13 (vérifiée le 2026-10-30 sur developer.android.com, « Restrictions on starting a foreground service from the background ») :** `BOOT_COMPLETED` permet de **démarrer** un service au premier plan depuis l'arrière-plan, mais **pas** d'accéder au micro : un service qui a besoin d'une permission « pendant l'utilisation » (micro) doit être démarré **pendant qu'une activité de l'appli est visible**, sauf exceptions (composant système, widget, **toucher d'une notification**, `PendingIntent` d'une appli visible, propriétaire de l'appareil, `VoiceInteractionService`). Démarré au boot, il tournerait mais n'entendrait que des zéros. (À partir d'Android 14, c'est même interdit pour le type micro.) Une appli qui a la permission « Afficher par-dessus d'autres applications » (`SYSTEM_ALERT_WINDOW`) a le droit de lancer une activité depuis l'arrière-plan.

**Interfaces :**
- `BootDecision.decide(autostart: Boolean, ready: Boolean /* appairé et mot d'éveil */, overlay: Boolean): BootAction` (pure) : `NOTHING` si `!autostart || !ready` ; `LAUNCH_TRAMPOLINE` si `overlay` ; sinon `NOTIFY_TAP_TO_START`.
- `BootReceiver.onReceive` (`BOOT_COMPLETED`, `MY_PACKAGE_REPLACED`) : `ready = pairedBrain(filesDir) != null && hasWakeword(filesDir)` ; `LAUNCH_TRAMPOLINE` → `startActivity(Intent(ctx, StartActivity::class.java).addFlags(FLAG_ACTIVITY_NEW_TASK or FLAG_ACTIVITY_NO_ANIMATION))` ; `NOTIFY_TAP_TO_START` → notification `ID_START` sur le canal `alerte` : « Jarvis : touchez pour démarrer l'écoute », `PendingIntent.getActivity(StartActivity)` (un toucher rend l'appli visible).
- `StartActivity` (transparente) : `onResume` → `ListenService.start(this)` (ou `send(ACTION_RESTART)` si l'intent le demande), annule la notification `ID_START`, `finish()`. `MY_PACKAGE_REPLACED` relance ainsi l'écoute après `scripts/android-install.sh`.

- [ ] Tests JVM : `BootDecisionTest.desactiveNeFaitRien`, `BootDecisionTest.pasPretNeFaitRien`, `BootDecisionTest.avecSuperpositionLanceLaTrampoline`, `BootDecisionTest.sansSuperpositionNotificationATapper`.
- [ ] `scripts/android-build.sh --test` vert ; commit `feat: démarrage automatique de l'écoute au démarrage du téléphone`.

### Task 9 : documentation et fiche d'essai (5.8)

**Files :** Create `docs/android.md`, `docs/essais/essai-5.7-android.md` ; Modify `README.md`, `docs/reseau.md` (lien vers `docs/android.md` dans « Appairer une oreille »), `docs/essais/README.md` (ligne du tableau), `docs/oreille.md` (une phrase : le code de session est commun avec Android)

- `docs/android.md` : ce qu'il faut (Pixel en mode développeur, débogage USB, même Wi-Fi que le PC, pare-feu : `jarvisctl doctor` rappelle les deux commandes `ufw`) ; construire (`scripts/android-build.sh`, JDK 21, premier passage en ligne) ; installer (`scripts/android-install.sh`, ce que font `-g`, la superposition et la liste blanche de batterie) ; première ouverture (autorisations, appairage par QR code ou adresse collée, enregistrement des 8 « Jarvis ») ; la notification et ses boutons ; démarrage automatique (pourquoi la superposition, sinon la notification à toucher) ; « micro bloqué » ; batterie (branché conseillé pour une oreille fixe ; verrous expliqués) ; repli : copier le mot d'éveil du PC (`--copy-wakeword`) ; journaux (`scripts/android-install.sh --logs`, et côté cerveau `jarvisctl logs | grep voix`) ; retirer l'oreille (`jarvis unpair "Pixel 4 XL"`, puis « Désappairer » dans l'appli) ; désinstaller (`adb uninstall fr.mawena.jarvis`).
- `docs/essais/essai-5.7-android.md` (style de `essai-4.12-reseau.md` : but, durée ~45 min + une nuit, avant de commencer, étapes à cocher avec « **Attendu :** », résultats à renvoyer) :
  1. construire et installer (`scripts/android-build.sh`, `scripts/android-install.sh`) ; l'appli s'ouvre ;
  2. appairage : `jarvis pair` sur le PC, scanner le QR code ; `jarvis ears` montre « Pixel 4 XL » (android) connectée ;
  3. enregistrer les 8 « Jarvis » à distance normale ;
  4. démarrer l'écoute : notification « À l'écoute · mode local », point vert du micro d'Android ;
  5. même pièce, écran allumé : « Jarvis, quelle heure est-il ? » → bip et réponse **sur le téléphone** ; relever dans `jarvisctl logs | grep voix` le score de la voix (et s'il est rejeté) ;
  6. écran éteint, téléphone posé : « Jarvis, suivant » (musique du PC) ;
  7. **depuis une autre pièce, écran éteint** (critère §9) : une demande, l'action sur le PC, la réponse vocale sur le téléphone ; noter le délai ressenti ;
  8. deux oreilles dans la même pièce que le PC : « Jarvis, suivant » → une seule exécution, réponse sur une seule oreille ;
  9. boutons de la notification sans ouvrir l'appli : « Couper l'écoute » (« Jarvis, quelle heure » ne fait rien ; « Jarvis, réactive l'écoute » marche), « Coupure totale » (le point vert disparaît), « Rouvrir le micro » ;
  10. PC endormi : `jarvisctl stop` (avec ton accord) → « Jarvis… » → « le PC est endormi » en moins de 2 s ; `jarvisctl start` → la notification repasse « À l'écoute » seule ;
  11. redémarrer le téléphone → après déverrouillage, l'écoute repart seule (ou la notification « touchez pour démarrer ») ;
  12. une nuit branché, puis une nuit sur batterie : pourcentage perdu par heure, l'oreille encore connectée le matin ;
  13. résultats : scores de voix, fausses détections de la journée, délais, ce qui a raté.
- `README.md` : l'oreille Android dans la liste des composants, lien vers `docs/android.md`.

- [ ] Relire chaque commande citée contre les scripts réels ; `scripts/check.sh` vert ; commit `docs: oreille Android et fiche d'essai 5.7`.

### Task 10 : relecture, corrections, essai de fumée (session principale) (5.8)

- [ ] Relecture de toute la branche (justesse, sécurité : jeton jamais journalisé dans `logcat`, fichiers privés, `PendingIntent` immuables, composants non exportés sauf le récepteur du boot) ; corrections dans des commits séparés, listées dans `ROADMAP.md` (5.8).
- [ ] Essai de fumée **sans micro ni son** par la session principale : `scripts/android-build.sh --test`, `scripts/android-install.sh --no-grant` (les permissions non accordées garantissent qu'aucun micro ne s'ouvre ; l'appli n'est ni appairée ni enrôlée, le service ne démarre pas), `adb logcat -d -s Jarvis JarvisRust AndroidRuntime` : « bibliothèque chargée », aucune ligne `FATAL` ; `adb shell dumpsys activity services fr.mawena.jarvis` : aucun service ; `adb shell getprop ro.build.version.sdk` = 33.
- [ ] `scripts/check.sh` vert ; cocher 5.8 ; fusion dans `develop` selon git flow quand Charles le demande.

### Task 11 : essai réel avec Charles (5.7)

- [ ] Charles suit [la fiche](../../essais/essai-5.7-android.md) ; ses résultats donnent les corrections (seuil du mot d'éveil, score de voix, latence écran éteint, batterie) ; cocher 5.7.

## Choix techniques (vérifiés le 2026-10-30)

| besoin | choix | pourquoi |
|---|---|---|
| partage du code | **machine d'états de l'oreille dans `jarvis-ear-core::session`**, entrées-sorties derrière `Capture` / `Player` / `Link` | ces trois traits existent déjà pour les tests ; une seule logique (élection, PC endormi, questions, annonces, coupures) testée une fois, et l'oreille du PC reste identique ; réécrire en Kotlin doublerait le code le plus délicat de la phase 4 |
| liaisons | **UniFFI 0.32.2** (2026-09-23), macros procédurales, `#[uniffi::export(foreign)]` pour `EarHost` (`with_foreign` est déprécié en 0.32) | prévu par la spec (§5.2) ; Kotlin et Swift (phase 10) depuis le même Rust ; pas de fichier UDL |
| génération | binaire `uniffi-bindgen` **dans `jarvis-ear-ffi` derrière `--features bindgen`**, en mode bibliothèque sur la `.so` Android (`generate <lib> --language kotlin`, `--library` n'est plus nécessaire depuis 0.31) | la forme documentée par UniFFI, sans alourdir `check.sh` (pas de clap/askama/goblin compilés à chaque vérification) ; la `.so` lue est celle qui est livrée, donc sommes de contrôle cohérentes |
| chargement | **JNA 5.17.0 `@aar`** (UniFFI exige ≥ 5.12) | la seule dépendance d'exécution du Kotlin généré ; `abiFilters` ne garde que `libjnidispatch.so` arm64 |
| son à travers la frontière | octets PCM i16 LE (`Vec<u8>` / `ByteArray`), blocs de 20 ms | `Vec<i16>` deviendrait `List<Short>` (une boîte par échantillon, 16 000 par seconde) ; 50 appels JNA par seconde sont négligeables ; 20 ms n'ajoutent rien à la latence du réveil |
| compilation | **cargo-ndk 4.1.2**, NDK 28.2.13676358, API 33, arm64-v8a seule ; `ring` (pas aws-lc-rs : absent de `jarvis-ear-core`) | `cargo ndk check -p jarvis-ear-core` passe déjà (18 s) ; aucune bibliothèque C++ partagée nécessaire |
| Gradle | **AGP 8.11.1 + Kotlin 2.2.20 + Gradle 8.14**, JDK 21 | tous trois déjà en cache (construction presque hors ligne) ; AGP 8.11 exige Gradle ≥ 8.13 et JDK ≥ 17, et accepte `compileSdk 36` ; le JDK 23 par défaut est évité pour AGP |
| interface Kotlin | `Activity` et vues du cadre, sans AndroidX | quatre écrans simples ; aucune bibliothèque de plus à télécharger ni à tenir à jour |
| QR code | **zxing-android-embedded 4.3.0** (une seule dépendance : zxing core 3.4.1), `IntentIntegrator` ; champ « coller l'adresse » toujours présent | libre, hors ligne, sans services Google, petite ; le scanner de Google Play (`play-services-code-scanner`, sans permission caméra) a été écarté : dépendance propriétaire et module téléchargé au premier usage |
| capture | `AudioRecord`, `VOICE_RECOGNITION`, 16 kHz mono 16 bits, repli 48 kHz rééchantillonné par le cœur (`Downmixer`) | réglage prévu pour la reconnaissance ; seul 44,1 kHz est garanti par Android, le repli couvre le reste ; même chemin que le PC |
| lecture | `AudioTrack` en flux, `USAGE_ASSISTANT`, focus `GAIN_TRANSIENT_MAY_DUCK`, numéro de génération | baisse la musique du téléphone pendant que Jarvis parle ; la génération empêche une vieille fin de lecture de remettre l'oreille au repos |
| écran éteint | service au premier plan type micro + `PARTIAL_WAKE_LOCK` + `WifiLock` `FULL_HIGH_PERF` + exemption d'optimisation de la batterie | Android l'autorise (spec §7.1) ; sans verrou Wi-Fi, l'économie d'énergie retarde les paquets au-delà de la fenêtre de 300 ms et du délai d'élection de 2 s ; coût en batterie assumé (oreille fixe branchée conseillée) |
| mDNS | **`MulticastLock`** tenu pendant l'écoute | Android jette le multicast sans lui ; l'adresse du QR code et la dernière adresse connue restent le premier choix, mDNS ne sert que si l'adresse du PC change |
| démarrage | trampoline `StartActivity` lancée au boot si « superposition » accordée, sinon notification à toucher ; « micro bloqué » détecté (3 s de zéros) | seul moyen d'avoir le micro après un boot sur Android 13 (règle « pendant l'utilisation ») ; jamais une oreille qui semble écouter sans rien entendre |
| mot d'éveil | **enregistré sur le téléphone** (8 prises, même construction rustpotter que le PC) ; repli : `android-install.sh --copy-wakeword` | le micro du Pixel n'est pas celui du PC ; un modèle fait sur le bon micro détecte mieux |
| vérification de la voix | inchangée, sur le cerveau ; score et oreille dans le journal | l'empreinte a été prise au micro du PC : l'essai dira si le Pixel passe le seuil ; sinon, phase suivante : seuil par oreille ou empreinte enregistrée depuis le téléphone |
| journaux | `tracing` → `logcat` (`JarvisRust`) par `__android_log_write`, sans nouvelle crate | `adb logcat` suffit pour l'essai ; le jeton est déjà masqué dans `Debug` |
| identifiant | `fr.mawena.jarvis` | à confirmer par Charles (modifiable tant que l'appli n'est pas installée ailleurs) |

**Ce qui ne change pas :** protocole v1 (plateforme « android » dans `Pair`, déjà prévue), cerveau (sauf deux lignes de journal), arbitre, baisse de la musique du PC (`duck = true` par défaut, aussi pour le téléphone, réglable dans `ears.toml`).

**Questions ouvertes pour Charles :**
- identifiant de l'appli `fr.mawena.jarvis` : d'accord ?
- démarrage seul : accorder « Afficher par-dessus d'autres applications » (le script le fait par `adb`), ou préférer toucher une notification après chaque redémarrage ?
- usage prévu : oreille fixe branchée (verrous sans souci) ou téléphone du quotidien (mesurer la batterie la première nuit) ?
- si le score de voix du Pixel est trop bas pendant l'essai : seuil propre à cette oreille, ou nouvelle empreinte enregistrée depuis le téléphone ?
