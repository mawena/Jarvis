# Essai 9.7 : mesurer les critères de réussite

**But :** vérifier sur une vraie journée les critères de réussite de la conception (§9) :
vitesse des réponses, moins d'une fausse détection par jour, aucune double exécution. Le
cerveau note chaque phrase dans `~/.local/state/jarvis/mesures.jsonl` (heure, oreille, durée,
route, issue, texte ; **jamais de son**) et `jarvisctl mesures` en fait le résumé. Cet essai
s'ajoute à l'[essai 2.15](essai-2.15-fausses-detections.md) : il compte la même chose, sans
lecture du journal à la main.

**Durée :** une journée normale, puis 10 minutes le soir.

---

## Le matin

- [ ] **1.** Mets Jarvis à jour (version 0.4.0 ou plus) : `jarvisctl update`.
- [ ] **2.** Vérifie que tout tourne : `jarvisctl doctor` (cerveau et oreille actifs).
- [ ] **3.** Repars d'un fichier vide pour ne compter que cette journée :

  ```bash
  rm -f ~/.local/state/jarvis/mesures.jsonl ~/.local/state/jarvis/mesures.jsonl.1
  ```

## Pendant la journée

- Utilise le PC normalement : musique, vidéos, conversations.
- Fais au moins **dix fois** chacune de ces phrases, depuis l'oreille du PC :
  - « Jarvis, suivant » (musique en cours) ;
  - « Jarvis, lance VS Code » ;
  - une question libre en mode local (« Jarvis, cherche la météo de demain ») ;
  - la même en mode en ligne (menu de l'icône → Mode en ligne), si la clé Claude est en place.
- Au moins **cinq fois**, avec le Pixel allumé dans la même pièce que le PC, dis « Jarvis,
  suivant » : une seule des deux oreilles doit agir.
- Note à part combien de fois **tu** as dit « Jarvis » en tout : ……

## Le soir

- [ ] **4.** Lance le résumé :

  ```bash
  jarvisctl mesures --jours 1
  ```

  Attendu : un tableau par route, avec la médiane et le 90e centile des durées, puis les
  réveils sans phrase, les phrases écartées et les doubles exécutions, chacun marqué ✓ ou ✗.
- [ ] **5.** Le critère « suivant en moins d'une seconde » se lit à part :

  ```bash
  jarvisctl mesures --jours 1 --phrase suivant
  ```

  Attendu : la ligne « raccourci » a une médiane et un 90e centile **sous 1,0 s**.
- [ ] **6.** Compare « Réveils sans phrase » à ton propre compte : un réveil où tu n'avais
  rien dit est une fausse détection. Un réveil où tu as parlé mais rien n'a été compris compte
  aussi ici : retire-les si tu les as notés.

## Résultats

Colle ici la sortie des deux commandes, puis :

- Nombre de « Jarvis » dits par toi : ……
- Fausses détections retenues : ……
- Double exécution vue ou entendue (oui / non) : ……
- Remarques :
