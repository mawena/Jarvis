# Jarvis : document de conception

*Date : 2026-10-28. Statut : proposition, en attente de validation par Charles.*

## 1. Intention

Charles veut un assistant vocal personnel « à la Jarvis », écrit en Rust, à qui il peut parler **presque tout le temps et de partout** : devant le PC, aux toilettes, dans une autre pièce. Il dit « Jarvis… » suivi d'une phrase libre en français, et Jarvis agit sur son PC Linux : passer la musique suivante, lancer une vidéo, une application ou un jeu, faire une recherche, ou confier un travail de code (par exemple « finis les tests du projet Jarvis ») à un agent quand il ne sait pas le faire lui-même.

L'idée centrale : **un cerveau sur le PC, des oreilles partout**. N'importe quel appareil capable d'entendre sa voix doit pouvoir servir de relais vers le PC. Concrètement, les oreilles de départ sont **le PC lui-même, un Pixel 4 XL (Android) et un iPhone 14 Pro**.

## 2. Objectifs, non-objectifs, anti-objectifs

### Objectifs (première version complète)
- Écoute permanente avec le mot d'éveil « Jarvis », sur le PC et sur les téléphones de Charles, via le Wi-Fi de la maison.
- Compréhension de phrases libres en français.
- Deux modes de compréhension, à choisir et à changer à tout moment (à la voix et dans la configuration) :
  - **local** : la compréhension se fait sur la machine (IA locale via Ollama) ;
  - **en ligne** : IA en ligne (Claude), plus intelligente mais payante et hors de la machine.
- Actions : contrôle des médias, lancement d'applications et de jeux, recherche web avec réponse vocale, délégation de tâches de code à un agent, commandes système de Jarvis.
- Réponse à voix haute **sur l'appareil qui a entendu** la demande.
- Confirmation vocale avant les actions risquées listées en 5.6. Les actions anodines et l'agent de code s'exécutent tout de suite.
- Architecture par compétences, pour ajouter facilement de nouvelles actions.
- **Rôles** : un seul utilisateur au départ (Charles, administrateur), mais un système de rôles qui permet plus tard de créer des rôles et de les attribuer à des personnes.

### Non-objectifs (plus tard, prévus dans la feuille de route)
- Accès depuis l'extérieur de la maison (4G).
- Réalisation native des tâches de code par Jarvis lui-même, sans agent externe.
- Reconnaissance de la personne à sa voix (au départ, la personne est déduite de l'appareil, voir 5.8).
- Windows et macOS côté cerveau.

### Anti-objectifs (ce qui serait un échec même si « ça marche »)
- Une action risquée de la liste 5.6 déclenchée sans confirmation à cause d'une fausse détection.
- Le son envoyé ou enregistré en dehors des moments qui suivent le mot d'éveil.
- En mode local, la voix ou le texte des phrases envoyés à une IA en ligne **pour la compréhension**. (Exception voulue par Charles : l'agent de code, voir 6.4.)
- Une même commande exécutée deux fois parce que deux oreilles l'ont entendue.
- Un assistant si lent pour « suivant » ou « pause » qu'il est plus rapide de se lever.

## 3. Contraintes et environnement constaté

Constaté sur la machine de Charles le 2026-10-28 :
- **PC portable** : Intel Core Ultra 7 155U (14 cœurs), GPU Intel intégré (pas de NVIDIA), 14 Go de RAM dont environ 9 Go déjà utilisés au quotidien, 65 Go de disque libre.
  → L'IA locale doit être **petite** (environ 3 milliards de paramètres) et la reconnaissance vocale doit rester légère.
- **Ubuntu 24.04, GNOME, session X11**, audio **PipeWire**, micro intégré détecté.
- Lecteurs : Spotify (snap), VLC, mpv, Rhythmbox, `playerctl` installé (MPRIS disponible).
- Applications installées en paquets classiques, snap et flatpak.
- Aucun lanceur de jeux trouvé (ni Steam, ni Lutris, ni Heroic).
- Rust 1.99, cargo, git, git-flow et Docker installés. Le CLI `claude` (Claude Code) est disponible dans `~/.local/bin/claude`.
- Ollama n'est pas installé.
- Téléphones : **Pixel 4 XL** (Android) et **iPhone 14 Pro** (iOS). Pas de Raspberry Pi.

Contraintes de Charles :
- Rust.
- **git flow** : `main` / `develop`, travail sur `feature/*`, `release/*`, `hotfix/*`. Commits signés uniquement de son nom, sans aucune ligne d'attribution.
- Suivi dans un fichier `ROADMAP.md` à la racine, organisé en **phases** contenant des **tâches à cocher**, cochées dès qu'elles sont terminées.

## 4. Ce qui a été décidé avec Charles

- Architecture cerveau sur le PC + oreilles sur n'importe quel appareil, qui relaient vers le PC.
- Les oreilles écoutent et transmettent. Tout le travail se fait sur le PC.
- Jarvis répond à voix haute sur l'appareil qui a entendu.
- Langue : français.
- Réseau Wi-Fi maison d'abord ; accès extérieur plus tard.
- Si plusieurs oreilles entendent la même phrase, une seule exécution.
- Deux modes de compréhension, local et en ligne, avec un switch.
- Liste des compétences de la section 6.
- Tâches de code : déléguées à un agent (Claude Code) quand Jarvis ne sait pas les faire. Le natif viendra plus tard.
- **L'agent de code fonctionne dans les deux modes et se lance directement, sans confirmation.**
- Oreilles : PC, Pixel 4 XL, iPhone 14 Pro.
- Charles est le seul utilisateur pour l'instant, mais il veut pouvoir créer des rôles et les attribuer à des personnes.
- Jarvis n'empêche pas la mise en veille par défaut ; les oreilles annoncent quand le PC est injoignable ; une option empêche la veille quand le portable est sur secteur.

## 5. Architecture (choix de l'auteur, modifiables)

### 5.1 Vue d'ensemble

```
 [Oreille PC]      [Oreille Android : Pixel 4 XL]      [Oreille iOS : iPhone 14 Pro]
       |  mot d'éveil sur l'appareil, puis envoi du son de la phrase
       v
 ===================== réseau local (WebSocket + TLS) =====================
       v
 [Cerveau sur le PC]
   ├─ Arbitre : une seule oreille gagne par phrase
   ├─ Reconnaissance vocale (voix → texte)
   ├─ Compréhension : raccourcis rapides, sinon IA (locale ou en ligne)
   ├─ Personnes et rôles : qui parle, qu'a-t-il le droit de faire
   ├─ Compétences : médias, applis, recherche, agent de code, système
   ├─ Garde-fou : confirmation des actions risquées
   └─ Synthèse vocale (texte → voix), renvoyée à l'oreille gagnante
```

### 5.2 Découpage du code (workspace Cargo)

- `jarvis-protocol` : messages échangés entre oreilles et cerveau (sérialisés, versionnés).
- `jarvis-ear-core` : cœur commun des oreilles en Rust : mot d'éveil, détection de fin de parole, connexion au cerveau, appairage. Compilé pour Linux, Android et iOS, et exposé aux applis mobiles via **UniFFI** (liaisons Kotlin et Swift générées).
- `jarvis-ear` : binaire oreille pour Linux (capture et lecture audio avec `cpal`).
- `jarvis-brain` : binaire cerveau (service permanent sur le PC).
- `jarvis-skills` : les compétences, chacune derrière un trait commun.
- `jarvis-cli` : outil en ligne de commande (`jarvis status`, `jarvis mode local|online`, `jarvis say "…"` pour tester sans micro, appairage d'oreilles, gestion des personnes et des rôles).
- `apps/android` : appli Android (Kotlin minimal autour de `jarvis-ear-core`).
- `apps/ios` : appli iOS (Swift minimal autour de `jarvis-ear-core`).

Le PC fait tourner **les deux** : le cerveau et une oreille locale (processus séparés, même protocole). Il n'y a donc pas de cas particulier « micro du PC ».

### 5.3 Chaîne vocale

1. **Oreille, écoute permanente** : capture du micro en 16 kHz mono. Détection du mot d'éveil « Jarvis » **sur l'oreille**, avec un moteur léger en Rust (`rustpotter`, entraîné sur des enregistrements de la voix de Charles). Tant que le mot d'éveil n'est pas détecté, aucun son ne quitte l'appareil et rien n'est enregistré.
2. **Après le mot d'éveil** : un petit son de confirmation. Le cerveau **baisse le volume des lecteurs du PC** (MPRIS/PipeWire) le temps de la phrase, puis le remet ; sans ça, la musique jouée par les haut-parleurs du portable couvrirait la voix. L'oreille envoie au cerveau le son de la phrase en continu, avec le score de détection et le niveau sonore. Elle s'arrête à la fin de la parole (détection d'activité vocale, par exemple Silero VAD) ou après un délai maximum de 10 s.
3. **Arbitre (cerveau)** : les réveils reçus dans une fenêtre d'environ 300 ms sont regroupés. L'oreille au meilleur score (niveau sonore et confiance) gagne, les autres reçoivent l'ordre d'arrêter.
4. **Reconnaissance vocale (cerveau)** : Whisper via `whisper-rs` (whisper.cpp), modèle `small` ou `base` en français, sur le CPU. **Locale dans les deux modes.**
5. **Compréhension** : voir 5.4.
6. **Réponse** : synthèse vocale **Piper** (locale, voix française), envoyée en audio à l'oreille gagnante, qui la joue. Locale dans les deux modes.

### 5.4 Compréhension et les deux modes

- **Raccourcis rapides** (sans IA), prévus **dès le départ** comme voie principale des commandes fréquentes, pas comme simple secours : « suivant », « précédent », « pause », « reprends », « plus fort », « moins fort », « annule », « passe en mode local / en ligne », et aussi **« lance / ouvre X »** avec recherche floue sur les applis connues. Ils répondent quasi instantanément, même en mode local sur ce portable.
- Pour tout le reste, une **IA avec appel d'outils** : chaque compétence déclare ses outils (nom, description, paramètres en schéma JSON) et l'IA choisit l'outil et ses arguments.
  - Mode **local** : Ollama avec un petit modèle compatible appel d'outils (point de départ : `qwen2.5:3b` ou `llama3.2:3b`, à départager par des tests sur un jeu de phrases).
  - Mode **en ligne** : API Claude (point de départ : `claude-haiku-5-5` pour la rapidité ; modèle configurable).
- Un trait `LlmBackend` unique cache la différence entre les deux. Le mode actif est persistant (conservé après redémarrage) et Jarvis annonce le changement à voix haute.
- **Garantie du mode local** : en mode local, la compréhension et les résumés n'utilisent que l'IA locale. La recherche web reste possible (seule la requête part vers le moteur de recherche, voir 6.3). L'agent de code reste disponible (voir 6.4).
- Si l'IA en ligne est injoignable, Jarvis le dit et propose de passer en local. Il ne bascule jamais tout seul.

### 5.5 Réseau entre oreilles et cerveau

- WebSocket sur le réseau local (`tokio-tungstenite`), audio en PCM 16 kHz brut (environ 32 Ko/s, sans problème en Wi-Fi). Compression Opus possible plus tard.
- Découverte automatique du cerveau par mDNS (`mdns-sd`), avec une adresse manuelle en secours.
- **Appairage** : `jarvis-cli pair` affiche un code (ou un QR code) ; l'oreille le saisit ou le scanne et reçoit un jeton secret. Le cerveau refuse les oreilles inconnues. Chiffrement TLS avec un certificat auto-signé épinglé à l'appairage.
- Une oreille qui perd le cerveau se reconnecte seule. Si le PC est injoignable (veille, éteint) et qu'on lui parle, elle répond elle-même « le PC est endormi » avec un message préenregistré.

### 5.6 Garde-fou (confirmations)

Chaque outil a un niveau de risque :
- **Direct** : médias, volume, ouvrir une appli, recherche, **agent de code** (choix de Charles).
- **Risqué** : fermer une application, commandes système (éteindre, redémarrer, mettre en veille), gestion des personnes et des rôles. Jarvis reformule et demande « je lance ? ». Une réponse oui/non est attendue **sur la même oreille**, dans les 8 secondes, sans redire « Jarvis ». Sans réponse, rien n'est fait.

### 5.7 Configuration, service, veille, journaux

- Configuration en TOML dans `~/.config/jarvis/config.toml` : mode, modèles, clé API (ou variable d'environnement), liste des projets connus pour l'agent, alias d'applications, voix, personnes et rôles.
- Le cerveau et l'oreille du PC tournent en **services systemd utilisateur**, démarrés à l'ouverture de session.
- **Veille** : Jarvis n'empêche pas la veille par défaut. Une option `inhibit_sleep_on_ac` empêche la veille quand le portable est branché (via `systemd-inhibit` / logind).
- Journaux avec `tracing`. Le texte des phrases est journalisé, **le son jamais** (sauf mode débogage activé explicitement).
- **« Jarvis, coupe le micro »** : l'oreille arrête d'envoyer du son au cerveau, mais continue d'écouter le mot d'éveil **sur l'appareil**, pour entendre « Jarvis, réactive le micro ». Rien ne sort de l'appareil pendant ce temps. Pour une coupure **totale** (micro réellement fermé), on passe par l'interface : bouton dans l'appli mobile, ou `jarvis-cli mute --hard` sur le PC.

### 5.7 bis Icône dans la zone de notification (PC) — demande de Charles
- Une icône Jarvis dans la zone de notification de GNOME (protocole StatusNotifierItem / AppIndicator, pris en charge par l'extension Ubuntu AppIndicators), lancée avec l'oreille du PC.
- **Statut visible d'un coup d'œil** (icône et infobulle) : à l'écoute, en train d'écouter une phrase, micro coupé, coupure totale, cerveau injoignable ; plus le mode actif (local / en ligne) et le modèle utilisé.
- **Menu** :
  - « Couper l'écoute » / « Réactiver l'écoute » : même effet que la commande vocale (le mot d'éveil reste écouté sur l'appareil) ;
  - « Coupure totale du micro » : le micro est réellement fermé, seul ce menu (ou le CLI) peut le rouvrir ;
  - « Mode local » / « Mode en ligne » (choix exclusif) ;
  - « Statut détaillé » : oreilles connectées, dernière phrase entendue et sa réponse, tâches d'agent en cours (à partir de la phase 7) ;
  - « Quitter l'oreille ».
- L'icône lit l'état via le socket du cerveau (requête `Status` enrichie) et se met à jour d'elle-même.

### 5.8 Personnes et rôles

- Le cerveau connaît des **personnes** ; chaque oreille appairée appartient à une personne. Au départ, il n'y a qu'une personne : Charles, avec le rôle **admin**.
- Un **rôle** est un nom plus une liste de permissions : compétences ou outils autorisés, et droit de confirmer les actions risquées. Le rôle `admin` a tout.
- Les commandes risquées et la compétence agent vérifient le rôle de la personne qui parle. Une oreille dont la personne n'a pas le droit reçoit « je ne peux pas faire ça pour vous ».
- Gestion via `jarvis-cli person …` et `jarvis-cli role …`, et dans le fichier de configuration. Pas de commande vocale pour ça au départ.
- Plus tard : reconnaissance de la personne à sa voix, pour les oreilles partagées comme celle du PC.

## 6. Compétences de la première version

### 6.1 Médias
Suivant, précédent, pause, lecture, volume (lecteur et système), « qu'est-ce qui passe ? ». Via **MPRIS sur D-Bus** (`zbus`), donc compatible avec Spotify, VLC, mpv, Rhythmbox et les navigateurs. Volume système via PipeWire (`wpctl`). Si plusieurs lecteurs sont ouverts, on cible le dernier actif.

### 6.2 Applications et jeux
« Lance Spotify », « ouvre VS Code », « lance [jeu] ». Jarvis indexe les fichiers `.desktop` (système, snap, flatpak, utilisateur) et fait une recherche floue sur le nom ; des alias sont possibles dans la configuration. Lancement via `gio launch` ou `gtk-launch`. Si l'appli tourne déjà, elle est mise au premier plan (X11 : `wmctrl`). Les jeux sont traités comme des applis ; un lanceur comme Steam s'ajoutera comme source s'il est installé.

### 6.3 Recherche
« Cherche… », « c'est quoi… ». Jarvis interroge un moteur de recherche, lit les meilleurs résultats, puis répond à voix haute par un **résumé court** produit par l'IA du mode actif. « Ouvre les résultats » les affiche dans le navigateur du PC.
- Moteur : **SearXNG auto-hébergé dans Docker** (déjà installé), pour que la recherche fonctionne de la même façon dans les deux modes sans dépendre d'une clé API.
- Vidéos : « lance la vidéo de… » cherche la vidéo (`yt-dlp "ytsearch1:…"` ou SearXNG catégorie vidéos) et l'ouvre dans le navigateur par défaut du PC.

### 6.4 Agent de code
« Finis les tests du projet Jarvis », « corrige le bug de connexion dans Mawena ».
- Disponible **dans les deux modes**, lancé **directement, sans confirmation** (choix de Charles). En mode local, c'est la seule compétence qui envoie du contenu (la consigne et le code du projet) à une IA en ligne.
- Jarvis retrouve le projet par son nom dans la liste de la configuration (nom → chemin). S'il ne le connaît pas, il le dit.
- Lancement de Claude Code en mode non interactif dans le dossier du projet (`claude -p "<consigne>" --output-format json`, avec des permissions définies dans la configuration). Le travail tourne en arrière-plan ; plusieurs tâches peuvent coexister. Changer de mode n'arrête pas les tâches en cours.
- Jarvis annonce le départ, puis la fin sur l'oreille la plus récemment active (« les tests de Jarvis passent, 3 fichiers modifiés » ou « l'agent a échoué : … »), avec en plus une notification de bureau.
- « Où en est la tâche ? » et « arrête la tâche » sont disponibles.
- Garde-fous techniques, puisqu'il n'y a pas de confirmation : l'agent travaille sur une branche `feature/*` créée pour l'occasion, jamais directement sur `develop` ou `main` ; il ne pousse rien et ne fusionne rien ; il refuse de démarrer si le projet a des modifications non enregistrées (pour ne rien écraser).
- Réservé aux rôles qui ont la permission `agent` (Charles par défaut).

### 6.5 Système de Jarvis
Changer de mode, « qu'est-ce que tu fais ? », « annule », couper/réactiver le micro, « répète ».

## 7. Les oreilles mobiles

### 7.1 Android (Pixel 4 XL)
- Appli Kotlin minimale autour de `jarvis-ear-core`, avec un **service au premier plan** de type micro (notification permanente « Jarvis écoute »). Android l'autorise et l'écoute continue fonctionne même écran éteint.
- Démarrage automatique au démarrage du téléphone, écran d'appairage (scan du QR code).
- **Notification permanente avec actions** (demande de Charles) : elle affiche le statut (à l'écoute, micro coupé, PC injoignable, mode local / en ligne) et propose les boutons « Couper l'écoute » / « Réactiver l'écoute » et « Coupure totale », sans ouvrir l'appli.
- Le Pixel 4 XL peut servir de téléphone au quotidien ou d'oreille fixe branchée dans une pièce : l'appli marche dans les deux cas.

### 7.2 iOS (iPhone 14 Pro)
- iOS **n'autorise pas** une appli tierce à écouter le micro en continu en arrière-plan pour un mot d'éveil. C'est une contrainte d'Apple, pas un choix.
- Deux façons de parler à Jarvis depuis l'iPhone :
  1. **Mode station** : l'appli ouverte au premier plan (par exemple posée et branchée) écoute « Jarvis » en continu, comme les autres oreilles.
  2. **Raccourci Siri** : « Dis Siri, Jarvis » (ou un widget sur l'écran de verrouillage, l'iPhone 14 Pro n'ayant pas de bouton Action) ouvre une écoute ponctuelle dans l'appli, qui envoie la phrase au cerveau et joue la réponse.
- Installation : compte développeur Apple gratuit (appli à réinstaller tous les 7 jours depuis Xcode) ou payant (99 $/an). **Xcode exige un Mac** : à prévoir pour cette phase.

## 8. Personnalité
Réponses **brèves**, polies, avec un léger ton « majordome » à la Jarvis. Le nom par lequel Jarvis s'adresse à Charles est configurable. Pour une action anodine réussie, un simple son peut remplacer la phrase (option).

## 9. Critères de réussite
- « Jarvis, suivant » : musique changée en **moins de 1 s** après la fin de la phrase, depuis l'oreille du PC.
- Commande libre en mode local (« cherche la météo de demain ») : réponse commencée en **moins de 6 s** ; en mode en ligne, **moins de 4 s**. « Lance VS Code » passe par les raccourcis : **moins de 2 s** dans les deux modes.
- Moins d'une fausse détection du mot d'éveil par jour en usage normal (musique, vidéos, conversations).
- Zéro double exécution avec deux oreilles dans la même pièce.
- Depuis le Pixel 4 XL dans une autre pièce, écran éteint : demande, action sur le PC, réponse vocale sur le téléphone.
- Une tâche confiée à l'agent est annoncée au départ et à la fin, même si Charles a changé de pièce.

## 10. Méthode de développement
- git flow initialisé dès la création du dépôt ; une branche `feature/*` par tâche ou groupe de tâches cohérent.
- `ROADMAP.md` à la racine : phases → tâches à cocher, cochées dès qu'elles sont terminées.
- Développement piloté par les tests là où c'est possible : compétences testées avec des doubles (faux D-Bus, faux lanceur), compréhension testée sur un **jeu de phrases de référence** en français (phrase → outil attendu) exécuté contre les deux modes, chaîne vocale testée avec des fichiers WAV enregistrés.
- `cargo fmt`, `cargo clippy -D warnings` et `cargo test` doivent passer avant chaque fusion.

## 11. Découpage en phases (esquisse pour la feuille de route)
0. Fondations : dépôt, git flow, workspace Cargo, configuration, journaux, `jarvis-cli say` pour tester sans micro.
1. Compréhension et premières compétences par le texte : raccourcis, deux modes et switch, médias, applis.
2. Oreille du PC : micro, mot d'éveil « Jarvis », baisse de la musique, fin de parole, Whisper. On peut parler au PC.
3. Voix de Jarvis : Piper, confirmations, personnalité.
4. Oreilles en réseau : protocole, découverte, appairage, TLS, arbitre, réponse sur l'oreille gagnante, PC endormi.
5. Oreille Android (Pixel 4 XL).
6. Recherche et vidéos : SearXNG, résumé vocal.
7. Agent de code : projets, Claude Code en arrière-plan, suivi, annonces.
8. Personnes et rôles.
9. Mise en service : systemd, coupure du micro, option anti-veille, mesures des critères de réussite.
10. Oreille iOS (iPhone 14 Pro) : mode station et raccourci Siri (nécessite un Mac).
11. (Plus tard) Accès hors de la maison.
12. (Plus tard) Tâches de code réalisées nativement par Jarvis.

## 12. Laissé au constructeur
- Choix exact des crates et des modèles (Whisper `base` ou `small`, modèle Ollama, voix Piper) après mesure sur la machine.
- Format exact des messages du protocole et stratégie de versionnement.
- Moteur de détection de fin de parole.
- Détails d'interface du CLI et des applis mobiles.
- Ordre précis des tâches dans chaque phase.
