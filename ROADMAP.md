# Feuille de route de Jarvis

Assistant vocal personnel en Rust : un cerveau sur le PC, des oreilles partout (PC, Pixel 4 XL, iPhone 14 Pro).

- Conception : [docs/superpowers/specs/2026-10-28-jarvis-design.md](docs/superpowers/specs/2026-10-28-jarvis-design.md)
- Plans détaillés : [docs/superpowers/plans/](docs/superpowers/plans/), un par groupe de phases, rédigé avant de commencer ces phases.
- Règle : cocher une tâche **dès** qu'elle est terminée.

## Phase 0 : fondations
*Plan : [phases 0 et 1](docs/superpowers/plans/2026-10-28-phases-0-1-fondations-comprehension.md), branche `feature/fondations`*

- [x] 0.1 Initialiser le dépôt git avec git flow (`main` / `develop`)
- [x] 0.2 Rédiger le document de conception
- [x] 0.3 Rédiger la feuille de route et le plan des phases 0 et 1
- [x] 0.4 Workspace Cargo et protocole partagé (tâche 1)
- [x] 0.5 Configuration TOML, mode persistant, journaux (tâche 2)
- [x] 0.6 Trait `Skill` et exécuteur de commandes (tâche 3)

## Phase 1 : compréhension et premières compétences (par le texte)
*Plan : [phases 0 et 1](docs/superpowers/plans/2026-10-28-phases-0-1-fondations-comprehension.md), branche `feature/comprehension-texte`*

- [x] 1.1 Raccourcis rapides sans IA (« suivant », « pause », « lance X »…) (tâche 4)
- [x] 1.2 Contrôle des médias via MPRIS et volume PipeWire (tâche 5)
- [x] 1.3 Lancement et mise au premier plan des applications et jeux (tâche 6)
- [x] 1.4 Compréhension par IA : Ollama (local) et Claude (en ligne) (tâche 7)
- [x] 1.5 Orchestrateur et switch de mode persistant (tâche 8)
- [x] 1.6 Service `jarvis-brain` et outil `jarvis` (tâche 9)
- [x] 1.7 Jeu de phrases de référence et tests de l'IA (tâche 10)
- [x] 1.8 Installer Ollama, comparer `qwen2.5:3b` et `llama3.2:3b`, choisir le modèle local (voir `docs/ia-locale.md`) → `llama3.2:3b`, 84 %, 2,4 s
- [x] 1.9 Préchauffer le modèle local au démarrage du cerveau et le garder chargé (première phrase : > 20 s → 2,9 s)
- [x] 1.10 Rendre l'IA locale robuste : appel d'outil écrit en texte reconnu, outils système retirés de l'IA (≈ 82-89 % sur le jeu de phrases)
- [ ] 1.11 Questions libres en mode local : `llama3.2:3b` appelle encore un outil au hasard (« capitale du Pérou » → musique) ; prévoir une étape d'aiguillage « commande ou conversation » avant l'appel d'outils
- [x] 1.12 Raccourcis élargis (« reprendre », « continue », « mets pause »…) et politesses ignorées (« Jarvis, tu peux… s'il te plaît »)
- [x] 1.13 Lecteurs inaccessibles (Firefox snap) ignorés, touches multimédia en repli, messages d'erreur courts
- [x] 1.14 Cerveau en service systemd utilisateur (`jarvis-brain.service`) et outil de gestion `jarvisctl` (install, uninstall, start, stop, restart, status, logs, doctor, set-api-key, update) — voir `docs/installation.md`
- [x] 1.15 Installer le service avec `jarvisctl install` sur le PC de Charles
- [x] 1.16 Essai réel : `jarvis say "suivant"` avec Spotify, `jarvis say "lance VS Code"`, mode en ligne avec clé API

## Phase 2 : oreille du PC (écoute permanente)
- [x] 2.1 Rédiger le plan de la phase 2 ([plan](docs/superpowers/plans/2026-10-28-phase-2-oreille-pc.md))
- [x] 2.2 Installer les prérequis système : `sudo apt install libasound2-dev libclang-dev` (Charles)
- [ ] 2.3 Socle : protocole d'écoute (`Wake`, `Listen`, `Heard`), crates `jarvis-ear-core` et `jarvis-ear` (tâche 1)
- [ ] 2.4 Capture du micro en 16 kHz mono (`cpal`)
- [ ] 2.5 Mot d'éveil « Jarvis » (`rustpotter`) et outil d'enregistrement des échantillons de voix
- [ ] 2.6 Détection de fin de parole (VAD) et délai maximum de 10 s
- [ ] 2.7 Reconnaissance vocale Whisper (`whisper-rs`), choix `base` / `small` après mesure
- [ ] 2.8 Baisse automatique de la musique après « Jarvis »
- [ ] 2.9 Brancher la chaîne vocale sur `Brain::handle_text`
- [ ] 2.10 Commandes « coupe / réactive l'écoute » et coupure totale du micro pour l'oreille du PC (déplacées ici depuis la phase 9)
- [ ] 2.11 Icône dans la zone de notification GNOME : statut (écoute, micro coupé, cerveau injoignable, mode, modèle)
- [ ] 2.12 Menu de l'icône : couper / réactiver l'écoute, coupure totale, mode local / en ligne, statut détaillé, quitter
- [ ] 2.13 Empreinte vocale : enregistrer la voix de Charles dans `jarvis-ear enroll` (quelques « Jarvis » + quelques phrases), ne garder que l'empreinte (aucun son)
- [ ] 2.14 Vérification du locuteur par le cerveau : une phrase dont la voix n'est pas celle de Charles est ignorée sans rien exécuter
- [ ] 2.15 Mesurer le taux de fausses détections sur une journée

## Phase 3 : voix de Jarvis
- [ ] 3.1 Rédiger le plan de la phase 3
- [ ] 3.2 Synthèse vocale Piper avec une voix française
- [ ] 3.3 Sons de confirmation (réveil, succès, échec)
- [ ] 3.4 Confirmation vocale des actions risquées (oui/non sur la même oreille, 8 s)
- [ ] 3.5 Commandes « annule » et « répète »
- [ ] 3.6 Personnalité : réponses brèves, ton majordome, nom d'adresse configurable

## Phase 4 : oreilles en réseau
- [ ] 4.1 Rédiger le plan de la phase 4
- [ ] 4.2 Protocole oreille ↔ cerveau versionné (`jarvis-protocol`)
- [ ] 4.3 Cœur commun des oreilles (`jarvis-ear-core`) et binaire `jarvis-ear` séparé du cerveau
- [ ] 4.4 Serveur WebSocket + TLS avec certificat épinglé
- [ ] 4.5 Découverte du cerveau par mDNS, adresse manuelle en secours
- [ ] 4.6 Appairage par code / QR code (`jarvis pair`)
- [ ] 4.7 Arbitre : une seule oreille gagne par phrase (fenêtre de 300 ms)
- [ ] 4.8 Réponse vocale jouée sur l'oreille gagnante
- [ ] 4.9 Reconnexion automatique et message « le PC est endormi »

## Phase 5 : oreille Android (Pixel 4 XL)
- [ ] 5.1 Rédiger le plan de la phase 5
- [ ] 5.2 Liaisons UniFFI de `jarvis-ear-core` pour Kotlin, compilation Android
- [ ] 5.3 Service au premier plan avec écoute continue, écran éteint
- [ ] 5.4 Écran d'appairage (scan du QR code)
- [ ] 5.5 Démarrage automatique
- [ ] 5.6 Notification permanente avec statut et boutons « Couper / Réactiver l'écoute » et « Coupure totale »
- [ ] 5.7 Essai de bout en bout depuis une autre pièce

## Phase 6 : recherche et vidéos
- [ ] 6.1 Rédiger le plan de la phase 6
- [ ] 6.2 SearXNG auto-hébergé dans Docker
- [ ] 6.3 Compétence recherche avec résumé vocal court (IA du mode actif)
- [ ] 6.4 « Ouvre les résultats » dans le navigateur du PC
- [ ] 6.5 « Lance la vidéo de… » (`yt-dlp` ou SearXNG vidéos)

## Phase 7 : agent de code
- [ ] 7.1 Rédiger le plan de la phase 7
- [ ] 7.2 Liste des projets connus dans la configuration
- [ ] 7.3 Lancement de Claude Code en arrière-plan (`claude -p`), sans confirmation
- [ ] 7.4 Garde-fous : branche `feature/*` dédiée, rien de poussé ni fusionné, refus si modifications non enregistrées
- [ ] 7.5 Suivi (« où en est la tâche ? ») et arrêt (« arrête la tâche »)
- [ ] 7.6 Annonce de fin sur l'oreille la plus récemment active et notification de bureau

## Phase 8 : personnes et rôles
- [ ] 8.1 Rédiger le plan de la phase 8
- [ ] 8.2 Personnes, rattachement de chaque oreille à une personne
- [ ] 8.3 Rôles et permissions (compétences autorisées, droit de confirmer), rôle `admin`
- [ ] 8.4 Vérification des droits avant chaque outil
- [ ] 8.5 Gestion par `jarvis person …` et `jarvis role …`

## Phase 9 : mise en service
- [ ] 9.1 Rédiger le plan de la phase 9
- [ ] 9.2 Service systemd utilisateur pour l'oreille du PC (celui du cerveau est fait en phase 1, `jarvisctl` à étendre)
- [ ] 9.3 Statut détaillé dans l'icône : oreilles connectées, tâches d'agent en cours
- [ ] 9.4 Option `inhibit_sleep_on_ac`
- [ ] 9.5 Mesure des critères de réussite (vitesse, fausses détections, doubles exécutions)
- [ ] 9.6 Première version `release/1.0.0` via git flow

## Phase 10 : oreille iOS (iPhone 14 Pro)
- [ ] 10.1 Rédiger le plan de la phase 10 (nécessite un Mac avec Xcode)
- [ ] 10.2 Liaisons UniFFI pour Swift, compilation iOS
- [ ] 10.3 Mode station (appli au premier plan, écoute continue)
- [ ] 10.4 Raccourci Siri « Jarvis » et widget d'écran de verrouillage
- [ ] 10.5 Appairage et essai de bout en bout

## Phase 11 : accès hors de la maison (plus tard)
- [ ] 11.1 Rédiger le plan de la phase 11
- [ ] 11.2 Accès sécurisé au cerveau depuis la 4G (par exemple un VPN de type Tailscale)

## Phase 12 : tâches de code natives (plus tard)
- [ ] 12.1 Rédiger le plan de la phase 12
- [ ] 12.2 Jarvis lance et interprète lui-même les tests d'un projet
- [ ] 12.3 Jarvis corrige lui-même les cas simples avant de déléguer à l'agent
