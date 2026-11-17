# Jarvis, phase 9 (mise en service) : plan d'implémentation

**Goal :** finir ce qu'il faut pour un usage quotidien : voir d'un coup d'œil dans l'icône les
oreilles connectées et les tâches d'agent en cours (9.3), empêcher la veille quand le portable est
branché si Charles le demande (9.4), et pouvoir **mesurer** les critères de réussite de la
conception §9 sur une vraie journée (9.5). Le service de l'oreille (9.2) est déjà fait.

**Spec :** [conception](../specs/2026-10-28-jarvis-design.md) §5.7 (veille), §5.7 bis (icône),
§9 (critères de réussite).

**Branche :** `feature/mise-en-service` (git flow), travail rédigé et vérifié sur le VPS
(`cargo` avec ALSA extrait sans root, voir « Machine »).

## Contraintes

- **Compatibilité du protocole** : tout champ ajouté à `StatusInfo` est `#[serde(default)]`.
  Une oreille Android d'avant la phase 9 ignore les champs inconnus ; un cerveau d'avant la
  phase 9 envoie un statut sans tâches : l'icône affiche « aucune tâche ».
- **Rien ne change par défaut** : `inhibit_sleep_on_ac = false` ; sans fichier de mesures, rien
  ne casse. Les tests des phases 1 à 8 restent verts sans changer leurs attentes.
- **Mesures** : le fichier ne contient ni son ni empreinte ; seulement l'heure, l'oreille, la
  durée, la route, l'issue et le texte de la phrase (déjà journalisé, spec §5.7). Taille bornée
  (rotation à 5 Mo, un seul ancien fichier). Une écriture ratée ne gêne jamais la réponse.
- **Veille** : le blocage passe par `systemd-inhibit` (logind), jamais par une commande root.
  Le processus d'inhibition est arrêté dès que le portable est débranché, que l'option est
  coupée ou que le cerveau s'arrête (`kill_on_drop`).
- Français partout, pas de nouvelle dépendance externe, `scripts/check.sh` vert (fmt, clippy
  `-D warnings`, tests). Tests : ni micro, ni son, ni vrai `systemd-inhibit`, fichiers dans des
  dossiers temporaires.
- **Machine (VPS)** : un seul travail lourd à la fois, `nice -n 10`, `-j 4`. ALSA de
  développement extrait sans root dans `~/.local/opt/alsa-dev` (`. ~/.local/opt/alsa-dev/env.sh`
  avant `cargo`).

## Décisions

| Question | Décision | Pourquoi |
|---|---|---|
| Source des tâches d'agent | `AgentSkill` publie la liste des tâches actives dans un `watch` (id, projet, heure de départ). Le hub l'écoute comme les changements de mode et diffuse `Status` à chaque changement. | Pas de lecture disque dans la boucle du hub ; mise à jour immédiate dans l'icône au départ et à la fin d'une tâche. |
| Forme dans le protocole | `StatusInfo.tasks: Vec<TaskSummary { id, project, started_at }>`. Les oreilles connectées sont déjà dans `StatusInfo.ears`. | Le cerveau diffuse déjà `Status` à la connexion et à la déconnexion d'une oreille : il suffisait de l'afficher. |
| Affichage | Infobulle : ligne d'état + « N oreille(s) connectée(s) » + « Agent : projet (depuis HH:MM) ». Menu : sous-menu « Statut détaillé » (oreilles connectées et déconnectées, tâches, dernière phrase). | La conception §5.7 bis : « statut détaillé : oreilles connectées, dernière phrase, tâches d'agent ». |
| Heure affichée | Heure locale `HH:MM` calculée par l'oreille à partir de `started_at` (secondes Unix) et du décalage de `/etc/localtime` lu par la libc (`localtime_r`). | Pas de crate de dates ; une durée « depuis 12 min » se périmerait dans un menu qui ne se redessine qu'aux changements. |
| Détection du secteur | Lecture de `/sys/class/power_supply/*/` : `type = Mains` et `online = 1`. Aucune alimentation `Mains` (PC fixe) = sur secteur. Relu toutes les 30 s. | Sans dépendance ni D-Bus ; 30 s suffit pour une mise en veille. |
| Blocage de la veille | Processus `systemd-inhibit --what=sleep:idle --who=Jarvis --why=… --mode=block sleep infinity`, tenu tant que l'option est vraie et le portable branché. | Méthode prévue par la conception ; s'arrête avec le cerveau. |
| Mesures | Une ligne JSON par phrase traitée dans `~/.local/state/jarvis/mesures.jsonl` : heure, oreille, durée fin de phrase → réponse prête (ms), route (`raccourci`, `competence`, `ia_locale`, `ia_en_ligne`, `aucune`), issue (`comprise`, `vide`, `ignoree`, `erreur`), texte. `jarvisctl mesures [--jours N]` résume face aux critères. | La spec fixe des seuils par route ; les fausses détections se voient comme des réveils sans phrase (`vide`) ou d'une autre voix (`ignoree`) ; une double exécution se voit comme la même phrase comprise sur deux oreilles à moins de 3 s. |
| Route d'une phrase | Notée par le `Brain` dans un `task_local` posé par le hub autour du traitement (absent hors hub : rien n'est noté). | Pas de changement de signature dans `Brain` ni `ListenService`. |
| Version | Fin de phase : **0.4.0** (fonctions nouvelles, compatibles). La 1.0.0 (9.6) attend les essais réels et les mesures de Charles. | Annoncer 1.0.0 sans avoir mesuré les critères de réussite serait trompeur. |

## Tâches

### Tâche 1 : tâches d'agent dans le statut (9.3, cerveau)
- [ ] `jarvis-protocol` : `TaskSummary`, champ `StatusInfo.tasks` (`serde(default)`), test
      `statut_sans_taches_compatible`.
- [ ] `jarvis-skills` : `AgentSkill::tasks() -> watch::Receiver<Vec<RunningTask>>`, publié à
      l'ajout et au retrait d'une tâche active ; test `tache_publiee_puis_retiree`.
- [ ] `jarvis-brain` : `Services.tasks`, écouté dans `State::run`, `status()` le remplit ;
      `main.rs` branche l'agent ; test du hub `statut_diffuse_au_depart_d_une_tache`.

### Tâche 2 : statut détaillé dans l'icône (9.3, oreille)
- [ ] `EarStatus` : `ears` et `tasks` copiés du statut du lien (`on_link_state`).
- [ ] `tray.rs` : infobulle et sous-menu « Statut détaillé », fonctions pures testées
      (`detail_lines`, `tooltip_text`).
- [ ] FFI Android : champs nouveaux ignorés (compilation seulement).

### Tâche 3 : option `inhibit_sleep_on_ac` (9.4)
- [ ] `[power] inhibit_sleep_on_ac = false` dans la configuration (et `config.example.toml`).
- [ ] `power.rs` : `on_ac(dir)` pur et testé (portable branché / débranché / PC fixe), boucle
      30 s qui tient ou relâche l'inhibition via un trait `Inhibitor` (double en test).
- [ ] `jarvisctl doctor` : ligne « veille » ; documentation dans `docs/installation.md`.

### Tâche 4 : mesures des critères (9.5)
- [ ] `jarvis-protocol::metrics` : `PhraseRecord`, `Route`, `Outcome`, `summarize()` pur et
      testé (latences médiane / 90e centile par route, phrases vides et ignorées par jour,
      doubles exécutions).
- [ ] `jarvis-brain` : `MetricsLog` (ajout + rotation), route notée par le `Brain`, ligne écrite
      par le hub à la fin du traitement.
- [ ] `jarvisctl mesures [--jours N]` : tableau face aux seuils de la spec §9.
- [ ] Fiche d'essai `docs/essais/essai-9.5-mesures.md` (une journée normale, puis lecture).

### Tâche 5 : relecture, documentation, version 0.4.0
- [ ] README, `docs/oreille.md`, `ROADMAP.md` ; `scripts/check.sh` vert.
- [ ] `git flow feature finish`, release 0.4.0 (CHANGELOG, étiquette, push).
