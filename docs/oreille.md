# L'oreille du PC

`jarvis-ear` écoute le micro du PC, détecte « Jarvis », puis envoie la phrase au cerveau
(`jarvis-brain`). Tout reste local : la reconnaissance vocale (Whisper) et la vérification de
ta voix tournent sur la machine. Rien ne quitte l'oreille avant le mot d'éveil, et les phrases
ne sont jamais enregistrées : seules les prises de « Jarvis » de l'enrôlement restent sur disque.

## Prérequis

```bash
sudo apt install libasound2-dev libclang-dev    # pour compiler
scripts/download-models.sh all                  # Whisper (small, base), empreinte vocale, voix de Jarvis
jarvisctl install                               # ou `jarvisctl update` ; copie aussi les bibliothèques
```

Les modèles vont dans `~/.local/share/jarvis/models/`. Les bibliothèques de sherpa-onnx sont
copiées dans `~/.local/lib/jarvis/`. Le script accepte aussi une seule cible : `small` (modèle
Whisper par défaut, rapide et fiable sur ce PC), `base` (plus léger mais moins fiable en
français, à éviter), `speaker` (empreinte vocale) ou `voice` (voix de Jarvis).

## Premier enrôlement

```bash
jarvis-ear enroll
```

La commande se déroule en deux temps. Parle normalement, à ta distance habituelle du micro.

1. **Mot d'éveil** : tu dis « Jarvis » plusieurs fois, après chaque appui sur Entrée (8 prises
   par défaut, `--count N` pour en changer). Le modèle du mot d'éveil est construit à partir
   de ces prises.
2. **Empreinte vocale** : tu lis 6 phrases courtes affichées à l'écran (environ 3,5 s chacune).
   Elles restent en mémoire, ne sont jamais écrites sur disque, et partent au cerveau avec les
   prises « Jarvis » pour calculer ton empreinte. Le service `jarvis-brain` doit tourner.

`jarvis-ear enroll --voice-only` refait seulement l'empreinte vocale (les 6 phrases), sans
toucher au mot d'éveil. Tu peux relancer l'une ou l'autre à tout moment.

Avec l'empreinte, seule ta voix déclenche une action : une autre voix est ignorée sans être
transcrite. Une phrase de moins d'une seconde est trop courte pour reconnaître une voix : elle
n'est acceptée que dans les 3 minutes qui suivent une phrase reconnue, et seulement pour les
commandes rapides (« suivant », « pause », « coupe l'écoute »…), jamais pour une question à
l'IA. Hors de ces 3 minutes, elle est ignorée.

Si l'empreinte existe mais que le fichier est abîmé, ou que le modèle d'empreinte manque,
**toutes les phrases sont refusées** (Jarvis répond « La vérification de votre voix est
indisponible. ») : refais `jarvis-ear enroll --voice-only` ou `scripts/download-models.sh
speaker`. Sans empreinte du tout (installation neuve), toutes les voix sont acceptées et un
avertissement est journalisé.

## Lancer l'oreille

```bash
jarvis-ear
```

Une icône « Jarvis » apparaît dans la zone de notification. Dis « Jarvis », attends que la
musique baisse, puis donne ta commande. La phrase se termine après 800 ms de silence. Sans
parole 4 s après « Jarvis », elle est abandonnée ; une phrase dure 10 s au plus.

## Icône et menu

- **Couper l'écoute / Réactiver l'écoute** : le micro reste ouvert et le mot d'éveil reste
  détecté, mais seule la phrase « réactive l'écoute » est retenue (et seulement avec ta voix).
- **Coupure totale du micro / Rouvrir le micro** : le flux du micro est fermé. Seul ce menu le
  rouvre.
- **Mode local / Mode en ligne** : change le mode du cerveau.
- **Dernière phrase** : le dernier texte entendu (affichage seulement, non cliquable).
- **Quitter l'oreille**. Si une phrase était en cours, la musique est d'abord remise à son
  volume.

La première ligne du menu résume l'état : écoute, micro coupé, cerveau injoignable, mode et
modèle.

L'écoute coupée est retenue au redémarrage de l'oreille (fichier `~/.local/state/jarvis/ear-muted`).
La coupure totale, elle, ne l'est pas : l'oreille redémarre micro ouvert.

Si la musique reste baissée (oreille ou cerveau arrêté au mauvais moment), elle remonte seule
au bout de 15 s, à l'arrêt du cerveau, ou à son redémarrage.

## Commandes vocales

Commandes rapides, comprises sans IA (toujours précédées de « Jarvis ») :

- Écoute : « coupe l'écoute » (ou « coupe le micro », « arrête d'écouter ») ; « réactive
  l'écoute » (ou « réactive le micro »).
- Musique et vidéo : « pause », « reprends », « suivant », « précédent », « plus fort »,
  « moins fort », « qu'est-ce qui passe ? ».
- Applications : « ouvre Spotify », « lance VS Code »…
- Mode : « passe en mode local », « passe en mode en ligne », « statut ».
- Alimentation, toujours confirmée par un « oui » : « mets le PC en veille », « redémarre le
  PC », « éteins le PC ».
- « annule » et « répète » (voir plus bas).

Les autres phrases vont à l'IA du mode courant. Pendant l'écoute coupée, les autres phrases ne
sont ni exécutées ni journalisées.

## Voix et réponses

Jarvis répond en majordome discret : une phrase brève, en te vouvoyant, et en te nommant
de temps en temps (« Je mets le PC en veille, Monsieur ? »).

**Bips** joués par l'oreille :

- **réveil** (une note montante) : « Jarvis » est entendu, tu peux parler ;
- **réussite** (deux notes montantes) : l'action est faite, avant la phrase de Jarvis ;
- **échec** (deux notes graves descendantes) : l'action n'a pas pu se faire ; la raison est
  toujours dite.

**Veille, redémarrage et arrêt** demandent une confirmation : Jarvis pose la question (« Je mets
le PC en veille, Monsieur ? ») et l'oreille écoute la réponse sans « Jarvis », 8 s au plus.
Seule ta voix compte.

- Oui : « oui », « ouais », « ok », « vas-y », « allez-y », « d'accord », « confirme »,
  « fais-le ».
- Non : « non », « annule », « laisse tomber », « surtout pas », « pas maintenant ».

Toute autre phrase, le silence ou une autre voix : rien n'est fait. Après un « oui », Jarvis
dit sa phrase, et la commande part 4 s plus tard, une fois la phrase dite.

**« Jarvis, annule »** (ou « laisse tomber ») efface la question en attente. **« Jarvis,
répète »** (ou « tu peux répéter ? », « qu'est-ce que tu as dit ? ») redit la dernière réponse
sans rien refaire ; si une question attend, elle est reposée et le délai de 8 s repart.

Deux réglages dans `~/.config/jarvis/config.toml` (puis `jarvisctl restart`) :

```toml
# address = "Monsieur"            # nom par lequel Jarvis s'adresse à toi ; "" : jamais nommé
# sound_only_for_success = false  # vrai : une commande rapide réussie n'est confirmée que par le bip
```

Avec `sound_only_for_success = true`, « Jarvis, suivant » donne le seul bip de réussite au lieu
de « Bien. ». Un échec, une réponse de l'IA et une action confirmée par « oui » restent dits.

## Voix de Jarvis

Jarvis répond à voix haute avec la voix Piper « pierre » (`vits-piper-fr_FR-upmc-medium`,
licence CC-BY-SA 4.0), calculée sur le PC par le cerveau, puis jouée par l'oreille.

```bash
scripts/download-models.sh voice    # 80 Mo, extrait dans ~/.local/share/jarvis/models/
jarvisctl restart
```

Sans la voix (non téléchargée, désactivée ou en panne), le cerveau démarre quand même : les
commandes s'exécutent et la réponse arrive en texte seul ; un avertissement est journalisé.
Au premier lancement, les données d'espeak-ng (conversion du texte en sons) sont décompressées
dans le dossier privé de la session : `/run/user/<uid>/piper-phonemize-espeak-ng-data/` (19 Mo,
en mémoire, recréé à chaque ouverture de session). Le service du cerveau fixe pour cela
`TMPDIR=%t` ; lancé à la main, il utilise `$TMPDIR` (par défaut `/tmp`).

## Fichiers créés

| Fichier | Contenu |
| --- | --- |
| `~/.local/share/jarvis/wakeword/samples/jarvis-NN.wav` | les prises de « Jarvis » (seuls sons conservés, droits 0600) |
| `~/.local/share/jarvis/wakeword/jarvis.rpw` | le modèle du mot d'éveil |
| `~/.local/share/jarvis/voiceprint/charles.json` | l'empreinte vocale : un vecteur de nombres, sans son (droits 0600) |
| `~/.config/jarvis/ear.toml` | réglages de l'oreille (optionnel) |
| `~/.local/state/jarvis/ear-muted` | présent tant que l'écoute est coupée |
| `$XDG_RUNTIME_DIR/jarvis/ducked-volume` | volume d'origine, le temps d'une phrase |

Pour refaire l'enrôlement : supprime ces fichiers, puis relance `jarvis-ear enroll`. Pour tout
retirer : `jarvisctl uninstall --purge` (binaires, bibliothèques de `~/.local/lib/jarvis/`,
configuration et données, modèles compris).

## Réglages

`~/.config/jarvis/ear.toml` (tout est optionnel) :

```toml
# wakeword_path = "~/.local/share/jarvis/wakeword/jarvis.rpw"
# threshold = 0.5          # seuil de détection du mot d'éveil (plus haut = moins de fausses détections)
# device = "nom du micro"  # micro par défaut sinon
```

`~/.config/jarvis/config.toml`, côté cerveau (puis `jarvisctl restart`) :

```toml
[stt]
# model_path = "~/.local/share/jarvis/models/ggml-small-q5_1.bin"   # ggml-base-q5_1.bin : déconseillé (répétitions)
# threads = 6

[speaker]
# enabled = true
# model_path = "~/.local/share/jarvis/models/3dspeaker_speech_campplus_sv_en_voxceleb_16k.onnx"
# threshold = 0.55          # phrase d'au moins 2 s
# strict_threshold = 0.65   # phrase de 1 à 2 s
# threads = 2

[tts]
# enabled = true
# model_dir = "~/.local/share/jarvis/models/vits-piper-fr_FR-upmc-medium"
# speaker = 1               # 1 = « pierre », 0 = « jessica »
# speed = 1.0               # plus grand = plus rapide
# threads = 2
```

Si ta voix est refusée trop souvent, baisse `threshold` ; si d'autres voix passent, monte-le.
Le score de chaque refus est dans les journaux (`jarvisctl logs`), sans le texte.

## Dépannage

Commence par :

```bash
jarvisctl doctor
```

Les lignes `!` concernent l'oreille (avertissements, pas d'échec) :

- **Bibliothèques sherpa-onnx absentes** : `jarvisctl install` ou `jarvisctl update`.
- **Voix de Jarvis absente** : `scripts/download-models.sh voice`, puis `jarvisctl restart`.
- **Modèle Whisper ou d'empreinte absent** : `scripts/download-models.sh all`. Sans Whisper, le
  cerveau répond « La reconnaissance vocale n'est pas installée. »
- **Mot d'éveil ou empreinte absents** : `jarvis-ear enroll`.
- **« Empreinte vocale indisponible : modèle introuvable »** : télécharge le modèle avec
  `scripts/download-models.sh speaker`, puis `jarvisctl restart`.
- **Jarvis ne réagit pas** : `jarvisctl logs -f` ; vérifie que le micro n'est pas en écoute
  coupée ou en coupure totale (icône), et que `jarvisctl status` montre le service actif.
