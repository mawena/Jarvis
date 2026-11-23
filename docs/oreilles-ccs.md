# Plan : les oreilles de Jarvis par CCS (phase 16)

Le cerveau de Jarvis tourne sur **laptop-maison**. Toute machine et tout téléphone doit pouvoir
lui parler, **même hors de la maison**, sans ouvrir de port sur la box.

```
 iPhone / web / bureau CCS ──wss (session CCS)──► ccs-serveur (VPS) ──┐
 Pixel (oreille Android) ───wss (session CCS)──►  relais d'oreilles   │ tunnel Ricochet
 autre Linux : jarvis-ear ──ricochet-agent tunnel──────────────────────┤ (WebSocket)
                                                                       ▼
                                              laptop-maison : cerveau, port 7341 (TLS épinglé)
```

## Pourquoi CCS
- Les applis CCS (iPhone, Pixel, web, bureau) sont déjà installées, connectées à
  ccs.mawena.cloud, et ont déjà un micro et une voix : pas de nouvelle appli iOS.
- Ricochet relie déjà toutes les machines au VPS, sans IP publique (`ricochet-agent tunnel`).
- Jarvis reste un projet à part : CCS n'est qu'un **transport** et un **bouton**.

## Le protocole ne change presque pas
Une oreille envoie `Hello` (jeton d'appairage), puis pour chaque phrase `Wake`, des trames
`MicAudio` (PCM 16 kHz, 16 bits), `ListenEnd`, et reçoit `Response` (texte et voix de Jarvis en
PCM). Une oreille « bouton » envoie un `Wake` marqué **manuel** : le cerveau ne l'arbitre pas
contre les autres oreilles par le score (il gagne toujours, élu dès son arrivée) et n'exige pas
de mot d'éveil. Les autres oreilles qui ont entendu la même phrase reçoivent `Stop` : une seule
réponse. La voix reste vérifiée sur
le cerveau (empreinte CAM++) : les droits (agent, chantiers) suivent la personne reconnue.

## Le relais d'oreilles dans ccs-serveur
- Point d'entrée WebSocket `/api/jarvis/oreille`, ouvert seulement à une session CCS connectée.
- Il ouvre le tunnel Ricochet vers `laptop-maison:7341`, fait le TLS **épinglé** sur l'empreinte
  du cerveau, et relaie les messages dans les deux sens, sans les lire (sauf la taille).
- Chaque appareil est appairé une fois (`jarvis pair` sur le laptop, code saisi dans l'appli) ;
  son jeton reste dans l'appareil (trousseau iOS, stockage chiffré Android, navigateur).
- Laptop éteint ou injoignable : réponse « Jarvis est injoignable » immédiate.

## Écoute
| Appareil | Déclencheur |
| --- | --- |
| Pixel | mot d'éveil « Jarvis » (oreille Android existante), bouton en plus |
| iPhone | bouton dans l'appli CCS, widget, raccourci Siri (iOS coupe le micro en arrière-plan) |
| web, bureau CCS | bouton |
| autre Linux | `jarvis-ear` (mot d'éveil), par le tunnel hors du Wi-Fi |

## Sécurité
- Rien n'est ouvert sur internet : le relais exige une session CCS (mot de passe + TOTP de CCS),
  puis le jeton d'appairage de l'appareil ; le cerveau vérifie toujours la voix.
- Le tunnel Ricochet n'autorise que le port 7341 de laptop-maison en plus de SSH.
- `jarvis unpair <nom>` retire un appareil perdu ; la révocation de la session CCS aussi.
