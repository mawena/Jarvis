# Essai 2.18 : une autre voix est ignorée

**But :** vérifier que Jarvis n'obéit qu'à toi, et mesurer à quel point ta voix est bien
reconnue (pour régler le seuil si besoin).

**Durée :** 10 minutes.

**Il te faut :** une autre personne, **ou** une vidéo où quelqu'un dit « Jarvis » (par exemple
une scène d'Iron Man sur YouTube), jouée sur un autre appareil (téléphone) posé près du PC.

**Avant de commencer :**

- [ ] La [mise à jour](README.md#avant-tout-essai--mise-à-jour) est faite : `jarvis-ear` tourne.
- [ ] Le journal du cerveau est ouvert, avec seulement les lignes utiles :

  ```bash
  journalctl --user -u jarvis-brain -f | grep -E "voix|texte entendu|phrase reçue"
  ```

- [ ] De la musique joue (pour voir si une commande passe).

---

## Question rapide d'abord

Le 28 octobre à 19 h 10, juste après l'enregistrement de ton empreinte, une phrase a été
**rejetée avec un score de 0,34** (ta voix est acceptée à partir de 0,55).

- [ ] C'était : ☐ une autre voix / une vidéo  ☐ moi  ☐ je ne sais plus

Pas grave si tu ne sais plus : la suite de l'essai refait la mesure proprement.

## Étape 1 : une phrase longue

- [ ] **Dis** : « Jarvis… quelle est la capitale de l'Australie ? »

**Attendu :** il répond. Dans le journal : `voix reconnue score="0.xx"`.

**À noter :** le score.

## Étape 2 : ta voix, 5 commandes courtes

- [ ] **Dis** 5 fois, en laissant passer la réponse à chaque fois : « Jarvis… suivant ».

**Attendu :** les 5 fois, le morceau change. Le journal peut afficher un score
(`voix reconnue score="0.xx"`) ou rien de particulier : une commande aussi courte est acceptée
grâce à la phrase de l'étape 1 (voir
[les phrases très courtes](README.md#bon-à-savoir--les-phrases-très-courtes)).

**À noter :** combien de « suivant » ont marché sur 5, et les scores affichés.

## Étape 3 : une autre voix

- [ ] Fais dire à l'autre personne (ou fais jouer la vidéo) : « Jarvis… suivant », 3 fois.

Deux cas possibles, **tous deux corrects** :

- l'oreille ne réagit pas du tout (pas de bip) : le mot d'éveil, entraîné sur ta voix, ne
  reconnaît pas la sienne ;
- l'oreille fait le bip, mais **le morceau ne change pas**, et le journal affiche
  `voix non reconnue (score 0.xx)`.

**Problème si :** le morceau change. Note-le tout de suite.

**À noter :** pour chaque essai, bip ou pas, et le score affiché s'il y en a un.

## Étape 4 : une autre voix pendant une question

Seulement si une autre personne est là (pas avec une vidéo).

- [ ] **Toi**, dis : « Jarvis… mets le PC en veille ». Après sa question, **l'autre personne**
  dit « oui ».

**Attendu :** le PC **ne se met pas** en veille. Jarvis redemande « Dites « oui, vas-y » pour
confirmer », ou ne fait rien. Dans le journal : `voix non reconnue` ou
`réponse courte : voix non reconnue (vérification stricte)`.

- [ ] Ensuite, **toi**, dis « non » pour annuler proprement.

---

## Résultats

Remplis, puis colle-moi cette section.

**Phrase de 19 h 10 :** autre voix / moi / je ne sais plus

| Étape | Résultat |
| --- | --- |
| 1. Phrase longue | score : … |
| 2. Tes 5 « suivant » | réussis : … / 5 ; scores : … |
| 3. Autre voix, essai 1 | bip ? oui/non ; score : … ; morceau changé ? oui/non |
| 3. Autre voix, essai 2 | |
| 3. Autre voix, essai 3 | |
| 4. « oui » d'une autre voix | PC resté allumé ? oui/non |

**Remarques :**
