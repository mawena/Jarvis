# IA locale et jeu de phrases de référence

## Modèle local : Qwen3.5 4B par LM Studio

Le mode local utilise **Qwen3.5 4B** (quantifié Q4_K_M) servi par [LM Studio](https://lmstudio.ai/download)
via son API compatible OpenAI (`POST /v1/chat/completions`, port 1234).

```bash
# Une fois LM Studio installé et lancé une fois (cela installe la commande `lms`)
~/.lmstudio/bin/lms get lmstudio-community/Qwen3.5-4B-GGUF   # ou depuis l'interface
~/.lmstudio/bin/lms ls                                       # identifiant exact du modèle
jarvisctl status                                             # vérifie le serveur et le modèle
```

Aucune étape de chargement : LM Studio charge le modèle à la première requête (chargement à la
demande) et le décharge de lui-même après `keep_alive` d'inactivité (le `ttl` de LM Studio). Le
cerveau préchauffe le modèle au démarrage (consignes et outils lus, rien généré).

Le serveur démarre à l'ouverture de session sans l'interface graphique : l'unité systemd du cerveau
lance `lms daemon up` puis `lms server start` avant le cerveau (échec ignoré si LM Studio est absent
ou déjà lancé). Si l'interface graphique de LM Studio est ouverte, c'est elle qui sert.

### Réglages

```toml
[local]
provider = "lmstudio"              # "lmstudio" (défaut) ou "ollama"
url = "http://127.0.0.1:1234"      # défaut selon le provider (Ollama : http://127.0.0.1:11434)
model = "qwen3.5-4b"               # identifiant donné par `lms ls` (Ollama : llama3.2:3b)
keep_alive = "30m"                 # déchargement après cette durée d'inactivité
```

`url` et `model` absents prennent la valeur du `provider`. Pour revenir à Ollama :
`provider = "ollama"` (puis `ollama pull llama3.2:3b`).

### Réflexion (thinking) coupée

@@REFLEXION@@

## Mesurer un modèle

Le jeu de phrases (`crates/jarvis-brain/tests/fixtures/phrases.toml`) est rejoué contre le modèle ;
le test échoue sous 80 % de réussite et affiche le temps médian.

```bash
# Local (LM Studio par défaut ; JARVIS_TEST_MODEL = identifiant du modèle)
cargo test -p jarvis-brain --test phrases -- --ignored local_jeu_de_phrases --nocapture
# Local (Ollama)
JARVIS_TEST_PROVIDER=ollama JARVIS_TEST_MODEL=llama3.2:3b cargo test -p jarvis-brain --test phrases -- --ignored local_jeu_de_phrases --nocapture

# En ligne (Claude)
ANTHROPIC_API_KEY=... cargo test -p jarvis-brain --test phrases -- --ignored claude_jeu_de_phrases --nocapture
```

## Résultats

@@RESULTATS@@

### Ancien banc (Ollama, 2026-10-28)

Mesuré sur le portable de Charles (Intel Core Ultra 7 155U, sans GPU dédié, 14 Go de RAM), Ollama 0.40.12, 31 phrases.

| Modèle | Réussite | Temps médian |
|--------|----------|--------------|
| `llama3.2:3b` | 26/31 (84 %) | 2,4 s |
| `qwen2.5:3b` | 20/31 (65 %) | 3,0 s |

Constats :
- `qwen2.5:3b` répond souvent par une phrase au lieu d'appeler l'outil (« Voulez-vous que je la fasse maintenant ? »).
- `llama3.2:3b` se trompe parfois d'outil (« reprends » → pause, « qu'est-ce qui passe » → suivant) ; ces phrases courtes passent de toute façon par les raccourcis rapides, sans IA.
- La toute première phrase après le démarrage du serveur dépassait le délai de 20 s : d'où le préchauffage au démarrage du cerveau et la durée de garde en mémoire (`keep_alive`).

## Reconnaissance vocale (Whisper)

La reconnaissance vocale est locale dans les deux modes : `whisper.cpp` via `whisper-rs`, langue `fr`, décodage glouton (`best_of: 1`), sans contexte ni horodatage, exécutée dans `spawn_blocking` (6 threads par défaut).

Modèles (quantifiés `q5_1`, dans `~/.local/share/jarvis/models/`) :

| Fichier | Taille | Usage |
|---------|--------|-------|
| `ggml-small-q5_1.bin` | 190 085 487 octets | défaut (meilleure précision) |
| `ggml-base-q5_1.bin` | 59 707 625 octets | rapide |

```bash
scripts/download-models.sh [small|base|all]   # idempotent, vérifie la taille
```

Configuration (`~/.config/jarvis/config.toml`) :

```toml
[stt]
model_path = "~/.local/share/jarvis/models/ggml-base-q5_1.bin"
threads = 6
```

`WhisperTranscriber::load` charge le modèle en mémoire (de 0,2 s à plus de 10 s selon le disque et la charge) : à appeler **une seule fois au démarrage**.

Mesure réelle (test ignoré) :

```bash
JARVIS_TEST_WAV=phrase.wav [JARVIS_TEST_MODEL=~/.local/share/jarvis/models/ggml-base-q5_1.bin] \
  cargo test --release -p jarvis-brain --lib whisper_transcrit -- --ignored --nocapture
```

### Résultats

Aucun synthétiseur français n'était installé (ni `espeak-ng`, ni `pico2wave` ; seul `ffmpeg` avec une voix anglaise `flite`), et le micro n'a pas été utilisé. Mesure faite sur une phrase **anglaise** de 4,2 s (« Jarvis, play the next song. What is the capital of Peru »), donc valable pour la durée, pas pour la précision en français. Portable de Charles, build `--release`, machine fortement chargée (charge moyenne 17 sur 14 cœurs, d'autres compilations en parallèle) : ces durées sont des majorants.

| Modèle | Transcription de 4,2 s d'audio |
|--------|--------------------------------|
| `base-q5_1` | 3,1 s |
| `small-q5_1` | 10,1 s |

À refaire sur machine au repos, avec une vraie phrase française (`arecord -f S16_LE -r 16000 -c 1 -d 4 phrase.wav`). Un build de test sans `--release` est 5 à 10 fois plus lent.
