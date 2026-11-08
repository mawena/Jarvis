# Personnes et rôles

Jarvis peut connaître **plusieurs personnes**. Chacune a un **rôle**, c'est-à-dire la liste de
ce qu'elle a le droit de lui demander. Par défaut il n'y en a qu'une : Charles, rôle `admin`
(tous les droits). Rien ne change pour lui tant qu'on n'ajoute personne.

À quoi ça sert : laisser un proche lancer la musique avec une oreille dans le salon (ou sur son
téléphone), sans qu'il puisse mettre le PC en veille, éteindre la machine ou lancer l'agent de
code.

Tout se gère **au clavier**, avec `jarvis person …` et `jarvis role …`. Il n'y a volontairement
**pas de gestion à la voix** : une phrase mal reconnue ne doit pas pouvoir créer un
administrateur.

## Qui parle ?

Jarvis déduit la personne de **l'oreille** qui a entendu la phrase. Une seule chose fait
exception : la **voix de Charles**, reconnue par son empreinte vocale, vaut Charles sur
**n'importe quelle oreille**.

L'empreinte ne connaît que Charles. Elle peut donc prouver que c'est lui, jamais que c'est un
invité : une phrase prise sur l'oreille d'un invité est dite **non vérifiée**. Elle ne peut
rien de risqué (mise en veille, redémarrage, arrêt), ni lancer l'agent, ni confirmer une
question.

| Sur l'oreille de… | Voix de Charles reconnue | Voix inconnue |
| --- | --- | --- |
| Charles (ou oreille non rattachée) | Charles, tous ses droits | ignorée, comme avant |
| un invité | Charles, tous ses droits | l'invité, non vérifié : ses outils, sans action risquée, sans agent, sans confirmation |
| une personne supprimée à la main | Charles | aucun droit |

Cas particuliers :

- **Phrases très courtes** (moins d'une seconde) : trop courtes pour reconnaître une voix. Sur
  l'oreille de Charles, elles sont acceptées comme **raccourcis seulement** (« suivant », jamais
  l'IA) dans les 3 minutes qui suivent une phrase longue où sa voix a été reconnue, sur cette
  même oreille ; sinon elles sont ignorées. Sur l'oreille d'un invité, elles restent à
  l'invité, en raccourcis seulement.
- **Sans empreinte enregistrée** (installation neuve, avant `jarvis-ear enroll`) : les phrases de
  l'oreille de Charles sont acceptées (tout sauf l'agent) ; sur l'oreille d'un invité, elles
  restent à l'invité.
- **Un « oui » d'un invité ne confirme jamais rien.** Seul le « oui » court de Charles, vérifié
  strictement, confirme. La confirmation doit venir de la **même oreille** que la question.
- **Texte tapé** (`jarvis say …`) : vaut Charles, `admin`. Une action risquée tapée reste
  refusée (« Pour cela, demandez-le-moi à voix haute. ») : elle se demande à la voix.

Les empreintes vocales **des autres personnes** ne sont pas faites : ce sera pour plus tard.
Jusque-là, un invité n'est jamais vérifié, et les droits `confirm` et `agent` d'un rôle
d'invité sont enregistrés mais n'ont pas d'effet pour lui (voir plus bas).

Quand une phrase est refusée, Jarvis répond « Je ne peux pas faire ça pour vous. » (bip
d'échec) et rien n'est lancé. Le journal (`jarvisctl logs -f`) dit pourquoi :

```
qui parle : oreille=… personne=Léa confiance=Unverified raccourcis_seuls=false
droit refusé : personne=Léa role=famille outil=suspend
```

## Les rôles

Un rôle donne :

- **`tools`** : les outils permis, par nom (`app_launch`) ou par préfixe (`media_*`), `*` pour
  tous ;
- **`confirm`** : le droit de lancer des actions risquées, après confirmation à la voix ;
- **`agent`** : le droit de lancer l'agent de code.

Le rôle `admin` est **intégré** : tous les droits, jamais écrit dans le fichier, ni modifiable
ni supprimable. Le nom `admin` est réservé.

```bash
jarvis role list
jarvis role add famille --tools 'media_*,volume_up,volume_down,app_launch,web_search,open_results,play_video'
jarvis role edit famille --tools 'media_*'          # remplace la liste des outils
jarvis role edit famille --confirm                  # ou --no-confirm
jarvis role edit famille --agent                    # ou --no-agent
jarvis role remove famille                          # refusé tant que quelqu'un l'a
```

`jarvis role add` accepte `--tools`, `--confirm` et `--agent` (tous facultatifs : sans
`--tools`, le rôle ne permet aucun outil). `jarvis role edit` exige au moins une option et
laisse les autres champs inchangés. Un nom de rôle peut avoir plusieurs mots (`jarvis role add
amis proches --tools …`).

Mettre les outils entre apostrophes évite que le shell développe `*`. Un motif qui ne
correspond à aucun outil connu est refusé (« aucun outil ne correspond à … »), ce qui
rattrape les fautes de frappe.

### Les outils

La liste exacte vient du cerveau (compétences et outils système) :

| Outil | Ce qu'il fait | Risque |
| --- | --- | --- |
| `media_play`, `media_pause` | lecture, pause du lecteur | non |
| `media_next`, `media_previous` | morceau suivant, précédent | non |
| `media_now_playing` | dit ce qui joue | non |
| `volume_up`, `volume_down` | volume du PC, plus ou moins 10 % | non |
| `app_launch` | lance une application | non |
| `web_search` | cherche sur internet | non |
| `open_results` | ouvre un résultat de recherche | non |
| `play_video` | cherche et lance une vidéo | non |
| `suspend` | met le PC en veille | **oui** |
| `reboot` | redémarre le PC | **oui** |
| `shutdown` | éteint le PC | **oui** |
| `set_mode` | passe en mode local ou en ligne | non |
| `status` | dit le mode et l'état de Jarvis | non |

Attention : `media_*` ne contient **pas** le volume (`volume_up`, `volume_down`) : pour qu'un
invité puisse dire « plus fort », ajoutez-les.

Un outil risqué (`suspend`, `reboot`, `shutdown`) demande qu'il soit dans la liste **et** que
le rôle ait `confirm`. Mais comme un invité n'est jamais vérifié (voir plus haut), il ne peut de
toute façon pas en lancer, même avec `confirm` : le drapeau prendra effet avec les empreintes
par personne.

Les outils de l'agent (`agent_start`, `agent_status`, `agent_stop`) **ne s'écrivent pas dans
`tools`** : ils dépendent uniquement du droit `agent` (`--agent`), et exigent en plus une voix
reconnue de Charles ou du texte tapé. Pour l'instant, seul Charles peut donc lancer l'agent.

Les futures compétences ne sont autorisées à personne d'autre que `admin` tant qu'on ne les a
pas ajoutées à un rôle.

## Les personnes

```bash
jarvis person list                          # nom, rôle, oreilles, empreinte vocale
jarvis person add Léa --role famille
jarvis person role Léa amis                 # change le rôle (le rôle est le dernier mot)
jarvis person attach Léa Pixel 4 XL         # rattache une oreille (nom, id ou début d'id)
jarvis person remove Léa                    # refusé tant qu'elle a une oreille
```

Les noms (de personnes et de rôles) font 1 à 40 caractères, sans caractère de contrôle, et sont
uniques sans tenir compte des majuscules ni des accents. Pour un nom de plusieurs mots avec
`person attach`, mets-le entre guillemets : `jarvis person attach "Léa Martin" Pixel 4 XL`.

Le propriétaire (Charles) ne peut être ni supprimé, ni changé de rôle : il reste `admin`.

Les changements valent **dès la phrase suivante**, sans redémarrer le cerveau. Une question
posée avant un changement de rôle est revérifiée au moment du « oui ».

### Rattacher une oreille

- À l'appairage : `jarvis pair --person Léa` (sans `--person`, l'oreille va au propriétaire).
  Le reste de l'appairage est inchangé, voir [reseau.md](reseau.md#appairer-une-oreille).
- Après coup : `jarvis person attach Léa <oreille>`.
- Une oreille appairée avant la phase 8, ou l'oreille du PC, est à Charles.
- `jarvis ears` a une colonne « Personne » (`-` si l'oreille n'est rattachée à personne).
- Retirer l'oreille (`jarvis unpair`) la détache de sa personne.

## Le fichier `people.toml`

`~/.local/share/jarvis/people.toml` (droits 0600, écrit de façon atomique). Il est créé au
premier lancement avec Charles `admin`. Le nom du propriétaire vient de `[people] owner =
"Charles"` dans `~/.config/jarvis/config.toml`, **utilisé seulement à la création du fichier**.

```toml
attached = ["3f9a1c07…", "b24e…"]     # toutes les oreilles rattachées

[[role]]
name = "famille"
tools = ["media_*", "volume_up", "volume_down", "app_launch"]
confirm = false
agent = false

[[person]]
name = "Charles"
role = "admin"
owner = true
ears = []

[[person]]
name = "Léa"
role = "famille"
ears = ["b24e…"]
```

Préfère les commandes à l'édition à la main. Si tu modifies le fichier à la main,
**redémarre le cerveau** (`jarvisctl restart cerveau`) : il ne le relit pas tout seul. Un
fichier invalide (propriétaire absent, doublon, oreille donnée à deux personnes…) met le
cerveau en mode dégradé (ci-dessous). Une oreille qui figure dans `attached` sans appartenir à
personne (personne supprimée à la main) n'a **aucun droit**, elle ne retombe jamais sur Charles.

## Mode dégradé

Si `people.toml` est illisible ou invalide, le cerveau démarre quand même, mais au
**moindre privilège** : seuls la voix reconnue de Charles et le texte tapé ont des droits ; les
oreilles n'en ont aucun ; les commandes de gestion qui écrivent sont refusées. `jarvisctl
doctor` le dit :

```
! Annuaire des personnes illisible (…) — mode dégradé : seule votre voix reconnue et le texte tapé ont des droits ; corrigez ou supprimez le fichier indiqué
```

Corrige le fichier (ou supprime-le : il sera recréé avec Charles seul), puis
`jarvisctl restart cerveau`.

## `jarvisctl doctor`

```
✓ Personnes et rôles : 1 personne (Charles, admin), 1 rôle intégré
✓ Personnes : 2 (Charles admin, Léa famille)
! Oreille « Salon » sans personne reconnue
```

La dernière ligne signale une oreille distante qui n'est rattachée à personne (elle vaut
alors Charles) : rattache-la si ce n'est pas lui.

## Ce qui viendra

- Une **empreinte vocale par personne**, pour vérifier un invité : alors seulement les droits
  `confirm` et `agent` d'un rôle d'invité auront un effet.
- Éventuellement la gestion par la voix, avec ces empreintes.
