//! Session de l'oreille, commune au PC et à Android : trames du micro, mot d'éveil,
//! phrase envoyée en flux au cerveau par le réseau, élection, réponses jouées,
//! annonces, questions, commandes (icône du PC, notification d'Android).
//!
//! Hors connexion, rien n'est capté ni gardé : « Jarvis » fait dire « le PC est
//! endormi » (phrase enregistrée, sinon bip d'échec) et relance la connexion.
//!
//! Les entrées-sorties de la plateforme restent derrière trois traits : [`Capture`]
//! (micro), [`Player`] (sons) et [`Link`] (cerveau).

pub mod audio;
pub mod sounds;
pub mod status;

#[cfg(test)]
mod tests;

pub use audio::{Capture, CaptureEvent, Player, SilentPlayer};
pub use status::{BrainLink, EarCommand, EarStatus};

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use jarvis_protocol::ear::{BrainMessage, EarMessage};
use jarvis_protocol::{Announcement, ControlResponse, EarAction, Sound, SpeechAudio, decode_pcm};
use tokio::sync::{mpsc, watch};
use tokio::time::Instant;

use crate::listener::{ListenEvent, Listener};
use crate::net::batch::AudioBatcher;
use crate::net::{ELECTION_TIMEOUT, Link, LinkEvent, LinkState};
use crate::resample::{Downmixer, i16_to_f32};
use crate::rustpotter::RustpotterDetector;
use crate::{MicState, Vad, WakeDetector};

/// Attente maximale de la réponse à une phrase : le pire cas du cerveau, plus une marge.
/// Au-delà, la réponse attendue n'est plus attendue (les annonces peuvent passer).
///
/// ```text
///   reconnaissance vocale (marge)           2 s
/// + choix de l'outil par le modèle local 20 s  (llm/lmstudio.rs, TIMEOUT)
/// + compétence la plus lente : recherche   15 s  (SearXNG 4 s + résumé local 10 s + 1 s)
/// + synthèse vocale                         8 s  (tts/mod.rs, SYNTH_DEADLINE)
/// = 45 s, arrondi à 50 s
/// ```
///
/// Vérifié côté cerveau par le test `budget_ecoute` de jarvis-brain (qui lit cette ligne).
pub const RESPONSE_TIMEOUT: Duration = Duration::from_secs(50);

/// Annonces en file : nouvel essai de lecture au repos à ce rythme.
pub const ANNOUNCE_RETRY: Duration = Duration::from_secs(1);

/// Délai pour commencer à répondre à une question de Jarvis (`EarAction::AwaitReply`).
pub const REPLY_WAIT: Duration = Duration::from_secs(8);

/// Délai maximal de la fermeture du lien en quittant.
const QUIT_TIMEOUT: Duration = Duration::from_secs(4);

/// Pendant que Jarvis parle, le réveil exige ce score en plus du seuil normal : la voix
/// de synthèse qui dit « Jarvis » ressemble moins au modèle (enregistré par
/// l'utilisateur) que l'utilisateur lui-même, qui peut donc toujours couper la parole.
pub const PLAYBACK_WAKE_MARGIN: f32 = 0.1;

/// Phrase « le PC est endormi » enregistrée (voix de Jarvis), si elle existe.
pub type OfflinePhrase = Arc<dyn Fn() -> Option<SpeechAudio> + Send + Sync>;

/// Capture en cours, envoyée en flux.
#[derive(Debug, Clone, Copy)]
struct Current {
    utterance: u32,
    muted: bool,
}

/// Phrase finie dont la réponse est attendue.
#[derive(Debug, Clone, Copy)]
struct Awaited {
    utterance: u32,
    muted: bool,
    since: Instant,
}

/// Session de l'oreille, la même sur toutes les plateformes.
pub struct EarSession<W: WakeDetector, V: Vad, C: Capture> {
    listener: Listener<W, V>,
    capture: C,
    link: Arc<dyn Link>,
    events: mpsc::Receiver<LinkEvent>,
    link_state: watch::Receiver<LinkState>,
    commands: mpsc::UnboundedReceiver<EarCommand>,
    status: watch::Sender<EarStatus>,
    downmixer: Option<Downmixer>,
    muted: bool,
    /// Présence = écoute coupée : survit à un redémarrage de l'oreille.
    mute_file: Option<PathBuf>,
    /// Coupure totale volontairement non retenue : au redémarrage, le micro se rouvre
    /// (un micro fermé pour de bon sans que l'on s'en souvienne serait pire).
    hard_muted: bool,
    unavailable: bool,
    current: Option<Current>,
    batcher: AudioBatcher,
    /// Numéro de la dernière phrase (réveil ou réponse à une question).
    utterance: u32,
    /// Réveil envoyé, ni `elected` ni `stop` reçu : échéance.
    election: Option<(u32, Instant)>,
    election_timeout: Duration,
    awaited: Option<Awaited>,
    /// Réponse attendue mise de côté par un nouveau réveil, le temps de la décision du
    /// cerveau : `elected` l'oublie (la nouvelle phrase la remplace), `stop` la rétablit
    /// (cerveau occupé par cette phrase : sa réponse viendra et sera jouée).
    superseded: Option<Awaited>,
    /// Une question attend la fin de la lecture pour écouter la réponse.
    awaiting_reply: bool,
    player: Box<dyn Player>,
    playing: watch::Receiver<bool>,
    brain: BrainLink,
    last_heard: Option<String>,
    /// Annonces poussées par le cerveau, jouées au repos.
    announcements: Vec<Announcement>,
    /// Score de réveil minimal pendant la lecture.
    playback_wake_score: f32,
    offline: Option<OfflinePhrase>,
}

impl<W: WakeDetector, V: Vad, C: Capture> EarSession<W, V, C> {
    /// L'écoute passe en mode flux.
    pub fn new(
        listener: Listener<W, V>,
        capture: C,
        link: Arc<dyn Link>,
        events: mpsc::Receiver<LinkEvent>,
        commands: mpsc::UnboundedReceiver<EarCommand>,
        status: watch::Sender<EarStatus>,
    ) -> Self {
        let player = SilentPlayer::new();
        let playing = player.playing();
        let link_state = link.state();
        Self {
            listener: listener.with_streaming(true),
            capture,
            link,
            events,
            link_state,
            commands,
            status,
            downmixer: None,
            muted: false,
            mute_file: None,
            hard_muted: false,
            unavailable: false,
            current: None,
            batcher: AudioBatcher::new(),
            utterance: 0,
            election: None,
            election_timeout: ELECTION_TIMEOUT,
            awaited: None,
            superseded: None,
            awaiting_reply: false,
            player: Box::new(player),
            playing,
            brain: BrainLink::Connecting,
            last_heard: None,
            announcements: Vec::new(),
            playback_wake_score: RustpotterDetector::DEFAULT_THRESHOLD + PLAYBACK_WAKE_MARGIN,
            offline: None,
        }
    }

    /// Sortie des sons (par défaut : aucune).
    pub fn with_player(mut self, player: Box<dyn Player>) -> Self {
        self.playing = player.playing();
        self.player = player;
        self
    }

    /// Seuil normal du mot d'éveil : celui de la lecture s'en déduit.
    pub fn with_wake_threshold(mut self, threshold: f32) -> Self {
        self.playback_wake_score = threshold + PLAYBACK_WAKE_MARGIN;
        self
    }

    /// Phrase « le PC est endormi » (sinon : bip d'échec).
    pub fn with_offline_phrase(mut self, offline: OfflinePhrase) -> Self {
        self.offline = Some(offline);
        self
    }

    /// Retient l'écoute coupée dans `path` ; un fichier présent coupe l'écoute dès le départ.
    pub fn with_mute_file(mut self, path: PathBuf) -> Self {
        self.muted = path.exists();
        self.mute_file = Some(path);
        self
    }

    fn set_muted(&mut self, muted: bool) {
        if self.muted == muted {
            return;
        }
        self.muted = muted;
        let Some(path) = &self.mute_file else { return };
        let result = if muted {
            path.parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|()| std::fs::write(path, ""))
        } else {
            std::fs::remove_file(path).or_else(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    Ok(())
                } else {
                    Err(e)
                }
            })
        };
        if let Err(e) = result {
            tracing::warn!("état de l'écoute non retenu ({}) : {e}", path.display());
        }
    }

    fn mic(&self) -> MicState {
        if self.hard_muted {
            MicState::HardMuted
        } else if self.unavailable {
            MicState::Unavailable
        } else if self.muted {
            MicState::Muted
        } else {
            MicState::Open
        }
    }

    fn publish(&self) {
        let state = EarStatus {
            mic: self.mic(),
            capturing: self.current.is_some(),
            brain: self.brain.clone(),
            last_heard: self.last_heard.clone(),
        };
        self.status.send_if_modified(|current| {
            let changed = *current != state;
            *current = state;
            changed
        });
    }

    /// Tourne jusqu'à la commande `Quit` (ou la fermeture du canal des commandes).
    pub async fn run(mut self) -> anyhow::Result<()> {
        let (cap_tx, mut cap_rx) = mpsc::channel(256);
        if let Err(e) = self.capture.start(cap_tx.clone()) {
            tracing::warn!("micro indisponible : {e:#}");
            self.unavailable = true;
        }
        self.on_link_state();
        self.publish();
        let mut retry = tokio::time::interval(ANNOUNCE_RETRY);
        retry.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            let election = self.election.map(|(_, deadline)| deadline);
            tokio::select! {
                Some(event) = cap_rx.recv() => self.on_capture(event),
                command = self.commands.recv() => match command {
                    Some(EarCommand::Quit) | None => break,
                    Some(command) => self.on_command(command, &cap_tx),
                },
                Some(event) = self.events.recv() => self.on_link_event(event),
                Ok(()) = self.link_state.changed() => self.on_link_state(),
                Ok(()) = self.playing.changed() => self.on_playback(),
                () = tokio::time::sleep_until(election.unwrap_or_else(Instant::now)),
                    if election.is_some() => self.on_election_timeout(),
                // Annonces en file : jouées dès le retour au repos (réponse attendue
                // échue, phrase arrêtée…).
                _ = retry.tick(), if !self.announcements.is_empty() => self.play_announcements(),
            }
            self.publish();
        }
        self.capture.stop();
        self.quit().await;
        Ok(())
    }

    /// Quitter pendant une capture : fin de phrase vide envoyée au mieux (le cerveau
    /// rétablit aussi le son à la déconnexion), puis fermeture propre du lien.
    async fn quit(&mut self) {
        self.cancel_capture();
        if tokio::time::timeout(QUIT_TIMEOUT, self.link.close())
            .await
            .is_err()
        {
            tracing::warn!("le lien avec le cerveau ne se ferme pas");
        }
    }

    fn on_link_state(&mut self) {
        self.brain = match &*self.link_state.borrow_and_update() {
            LinkState::Connected(status) => BrainLink::Connected {
                mode: status.mode,
                local_model: status.local_model.clone(),
            },
            LinkState::Connecting => BrainLink::Connecting,
            LinkState::Unreachable => BrainLink::Unreachable,
            LinkState::Rejected(_) => BrainLink::Refused,
        };
    }

    fn on_link_event(&mut self, event: LinkEvent) {
        match event {
            LinkEvent::Connected(_) => {}
            LinkEvent::Disconnected => {
                // La réponse attendue ne viendra plus.
                let awaited = self
                    .awaited
                    .take()
                    .is_some_and(|a| a.since.elapsed() < RESPONSE_TIMEOUT);
                self.superseded = None;
                if self.current.is_some() || self.election.is_some() {
                    tracing::warn!("connexion au cerveau perdue pendant la phrase");
                    self.abandon();
                } else if awaited {
                    tracing::warn!("connexion au cerveau perdue avant la réponse");
                    self.say_asleep();
                    self.link.nudge();
                }
            }
            LinkEvent::Message(message) => self.on_message(message),
        }
    }

    fn on_message(&mut self, message: BrainMessage) {
        match message {
            BrainMessage::Elected { utterance } => {
                if self.election.is_some_and(|(u, _)| u == utterance) {
                    tracing::debug!("oreille élue (phrase {utterance})");
                    self.election = None;
                    // La nouvelle phrase est prise : la réponse de la précédente n'est
                    // plus jouée.
                    self.superseded = None;
                }
            }
            BrainMessage::Stop { utterance } => {
                let refused = self.election.is_some_and(|(u, _)| u == utterance);
                if refused {
                    self.election = None;
                }
                if self.awaited.is_some_and(|a| a.utterance == utterance) {
                    self.awaited = None;
                }
                if self.superseded.is_some_and(|a| a.utterance == utterance) {
                    self.superseded = None;
                }
                if self.current.is_some_and(|c| c.utterance == utterance) {
                    tracing::info!("capture arrêtée par le cerveau (autre oreille ou phrase)");
                    self.current = None;
                    self.batcher = AudioBatcher::new();
                    self.listener.reset();
                }
                // Réveil refusé pendant que le cerveau traite la phrase précédente :
                // sa réponse reste attendue, et sera jouée. Pas de bip d'échec : `stop`
                // est aussi ce que reçoit l'oreille perdante d'un réveil entendu par
                // deux oreilles, que rien ne distingue d'un refus ; sans réponse en
                // attente ici, la phrase en cours est celle d'une autre oreille, qui
                // répondra.
                if refused && self.awaited.is_none() {
                    self.awaited = self.superseded.take();
                }
            }
            BrainMessage::Response {
                utterance: Some(utterance),
                response,
            } => {
                let Some(awaited) = self.awaited.filter(|a| a.utterance == utterance) else {
                    tracing::debug!("réponse d'une ancienne phrase ignorée");
                    return;
                };
                self.awaited = None;
                if self.election.is_some_and(|(u, _)| u == utterance) {
                    self.election = None;
                }
                self.on_heard(response, awaited.muted);
                self.play_announcements();
            }
            // Annonces des agents, poussées hors phrase (déjà retirées du tableau du
            // cerveau) : gardées jusqu'au repos, jouées une fois.
            BrainMessage::Response {
                utterance: None,
                response: ControlResponse::Announcements { items },
            } => {
                tracing::debug!("{} annonce(s) reçue(s)", items.len());
                self.announcements.extend(items);
                self.play_announcements();
            }
            BrainMessage::Response {
                utterance: None,
                response: ControlResponse::Error { message },
            } => tracing::warn!("le cerveau signale une erreur : {message}"),
            // L'état (mode) est déjà publié par le lien.
            _ => {}
        }
    }

    fn on_capture(&mut self, event: CaptureEvent<C::Samples>) {
        if self.hard_muted {
            return;
        }
        match event {
            CaptureEvent::Format { rate, channels } => {
                self.downmixer = Some(Downmixer::new(rate, channels));
                self.unavailable = false;
            }
            CaptureEvent::Unavailable => {
                self.downmixer = None;
                self.unavailable = true;
                // La musique avait été baissée au réveil : on la rétablit.
                self.cancel_capture();
            }
            CaptureEvent::Samples(data) => {
                let Some(downmixer) = self.downmixer.as_mut() else {
                    return;
                };
                let mono = downmixer.push(&data);
                // Le tampon retourne au pool de la capture.
                drop(data);
                let min_score = if self.player.is_playing() {
                    self.playback_wake_score
                } else {
                    0.0
                };
                self.listener.set_min_wake_score(min_score);
                for event in self.listener.push(&mono) {
                    self.on_listen(event);
                }
            }
        }
    }

    fn on_listen(&mut self, event: ListenEvent) {
        match event {
            ListenEvent::Wake { score, level_db } => self.on_wake(score, level_db),
            ListenEvent::Chunk(samples) => self.stream(&samples),
            ListenEvent::Ended { cut } => {
                self.end(cut.map(|(a, b)| (a as u32, b as u32)), false);
            }
            ListenEvent::NoSpeech => self.end(None, true),
            // Jamais produit en flux.
            ListenEvent::Utterance(_) => {}
        }
    }

    fn on_wake(&mut self, score: f32, level_db: f32) {
        tracing::info!("mot d'éveil détecté (score {score:.2})");
        // « Jarvis » coupe la parole de Jarvis, et une question en attente.
        if self.player.is_playing() {
            self.player.stop();
        }
        self.awaiting_reply = false;
        self.utterance = self.utterance.wrapping_add(1);
        let utterance = self.utterance;
        let wake = EarMessage::Wake {
            utterance,
            score,
            level_db,
            muted: self.muted,
        };
        if self.link.send(wake).is_err() {
            // Pas de capture : rien n'est envoyé ni gardé.
            tracing::info!("cerveau injoignable : le PC est endormi");
            self.listener.reset();
            self.say_asleep();
            self.link.nudge();
            return;
        }
        // Nouvelle phrase : la réponse de la précédente est mise de côté jusqu'à la
        // décision du cerveau (élue : oubliée ; refusée car il est occupé : rétablie).
        if let Some(previous) = self.awaited.take() {
            self.superseded = Some(previous);
        }
        // Écoute coupée : pas de bip (le mot d'éveil n'est là que pour réactiver).
        if !self.muted {
            let rate = self.player.sample_rate();
            self.player.play(sounds::wake(rate), rate);
        }
        self.batcher = AudioBatcher::new();
        self.current = Some(Current {
            utterance,
            muted: self.muted,
        });
        self.election = Some((utterance, Instant::now() + self.election_timeout));
    }

    /// Envoie le son de la capture en cours, par paquets de 100 ms.
    fn stream(&mut self, samples: &[f32]) {
        let Some(current) = self.current else {
            return;
        };
        for packet in self.batcher.push(samples) {
            if self.link.send_audio(current.utterance, &packet).is_err() {
                tracing::warn!("connexion au cerveau perdue pendant la capture");
                self.abandon();
                return;
            }
        }
    }

    /// Fin de la capture : reste du son, puis `listen_end`.
    fn end(&mut self, cut: Option<(u32, u32)>, no_speech: bool) {
        let Some(current) = self.current.take() else {
            return;
        };
        let rest = std::mem::take(&mut self.batcher).flush();
        let mut sent = match rest {
            Some(rest) if !no_speech => self.link.send_audio(current.utterance, &rest).is_ok(),
            _ => true,
        };
        sent = sent
            && self
                .link
                .send(EarMessage::ListenEnd {
                    utterance: current.utterance,
                    muted: current.muted,
                    no_speech,
                    cut,
                })
                .is_ok();
        if !sent {
            tracing::warn!("fin de phrase non transmise : connexion perdue");
            self.abandon();
            return;
        }
        self.awaited = Some(Awaited {
            utterance: current.utterance,
            muted: current.muted,
            since: Instant::now(),
        });
    }

    /// Lien perdu ou pas d'élection : phrase abandonnée, « le PC est endormi ».
    fn abandon(&mut self) {
        self.current = None;
        self.election = None;
        self.awaited = None;
        self.superseded = None;
        self.batcher = AudioBatcher::new();
        self.listener.reset();
        self.say_asleep();
        self.link.nudge();
    }

    /// Capture interrompue ici (coupure totale, micro perdu, départ) : fin de phrase
    /// vide envoyée au mieux, pour que le cerveau rétablisse le son.
    fn cancel_capture(&mut self) {
        if let Some(current) = self.current.take() {
            let _ = self.link.send(EarMessage::ListenEnd {
                utterance: current.utterance,
                muted: current.muted,
                no_speech: true,
                cut: None,
            });
            if self.election.is_some_and(|(u, _)| u == current.utterance) {
                self.election = None;
            }
        }
        self.batcher = AudioBatcher::new();
        self.listener.reset();
    }

    fn on_election_timeout(&mut self) {
        let Some((utterance, _)) = self.election.take() else {
            return;
        };
        tracing::warn!(
            "ni élue ni arrêtée en {:?} : le PC est tenu pour endormi",
            self.election_timeout
        );
        // Le lien peut sembler ouvert (cerveau figé, PC suspendu brutalement…) : fin de
        // phrase vide envoyée au mieux, pour que le cerveau ne traite pas une phrase
        // dont l'utilisateur a entendu « le PC est endormi » et rétablisse le son.
        let capturing = self.current.is_some_and(|c| c.utterance == utterance);
        let ended = self.awaited.filter(|a| a.utterance == utterance);
        if capturing {
            self.cancel_capture();
        } else if let Some(ended) = ended {
            // Phrase déjà finie : annulée si le cerveau ne l'a pas encore lancée.
            let _ = self.link.send(EarMessage::ListenEnd {
                utterance,
                muted: ended.muted,
                no_speech: true,
                cut: None,
            });
        }
        if capturing || ended.is_some() {
            self.abandon();
        }
    }

    /// « Le PC est endormi » (voix de Jarvis enregistrée), sinon bip d'échec. Rien en
    /// écoute coupée : le mot d'éveil n'y sert qu'à réactiver.
    fn say_asleep(&mut self) {
        if self.muted {
            return;
        }
        // À la suite du son en cours (bip de réveil) : la lecture est en file.
        match self.offline.as_ref().and_then(|offline| offline()) {
            Some(audio) => self.play_voice(&audio),
            None => {
                let rate = self.player.sample_rate();
                self.player.play(sounds::failure(rate), rate);
            }
        }
    }

    fn on_command(&mut self, command: EarCommand, cap_tx: &mpsc::Sender<CaptureEvent<C::Samples>>) {
        match command {
            EarCommand::Mute if !self.hard_muted => self.set_muted(true),
            EarCommand::Unmute if !self.hard_muted => self.set_muted(false),
            EarCommand::HardMute if !self.hard_muted => {
                self.capture.stop();
                self.hard_muted = true;
                self.downmixer = None;
                // La musique avait été baissée au réveil : on la rétablit ; plus de
                // capture en cours, donc pas de fin orpheline à la réouverture.
                self.cancel_capture();
            }
            EarCommand::Reopen if self.hard_muted => {
                self.hard_muted = false;
                self.unavailable = false;
                if let Err(e) = self.capture.start(cap_tx.clone()) {
                    tracing::warn!("micro indisponible : {e:#}");
                    self.unavailable = true;
                }
            }
            EarCommand::SetMode(mode) => {
                // Le cerveau diffuse ensuite le nouvel état.
                if let Err(e) = self.link.send(EarMessage::SetMode { mode }) {
                    tracing::warn!("changement de mode impossible : {e}");
                }
            }
            _ => {}
        }
    }

    fn play_voice(&mut self, audio: &SpeechAudio) {
        match decode_pcm(&audio.pcm) {
            Ok(pcm) if audio.sample_rate > 0 => {
                let samples = pcm.into_iter().map(i16_to_f32).collect();
                self.player.play(samples, audio.sample_rate);
            }
            Ok(_) => tracing::warn!("voix de Jarvis sans fréquence : ignorée"),
            Err(e) => tracing::warn!("voix de Jarvis illisible : {e}"),
        }
    }

    /// Joue le bip puis la voix de la réponse. Renvoie `false` si une nouvelle phrase est
    /// déjà en cours de capture : la réponse précédente n'est alors pas jouée (le micro
    /// capterait la voix de Jarvis).
    fn play_reply(&mut self, sound: Option<Sound>, audio: Option<SpeechAudio>) -> bool {
        if self.current.is_some() {
            tracing::debug!("réponse non jouée : nouvelle phrase en cours");
            return false;
        }
        let rate = self.player.sample_rate();
        match sound {
            Some(Sound::Success) => self.player.play(sounds::success(rate), rate),
            Some(Sound::Failure) => self.player.play(sounds::failure(rate), rate),
            None => {}
        }
        if let Some(audio) = audio {
            self.play_voice(&audio);
        }
        true
    }

    /// Lecture finie : écoute la réponse attendue, sinon joue les annonces en file.
    fn on_playback(&mut self) {
        self.listen_for_reply();
        self.play_announcements();
    }

    /// Lecture finie et question en attente : écoute la réponse sans mot d'éveil.
    /// Jamais en écoute coupée : seul le mot d'éveil peut y ouvrir une capture.
    fn listen_for_reply(&mut self) {
        if !self.awaiting_reply || *self.playing.borrow_and_update() {
            return;
        }
        self.awaiting_reply = false;
        if self.current.is_some()
            || self.muted
            || self.hard_muted
            || self.unavailable
            || self.downmixer.is_none()
        {
            tracing::debug!("réponse non écoutée : écoute coupée, micro occupé ou fermé");
            return;
        }
        self.utterance = self.utterance.wrapping_add(1);
        let utterance = self.utterance;
        if self
            .link
            .send(EarMessage::ListenStart { utterance })
            .is_err()
        {
            tracing::info!("cerveau injoignable : réponse non écoutée");
            self.say_asleep();
            self.link.nudge();
            return;
        }
        tracing::info!("écoute de la réponse");
        self.listener.listen_now(REPLY_WAIT);
        self.batcher = AudioBatcher::new();
        self.current = Some(Current {
            utterance,
            muted: false,
        });
        // Comme après un réveil : cerveau figé, « le PC est endormi » plutôt que le
        // silence.
        self.election = Some((utterance, Instant::now() + self.election_timeout));
    }

    /// Au repos : ni capture, ni lecture, ni réponse attendue, ni élection en cours.
    fn idle(&self) -> bool {
        self.current.is_none()
            && !self.player.is_playing()
            && !self.awaiting_reply
            && self.election.is_none()
            && self
                .awaited
                .is_none_or(|a| a.since.elapsed() >= RESPONSE_TIMEOUT)
    }

    /// Joue les annonces en file si l'oreille est au repos, même écoute coupée
    /// (sortie seulement). « Jarvis » les coupe comme une réponse.
    fn play_announcements(&mut self) {
        if self.announcements.is_empty() || !self.idle() {
            return;
        }
        for announcement in std::mem::take(&mut self.announcements) {
            tracing::info!("annonce jouée");
            tracing::debug!("Jarvis : {}", announcement.speech);
            self.play_reply(announcement.sound, announcement.audio);
        }
    }

    fn on_heard(&mut self, response: ControlResponse, muted: bool) {
        match response {
            ControlResponse::Heard {
                text,
                speech,
                ear_action,
                audio,
                sound,
            } => {
                if !speech.is_empty() {
                    tracing::debug!("Jarvis : {speech}");
                }
                if !muted {
                    if !text.is_empty() {
                        tracing::info!("entendu : {text}");
                        self.last_heard = Some(text);
                    }
                } else if ear_action == Some(EarAction::Unmute) {
                    // Seule la réactivation est journalisée pendant l'écoute coupée.
                    tracing::info!("entendu : {text}");
                }
                let played = self.play_reply(sound, audio);
                match ear_action {
                    Some(EarAction::Mute) => self.set_muted(true),
                    Some(EarAction::Unmute) => self.set_muted(false),
                    Some(EarAction::AwaitReply) if played => {
                        self.awaiting_reply = true;
                        self.listen_for_reply();
                    }
                    Some(EarAction::AwaitReply) | None => {}
                }
            }
            ControlResponse::Error { message } => {
                tracing::warn!("le cerveau signale une erreur : {message}");
                // Traitement interrompu ou trop long : pas de silence. Rien en écoute
                // coupée, comme pour toute réponse qui n'est pas la réactivation.
                if !muted {
                    self.play_reply(Some(Sound::Failure), None);
                }
            }
            _ => {}
        }
    }
}
