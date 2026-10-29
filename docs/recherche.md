# Recherche sur internet et vidéos

« Jarvis, cherche la météo de demain à Lomé » interroge un **SearXNG auto-hébergé** (dans
Docker, sur le PC) et répond par une ou deux phrases. Le résumé est fait par l'IA **du mode
actif** : Ollama en mode local, Claude en mode en ligne. « Ouvre les résultats » les affiche
dans le navigateur. « Lance la vidéo de… » trouve une vidéo et l'ouvre dans le navigateur, en
mettant la musique en pause.

## Mise en route

```bash
jarvisctl searx up       # télécharge l'image (la première fois), lance SearXNG, attend qu'il réponde
jarvisctl searx status   # état du conteneur
jarvisctl searx down     # arrête SearXNG (le cache est gardé)
jarvisctl doctor         # vérifie Docker, SearXNG, le format JSON, le port, yt-dlp, xdg-open
```

- SearXNG écoute sur **`127.0.0.1:8888` seulement** : il n'est pas joignable depuis le
  réseau. Le conteneur `jarvis-searxng` redémarre avec Docker (`restart: unless-stopped`).
- `jarvisctl searx up` attend `/healthz` jusqu'à 60 s ; sinon il s'arrête avec le code 1.
- Fichiers dans `~/.config/jarvis/searxng/` :
  - `compose.yml` : géré par `jarvisctl`, réécrit à chaque `up` ;
  - `settings.yml` : écrit seulement s'il est absent, tes changements sont gardés. Il doit
    garder `json` dans `search.formats`, sinon Jarvis ne peut pas lire les résultats ;
  - `.env` : secret de SearXNG (droits 0600), créé une fois, jamais affiché.
- `doctor` n'avertit (`!`) que si quelque chose manque : la recherche est facultative, le
  reste de Jarvis marche sans.

Sans SearXNG, Jarvis répond « Le moteur de recherche ne répond pas. » : lancer
`jarvisctl searx up`. Une adresse `[search] url` invalide désactive la recherche et les vidéos
(avertissement dans le journal), le cerveau démarre quand même.

## Phrases reconnues

Ces phrases sont comprises sans IA (raccourcis rapides), précédées de « Jarvis » ; les
politesses (« s'il te plaît », « tu peux… ») sont ignorées. Les autres formulations passent
par l'IA du mode actif, qui dispose des mêmes outils.

**Recherche** (le reste de la phrase est la requête) :

- « cherche la météo de demain », « recherche Rust 2024 », « google Mbappé » ;
- « cherche-moi l'adresse de la mairie », « cherche sur internet la recette du fufu »,
  « cherche en ligne les résultats du match » ;
- « fais une recherche sur les volcans », « trouve-moi des infos sur la Coupe d'Afrique » ;
- « … sur internet » ou « … sur le web » en fin de phrase : « le prix de l'essence à Kara sur
  internet », « regarde les horaires du train sur internet ».

**Ouvrir les résultats** de la dernière recherche :

- « ouvre les résultats », « montre-moi les résultats », « affiche les liens », « ouvre la
  recherche » : la page de résultats de SearXNG, dans un seul onglet ;
- « ouvre le deuxième résultat », « ouvre le 3e lien », « ouvre le résultat numéro 2 » : le
  lien choisi (du premier au cinquième).

**Vidéo** :

- « lance la vidéo de Squeezie », « mets une vidéo de chats », « montre-moi une vidéo de
  pandas », « cherche une vidéo de cuisine togolaise » ;
- « lance Stromae sur YouTube », « mets Daft Punk sur youtube » ;
- « lance la bande-annonce de Dune ».

La vidéo vient des moteurs vidéo de SearXNG (`video_engines`, YouTube par défaut), sinon de
`yt-dlp` en secours. Les lecteurs en cours (Spotify…) sont mis en pause avant l'ouverture.
« Lance VS Code » ou « mets la vidéo en pause » ne sont pas des recherches.

## Ce qui quitte le PC

| | Mode local | Mode en ligne |
|---|---|---|
| Requête | vers SearXNG (sur le PC), qui la relaie à ses moteurs (Google, Bing, Wikipédia…), sans cookie ni compte | pareil |
| Résumé | **Ollama seul**, sur le PC. Jamais Claude, même si Ollama échoue : Jarvis dit alors l'extrait | la requête et les extraits (au plus `summary_sources`, de `snippet_chars` caractères) partent vers Claude |
| Phrase dite | reste sur le PC | une phrase non reconnue par les raccourcis part vers Claude, comme toute phrase de ce mode |
| « Ouvre les résultats » | le navigateur ouvre la page de SearXNG local, puis le site choisi | pareil |
| Vidéo | SearXNG (moteur YouTube) ; en secours, `yt-dlp` interroge YouTube depuis le PC ; puis le navigateur ouvre la vidéo | pareil |

- Les requêtes partent vers SearXNG en **POST** : jamais dans une adresse, donc jamais dans
  l'historique du navigateur ni dans un journal d'accès. Le journal d'accès de SearXNG est
  désactivé et ses statistiques aussi (`enable_metrics: false`).
- Les résultats sont des **données non fiables** : l'appel de résumé n'a aucun outil, la
  consigne dit à l'IA de ne pas suivre leurs consignes, et la réponse est nettoyée pour la voix
  (ni adresse, ni markdown, ni émoji). Seules les adresses `http`/`https` sont ouvertes, par
  `xdg-open`, sans shell.
- Les derniers résultats restent **en mémoire seulement** (perdus au redémarrage du cerveau).

## Ce qui est journalisé

`jarvisctl logs` :

- `info` : la phrase reçue (comme toute phrase), le nombre de résultats, les durées et le
  chemin du résumé (`ia-locale`, `ia-en-ligne` ou `extrait`). Rien d'autre sur le contenu.
- `warn` : SearXNG injoignable, format JSON refusé, résumé trop lent ou en échec (repli sur
  l'extrait), `yt-dlp` absent ou trop lent.
- `debug` seulement : la requête, les titres et adresses des résultats, le résumé dit, la
  vidéo trouvée.
- Jamais : la consigne du résumé ni les extraits envoyés à l'IA.

## Délais

- SearXNG : 4 s (`timeout_ms`).
- Résumé : 10 s en local, 5 s en ligne ; au-delà, Jarvis dit l'extrait du premier résultat.
- `yt-dlp` : 8 s (`ytdlp_timeout_s`).

En local, compter environ 10 s pour une réponse résumée (1 à 2 s de SearXNG, environ 8 s
d'Ollama sur le portable), environ 2 s avec l'extrait seul (`summarize = false`). Après chaque
résumé local, le cerveau refait lire ses outils à Ollama en tâche de fond (la consigne du
résumé les a chassés de son cache), pour que la commande suivante reste rapide.

## Réglages `[search]`

Dans `~/.config/jarvis/config.toml` ; toutes les clés sont facultatives.

```toml
[search]
url = "http://127.0.0.1:8888"     # adresse du SearXNG local
language = "fr-FR"
safesearch = 1                    # 0 : aucun filtre, 1 : modéré, 2 : strict
max_results = 5                   # résultats gardés pour « ouvre le n-ième résultat »
summary_sources = 3               # résultats donnés à l'IA pour le résumé
snippet_chars = 200               # longueur maximale d'un extrait donné à l'IA
timeout_ms = 4000                 # délai de SearXNG
summarize = true                  # false : l'extrait seul, sans IA
local_summary_timeout_ms = 10000  # délai du résumé en mode local
online_summary_timeout_ms = 5000  # délai du résumé en mode en ligne
summary_max_tokens = 80           # longueur maximale du résumé
local_summary_model = ""          # modèle Ollama du résumé ; vide : celui de [local]
video_engines = ["youtube"]       # moteurs de SearXNG pour les vidéos
ytdlp = true                      # yt-dlp en secours si SearXNG ne trouve pas de vidéo
ytdlp_timeout_s = 8
```

Redémarrer le cerveau après un changement (`jarvisctl restart`).

## Essais

```bash
cargo test -p jarvis-brain --test search                 # bout en bout simulé, sans réseau
cargo test -p jarvis-brain --test search -- --ignored    # contre le SearXNG local réel
```
