# Essai 5.7 : l'oreille Android

**But :** vérifier que le téléphone (Pixel 4 XL) marche comme une oreille de Jarvis : il
s'appaire, enregistre « Jarvis », écoute écran allumé puis éteint, **depuis une autre pièce**, se
coordonne avec l'oreille du PC (une seule réponse), obéit aux boutons de sa notification, repart
après un redémarrage et tient une nuit. On relève aussi le **score de ta voix** sur le micro du
téléphone : l'empreinte a été prise avec le micro du PC, il peut être plus bas.

**Durée :** 45 minutes, plus une nuit branché et une nuit sur batterie.

**Avant de commencer :**

- [ ] La [mise à jour](README.md#avant-tout-essai--mise-à-jour) est faite après la fusion de la
  phase 5 :

  ```bash
  cd ~/Work/Mawena/Projects/Jarvis
  cargo run -q -p jarvisctl -- update
  ```

- [ ] Le cerveau et l'oreille du PC tournent. Tu avais quitté l'oreille du PC : relance-la.

  ```bash
  jarvisctl status
  jarvisctl start oreille      # si elle est arrêtée
  ```

- [ ] Le pare-feu laisse passer les oreilles. `jarvisctl doctor` : si une ligne « Pare-feu ufw
  actif » apparaît, lance les deux commandes `sudo ufw allow …` qu'elle donne.
- [ ] Le téléphone est sur le **même Wi-Fi** que le PC, en mode développeur avec le débogage
  USB, branché en USB au PC (voir [docs/android.md](../android.md#ce-quil-faut)).
- [ ] Le journal du cerveau est ouvert dans un terminal : `jarvisctl logs -f`.

---

## Étape 1 : construire et installer

- [ ] Construis l'appli (premier passage en ligne) :

  ```bash
  scripts/android-build.sh
  ```

  **Attendu :** une ligne `APK : …` d'environ 9 Mo, sans erreur.

- [ ] Installe-la et lance-la :

  ```bash
  scripts/android-install.sh
  ```

  **Attendu :** l'appli « Jarvis » s'ouvre sur le téléphone, avec cinq étapes : Autorisations,
  Appairage, Mot d'éveil, Batterie, Démarrage seul. Autorisations, Batterie et Démarrage seul
  sont déjà accordés (le script s'en est chargé). « Démarrer l'écoute » est grisé :
  « Terminez les étapes 1 à 3 pour démarrer l'écoute ».

## Étape 2 : appairer

- [ ] Sur le PC :

  ```bash
  jarvis pair
  ```

- [ ] Sur le téléphone : étape 2 > « Appairer » > « Scanner le QR code », vise le QR code du
  terminal. (Si le scan échoue, colle l'adresse `jarvis://pair?…` dans le champ prévu.) Laisse
  le nom « Pixel 4 XL » et valide avec « Appairer ».

  **Attendu :** le téléphone dit « Appairé à « … » » ; `jarvis pair` s'arrête en disant
  qu'une oreille s'est appairée.

- [ ] Lance :

  ```bash
  jarvis ears
  ```

  **Attendu :** une ligne « Pixel 4 XL », plateforme `android`, connectée **non** (l'écoute
  n'est pas encore démarrée) ; l'oreille du PC est connectée.

## Étape 3 : enregistrer « Jarvis »

- [ ] Sur le téléphone : étape 3 > « Enregistrer ». Tiens-le **à la distance où tu t'en
  serviras**, dans une pièce calme, et dis « Jarvis » à chaque prise (« Préparez-vous… », puis
  « Dites « Jarvis » » pendant 1,5 s). 8 prises.

  **Attendu :** « Mot d'éveil prêt (8 prises) ». Si une prise est « Trop faible », rapproche-toi :
  la même prise recommence. Touche « Terminer ».

## Étape 4 : démarrer l'écoute

- [ ] Touche « Démarrer l'écoute ».

  **Attendu :** une notification « À l'écoute · mode local » et le **point vert du micro**
  d'Android en haut de l'écran. `jarvis ears` montre « Pixel 4 XL » connectée.

## Étape 5 : écran allumé, même pièce

- [ ] Au calme, à côté du téléphone, dis : « Jarvis, quelle heure est-il ? »

  **Attendu :** le bip puis la réponse **sur le téléphone** (pas sur le PC), une seule fois.

- [ ] Relève le score de ta voix :

  ```bash
  jarvisctl logs -n 300 | grep voix
  ```

  **Attendu :** une ligne `voix reconnue oreille=Pixel 4 XL score=… seuil=…`. Note le score et
  le seuil. Si c'est `voix non reconnue`, note-le aussi : la phrase a été ignorée, ce qui est
  une information utile (pas une panne).

## Étape 6 : écran éteint, téléphone posé

- [ ] Lance une musique sur le PC. Éteins l'écran du téléphone et pose-le sur une table. Dis :
  « Jarvis, suivant. »

  **Attendu :** la musique du PC passe au morceau suivant. (Si la commande est ignorée avec
  `voix non reconnue (phrase trop courte)`, fais d'abord une vraie question, voir
  [le rappel](README.md#bon-à-savoir--les-phrases-très-courtes).)

## Étape 7 : depuis une autre pièce, écran éteint

C'est le critère principal de la phase.

- [ ] Va dans une **autre pièce** que le PC avec le téléphone (écran éteint, dans ta poche ou
  posé). Demande une action sur le PC, par exemple « Jarvis, mets en pause » (ou « Jarvis,
  suivant »), puis une question : « Jarvis, quelle heure est-il ? »

  **Attendu :** l'action a lieu sur le PC, la réponse parlée sort **du téléphone**. Note le
  délai ressenti entre ta phrase et la réponse, et si le mot d'éveil a dû être répété.

## Étape 8 : deux oreilles dans la même pièce

- [ ] Reviens près du PC, avec le téléphone (écran allumé ou non). Dis : « Jarvis, suivant. »

  **Attendu :** une seule exécution (un seul morceau passé) et la réponse sur **une seule**
  oreille. Dans le journal du cerveau : une seule ligne « oreille élue ». Recommence 3 fois
  avec « Jarvis, quel jour on est ? ».

## Étape 9 : les boutons de la notification

Sans ouvrir l'appli, depuis la notification (déroule-la).

- [ ] Touche « **Couper l'écoute** ». Dis « Jarvis, quelle heure est-il ? »

  **Attendu :** rien ne se passe ; la notification dit « Écoute coupée · … » et propose
  « Réactiver l'écoute ». Dis ensuite « Jarvis, réactive l'écoute » : l'écoute revient.

- [ ] Touche « **Coupure totale** ».

  **Attendu :** le point vert du micro disparaît ; la notification dit « Micro coupé (coupure
  totale) · … » et ne propose plus que « Rouvrir le micro ».

- [ ] Touche « **Rouvrir le micro** ».

  **Attendu :** le point vert revient, « À l'écoute · … ».

## Étape 10 : le PC est endormi

- [ ] Arrête le cerveau seul :

  ```bash
  jarvisctl stop cerveau
  ```

- [ ] Attends 5 secondes, puis dis : « Jarvis, quelle heure est-il ? »

  **Attendu :** après le bip, en 2 secondes environ, Jarvis dit que le PC est endormi (sur le
  téléphone) ; la notification dit « … · PC injoignable ».

- [ ] Relance le cerveau :

  ```bash
  jarvisctl start cerveau
  ```

  **Attendu :** en moins d'une minute la notification redevient « À l'écoute · mode local » seule,
  sans rien toucher ; `jarvis ears` montre le téléphone connecté.

## Étape 11 : redémarrer le téléphone

- [ ] Redémarre le téléphone, déverrouille-le, sans ouvrir l'appli.

  **Attendu :** l'écoute repart seule (notification « À l'écoute · … » et point vert). Si la
  superposition n'était pas accordée : une notification « Jarvis : touchez pour démarrer
  l'écoute » ; touche-la. Note ce qui s'est passé et le temps pris.

- [ ] Si la notification « Micro bloqué par Android : touchez pour relancer » apparaît, touche-la :
  **Attendu :** l'écoute repart. Note-le.

## Étape 12 : une nuit branché, une nuit sur batterie

- [ ] Soir 1 : téléphone **branché**, écoute lancée. Le matin, dis « Jarvis, quelle heure
  est-il ? ».

  **Attendu :** il répond, la notification est toujours « À l'écoute ».

- [ ] Soir 2 : téléphone **débranché**, écoute lancée. Note le pourcentage de batterie au
  coucher et au réveil, et l'heure. Le matin, même question.

  **Attendu :** il répond encore. Calcule le pourcentage perdu par heure.

## Étape 13 : la journée

- [ ] Garde l'écoute un jour. Note les fausses détections (le téléphone qui bipe sans que tu aies
  dit « Jarvis »), les phrases ignorées, les scores de voix :

  ```bash
  jarvisctl logs -n 1000 | grep voix
  ```

## Si ça ne va pas

- Journal du téléphone : `scripts/android-install.sh --logs` (lignes `Jarvis`, `JarvisRust`).
- Le mot d'éveil détecte mal : réenregistre-le plus près, ou copie celui du PC avec
  `scripts/android-install.sh --copy-wakeword`, ou change le seuil dans les réglages de l'appli
  (au prochain démarrage de l'écoute).
- L'appairage échoue : même Wi-Fi ? `jarvis pair` encore ouvert (2 minutes) ? pare-feu ?

## Ranger (facultatif)

- [ ] Pour retirer le téléphone : `jarvis unpair "Pixel 4 XL"`, puis « Désappairer » dans l'appli.

---

## Résultats

Recopie ce bloc, remplis-le et envoie-le-moi.

```
Étape 1 build et install OK ? taille de l'APK :
Étape 2 appairage par QR code OK ? jarvis ears (ligne du téléphone) :
Étape 3 8 prises OK ? prises « trop faible » :
Étape 4 notification et point vert OK ? :
Étape 5 réponse sur le téléphone ? score de la voix / seuil / reconnue ou non :
Étape 6 « suivant » écran éteint OK ? :
Étape 7 autre pièce, écran éteint : action OK ? réponse sur le téléphone ? délai ressenti :
Étape 8 une seule réponse sur combien d'essais ? (ex. 4/4) :
Étape 9 « Couper l'écoute » / « Coupure totale » / « Rouvrir le micro » OK ? :
Étape 10 « PC endormi » entendu ? en combien de temps ? reconnexion ? :
Étape 11 redémarrage : écoute repartie seule ? notification à toucher ? micro bloqué ? :
Étape 12 batterie : % perdu par heure (nuit sur batterie), répond le matin ? :
Étape 13 fausses détections, scores de voix, phrases ignorées :
Ce qui t'a surpris ou a raté :
```
