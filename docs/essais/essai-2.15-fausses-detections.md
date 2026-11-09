# Essai 2.15 : fausses détections sur une journée

**But :** compter combien de fois l'oreille croit entendre « Jarvis » alors que personne ne l'a
dit (musique, vidéos, conversations). L'objectif est **moins d'une fausse détection par jour**.

**Durée :** une journée normale de travail. Tu n'as rien à faire pendant la journée, à part
noter quand **tu** dis « Jarvis ».

---

## Le matin : vérifier que l'oreille tourne

L'oreille tourne en service : son journal est gardé par le système, il n'y a ni terminal à
laisser ouvert ni fichier à créer.

- [ ] **1.** Vérifie que l'icône Jarvis est en haut de l'écran. Sinon :
  `jarvisctl start oreille`.
- [ ] **2.** Vérifie que le service est actif :

  ```bash
  jarvisctl status oreille
  ```

  Attendu : « Oreille   : active (pid …, depuis …) ».
- [ ] **3.** Note l'heure de départ : ……

Le journal ne contient que des lignes de texte (heure, « mot d'éveil détecté », phrase
entendue) : **aucun son n'est enregistré**. Ne quitte pas l'oreille dans la journée (pas de
« Quitter l'oreille ») : les détections ne seraient plus comptées.

## Pendant la journée

- Utilise le PC normalement : musique, vidéos, appels, conversations autour de toi.
- **Chaque fois que tu dis « Jarvis » toi-même**, fais une marque (papier, ou un compteur sur ton
  téléphone). C'est ce qui permet de séparer tes vrais appels des fausses détections.
- Si tu remarques un bip alors que personne n'a dit « Jarvis », note l'heure et ce qui passait
  (musique, vidéo, conversation…). Facultatif, mais utile.

## Le soir : compter

- [ ] **4.** Note l'heure de fin : ……
- [ ] **5.** Compte toutes les détections de la journée :

  ```bash
  journalctl --user -u jarvis-ear --since "08:00" | grep -c "mot d'éveil détecté"
  ```

  (remplace `08:00` par ton heure de départ, ici et aux étapes 6 et 7)

- [ ] **6.** Affiche-les avec leur heure et leur score, pour repérer les fausses :

  ```bash
  journalctl --user -u jarvis-ear --since "08:00" | grep "mot d'éveil détecté"
  ```

- [ ] **7.** Vois ce que le cerveau a entendu après chaque détection (les fausses donnent
  souvent rien, un bout de paroles de chanson, ou « voix non reconnue ») :

  ```bash
  journalctl --user -u jarvis-brain --since "08:00" | grep -E "voix|texte entendu"
  ```

- [ ] **8.** Calcule : **fausses détections = détections (étape 5) − tes « Jarvis » (tes marques)**.

Rien à arrêter : l'oreille continue de tourner normalement.

---

## Résultats

Remplis, puis colle-moi cette section (et, si tu veux bien, le résultat de l'étape 6).

| | |
| --- | --- |
| Heures | de … à … |
| Détections au total (étape 5) | |
| Tes « Jarvis » (tes marques) | |
| **Fausses détections** | |
| Fausses détections qui ont **fait quelque chose** (musique changée, réponse…) | |
| Ce qui passait lors des fausses détections | |

**Remarques :**

Si le nombre de fausses détections dépasse 1, je relèverai le seuil du mot d'éveil (réglage
`threshold` de `~/.config/jarvis/ear.toml`) ou je te proposerai de refaire les prises de
« Jarvis ».
