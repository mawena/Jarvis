# Feuille de route de Jarvis

Assistant vocal personnel en Rust : un cerveau sur le PC, des oreilles partout (PC, Pixel 4 XL, iPhone 14 Pro).

- Conception : [docs/superpowers/specs/2026-10-28-jarvis-design.md](docs/superpowers/specs/2026-10-28-jarvis-design.md)
- Plans détaillés : [docs/superpowers/plans/](docs/superpowers/plans/), un par groupe de phases, rédigé avant de commencer ces phases.
- Règle : cocher une tâche **dès** qu'elle est terminée.
- Essais à faire en vrai : [docs/essais/](docs/essais/README.md), une fiche par essai.

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
- [x] 1.11 Aiguillage « commande ou conversation » en mode local : questions libres répondues en texte (12-14/14), commandes inchangées (82-84 %)
- [x] 1.12 Raccourcis élargis (« reprendre », « continue », « mets pause »…) et politesses ignorées (« Jarvis, tu peux… s'il te plaît »)
- [x] 1.13 Lecteurs inaccessibles (Firefox snap) ignorés, touches multimédia en repli, messages d'erreur courts
- [x] 1.14 Cerveau en service systemd utilisateur (`jarvis-brain.service`) et outil de gestion `jarvisctl` (install, uninstall, start, stop, restart, status, logs, doctor, set-api-key, update) — voir `docs/installation.md`
- [x] 1.15 Installer le service avec `jarvisctl install` sur le PC de Charles
- [x] 1.16 Essai réel : `jarvis say "suivant"` avec Spotify, `jarvis say "lance VS Code"`, mode en ligne avec clé API

## Phase 2 : oreille du PC (écoute permanente)
*Plan : [phase 2](docs/superpowers/plans/2026-10-28-phase-2-oreille-pc.md), branche `feature/oreille-pc` (fusionnée dans `develop`)*

- [x] 2.1 Rédiger le plan de la phase 2 ([plan](docs/superpowers/plans/2026-10-28-phase-2-oreille-pc.md))
- [x] 2.2 Installer les prérequis système : `sudo apt install libasound2-dev libclang-dev` (Charles)
- [x] 2.3 Socle : protocole d'écoute (`Wake`, `Listen`, `Heard`), crates `jarvis-ear-core` et `jarvis-ear` (tâche 1)
- [x] 2.4 Capture du micro en 16 kHz mono (`cpal`)
- [x] 2.5 Mot d'éveil « Jarvis » (`rustpotter`) et outil d'enregistrement des échantillons de voix
- [x] 2.6 Détection de fin de parole (VAD) et délai maximum de 10 s
- [x] 2.7 Reconnaissance vocale Whisper (`whisper-rs`), choix `base` / `small` après mesure — `small` retenu (fenêtre ajustée : 0,9-2 s ; `base` part en répétitions)
- [x] 2.8 Baisse automatique de la musique après « Jarvis »
- [x] 2.9 Brancher la chaîne vocale sur `Brain::handle_text`
- [x] 2.10 Commandes « coupe / réactive l'écoute » et coupure totale du micro pour l'oreille du PC (déplacées ici depuis la phase 9)
- [x] 2.11 Icône dans la zone de notification GNOME : statut (écoute, micro coupé, cerveau injoignable, mode, modèle)
- [x] 2.12 Menu de l'icône : couper / réactiver l'écoute, coupure totale, mode local / en ligne, statut détaillé, quitter
- [x] 2.13 Empreinte vocale : enregistrer la voix de Charles dans `jarvis-ear enroll` (quelques « Jarvis » + quelques phrases), ne garder que l'empreinte (aucun son)
- [x] 2.14 Vérification du locuteur par le cerveau : une phrase dont la voix n'est pas celle de Charles est ignorée sans rien exécuter
- [x] 2.16 Whisper accéléré : fenêtre ajustée à la phrase, attention flash, sans relances (`small` : ~6 s → ~1 s, outil `bench_stt`)
- [x] 2.17 Relecture finale de la phase 2 et corrections : vérification du locuteur fermée en cas d'échec (empreinte abîmée, modèle absent), phrases courtes non vérifiées limitées aux raccourcis, musique rétablie (quitter, surveillance 15 s, arrêt et redémarrage du cerveau), écoute coupée retenue, prises privées, requêtes bornées, `doctor` suit la configuration
- [ ] 2.15 Mesurer le taux de fausses détections sur une journée (Charles) — [fiche d'essai](docs/essais/essai-2.15-fausses-detections.md)
- [ ] 2.18 Essai réel : confirmer qu'une autre voix est ignorée (phrase rejetée à 0,34 le 2026-10-28 : autre voix ou Charles ?) (Charles) — [fiche d'essai](docs/essais/essai-2.18-autre-voix.md)

## Phase 3 : voix de Jarvis
*Plan : [phase 3](docs/superpowers/plans/2026-10-28-phase-3-voix.md), branche `feature/voix` (fusionnée dans `develop`)*

- [x] 3.1 Rédiger le plan de la phase 3 ([plan](docs/superpowers/plans/2026-10-28-phase-3-voix.md))
- [x] 3.2 Synthèse vocale Piper avec une voix française — voix `upmc-medium` « pierre » via sherpa-onnx + `piper-phonemize` (0,1-0,4 s), `scripts/download-models.sh voice`, lecture dans l'oreille
- [x] 3.3 Sons de confirmation (réveil, succès, échec)
- [x] 3.4 Confirmation vocale des actions risquées (oui/non sur la même oreille, 8 s) — veille, redémarrage, extinction ; seule la voix vérifiée de Charles confirme
- [x] 3.5 Commandes « annule » et « répète »
- [x] 3.6 Personnalité : réponses brèves, ton majordome, nom d'adresse configurable, option « bip seul » pour les réussites
- [x] 3.7 Relectures et corrections : bip et écho de Jarvis ignorés par l'oreille, lecture robuste, voix bornée (400 caractères, 8 s), faille de confirmation par une voix courte inconnue corrigée, alimentation lancée 4 s après la réponse
- [ ] 3.8 Essai réel avec Charles : réponse parlée, bips, coupure par « Jarvis », veille puis « non », « répète » (Charles) — [fiche d'essai](docs/essais/essai-3.8-voix.md)

## Phase 4 : oreilles en réseau
- [x] 4.1 Rédiger le plan de la phase 4 ([plan](docs/superpowers/plans/2026-10-29-phase-4-oreilles-reseau.md))
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
- [x] 6.1 Rédiger le plan de la phase 6 ([plan](docs/superpowers/plans/2026-10-29-phase-6-recherche.md))
*Plan : [phase 6](docs/superpowers/plans/2026-10-29-phase-6-recherche.md), branche `feature/recherche`*

- [x] 6.2 SearXNG auto-hébergé dans Docker — `jarvisctl searx up|down|status`, `127.0.0.1:8888` seulement, vérifié par `doctor`
- [x] 6.3 Compétence recherche avec résumé vocal court (IA du mode actif) — Ollama seul en mode local (repli sur un extrait au-delà de 10 s), Claude en ligne
- [x] 6.4 « Ouvre les résultats » dans le navigateur du PC (page de résultats, ou le n-ième lien)
- [x] 6.5 « Lance la vidéo de… » (`yt-dlp` ou SearXNG vidéos) — dans le navigateur, musique mise en pause
- [ ] 6.6 Relecture de la phase 6 et corrections (en cours)
- [ ] 6.7 Essai réel avec Charles (Charles) — [fiche d'essai](docs/essais/essai-6.7-recherche.md)

## Phase 7 : agent de code
- [x] 7.1 Rédiger le plan de la phase 7 ([plan](docs/superpowers/plans/2026-10-29-phase-7-agent-de-code.md))
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
- [x] 9.2 Service systemd utilisateur pour l'oreille du PC (celui du cerveau est fait en phase 1, `jarvisctl` à étendre) — fait en avance, le 2026-10-29 : `jarvis-ear.service` installé par `jarvisctl` (cibles `cerveau` / `oreille`, `logs --oreille`), pause du service pendant l'enrôlement, refus d'une deuxième oreille lancée à la main ; voir `docs/installation.md` et `docs/oreille.md`
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
