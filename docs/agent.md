# Agent de code

« Jarvis, finis les tests du projet Jarvis » lance aussitôt **Claude Code** en arrière-plan,
sur une branche créée pour l'occasion dans un **worktree à part** : la copie de travail de
Charles n'est jamais touchée. Jarvis répond « Bien. Je confie cela à Claude, sur une branche à
part du projet Jarvis. Je vous préviens à la fin. », répond à « où en est la tâche ? » et à
« arrête la tâche », puis annonce la fin sur l'oreille la plus récemment active, avec une
notification de bureau. Rien n'est poussé ni fusionné.

Le départ est **direct, sans confirmation, dans les deux modes**, mais seulement pour une voix
**reconnue** par l'empreinte vocale ou du texte tapé (`jarvis say "…"`). Sans empreinte
enregistrée, ou avec la vérification désactivée, aucune voix ne lance l'agent : « Je ne confie
une tâche à l'agent qu'à une voix reconnue : enregistrez votre voix avec jarvis-ear enroll. »
(`jarvis say` marche toujours). L'IA de Jarvis ne peut pas le lancer : l'outil ne lui est
jamais proposé, et un outil qu'elle inventerait est refusé.

## Mise en route

Il faut [Claude Code](https://claude.com/claude-code) connecté au compte de Charles
(`~/.local/bin/claude`), `notify-send` (`sudo apt install libnotify-bin`) et, pour le bac à
sable des commandes de l'agent, `bubblewrap` et `socat` (`sudo apt install bubblewrap socat`).
Sans bac à sable, Claude Code refuse de démarrer (`failIfUnavailable`).

```bash
jarvisctl doctor    # Claude Code, ses options, le bac à sable, les projets, notify-send
```

`doctor` n'avertit (`!`) que si quelque chose manque : l'agent est facultatif, le reste de
Jarvis marche sans. Il vérifie :

- que `claude --help` connaît toutes les options utilisées (`--restricted`, `--tools`,
  `--strict-mcp-config`, `--mcp-config`, `--permission-mode`, `--permission-prompts`,
  `--settings`, `--session-id`, `--append-system-prompt`, `--disallowedTools`, `--verbose`,
  `stream-json`) ;
- que `bwrap` et `socat` sont là et que `bwrap` peut isoler une commande. Ubuntu 24.04 limite
  les espaces de noms : si ce test échoue, la ligne indique le profil AppArmor à ajouter ;
- pour chaque projet, que le dossier existe, que c'est un dépôt git et que la branche de base
  est là.

## Projets

Seuls les projets déclarés dans `~/.config/jarvis/config.toml` peuvent être confiés à l'agent.
Toutes les clés sont facultatives ; valeurs par défaut :

```toml
[agent]
enabled = true
claude_path = "~/.local/bin/claude"
max_minutes = 45          # durée maximale d'une tâche
idle_minutes = 30         # sans aucune sortie : tâche bloquée, arrêtée (> 25 min, délai max d'une commande)
max_tasks = 2             # en même temps ; une seule par projet
model = ""                # vide : modèle par défaut de Claude Code
max_budget_usd = 0.0      # 0 : sans plafond
pass_api_key = false      # aucune ANTHROPIC_API_KEY : l'agent utilise le compte de Charles
api_key_env = "ANTHROPIC_API_KEY"   # avec pass_api_key : variable lue, passée en ANTHROPIC_API_KEY
commit_at_end = true      # un commit de Jarvis à la fin d'une tâche réussie
worktree_dir = "~/.local/share/jarvis/worktrees"
cargo_target_dir = "~/.cache/jarvis/agent-target"   # suivi de /<projet>
cargo_home = "~/.cache/jarvis/agent-cargo"          # CARGO_HOME de l'agent, jamais ~/.cargo
extra_path = ["~/.cargo/bin", "~/.local/bin"]       # en tête du PATH de l'agent
extra_allowed_tools = []  # ajoutés aux outils permis ; les interdits restent prioritaires

[[agent.projects]]
name = "Jarvis"
path = "~/Work/Mawena/Projects/Jarvis"
aliases = ["l'assistant"]
# base = "develop"                        # sinon gitflow.branch.develop, sinon develop
# extra_allowed_tools = ["Bash(npm test*)"]
```

Sans `[[agent.projects]]`, seul le projet Jarvis ci-dessus est connu. Déclarer des projets
remplace cette liste. Redémarrer le cerveau après un changement (`jarvisctl restart`).

## Phrases reconnues

Comprises sans IA, précédées ou non de « Jarvis » ; les politesses (« s'il te plaît »,
« tu peux… », « merci », « merci Jarvis » en fin de phrase) sont ignorées.

**Départ** : un verbe de code en tête de phrase **et** un projet connu (nom ou alias, ou un
nom proche juste après « projet ») :

- « finis les tests du projet Jarvis », « corrige le bug dans Mawena »,
  « ajoute un test au projet Jarvis », « lance les tests de l'assistant » ;
- le projet **Jarvis** porte le nom de l'assistant : il faut dire « projet Jarvis » ou son
  alias « l'assistant ». « Mets à jour Jarvis », « supprime l'alarme Jarvis » ou « ajoute du
  lait à la liste, merci Jarvis » ne lancent rien ;
- verbes : finis, termine, achève, corrige, répare, ajoute, écris, implémente, refactorise,
  nettoie, teste, documente, améliore, optimise, renomme, migre, supprime, prépare, mets à
  jour, fais passer, lance les tests, fais tourner les tests ;
- ou une ouverture explicite : « demande à l'agent de relire le module audio du projet
  Jarvis », « confie à l'agent… », « tâche de code… ».

La consigne envoyée est la phrase telle que dite (accents compris), sans « Jarvis, » de tête
ni politesse finale, 2 000 caractères au plus. Projet inconnu après une ouverture explicite
(« demande à l'agent… du projet Zorglub »), ou nom proche d'un projet connu (« projet
Jarvoss ») : « Je ne connais pas le projet « Zorglub ». Projets connus : … ». Sinon (« ajoute
le projet vacances à ma liste »), comme sans projet nommé (« corrige le bug »), ce n'est pas
une tâche : Jarvis répond normalement. « Lance Jarvis » ou « ouvre le projet
Jarvis » ne lancent jamais l'agent.

**Suivi** : « où en est la tâche ? », « elle en est où, la tâche ? », « ça avance,
l'agent ? », « la tâche est finie ? », « l'agent a terminé ? », « quelles tâches tournent ? »,
« état de la tâche », « où en est la tâche de Mawena ? ». Réponse : depuis
combien de temps la tâche tourne et sa dernière action (« lance « cargo test » »), ou « Aucune
tâche en cours. » et la dernière terminée. Calculé à partir du flux de Claude Code : aucune IA,
rien ne part en ligne.

**Arrêt** : « arrête la tâche », « tu peux arrêter la tâche ? », « stoppe l'agent »,
« stop la tâche », « arrêtez la tâche », « annule les tâches », « interromps la tâche sur
Mawena ». Avec deux tâches en cours, nommer le projet (« arrête la tâche du projet Jarvis »).
« Arrête » ou « stop » seuls, ou « arrête la musique », ne touchent pas à l'agent. Le travail
reste sur sa branche.

## Garde-fous

- **Branche à part** : `<gitflow.prefix.feature>jarvis-<slug>` (par exemple
  `feature/jarvis-finis-tests`), créée depuis la branche de base (`base`, sinon
  `gitflow.branch.develop`, sinon `develop`) dans un worktree
  `~/.local/share/jarvis/worktrees/<dossier du dépôt>-jarvis-<slug>-<id>` (nom du dossier en
  minuscules, sans accents). Jamais `develop` ni `main` ;
  la copie de travail de Charles n'est jamais touchée.
- **Modifications non enregistrées** (fichiers suivis modifiés ou indexés) : « Le projet
  Jarvis a des modifications non enregistrées : je ne lance rien. » Rien n'est créé. Les
  fichiers non suivis sont ignorés.
- **Rien de poussé ni fusionné**, trois couches :
  1. Claude Code en **mode restreint** (`--restricted`) : réglages utilisateur, projet
     (`.claude/settings.json` du dépôt) et local ignorés ; outils `Bash, Read, Edit, Write,
     Glob, Grep` seulement (`--tools`), aucun serveur MCP ni connecteur ; outils de fichiers
     confinés au worktree. Mode `dontAsk` (tout ce qui n'est pas permis est refusé) ; permis :
     lire et modifier dans le worktree (`Read(./**)`, `Edit(./**)`), chercher,
     `cargo build/check/test/clippy/fmt/doc/tree`, `scripts/check.sh`, git en lecture ;
     interdits : `git push/merge/rebase/reset/checkout/switch/branch/commit/worktree/remote/config/flow/tag`,
     `gh`, `cargo run/publish/install/update/login`, WebFetch, WebSearch, la lecture des
     secrets (`~/.ssh`, `~/.gnupg`, `~/.config/jarvis`, `~/.config/gh`, `~/.aws`…) et la
     modification de `.git`, `.claude`, `.cargo`, `.githooks`, `.husky` ;
     **bac à sable** obligatoire pour chaque commande (et ses enfants : `build.rs`, tests) :
     écriture limitée au worktree, au dossier de compilation et au `CARGO_HOME` de l'agent,
     secrets illisibles, réseau limité à `crates.io`, aucun socket Unix (ni agent SSH, ni
     D-Bus, ni socket de Jarvis) ;
  2. environnement du processus **vidé** puis rempli d'une liste fermée (`HOME`, `USER`,
     langue, `TMPDIR`, `XDG_*_HOME`, `PATH`, cargo) : ni `SSH_AUTH_SOCK`, ni `DBUS_*`, ni
     `XDG_RUNTIME_DIR`, ni jeton (`GH_TOKEN`, `ANTHROPIC_AUTH_TOKEN`, `AWS_*`…) ; chaque
     remote pousse vers une adresse sans transport, aucun assistant d'identifiants,
     `GIT_SSH_COMMAND=false` ;
  3. vérification après coup : branches protégées inchangées, aucune nouvelle branche
     distante, worktree toujours sur sa branche, partie de la base. Toute violation est annoncée en urgence
     (« Attention : la branche develop a été modifiée. Vérifiez le projet Jarvis. »).
- **Limites** : 45 minutes, 30 minutes sans sortie, 2 tâches en même temps, une par projet ;
  une commande de l'agent dure 15 minutes par défaut, 25 au plus. L'arrêt envoie `TERM` à
  Claude Code (qui arrête ses commandes), puis, 10 s plus tard, tue tout le groupe de
  processus (y compris un `cargo test` lancé par l'agent).
- **Compilation dans le bac à sable** : l'agent a son propre `CARGO_HOME`, donc sa première
  compilation retélécharge les crates. Seul `crates.io` est joignable : une dépendance git ou
  un autre registre échoue. Le `~/.cargo/config.toml` de Charles ne s'applique pas.
- **Redémarrage du cerveau** : les tâches en cours sont interrompues (`jarvisctl restart`,
  `stop` et `update` le disent avant) ; au démarrage suivant, une notification les signale.
  Le worktree et la branche restent.

## Fin de tâche

À la fin d'une tâche **réussie** (sortie normale, aucune erreur, aucune violation, au moins un
fichier modifié), Jarvis fait **un seul commit** dans le worktree, `wip(jarvis): <consigne>`,
avec l'identité et la signature git de Charles, sans ligne d'attribution (l'agent lui-même ne
commite jamais). Commit ou signature impossible : le travail reste non enregistré dans le
worktree, et Jarvis le dit. `commit_at_end = false` : jamais de commit.

Annonce : « La tâche sur Jarvis est terminée : <résumé de l'agent>. 3 fichiers modifiés. »
Échec, délai dépassé ou violation sont dits aussi ; un arrêt demandé ne donne qu'une
notification. La notification indique la branche, le worktree, la commande de reprise et le
journal.

## Où trouver le travail, comment finir

```bash
git worktree list                                   # dans le dépôt du projet
cd ~/.local/share/jarvis/worktrees/jarvis-jarvis-finis-tests-1a2b3c4d
git log develop..HEAD                               # le commit de Jarvis
claude --resume <id>                                # reprendre la session de l'agent (id dans la notification)
```

Pour garder le travail, depuis le dépôt du projet :

```bash
git worktree remove ~/.local/share/jarvis/worktrees/jarvis-jarvis-finis-tests-1a2b3c4d
git flow feature finish jarvis-finis-tests
```

Pour l'abandonner : `git worktree remove --force <dossier>` puis
`git branch -D feature/jarvis-…`. Les worktrees ne sont jamais nettoyés automatiquement.

## Ce qui part en ligne

L'agent est **la seule compétence qui envoie du contenu à une IA en ligne, même en mode
local** : la consigne et le code du projet partent chez Claude Code (Anthropic), sous
l'**abonnement Claude Code de Charles**. La phrase de départ le dit (« je confie cela à
Claude »). Aucune `ANTHROPIC_API_KEY` n'est transmise à l'agent (`pass_api_key = false`) : il
n'est pas facturé sur la clé API du mode en ligne. Le suivi, l'arrêt et le compte des fichiers
n'utilisent aucune IA ; le résumé de fin est la dernière ligne `RÉSUMÉ : …` de l'agent, ou à
défaut la dernière phrase de sa réponse.

## Journaux

- Journal brut de chaque tâche : `~/.local/state/jarvis/tasks/<id>.log` (sortie de Claude
  Code et erreurs, 20 Mo au plus), à côté de son enregistrement `<id>.json`. Dossier 0700,
  fichiers 0600. Au démarrage du cerveau, seules les 50 dernières tâches terminées sont gardées.
- `jarvisctl logs` : la consigne (en `info`, comme toute phrase), « consigne envoyée à Claude
  Code (en ligne) » au départ, puis la fin et les refus.
