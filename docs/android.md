# L'oreille Android

Un téléphone Android (testé sur un Pixel 4 XL, Android 13) peut être une **oreille** de Jarvis,
comme l'oreille du PC : il écoute « Jarvis », envoie la phrase au cerveau sur le PC par le
réseau local (TLS), et joue les bips et la voix de Jarvis. La logique (élection entre
plusieurs oreilles, « le PC est endormi », questions, coupures) est celle de l'oreille du PC :
c'est le même code Rust (`jarvis-ear-core`), relié à l'appli par `jarvis-ear-ffi`. Le protocole
est décrit dans [reseau.md](reseau.md).

## Ce qu'il faut

- Un Pixel (ou un téléphone Android 13 en arm64) avec les **options pour les développeurs** :
  Réglages > À propos du téléphone > touche 7 fois « Numéro de build », puis active
  « Débogage USB » et branche le téléphone au PC (accepte la clé « Autoriser le débogage USB »).
- Le téléphone sur le **même Wi-Fi que le PC** (pas la 4G).
- Si le pare-feu `ufw` est actif sur le PC, il doit laisser passer les oreilles du réseau.
  `jarvisctl doctor` affiche alors les deux commandes à lancer :

  ```bash
  sudo ufw allow from 192.168.0.0/16 to any port 7341 proto tcp
  sudo ufw allow from 192.168.0.0/16 to any port 5353 proto udp
  ```

  (voir [reseau.md](reseau.md#mise-en-route))
- Le cerveau à jour et démarré sur le PC (`jarvisctl status`).
- Pour construire : le SDK Android (`ANDROID_HOME`, `~/Android` par défaut), le NDK
  28.2.13676358, `cargo-ndk`, la cible Rust `aarch64-linux-android` et un JDK 21 (ou 17). Le
  script dit ce qui manque.

## Construire

```bash
scripts/android-build.sh              # bibliothèque Rust, liaisons Kotlin, APK
scripts/android-build.sh --test       # + tests Kotlin sur la JVM
scripts/android-build.sh --offline    # Gradle sans réseau
```

Le **premier passage doit se faire en ligne** (Gradle télécharge JNA, zxing et JUnit) ; ensuite
`--offline` suffit. Le JDK 21 est choisi tout seul (`JARVIS_JAVA_HOME` pour en imposer un). L'APK
(environ 9 Mo, arm64 seulement) est `apps/android/app/build/outputs/apk/debug/app-debug.apk`.

## Installer

Téléphone branché en USB :

```bash
scripts/android-install.sh [--no-grant] [--copy-wakeword] [--logs]
```

Par défaut, le script installe l'APK avec `-g` (il **accorde** micro, notifications et caméra),
puis, par `adb` :

- accorde la **superposition** (« Afficher par-dessus d'autres applications ») : elle permet à
  l'écoute de repartir seule au redémarrage du téléphone (voir plus bas) ;
- met l'appli dans la **liste blanche de batterie** : sans elle, Android peut endormir l'écoute
  écran éteint ;

et enfin lance l'appli. `--no-grant` n'accorde rien (aucun micro ne peut s'ouvrir). `--logs` suit
le journal à la fin (`adb logcat`).

## Première ouverture

L'écran principal liste cinq étapes, chacune avec son état et son bouton :

1. **Autorisations** : micro et notifications (bouton « Accorder » si besoin).
2. **Appairage** : bouton « Appairer ».
3. **Mot d'éveil** : bouton « Enregistrer ».
4. **Batterie** : « Écoute sans restriction de batterie » (bouton « Autoriser » sinon).
5. **Démarrage seul** : « Autorisé : l'écoute repart seule au redémarrage » (bouton « Autoriser »
   sinon).

Le bouton « Démarrer l'écoute » ne s'active qu'une fois les étapes 1 à 3 faites (« Terminez les
étapes 1 à 3 pour démarrer l'écoute » sinon) ; il devient « Arrêter l'écoute » pendant l'écoute.

### Appairage

Sur le PC :

```bash
jarvis pair
```

Sur le téléphone, étape 2 > « Appairer » : soit « Scanner le QR code » (QR code de
`jarvis pair`, le téléphone peut être tenu dans n'importe quel sens), soit colle l'adresse
`jarvis://pair?…` dans le champ prévu. Le nom de l'oreille est prérempli avec le modèle du
téléphone (« Pixel 4 XL ») ; change-le si tu veux. La fenêtre d'appairage dure 2 minutes.
Ensuite `jarvis ears` montre l'oreille, plateforme `android`.

Si le message dit « PC injoignable : même Wi-Fi que le PC ? `jarvis pair` est-il ouvert ? »,
vérifie le Wi-Fi, le pare-feu et que `jarvis pair` est toujours ouvert. Rien n'est enregistré
tant que l'appairage n'a pas réussi.

### Enregistrer le mot d'éveil

Étape 3 > « Enregistrer » : **huit prises** du mot « Jarvis », dites naturellement, à la
distance où tu comptes t'en servir. À chaque prise, le bouton « Prise n / 8 » affiche
« Préparez-vous… » (moins d'une seconde), puis « Dites « Jarvis » » pendant 1,5 s. Si la prise
est trop faible (« Trop faible : rapprochez-vous et recommencez »), la même prise est refaite.
À la fin : « Mot d'éveil prêt (8 prises) », puis « Terminer ». Le micro du téléphone n'est pas
celui du PC : c'est pour ça que le modèle est fait sur le téléphone.

Les réglages (en bas de l'écran principal) : « Démarrer avec le téléphone », « Nom de
l'oreille » (envoyé au cerveau à l'appairage : un changement ne compte qu'au prochain appairage), « Seuil du mot d'éveil » (de 0,30 à 0,80, 0,50 par défaut ; pris en compte **au
prochain démarrage** de l'écoute) et « Désappairer ».

## La notification et ses boutons

Pendant l'écoute, une notification permanente (canal « Écoute », silencieux) montre l'état :

| Texte | Sens |
| --- | --- |
| « À l'écoute · mode local » (ou « mode en ligne ») | tout va bien, connectée au PC |
| « À l'écoute · connexion au PC… » | connexion en cours |
| « Écoute coupée · … » | écoute coupée (le micro reste ouvert pour entendre « réactive l'écoute ») |
| « Micro coupé (coupure totale) · … » | micro fermé : plus de point vert |
| « … · PC injoignable » | PC éteint, endormi ou hors du Wi-Fi ; l'oreille réessaie seule |
| « … · appairage refusé : réappairez depuis l'appli » | l'oreille a été retirée côté PC |
| « Micro bloqué par Android : touchez pour relancer · … » | voir plus bas |

Boutons : **« Couper l'écoute »** (qui devient **« Réactiver l'écoute »**) et **« Coupure
totale »**. En coupure totale, il ne reste que **« Rouvrir le micro »**. Toucher la notification
ouvre l'appli.

## Démarrage automatique

Au redémarrage du téléphone (et après une mise à jour de l'appli), si l'oreille est appairée, a
son mot d'éveil et que « Démarrer avec le téléphone » est coché :

- **avec la superposition accordée** (le script d'installation s'en occupe) : une activité
  transparente s'ouvre un instant et lance l'écoute seule ;
- **sans** : une notification « Jarvis : touchez pour démarrer l'écoute » attend ton toucher.

La raison : Android 13 refuse le micro à un service lancé en arrière-plan, sans appli visible.
Il faut donc une activité au premier plan, même une seconde.

## « Micro bloqué »

Quand Android coupe le micro à l'appli (service lancé en arrière-plan, accès micro coupé dans
les réglages rapides…), la capture ne livre que des zéros. Après 3 s de zéros exacts, la
notification devient « Micro bloqué par Android : touchez pour relancer ». Toucher la
notification relance l'écoute. Si ça se répète, vérifie que le micro n'est pas coupé dans les
réglages rapides et que l'autorisation « Micro » est bien accordée.

## Batterie

Une oreille qui écoute écran éteint consomme : **branche-la** si elle reste à poste fixe. Pour
rester joignable, l'écoute tient trois verrous :

- un verrou du processeur (`PARTIAL_WAKE_LOCK`) : le micro continue écran éteint ;
- un verrou Wi-Fi en haute performance : sans lui, l'économie d'énergie du Wi-Fi retarde les
  paquets au-delà de la fenêtre de 300 ms du réveil ;
- un verrou multicast : sans lui, Android jette les paquets mDNS (retrouver le PC si son
  adresse change).

## Repli : copier le mot d'éveil du PC

Si l'enregistrement sur le téléphone déçoit, tu peux y copier celui du PC
(`~/.local/share/jarvis/wakeword/jarvis.rpw`) :

```bash
scripts/android-install.sh --copy-wakeword
```

## Journaux

```bash
scripts/android-install.sh --logs          # côté téléphone (adb logcat : Jarvis, JarvisRust)
jarvisctl logs -n 300 | grep voix          # côté cerveau : scores de la voix
```

Le cerveau écrit, pour chaque phrase, l'oreille, le score de ta voix et le seuil :
`voix reconnue oreille=… score=… seuil=…` ou `voix non reconnue oreille=… score=… seuil=…`.
L'empreinte de ta voix a été prise avec le micro du PC : le score d'un téléphone peut être plus
bas. C'est justement ce que l'[essai 5.7](essais/essai-5.7-android.md) mesure.

## Retirer l'oreille

```bash
jarvis unpair "Pixel 4 XL"
```

Puis, sur le téléphone, « Désappairer » dans les réglages (confirme avec « Oui »). Pour
désinstaller l'appli :

```bash
adb uninstall fr.mawena.jarvis
```
