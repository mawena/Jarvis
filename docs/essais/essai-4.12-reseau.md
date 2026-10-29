# Essai 4.12 : les oreilles en réseau

**But :** vérifier que l'oreille du PC marche toujours, maintenant qu'elle passe par le réseau,
puis appairer une **deuxième oreille** et voir que Jarvis ne répond qu'**une seule fois** quand
les deux l'entendent. On vérifie aussi « le PC est endormi », le retrait d'une oreille et
l'appairage par code.

La deuxième oreille tourne **sur le PC lui-même**, dans un terminal (profil `essai`), avec le
même micro. L'application Android viendra à la phase 5 : en attendant, c'est le moyen le plus
simple d'avoir deux oreilles qui entendent la même phrase.

**Durée :** 30 minutes.

**Avant de commencer :**

- [ ] La [mise à jour](README.md#avant-tout-essai--mise-à-jour) est faite après la phase 4 :

  ```bash
  cd ~/Work/Mawena/Projects/Jarvis
  cargo run -q -p jarvisctl -- update
  ```

  Le cerveau et l'oreille **doivent** être mis à jour ensemble : une ancienne oreille ne sait
  plus parler au nouveau cerveau. `jarvisctl update` fait les deux.
- [ ] Le journal du cerveau est ouvert dans un terminal : `jarvisctl logs -f`.
- [ ] Un deuxième terminal est libre pour la deuxième oreille, un troisième pour les commandes.

---

## Étape 1 : vérifier l'installation

- [ ] Lance :

  ```bash
  jarvisctl doctor
  ```

  **Attendu**, dans la partie réseau :
  - « Certificat du cerveau » avec une empreinte courte (`A1B2-C3D4-…`) ;
  - le port 7341 en écoute ;
  - l'oreille du PC appairée.

  Une ligne « Pare-feu ufw actif » n'empêche pas cet essai (tout reste sur le PC). Note-la
  quand même dans les résultats.

- [ ] Lance :

  ```bash
  jarvis ears
  ```

  **Attendu :** une ligne pour l'oreille du PC, marquée connectée.

## Étape 2 : l'oreille du PC, comme avant

- [ ] Dis : « Jarvis, quelle heure est-il ? »

  **Attendu :** le bip, puis la réponse, aussi vite qu'avant la mise à jour. Dans le journal :
  « oreille élue », puis « texte entendu ».

- [ ] Lance une musique (Spotify ou une vidéo), puis dis : « Jarvis, suivant. »

  **Attendu :** la musique baisse pendant que tu parles, passe au morceau suivant et revient à
  son volume normal.

## Étape 3 : appairer la deuxième oreille avec l'adresse

- [ ] Dans le terminal de commandes :

  ```bash
  jarvis pair
  ```

  **Attendu :** un code à 6 chiffres, un QR code, une adresse `jarvis://pair?…`, l'empreinte
  courte du PC et un compte à rebours de 2 minutes. Laisse-le ouvert.

- [ ] Dans le deuxième terminal, colle l'adresse **entre apostrophes** :

  ```bash
  jarvis-ear --profile essai pair 'jarvis://pair?…'
  ```

  **Attendu :** l'oreille dit qu'elle est appairée. Dans le premier terminal, `jarvis pair`
  s'arrête tout seul en disant qu'une oreille s'est appairée.

- [ ] Lance `jarvis ears`.

  **Attendu :** deux oreilles. La nouvelle porte le nom de la machine, et elle est **non
  connectée** (elle n'est pas encore lancée).

## Étape 4 : deux oreilles, une seule réponse

- [ ] Dans le deuxième terminal, lance la deuxième oreille et laisse-la tourner :

  ```bash
  jarvis-ear --profile essai
  ```

  **Attendu :** elle se connecte en quelques secondes. Si une deuxième icône Jarvis apparaît en
  haut de l'écran, c'est normal. `jarvis ears` (dans le troisième terminal) montre les deux
  oreilles connectées.

- [ ] Dis : « Jarvis, quelle heure est-il ? »

  **Attendu :**
  - tu entends **une seule** réponse, pas deux, et pas de bip d'échec ;
  - dans le journal du cerveau, **une seule** ligne « oreille élue » pour cette phrase.

- [ ] Recommence 4 ou 5 fois avec des phrases courtes (« Jarvis, quel jour on est ? »,
  « Jarvis, suivant »).

  **Attendu :** toujours une seule réponse. L'oreille élue peut changer d'une fois à l'autre :
  les deux entendent le même micro, c'est la plus forte qui gagne.

- [ ] Avec la musique lancée, dis : « Jarvis, plus fort. »

  **Attendu :** la musique baisse pendant que tu parles, le volume monte une fois, puis la
  musique reprend à son nouveau volume. Elle ne doit **pas** rester basse.

## Étape 5 : une confirmation sur la même oreille

Les deux oreilles tournent toujours.

- [ ] Dis : « Jarvis, mets le PC en veille. »

  **Attendu :** Jarvis demande une confirmation.

- [ ] Réponds : « **non** ». Ne dis pas « oui » : le PC se mettrait en veille.

  **Attendu :** Jarvis annule, une seule fois, et le PC reste allumé.

## Étape 6 : le PC est endormi

- [ ] Dans le troisième terminal, arrête le cerveau seul :

  ```bash
  jarvisctl stop cerveau
  ```

- [ ] Attends 5 secondes, puis dis : « Jarvis, quelle heure est-il ? »

  **Attendu :** en moins de 2 secondes, la voix de Jarvis dit que le PC est endormi (ou un bip
  d'échec). L'icône de l'oreille du PC dit « PC injoignable » ou « connexion au PC… ».

- [ ] Relance le cerveau :

  ```bash
  jarvisctl start cerveau
  ```

  **Attendu :** en moins d'une minute, les deux oreilles se reconnectent seules (`jarvis ears`
  les montre connectées, l'icône redevient normale). « Jarvis, quelle heure est-il ? » marche de
  nouveau, avec une seule réponse.

## Étape 7 : retirer la deuxième oreille

- [ ] Dans `jarvis ears`, repère le nom de la deuxième oreille, puis :

  ```bash
  jarvis unpair <nom de la deuxième oreille>
  ```

  **Attendu :** dans le deuxième terminal, la deuxième oreille est déconnectée tout de suite et
  dit que l'appairage est refusé. Elle ne se reconnecte pas.

- [ ] Arrête-la avec `Ctrl+C`, puis dis : « Jarvis, quelle heure est-il ? »

  **Attendu :** l'oreille du PC répond normalement.

## Étape 8 : appairer avec le code seul

- [ ] Dans le troisième terminal, lance `jarvis pair` et note le code et l'empreinte courte.

- [ ] Dans le deuxième terminal, **tape un faux code** (par exemple `000000`) :

  ```bash
  jarvis-ear --profile essai pair --code 000000
  ```

  **Attendu :** l'oreille trouve le PC, montre son empreinte et demande « Est-ce la même que sur
  le PC ? [o/N] ». Réponds `o`. L'appairage est refusé : « code faux (encore 2 essai(s)) »
  (3 essais par machine), et `jarvis pair` reste ouvert.

- [ ] Recommence avec le **bon code** :

  ```bash
  jarvis-ear --profile essai pair --code <le code affiché>
  ```

  **Attendu :** l'empreinte affichée est **la même** que celle de `jarvis pair`. Réponds `o`.
  L'oreille est appairée et `jarvis pair` s'arrête.

  Si la découverte ne trouve pas le PC, note-le et ajoute `--address 127.0.0.1`.

- [ ] Lance `jarvis-ear --profile essai`, dis « Jarvis, quelle heure est-il ? » une fois
  (toujours une seule réponse), puis arrête-la avec `Ctrl+C`.

## Étape 9 : ranger

- [ ] Retire la deuxième oreille et ses fichiers :

  ```bash
  jarvis unpair <nom de la deuxième oreille>
  rm -r ~/.local/share/jarvis/ear-essai ~/.local/state/jarvis/ear-essai-muted 2>/dev/null
  jarvis ears
  ```

  **Attendu :** `jarvis ears` ne montre plus que l'oreille du PC, connectée.

## Facultatif : une autre machine Linux

Si tu as un autre ordinateur Linux sur le même Wi-Fi, avec Jarvis compilé dessus :

- [ ] Sur le PC, si `jarvisctl doctor` signale ufw, ouvre le port (voir
  [docs/reseau.md](../reseau.md#mise-en-route)).
- [ ] Sur l'autre machine : `jarvis-ear pair --code <code>`, puis `jarvis-ear`.
- [ ] Parle près de l'une ou de l'autre machine : c'est **celle où tu es** qui répond.

---

## Résultats

Recopie ce bloc, remplis-le et envoie-le-moi.

```
Étape 1 doctor (lignes réseau) :
Étape 1 jarvis ears :
Étape 2 réponse et musique OK ? :
Étape 3 appairage par adresse OK ? :
Étape 4 une seule réponse sur combien d'essais ? (ex. 5/5) :
Étape 4 musique revenue à la normale ? :
Étape 5 « non » bien pris, une seule fois ? :
Étape 6 « PC endormi » entendu ? en combien de temps ? :
Étape 6 reconnexion des deux oreilles ? en combien de temps ? :
Étape 7 deuxième oreille refusée, PC toujours OK ? :
Étape 8 faux code refusé ? bon code accepté ? empreintes identiques ? :
Étape 9 rangement OK ? :
Autre machine (facultatif) :
Ce qui t'a surpris :
```
