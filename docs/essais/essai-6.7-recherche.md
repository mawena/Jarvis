# Essai 6.7 : recherche et vidéos

**But :** vérifier que Jarvis cherche sur internet et résume la réponse à voix haute, qu'il
ouvre les résultats dans le navigateur, et qu'il lance une vidéo en mettant la musique en pause.
On vérifie aussi qu'en mode local, rien ne part chez Claude.

**Durée :** 20 minutes (plus le premier téléchargement de SearXNG, environ 1 à 3 minutes).

**Avant de commencer :**

- [ ] La [mise à jour](README.md#avant-tout-essai--mise-à-jour) est faite après cette phase
  (`cargo run -q -p jarvisctl -- update`), l'icône Jarvis est là.
- [ ] Le journal du cerveau est ouvert dans un terminal : `jarvisctl logs -f`.
- [ ] Spotify (ou un autre lecteur) joue de la musique.

---

## Étape 1 : démarrer le moteur de recherche

- [ ] Lance SearXNG (la première fois, Docker télécharge l'image) :

  ```bash
  jarvisctl searx up
  ```

  **Attendu :** la progression du téléchargement, puis un message disant que SearXNG répond.

- [ ] Vérifie :

  ```bash
  jarvisctl doctor
  ```

  **Attendu :** des coches `✓` pour Docker, SearXNG, le format JSON, le port (127.0.0.1
  seulement), yt-dlp et xdg-open.

- [ ] Vérifie que SearXNG n'est **pas** visible depuis le réseau :

  ```bash
  ss -ltn | grep 8888
  ```

  **Attendu :** une seule ligne, avec `127.0.0.1:8888` (et pas `0.0.0.0` ni `*`).

**Si `searx up` échoue** (par exemple le conteneur redémarre en boucle) : copie la sortie de
`docker logs jarvis-searxng` dans les résultats.

## Étape 2 : une recherche en mode local, au clavier

On commence au clavier, pour mesurer le temps sans le micro. Vérifie que Jarvis est en mode
local (l'icône l'indique ; sinon `jarvis say "passe en mode local"`).

- [ ] Lance, en chronométrant :

  ```bash
  time jarvis say "cherche la météo de demain à Lomé"
  ```

**Attendu :**
- une réponse d'une ou deux phrases sur la météo ;
- environ 10 s (le résumé par l'IA locale prend environ 8 s) ;
- dans le journal, une ligne avec `resume="ia"` (résumé par l'IA) ou `resume="extrait"` (l'IA a
  été trop lente : Jarvis a lu un extrait).

**À noter :** le temps (`real`), la réponse, et `ia` ou `extrait`.

## Étape 3 : ouvrir les résultats

- [ ] `jarvis say "ouvre les résultats"`

**Attendu :** Firefox ouvre la page de résultats de SearXNG pour « la météo de demain à Lomé ».

- [ ] `jarvis say "ouvre le deuxième résultat"`

**Attendu :** Firefox ouvre le deuxième lien.

## Étape 4 : la même recherche en mode en ligne

- [ ] `jarvis say "passe en mode en ligne"`, puis :

  ```bash
  time jarvis say "cherche la météo de demain à Lomé"
  ```

**À noter :** le temps et la qualité de la réponse, comparés à l'étape 2.

- [ ] Reviens en mode local : `jarvis say "passe en mode local"`.

## Étape 5 : à la voix

Commence par une phrase longue pour que ta voix soit reconnue (voir
[les phrases très courtes](README.md#bon-à-savoir--les-phrases-très-courtes)).

- [ ] **Dis** : « Jarvis… cherche qui a gagné la dernière Coupe d'Afrique des nations ».

**Attendu :** bip, la musique baisse, puis Jarvis répond à voix haute ; la musique remonte.

- [ ] **Dis** : « Jarvis… ouvre les résultats ».

**Attendu :** la page de résultats s'ouvre.

## Étape 6 : une vidéo

- [ ] La musique joue. **Dis** : « Jarvis… lance la vidéo de Daft Punk ».

**Attendu :**
- Spotify se met en pause ;
- Firefox ouvre une vidéo YouTube de Daft Punk ;
- Jarvis dit « Je lance « … ». ».

**À noter :** la vidéo démarre-t-elle seule, ou faut-il cliquer sur lecture ? (Firefox bloque
parfois la lecture automatique : on pourra l'autoriser pour youtube.com.)

- [ ] Pendant la vidéo, **dis** : « Jarvis… » puis rien.

**Attendu :** le son de la vidéo baisse, puis remonte.

- [ ] **Dis** : « Jarvis… pause ».

**À noter :** la vidéo se met-elle en pause ? (Firefox installé en snap n'est pas toujours
pilotable : Jarvis envoie alors la touche « pause » du clavier.)

## Étape 7 : rien chez Claude en mode local

- [ ] En mode local, refais une recherche au clavier, puis regarde le journal :

  ```bash
  jarvis say "cherche le prix du pain à Kara"
  jarvisctl logs | tail -20
  ```

**Attendu :** aucune ligne ne mentionne Claude ou l'IA en ligne pour cette recherche, et le
journal ne contient ni les titres ni les adresses des résultats.

---

## Résultats

Remplis, puis colle-moi cette section.

| Étape | OK ? | Remarque |
| --- | --- | --- |
| 1. `searx up` et `doctor` | | |
| 1. Port 127.0.0.1 seulement | | |
| 2. Recherche locale | | temps : … ; `ia` ou `extrait` : … |
| 3. Ouvrir les résultats / le 2e | | |
| 4. Recherche en ligne | | temps : … |
| 5. Recherche à la voix | | |
| 6. Vidéo + Spotify en pause | | lecture automatique ? |
| 6. « pause » sur la vidéo | | |
| 7. Rien chez Claude en local | | |

**Les réponses sont-elles justes et assez courtes ?**

**10 s en local, c'est supportable ?** (sinon : extrait seul en local, environ 2 s)

**Autre chose :**
