# Essais de Jarvis

Chaque fichier décrit un essai à faire en vrai, étape par étape : ce que tu fais, ce que tu
dois voir ou entendre, et ce qu'il faut me renvoyer. Coche les cases au fur et à mesure (dans
l'éditeur, remplace `[ ]` par `[x]`) et remplis la section « Résultats » en bas de chaque fichier.

## Dans quel ordre

| Ordre | Essai | Durée | Tâche |
| --- | --- | --- | --- |
| 1 | [Mise à jour](#avant-tout-essai--mise-à-jour) (ci-dessous) | 5 min | — |
| 2 | [La voix de Jarvis](essai-3.8-voix.md) | 15 min | 3.8 |
| 3 | [Une autre voix est ignorée](essai-2.18-autre-voix.md) | 10 min | 2.18 |
| 4 | [Fausses détections sur une journée](essai-2.15-fausses-detections.md) | une journée | 2.15 |

Quand un essai est fini, dis-le-moi (ou colle la section « Résultats ») : je coche la tâche
dans [ROADMAP.md](../../ROADMAP.md) et je corrige ce qui n'a pas marché.

## Avant tout essai : mise à jour

À faire une seule fois, avant le premier essai.

- [ ] **1. Quitte l'oreille lancée à la main** (l'ancienne façon) : icône Jarvis en haut de
  l'écran → **Quitter l'oreille**, ou `Ctrl+C` dans son terminal. Désormais l'oreille tourne en
  service, toute seule ; une oreille restée dans un terminal ferait doublon.
- [ ] **2. Mets à jour Jarvis.** Dans un terminal, depuis le dossier du projet :

  ```bash
  cd ~/Work/Mawena/Projects/Jarvis
  cargo run -q -p jarvisctl -- update
  ```

  Attendu : la compilation se termine, puis « Mise à jour terminée, cerveau et oreille
  redémarrés. ». La première fois, une ligne « Service jarvis-ear.service activé au
  démarrage. » s'affiche aussi : l'oreille devient un service qui démarre avec la session.
  Le cerveau et l'oreille redémarrent tout seuls : il n'y a plus rien à lancer.
- [ ] **3. Vérifie l'icône.** L'icône Jarvis apparaît en haut de l'écran, quelques secondes
  après la mise à jour. Si elle n'apparaît pas :

  ```bash
  jarvisctl status
  ```

  La ligne « Oreille » dit pourquoi : « non démarrée, aucun mot d'éveil enregistré » → lance
  `jarvis-ear enroll` (l'oreille démarre à la fin) ; « inactive » → `jarvisctl start oreille`.
- [ ] **4. Vérifie l'installation.**

  ```bash
  jarvisctl doctor
  ```

  Attendu : une coche `✓` devant chaque ligne, en particulier « Voix de Jarvis »,
  « Modèle Whisper », « Modèle d'empreinte vocale », « Mot d'éveil enregistré »,
  « Empreinte vocale enregistrée » et « Oreille active ». Une ligne `!` = quelque chose
  manque : la ligne dit quoi lancer (par exemple `scripts/download-models.sh voice`).
- [ ] **5. Ouvre un deuxième terminal pour le journal du cerveau** et laisse-le ouvert pendant
  les essais :

  ```bash
  jarvisctl logs -f
  ```

  C'est là que tu verras ce que Jarvis a entendu (« texte entendu »), si ta voix est reconnue
  (« voix reconnue score=… ») et ce qu'il a fait.
- [ ] **6. Si tu veux aussi le journal de l'oreille** (facultatif, dans un troisième terminal) :

  ```bash
  jarvisctl logs --oreille -f
  ```

  C'est là que s'affichent « mot d'éveil détecté (score …) » et l'état du micro.

## Bon à savoir : les phrases très courtes

Un « suivant » ou un « oui » dure moins d'une seconde : c'est trop court pour reconnaître ta
voix à coup sûr. Jarvis l'accepte quand même pour une commande rapide **dans les 3 minutes qui
suivent une phrase plus longue où ta voix a été reconnue**. C'est pour ça que chaque essai
commence par une vraie question : elle « ouvre » ces 3 minutes. Si une commande courte est
ignorée et que le journal dit `voix non reconnue (phrase trop courte)`, redis une phrase longue,
puis recommence.

## En cas de problème

- Le journal du cerveau est vide ou dit « cerveau arrêté » : `jarvisctl status`, puis
  `jarvisctl restart`.
- L'icône affiche « Cerveau injoignable » : même chose.
- Plus d'icône du tout (après « Quitter l'oreille », par exemple) : `jarvisctl start oreille`.
  Si elle ne revient pas : `jarvisctl logs --oreille`.
- Jarvis ne parle pas mais fait les actions : regarde la ligne « Voix de Jarvis » de
  `jarvisctl doctor`, et cherche « voix » dans le journal.
- Note simplement ce que tu as vu : je m'occupe du reste.
