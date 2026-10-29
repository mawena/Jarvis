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
copie les bibliothèques de l'empreinte vocale (sherpa-onnx) dans `~/.local/lib/jarvis/`, installe
les deux unités systemd utilisateur `~/.config/systemd/user/jarvis-brain.service` (le cerveau) et
`~/.config/systemd/user/jarvis-ear.service` (l'oreille du PC), crée
`~/.config/jarvis/{env,config.toml,install.toml}` puis active et démarre les deux services.
Options : `--source <dépôt>`, `--no-build`.

Vérifiez que `~/.local/bin` est dans le `PATH`, puis lancez `jarvisctl doctor`.
Si un ancien `jarvis-brain` existe dans `~/.cargo/bin` : `cargo uninstall jarvis-brain jarvis-cli`.

## Les deux services

- **Le cerveau** (`jarvis-brain.service`) et **l'oreille du PC** (`jarvis-ear.service`)
  démarrent seuls à l'ouverture de la session graphique et s'arrêtent avec elle.
- L'oreille démarre après le cerveau, mais n'en dépend pas : sans cerveau, elle tourne quand
  même et son icône affiche « cerveau injoignable ».
- Tant que le mot d'éveil n'est pas enregistré (`jarvis-ear enroll`), le service de l'oreille
  est **sauté** (ni démarré ni en échec). Le premier enrôlement le démarre à la fin. Ce test
  porte sur le chemin par défaut `~/.local/share/jarvis/wakeword/jarvis.rpw`.
- **Quitter l'oreille** (menu de l'icône) arrête le service jusqu'à la prochaine session, ou
  jusqu'à `jarvisctl start oreille`. Une panne, elle, relance l'oreille au bout de 5 s.
- Pas de variable d'environnement à importer pour l'oreille : l'icône passe par le bus D-Bus de
  la session, le micro et le son par PipeWire, et le gestionnaire systemd de la session connaît
  déjà les deux (`DBUS_SESSION_BUS_ADDRESS`, `XDG_RUNTIME_DIR`). Vérification :
  `systemctl --user show-environment`.

## Commandes

Sans cible, `start`, `stop`, `restart`, `enable`, `disable` et `status` agissent sur les deux
services. Avec une cible, sur un seul : `cerveau` (ou `brain`), `oreille` (ou `ear`), par exemple
`jarvisctl restart oreille`.

| Commande | Effet |
| --- | --- |
| `jarvisctl status [cible]` | état du cerveau et de l'oreille, mode, Ollama, clé (code 3 si le cerveau est inactif ; avec `oreille` seule, si l'oreille l'est) |
| `jarvisctl start` / `stop` / `restart` `[cible]` | pilote les services |
| `jarvisctl enable` / `disable` `[cible]` | démarrage automatique à la session |
| `jarvisctl logs [-f] [-n N]` | journaux du cerveau |
| `jarvisctl logs --oreille [-f] [-n N]` | journaux de l'oreille (aussi `jarvisctl logs oreille`) |
| `jarvisctl doctor` | diagnostic avec conseils de correction |
| `jarvisctl update` | recompile depuis le dépôt source, réinstalle, redémarre les deux services |
| `jarvisctl set-api-key [CLÉ]` | enregistre la clé Claude |
| `jarvisctl searx up` / `down` / `status` | moteur de recherche SearXNG (voir plus bas) |

Les journaux sont aussi lisibles directement : `journalctl --user -u jarvis-brain -f` et
`journalctl --user -u jarvis-ear -f`.

Une installation antérieure à l'oreille en service n'a que l'unité du cerveau :
`jarvisctl update` ajoute et active celle de l'oreille. Quitte d'abord une oreille lancée à la
main dans un terminal, sinon deux oreilles tourneraient.

## Recherche (SearXNG)

La recherche (« cherche… », « lance la vidéo de… ») passe par un SearXNG local, dans Docker.
Elle est facultative : sans elle, le reste de Jarvis marche.

Il faut Docker et Compose (`sudo apt install docker.io docker-compose-v2`) et être dans le
groupe `docker`. Pour les vidéos, `yt-dlp` sert de secours (`sudo apt install yt-dlp`).

```bash
jarvisctl searx up       # écrit les fichiers, démarre le conteneur, attend qu'il réponde (60 s)
jarvisctl searx status   # état (code 3 s'il ne répond pas)
jarvisctl searx down     # arrête le conteneur, garde le cache et les réglages
```

- SearXNG écoute sur `http://127.0.0.1:8888`, sur le PC seulement.
- Les fichiers sont dans `~/.config/jarvis/searxng/` :
  - `compose.yml` est réécrit à chaque `up`, ne le modifiez pas ;
  - `settings.yml` est créé une seule fois, puis vos changements sont gardés. Appliquez-les avec
    `docker restart jarvis-searxng`. Gardez `json` dans `search.formats` : Jarvis en a besoin ;
  - `.env` contient le secret de SearXNG (droits 0600), créé une fois et jamais affiché.
- Docker relance le conteneur à son démarrage (`restart: unless-stopped`) : aucun service
  systemd en plus.
- L'image est épinglée (`docker.io/searxng/searxng:2026.10.28-c360868e5`). Le premier `up` la
  télécharge et affiche la progression.
- `jarvisctl doctor` vérifie Docker, le conteneur, `/healthz`, le format JSON, le port (lié à
  `127.0.0.1` seulement), `yt-dlp` et `xdg-open`. Ce ne sont que des avertissements.

## Clé API Claude

```bash
jarvisctl set-api-key      # saisie masquée
jarvisctl restart
```

La clé est stockée dans `~/.config/jarvis/env` (droits 0600) et jamais affichée.

## Désinstaller

```bash
jarvisctl uninstall           # arrête et retire les deux services, garde la configuration
jarvisctl uninstall --purge   # efface aussi config, état et données (confirmation o/N, ou --yes)
```

Ollama et ses modèles ne sont jamais touchés.
