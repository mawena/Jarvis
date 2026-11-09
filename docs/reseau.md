# Oreilles en réseau

Une **oreille** (le micro du PC, une autre machine Linux, plus tard un téléphone) parle au
cerveau par le réseau de la maison : WebSocket sur TLS 1.3, port **7341**. Le cerveau s'annonce
en mDNS (`_jarvis._tcp`), l'oreille le trouve seule. Chaque oreille est **appairée** une fois :
elle garde un jeton et l'empreinte du certificat du cerveau, et refuse tout autre certificat.
L'oreille du PC passe par le même chemin (sur `127.0.0.1`) et s'appaire seule.

Quand plusieurs oreilles entendent « Jarvis », **une seule** est retenue (la plus sûre et la
plus proche, sur une fenêtre de 300 ms) : elle joue la réponse, les autres s'arrêtent. Une
confirmation (« oui ») ne compte que sur l'oreille qui a posé la question.

## Mise en route

```bash
jarvisctl update    # cerveau et oreille du PC ensemble
jarvisctl doctor    # certificat, port, oreilles, pare-feu
jarvis ears         # l'oreille « PC » doit être là, connectée
```

- **Le cerveau et l'oreille se mettent à jour ensemble** (`jarvisctl update` le fait). Une
  oreille trop ancienne reçoit « requête invalide » du cerveau : plus aucune phrase ne passe
  par le socket Unix.
- Au premier démarrage, le cerveau crée son certificat (`~/.local/share/jarvis/tls/`) ;
  l'oreille du PC s'appaire seule par le socket Unix, sans code.
- **Pare-feu** : si `ufw` est actif (`jarvisctl doctor` le dit), ouvrez le port et mDNS au
  réseau local, une fois :

  ```bash
  sudo ufw allow from 192.168.0.0/16 to any port 7341 proto tcp
  sudo ufw allow from 192.168.0.0/16 to any port 5353 proto udp
  ```

  Adaptez `192.168.0.0/16` à votre réseau. L'oreille du PC n'en a pas besoin.
- `avahi-daemon` peut rester actif : il cohabite avec l'annonce de Jarvis.

## Appairer une oreille

Sur le PC :

```bash
jarvis pair
```

Le terminal affiche un **code** à 6 chiffres, un **QR code**, l'adresse d'appairage
(`jarvis://pair?…`), l'**empreinte courte** du PC (`A1B2-C3D4-E5F6-0718`) et un compte à
rebours. La fenêtre reste ouverte 2 minutes, pour une seule oreille ; le code est à usage
unique. Chaque machine a 3 essais : après 3 codes faux, elle est refusée jusqu'à la fin de la
fenêtre (« plus aucun essai depuis cette machine »), mais les autres peuvent encore s'appairer.
Après 20 codes faux en tout, la fenêtre se ferme. `jarvis pair` rend 0 quand une oreille s'est
appairée, 1 sinon (délai écoulé, trop d'essais, `Ctrl+C`).

Pour rattacher l'oreille à une personne autre que Charles (voir [personnes.md](personnes.md)) :

```bash
jarvis pair --person Léa
```

Sans `--person`, l'oreille est rattachée au propriétaire (Charles). Une personne inconnue est
refusée, et si le rattachement échoue, l'appairage échoue. `jarvis ears` montre la personne de
chaque oreille (colonne « Personne »), et `jarvis person attach` rattache une oreille déjà
appairée.

Pour un téléphone Android, voir [android.md](android.md) : l'appli scanne le QR code ou prend
l'adresse collée.

Sur l'oreille, au choix :

```bash
jarvis-ear pair 'jarvis://pair?…'     # adresse copiée : rien à vérifier
jarvis-ear pair --code 482913          # code seul : cerveau trouvé par mDNS
```

- Avec l'adresse (ou le QR code, pour le téléphone), l'empreinte complète est dedans :
  l'appairage est sûr d'emblée.
- Avec le code seul, l'oreille affiche l'empreinte du cerveau trouvé et demande « Est-ce la
  même que sur le PC ? [o/N] ». Comparez-la à celle de `jarvis pair` : si elle diffère,
  répondez non, rien n'est envoyé.
- `--address hôte[:port]` : cerveau introuvable en mDNS (port 7341 par défaut).
- `--name Salon` : nom de l'oreille (défaut : `name` de `ear.toml`, sinon le nom de la machine).

Puis relancez l'oreille : `systemctl --user restart jarvis-ear` (ou `jarvis-ear --profile …`,
voir plus bas). L'appairage est dans `~/.local/share/jarvis/ear/pairing.toml` (0600).

## Lister et retirer les oreilles

```bash
jarvis ears              # identifiant court, nom, plateforme, connectée ou non, personne
jarvis unpair Salon      # par nom
jarvis unpair 3f9a1c07   # ou par identifiant (la colonne Id de jarvis ears suffit)
```

Une machine réappairée sous le même nom laisse son ancienne fiche : le cerveau ne la remplace
pas, deux téléphones du même modèle pouvant porter le même nom. Avec deux oreilles du même nom,
`jarvis unpair <nom>` demande l'identifiant : retirez l'ancienne (« Connectée : non ») par sa
colonne Id.

Une oreille retirée est déconnectée tout de suite et refusée ensuite ; pour la reprendre,
refaites l'appairage. L'oreille du PC, elle, se réappaire aussitôt par le socket Unix : pour
la couper, `jarvisctl stop oreille`.

Le registre est `~/.local/share/jarvis/ears.toml` (0600) : seul le SHA-256 de chaque jeton y
est gardé. Deux réglages par oreille s'y modifient cerveau arrêté : `duck = false` (ne pas
baisser la musique du PC à son réveil) et `bias` (petit avantage dans le choix de l'oreille,
`0.1` par exemple).

## Mode en ligne depuis une oreille

Seule l'oreille du PC (son icône) peut passer le cerveau en mode en ligne. Une oreille d'une
autre machine peut le repasser en local ; pour le mode en ligne, elle reçoit « Le mode en
ligne se demande à la voix (avec confirmation) ou depuis le PC. » (dans son journal) : dites-le
à voix haute (la phrase de confirmation est demandée), ou `jarvis mode en-ligne` sur le PC.

## Adresse manuelle

Si l'oreille ne trouve pas le PC (mDNS bloqué par le routeur, autre sous-réseau), donnez son
adresse dans `~/.config/jarvis/ear.toml` de l'oreille :

```toml
brain_address = "192.168.1.20"     # hôte[:port], port 7341 par défaut
# name = "Salon"                   # nom de l'oreille
```

Elle est essayée avant les adresses déjà connues et la découverte mDNS.

## Profil de test

Une seconde oreille sur la même machine (un autre micro) ou une oreille d'essai :

```bash
jarvis pair                                   # sur le PC
jarvis-ear --profile essai pair 'jarvis://pair?…'
jarvis-ear --profile essai                    # lance cette oreille dans le terminal
```

Un profil a ses propres fichiers : `~/.config/jarvis/ear-essai.toml` (par exemple
`device = "nom du micro"`), `~/.local/share/jarvis/ear-essai/` (appairage, phrase hors ligne)
et `~/.local/state/jarvis/ear-essai-muted`. Il doit être appairé avant d'être lancé. Il tourne
à côté du service sans `--force`.

## Le PC ne répond pas

Une oreille qui perd le cerveau (PC en veille, cerveau arrêté) se reconnecte seule. Si on lui
parle entre-temps, elle répond « le PC est endormi » (la voix de Jarvis, reçue à l'avance du
cerveau ; sinon un bip d'échec) en 2 s environ. Rien n'est envoyé ni gardé en attente : la
phrase est perdue.

Cerveau arrêté proprement : la connexion tombe aussitôt, la réponse est immédiate. Après une
mise en veille brutale du PC, la connexion peut sembler ouverte un moment (jusqu'à ce que
l'oreille constate le silence) : l'oreille envoie alors la phrase, mais sans élection du
cerveau dans les 2 s, elle dit quand même « le PC est endormi » et abandonne la phrase. De
même si le lien tombe pendant que Jarvis réfléchit : la réponse ne viendra plus, l'oreille le
dit. Un traitement trop long (plus de 47 s) se termine par un bip d'échec.

« Jarvis » redit pendant que Jarvis réfléchit (cerveau occupé par la phrase précédente) : le
bip de réveil retentit, puis la réponse à la première phrase arrive comme prévu ; le nouveau
réveil est simplement ignoré, sans bip d'échec.

L'icône de l'oreille du PC dit « PC injoignable », « connexion au PC… » ou « appairage refusé :
relancez jarvis-ear pair » (oreille retirée ou jeton refusé).

## Si le PC change d'adresse ou de port

- **Nouvelle adresse IP** (DHCP) : rien à faire, l'oreille le retrouve par mDNS. Avec une
  `brain_address` fixe, corrigez-la. Une réservation DHCP dans la box évite le problème.
- **Nouveau port** (`[network] listen`) : les autres oreilles le retrouvent par mDNS ; l'oreille
  du PC, elle, garde l'ancien. Supprimez son appairage, elle se réappaire seule :

  ```bash
  rm ~/.local/share/jarvis/ear/pairing.toml
  systemctl --user restart jarvis-ear
  ```

## Réinitialiser le certificat

Clé perdue ou volée : recréez l'identité du cerveau. **Toutes les oreilles sont à réappairer.**

```bash
jarvisctl stop
rm -r ~/.local/share/jarvis/tls ~/.local/share/jarvis/ears.toml
rm ~/.local/share/jarvis/ear/pairing.toml     # l'oreille du PC se réappaire seule
jarvisctl start
```

Puis, pour chaque autre oreille : `jarvis pair` sur le PC, `jarvis-ear pair …` sur l'oreille.

## Réglages du cerveau

`~/.config/jarvis/config.toml` (puis `jarvisctl restart`) :

```toml
[network]
# enabled = true
# listen = "0.0.0.0:7341"     # toutes les interfaces
# name = ""                   # nom annoncé ; vide : nom de la machine
# arbiter_window_ms = 300     # fenêtre de regroupement des réveils
```

- Gardez `listen` sur `0.0.0.0` : sur une adresse précise (`192.168.1.20:7341`), l'oreille du
  PC (`127.0.0.1`) ne peut plus se connecter ; sur `127.0.0.1`, les autres machines non plus.
- `arbiter_window_ms` : si une oreille en Wi-Fi répond souvent en retard et n'est jamais
  retenue, montez-la (400, 500) ; chaque milliseconde s'ajoute au temps de réponse.
- Si le réseau ne peut pas démarrer (désactivé, certificat ou registre illisible, adresse
  invalide, port pris), le cerveau tourne quand même, sans oreilles : `jarvis pair`, `jarvis
  ears` et `jarvis unpair` répondent « Le réseau des oreilles est désactivé (voir les journaux
  du cerveau). ». La cause est dans `jarvisctl logs`.

## Sécurité

- **Le réseau est ouvert par défaut, sur toutes les interfaces** : `listen = "0.0.0.0:7341"`
  écoute aussi sur le VPN, les réseaux docker et tout Wi-Fi où se trouve le PC. Pour
  restreindre, au choix :
  - le pare-feu (`ufw`, voir « Mise en route ») : seul le réseau de la maison passe ;
  - `listen = "127.0.0.1:7341"` : seule l'oreille du PC passe ;
  - `[network] enabled = false` : plus aucune oreille, **celle du PC comprise**.
- **Sur un Wi-Fi public** (gare, café, hôtel), les autres voient le cerveau annoncé par mDNS
  (son nom, son port) et peuvent tenter de s'y connecter. Sans le code, ils ne peuvent pas
  s'appairer : la fenêtre ne s'ouvre qu'avec `jarvis pair`, 2 minutes, 3 essais par machine,
  20 en tout. Une connexion sans `hello` valide est fermée au bout de 3 s, 2 à la fois au plus
  par machine, et des places restent toujours libres pour l'oreille du PC. Le plus simple
  reste de restreindre (ci-dessus) hors de chez soi.
- **Téléphone ou oreille volé·e** : `jarvis unpair <nom>` tout de suite. Son jeton suffit pour
  se connecter comme elle et envoyer du son, par exemple un enregistrement de votre voix pour
  donner des ordres.
- Le jeton de l'oreille du PC ne vaut que depuis le PC lui-même (`127.0.0.1`), et seule elle
  peut passer le cerveau en mode en ligne (sinon : à la voix, avec confirmation).
- **Annonces** (fin de tâche d'un agent…) : elles vont à l'oreille de la dernière phrase, ou, si
  elle n'est plus connectée, à une autre oreille connectée. Elles peuvent donc être dites dans
  une autre pièce, ou sur le téléphone.
- **Socket du cerveau** : `jarvis`, `jarvisctl` et `jarvis-ear` refusent de lui parler si son
  dossier (`$XDG_RUNTIME_DIR/jarvis`, sinon `/tmp/jarvis-<uid>`) n'est pas à vous et fermé aux
  autres : « dossier du socket du cerveau peu sûr ». Si `/tmp/jarvis-<uid>` appartient à un
  autre utilisateur, quelqu'un a tenté de se faire passer pour le cerveau : arrêtez Jarvis,
  faites-le supprimer par l'administrateur, puis relancez.

## Dépannage

`jarvisctl doctor` vérifie le réseau (lignes `!` : avertissements, pas d'échec) :

- **Certificat du cerveau absent** : le cerveau ne l'a pas encore créé, `jarvisctl restart
  cerveau`.
- **Clé privée du cerveau absente** : le cerveau refuse de recréer un certificat seul (tout
  serait à réappairer) ; voir « Réinitialiser le certificat ».
- **Registre des oreilles illisible** : corrigez `ears.toml`, ou supprimez-le et réappairez.
- **Port 7341 pas en écoute** : cerveau arrêté ou réseau désactivé, voir `jarvisctl logs`.
- **Oreille du PC pas encore appairée** : `jarvisctl restart oreille`.
- **Cerveau trop ancien** : `jarvisctl update`.
- **Pare-feu ufw actif** : les deux commandes de « Mise en route ».
- **Une oreille ne trouve pas le PC** : pare-feu (5353/udp), même réseau Wi-Fi, sinon adresse
  manuelle. `jarvis-ear` journalise chaque essai (`RUST_LOG=debug jarvis-ear --profile …`).
