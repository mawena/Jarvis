# Essai 3.8 : la voix de Jarvis

**But :** vérifier que Jarvis répond à voix haute, que les bips sont clairs, qu'on peut le couper
en disant « Jarvis », que la veille demande bien confirmation, et que « répète » marche.

**Durée :** 15 minutes.

**Avant de commencer :**

- [ ] La [mise à jour](README.md#avant-tout-essai--mise-à-jour) est faite : l'oreille tourne en
  service (icône Jarvis en haut de l'écran) et `jarvisctl logs -f` est ouvert dans un terminal.
- [ ] Spotify (ou un autre lecteur) joue de la musique, à un volume normal.
- [ ] Tu es à ta distance habituelle du micro.
- [ ] Ton travail est enregistré (à l'étape 5, tu pourras mettre le PC en veille pour de vrai,
  si tu le veux).

Partout ci-dessous, **dis** = parle à voix haute ; attends le bip après « Jarvis » avant de
continuer ta phrase.

---

## Étape 1 : le bip de réveil

- [ ] **Dis** : « Jarvis ». Puis ne dis plus rien.

**Attendu :**
- un bip court qui monte, aussitôt après « Jarvis » ;
- la musique baisse, puis remonte toute seule au bout de quelques secondes (rien n'a été dit).

## Étape 2 : une question

Cette phrase longue sert aussi à faire reconnaître ta voix pour les 3 minutes suivantes (voir
[les phrases très courtes](README.md#bon-à-savoir--les-phrases-très-courtes)).

- [ ] **Dis** : « Jarvis… quelle est la capitale du Pérou ? »

**Attendu :**
- Jarvis répond à voix haute, en une phrase courte (« Lima… ») ;
- la musique n'est pas changée ;
- dans le journal : `voix reconnue score="0.xx"`.

**À noter :** le temps entre la fin de ta question et le début de sa réponse, et le score.

## Étape 3 : une commande rapide

- [ ] **Dis** : « Jarvis… suivant ».

**Attendu :**
- bip de réveil, la musique baisse ;
- le morceau suivant démarre, et la musique remonte ;
- un bip de réussite (deux notes qui montent), puis Jarvis dit « Bien. » ;
- dans le journal : `texte entendu phrase=suivant` (ou proche).

**À noter :** le temps entre la fin de ton « suivant » et le changement de morceau
(au jugé : moins d'1 s ? 1-2 s ? plus ?).

**Si rien ne se passe** et que le journal dit `voix non reconnue` : note le message exact et le
score, refais l'étape 2, puis réessaie.

## Étape 3 bis : couper Jarvis en disant « Jarvis »

- [ ] **Dis** : « Jarvis… raconte-moi l'histoire de Paris. » Puis, **pendant qu'il parle**, dis
  « Jarvis ».

**Attendu :**
- sa voix s'arrête net, tu entends le bip de réveil ;
- tu peux enchaîner une nouvelle commande (par exemple « pause »).

## Étape 4 : « répète »

- [ ] **Dis** : « Jarvis… quelle heure est-il ? » Attends la réponse.
- [ ] **Dis** : « Jarvis… répète ».

**Attendu :** il redit exactement sa réponse précédente, sans rien refaire d'autre.

## Étape 5 : la veille, avec confirmation

### 5a. Refuser

- [ ] **Dis** : « Jarvis… mets le PC en veille ».

**Attendu :** Jarvis demande « Je mets le PC en veille, Monsieur ? ». Après sa question,
**ne redis pas « Jarvis »** : l'oreille écoute ta réponse directement (8 s au plus).

- [ ] **Dis** : « non ».

**Attendu :** il répond qu'il annule, et **le PC ne se met pas en veille**. Dans le journal :
`action annulée`.

### 5b. Ne rien répondre

- [ ] **Dis** : « Jarvis… mets le PC en veille », puis **ne dis rien** pendant 10 s.

**Attendu :** rien ne se passe, le PC reste allumé.

### 5c. Faire répéter la question

- [ ] **Dis** : « Jarvis… mets le PC en veille ». Après sa question, **dis** : « répète »
  (sans « Jarvis »).

**Attendu :** il repose la question une fois. **Dis** alors « non » : il annule.

**Si rien ne se passe** après ton « répète » : ton « répète », très court, n'a pas été reconnu
comme ta voix, et la question a été effacée par prudence (journal : `« répète » court non
vérifié, ignoré`). Note-le : ce n'est pas dangereux, mais on ajustera.

### 5d. Accepter (facultatif : le PC se met vraiment en veille)

Ne fais cette étape que si tu acceptes la mise en veille maintenant.

- [ ] **Dis** : « Jarvis… mets le PC en veille », puis « oui ».

**Attendu :**
- Jarvis dit sa phrase (par exemple « Bien, Monsieur. ») **en entier** ;
- environ 4 s plus tard, le PC se met en veille ;
- dans le journal : `réponse courte : voix reconnue (vérification stricte)` puis
  `action confirmée`.

**Si Jarvis répond** « Je n'ai pas bien reconnu votre voix. Dites « oui, vas-y » pour confirmer,
Monsieur. » : **dis** « oui, vas-y ». Note-le dans les résultats (ton « oui » court n'a pas été
reconnu : on ajustera le seuil, ou tu referas ton empreinte).

Au réveil du PC : vérifie que l'icône Jarvis est toujours là et que « Jarvis… suivant » marche
encore.

## Étape 6 : l'écho de Jarvis

- [ ] Monte le son des haut-parleurs un peu plus fort que d'habitude. **Dis** : « Jarvis… mets
  le PC en veille ». Pendant et juste après sa question, **reste silencieux**.

**Attendu :** l'oreille n'entend pas sa propre voix comme une réponse : rien n'est fait, le PC
reste allumé. Dans le journal, pas de `réponse entendue` avec sa propre phrase.

---

## Résultats

Remplis, puis colle-moi cette section.

| Étape | OK ? | Remarque |
| --- | --- | --- |
| 1. Bip de réveil | | |
| 2. Réponse parlée | | délai : … ; score : … |
| 3. « suivant » : morceau changé + « Bien. » | | délai : |
| 3 bis. Coupure par « Jarvis » | | |
| 4. « répète » | | |
| 5a. Veille refusée par « non » | | |
| 5b. Sans réponse : rien | | |
| 5c. « répète » pendant la question | | |
| 5d. Veille acceptée (facultatif) | | « oui » reconnu du premier coup ? |
| 6. Pas d'écho capté | | |

**La voix « pierre » te convient-elle ?** (sinon : plus aiguë, plus grave, plus rapide…)

**Les bips sont-ils trop forts, trop faibles, agréables ?**

**Une phrase coupée trop tôt après « Jarvis » ?** (oui / non, laquelle)

**Autre chose de bizarre :**
