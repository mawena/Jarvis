# Journal des versions

Toutes les évolutions notables de Jarvis. Format inspiré de
[Keep a Changelog](https://keepachangelog.com/fr/1.1.0/), versions en
[SemVer](https://semver.org/lang/fr/).

## [Non publié]

## [0.3.0] - 2026-11-10

Jarvis ne se lance plus tout seul au démarrage du PC par défaut. Les installations existantes
gardent leur état d'activation.

### Modifié

- `jarvisctl install` démarre LM Studio, le cerveau et l'oreille **sans les activer** au
  démarrage du PC ; le message indique `jarvisctl enable` pour le faire.
- `jarvis-lmstudio.service` n'est plus jamais activé : le cerveau le réclame (`Wants=`) et le
  démarre avec lui. `jarvisctl disable` du cerveau le désactive aussi (anciennes installations).
- `jarvisctl update` n'active plus une unité nouvelle d'office ; l'oreille nouvelle n'est
  activée que si le cerveau l'est.
- `jarvisctl doctor` : « non lancé au démarrage » devient une information neutre (ℹ) pour le
  cerveau et l'oreille, sans effet sur le code de sortie.

### Ajouté

- `jarvisctl install --autostart` : rétablit l'ancien comportement (cerveau et oreille activés
  et démarrés avec `enable --now`).

## [0.2.0] - 2026-11-09

L'IA locale passe à Qwen3.5 4B servi par LM Studio : 56 phrases sur 56 comprises au banc
(`docs/ia-locale.md`) ; `llama3.2:3b` en comprenait 26 sur 31 (84 %) à l'ancien banc, plus court.

### Modifié

- **IA locale : Qwen3.5 4B servi par LM Studio** (API compatible OpenAI) à la place de
  `llama3.2:3b` servi par Ollama. Nouveau réglage `[local] provider = "lmstudio" | "ollama"`
  (LM Studio par défaut, `http://127.0.0.1:1234`) ; le backend Ollama reste disponible. La
  réflexion de Qwen3.5 est coupée, `keep_alive` devient le `ttl` de LM Studio (déchargement
  automatique), le modèle est préchauffé au démarrage.
- Nouvelle unité `jarvis-lmstudio.service`, installée, activée et retirée par `jarvisctl`, qui
  démarre le serveur LM Studio sans interface graphique (`lms daemon up`, `lms server start`) ;
  le cerveau la réclame. `jarvisctl status` affiche son état.
- `jarvisctl status` et `doctor` vérifient LM Studio (ou Ollama selon le réglage).

## [0.1.0] - 2026-11-09

Première préversion. Jarvis est utilisable au quotidien sur le PC, mais plusieurs essais en
conditions réelles restent à faire (voir « Limites connues »). La version `1.0.0` est prévue
après la mise en service (phase 9) et ces essais.

### Ajouté

- **Cerveau** (`jarvis-brain`, service systemd utilisateur) : comprend une phrase, choisit la
  compétence et répond.
  - Raccourcis sans IA pour les commandes courantes (« suivant », « pause », « lance VS Code »…),
    en une à deux secondes.
  - IA locale par Ollama (`llama3.2:3b`, préchauffé au démarrage) ou en ligne par Claude, au choix
    (`jarvis mode local` / `en-ligne`, ou à la voix) ; questions libres répondues en texte.
- **Compétences** : médias (MPRIS, volume PipeWire, touches multimédia en repli), lancement et mise
  au premier plan des applications et des jeux, veille / redémarrage / extinction (toujours
  confirmés par un « oui »).
- **Oreille du PC** (`jarvis-ear`, service systemd) : mot d'éveil « Jarvis » (rustpotter), fin de
  phrase détectée, bips, musique baissée pendant qu'on parle, icône GNOME avec menu (couper /
  réactiver l'écoute, coupure totale du micro, mode, statut).
- **Reconnaissance de la parole** Whisper `small` sur le PC (~1 s par phrase), vocabulaire réglable
  pour les noms rares.
- **Empreinte vocale** (CAM++) : seule la voix du propriétaire est obéie ; aucun son n'est
  enregistré sur le disque.
- **Voix de Jarvis** : synthèse Piper en français (voix « pierre »), ton majordome, réponses
  brèves, « annule » et « répète ».
- **Oreilles en réseau** : WebSocket sur TLS 1.3 (port 7341) avec certificat épinglé, découverte
  mDNS, appairage par code ou QR code (`jarvis pair`, `jarvis ears`, `jarvis unpair`), une seule
  oreille retenue quand plusieurs entendent la même phrase, réponse jouée sur cette oreille,
  message « le PC est endormi » quand le cerveau est injoignable.
- **Oreille Android** (Pixel 4 XL, Android 13) : appli Kotlin autour du cœur Rust commun
  (UniFFI), écoute continue écran éteint dans un service au premier plan, appairage par QR code,
  mot d'éveil enregistré sur le téléphone, notification permanente avec « Couper / Réactiver
  l'écoute » et « Coupure totale », démarrage automatique. Construction et installation par
  `scripts/android-build.sh` et `scripts/android-install.sh`.
- **Recherche sur internet** par SearXNG auto-hébergé (`jarvisctl searx up`), résumé vocal court,
  « ouvre les résultats », « lance la vidéo de… ».
- **Agent de code** : « finis les tests du projet X » confie la tâche à Claude Code en
  arrière-plan, dans un bac à sable, sur une branche à part, sans rien pousser ni fusionner ;
  suivi, arrêt et annonce de fin.
- **Personnes et rôles** : annuaire `people.toml`, rôles (outils permis, droit de confirmer, droit
  d'utiliser l'agent), chaque oreille rattachée à une personne, droits vérifiés avant chaque
  outil ; gestion par `jarvis person …` et `jarvis role …`.
- **`jarvisctl`** : installation, mise à jour, services, journaux et diagnostic complet
  (`doctor`) : modèles, voix, oreilles, réseau, pare-feu, agent, personnes.
- Documentation (`docs/`) et fiches d'essai pas à pas (`docs/essais/`).

### Limites connues

- Essais réels encore à faire : fausses détections sur une journée, autre voix ignorée, voix de
  Jarvis, oreilles en réseau, oreille Android, recherche à la voix et en ligne, agent de code,
  personnes et rôles.
- L'agent de code refuse de démarrer tant que le bac à sable (`bwrap`) n'est pas autorisé par
  AppArmor sur Ubuntu 24.04 (`jarvisctl doctor` l'indique).
- Oreille iOS pas encore faite (phase 10). Accès hors de la maison non prévu dans cette version.
- Le cerveau et les oreilles doivent être mis à jour ensemble (`jarvisctl update`).

[0.2.0]: https://github.com/mawena/Jarvis/releases/tag/v0.2.0
[0.1.0]: https://github.com/mawena/Jarvis/releases/tag/v0.1.0
