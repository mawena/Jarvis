# Feuille de route de Jarvis

Assistant vocal personnel en Rust : un cerveau sur le PC, des oreilles partout (PC, Pixel 4 XL, iPhone 14 Pro).

- Conception : [docs/superpowers/specs/2026-10-28-jarvis-design.md](docs/superpowers/specs/2026-10-28-jarvis-design.md)
- Plans détaillés : [docs/superpowers/plans/](docs/superpowers/plans/), un par groupe de phases, rédigé avant de commencer ces phases.
- Règle : cocher une tâche **dès** qu'elle est terminée.

## Phase 0 : fondations
*Plan : [phases 0 et 1](docs/superpowers/plans/2026-10-28-phases-0-1-fondations-comprehension.md), branche `feature/fondations`*

- [x] Initialiser le dépôt git avec git flow (`main` / `develop`)
- [x] Rédiger le document de conception
- [x] Rédiger la feuille de route et le plan des phases 0 et 1
- [x] Workspace Cargo et protocole partagé (tâche 1)
- [x] Configuration TOML, mode persistant, journaux (tâche 2)
- [x] Trait `Skill` et exécuteur de commandes (tâche 3)

## Phase 1 : compréhension et premières compétences (par le texte)
*Plan : [phases 0 et 1](docs/superpowers/plans/2026-10-28-phases-0-1-fondations-comprehension.md), branche `feature/comprehension-texte`*

- [x] Raccourcis rapides sans IA (« suivant », « pause », « lance X »…) (tâche 4)
- [x] Contrôle des médias via MPRIS et volume PipeWire (tâche 5)
- [x] Lancement et mise au premier plan des applications et jeux (tâche 6)
- [x] Compréhension par IA : Ollama (local) et Claude (en ligne) (tâche 7)
- [ ] Orchestrateur et switch de mode persistant (tâche 8)
- [ ] Service `jarvis-brain` et outil `jarvis` (tâche 9)
- [ ] Jeu de phrases de référence, installation d'Ollama, choix du modèle local, essai réel (tâche 10)

## Phase 2 : oreille du PC (écoute permanente)
- [ ] Rédiger le plan de la phase 2
- [ ] Capture du micro en 16 kHz mono (`cpal`)
- [ ] Mot d'éveil « Jarvis » (`rustpotter`) et outil d'enregistrement des échantillons de voix
- [ ] Détection de fin de parole (VAD) et délai maximum de 10 s
- [ ] Reconnaissance vocale Whisper (`whisper-rs`), choix `base` / `small` après mesure
- [ ] Baisse automatique de la musique après « Jarvis »
- [ ] Brancher la chaîne vocale sur `Brain::handle_text`
- [ ] Mesurer le taux de fausses détections sur une journée

## Phase 3 : voix de Jarvis
- [ ] Rédiger le plan de la phase 3
- [ ] Synthèse vocale Piper avec une voix française
- [ ] Sons de confirmation (réveil, succès, échec)
- [ ] Confirmation vocale des actions risquées (oui/non sur la même oreille, 8 s)
- [ ] Commandes « annule » et « répète »
- [ ] Personnalité : réponses brèves, ton majordome, nom d'adresse configurable

## Phase 4 : oreilles en réseau
- [ ] Rédiger le plan de la phase 4
- [ ] Protocole oreille ↔ cerveau versionné (`jarvis-protocol`)
- [ ] Cœur commun des oreilles (`jarvis-ear-core`) et binaire `jarvis-ear` séparé du cerveau
- [ ] Serveur WebSocket + TLS avec certificat épinglé
- [ ] Découverte du cerveau par mDNS, adresse manuelle en secours
- [ ] Appairage par code / QR code (`jarvis pair`)
- [ ] Arbitre : une seule oreille gagne par phrase (fenêtre de 300 ms)
- [ ] Réponse vocale jouée sur l'oreille gagnante
- [ ] Reconnexion automatique et message « le PC est endormi »

## Phase 5 : oreille Android (Pixel 4 XL)
- [ ] Rédiger le plan de la phase 5
- [ ] Liaisons UniFFI de `jarvis-ear-core` pour Kotlin, compilation Android
- [ ] Service au premier plan avec écoute continue, écran éteint
- [ ] Écran d'appairage (scan du QR code)
- [ ] Démarrage automatique et bouton de coupure totale du micro
- [ ] Essai de bout en bout depuis une autre pièce

## Phase 6 : recherche et vidéos
- [ ] Rédiger le plan de la phase 6
- [ ] SearXNG auto-hébergé dans Docker
- [ ] Compétence recherche avec résumé vocal court (IA du mode actif)
- [ ] « Ouvre les résultats » dans le navigateur du PC
- [ ] « Lance la vidéo de… » (`yt-dlp` ou SearXNG vidéos)

## Phase 7 : agent de code
- [ ] Rédiger le plan de la phase 7
- [ ] Liste des projets connus dans la configuration
- [ ] Lancement de Claude Code en arrière-plan (`claude -p`), sans confirmation
- [ ] Garde-fous : branche `feature/*` dédiée, rien de poussé ni fusionné, refus si modifications non enregistrées
- [ ] Suivi (« où en est la tâche ? ») et arrêt (« arrête la tâche »)
- [ ] Annonce de fin sur l'oreille la plus récemment active et notification de bureau

## Phase 8 : personnes et rôles
- [ ] Rédiger le plan de la phase 8
- [ ] Personnes, rattachement de chaque oreille à une personne
- [ ] Rôles et permissions (compétences autorisées, droit de confirmer), rôle `admin`
- [ ] Vérification des droits avant chaque outil
- [ ] Gestion par `jarvis person …` et `jarvis role …`

## Phase 9 : mise en service
- [ ] Rédiger le plan de la phase 9
- [ ] Services systemd utilisateur pour le cerveau et l'oreille du PC
- [ ] « Coupe le micro » / « réactive le micro » et coupure totale (`jarvis mute --hard`)
- [ ] Option `inhibit_sleep_on_ac`
- [ ] Mesure des critères de réussite (vitesse, fausses détections, doubles exécutions)
- [ ] Première version `release/1.0.0` via git flow

## Phase 10 : oreille iOS (iPhone 14 Pro)
- [ ] Rédiger le plan de la phase 10 (nécessite un Mac avec Xcode)
- [ ] Liaisons UniFFI pour Swift, compilation iOS
- [ ] Mode station (appli au premier plan, écoute continue)
- [ ] Raccourci Siri « Jarvis » et widget d'écran de verrouillage
- [ ] Appairage et essai de bout en bout

## Phase 11 : accès hors de la maison (plus tard)
- [ ] Rédiger le plan de la phase 11
- [ ] Accès sécurisé au cerveau depuis la 4G (par exemple un VPN de type Tailscale)

## Phase 12 : tâches de code natives (plus tard)
- [ ] Rédiger le plan de la phase 12
- [ ] Jarvis lance et interprète lui-même les tests d'un projet
- [ ] Jarvis corrige lui-même les cas simples avant de déléguer à l'agent
