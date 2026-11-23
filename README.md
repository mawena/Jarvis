# Jarvis

Assistant vocal personnel en Rust, en français. On lui parle (« Jarvis, mets la musique en
pause », « Jarvis, cherche la météo de demain à Lomé »), il comprend, agit sur le PC et répond
à voix haute. L'écoute, la reconnaissance de la parole, la vérification de la voix et la voix de
Jarvis tournent **entièrement sur le PC**. Seuls le mode « en ligne », choisi exprès, et le **cerveau
d'appoint** (phrases non sensibles, voir plus bas) envoient le texte de certaines phrases à
Claude.

> **État :** préversion `0.4.0` sur `main`, utilisée au quotidien sur le PC de son auteur ; le
> développement continue sur `develop`. La `1.0.0` viendra après la mise en service et les essais
> réels. Nouveautés : [CHANGELOG.md](CHANGELOG.md) ; avancement détaillé :
> [ROADMAP.md](ROADMAP.md).

## Ce qu'il sait faire

| Domaine | Exemples de phrases (après « Jarvis… ») |
| --- | --- |
| Musique et vidéo | « pause », « reprends », « suivant », « plus fort », « qu'est-ce qui passe ? » |
| Applications et jeux | « lance VS Code », « ouvre Spotify » (ouverte ou mise au premier plan) |
| Recherche sur internet | « cherche les horaires du musée », « ouvre le deuxième résultat » |
| Vidéos | « lance la vidéo de Squeezie », « mets Daft Punk sur YouTube » |
| PC | « mets le PC en veille », « éteins le PC » (toujours confirmé par un « oui ») |
| Agent de code | « finis les tests du projet Jarvis » : lance Claude Code en arrière-plan sur une branche à part ([docs/agent.md](docs/agent.md)) |
| Grands chantiers | « grand chantier sur CCS : ajoute X » : une fonction entière confiée à Claude sur le serveur, par la file de tâches de CCS ([docs/chantiers.md](docs/chantiers.md)) |
| Réseau CCS | « état du réseau », « quelles sessions tournent ? », « tue le premier sous-agent de la session GliGli », « lance une session sur le projet X » (arrêts et lancement confirmés par un « oui ») |
| Conversation | « annule », « répète », questions libres répondues par l'IA |
| Écoute | « coupe l'écoute », « réactive l'écoute », « passe en mode local / en ligne » |

Les commandes courantes sont reconnues **sans IA**, en une à deux secondes. Les autres phrases
passent par l'IA du mode choisi : **Qwen3.5 4B servi par LM Studio** sur le PC (mode local, par défaut ; Ollama reste possible) ou **Claude**
(mode en ligne).

Garde-fous :
- Jarvis reconnaît la voix de son propriétaire et **ignore les autres voix**.
- Les actions risquées (veille, redémarrage, extinction) attendent un « oui » de cette voix.
- La musique baisse pendant qu'on lui parle.
- L'icône du PC montre l'état, les oreilles connectées et les tâches d'agent en cours.
- En option, le PC ne se met pas en veille tant que le portable est branché
  (`inhibit_sleep_on_ac`, voir [docs/installation.md](docs/installation.md#veille-du-pc)).

## Comment ça marche

```
 micro ──► oreille (jarvis-ear)                      cerveau (jarvis-brain, service systemd)
           mot d'éveil « Jarvis »  ── phrase ──►     Whisper : parole → texte
           fin de phrase, bips                        empreinte vocale : est-ce bien toi ?
           voix de Jarvis          ◄── réponse ──    raccourcis sans IA, sinon LM Studio / Claude
           icône GNOME                                compétences : médias, applis, recherche…
```

- **L'oreille** écoute en permanence, mais n'envoie rien avant d'avoir entendu « Jarvis ». Elle
  joue les bips et la voix de Jarvis, et affiche une icône avec un menu (couper l'écoute, mode,
  statut). Sur le PC, et sur toute machine Linux de la maison appairée par `jarvis pair`
  (réseau local, TLS) ; téléphone Android (voir [docs/android.md](docs/android.md)), iPhone prévu. Si plusieurs oreilles entendent
  « Jarvis » (téléphones, PC, micros de la maison), la phrase n'est traitée qu'une fois et une
  seule répond : la plus sûre et la plus proche, sur une fenêtre réglable (`[network]
  arbiter_window_ms`, 300 ms par défaut) ; un appui sur le bouton Jarvis gagne toujours, sans
  attendre. Les autres s'arrêtent sans rien dire.
- **Le cerveau** transcrit la phrase (Whisper), vérifie la voix (CAM++), comprend la demande
  et exécute la bonne compétence. Il répond avec une voix française (Piper).
- **Aucun son n'est enregistré sur le disque** : seule une empreinte de la voix est gardée,
  pour reconnaître son propriétaire.

## Installation rapide

Prévu pour **Ubuntu 24.04 avec GNOME sur X11** et une session systemd utilisateur.

```bash
# Prérequis
sudo apt install wmctrl xdotool wireplumber libglib2.0-bin libasound2-dev libclang-dev
# IA locale : LM Studio (https://lmstudio.ai/download), puis télécharger Qwen3.5 4B (docs/ia-locale.md)
lms get lmstudio-community/Qwen3.5-4B-GGUF

# Modèles (Whisper, empreinte vocale, voix) dans ~/.local/share/jarvis/models/
scripts/download-models.sh all

# Compile, installe dans ~/.local/bin et démarre les services (sans les lancer au
# démarrage du PC ; ajoute --autostart pour qu'ils démarrent à chaque session)
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
- **Lancement au démarrage du PC** : désactivé par défaut. `jarvisctl enable` l'active
  (`jarvisctl disable` le retire) ; `jarvisctl install --autostart` l'active dès l'installation.

Le détail est dans [docs/installation.md](docs/installation.md).

## Utilisation

| Commande | Rôle |
| --- | --- |
| « Jarvis, … » | parler à Jarvis (l'oreille tourne en service) |
| `jarvis say "lance VS Code"` | même chose, par écrit |
| `jarvis mode local` / `jarvis mode en-ligne` | choisir l'IA |
| `jarvis pair` / `jarvis ears` / `jarvis unpair <nom ou id>` | appairer, lister, retirer une oreille |
| `jarvis person …` / `jarvis role …` | personnes et rôles (qui a le droit de demander quoi) |
| `jarvisctl status` | état du cerveau, de l'oreille, du mode, de LM Studio |
| `jarvisctl logs [-f] [--oreille]` | journaux |
| `jarvisctl start` / `stop` / `restart` `[cerveau\|oreille]` | piloter les services |
| `jarvisctl doctor` | diagnostic avec conseils |
| `jarvisctl mesures [--jours N]` | vitesse des réponses, fausses détections et doublons face aux objectifs |

Les réglages sont dans `~/.config/jarvis/config.toml` (cerveau) et `~/.config/jarvis/ear.toml`
(oreille). Toutes les clés sont facultatives.

## Cerveau Claude d'appoint

En mode local, chaque phrase qui passe par l'IA est aiguillée : **Claude** si elle n'est pas
sensible, **l'IA locale** sinon. Claude est appelé par la CLI `claude -p` avec le compte actif
de CCS ; sur une limite atteinte, `ccs meilleur` bascule sur le compte le plus disponible et
l'appel est refait une fois. Si Claude échoue (plus de quota sur aucun compte, réseau, délai,
CLI absente), la phrase part à l'IA locale **sans message d'erreur**, et Claude n'est plus
essayé pendant une pause (10 min sans quota, 1 min après un autre échec).

Par défaut `routage = "local"` : tout reste sur l'IA locale (choix de Charles, 23/11/2026, tant que
la règle n'est pas validée en vrai). Avec `routage = "auto"`, reste en local (dans le doute, local) :
- un mot d'une catégorie sensible : **fichiers personnels**, **secrets** (mots de passe, clés,
  jetons…), **santé**, **argent**, **famille**, **adresse** (et position) ;
- une phrase dite à **un micro de la maison** sans ordre explicite (« demande à Claude… ») ;
- une voix non reconnue, une phrase vide.

Le texte tapé (`jarvis say`) et les oreilles déclarées hors de la maison vont à Claude s'ils ne
sont pas sensibles. Une phrase sensible ne part jamais à Claude, même avec « demande à Claude ».

```toml
# ~/.config/jarvis/config.toml
[cerveau]
routage = "local"             # défaut ; "auto" : règle ci-dessus ; "claude" : toujours Claude
micros_maison = true          # voix des micros de la maison : local sauf ordre explicite
ordres_explicites = ["demande a claude", "avec claude"]   # sans accents
oreilles_hors_maison = []     # identifiants d'oreilles qui ne sont pas des micros de la maison
mots_sensibles = ["projet zebre"]   # mots ajoutés ; « medic* » : préfixe

[cerveau.categories]          # remplace la catégorie du même nom ; [] la désactive
argent = []
famille = ["ma soeur", "cousin*"]

[cerveau.claude]
commande = "claude"           # CLI Claude Code
modele = "haiku"
delai_s = 20                  # au-delà : repli local
ccs = "ccs"                   # "" : pas de bascule de compte
pause_sans_quota_min = 10
pause_apres_echec_s = 60
```

Les mots sont comparés mot pour mot, sans accents ni majuscules (« pin » ne touche pas
« sapin »). La liste complète des mots par défaut est dans `crates/jarvis-brain/src/config.rs`
(`SensitiveCategories`). Le mode « en ligne » (`jarvis mode en-ligne`, API Claude) n'est pas
concerné : tout y part à Claude.

## Piloter le réseau CCS

Jarvis voit et pilote tout le réseau CCS (machines, sessions Claude, sous-agents) par les
méthodes `jarvis.*` du pont CCS du serveur ([docs/protocole-ccs-jarvis.md](docs/protocole-ccs-jarvis.md)).
Il passe par le même chemin que les grands chantiers : la commande SSH limitée au pont
`scripts/jarvis-pont-ccs`, avec l'ordre `jarvis <méthode> <paramètres en hexadécimal>` ; le pont
parle au socket Unix du pont CCS de l'utilisateur (ticket HMAC tiré de sa clé). Aucun jeton
supplémentaire n'est gardé sur la machine de Jarvis.

```toml
[ccs]
enabled = true
# Vide : la commande de [chantier].
command = []
```

| Outil | Phrase reconnue sans IA | Méthode CCS |
|---|---|---|
| `ccs_etat` | « état du réseau », « état de CCS » | `jarvis.state` |
| `ccs_sessions` | « quelles sessions tournent ? », « liste les sessions » | `jarvis.sessions` |
| `ccs_sous_agents` | « quels sous-agents tournent dans la session GliGli ? » | `jarvis.subagents` |
| `ccs_arreter_sous_agent` | « tue le premier sous-agent de la session GliGli », « arrête le dernier sous-agent de GliGli » | `jarvis.stop_subagent` |
| `ccs_arreter_session` | « arrête la session GliGli », « ferme la session GliGli » | `jarvis.stop_session` |
| `ccs_lancer_session` | « lance une session sur le projet Jarvis (sur la machine lp-work) » | `jarvis.start_session` |

Les six outils sont aussi proposés à l'IA locale et à Claude d'appoint. Les deux arrêts et le
lancement sont risqués : pour un arrêt, Jarvis appelle d'abord CCS **sans** `confirme` (rien
n'est fait), lit la question rendue (« Arrêter le sous-agent 1 « Lire le code » de la session
GliGli sur lp-work ? ») et garde la cible exacte (`chat_id`, machine, `tool_id`). Seul un « oui »
de Charles au tour suivant exécute, sur cette cible ; toute autre réponse annule. Une cible
introuvable ou ambiguë est dite tout de suite (« Lequel ? »), sans question. Comme toute action
risquée, rien de tout cela ne marche en texte tapé.

Côté serveur : copier la nouvelle version de `scripts/jarvis-pont-ccs` à la place de l'ancienne
(même clé SSH) ; CCS doit avoir les méthodes `jarvis.*`.

## Personnes et rôles

Jarvis peut connaître plusieurs personnes, chacune avec un rôle (la liste de ce qu'elle peut
demander). Par défaut il n'y a que Charles, `admin`, et rien ne change pour lui. On peut par
exemple rattacher l'oreille d'un proche à un rôle `famille` qui ne permet que la musique : la
personne qui parle est déduite de l'oreille, sauf la voix reconnue de Charles qui vaut Charles
partout. Un invité ne peut jamais faire d'action risquée ni lancer l'agent. Tout se gère au
clavier (`jarvis person …`, `jarvis role …`). Le détail est dans
[docs/personnes.md](docs/personnes.md).

## Ce qui quitte le PC

| | Mode local | Mode en ligne |
| --- | --- | --- |
| Son du micro | jamais (une autre oreille l'envoie au PC après « Jarvis », chiffré, sur le réseau local) | pareil |
| Texte des phrases | reste sur le PC, sauf phrases non sensibles → Claude d'appoint (`[cerveau]`) | phrases non reconnues par les raccourcis → Claude |
| Recherche | SearXNG local, qui relaie la requête aux moteurs | pareil, résumé par Claude |

Le détail est dans [docs/recherche.md](docs/recherche.md) et
[docs/ia-locale.md](docs/ia-locale.md).

## Documentation

- [docs/installation.md](docs/installation.md) : installation, services, `jarvisctl`,
  désinstallation.
- [docs/oreille.md](docs/oreille.md) : l'oreille du PC, enrôlement, icône, commandes vocales,
  voix, réglages, dépannage (dont les mots mal compris).
- [docs/reseau.md](docs/reseau.md) : oreilles en réseau, appairage, pare-feu, certificat.
- [docs/android.md](docs/android.md) : l'oreille Android (construire, installer, appairer, batterie).
- [docs/personnes.md](docs/personnes.md) : personnes, rôles et droits, qui parle, `people.toml`.
- [docs/agent.md](docs/agent.md) : l'agent de code (Claude Code lancé par la voix, dans un worktree à part).
- [docs/chantiers.md](docs/chantiers.md) : les grands chantiers, confiés par la voix à CCS sur le serveur (pont SSH restreint).
- [docs/protocole-ccs-jarvis.md](docs/protocole-ccs-jarvis.md) : méthodes `jarvis.*` du pont CCS (pilotage du réseau).
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
| `jarvis-ear-core` | cœur commun des oreilles : mot d'éveil, fin de phrase, réseau (connexion, appairage, découverte) |
| `jarvis-ear` | l'oreille du PC : micro, haut-parleur, icône |
| `jarvis-ear-ffi` | façade UniFFI de l'oreille pour l'appli Android (`apps/android`) |
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

## Licence

Code publié sous licence MIT : voir [LICENSE](LICENSE). Les modèles téléchargés à l'installation (reconnaissance
de la parole, empreinte vocale, voix, IA locale) gardent leurs propres licences.
