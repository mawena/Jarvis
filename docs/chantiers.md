# Grands chantiers (Jarvis + CCS)

L'agent de code ([agent.md](agent.md)) fait une tâche courte sur la machine de Jarvis, dans un
bac à sable, en 45 minutes au plus. Un **grand chantier** est l'inverse : une fonction entière,
confiée par la voix à Claude Code **sur le serveur**, par la file de tâches de
[CCS](https://github.com/mawena/CCS). CCS choisit le compte qui a du quota, reprend après une
limite atteinte, prévient sur le téléphone, et le chantier se suit sur https://ccs.mawena.cloud.
Jarvis annonce la fin à voix haute.

```
 « Jarvis, grand chantier sur CCS :          laptop-maison (cerveau)        serveur (VPS)
   ajoute la synchronisation des réglages »  ── ssh, clé restreinte ──►  jarvis-pont-ccs
                                                                           └► ccs tache ajouter
 « …est fini. Le résumé est dans CCS. »  ◄── veille toutes les 3 min ──   Claude Code (git flow)
```

## Phrases

| Phrase (après « Jarvis… ») | Effet |
| --- | --- |
| « lance un grand chantier sur CCS : ajoute X » | question « Grand chantier sur CCS : « ajoute X ». Je le confie à Claude sur le serveur ? », puis « oui » |
| « nouveau chantier pour le projet Jarvis pour ajouter un minuteur » | idem |
| « où en est le chantier ? », « le chantier est fini ? » | état de chaque chantier actif (en attente, en cours, en pause de quota) |
| « annule le chantier (sur CCS) » | annule le dernier chantier actif (du projet) |

Il faut un mot de départ (« lance », « nouveau », « grand », « démarre »…) devant « chantier » et
le nom d'un projet déclaré juste après. Comme l'agent : jamais lancé par l'IA, voix reconnue
obligatoire, droit `agent` dans [personnes.md](personnes.md), et un « oui » avant le départ.

## Ce que fait Claude sur le serveur

Le pont enveloppe la consigne dans une marche à suivre fixe (`BRIEF` dans
`scripts/jarvis-pont-ccs`) : lire README et ROADMAP, branche git flow `feature/…` depuis
`develop`, tâches ajoutées et cochées dans `ROADMAP.md`, tests des seuls modules touchés avec
`nice -n 10`, README et CHANGELOG, commits en français au nom de Charles, **push de la branche
feature seulement**. Jamais de fusion, de tag, de release, de `sudo`, ni de service en
production : Charles relit puis termine la feature lui-même. Le résumé final de Claude est repris
dans la notification de fin.

Par défaut la tâche CCS a les droits `tout` (sans eux, Claude ne peut ni tester ni committer en
mode `-p`), et part dès qu'un compte a du quota (`quota`).

## Installation

### 1. Sur le serveur (là où tournent CCS et les projets)

```bash
install -m 755 scripts/jarvis-pont-ccs ~/.local/bin/jarvis-pont-ccs
jarvis-pont-ccs projets          # liste les projets trouvés sous ~/Work/{perso,pro,Mawena}
```

Réglages facultatifs dans `~/.config/jarvis-pont/pont.env` (`KEY=valeur`) :
`JARVIS_PONT_RACINES`, `JARVIS_PONT_DROITS` (`lecture`, `modifications`, `tout`),
`JARVIS_PONT_MODELE`, `JARVIS_PONT_QUAND` (`quota`, `nuit`, `HH:MM`).

### 2. Sur la machine de Jarvis

Une clé SSH à elle, sans phrase de passe, qui ne sert qu'au pont :

```bash
ssh-keygen -t ed25519 -N "" -C "jarvis-chantier@$(hostname)" -f ~/.ssh/jarvis_chantier
cat ~/.ssh/jarvis_chantier.pub
```

### 3. Autoriser la clé sur le serveur, limitée au pont

Dans `~/.ssh/authorized_keys` du serveur, une ligne (la clé ne peut rien lancer d'autre, ni
tunnel, ni terminal) :

```
command="/home/<utilisateur>/.local/bin/jarvis-pont-ccs",restrict ssh-ed25519 AAAA… jarvis-chantier@laptop-maison
```

Essai depuis la machine de Jarvis : `ssh -i ~/.ssh/jarvis_chantier -p 2244 <utilisateur>@mawena.cloud projets`.

### 4. Configuration de Jarvis (`~/.config/jarvis/config.toml`)

```toml
[chantier]
enabled = true
command = ["ssh", "-i", "~/.ssh/jarvis_chantier", "-p", "2244",
           "-o", "BatchMode=yes", "-o", "ConnectTimeout=10", "-o", "IdentitiesOnly=yes",
           "<utilisateur>@mawena.cloud"]
poll_minutes = 3

[[chantier.projects]]
name = "CCS"                 # nom du dossier sur le serveur
aliases = ["c c s"]

[[chantier.projects]]
name = "Jarvis"
aliases = ["l'assistant"]
```

Puis `systemctl --user restart jarvis-brain`. Les chantiers suivis sont gardés dans
`~/.local/state/jarvis/chantiers.json` : la veille reprend après un redémarrage.

## Sécurité

- La clé de Jarvis n'ouvre ni terminal ni tunnel : le serveur n'exécute que le pont, qui ne
  connaît que quatre ordres (`projets`, `ajouter`, `voir`, `annuler`) aux arguments vérifiés
  (nom de dossier, nombres, consigne en hexadécimal, jamais passée à un shell).
- Le pont ne voit et n'annule que les chantiers qu'il a lui-même ajoutés.
- Un projet doit être un dépôt git sous une des racines ; aucun autre chemin n'est accepté.
- Les droits `tout` sont ceux d'une tâche CCS lancée à la main : la marche à suivre interdit
  `sudo` et les services en production, mais ce n'est pas un bac à sable. Pour un projet
  sensible, `JARVIS_PONT_DROITS=modifications` (Claude écrit, mais ne lance rien).
