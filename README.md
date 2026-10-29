# Jarvis

Assistant vocal personnel en Rust, en français. On lui parle (« Jarvis, mets la musique en
pause », « Jarvis, cherche la météo de demain à Lomé »), il comprend, agit sur le PC et répond
à voix haute. L'écoute, la reconnaissance de la parole, la vérification de la voix et la voix de
Jarvis tournent **entièrement sur le PC**. Seul le mode « en ligne », choisi exprès, envoie le
texte des phrases à Claude.

> **État :** en développement sur `develop`, utilisé au quotidien sur le PC de son auteur.
> `main` recevra la première version (`1.0.0`). Avancement détaillé :
> [ROADMAP.md](ROADMAP.md).

## Ce qu'il sait faire

| Domaine | Exemples de phrases (après « Jarvis… ») |
| --- | --- |
| Musique et vidéo | « pause », « reprends », « suivant », « plus fort », « qu'est-ce qui passe ? » |
| Applications et jeux | « lance VS Code », « ouvre Spotify » (ouverte ou mise au premier plan) |
| Recherche sur internet | « cherche les horaires du musée », « ouvre le deuxième résultat » |
| Vidéos | « lance la vidéo de Squeezie », « mets Daft Punk sur YouTube » |
| PC | « mets le PC en veille », « éteins le PC » (toujours confirmé par un « oui ») |
| Conversation | « annule », « répète », questions libres répondues par l'IA |
| Écoute | « coupe l'écoute », « réactive l'écoute », « passe en mode local / en ligne » |

Les commandes courantes sont reconnues **sans IA**, en une à deux secondes. Les autres phrases
passent par l'IA du mode choisi : **Ollama** sur le PC (mode local, par défaut) ou **Claude**
(mode en ligne).

Garde-fous :
- Jarvis reconnaît la voix de son propriétaire et **ignore les autres voix**.
- Les actions risquées (veille, redémarrage, extinction) attendent un « oui » de cette voix.
- La musique baisse pendant qu'on lui parle.

## Comment ça marche

```
 micro ──► oreille (jarvis-ear)                      cerveau (jarvis-brain, service systemd)
           mot d'éveil « Jarvis »  ── phrase ──►     Whisper : parole → texte
           fin de phrase, bips                        empreinte vocale : est-ce bien toi ?
           voix de Jarvis          ◄── réponse ──    raccourcis sans IA, sinon Ollama / Claude
           icône GNOME                                compétences : médias, applis, recherche…
```

- **L'oreille** écoute en permanence, mais n'envoie rien avant d'avoir entendu « Jarvis ». Elle
  joue les bips et la voix de Jarvis, et affiche une icône avec un menu (couper l'écoute, mode,
  statut). Aujourd'hui sur le PC ; téléphone Android et iPhone prévus.
- **Le cerveau** transcrit la phrase (Whisper), vérifie la voix (CAM++), comprend la demande
  et exécute la bonne compétence. Il répond avec une voix française (Piper).
- **Aucun son n'est enregistré sur le disque** : seule une empreinte de la voix est gardée,
  pour reconnaître son propriétaire.

## Installation rapide

Prévu pour **Ubuntu 24.04 avec GNOME sur X11** et une session systemd utilisateur.

```bash
# Prérequis
sudo apt install wmctrl xdotool wireplumber libglib2.0-bin libasound2-dev libclang-dev
ollama pull llama3.2:3b                      # IA locale (https://ollama.com/download)

# Modèles (Whisper, empreinte vocale, voix) dans ~/.local/share/jarvis/models/
scripts/download-models.sh all

# Compile, installe dans ~/.local/bin et démarre les services
cargo run --release -p jarvisctl -- install

# Apprendre « Jarvis » et ta voix (quelques prises au micro)
jarvis-ear enroll

jarvisctl doctor                             # vérifie tout, dit quoi corriger
```

Ensuite :
- **Mode en ligne**, facultatif : `jarvisctl set-api-key`.
- **Recherche sur internet**, facultative : `jarvisctl searx up`. Elle lance SearXNG dans
  Docker, joignable seulement depuis le PC.
- **Mises à jour** : `jarvisctl update`.

Le détail est dans [docs/installation.md](docs/installation.md).

## Utilisation

| Commande | Rôle |
| --- | --- |
| « Jarvis, … » | parler à Jarvis (l'oreille tourne en service) |
| `jarvis say "lance VS Code"` | même chose, par écrit |
| `jarvis mode local` / `jarvis mode en-ligne` | choisir l'IA |
| `jarvisctl status` | état du cerveau, de l'oreille, du mode, d'Ollama |
| `jarvisctl logs [-f] [--oreille]` | journaux |
| `jarvisctl start` / `stop` / `restart` `[cerveau\|oreille]` | piloter les services |
| `jarvisctl doctor` | diagnostic avec conseils |

Les réglages sont dans `~/.config/jarvis/config.toml` (cerveau) et `~/.config/jarvis/ear.toml`
(oreille). Toutes les clés sont facultatives.

## Ce qui quitte le PC

| | Mode local | Mode en ligne |
| --- | --- | --- |
| Son du micro | jamais | jamais |
| Texte des phrases | reste sur le PC | phrases non reconnues par les raccourcis → Claude |
| Recherche | SearXNG local, qui relaie la requête aux moteurs | pareil, résumé par Claude |

Le détail est dans [docs/recherche.md](docs/recherche.md) et
[docs/ia-locale.md](docs/ia-locale.md).

## Documentation

- [docs/installation.md](docs/installation.md) : installation, services, `jarvisctl`,
  désinstallation.
- [docs/oreille.md](docs/oreille.md) : l'oreille du PC, enrôlement, icône, commandes vocales,
  voix, réglages, dépannage (dont les mots mal compris).
- [docs/recherche.md](docs/recherche.md) : recherche sur internet et vidéos.
- [docs/ia-locale.md](docs/ia-locale.md) : choix du modèle local et de Whisper, mesures.
- [docs/essais/](docs/essais/README.md) : fiches d'essai en conditions réelles.
- [docs/superpowers/specs/](docs/superpowers/specs/) : conception d'ensemble.
- [docs/superpowers/plans/](docs/superpowers/plans/) : plan détaillé de chaque phase.

## Organisation du code

Espace de travail Cargo (Rust, édition 2024) :

| Crate | Rôle |
| --- | --- |
| `jarvis-protocol` | messages échangés entre oreilles, outils et cerveau |
| `jarvis-skills` | compétences : médias (MPRIS, PipeWire), applications, recherche, vidéos, alimentation… |
| `jarvis-brain` | le cerveau : service, compréhension, IA, Whisper, empreinte vocale, voix |
| `jarvis-ear-core` | cœur commun des oreilles : mot d'éveil, fin de phrase |
| `jarvis-ear` | l'oreille du PC : micro, haut-parleur, icône |
| `jarvis-cli` | l'outil `jarvis` |
| `jarvisctl` | installation, services, diagnostic, mises à jour |

## Développement

```bash
./scripts/check.sh            # format, clippy sans avertissement, tous les tests
cargo test -p jarvis-brain    # une seule crate
```

- **Branches :** git flow. `main` porte les versions publiées, `develop` le développement, et
  chaque phase a sa branche `feature/*`.
- **Suivi :**
  - [ROADMAP.md](ROADMAP.md) : phases et tâches, cochées dès qu'elles sont terminées ;
  - un plan détaillé par phase dans [docs/superpowers/plans/](docs/superpowers/plans/) ;
  - une fiche d'essai réel par fonction dans [docs/essais/](docs/essais/README.md).
- **Tests sans effet sur le PC :**
  - les tests n'ouvrent pas le micro, ne jouent aucun son et ne touchent ni au volume ni aux
    services ;
  - les essais sur le vrai matériel ou les vrais modèles sont marqués `#[ignore]`.
- **Dépendances :** ne jamais lancer `cargo update` sans `-p` : `half` doit rester en 2.4.1,
  figé dans `Cargo.lock`.
- **Langue :** le code est commenté en français, comme les messages et la documentation.
