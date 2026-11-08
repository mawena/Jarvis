# Essai 8.8 : personnes et rôles

**But :** vérifier qu'une **deuxième personne** (un invité) peut lancer la musique depuis sa
propre oreille, mais pas mettre le PC en veille ni lancer l'agent de code, que ta voix reste
reconnue partout, et que les changements de rôle ou de personne valent tout de suite, sans
redémarrer.

L'oreille de l'invité est une **deuxième oreille** : soit l'oreille d'essai sur le PC
(`jarvis-ear --profile essai`, voir [docs/reseau.md](../reseau.md#profil-de-test)), soit le
Pixel (voir [docs/android.md](../android.md)). Il faut aussi **une autre voix que la tienne**
pour les étapes 6 et 7 (un proche, ou le Pixel dans les mains de quelqu'un) : ta propre voix
est toujours reconnue, sur n'importe quelle oreille. Sans personne d'autre, fais ces étapes
seulement avec des mots très courts (voir « Bon à savoir »).

**Durée :** 25 minutes.

**Avant de commencer :**

- [ ] La [mise à jour](README.md#avant-tout-essai--mise-à-jour) est faite après la phase 8 :

  ```bash
  cd ~/Work/Mawena/Projects/Jarvis
  cargo run -q -p jarvisctl -- update
  ```

  Le cerveau et l'oreille **doivent** être mis à jour ensemble. `jarvisctl update` fait les
  deux.
- [ ] Le journal du cerveau est ouvert dans un terminal : `jarvisctl logs -f`.
- [ ] Un deuxième terminal est libre pour l'oreille de l'invité (si c'est l'oreille d'essai), un
  troisième pour les commandes.
- [ ] Tu es en mode **local** ou **en ligne**, comme tu veux : ne change rien pendant l'essai.

**Règle d'or :** ne réponds **jamais « oui »** à la mise en veille. Tu réponds « non ».

---

## Étape 1 : vérifier l'installation

- [ ] Lance :

  ```bash
  jarvisctl doctor
  ```

  **Attendu :** une ligne
  `✓ Personnes et rôles : 1 personne (Charles, admin), 1 rôle intégré`.

## Étape 2 : l'état de départ

- [ ] Lance :

  ```bash
  jarvis person list
  jarvis role list
  jarvis ears
  ```

  **Attendu :**
  - `person list` : une seule ligne, `Charles (propriétaire)`, rôle `admin`, avec l'oreille du
    PC, empreinte vocale `oui` ;
  - `role list` : une seule ligne, `admin`, outils `tout`, confirmer `non`, agent `non`,
    1 personne (c'est un rôle intégré : ses droits ne se lisent pas dans ces colonnes) ;
  - `ears` : colonne « Personne » avec `Charles` pour l'oreille du PC (ou `-` : elle vaut
    alors Charles).

## Étape 3 : créer le rôle et la personne

- [ ] Crée le rôle `famille` (musique, volume, applis, recherche, vidéos), puis la personne :

  ```bash
  jarvis role add famille --tools 'media_*,volume_up,volume_down,app_launch,web_search,open_results,play_video'
  jarvis person add Invité --role famille
  jarvis person list
  ```

  **Attendu :** « Rôle « famille » créé. », « Personne « Invité » ajoutée (rôle famille). »,
  puis deux lignes dans le tableau, `Invité` sans oreille (`-`) et sans empreinte vocale.

- [ ] Essaie une faute de frappe :

  ```bash
  jarvis role add test --tools 'musique_*'
  ```

  **Attendu :** une erreur « aucun outil ne correspond à « musique_* » », et `jarvis role list`
  ne montre pas `test`.

## Étape 4 : appairer l'oreille de l'invité

Avec l'oreille d'essai sur le PC (pour le Pixel, appaire-le comme dans l'essai 5.7, mais avec
`jarvis pair --person Invité`) :

- [ ] Dans le terminal de commandes :

  ```bash
  jarvis pair --person Invité
  ```

  **Attendu :** l'écran d'appairage habituel (code, QR code, adresse `jarvis://pair?…`).

- [ ] Dans le deuxième terminal :

  ```bash
  jarvis-ear --profile essai pair 'jarvis://pair?…'
  jarvis-ear --profile essai
  ```

- [ ] Lance :

  ```bash
  jarvis ears
  jarvis person list
  ```

  **Attendu :** la nouvelle oreille est connectée, avec `Invité` dans la colonne « Personne » ;
  `person list` montre cette oreille sur la ligne d'Invité.

- [ ] Lance `jarvisctl doctor`.

  **Attendu :** `✓ Personnes : 2 (Charles admin, Invité famille)` et **aucune** ligne
  « Oreille … sans personne reconnue ».

## Étape 5 : toi, sur ton oreille (rien ne change)

Pour que ce soit ton oreille du PC qui entende, coupe l'oreille d'essai le temps de cette
étape : `Ctrl+C` dans le deuxième terminal. Tu la relanceras à l'étape suivante.

- [ ] Dis : « Jarvis, quelle heure est-il ? »

  **Attendu :** la réponse, comme toujours. Dans le journal : une ligne « qui parle » avec
  `personne=Charles` et `confiance=Recognized`.

- [ ] Dis : « Jarvis, mets le PC en veille. »

  **Attendu :** Jarvis demande une confirmation.

- [ ] Réponds : « **non** ».

  **Attendu :** « Entendu, j'annule. », le PC reste allumé.

## Étape 6 : l'invité sur son oreille

Relance l'oreille d'essai (`jarvis-ear --profile essai`) et **coupe l'oreille du PC** pour que
seule celle de l'invité entende : `jarvisctl stop oreille`. L'invité parle à côté du micro
(ou au Pixel). Tu peux rester à côté, mais **ne parle pas**.

- [ ] L'invité lance une musique ou une vidéo (ou tu le fais toi-même avant), puis dit :
  « Jarvis, suivant. »

  **Attendu :** la musique passe au morceau suivant. Dans le journal : « qui parle » avec
  `personne=Invité` et `confiance=Unverified`.

- [ ] L'invité dit : « Jarvis, mets le PC en veille. »

  **Attendu :** Jarvis répond « Je ne peux pas faire ça pour vous. », bip d'échec, **aucune
  question de confirmation**, le PC reste allumé. Dans le journal : « droit refusé » avec
  `personne=Invité`, `role=famille`, `outil=suspend`.

- [ ] L'invité dit : « Jarvis, finis les tests du projet Jarvis. »

  **Attendu :** « Je ne peux pas faire ça pour vous. » (ou, si l'IA ne choisit pas l'outil de
  l'agent, une autre réponse sans rapport), et **aucune tâche de l'agent n'est lancée**. Vérifie
  qu'aucune fenêtre ni aucune tâche n'apparaît (journal : pas de « agent » démarré).

- [ ] L'invité dit : « Jarvis, mets le PC en veille » puis, si Jarvis ne répond pas par un
  refus mais par une question (ce serait un défaut grave), **ne dis pas oui** et note-le.

## Étape 7 : ta voix sur l'oreille de l'invité

L'oreille d'essai tourne toujours, l'oreille du PC est toujours coupée. Cette fois c'est **toi**
qui parles, à côté du micro de l'invité.

- [ ] Dis une phrase longue : « Jarvis, quelle heure est-il ? »

  **Attendu :** dans le journal, « qui parle » avec `personne=Charles`, `confiance=Recognized` :
  ta voix est reconnue même sur l'oreille de l'invité.

- [ ] Dis : « Jarvis, mets le PC en veille. »

  **Attendu :** Jarvis demande une confirmation (c'est bien toi).

- [ ] Réponds : « **non** ».

  **Attendu :** « Entendu, j'annule. »

Relance l'oreille du PC : `jarvisctl start oreille`.

## Étape 8 : changer le rôle à chaud

Sans redémarrer quoi que ce soit.

- [ ] Retire les applications du rôle :

  ```bash
  jarvis role edit famille --tools 'media_*'
  jarvis role list
  ```

  **Attendu :** « Rôle « famille » modifié. » ; `role list` montre `media_*` pour `famille`.

- [ ] L'invité dit : « Jarvis, lance VS Code. »

  **Attendu :** « Je ne peux pas faire ça pour vous. », VS Code ne s'ouvre pas.

- [ ] Remets les applications, sans redémarrer :

  ```bash
  jarvis role edit famille --tools 'media_*,volume_up,volume_down,app_launch'
  ```

  L'invité redit : « Jarvis, lance VS Code. »

  **Attendu :** VS Code s'ouvre (ferme-le ensuite). Si tu préfères ne rien ouvrir, essaie une
  autre application que tu as sous la main.

## Étape 9 : retirer la personne

- [ ] Lance :

  ```bash
  jarvis person remove Invité
  ```

  **Attendu :** refusé, avec un message « Invité a encore 1 oreille(s) : retirez-les d'abord. ».

- [ ] Lance :

  ```bash
  jarvis person remove Charles
  jarvis role remove famille
  ```

  **Attendu :** deux refus : « Charles est le propriétaire : impossible de la supprimer. » et
  « Le rôle famille est encore donné à 1 personne(s). ».

- [ ] Retire l'oreille puis la personne et le rôle :

  ```bash
  jarvis unpair <nom de l'oreille d'essai>
  jarvis person remove Invité
  jarvis role remove famille
  jarvis person list
  jarvis role list
  ```

  **Attendu :** chaque commande réussit ; il ne reste que Charles et `admin`. L'oreille d'essai,
  dans son terminal, est déconnectée puis refusée (`connexion refusée (UnknownEar)`).

## Étape 10 : ranger

- [ ] Arrête l'oreille d'essai (`Ctrl+C`) et supprime ses fichiers :

  ```bash
  rm -r ~/.local/share/jarvis/ear-essai ~/.local/state/jarvis/ear-essai-muted 2>/dev/null
  jarvis ears
  jarvisctl doctor
  ```

  **Attendu :** `jarvis ears` ne montre plus que l'oreille du PC, connectée ; `doctor` dit de
  nouveau `✓ Personnes et rôles : 1 personne (Charles, admin), 1 rôle intégré`, sans
  avertissement. Vérifie aussi que `jarvisctl status` montre l'oreille du PC en route.

## Facultatif : le mode dégradé

À ne faire que si tu en as envie : il touche au fichier des personnes.

- [ ] Sauvegarde le fichier, abîme-le et redémarre :

  ```bash
  cp ~/.local/share/jarvis/people.toml /tmp/people.toml.sauvegarde
  echo 'ceci n est pas du toml [' >> ~/.local/share/jarvis/people.toml
  jarvisctl restart cerveau
  jarvisctl doctor
  ```

  **Attendu :** `! Annuaire des personnes illisible (…) — mode dégradé : seule votre voix
  reconnue et le texte tapé ont des droits …`. Ta voix marche toujours ; `jarvis person add …`
  est refusé (« L'annuaire est illisible : modifications refusées. »).

- [ ] Remets le fichier et redémarre :

  ```bash
  cp /tmp/people.toml.sauvegarde ~/.local/share/jarvis/people.toml
  jarvisctl restart cerveau
  jarvisctl doctor
  ```

  **Attendu :** la ligne « Personnes et rôles » est de retour, sans `!`.

---

## Résultats

Recopie ce bloc, remplis-le et envoie-le-moi.

```
Étape 1 doctor (ligne personnes) :
Étape 2 person list / role list / ears corrects ? :
Étape 3 rôle et personne créés ? faute de frappe refusée ? :
Étape 4 appairage avec --person OK ? colonne Personne ? doctor 2 personnes ? :
Étape 5 « non » bien pris sur ton oreille ? :
Étape 6 musique OK pour l'invité ? veille refusée (message exact) ? agent refusé ? :
Étape 6 voix de l'invité : qui était-ce (autre personne ou toi en mot court) ? :
Étape 7 ta voix reconnue sur l'oreille de l'invité ? question de veille posée ? :
Étape 8 rôle changé à chaud (refus puis réussite) sans redémarrer ? :
Étape 9 refus de suppression (oreille, Charles, rôle utilisé) ? suppression finale OK ? :
Étape 10 rangement OK ? doctor propre ? :
Mode dégradé (facultatif) :
Ce qui t'a surpris :
```

## Bon à savoir

- **Ta voix est reconnue sur toutes les oreilles.** Tant que c'est toi qui parles, avec une
  phrase d'au moins une seconde ou deux, Jarvis te traite comme Charles même sur l'oreille de
  l'invité. Pour tester l'invité pour de vrai, il faut une autre voix.
- **Les mots très courts** (« suivant », « oui ») ne suffisent pas à reconnaître une voix.
  Sur une oreille d'invité, ta propre voix en mot court est donc traitée comme celle de
  l'invité (raccourcis seulement) tant que tu n'as pas dit de phrase longue sur cette oreille
  dans les 3 minutes précédentes : c'est un moyen de tester l'invité sans personne d'autre
  (« Jarvis, suivant » marche, « Jarvis, mets le PC en veille » est trop long et te
  reconnaît).
- **Un invité ne confirme jamais** : même avec le droit `confirm`, sa voix n'est pas
  vérifiée. C'est voulu, en attendant les empreintes par personne.

## En cas de problème

- La phrase de l'invité est ignorée : vérifie que l'oreille d'essai est connectée
  (`jarvis ears`) et regarde la ligne « qui parle » du journal.
- Tout est refusé à l'invité, même la musique : regarde `jarvis role list` (le rôle doit
  contenir `media_*`) et la ligne « droit refusé » du journal, qui nomme l'outil.
- `jarvisctl doctor` parle d'un annuaire illisible : voir le mode dégradé ci-dessus, ou
  [docs/personnes.md](../personnes.md#mode-dégradé).
- Note simplement ce que tu as vu : je m'occupe du reste.
