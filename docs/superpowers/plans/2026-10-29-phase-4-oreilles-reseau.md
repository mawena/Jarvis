# Jarvis, phase 4 (oreilles en réseau) : plan d'implémentation

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal :** n'importe quelle oreille appairée parle au cerveau par le réseau de la maison (WebSocket + TLS, certificat épinglé), le trouve seule (mDNS, adresse manuelle en secours), s'appaire par code ou QR code (`jarvis pair`) ; quand plusieurs oreilles entendent « Jarvis », une seule gagne (fenêtre de 300 ms), les autres s'arrêtent ; la réponse est jouée sur la gagnante ; une confirmation se donne sur la même oreille ; une oreille qui perd le cerveau se reconnecte seule et répond « le PC est endormi » si on lui parle.

**Architecture :** le **cerveau** ouvre un serveur WebSocket sur TLS 1.3 (port 7341 par défaut), s'annonce en mDNS (`_jarvis._tcp.local.`) et tient un **registre des oreilles** (identifiant, nom, jeton haché). Un **hub** reçoit les réveils et le son **en continu**, les confie à l'**arbitre** (logique pure, horloge injectable), envoie `elected` à la gagnante et `stop` aux autres, puis passe la phrase de la gagnante à `ListenService` (inchangé sur le fond, mais qui connaît désormais l'oreille). Toute la logique réseau des oreilles (connexion, poignée de main, reconnexion, épinglage, découverte, appairage, phrase hors ligne) vit dans **`jarvis-ear-core`**, pour être reprise telle quelle par l'oreille Android en phase 5 (UniFFI, spec §7.1). Le binaire `jarvis-ear` ne garde que le micro, la lecture, l'icône et la colle. L'oreille du PC passe **par le même protocole** (TLS sur 127.0.0.1, spec §5.2) ; le socket Unix reste pour le CLI, l'enrôlement vocal et l'appairage automatique de l'oreille locale.

**Tech Stack :** tokio-tungstenite 0.30 (sans `connect`, TLS fourni par nous), tokio-rustls 0.26.6 / rustls 0.23.46 (fournisseur `ring`, TLS 1.3), rcgen 0.14.10 (certificat auto-signé, `ring`), mdns-sd 0.21.6, qrcode 0.14.1 (rendu Unicode au terminal), sha2 0.11, getrandom 0.4, subtle 2.6, futures-util 0.3. Existant : tokio, serde, base64, tracing, async-trait.

**Spec :** [docs/superpowers/specs/2026-10-28-jarvis-design.md](../specs/2026-10-28-jarvis-design.md) (§5.1, §5.2, §5.3, §5.5, §5.6, §7.1)

## Global Constraints

- **Une phrase, une exécution** (anti-objectif §2) : une seule phrase est traitée à la fois ; tout réveil arrivé après la fenêtre, ou pendant la capture d'une autre oreille, reçoit `stop` ; la fin de phrase d'une perdante est ignorée.
- **Aucun son hors des moments qui suivent « Jarvis »** : hors connexion, rien n'est envoyé ni gardé (pas de file d'attente audio : `send_audio` échoue tout de suite). Le son n'est jamais journalisé, les trames binaires non plus. La phrase « le PC est endormi » stockée sur l'oreille est la voix de Jarvis, jamais celle de Charles.
- **Secrets** : le jeton d'une oreille n'apparaît jamais dans les journaux (`Debug` masqué) ; côté cerveau seul son SHA-256 est stocké (`ears.toml`, 0600) et comparé en temps constant (`subtle`) ; côté oreille, fichier d'appairage 0600 dans un dossier 0700. Clé privée du certificat : 0600.
- **TLS** : TLS 1.3 seulement ; fournisseur **`ring` donné explicitement** (`ServerConfig::builder_with_provider` / `ClientConfig::builder_with_provider`), jamais le fournisseur par défaut du processus. `aws-lc-rs` est déjà présent par reqwest : avec deux fournisseurs, `builder()` panique. Pas d'autorité de certification ni de vérification du nom d'hôte : l'oreille vérifie le SHA-256 du certificat du cerveau (épinglé à l'appairage) et la signature de la poignée de main.
- **Rien n'est accepté avant un `hello` valide** (ou un `pair` pendant une fenêtre ouverte) : délai de 5 s pour TLS puis pour le premier message, sinon fermeture. Version inconnue → `rejected` explicite.
- **Bornes** : message reçu par le cerveau ≤ 1 Mio, trame audio ≤ 16 000 échantillons ; tampon d'une phrase ≤ 13 s (1 s de tête + 12 s), au-delà phrase abandonnée et son rétabli ; 16 sessions au plus, 8 poignées de main en cours au plus. Message reçu par l'oreille ≤ 8 Mio (voix de Jarvis).
- **La musique est toujours rétablie** : perte de la gagnante, abandon, arrêt de toutes les oreilles ou phrase trop longue → `restore` ; la surveillance de 15 s de `WpctlDucker` reste le dernier filet.
- **`jarvis-ear-core` reste portable** : ni `cpal`, ni `directories`, ni D-Bus dans la partie réseau ; les chemins sont fournis par l'appelant ; façade publique sans générique ni durée de vie (types possédés, `Arc<dyn Trait>`), pour UniFFI en phase 5 (pas d'annotation UniFFI dans cette phase).
- **Compatibilité** dans la v1 : nouveaux champs `#[serde(default)]`, types de message inconnus ignorés (`#[serde(other)] Unknown`), trames binaires de type inconnu ignorées. Tout changement incompatible fait passer `PROTOCOL_VERSION` à 2.
- Tests réseau sur `127.0.0.1` avec le port 0 ; un test mDNS réel est marqué `#[ignore]`.
- Les règles des phases précédentes restent valables (git flow, commits au nom de Charles sans attribution, `ROADMAP.md` coché dès qu'une tâche est validée, `scripts/check.sh` vert, jamais `cargo update` sans `-p` : l'ajout de dépendances ne doit faire apparaître que de nouvelles entrées dans `Cargo.lock`, à vérifier par `git diff Cargo.lock` ; ne pas redémarrer le service, ne pas ouvrir le micro ni jouer de son sans Charles ; le pare-feu (`sudo ufw …`) est l'affaire de Charles).

## Review Focus

1. **Zéro double exécution** : deux oreilles qui entendent la même phrase → une seule exécution ; la perdante reçoit `stop` et ne joue rien. → `deux_oreilles_une_seule_execution`, `la_perdante_recoit_stop` (tâche 8), tests de l'arbitre (tâche 3).
2. **Oreille inconnue ou jeton faux** : refusée avant le moindre audio ; certificat différent de l'empreinte épinglée → connexion refusée par l'oreille ; code d'appairage à usage unique, essais bornés, fenêtre limitée. → `oreille_inconnue_refusee_avant_tout_audio` (tâche 6), `empreinte_differente_refusee`, `trop_d_essais_ferme_la_fenetre` (tâche 2).
3. **Musique jamais laissée baissée** : gagnante déconnectée en pleine phrase, phrase abandonnée, phrase trop longue. → `musique_retablie_si_la_gagnante_se_deconnecte` (tâche 8), `phrase_trop_longue_abandonnee_et_son_retabli` (tâche 6).
4. **Confirmation sur la même oreille** : un « oui » venu d'une autre oreille n'exécute rien. → `oui_d_une_autre_oreille_n_execute_pas` (tâche 3), `confirmation_d_une_autre_oreille_ignoree` (tâche 8).
5. **PC endormi** : aucune file d'attente de son ; phrase hors ligne jouée en moins de 2 s, même si la connexion TCP semble encore ouverte. → `rien_n_est_envoye_ni_garde_hors_connexion`, `pas_d_election_en_deux_secondes_pc_endormi` (tâche 7).

---

Démarrage : `git flow feature start oreilles-reseau` depuis `develop`.

## Ordre et parallélisme

| Vague | Tâches (une par agent) | Dépend de |
|---|---|---|
| 0 | **Task 1** socle | — |
| A (4 agents en parallèle) | **Task 2** TLS, registre, fenêtre d'appairage · **Task 3** arbitre et état par oreille · **Task 4** cœur réseau de l'oreille · **Task 5** mDNS et CLI d'appairage | Task 1 |
| B (2 agents en parallèle) | **Task 6** serveur WebSocket et hub du cerveau · **Task 7** oreille Linux sur le réseau | 6 : 2, 3, 5 ; 7 : 2, 4, 5 |
| C (2 agents en parallèle) | **Task 8** tests de bout en bout · **Task 9** retrait de l'ancien chemin, `doctor`, documentation | 6, 7 |
| D | **Task 10** essai réel avec Charles | tout |

Chaque tâche ne touche **que** ses fichiers (liste « Files »). Task 1 crée tous les modules (fichiers et déclarations `pub mod`) avec leurs signatures et des corps `todo!("tâche N")`, pour que les vagues A et B compilent sans se marcher dessus. Fusion de chaque tâche dans `feature/oreilles-reseau` après relecture, `scripts/check.sh` vert.

### Task 1 : socle (protocole v1, dépendances, interfaces) (4.2)

**Files :** Modify `Cargo.toml` (workspace), `Cargo.lock` (nouvelles entrées seulement), `crates/jarvis-protocol/{Cargo.toml,src/lib.rs}` ; Create `crates/jarvis-protocol/src/{ear.rs,frame.rs,pairing_uri.rs,fingerprint.rs}` ; Modify `crates/jarvis-brain/src/{lib.rs,brain.rs,listen.rs,server.rs,speaker/mod.rs}` (passage mécanique de l'oreille) ; Create les squelettes `crates/jarvis-brain/src/{tls.rs,ears.rs,pairing.rs,arbiter.rs,discovery.rs,net/mod.rs,net/session.rs,net/hub.rs}`, `crates/jarvis-ear-core/src/net/{mod.rs,tls.rs,link.rs,backoff.rs,store.rs,discovery.rs,batch.rs}` ; Modify `crates/jarvis-ear-core/{Cargo.toml,src/lib.rs}`

**Interfaces :**
- Workspace : `tokio-tungstenite = { version = "0.30", default-features = false, features = ["handshake"] }`, `tokio-rustls = { version = "0.26", default-features = false, features = ["ring"] }`, `rustls = { version = "0.23", default-features = false, features = ["ring", "std"] }`, `rcgen = { version = "0.14", default-features = false, features = ["crypto", "ring"] }`, `mdns-sd = "0.21"`, `qrcode = { version = "0.14", default-features = false }`, `sha2 = "0.11"`, `getrandom = "0.4"`, `subtle = "2.6"`, `futures-util = { version = "0.3", default-features = false, features = ["sink", "std"] }`.
- `jarvis_protocol::ear` (nouveau, v1) :
  - `pub const PROTOCOL_VERSION: u16 = 1; pub const DEFAULT_PORT: u16 = 7341; pub const MDNS_SERVICE: &str = "_jarvis._tcp.local.";`
  - `#[serde(transparent)] pub struct EarId(String)` (`new`, `as_str`, `Display`) ; `EarId::unix_socket()` provisoire (« unix-socket », retiré en tâche 9).
  - `enum EarMessage` (oreille → cerveau, texte JSON, `tag = "type"`, `snake_case`) : `Hello { version: u16, ear_id: EarId, token: String, #[serde(default)] offline_phrase: Option<String> /* id de la phrase stockée */ }` · `Pair { version: u16, code: String, name: String, platform: String }` · `Wake { utterance: u32, score: f32, level_db: f32, muted: bool }` · `ListenStart { utterance: u32 }` (réponse à `AwaitReply`, sans mot d'éveil) · `ListenEnd { utterance: u32, muted: bool, no_speech: bool, #[serde(default)] cut: Option<(u32, u32)> }` · `Status` · `SetMode { mode: Mode }` · `#[serde(other)] Unknown`. `Debug` de `Hello` écrit à la main : jeton masqué.
  - `enum BrainMessage` (cerveau → oreille) : `Welcome { version: u16, brain_id: String, brain_name: String, status: StatusInfo }` · `Rejected { reason: RejectReason }` · `Paired { ear_id: EarId, token: String, brain_id: String, brain_name: String }` · `Elected { utterance: u32 }` · `Stop { utterance: u32 }` · `Response { utterance: Option<u32>, response: ControlResponse }` · `Status(StatusInfo)` · `OfflinePhrase { id: String, audio: SpeechAudio }` · `#[serde(other)] Unknown`.
  - `struct StatusInfo { mode: Mode, version: String, local_model: String, #[serde(default)] ears: Vec<EarSummary> }` ; `struct EarSummary { id: EarId, name: String, platform: String, connected: bool, local: bool }`.
  - `enum RejectReason { UnknownEar, BadToken, Version { supported: Vec<u16> }, PairingClosed, WrongCode { attempts_left: u8 } }`.
- `frame.rs` : trame binaire `[kind: u8 = 0x01 (MicAudio)][utterance: u32 LE][i16 LE mono 16 kHz…]` ; `encode_mic_frame(utterance, &[i16]) -> Vec<u8>`, `decode_frame(&[u8]) -> Result<Frame, FrameError>` (`Frame::MicAudio { utterance, samples }`, `Frame::Unknown(u8)`), `MAX_FRAME_SAMPLES = 16_000`.
- `fingerprint.rs` : `pub struct Fingerprint([u8; 32])` : `of_der(&[u8])` (SHA-256), `to_b64()` / `parse_b64()` (base64url sans remplissage), `short()` → `"A1B2-C3D4-E5F6-0718"` (64 premiers bits, à comparer à l'œil).
- `pairing_uri.rs` : `struct PairingUri { brain_id, hosts: Vec<IpAddr>, port, fingerprint: Fingerprint, code }` ; `to_string()` → `jarvis://pair?v=1&id=…&h=192.168.1.20,10.0.0.5&p=7341&fp=…&c=482913` (que de l'ASCII sûr, pas de nom) ; `parse(&str)`.
- Socket Unix (`ControlRequest` / `ControlResponse`, nouveaux variants) : `PairStart` → `Pairing { code, uri, fingerprint_short, expires_in_secs }` puis, sur la même connexion, `PairingDone { name }` ou `PairingEnded { reason }` ; `PairLocal { name }` → `PairedLocal { ear_id, token, port, fingerprint, brain_id, brain_name }` ; `ListEars` → `Ears { ears: Vec<EarSummary> }` ; `Unpair { ear: String /* nom ou id */ }` → `Reply`.
- Cerveau, passage mécanique (comportement inchangé, `_ear` inutilisé) : `ListenService::listen(&self, ear: &EarId, audio, muted, follow_up)`, `ListenService::restore()` (public), `Brain::answer_text(&self, ear: &EarId, text)`, `Brain::answer_fast(&self, ear, text)`, `Brain::answer_follow_up(&self, ear, text, spoken_at, voice)`, `SpeakerGate::check(&self, ear, audio)` ; `server.rs` passe `EarId::unix_socket()`.
- Squelettes : les signatures des tâches 2 à 7 ci-dessous, corps `todo!("tâche N")`, aucun appel depuis `main`.

- [ ] Tests (`jarvis-protocol`) : `version_du_protocole_vaut_1`, `hello_aller_retour`, `hello_debug_masque_le_jeton`, `message_inconnu_ignore`, `champ_ajoute_reste_lisible`, `response_contient_heard`, `trame_audio_aller_retour`, `trame_audio_impaire_refusee`, `trame_trop_grande_refusee`, `trame_type_inconnu`, `empreinte_sha256_et_forme_courte`, `uri_appairage_aller_retour`, `uri_appairage_incomplete_refusee`, `pair_start_et_pairing_serialises`.
- [ ] Implémenter ; mettre à jour les appels (et les tests existants) du cerveau avec une `EarId` de test ; tout compile.
- [ ] `git diff Cargo.lock` : seulement des ajouts ; `scripts/check.sh` vert ; commit `feat: protocole des oreilles en réseau (v1)`.

### Task 2 : TLS, identité du cerveau, registre des oreilles, fenêtre d'appairage (4.4, 4.6)

**Files :** `crates/jarvis-brain/src/{tls.rs,ears.rs,pairing.rs}`, `crates/jarvis-brain/Cargo.toml` (rustls, rcgen, sha2, getrandom, subtle) ; `crates/jarvis-ear-core/src/net/tls.rs`, `crates/jarvis-ear-core/Cargo.toml` (rustls, tokio-rustls ; rcgen en dev-dependency)

**Interfaces :**
- `tls.rs` (cerveau) : `struct BrainIdentity { pub cert_der: Vec<u8>, key_der: Vec<u8>, pub fingerprint: Fingerprint }` ; `load_or_create(dir: &Path) -> anyhow::Result<Self>` (ECDSA P-256, `CertificateParams::new(vec!["jarvis-brain".into(), "localhost".into()])`, `not_after` lointain, p. ex. `rcgen::date_time_ymd(2126, 1, 1)` ; fichiers `brain.crt.der` / `brain.key.der`, dossier 0700, clé 0600) ; `default_dir()` → `~/.local/share/jarvis/tls/` ; `server_config(&self) -> anyhow::Result<Arc<rustls::ServerConfig>>` (fournisseur `ring`, TLS 1.3, `with_no_client_auth().with_single_cert(..)`). `brain_id` : 16 octets aléatoires en hexadécimal, stockés à côté (`brain.id`).
- `ears.rs` : `struct EarRecord { id: EarId, name, platform, token_sha256: String, local: bool, duck: bool /* défaut true */, bias: f32 /* défaut 0 */, paired_at: u64 }` ; `struct EarRegistry` (fichier `~/.local/share/jarvis/ears.toml`, 0600, écriture atomique) : `load(path)`, `default_path()`, `authenticate(&EarId, token) -> Option<EarRecord>` (temps constant), `add(name, platform, local) -> io::Result<(EarRecord, String /* jeton en clair, rendu une seule fois */)>`, `add_local(name)` (remplace l'oreille locale du même nom : nouveau jeton), `remove(name_or_id) -> io::Result<Option<EarRecord>>`, `list()`, `get(&EarId)`. Jeton : 32 octets `getrandom`, base64url ; identifiant d'oreille : 16 octets hexadécimaux.
- `pairing.rs` : `struct PairingWindow` (horloge injectée) : `open(now, ttl /* 120 s */) -> (Self, code)` (6 chiffres, tirage sans biais), `try_code(&mut self, code, now) -> PairAttempt` (`Accepted` une seule fois, `WrongCode { attempts_left }`, `Closed`), 5 essais au plus, `is_open(now)`, `expires_at()`.
- `net/tls.rs` (oreille) : `pinned_client_config(&Fingerprint) -> Arc<rustls::ClientConfig>` (vérificateur `PinnedVerifier` : SHA-256 du certificat final == empreinte en temps constant ; signature TLS 1.3 par `rustls::crypto::verify_tls13_signature` avec `signature_verification_algorithms` du fournisseur `ring` ; TLS 1.2 refusé) ; `tofu_client_config() -> (Arc<ClientConfig>, Arc<Mutex<Option<Fingerprint>>>)` (accepte tout certificat et retient l'empreinte vue, pour l'appairage par code seul) ; `struct TlsConnector { config, server_name: "jarvis-brain" }` qui implémente `Connector` (tâche 4).

- [ ] Tests : `certificat_cree_puis_relu_meme_empreinte`, `fichiers_tls_prives`, `poignee_tls_avec_empreinte_epinglee` (deux bouts sur `tokio::io::duplex`), `empreinte_differente_refusee`, `tls12_refuse`, `tofu_capture_l_empreinte`, `jeton_correct_accepte`, `jeton_faux_refuse`, `oreille_inconnue_refusee`, `jeton_stocke_hache_jamais_en_clair`, `registre_relu_apres_redemarrage`, `registre_prive_0600`, `retrait_d_une_oreille`, `oreille_locale_reappairee_remplace_le_jeton`, `code_a_six_chiffres_usage_unique`, `fenetre_expiree_refuse`, `trop_d_essais_ferme_la_fenetre`.
- [ ] `scripts/check.sh` vert ; commit `feat: TLS épinglé, registre des oreilles et fenêtre d'appairage`.

### Task 3 : arbitre et état par oreille (4.7, §5.6)

**Files :** `crates/jarvis-brain/src/{arbiter.rs,brain.rs,listen.rs,speaker/mod.rs}`

**Interfaces :**
- `arbiter.rs` (pur, aucune entrée-sortie, `Instant` passé en argument) :
  - `struct WakeReport { ear: EarId, utterance: u32, score: f32, level_db: f32, muted: bool, bias: f32, duck: bool }` ; `pub fn rank(&WakeReport) -> f32` = `score + 0.5 × clamp((level_db + 60) / 60, 0, 1) + bias`.
  - `enum ArbiterAction { Duck, Elect { ear, utterance }, Stop { ear, utterance }, Restore }` ; `enum EndVerdict { Process, Wait, Ignore }`.
  - `Arbiter::new(window: Duration)` ; `wake(&mut self, WakeReport, now) -> Vec<ArbiterAction>` ; `deadline(&self) -> Option<Instant>` ; `tick(&mut self, now) -> Vec<ArbiterAction>` ; `ended(&mut self, &EarId, utterance) -> EndVerdict` ; `follow_up(&mut self, &EarId, utterance) -> bool` (capture sans mot d'éveil : acceptée si aucune phrase n'est en cours) ; `finished(&mut self, &EarId, utterance)` ; `disconnected(&mut self, &EarId) -> Vec<ArbiterAction>` ; `cancelled(&mut self, &EarId, utterance) -> Vec<ArbiterAction>`.
  - Règles : le premier réveil ouvre un groupe et la fenêtre (300 ms, à l'arrivée au cerveau) ; `Duck` au premier réveil d'une oreille `duck = true` du groupe, une seule fois ; à la fin de la fenêtre : une oreille ouverte bat une oreille coupée, puis le meilleur `rank`, puis la première arrivée ; `Elect` pour elle, `Stop` pour les autres ; tout réveil hors fenêtre tant que la phrase est en cours → `Stop` immédiat (« une phrase à la fois ») ; fin d'une perdante → `Ignore` ; fin avant décision → `Wait` (le hub garde l'audio et rappelle `ended` après `Elect`) ; gagnante déconnectée ou phrase abandonnée → `Restore`, groupe fermé.
- `brain.rs` : `Pending` gagne `ear: EarId`. Seule l'oreille qui a reçu la question peut la confirmer : `answer_follow_up` d'une autre oreille → « rien », attente effacée (par sécurité). Une nouvelle commande, de n'importe quelle oreille, efface l'attente (règle de la phase 3 conservée). `repeat()` ne repose la question que pour son oreille ; sinon la dernière phrase.
- `speaker/mod.rs` : la session de voix vérifiée devient `HashMap<EarId, Instant>` : une phrase courte n'est acceptée que dans la session **de la même oreille**.
- `listen.rs` : `ear` transmis à `Brain` et `SpeakerGate` ; le reste ne change pas.

- [ ] Tests de l'arbitre : `une_seule_oreille_elue_a_la_fin_de_la_fenetre`, `meilleur_score_gagne`, `oreille_plus_forte_gagne_a_confiance_egale`, `egalite_premiere_arrivee_gagne`, `biais_de_l_oreille_compte`, `oreille_coupee_perd_contre_oreille_ouverte`, `reveil_tardif_recoit_stop`, `reveil_pendant_une_phrase_recoit_stop`, `fin_de_la_perdante_ignoree`, `fin_avant_decision_attend`, `deconnexion_de_la_gagnante_retablit_le_son`, `baisse_du_son_une_seule_fois_par_phrase`, `pas_de_baisse_si_l_oreille_ne_le_demande_pas`, `reponse_sans_mot_d_eveil_acceptee_hors_phrase`.
- [ ] Tests du cerveau : `confirmation_sur_la_meme_oreille_execute`, `oui_d_une_autre_oreille_n_execute_pas`, `commande_d_une_autre_oreille_annule_l_attente`, `repete_d_une_autre_oreille_ne_repose_pas_la_question`, `session_vocale_propre_a_chaque_oreille` ; tous les tests des phases 2 et 3 restent verts.
- [ ] `scripts/check.sh` vert ; commit `feat: arbitre des oreilles, confirmation et session par oreille`.

### Task 4 : cœur réseau de l'oreille (`jarvis-ear-core`) (4.3, 4.9)

**Files :** `crates/jarvis-ear-core/src/{listener.rs,lib.rs}`, `crates/jarvis-ear-core/src/net/{mod.rs,link.rs,backoff.rs,store.rs,batch.rs}`, `crates/jarvis-ear-core/Cargo.toml` (tokio, tokio-tungstenite, futures-util, serde, toml, async-trait, jarvis-protocol, tracing, getrandom) ; `crates/jarvis-ear/src/app.rs` (seulement le motif `ListenEvent::Wake { score, .. }`)

**Interfaces :**
- `listener.rs` : `Listener::with_streaming(self, bool)`. En mode flux : `ListenEvent::Wake { score, level_db }` (niveau RMS en dBFS des 0,5 dernières secondes avant le réveil, plancher −90), puis `ListenEvent::Chunk(Vec<f32>)` (d'abord la seconde de tête, puis chaque trame VAD), puis `ListenEvent::Ended { cut: Option<(usize, usize)> }` (plage à retirer du flux, entre la tête et le pré-roulement) ou `NoSpeech`. Sans flux : comportement actuel (`Utterance`), avec `level_db` dans `Wake`.
- `batch.rs` : `AudioBatcher` regroupe les trames en paquets d'environ 100 ms (1 600 échantillons, i16).
- `net/mod.rs` : `#[async_trait] trait Connector: Send + Sync { async fn connect(&self, addr: SocketAddr) -> io::Result<Box<dyn AsyncStream>>; }` (TLS par `TlsConnector` de la tâche 2, TCP simple dans les tests) ; `#[async_trait] trait Discovery: Send + Sync { async fn find(&self, brain_id: &str, wait: Duration) -> Vec<SocketAddr>; }`.
- `store.rs` : `struct Pairing { brain_id, brain_name, ear_id: EarId, token, fingerprint: Fingerprint, addresses: Vec<SocketAddr>, manual: Option<String> }` ; `PairingStore::new(dir: PathBuf)` (dossier fourni par l'appelant, 0700) : `load() -> io::Result<Option<Pairing>>`, `save(&Pairing)` (`pairing.toml`, 0600, atomique), `remember_address(SocketAddr)`, `offline_phrase() -> Option<(String, SpeechAudio)>`, `save_offline_phrase(id, &SpeechAudio)` (`offline-phrase.bin` : fréquence u32 LE puis i16 LE).
- `backoff.rs` : `Backoff { min: 500 ms, max: 30 s, factor: 2, jitter: ±20 % }` : `next() -> Duration`, `reset()`. Après `Rejected { UnknownEar | BadToken }` : plafond porté à 5 min.
- `link.rs` : `EarLink::spawn(LinkConfig) -> (EarLink, mpsc::Receiver<LinkEvent>)` avec `LinkConfig { pairing: Pairing, store: PairingStore, connector: Arc<dyn Connector>, discovery: Option<Arc<dyn Discovery>>, heartbeat: Duration /* 5 s */, dead_after: Duration /* 15 s */ }` ; `send(&self, EarMessage) -> Result<(), LinkDown>` ; `send_audio(&self, utterance, &[i16]) -> Result<(), LinkDown>` (jamais mis en attente hors connexion) ; `nudge(&self)` (tentative immédiate) ; `state(&self) -> watch::Receiver<LinkState>` (`Connecting`, `Connected(StatusInfo)`, `Unreachable`, `Rejected(RejectReason)`). `enum LinkEvent { Connected(StatusInfo), Disconnected, Message(BrainMessage) }`. Adresses essayées dans l'ordre : adresse manuelle, dernière adresse connue, puis mDNS (2 s) ; une adresse trouvée est retenue. `OfflinePhrase` est enregistrée par le lien lui-même (non transmise). Ping toutes les 5 s ; rien reçu pendant 15 s → connexion tenue pour morte.
- `pub async fn pair_with_code(stream, code, name, platform) -> Result<Paired, PairError>` (échange `pair` → `paired` / `rejected`).
- `pub const ELECTION_TIMEOUT: Duration = 2 s` (utilisé par l'oreille, tâche 7).

- [ ] Tests (faux cerveau tokio-tungstenite sur TCP simple, délais de test courts) : `flux_moins_coupe_egale_la_phrase`, `niveau_sonore_du_reveil`, `fin_sans_parole_signalee`, `paquets_de_cent_millisecondes`, `hello_envoye_en_premier`, `welcome_publie_l_etat_connecte`, `jeton_refuse_pas_de_reconnexion_effrenee`, `reconnexion_apres_coupure`, `delai_croissant_plafonne`, `delai_remis_a_zero_apres_connexion`, `nudge_reconnecte_tout_de_suite`, `silence_du_cerveau_detecte`, `audio_jamais_mis_en_attente_hors_connexion`, `phrase_hors_ligne_enregistree`, `adresse_mdns_utilisee_si_la_derniere_echoue`, `appairage_par_code_rend_le_jeton`, `fichier_d_appairage_prive`.
- [ ] `scripts/check.sh` vert ; commit `feat: cœur réseau des oreilles (connexion, reconnexion, flux)`.

### Task 5 : découverte mDNS et appairage en ligne de commande (4.5, 4.6)

**Files :** `crates/jarvis-brain/src/discovery.rs`, `crates/jarvis-ear-core/src/net/discovery.rs`, `crates/jarvis-brain/Cargo.toml` et `crates/jarvis-ear-core/Cargo.toml` (mdns-sd) ; `crates/jarvis-cli/src/{main.rs,pair.rs}`, `crates/jarvis-cli/Cargo.toml` (qrcode)

**Interfaces :**
- Cerveau : `Advertiser::start(name, brain_id, port) -> anyhow::Result<Self>` (`ServiceDaemon::new()`, `ServiceInfo::new(MDNS_SERVICE, name, "<hostname>.local.", (), port, txt)` puis `enable_addr_auto()`, à confirmer à la compilation) ; TXT `id`, `v=1`, `pairing=0|1` ; `set_pairing(bool)` (nouvelle annonce) ; arrêt propre (`unregister`, `shutdown`). Fonction pure `txt_properties(..)`.
- Oreille : `MdnsDiscovery` implémente `Discovery` (`browse`, `ServiceEvent::ServiceResolved(ResolvedService)` : `get_property_val_str("id")`, `get_addresses_v4()`, `get_port()`) ; `find_pairing(wait) -> Vec<FoundBrain { brain_id, name, addrs }>` (annonces `pairing=1`) ; fonction pure `match_service(id_attendu, propriétés, adresses, port) -> Option<Vec<SocketAddr>>`.
- CLI : `jarvis pair` → `PairStart` ; affiche le code en gros, le QR code de l'URI (`QrCode::new(uri)?.render::<unicode::Dense1x2>()…build()`, à vérifier), l'URI à copier, l'empreinte courte et le compte à rebours ; attend `PairingDone` (« Oreille « Pixel 4 XL » appairée. ») ou `PairingEnded` ; Ctrl-C ferme la fenêtre (connexion fermée). `jarvis ears` → tableau nom / plateforme / connectée / locale. `jarvis unpair <nom|id>`. Codes de sortie comme aujourd'hui (2 si le cerveau ne tourne pas).

- [ ] Tests : `proprietes_txt_de_l_annonce`, `annonce_d_un_autre_cerveau_ignoree`, `annonce_sans_adresse_ignoree`, `annonce_puis_decouverte_sur_la_machine` (`#[ignore]`, vrai réseau), `qr_code_contient_l_uri`, `pair_affiche_code_et_attend_la_fin` (faux socket Unix dans un dossier temporaire, via `XDG_RUNTIME_DIR`), `pair_fenetre_expiree_code_1`, `ears_liste_les_oreilles`, `unpair_oreille_inconnue_code_1`.
- [ ] Vérifier à la main (Charles présent ou non, sans micro) que mdns-sd cohabite avec `avahi-daemon` (port 5353 partagé) ; noter le résultat dans le commit.
- [ ] `scripts/check.sh` vert ; commit `feat: découverte mDNS et jarvis pair`.

### Task 6 : serveur WebSocket et hub du cerveau (4.4, 4.7, 4.8)

**Files :** `crates/jarvis-brain/src/net/{mod.rs,session.rs,hub.rs}`, `crates/jarvis-brain/src/{server.rs,main.rs,config.rs,mode_store.rs,tts/mod.rs}`, `crates/jarvis-brain/Cargo.toml` (tokio-tungstenite, tokio-rustls, futures-util ; dev : jarvis-ear-core)

**Interfaces :**
- Config `[network] enabled = true`, `listen = "0.0.0.0:7341"`, `name = ""` (vide : nom de la machine), `arbiter_window_ms = 300`.
- `net::serve(listener: TcpListener, tls: Arc<ServerConfig>, hub: Hub) -> anyhow::Result<()>` : TLS (5 s), `accept_async_with_config` (`max_message_size` et `max_frame_size` 1 Mio), premier message en 5 s : `Hello` (version, puis `EarRegistry::authenticate`) → `Welcome` + `Status`, puis `OfflinePhrase` si l'id diffère ; sinon `Rejected` et fermeture. `Pair` : seulement si une fenêtre est ouverte → `Paired` et fermeture (l'oreille revient avec `Hello`). 16 sessions, 8 poignées de main au plus. Une nouvelle session de la même oreille remplace l'ancienne.
- `session.rs` : une tâche de lecture et une tâche d'écriture (canal borné 64) ; texte → `EarMessage`, binaire → `decode_frame` ; ping du cerveau et fermeture après 30 s sans rien reçu ; à la fermeture : `hub.disconnected(ear)`.
- `hub.rs` : `Hub` (tâche unique qui possède l'`Arbiter`, les tampons des phrases, les sessions, la fenêtre d'appairage) : `Duck` → `listen.wake()` ; `Restore` → `listen.restore()` ; `Elect` / `Stop` envoyés aux oreilles ; fin de la gagnante → tampon reconstitué (`cut` retiré, `no_speech` → vide), puis `listen.listen(ear, ..)` et `voice.speak(..)` dans une tâche, puis `Response { utterance, response }` à cette oreille seule (si elle est partie : journalisé, rien d'autre) ; `ListenStart` accepté si `arbiter.follow_up(..)`. Tampon > 13 s → phrase abandonnée, `Restore`, `Stop` à l'oreille. Changement de mode (CLI, icône ou voix) → `Status` diffusé (`ModeStore::subscribe() -> watch::Receiver<Mode>`). Phrase hors ligne : `persona.addressed("Le PC est endormi.")` synthétisée une fois au démarrage (`Voice::synthesize(text) -> Option<SpeechAudio>` ajouté à `tts/mod.rs`), id = SHA-256 court du texte et de la voix.
- `server.rs` (socket Unix) : `PairStart` (ouvre la fenêtre, `Advertiser::set_pairing(true)`, envoie `Pairing` puis attend la fin et envoie `PairingDone` / `PairingEnded` ; connexion fermée → fenêtre fermée), `PairLocal` (`EarRegistry::add_local`), `ListEars`, `Unpair` (retire et déconnecte). `Wake` / `Listen` restent sur le socket jusqu'à la tâche 9.
- `main.rs` : `BrainIdentity::load_or_create`, `EarRegistry::load`, `Advertiser::start`, `net::serve` lancé à côté du socket Unix ; réseau désactivé ou port pris → avertissement, le cerveau tourne quand même.

- [ ] Tests (vrai TLS de la tâche 2, `TlsConnector` de `jarvis-ear-core`, `FakeTranscriber`, `FakeDucker`, `FakeSynthesizer`) : `oreille_inconnue_refusee_avant_tout_audio`, `jeton_faux_refuse`, `version_inconnue_refusee`, `hello_trop_lent_ferme`, `appairage_par_code_puis_connexion`, `appairage_hors_fenetre_refuse`, `appairage_local_par_le_socket`, `reconnexion_remplace_l_ancienne_session`, `trame_trop_grande_ferme_la_session`, `phrase_trop_longue_abandonnee_et_son_retabli`, `reponse_envoyee_a_l_oreille_gagnante`, `changement_de_mode_diffuse`, `retrait_deconnecte_l_oreille`, `phrase_hors_ligne_envoyee_si_absente`, `phrase_hors_ligne_non_renvoyee_si_a_jour`, `reseau_desactive_le_cerveau_demarre`.
- [ ] `scripts/check.sh` vert ; commit `feat: serveur WebSocket TLS et hub des oreilles`.

### Task 7 : oreille Linux sur le réseau (4.3, 4.6, 4.8, 4.9)

**Files :** `crates/jarvis-ear/src/{app.rs,main.rs,client.rs,config.rs,tray.rs}` ; Create `crates/jarvis-ear/src/pair.rs` ; `crates/jarvis-ear/Cargo.toml`

**Interfaces :**
- `app.rs` : le trait `Brain` (requête / réponse) devient `trait Link` (`send`, `send_audio`, `nudge`, état), implémenté par `EarLink` et par un faux lien dans les tests ; événements du lien dans le `select!`. Écoute en mode flux (`with_streaming(true)`), `AudioBatcher`.
  - Réveil, lien connecté : bip, `Wake { utterance, score, level_db, muted }`, puis le flux ; délai d'élection `ELECTION_TIMEOUT`. `Stop` → capture abandonnée (`listener.reset()`), rien n'est joué. `Elected` → la capture continue. Fin → `ListenEnd`. `Response` d'une autre phrase que la dernière → ignorée. `AwaitReply` → après la lecture, `ListenStart` et flux (`follow_up`).
  - Réveil, lien coupé : pas de capture, phrase « le PC est endormi » jouée (sinon bip d'échec), `nudge()`. Lien perdu pendant la capture, ou pas d'élection en 2 s : capture abandonnée, même phrase, `nudge()`.
  - Quitter pendant une capture : `ListenEnd { no_speech: true }` envoyé au mieux (le cerveau rétablit aussi le son à la déconnexion).
  - Icône : `BrainLink::{Connected { mode, local_model }, Connecting, Unreachable, Refused}` (« PC injoignable », « appairage refusé : relancez jarvis-ear pair ») ; mode changé par `SetMode` sur le lien ; état poussé par le cerveau (plus d'interrogation toutes les 5 s).
- `main.rs` : `--profile <nom>` (dossier d'appairage et fichier de coupure séparés : deux oreilles possibles sur le même PC pour l'essai) ; sans appairage, l'oreille locale s'appaire seule par le socket Unix (`PairLocal { name: "PC" }`, nouvelles tentatives tant que le cerveau ne répond pas) et se connecte à `127.0.0.1:<port>` avec l'empreinte reçue ; `Rejected { UnknownEar | BadToken }` sur une oreille locale → nouvel appairage local automatique. `MdnsDiscovery` branchée pour les oreilles non locales. Dossier : `~/.local/share/jarvis/ear[-<profil>]/`.
- `pair.rs` : `jarvis-ear pair <uri>` (empreinte de l'URI, épinglée tout de suite) ; `jarvis-ear pair --code 123456 [--address hôte:port]` (cerveau en appairage trouvé par mDNS ou adresse donnée, TLS « au premier contact », affichage de l'empreinte courte, « Est-ce la même que sur le PC ? [o/N] », le code n'est envoyé qu'après « o »).
- `config.rs` : `brain_address: Option<String>`, `name: Option<String>` (`deny_unknown_fields` conservé).
- `client.rs` : socket Unix réduit à `PairLocal` et `EnrollVoice` (l'enrôlement reste sur le PC).

- [ ] Tests (faux lien, faux micro, faux lecteur) : `reveil_envoie_wake_puis_le_flux`, `stop_abandonne_la_capture_sans_reponse`, `reponse_d_une_ancienne_phrase_ignoree`, `reponse_jouee_apres_le_bip` (repris), `attente_de_reponse_ouvre_un_flux_follow_up`, `cerveau_injoignable_dit_pc_endormi`, `sans_phrase_enregistree_bip_d_echec`, `rien_n_est_envoye_ni_garde_hors_connexion`, `pas_d_election_en_deux_secondes_pc_endormi`, `coupure_pendant_la_capture`, `quitter_pendant_la_capture_previent_le_cerveau`, `appairage_local_automatique`, `jeton_refuse_reappairage_local`, `uri_d_appairage_enregistree`, `code_appairage_demande_confirmation_de_l_empreinte`, `profil_separe_les_fichiers`, `icone_pc_injoignable` ; tous les tests de l'oreille de la phase 3 adaptés et verts.
- [ ] `scripts/check.sh` vert ; commit `feat: oreille Linux sur le réseau (flux, arbitre, PC endormi)`.

### Task 8 : tests de bout en bout (4.7, 4.8, 4.9)

**Files :** Create `crates/jarvis-brain/tests/reseau.rs` ; Modify `crates/jarvis-brain/Cargo.toml` (dev-dependencies si besoin). Corrections éventuelles limitées à `crates/jarvis-brain/src/{net/*,arbiter.rs}` et `crates/jarvis-ear-core/src/net/*` (la tâche 9 tient `server.rs` et `jarvis-protocol`).

- Montage : vrai cerveau (`net::serve`, vrai TLS, `127.0.0.1:0`, fenêtre de 300 ms réelle) avec `FakeTranscriber`, `FakeSkill` (compte les exécutions, une compétence risquée), `FakeDucker`, `FakeSynthesizer` ; deux **fausses oreilles** = vrais `EarLink` de `jarvis-ear-core` qui envoient un `Wake` (scores et niveaux choisis) et un son scripté par trames, comme l'oreille réelle.
- Tests : `deux_oreilles_une_seule_execution`, `la_perdante_recoit_stop`, `la_plus_forte_gagne`, `reponse_jouee_sur_la_gagnante_seulement`, `reveil_tardif_d_une_oreille_wifi_recoit_stop` (second réveil 600 ms après), `confirmation_sur_la_meme_oreille`, `confirmation_d_une_autre_oreille_ignoree`, `musique_retablie_si_la_gagnante_se_deconnecte`, `oreille_inconnue_refusee`, `jeton_revoque_deconnecte`, `cerveau_coupe_puis_relance_reconnexion` (serveur arrêté puis relancé sur le même port : `Welcome` reçu de nouveau), `appairage_par_code_de_bout_en_bout` (`PairStart` par le socket Unix, `pair_with_code`, `Hello`).
- [ ] Tests verts trois fois de suite (pas d'instabilité de délais) ; `scripts/check.sh` vert ; commit `test: oreilles en réseau de bout en bout`.

### Task 9 : retrait de l'ancien chemin, diagnostic, documentation

**Files :** `crates/jarvis-protocol/src/lib.rs`, `crates/jarvis-brain/src/server.rs`, `crates/jarvisctl/src/ops.rs`, `docs/oreille.md`, `docs/installation.md` ; Create `docs/reseau.md`

- Retirer `ControlRequest::Wake` / `Listen` (et `EarId::unix_socket()`) : une oreille trop ancienne reçoit « requête invalide » ; plus aucun chemin ne contourne l'arbitre. Tests serde mis à jour (`wake_serialise`, `listen_aller_retour`, `ancien_listen_sans_follow_up` retirés ; `ancienne_requete_listen_refusee` ajouté).
- `jarvisctl doctor` : certificat présent et empreinte courte ; port 7341 en écoute (sur toutes les interfaces) ; oreilles appairées et connectées (`ListEars`) ; pare-feu `ufw` actif → rappel de la commande à lancer par Charles (`sudo ufw allow from 192.168.0.0/16 to any port 7341 proto tcp`) ; `avahi-daemon` présent ou non (information).
- Docs : appairer une oreille (`jarvis pair`, QR, code et vérification de l'empreinte), retirer une oreille, adresse manuelle, profil de test, que faire si le PC change d'adresse, réinitialiser le certificat (tout réappairer).
- Tests : `doctor_affiche_l_empreinte`, `doctor_signale_le_pare_feu`, `doctor_liste_les_oreilles`, `ancienne_requete_listen_refusee`.
- [ ] `scripts/check.sh` vert ; commit `feat: diagnostic réseau et retrait du chemin d'écoute par le socket`.

### Task 10 : essai réel avec Charles

- `jarvisctl update` ; `jarvisctl doctor` (pare-feu ouvert par Charles si besoin) ; `jarvis ears` : l'oreille « PC » est appairée seule et connectée.
- Seconde oreille : `jarvis-ear --profile essai` sur une autre machine Linux ou avec un autre micro (`device` dans `ear-essai.toml`) ; `jarvis pair` puis `jarvis-ear --profile essai pair '<uri>'`, puis un appairage par code avec vérification de l'empreinte.
- « Jarvis, suivant » entendu par les deux oreilles : une seule exécution, réponse sur une seule oreille, `stop` visible dans les journaux de l'autre ; noter la latence d'arrivée des réveils et ajuster `arbiter_window_ms` si besoin.
- « Jarvis, mets le PC en veille » puis « non » sur la même oreille ; « oui » dit uniquement près de l'autre oreille → rien.
- Cerveau arrêté (`jarvisctl stop`, avec l'accord de Charles) : « Jarvis… » → « le PC est endormi » en moins de 2 s ; cerveau relancé → reconnexion seule. Mise en veille réelle du PC puis réveil : reconnexion en moins de 30 s.
- `jarvis unpair essai` : l'oreille est déconnectée et refusée.
- Cocher 4.2 à 4.9 dans `ROADMAP.md` (4.1 en même temps que ce plan).

## Choix techniques (vérifiés le 2026-10-29)

| besoin | choix | pourquoi |
|---|---|---|
| WebSocket | **tokio-tungstenite 0.30** (`handshake` seul ; `accept_async_with_config`, `client_async_with_config(request, stream, Some(config))`, `Message::{Text(Utf8Bytes), Binary(Bytes), Ping, Pong, Close}`) | prévu par la spec (§5.5), sur tokio comme le reste ; TLS fourni par nous, donc pas de fonction `connect` |
| TLS | **tokio-rustls 0.26.6 / rustls 0.23.46** (déjà dans `Cargo.lock`), fournisseur **`ring`** explicite, TLS 1.3 | pur Rust ; `ring` se compile pour Android et iOS sans cmake (contrairement à aws-lc-rs) ; `builder_with_provider` évite la panique liée aux deux fournisseurs déjà liés (reqwest tire aws-lc-rs) |
| certificat | **rcgen 0.14.10** (`ring`, sans `pem`) : `CertificateParams::new(..)?`, `KeyPair::generate()`, `params.self_signed(&key)?`, `cert.der()`, `key.serialize_der()` (à confirmer à la compilation), `rcgen::date_time_ymd` | certificat auto-signé local, sans CA |
| épinglage | SHA-256 du certificat DER (`sha2 0.11`, déjà dans `Cargo.lock`), vérificateur `ServerCertVerifier` personnalisé (`ServerCertVerified::assertion()`, `rustls::crypto::verify_tls13_signature`) | identité stable sans nom d'hôte (adresse IP changeante en DHCP) |
| jetons | 32 octets `getrandom 0.4` (`getrandom::fill`), base64url ; SHA-256 stocké ; comparaison `subtle 2.6` | un fichier volé côté cerveau ne donne aucun jeton |
| découverte | **mdns-sd 0.21.6** (`ServiceDaemon`, `ServiceInfo::new(..)`, `browse` → `ServiceEvent::ServiceResolved(ResolvedService)` avec `get_addresses_v4()`, `get_port()`, `get_property_val_str()`) | pur Rust, sans runtime imposé, prévu par la spec ; sur Android il faudra un `MulticastLock` côté Kotlin (phase 5) |
| QR code | **qrcode 0.14.1** sans fonctionnalité par défaut, `render::unicode::Dense1x2` | lisible dans un terminal, sans image |

**Flux plutôt qu'envoi à la fin** (spec §5.3) : Whisper ne travaille pas en flux, mais envoyer pendant la parole (1) cache la fenêtre de 300 ms derrière la parole (la décision tombe bien avant la fin de phrase, qui exige 0,8 s de silence), (2) arrête vite les perdantes (batterie et Wi-Fi du téléphone), (3) retire du budget « suivant en moins d'1 s » le transfert de la phrase entière (jusqu'à 320 Ko en Wi-Fi faible), (4) permet de savoir en 2 s si le PC répond encore (`elected` / `stop` attendu). La coupe du silence entre « Jarvis » et la parole est gardée exacte grâce à `cut`.

**Binaire pour le micro, JSON pour le reste** : le micro envoie environ 10 paquets par seconde, d'où les trames binaires (ni base64 ni JSON à analyser). La voix de Jarvis, envoyée une fois par réponse, reste en `SpeechAudio` base64 dans `Response` : on reprend `ControlResponse::Heard` tel quel (prévu en phase 3), et le surcoût d'un tiers sur moins de 2 Mo est sans effet en réseau local. Une voix en trames binaires, voire phrase par phrase, pourra venir plus tard (v2).

**Oreille du PC par le réseau** : un seul chemin vers l'arbitre, donc aucune double exécution possible entre l'oreille du PC et une autre (spec §5.2 : « même protocole »). Son appairage passe par le socket Unix (0600, même utilisateur), qui vaut preuve sans code. Le socket reste pour `jarvis`, `jarvisctl`, l'enrôlement vocal et l'amorçage.

**Baisse du son pour toutes les oreilles par défaut** : un téléphone peut être dans la même pièce que le PC (oreille fixe, canapé) ; ailleurs, le coût est une baisse de quelques secondes, toujours rétablie. Réglage par oreille `duck = false` dans `ears.toml`.

**Appairage** : le QR code et l'URI portent l'empreinte complète, donc l'épinglage est sûr dès le premier contact (Android et iOS scanneront le QR). Le code seul (6 chiffres, 2 min, usage unique, 5 essais) se complète d'une comparaison à l'œil de l'empreinte courte (64 bits), comme SSH : un code court ne protège pas, à lui seul, d'un intermédiaire actif. Un PAKE (SPAKE2) l'éviterait, mais la crate n'existe qu'en préversion (`spake2 0.5.0-pre.0`) : écarté pour l'instant.

**Phrase « le PC est endormi »** : synthétisée par le cerveau avec la voix de Jarvis, envoyée à la connexion quand l'oreille n'a pas la bonne version (id = empreinte du texte et de la voix), stockée sur l'oreille ; sans elle, bip d'échec. Aucune synthèse n'est possible sur l'oreille (la voix est sur le PC), d'où l'enregistrement à l'avance.

**Questions ouvertes pour Charles** : seconde oreille pour l'essai (autre machine Linux ou micro USB) ; pare-feu `ufw` actif et port 7341 ; baisse du son pour toutes les oreilles ; empreinte vocale enregistrée sur le micro du PC, peut-être trop stricte pour un autre micro (enrôlement par oreille en phase 5 ?) ; appairage par code seul avec comparaison de l'empreinte, ou QR / URI seulement.
