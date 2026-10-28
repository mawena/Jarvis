# Installation de Jarvis

## Prérequis

- Ubuntu 24.04, GNOME sur X11, session systemd utilisateur
- Rust (cargo) et git
- [Ollama](https://ollama.com/download) avec le modèle local : `ollama pull llama3.2:3b`
- Outils système : `sudo apt install wmctrl xdotool wireplumber libglib2.0-bin`

## Installer

Depuis le dépôt :

```bash
cargo run --release -p jarvisctl -- install
```

Cela compile, copie `jarvis-brain`, `jarvis`, `jarvisctl` et `jarvis-ear` dans `~/.local/bin/`,
copie les bibliothèques de l'empreinte vocale (sherpa-onnx) dans `~/.local/lib/jarvis/`, installe l'unité `~/.config/systemd/user/jarvis-brain.service`, crée
`~/.config/jarvis/{env,config.toml,install.toml}` puis active et démarre le service.
Options : `--source <dépôt>`, `--no-build`.

Vérifiez que `~/.local/bin` est dans le `PATH`, puis lancez `jarvisctl doctor`.
Si un ancien `jarvis-brain` existe dans `~/.cargo/bin` : `cargo uninstall jarvis-brain jarvis-cli`.

## Commandes

| Commande | Effet |
| --- | --- |
| `jarvisctl status` | état du service, mode, Ollama, clé (code 3 si inactif) |
| `jarvisctl start` / `stop` / `restart` | pilote le service |
| `jarvisctl enable` / `disable` | démarrage automatique à la session |
| `jarvisctl logs [-f] [-n N]` | journaux du service |
| `jarvisctl doctor` | diagnostic avec conseils de correction |
| `jarvisctl update` | recompile depuis le dépôt source, réinstalle, redémarre |
| `jarvisctl set-api-key [CLÉ]` | enregistre la clé Claude |

## Clé API Claude

```bash
jarvisctl set-api-key      # saisie masquée
jarvisctl restart
```

La clé est stockée dans `~/.config/jarvis/env` (droits 0600) et jamais affichée.

## Désinstaller

```bash
jarvisctl uninstall           # garde la configuration
jarvisctl uninstall --purge   # efface aussi config, état et données (confirmation o/N, ou --yes)
```

Ollama et ses modèles ne sont jamais touchés.
