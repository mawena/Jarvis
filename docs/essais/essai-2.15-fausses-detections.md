# Essai 2.15 : fausses détections sur une journée

**But :** compter combien de fois l'oreille croit entendre « Jarvis » alors que personne ne l'a
dit (musique, vidéos, conversations). L'objectif est **moins d'une fausse détection par jour**.

**Durée :** une journée normale de travail. Tu n'as rien à faire pendant la journée, à part
noter quand **tu** dis « Jarvis ».

---

## Le matin : démarrer l'enregistrement du journal

- [ ] **1.** Quitte l'oreille si elle tourne (icône Jarvis → **Quitter l'oreille**).
- [ ] **2.** Relance-la en gardant son journal dans un fichier :

  ```bash
  jarvis-ear 2>&1 | tee -a ~/jarvis-essai-2.15.log
  ```

  Laisse ce terminal ouvert toute la journée (tu peux le réduire).
- [ ] **3.** Note l'heure de départ : ……

Le fichier ne contient que des lignes de texte (heure, « mot d'éveil détecté », phrase
entendue) : **aucun son n'est enregistré**.

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
  grep -c "mot d'éveil détecté" ~/jarvis-essai-2.15.log
  ```

- [ ] **6.** Affiche-les avec leur heure et leur score, pour repérer les fausses :

  ```bash
  grep "mot d'éveil détecté" ~/jarvis-essai-2.15.log
  ```

- [ ] **7.** Vois ce que le cerveau a entendu après chaque détection (les fausses donnent
  souvent rien, un bout de paroles de chanson, ou « voix non reconnue ») :

  ```bash
  journalctl --user -u jarvis-brain --since "08:00" | grep -E "voix|texte entendu"
  ```

  (remplace `08:00` par ton heure de départ)

- [ ] **8.** Calcule : **fausses détections = détections (étape 5) − tes « Jarvis » (tes marques)**.

- [ ] **9.** Quitte l'oreille avec `Ctrl+C` dans son terminal, puis relance-la normalement avec
  `jarvis-ear` si tu veux continuer à t'en servir.

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
