# Jarvis aux commandes du réseau CCS (protocole)

Ce document s'adresse au côté Jarvis (projet `~/Work/perso/Jarvis`) : il décrit les méthodes du
pont CCS qui permettent à Jarvis de voir et piloter tout le réseau (sessions, sous-agents,
machines, travaux distants) par la voix, par exemple « tue le premier sous-agent de la session
GliGli ». Code : `ccs_app/server/bridge_pilot.py` (suivi des sous-agents : `ccs_app/subagents.py`).

## Connexion

Comme l'appli web : WebSocket `wss://<serveur>/api/ws?protocol=1` avec la session CCS ou
`Authorization: Bearer <jeton d'application>` (ou, sur la machine même, le socket Unix du pont
local, `hello` d'abord). Un objet JSON par message :

- requête `{"id": 1, "method": "jarvis.state", "params": {}}` ;
- réponse `{"id": 1, "result": {…}}` ou `{"id": 1, "error": {"code": "…", "message": "…"}}`.

Ne pas ajouter `?machine=` : les méthodes `jarvis.*` sont toujours traitées par le pont du
serveur, qui joint lui-même les autres machines (ssh sur le réseau maillé, Ricochet en secours),
toutes en même temps, 20 s au plus chacune.

Codes d'erreur utiles à Jarvis (le `message` est une phrase en français, lisible telle quelle) :

| Code | Sens | Que faire |
|---|---|---|
| `introuvable` | rien ne correspond (session, machine, sous-agent, projet), ou machine injoignable | lire le message (il liste ce qui existe) |
| `conflit` | plusieurs cibles possibles, ou plusieurs sous-agents sans rang donné | poser la question du message (« Lequel ? », « Sur quelle machine ? ») |
| `invalide` | paramètre mal formé (rang incompris…) | redemander |
| `refuse` | action impossible (sous-agent d'une session de terminal) | dire le message |

## Désigner une cible

- **Session** (`session`) : nom approché. Casse, accents, tirets et petits mots (« la »,
  « session », « projet », « de »…) sont ignorés : « la session Gli-Gli » trouve `GliGli`. Comparé
  au titre de la conversation, au nom de session Claude Code, au dossier du projet, à
  l'identifiant. Accepte aussi un rang (`1`, `"premier"`, `"dernier"`) dans l'ordre de
  `jarvis.sessions`. Ambigu → `conflit` ; ajouter `machine`.
- **Machine** (`machine`, facultatif) : identifiant ou nom approché (`"lp work"`, `"serveur ccs"`,
  `"laptop maison"`).
- **Sous-agent** (`index`) : rang **parmi les sous-agents qui tournent encore**, dans l'ordre de
  lancement : `1` ou `"premier"`, `2` ou `"deuxième"`, …, `-1` ou `"dernier"`. C'est le champ
  `active_index` des listes. Sans `index` : le seul qui tourne, sinon `conflit` (le message les
  énumère). `tool_id` (exact) désigne un sous-agent sans ambiguïté.
- **Projet** (`project`) : nom approché du dossier, ou chemin (`/…`, `~/…`).

## Méthodes

### `jarvis.state` — vue d'ensemble

Paramètres : aucun. Réponse :

```json
{
  "summary": "2 machines en ligne sur 3 ; 3 sessions (1 au travail, 1 en attente de réponse) ; 2 sous-agents en cours ; 1 travail distant actif.",
  "machines": [{"id": "vps", "name": "Serveur CCS", "online": true, "local": true, "reachable": true,
                "cores": 6, "load": 0.4, "mem_used": 0, "mem_total": 0, "running": 0, "waiting": 0}],
  "sessions": [<session>, …],
  "jobs": [{"id": "abc123", "command": "make test", "machine": "pc", "state": "running",
            "state_label": "en cours", "code": null, "cwd": "…", "created": 1763900000.0, "ended": null}],
  "errors": [{"machine": "pc", "message": "pc injoignable : délai dépassé"}]
}
```

`summary` peut être lu à voix haute. Dans `jarvis.state`, chaque session ne garde que ses
sous-agents **en cours**. `jobs` : travaux de `ccs distant` actifs, puis les 5 derniers finis
(`state` : `waiting`, `scheduled`, `sending`, `running`, `done`, `failed`, `stopped`).
`errors` : machines en ligne qui n'ont pas répondu (une machine hors ligne n'est pas une erreur).
`reachable` : la machine a répondu.

Une **session** :

```json
{
  "machine": "pc", "machine_name": "lp-work", "local_machine": false,
  "kind": "chat", "chat_id": "c1a2b3-4", "pid": 4242, "proc_start": null, "session_id": "…",
  "name": "GliGli", "title": "GliGli", "label": "", "project": "GliGli", "cwd": "/home/…/GliGli",
  "state": "working", "state_label": "au travail", "started": 1763900000.0, "devices": 1,
  "subagents": [<sous-agent>, …], "subagents_running": 2, "subagents_stoppable": true
}
```

- `kind` : `chat` (session tenue par CCS : web, applis, bureau ; tout est pilotable) ou
  `terminal` (Claude Code lancé dans un terminal : on ne peut que l'arrêter en entier).
- `name` : nom à dire (titre de la conversation, sinon nom de session, sinon projet).
- `state` : `working`, `waiting` (attend une réponse ou une autorisation), `idle`, `suspended`
  (en veille, relancée au prochain message), `stopped` ; `state_label` en français.

Un **sous-agent** :

```json
{"index": 2, "active_index": 1, "description": "Lire le code", "type": "Explore",
 "state": "running", "background": true, "started": 1763900100.0, "ended": null,
 "summary": "", "tool_id": "toolu_…", "task_id": "a1b2…"}
```

`index` : rang parmi tous (ordre de lancement) ; `active_index` : rang parmi ceux qui tournent
(`null` s'il a fini) ; `state` : `running`, `done`, `failed`, `stopped`.

### `jarvis.sessions` — sessions, sous-agents compris

Paramètres : `machine` (facultatif). Réponse : `{"sessions": [<session>…], "errors": […]}`, avec
**tous** les sous-agents de chaque session.

### `jarvis.subagents` — sous-agents d'une session

Paramètres : `session`, `machine` (facultatif). Réponse :
`{"session": <session sans "subagents">, "subagents": [<sous-agent>…]}` dans l'ordre de lancement.

### `jarvis.stop_subagent` — arrêter un sous-agent (destructeur)

Paramètres : `session`, `machine` (facultatif), `index` ou `tool_id`, `confirme`.

Sans `confirme: true`, **rien n'est fait** :

```json
{"done": false, "confirm_needed": true, "action": "stop_subagent",
 "target": {"machine": "pc", "machine_name": "lp-work", "session": "GliGli", "kind": "chat",
            "chat_id": "…", "pid": 4242, "project": "GliGli", "state": "working",
            "subagent": {"active_index": 1, "index": 2, "description": "Lire le code",
                         "type": "Explore", "tool_id": "toolu_…", "task_id": "a1b2…"}},
 "question": "Arrêter le sous-agent 1 « Lire le code » de la session GliGli sur lp-work ?"}
```

Jarvis pose `question` à voix haute ; sur un « oui », il renvoie **le même appel avec
`tool_id` = `target.subagent.tool_id`** (pour ne pas viser un autre sous-agent si l'ordre a changé
entre-temps) et `confirme: true`. Réponse : `{"done": true, "target": {…}, "message": "Arrêt
demandé pour le sous-agent « Lire le code »."}`. L'arrêt est demandé à Claude Code (`stop_task`) :
le reste de la session continue.

Session de terminal → `refuse` (seule toute la session peut être arrêtée).

### `jarvis.stop_session` — arrêter une session (destructeur)

Paramètres : `session`, `machine` (facultatif), `confirme`.

Sans confirmation : `{"done": false, "confirm_needed": true, "action": "stop_session", "target":
{…}, "question": "Arrêter la session GliGli sur lp-work (au travail, avec 2 sous-agent(s) en
cours) ?"}`. Avec `confirme: true` : `{"done": true, "target": {…}, "message": "Session GliGli
arrêtée."}`. Un chat est fermé (sa conversation reste enregistrée et peut être reprise) ; une
session de terminal reçoit SIGTERM. Pour confirmer, renvoyer de préférence la cible exacte :
`session` = `target.chat_id` (un identifiant de chat est reconnu) et `machine` = `target.machine`.

### `jarvis.start_session` — lancer une session

Paramètres : `project` (nom approché ou chemin), `machine` (facultatif), `message` (facultatif :
premier message envoyé à Claude), `options` (facultatif : `model`, `effort`, `rights` comme
`chat.open`). Pas de confirmation (rien n'est détruit).

Sans `machine`, le projet est cherché sur toutes les machines en ligne ; présent sur plusieurs
(cas courant : le serveur a une copie des projets du PC) → `conflit` « Sur quelle machine ? ».
Un chemin sans `machine` vise le serveur. Réponse :

```json
{"done": true, "chat_id": "c…-5", "cwd": "/home/…/GliGli", "project": "GliGli", "sent": true,
 "machine": "pc", "machine_name": "lp-work"}
```

La session apparaît aussitôt dans le web et les applis (Jarvis n'en suit pas le fil).

### Méthodes internes

Appelées par le serveur sur les autres machines, pas par Jarvis : `jarvis.local` (sessions de
cette machine seulement), `jarvis.projects` (projets connus : `{name, cwd}`), et les trois actions
avec `local: true` et une cible exacte (`chat_id`, `pid`, `tool_id`, `cwd`).

## Exemple : « tue le premier sous-agent de la session GliGli »

1. `jarvis.stop_subagent {"session": "GliGli", "index": 1}` → `question`.
2. Jarvis : « Arrêter le sous-agent 1 « Lire le code » de la session GliGli sur lp-work ? » —
   « Oui. »
3. `jarvis.stop_subagent {"session": "GliGli", "machine": "pc", "tool_id": "toolu_…",
   "confirme": true}` → `done: true` ; Jarvis lit `message`.

## Limites

- Les sous-agents d'une session de terminal sont lus sur le disque (`<session>/subagents/*.meta.json`
  de Claude Code) : liste seulement, pas d'arrêt un par un.
- Arrêter un sous-agent seul demande une version de Claude Code qui connaît `stop_task` ; sinon
  `introuvable` (« identifiant de tâche inconnu »).
- Une machine dont le pont CCS est plus ancien ne montre que ses chats, sans sous-agents.
