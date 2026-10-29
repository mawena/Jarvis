//! Boucle principale de l'oreille : trames du micro, mot d'éveil, phrase envoyée en
//! flux au cerveau par le réseau, élection, réponses jouées, commandes de l'icône.
//!
//! Hors connexion, rien n'est capté ni gardé : « Jarvis » fait dire « le PC est
//! endormi » (phrase enregistrée, sinon bip d'échec) et relance la connexion.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use jarvis_ear_core::listener::{ListenEvent, Listener};
use jarvis_ear_core::net::batch::AudioBatcher;
use jarvis_ear_core::net::{ELECTION_TIMEOUT, LinkEvent, LinkState};
use jarvis_ear_core::resample::{Downmixer, i16_to_f32};
use jarvis_ear_core::rustpotter::RustpotterDetector;
use jarvis_ear_core::{MicState, Vad, WakeDetector};
use jarvis_protocol::ear::{BrainMessage, EarMessage};
use jarvis_protocol::{
    Announcement, ControlRequest, ControlResponse, DEFAULT_EAR, EarAction, Sound, SpeechAudio,
    decode_pcm,
};
use tokio::sync::{mpsc, watch};
use tokio::time::Instant;

use crate::capture::{Capture, CaptureEvent};
use crate::client::{Brain, ClientError, RESPONSE_TIMEOUT};
use crate::link::Link;
use crate::playback::{Player, SilentPlayer};
use crate::sounds;
use crate::tray::{BrainLink, TrayCommand, TrayState};

/// Période de relève des annonces (socket Unix, oreille du PC).
pub const ANNOUNCE_PERIOD: Duration = Duration::from_secs(5);

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

/// Résultats des requêtes lancées en tâche de fond.
enum Internal {
    Announcements(Result<ControlResponse, ClientError>),
}

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

pub struct App<W: WakeDetector, V: Vad, C: Capture> {
    listener: Listener<W, V>,
    capture: C,
    link: Arc<dyn Link>,
    events: mpsc::Receiver<LinkEvent>,
    link_state: watch::Receiver<LinkState>,
    /// Socket de contrôle (oreille du PC) : relève des annonces, en secours.
    control: Option<Arc<dyn Brain>>,
    commands: mpsc::UnboundedReceiver<TrayCommand>,
    tray: watch::Sender<TrayState>,
    announce_period: Duration,
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
    /// Une question attend la fin de la lecture pour écouter la réponse.
    awaiting_reply: bool,
    player: Box<dyn Player>,
    playing: watch::Receiver<bool>,
    brain: BrainLink,
    last_heard: Option<String>,
    /// Nom de l'oreille, pour les annonces.
    name: String,
    /// Faux si le cerveau ne connaît pas les annonces (ancienne version).
    announcements_supported: bool,
    announcements_in_flight: bool,
    /// Annonces relevées, jouées au repos.
    announcements: Vec<Announcement>,
    /// Score de réveil minimal pendant la lecture.
    playback_wake_score: f32,
    offline: Option<OfflinePhrase>,
    internal_tx: mpsc::UnboundedSender<Internal>,
    internal_rx: mpsc::UnboundedReceiver<Internal>,
}

impl<W: WakeDetector, V: Vad, C: Capture> App<W, V, C> {
    /// L'écoute passe en mode flux.
    pub fn new(
        listener: Listener<W, V>,
        capture: C,
        link: Arc<dyn Link>,
        events: mpsc::Receiver<LinkEvent>,
        commands: mpsc::UnboundedReceiver<TrayCommand>,
        tray: watch::Sender<TrayState>,
    ) -> Self {
        let (internal_tx, internal_rx) = mpsc::unbounded_channel();
        let player = SilentPlayer::new();
        let playing = player.playing();
        let link_state = link.state();
        Self {
            listener: listener.with_streaming(true),
            capture,
            link,
            events,
            link_state,
            control: None,
            commands,
            tray,
            announce_period: ANNOUNCE_PERIOD,
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
            awaiting_reply: false,
            player: Box::new(player),
            playing,
            brain: BrainLink::Connecting,
            last_heard: None,
            name: DEFAULT_EAR.to_string(),
            announcements_supported: true,
            announcements_in_flight: false,
            announcements: Vec::new(),
            playback_wake_score: RustpotterDetector::DEFAULT_THRESHOLD + PLAYBACK_WAKE_MARGIN,
            offline: None,
            internal_tx,
            internal_rx,
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

    /// Nom de l'oreille pour les annonces (défaut : `pc`).
    pub fn with_name(mut self, name: String) -> Self {
        self.name = name;
        self
    }

    /// Relève des annonces par le socket de contrôle (oreille du PC).
    pub fn with_control(mut self, control: Arc<dyn Brain>) -> Self {
        self.control = Some(control);
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
        let state = TrayState {
            mic: self.mic(),
            capturing: self.current.is_some(),
            brain: self.brain.clone(),
            last_heard: self.last_heard.clone(),
        };
        self.tray.send_if_modified(|current| {
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
        let mut tick = tokio::time::interval(self.announce_period);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            let election = self.election.map(|(_, deadline)| deadline);
            tokio::select! {
                Some(event) = cap_rx.recv() => self.on_capture(event),
                command = self.commands.recv() => match command {
                    Some(TrayCommand::Quit) | None => break,
                    Some(command) => self.on_command(command, &cap_tx),
                },
                Some(event) = self.events.recv() => self.on_link_event(event),
                Ok(()) = self.link_state.changed() => self.on_link_state(),
                Some(internal) = self.internal_rx.recv() => self.on_internal(internal),
                Ok(()) = self.playing.changed() => self.on_playback(),
                () = tokio::time::sleep_until(election.unwrap_or_else(Instant::now)),
                    if election.is_some() => self.on_election_timeout(),
                _ = tick.tick() => self.poll_announcements(),
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
            // Cerveau revenu (peut-être mis à jour) : on redemande les annonces.
            LinkEvent::Connected(_) => self.announcements_supported = true,
            LinkEvent::Disconnected => {
                // La réponse attendue ne viendra plus.
                self.awaited = None;
                if self.current.is_some() || self.election.is_some() {
                    tracing::warn!("connexion au cerveau perdue pendant la phrase");
                    self.abandon();
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
                }
            }
            BrainMessage::Stop { utterance } => {
                if self.election.is_some_and(|(u, _)| u == utterance) {
                    self.election = None;
                }
                if self.awaited.is_some_and(|a| a.utterance == utterance) {
                    self.awaited = None;
                }
                if self.current.is_some_and(|c| c.utterance == utterance) {
                    tracing::info!("capture arrêtée par le cerveau (autre oreille ou phrase)");
                    self.current = None;
                    self.batcher = AudioBatcher::new();
                    self.listener.reset();
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
            BrainMessage::Response {
                utterance: None,
                response: ControlResponse::Error { message },
            } => tracing::warn!("le cerveau signale une erreur : {message}"),
            // L'état (mode) est déjà publié par le lien.
            _ => {}
        }
    }

    fn on_capture(&mut self, event: CaptureEvent) {
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
        // Nouvelle phrase : la réponse de la précédente n'est plus jouée.
        self.awaited = None;
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
        if self.current.is_some_and(|c| c.utterance == utterance)
            || self.awaited.is_some_and(|a| a.utterance == utterance)
        {
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

    fn on_command(&mut self, command: TrayCommand, cap_tx: &mpsc::Sender<CaptureEvent>) {
        match command {
            TrayCommand::Mute if !self.hard_muted => self.set_muted(true),
            TrayCommand::Unmute if !self.hard_muted => self.set_muted(false),
            TrayCommand::HardMute if !self.hard_muted => {
                self.capture.stop();
                self.hard_muted = true;
                self.downmixer = None;
                // La musique avait été baissée au réveil : on la rétablit ; plus de
                // capture en cours, donc pas de fin orpheline à la réouverture.
                self.cancel_capture();
            }
            TrayCommand::Reopen if self.hard_muted => {
                self.hard_muted = false;
                self.unavailable = false;
                if let Err(e) = self.capture.start(cap_tx.clone()) {
                    tracing::warn!("micro indisponible : {e:#}");
                    self.unavailable = true;
                }
            }
            TrayCommand::SetMode(mode) => {
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

    /// Relève les annonces par le socket de contrôle (le protocole réseau ne les porte
    /// pas encore), seulement lien établi.
    fn poll_announcements(&mut self) {
        let Some(control) = self.control.clone() else {
            return;
        };
        if !self.announcements_supported
            || self.announcements_in_flight
            || !matches!(self.brain, BrainLink::Connected { .. })
        {
            return;
        }
        self.announcements_in_flight = true;
        let tx = self.internal_tx.clone();
        let ear = Some(self.name.clone());
        tokio::spawn(async move {
            let _ = tx.send(Internal::Announcements(
                control.request(ControlRequest::Announcements { ear }).await,
            ));
        });
    }

    fn on_internal(&mut self, internal: Internal) {
        match internal {
            Internal::Announcements(result) => {
                self.announcements_in_flight = false;
                match result {
                    Ok(ControlResponse::Announcements { items }) => {
                        self.announcements.extend(items);
                        self.play_announcements();
                    }
                    // Ancien cerveau : il ne connaît pas cette requête.
                    Ok(ControlResponse::Error { message }) => {
                        tracing::debug!(
                            "annonces non prises en charge ({message}) : relève arrêtée"
                        );
                        self.announcements_supported = false;
                    }
                    Ok(_) => {}
                    Err(e) => tracing::debug!("annonces non relevées : {e}"),
                }
            }
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
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::Samples;
    use jarvis_ear_core::net::LinkDown;
    use jarvis_protocol::ear::{RejectReason, StatusInfo};
    use jarvis_protocol::{Mode, encode_pcm};
    use std::collections::HashSet;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use tokio::task::JoinHandle;

    /// Détecteur de mot d'éveil qui se déclenche toutes les `every` trames de 160 échantillons.
    struct FakeWake {
        every: usize,
        frame: usize,
        /// Nombre maximal de déclenchements.
        left: usize,
        score: f32,
    }
    impl WakeDetector for FakeWake {
        fn frame_len(&self) -> usize {
            160
        }
        fn process(&mut self, _: &[f32]) -> Option<f32> {
            self.frame += 1;
            if self.left > 0 && self.frame.is_multiple_of(self.every) {
                self.left -= 1;
                return Some(self.score);
            }
            None
        }
    }

    /// Détecteur de parole : parole pendant les `speech_frames` premières trames
    /// analysées, et pour toute trame plus forte que 0,5.
    struct FakeVad {
        speech_frames: usize,
        frame: usize,
    }
    impl Vad for FakeVad {
        fn frame_len(&self) -> usize {
            256
        }
        fn is_speech(&mut self, frame: &[f32]) -> bool {
            self.frame += 1;
            self.frame <= self.speech_frames || frame.iter().any(|v| v.abs() > 0.5)
        }
    }

    /// Ce que l'application a demandé au lecteur.
    #[derive(Debug, Clone, PartialEq)]
    enum Played {
        Sound { len: usize, rate: u32 },
        Stop,
    }

    /// Fréquence de sortie du faux lecteur.
    const OUT_RATE: u32 = 48_000;

    struct PlayerShared {
        calls: Mutex<Vec<Played>>,
        playing: watch::Sender<bool>,
    }
    impl PlayerShared {
        /// Fin de la lecture (le faux lecteur ne finit jamais seul).
        fn finish(&self) {
            self.playing.send_replace(false);
        }
    }

    struct FakePlayer(Arc<PlayerShared>);
    impl Player for FakePlayer {
        fn sample_rate(&self) -> u32 {
            OUT_RATE
        }
        fn play(&mut self, samples: Vec<f32>, rate: u32) {
            self.0.calls.lock().unwrap().push(Played::Sound {
                len: samples.len(),
                rate,
            });
            self.0.playing.send_replace(true);
        }
        fn stop(&mut self) {
            self.0.calls.lock().unwrap().push(Played::Stop);
            self.0.playing.send_replace(false);
        }
        fn playing(&self) -> watch::Receiver<bool> {
            self.0.playing.subscribe()
        }
    }

    fn wake_beep() -> Played {
        Played::Sound {
            len: sounds::wake(OUT_RATE).len(),
            rate: OUT_RATE,
        }
    }

    fn success_beep() -> Played {
        Played::Sound {
            len: sounds::success(OUT_RATE).len(),
            rate: OUT_RATE,
        }
    }

    fn failure_beep() -> Played {
        Played::Sound {
            len: sounds::failure(OUT_RATE).len(),
            rate: OUT_RATE,
        }
    }

    #[derive(Default)]
    struct CaptureShared {
        tx: Mutex<Option<mpsc::Sender<CaptureEvent>>>,
        starts: AtomicUsize,
        stops: AtomicUsize,
    }

    struct FakeCapture {
        shared: Arc<CaptureShared>,
        fail: bool,
    }
    impl Capture for FakeCapture {
        fn start(&mut self, tx: mpsc::Sender<CaptureEvent>) -> anyhow::Result<()> {
            self.shared.starts.fetch_add(1, Ordering::SeqCst);
            if self.fail {
                return Err(anyhow::anyhow!("aucun micro"));
            }
            *self.shared.tx.lock().unwrap() = Some(tx);
            Ok(())
        }
        fn stop(&mut self) {
            self.shared.stops.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// Ce que le faux lien a accepté.
    #[derive(Debug, Clone, PartialEq)]
    enum Sent {
        Msg(EarMessage),
        Audio { utterance: u32, len: usize },
    }

    /// Réponse du faux cerveau à une phrase finie ; `follow_up` : réponse à une question.
    type Answer = dyn Fn(bool) -> Option<ControlResponse> + Send + Sync;

    /// Faux lien : hors connexion, tout échoue (rien n'est gardé, mais chaque essai est
    /// compté) ; connecté, le faux cerveau élit l'oreille et répond.
    struct FakeLink {
        connected: AtomicBool,
        /// Le faux cerveau répond `elected` au réveil.
        elect: AtomicBool,
        /// Le faux cerveau répond `stop` au réveil.
        stop: AtomicBool,
        sent: Mutex<Vec<Sent>>,
        attempts: AtomicUsize,
        nudges: AtomicUsize,
        closed: AtomicBool,
        follow_ups: Mutex<HashSet<u32>>,
        state: watch::Sender<LinkState>,
        events: mpsc::Sender<LinkEvent>,
        answer: Box<Answer>,
    }

    impl FakeLink {
        fn reply(&self, message: BrainMessage) {
            self.events
                .try_send(LinkEvent::Message(message))
                .expect("événement transmis");
        }

        fn disconnect(&self) {
            self.connected.store(false, Ordering::SeqCst);
            self.state.send_replace(LinkState::Connecting);
            self.events.try_send(LinkEvent::Disconnected).unwrap();
        }

        fn reconnect(&self) {
            self.connected.store(true, Ordering::SeqCst);
            self.state
                .send_replace(LinkState::Connected(status(Mode::Local)));
            self.events
                .try_send(LinkEvent::Connected(status(Mode::Local)))
                .unwrap();
        }
    }

    #[async_trait::async_trait]
    impl Link for FakeLink {
        fn send(&self, message: EarMessage) -> Result<(), LinkDown> {
            self.attempts.fetch_add(1, Ordering::SeqCst);
            if !self.connected.load(Ordering::SeqCst) {
                return Err(LinkDown);
            }
            self.sent.lock().unwrap().push(Sent::Msg(message.clone()));
            match message {
                EarMessage::Wake { utterance, .. } => {
                    if self.stop.load(Ordering::SeqCst) {
                        self.reply(BrainMessage::Stop { utterance });
                    } else if self.elect.load(Ordering::SeqCst) {
                        self.reply(BrainMessage::Elected { utterance });
                    }
                }
                EarMessage::ListenStart { utterance } => {
                    self.follow_ups.lock().unwrap().insert(utterance);
                }
                EarMessage::ListenEnd { utterance, .. } => {
                    let follow_up = self.follow_ups.lock().unwrap().contains(&utterance);
                    if let Some(response) = (self.answer)(follow_up) {
                        self.reply(BrainMessage::Response {
                            utterance: Some(utterance),
                            response,
                        });
                    }
                }
                _ => {}
            }
            Ok(())
        }
        fn send_audio(&self, utterance: u32, samples: &[i16]) -> Result<(), LinkDown> {
            self.attempts.fetch_add(1, Ordering::SeqCst);
            if !self.connected.load(Ordering::SeqCst) {
                return Err(LinkDown);
            }
            if !samples.is_empty() {
                self.sent.lock().unwrap().push(Sent::Audio {
                    utterance,
                    len: samples.len(),
                });
            }
            Ok(())
        }
        fn nudge(&self) {
            self.nudges.fetch_add(1, Ordering::SeqCst);
        }
        fn state(&self) -> watch::Receiver<LinkState> {
            self.state.subscribe()
        }
        async fn close(&self) {
            self.closed.store(true, Ordering::SeqCst);
        }
    }

    type Responder = dyn Fn(&ControlRequest) -> Result<ControlResponse, ClientError> + Send + Sync;

    /// Faux socket de contrôle (annonces).
    struct FakeBrain {
        requests: Mutex<Vec<ControlRequest>>,
        responder: Box<Responder>,
    }
    #[async_trait::async_trait]
    impl Brain for FakeBrain {
        async fn request(&self, req: ControlRequest) -> Result<ControlResponse, ClientError> {
            let answer = (self.responder)(&req);
            self.requests.lock().unwrap().push(req);
            answer
        }
    }

    fn status(mode: Mode) -> StatusInfo {
        StatusInfo {
            mode,
            version: "test".into(),
            local_model: "llama3.2:3b".into(),
            ears: vec![],
        }
    }

    /// Réglages du banc d'essai.
    struct Setup {
        answer: Box<Answer>,
        capture_fails: bool,
        speech_frames: usize,
        wakes: usize,
        mute_file: Option<PathBuf>,
        wake_score: f32,
        announce_period: Duration,
        name: Option<&'static str>,
        control: Option<Arc<FakeBrain>>,
        offline: Option<SpeechAudio>,
        connected: bool,
        elect: bool,
    }

    impl Default for Setup {
        fn default() -> Self {
            Setup {
                answer: Box::new(|_| Some(silent())),
                capture_fails: false,
                speech_frames: 0,
                wakes: usize::MAX,
                mute_file: None,
                wake_score: 0.9,
                announce_period: ANNOUNCE_PERIOD,
                name: None,
                control: None,
                offline: None,
                connected: true,
                elect: true,
            }
        }
    }

    struct Harness {
        capture: Arc<CaptureShared>,
        link: Arc<FakeLink>,
        control: Option<Arc<FakeBrain>>,
        player: Arc<PlayerShared>,
        commands: mpsc::UnboundedSender<TrayCommand>,
        tray: watch::Receiver<TrayState>,
        task: JoinHandle<anyhow::Result<()>>,
    }

    impl Setup {
        fn answer(
            mut self,
            answer: impl Fn(bool) -> Option<ControlResponse> + Send + Sync + 'static,
        ) -> Self {
            self.answer = Box::new(answer);
            self
        }

        fn start(self) -> Harness {
            let shared = Arc::new(CaptureShared::default());
            let (events_tx, events) = mpsc::channel(1_024);
            let initial = if self.connected {
                LinkState::Connected(status(Mode::Local))
            } else {
                LinkState::Unreachable
            };
            let link = Arc::new(FakeLink {
                connected: AtomicBool::new(self.connected),
                elect: AtomicBool::new(self.elect),
                stop: AtomicBool::new(false),
                sent: Mutex::new(Vec::new()),
                attempts: AtomicUsize::new(0),
                nudges: AtomicUsize::new(0),
                closed: AtomicBool::new(false),
                follow_ups: Mutex::new(HashSet::new()),
                state: watch::channel(initial).0,
                events: events_tx,
                answer: self.answer,
            });
            let (commands, commands_rx) = mpsc::unbounded_channel();
            let (tray_tx, tray) = watch::channel(TrayState {
                mic: MicState::Open,
                capturing: false,
                brain: BrainLink::Connecting,
                last_heard: None,
            });
            let listener = Listener::new(
                FakeWake {
                    every: 10,
                    frame: 0,
                    left: self.wakes,
                    score: self.wake_score,
                },
                FakeVad {
                    speech_frames: self.speech_frames,
                    frame: 0,
                },
            );
            let capture = FakeCapture {
                shared: shared.clone(),
                fail: self.capture_fails,
            };
            let player = Arc::new(PlayerShared {
                calls: Mutex::new(Vec::new()),
                playing: watch::channel(false).0,
            });
            let mut app = App::new(
                listener,
                capture,
                link.clone(),
                events,
                commands_rx,
                tray_tx,
            )
            .with_player(Box::new(FakePlayer(player.clone())));
            if let Some(path) = self.mute_file {
                app = app.with_mute_file(path);
            }
            if let Some(name) = self.name {
                app = app.with_name(name.to_string());
            }
            if let Some(control) = &self.control {
                app = app.with_control(control.clone());
            }
            if let Some(audio) = self.offline {
                app = app.with_offline_phrase(Arc::new(move || Some(audio.clone())));
            }
            app.announce_period = self.announce_period;
            Harness {
                capture: shared,
                link,
                control: self.control,
                player,
                commands,
                tray,
                task: tokio::spawn(app.run()),
            }
        }
    }

    impl Harness {
        async fn wait_for(&self, what: &str, cond: impl Fn(&Harness) -> bool) {
            for _ in 0..300 {
                if cond(self) {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            panic!("délai dépassé : {what}");
        }

        async fn started(&self) {
            self.wait_for("capture", |h| h.capture.tx.lock().unwrap().is_some())
                .await;
        }

        async fn feed(&self, samples: Vec<f32>) {
            let tx = self
                .capture
                .tx
                .lock()
                .unwrap()
                .clone()
                .expect("capture démarrée");
            tx.send(CaptureEvent::Format {
                rate: 16_000,
                channels: 1,
            })
            .await
            .unwrap();
            for chunk in samples.chunks(1_600) {
                tx.send(CaptureEvent::Samples(Samples::detached(chunk.to_vec())))
                    .await
                    .unwrap();
            }
        }

        fn sent(&self) -> Vec<Sent> {
            self.link.sent.lock().unwrap().clone()
        }

        fn messages(&self) -> Vec<EarMessage> {
            self.sent()
                .into_iter()
                .filter_map(|s| match s {
                    Sent::Msg(m) => Some(m),
                    Sent::Audio { .. } => None,
                })
                .collect()
        }

        fn audio_of(&self, utterance: u32) -> usize {
            self.sent()
                .iter()
                .map(|s| match s {
                    Sent::Audio { utterance: u, len } if *u == utterance => *len,
                    _ => 0,
                })
                .sum()
        }

        fn played(&self) -> Vec<Played> {
            self.player.calls.lock().unwrap().clone()
        }

        fn nudges(&self) -> usize {
            self.link.nudges.load(Ordering::SeqCst)
        }

        fn wakes(&self) -> usize {
            self.messages()
                .iter()
                .filter(|m| matches!(m, EarMessage::Wake { .. }))
                .count()
        }

        /// `muted` de chaque fin de phrase.
        fn listens(&self) -> Vec<bool> {
            self.messages()
                .into_iter()
                .filter_map(|m| match m {
                    EarMessage::ListenEnd { muted, .. } => Some(muted),
                    _ => None,
                })
                .collect()
        }

        /// Chaque fin de phrase : (parole envoyée, réponse à une question).
        fn follow_ups(&self) -> Vec<(bool, bool)> {
            let starts: Vec<u32> = self
                .messages()
                .into_iter()
                .filter_map(|m| match m {
                    EarMessage::ListenStart { utterance } => Some(utterance),
                    _ => None,
                })
                .collect();
            self.messages()
                .into_iter()
                .filter_map(|m| match m {
                    EarMessage::ListenEnd {
                        utterance,
                        no_speech,
                        ..
                    } => Some((
                        !no_speech && self.audio_of(utterance) > 0,
                        starts.contains(&utterance),
                    )),
                    _ => None,
                })
                .collect()
        }

        fn announcement_requests(&self) -> Vec<Option<String>> {
            self.control
                .as_ref()
                .expect("socket de contrôle")
                .requests
                .lock()
                .unwrap()
                .iter()
                .filter_map(|r| match r {
                    ControlRequest::Announcements { ear } => Some(ear.clone()),
                    _ => None,
                })
                .collect()
        }
    }

    /// Réveil à la trame 10 (1 600 échantillons), puis 4 s sans parole (marge comprise).
    fn wake_then_silence() -> Vec<f32> {
        vec![0.0; 65_900]
    }

    /// Réveil, « suivant » (parole pendant les 20 premières trames du VAD), puis silence.
    fn wake_then_speech() -> Vec<f32> {
        vec![0.1; 1_600 + 30_000]
    }

    /// Rien entendu : réponse vide, sans son.
    fn silent() -> ControlResponse {
        ControlResponse::Heard {
            text: String::new(),
            speech: String::new(),
            ear_action: None,
            audio: None,
            sound: None,
        }
    }

    /// Voix de Jarvis : 2 205 échantillons à 22,05 kHz (0,1 s).
    fn voice() -> Option<SpeechAudio> {
        Some(SpeechAudio {
            pcm: encode_pcm(&[1_000; 2_205]),
            sample_rate: 22_050,
        })
    }

    fn voice_played() -> Played {
        Played::Sound {
            len: 2_205,
            rate: 22_050,
        }
    }

    /// « Le PC est endormi » : 1 600 échantillons à 16 kHz.
    fn asleep() -> SpeechAudio {
        SpeechAudio {
            pcm: encode_pcm(&[500; 1_600]),
            sample_rate: 16_000,
        }
    }

    fn asleep_played() -> Played {
        Played::Sound {
            len: 1_600,
            rate: 16_000,
        }
    }

    fn heard(text: &str, ear_action: Option<EarAction>, sound: Option<Sound>) -> ControlResponse {
        ControlResponse::Heard {
            text: text.into(),
            speech: "Bien.".into(),
            ear_action,
            audio: voice(),
            sound,
        }
    }

    #[tokio::test]
    async fn icone_pc_injoignable() {
        let h = Setup {
            connected: false,
            ..Setup::default()
        }
        .start();
        h.wait_for("icône injoignable", |h| {
            h.tray.borrow().brain == BrainLink::Unreachable
        })
        .await;
        assert_eq!(h.tray.borrow().mic, MicState::Open);
        h.link
            .state
            .send_replace(LinkState::Rejected(RejectReason::BadToken));
        h.wait_for("appairage refusé", |h| {
            h.tray.borrow().brain == BrainLink::Refused
        })
        .await;
        h.link.state.send_replace(LinkState::Connecting);
        h.wait_for("connexion", |h| {
            h.tray.borrow().brain == BrainLink::Connecting
        })
        .await;
    }

    #[tokio::test]
    async fn etat_pousse_par_le_cerveau_affiche_le_mode() {
        let h = Setup::default().start();
        h.wait_for("lien établi", |h| {
            h.tray.borrow().brain
                == BrainLink::Connected {
                    mode: Mode::Local,
                    local_model: "llama3.2:3b".into(),
                }
        })
        .await;
        h.link
            .state
            .send_replace(LinkState::Connected(status(Mode::Online)));
        h.wait_for("mode en ligne", |h| {
            matches!(
                h.tray.borrow().brain,
                BrainLink::Connected {
                    mode: Mode::Online,
                    ..
                }
            )
        })
        .await;
    }

    #[tokio::test]
    async fn mode_change_par_le_lien() {
        let h = Setup::default().start();
        h.commands.send(TrayCommand::SetMode(Mode::Online)).unwrap();
        h.wait_for("mode demandé", |h| {
            h.messages() == vec![EarMessage::SetMode { mode: Mode::Online }]
        })
        .await;
    }

    #[tokio::test]
    async fn capture_absente_etat_indisponible() {
        let h = Setup {
            capture_fails: true,
            ..Setup::default()
        }
        .start();
        h.wait_for("micro indisponible", |h| {
            h.tray.borrow().mic == MicState::Unavailable
        })
        .await;
    }

    #[tokio::test]
    async fn perte_du_micro_puis_retour() {
        let h = Setup::default().start();
        h.started().await;
        let tx = h.capture.tx.lock().unwrap().clone().unwrap();
        tx.send(CaptureEvent::Unavailable).await.unwrap();
        h.wait_for("indisponible", |h| {
            h.tray.borrow().mic == MicState::Unavailable
        })
        .await;
        tx.send(CaptureEvent::Format {
            rate: 16_000,
            channels: 1,
        })
        .await
        .unwrap();
        h.wait_for("rétabli", |h| h.tray.borrow().mic == MicState::Open)
            .await;
    }

    #[tokio::test]
    async fn reveil_envoie_wake_puis_le_flux() {
        let h = Setup {
            speech_frames: 20,
            wakes: 1,
            ..Setup::default()
        }
        .answer(|_| Some(heard("suivant", None, None)))
        .start();
        h.started().await;
        h.feed(wake_then_speech()).await;
        h.wait_for("phrase entendue", |h| {
            h.tray.borrow().last_heard.as_deref() == Some("suivant")
        })
        .await;
        let sent = h.sent();
        assert!(
            matches!(
                &sent[0],
                Sent::Msg(EarMessage::Wake { utterance: 1, score, muted: false, .. })
                    if (*score - 0.9).abs() < 1e-6
            ),
            "{:?}",
            sent[0]
        );
        // Le son suit le réveil, par paquets d'au plus 100 ms, avant la fin de phrase.
        let audio: Vec<usize> = sent[1..sent.len() - 1]
            .iter()
            .map(|s| match s {
                Sent::Audio { utterance: 1, len } => *len,
                other => panic!("{other:?}"),
            })
            .collect();
        assert!(audio.len() > 5, "{audio:?}");
        assert!(audio.iter().all(|len| *len <= 1_600));
        assert!(matches!(
            sent.last().unwrap(),
            Sent::Msg(EarMessage::ListenEnd {
                utterance: 1,
                muted: false,
                no_speech: false,
                ..
            })
        ));
        assert!(!h.tray.borrow().capturing);
    }

    #[tokio::test]
    async fn stop_abandonne_la_capture_sans_reponse() {
        let h = Setup {
            wakes: 1,
            offline: Some(asleep()),
            ..Setup::default()
        }
        .start();
        h.link.stop.store(true, Ordering::SeqCst);
        h.started().await;
        h.feed(vec![0.0; 3_200]).await;
        h.wait_for("réveil", |h| h.wakes() == 1).await;
        h.wait_for("capture arrêtée", |h| !h.tray.borrow().capturing)
            .await;
        let before = h.sent().len();
        // La suite du son n'est plus envoyée, et aucune fin de phrase.
        h.feed(vec![0.0; 70_000]).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(h.sent().len(), before, "{:?}", h.sent());
        assert!(h.listens().is_empty());
        assert_eq!(h.played(), vec![wake_beep()], "rien d'autre n'est joué");
        // Ni « PC endormi » à l'échéance de l'élection.
        tokio::time::sleep(ELECTION_TIMEOUT).await;
        assert_eq!(h.played(), vec![wake_beep()]);
    }

    #[tokio::test]
    async fn reponse_d_une_ancienne_phrase_ignoree() {
        let h = Setup {
            wakes: 2,
            ..Setup::default()
        }
        // Le cerveau tarde : pas de réponse tout de suite.
        .answer(|_| None)
        .start();
        h.started().await;
        h.feed(wake_then_silence()).await;
        h.wait_for("première phrase", |h| h.listens().len() == 1)
            .await;
        // Seconde phrase en cours.
        h.feed(vec![0.0; 3_200]).await;
        h.wait_for("second réveil", |h| h.wakes() == 2).await;
        h.link.reply(BrainMessage::Response {
            utterance: Some(1),
            response: heard("ancienne", None, Some(Sound::Success)),
        });
        // Fin de la seconde phrase, puis une réponse sans numéro : ignorée aussi.
        h.feed(vec![0.0; 70_000]).await;
        h.wait_for("seconde fin", |h| h.listens().len() == 2).await;
        h.link.reply(BrainMessage::Response {
            utterance: None,
            response: heard("sans numéro", None, Some(Sound::Success)),
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        // Le second réveil coupe le premier bip (le faux lecteur ne finit jamais seul).
        assert_eq!(h.played(), vec![wake_beep(), Played::Stop, wake_beep()]);
        assert_eq!(h.tray.borrow().last_heard, None);
        // La réponse de la dernière phrase est jouée.
        h.link.reply(BrainMessage::Response {
            utterance: Some(2),
            response: heard("dernière", None, None),
        });
        h.wait_for("dernière jouée", |h| {
            h.tray.borrow().last_heard.as_deref() == Some("dernière")
        })
        .await;
    }

    #[tokio::test]
    async fn cerveau_injoignable_dit_pc_endormi() {
        let h = Setup {
            connected: false,
            wakes: 1,
            offline: Some(asleep()),
            ..Setup::default()
        }
        .start();
        h.started().await;
        h.feed(vec![0.0; 3_200]).await;
        h.wait_for("phrase hors ligne", |h| h.played() == vec![asleep_played()])
            .await;
        assert!(h.nudges() >= 1);
        assert!(!h.tray.borrow().capturing, "pas de capture");
    }

    #[tokio::test]
    async fn sans_phrase_enregistree_bip_d_echec() {
        let h = Setup {
            connected: false,
            wakes: 1,
            ..Setup::default()
        }
        .start();
        h.started().await;
        h.feed(vec![0.0; 3_200]).await;
        h.wait_for("bip d'échec", |h| h.played() == vec![failure_beep()])
            .await;
        assert!(h.nudges() >= 1);
    }

    #[tokio::test]
    async fn rien_n_est_envoye_ni_garde_hors_connexion() {
        let h = Setup {
            connected: false,
            wakes: 1,
            speech_frames: 20,
            ..Setup::default()
        }
        .start();
        h.started().await;
        // « Jarvis, suivant » pendant que le PC dort.
        h.feed(wake_then_speech()).await;
        h.wait_for("bip d'échec", |h| h.played() == vec![failure_beep()])
            .await;
        // Un seul essai : le réveil. Aucun son tenté, rien de capté.
        assert_eq!(h.link.attempts.load(Ordering::SeqCst), 1);
        assert!(!h.tray.borrow().capturing);
        // Le PC revient : rien n'est rattrapé.
        h.link.reconnect();
        h.wait_for("lien établi", |h| {
            matches!(h.tray.borrow().brain, BrainLink::Connected { .. })
        })
        .await;
        h.feed(vec![0.1; 30_000]).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(h.sent().is_empty(), "{:?}", h.sent());
        assert_eq!(h.link.attempts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn pas_d_election_en_deux_secondes_pc_endormi() {
        assert_eq!(ELECTION_TIMEOUT, Duration::from_secs(2));
        // Connexion apparemment ouverte, mais le cerveau ne répond plus.
        let h = Setup {
            elect: false,
            wakes: 1,
            offline: Some(asleep()),
            ..Setup::default()
        }
        .start();
        h.started().await;
        let start = std::time::Instant::now();
        h.feed(vec![0.0; 3_200]).await;
        h.wait_for("réveil", |h| h.wakes() == 1).await;
        assert!(h.tray.borrow().capturing);
        h.wait_for("PC endormi", |h| {
            h.played() == vec![wake_beep(), asleep_played()]
        })
        .await;
        let elapsed = start.elapsed();
        assert!(elapsed >= Duration::from_millis(1_900), "{elapsed:?}");
        assert!(elapsed < Duration::from_millis(2_500), "{elapsed:?}");
        assert!(!h.tray.borrow().capturing, "capture abandonnée");
        assert!(h.nudges() >= 1);
        // Plus rien n'est envoyé pour cette phrase.
        let before = h.sent().len();
        h.feed(vec![0.0; 16_000]).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(h.sent().len(), before);
    }

    #[tokio::test]
    async fn election_recue_la_capture_continue() {
        let h = Setup {
            wakes: 1,
            offline: Some(asleep()),
            ..Setup::default()
        }
        .start();
        h.started().await;
        h.feed(vec![0.0; 3_200]).await;
        h.wait_for("réveil", |h| h.wakes() == 1).await;
        tokio::time::sleep(ELECTION_TIMEOUT + Duration::from_millis(200)).await;
        assert!(h.tray.borrow().capturing);
        assert_eq!(h.played(), vec![wake_beep()]);
    }

    #[tokio::test]
    async fn coupure_pendant_la_capture() {
        let h = Setup {
            wakes: 1,
            offline: Some(asleep()),
            ..Setup::default()
        }
        .start();
        h.started().await;
        h.feed(vec![0.0; 3_200]).await;
        h.wait_for("capture en cours", |h| h.tray.borrow().capturing)
            .await;
        h.link.disconnect();
        h.wait_for("PC endormi", |h| {
            h.played() == vec![wake_beep(), asleep_played()]
        })
        .await;
        assert!(!h.tray.borrow().capturing);
        assert!(h.nudges() >= 1);
        // La suite n'est ni tentée ni gardée, même après la reconnexion.
        let attempts = h.link.attempts.load(Ordering::SeqCst);
        h.link.reconnect();
        h.feed(vec![0.0; 70_000]).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(h.link.attempts.load(Ordering::SeqCst), attempts);
        assert!(h.listens().is_empty());
        assert_eq!(h.played().len(), 2, "phrase dite une seule fois");
    }

    #[tokio::test]
    async fn action_mute_appliquee() {
        let h = Setup::default()
            .answer(|_| Some(heard("coupe l'écoute", Some(EarAction::Mute), None)))
            .start();
        h.started().await;
        h.feed(wake_then_silence()).await;
        h.wait_for("écoute coupée", |h| {
            h.tray.borrow().mic == MicState::Muted
        })
        .await;
        // La phrase suivante part marquée « muted » et n'alimente pas « dernière phrase ».
        h.feed(wake_then_silence()).await;
        h.wait_for("deuxième phrase", |h| h.listens().len() == 2)
            .await;
        assert_eq!(h.listens(), vec![false, true]);
        assert!(h.messages().iter().any(|m| matches!(
            m,
            EarMessage::Wake {
                utterance: 2,
                muted: true,
                ..
            }
        )));
        assert_eq!(
            h.tray.borrow().last_heard.as_deref(),
            Some("coupe l'écoute")
        );
    }

    #[tokio::test]
    async fn pas_de_derniere_phrase_pendant_l_ecoute_coupee() {
        let h = Setup::default()
            .answer(|_| {
                Some(ControlResponse::Heard {
                    text: "secret".into(),
                    speech: String::new(),
                    ear_action: None,
                    audio: None,
                    sound: None,
                })
            })
            .start();
        h.commands.send(TrayCommand::Mute).unwrap();
        h.wait_for("coupée", |h| h.tray.borrow().mic == MicState::Muted)
            .await;
        h.started().await;
        h.feed(wake_then_silence()).await;
        h.wait_for("phrase envoyée", |h| h.listens() == vec![true])
            .await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(h.tray.borrow().last_heard, None);
    }

    #[tokio::test]
    async fn coupure_totale_arrete_la_capture() {
        let h = Setup::default().start();
        h.wait_for("capture démarrée", |h| {
            h.capture.starts.load(Ordering::SeqCst) == 1
        })
        .await;
        h.commands.send(TrayCommand::HardMute).unwrap();
        h.wait_for("flux fermé", |h| {
            h.capture.stops.load(Ordering::SeqCst) == 1
        })
        .await;
        h.wait_for("icône", |h| h.tray.borrow().mic == MicState::HardMuted)
            .await;
        h.commands.send(TrayCommand::Reopen).unwrap();
        h.wait_for("flux rouvert", |h| {
            h.capture.starts.load(Ordering::SeqCst) == 2
        })
        .await;
        h.wait_for("icône ouverte", |h| h.tray.borrow().mic == MicState::Open)
            .await;
        h.commands.send(TrayCommand::Quit).unwrap();
        h.task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn coupure_totale_pendant_la_capture_puis_reouverture_sans_fin_orpheline() {
        let h = Setup {
            wakes: 1,
            ..Setup::default()
        }
        .start();
        h.started().await;
        // Réveil seul : la capture est en cours (aucune parole, pas encore d'abandon).
        h.feed(vec![0.0; 3_200]).await;
        h.wait_for("réveil", |h| h.wakes() == 1).await;
        h.commands.send(TrayCommand::HardMute).unwrap();
        // Une fin de phrase vide rétablit la musique.
        h.wait_for("fin vide", |h| h.listens() == vec![false]).await;
        assert!(matches!(
            h.messages().last(),
            Some(EarMessage::ListenEnd {
                no_speech: true,
                ..
            })
        ));
        h.commands.send(TrayCommand::Reopen).unwrap();
        h.wait_for("flux rouvert", |h| {
            h.capture.starts.load(Ordering::SeqCst) == 2
        })
        .await;
        // Plus de 4 s de silence : sans réinitialisation, l'ancienne capture enverrait
        // une seconde fin (abandon faute de parole).
        h.feed(vec![0.0; 70_000]).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(h.listens(), vec![false]);
    }

    #[tokio::test]
    async fn perte_du_micro_pendant_la_capture_retablit_la_musique() {
        let h = Setup {
            wakes: 1,
            ..Setup::default()
        }
        .start();
        h.started().await;
        h.feed(vec![0.0; 3_200]).await;
        h.wait_for("réveil", |h| h.wakes() == 1).await;
        let tx = h.capture.tx.lock().unwrap().clone().unwrap();
        tx.send(CaptureEvent::Unavailable).await.unwrap();
        h.wait_for("fin vide", |h| h.listens() == vec![false]).await;
        h.wait_for("indisponible", |h| {
            h.tray.borrow().mic == MicState::Unavailable
        })
        .await;
        // Micro de retour : pas de seconde fin issue de l'ancienne capture.
        h.feed(vec![0.0; 70_000]).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(h.listens(), vec![false]);
        assert!(!h.tray.borrow().capturing);
    }

    #[tokio::test]
    async fn quitter_pendant_la_capture_previent_le_cerveau() {
        let h = Setup {
            wakes: 1,
            ..Setup::default()
        }
        .start();
        h.started().await;
        // Réveil seul : la capture est en cours.
        h.feed(vec![0.0; 3_200]).await;
        h.wait_for("capture en cours", |h| h.tray.borrow().capturing)
            .await;
        h.commands.send(TrayCommand::Quit).unwrap();
        let link = h.link.clone();
        h.task.await.unwrap().unwrap();
        // La fin vide est partie avant la fermeture du lien, pas dans une tâche détachée.
        let messages: Vec<EarMessage> = link
            .sent
            .lock()
            .unwrap()
            .iter()
            .filter_map(|s| match s {
                Sent::Msg(m) => Some(m.clone()),
                Sent::Audio { .. } => None,
            })
            .collect();
        assert_eq!(messages.len(), 2, "{messages:?}");
        assert!(matches!(messages[0], EarMessage::Wake { utterance: 1, .. }));
        assert_eq!(
            messages[1],
            EarMessage::ListenEnd {
                utterance: 1,
                muted: false,
                no_speech: true,
                cut: None,
            }
        );
        assert!(link.closed.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn quitter_hors_capture_ferme_le_lien_sans_fin_de_phrase() {
        let h = Setup {
            wakes: 1,
            ..Setup::default()
        }
        .start();
        h.started().await;
        h.commands.send(TrayCommand::Quit).unwrap();
        let link = h.link.clone();
        h.task.await.unwrap().unwrap();
        assert!(link.sent.lock().unwrap().is_empty());
        assert!(link.closed.load(Ordering::SeqCst));
    }

    fn mute_file_setup(path: &std::path::Path) -> Harness {
        Setup {
            mute_file: Some(path.to_path_buf()),
            ..Setup::default()
        }
        .start()
    }

    #[tokio::test]
    async fn ecoute_coupee_retrouvee_au_redemarrage() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jarvis/ear-muted");
        let h = mute_file_setup(&path);
        h.wait_for("ouverte", |h| h.tray.borrow().mic == MicState::Open)
            .await;
        h.commands.send(TrayCommand::Mute).unwrap();
        h.wait_for("coupée", |h| h.tray.borrow().mic == MicState::Muted)
            .await;
        assert!(path.exists());
        h.commands.send(TrayCommand::Quit).unwrap();
        h.task.await.unwrap().unwrap();

        // Redémarrage : toujours coupée.
        let h = mute_file_setup(&path);
        h.wait_for("coupée au démarrage", |h| {
            h.tray.borrow().mic == MicState::Muted
        })
        .await;
        h.commands.send(TrayCommand::Unmute).unwrap();
        h.wait_for("rouverte", |h| h.tray.borrow().mic == MicState::Open)
            .await;
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn coupure_par_la_voix_aussi_retenue() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ear-muted");
        let h = Setup {
            mute_file: Some(path.clone()),
            ..Setup::default()
        }
        .answer(|_| Some(heard("coupe l'écoute", Some(EarAction::Mute), None)))
        .start();
        h.started().await;
        h.feed(wake_then_silence()).await;
        h.wait_for("coupée", |h| h.tray.borrow().mic == MicState::Muted)
            .await;
        assert!(path.exists());
    }

    #[tokio::test]
    async fn coupure_totale_non_retenue() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ear-muted");
        let h = mute_file_setup(&path);
        h.commands.send(TrayCommand::HardMute).unwrap();
        h.wait_for("coupure totale", |h| {
            h.tray.borrow().mic == MicState::HardMuted
        })
        .await;
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn phrase_en_ecoute_coupee_porte_le_son() {
        // Parole pendant 20 trames de VAD après le réveil, puis silence.
        let h = Setup {
            speech_frames: 20,
            wakes: 1,
            ..Setup::default()
        }
        .start();
        h.commands.send(TrayCommand::Mute).unwrap();
        h.wait_for("coupée", |h| h.tray.borrow().mic == MicState::Muted)
            .await;
        h.started().await;
        h.feed(wake_then_speech()).await;
        h.wait_for("phrase envoyée", |h| h.listens() == vec![true])
            .await;
        assert_eq!(h.follow_ups(), vec![(true, false)]);
    }

    #[tokio::test]
    async fn reveil_joue_le_bip() {
        let h = Setup {
            wakes: 1,
            ..Setup::default()
        }
        .start();
        h.started().await;
        h.feed(vec![0.0; 3_200]).await;
        h.wait_for("bip", |h| h.played() == vec![wake_beep()]).await;
        assert!(h.tray.borrow().capturing);
    }

    #[tokio::test]
    async fn ecoute_coupee_sans_bip_de_reveil() {
        let h = Setup {
            wakes: 1,
            ..Setup::default()
        }
        .start();
        h.commands.send(TrayCommand::Mute).unwrap();
        h.wait_for("coupée", |h| h.tray.borrow().mic == MicState::Muted)
            .await;
        h.started().await;
        h.feed(vec![0.0; 3_200]).await;
        h.wait_for("réveil", |h| h.tray.borrow().capturing).await;
        assert!(h.played().is_empty(), "{:?}", h.played());
    }

    #[tokio::test]
    async fn ecoute_coupee_hors_connexion_reste_muette() {
        let h = Setup {
            connected: false,
            wakes: 1,
            offline: Some(asleep()),
            ..Setup::default()
        }
        .start();
        h.commands.send(TrayCommand::Mute).unwrap();
        h.wait_for("coupée", |h| h.tray.borrow().mic == MicState::Muted)
            .await;
        h.started().await;
        h.feed(vec![0.0; 3_200]).await;
        h.wait_for("relance", |h| h.nudges() >= 1).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(h.played().is_empty(), "{:?}", h.played());
    }

    #[tokio::test]
    async fn reponse_jouee_apres_le_bip() {
        let h = Setup::default()
            .answer(|_| Some(heard("suivant", None, Some(Sound::Success))))
            .start();
        h.started().await;
        h.feed(wake_then_silence()).await;
        h.wait_for("réponse jouée", |h| h.played().len() == 3).await;
        assert_eq!(
            h.played(),
            vec![wake_beep(), success_beep(), voice_played()]
        );
    }

    #[tokio::test]
    async fn reveil_coupe_la_lecture() {
        let h = Setup {
            wakes: 2,
            ..Setup::default()
        }
        .answer(|_| Some(heard("quelle heure est-il", None, None)))
        .start();
        h.started().await;
        h.feed(wake_then_silence()).await;
        h.wait_for("voix jouée", |h| {
            h.played() == vec![wake_beep(), voice_played()]
        })
        .await;
        // « Jarvis » pendant que Jarvis parle : coupure, bip, puis capture normale.
        h.feed(wake_then_silence()).await;
        h.wait_for("seconde phrase", |h| h.listens().len() == 2)
            .await;
        assert_eq!(
            h.played()[..4],
            [wake_beep(), voice_played(), Played::Stop, wake_beep()]
        );
        assert_eq!(h.wakes(), 2);
    }

    /// Première phrase : le cerveau pose une question ; réponse « oui » ensuite.
    fn question() -> Harness {
        Setup {
            wakes: 1,
            ..Setup::default()
        }
        .answer(|follow_up| {
            Some(if follow_up {
                ControlResponse::Heard {
                    text: "oui".into(),
                    speech: String::new(),
                    ear_action: None,
                    audio: None,
                    sound: None,
                }
            } else {
                heard("mets le PC en veille", Some(EarAction::AwaitReply), None)
            })
        })
        .start()
    }

    /// Question posée et lue ; la lecture n'est pas encore finie.
    async fn ask(h: &Harness) {
        h.started().await;
        h.feed(wake_then_silence()).await;
        h.wait_for("question lue", |h| {
            h.played() == vec![wake_beep(), voice_played()]
        })
        .await;
        // Pas d'écoute tant que Jarvis parle.
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!h.tray.borrow().capturing);
        h.player.finish();
        h.wait_for("écoute de la réponse", |h| h.tray.borrow().capturing)
            .await;
    }

    /// Garde du début d'écoute, « oui » (20 trames de parole), puis le silence qui clôt
    /// la phrase.
    fn yes() -> Vec<f32> {
        let mut reply = vec![0.0; 256 * 20];
        reply.extend(vec![0.9; 256 * 20]);
        reply.extend(vec![0.0; 256 * 60]);
        reply
    }

    #[tokio::test]
    async fn attente_de_reponse_ouvre_un_flux_follow_up() {
        let h = question();
        ask(&h).await;
        assert!(
            h.messages()
                .contains(&EarMessage::ListenStart { utterance: 2 })
        );
        h.feed(yes()).await;
        h.wait_for("réponse entendue", |h| {
            h.tray.borrow().last_heard.as_deref() == Some("oui")
        })
        .await;
        assert_eq!(h.follow_ups(), vec![(false, false), (true, true)]);
        assert!(h.audio_of(2) > 0);
        // Aucun réveil (donc aucun son baissé) pour la réponse.
        assert_eq!(h.wakes(), 1);
        assert!(!h.tray.borrow().capturing);
    }

    #[tokio::test]
    async fn attente_sans_parole_abandonnee() {
        let h = question();
        ask(&h).await;
        // 5 s de silence : on attend toujours (le délai est de 8 s, pas 4).
        h.feed(vec![0.0; 5 * 16_000]).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(h.follow_ups().len(), 1);
        assert!(h.tray.borrow().capturing);
        // Au-delà de 8 s : abandon, signalé par une fin sans parole.
        h.feed(vec![0.0; 3 * 16_000 + 1_000]).await;
        h.wait_for("abandon", |h| h.follow_ups().len() == 2).await;
        assert_eq!(h.follow_ups()[1], (false, true));
        h.wait_for("fin de capture", |h| !h.tray.borrow().capturing)
            .await;
        assert_eq!(h.wakes(), 1);
    }

    #[tokio::test]
    async fn question_pendant_l_ecoute_coupee_pas_d_ecoute_de_reponse() {
        let h = question();
        h.commands.send(TrayCommand::Mute).unwrap();
        h.wait_for("coupée", |h| h.tray.borrow().mic == MicState::Muted)
            .await;
        h.started().await;
        h.feed(wake_then_silence()).await;
        // Pas de bip de réveil en écoute coupée : seule la voix est lue.
        h.wait_for("question lue", |h| h.played() == vec![voice_played()])
            .await;
        h.player.finish();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!h.tray.borrow().capturing, "pas d'écoute directe");
        assert_eq!(h.follow_ups().len(), 1);
    }

    #[tokio::test]
    async fn reponse_tardive_pendant_l_ecoute_de_la_reponse_ignoree() {
        let h = question();
        ask(&h).await;
        let before = h.played();
        // Réponse de la phrase précédente qui arrive pendant l'écoute de la réponse.
        h.link.reply(BrainMessage::Response {
            utterance: Some(1),
            response: heard("autre", Some(EarAction::AwaitReply), Some(Sound::Success)),
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(h.played(), before, "rien n'est joué");
        assert!(h.tray.borrow().capturing, "capture intacte");
        h.feed(yes()).await;
        h.wait_for("réponse entendue", |h| {
            h.tray.borrow().last_heard.as_deref() == Some("oui")
        })
        .await;
        assert_eq!(h.follow_ups().len(), 2);
        assert!(h.follow_ups()[1].1);
        assert!(!h.tray.borrow().capturing);
    }

    #[tokio::test]
    async fn question_hors_connexion_pc_endormi() {
        let h = question();
        h.started().await;
        h.feed(wake_then_silence()).await;
        h.wait_for("question lue", |h| {
            h.played() == vec![wake_beep(), voice_played()]
        })
        .await;
        h.link.disconnect();
        h.player.finish();
        h.wait_for("bip d'échec", |h| {
            h.played() == vec![wake_beep(), voice_played(), failure_beep()]
        })
        .await;
        assert!(!h.tray.borrow().capturing);
    }

    #[tokio::test]
    async fn reveil_faible_ignore_pendant_que_jarvis_parle() {
        // Score au-dessus du seuil normal (0,5), sous le seuil de lecture.
        let h = Setup {
            wake_score: 0.55,
            ..Setup::default()
        }
        .start();
        h.started().await;
        // Jarvis parle : sa propre voix dit « Jarvis ».
        h.player.playing.send_replace(true);
        h.feed(vec![0.0; 3_200]).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(h.wakes(), 0);
        assert!(
            h.played().is_empty(),
            "lecture non coupée : {:?}",
            h.played()
        );
        assert!(!h.tray.borrow().capturing);
        // Lecture finie : le même score réveille.
        h.player.finish();
        h.feed(vec![0.0; 3_200]).await;
        h.wait_for("réveil", |h| h.wakes() == 1).await;
    }

    #[tokio::test]
    async fn reveil_franc_coupe_toujours_la_lecture() {
        let h = Setup {
            wakes: 1,
            ..Setup::default()
        }
        .start();
        h.started().await;
        h.player.playing.send_replace(true);
        h.feed(vec![0.0; 3_200]).await;
        h.wait_for("réveil", |h| h.wakes() == 1).await;
        assert_eq!(h.played(), vec![Played::Stop, wake_beep()]);
    }

    #[tokio::test]
    async fn question_sans_lecture_ecoute_tout_de_suite() {
        let h = Setup {
            wakes: 1,
            ..Setup::default()
        }
        .answer(|follow_up| {
            (!follow_up).then(|| ControlResponse::Heard {
                text: "mets le PC en veille".into(),
                speech: "Je mets le PC en veille ?".into(),
                ear_action: Some(EarAction::AwaitReply),
                audio: None,
                sound: None,
            })
        })
        .start();
        h.started().await;
        h.feed(wake_then_silence()).await;
        h.wait_for("bip lu", |h| h.played() == vec![wake_beep()])
            .await;
        h.player.finish();
        // Voix absente : l'écoute de la réponse commence dès la fin du bip.
        h.wait_for("écoute de la réponse", |h| h.tray.borrow().capturing)
            .await;
    }

    /// Socket de contrôle qui donne une annonce (bip de réussite et voix) une seule fois,
    /// dès que `ready` est levé ; `delivered` est levé quand elle est partie.
    struct Announcing {
        ready: Arc<AtomicBool>,
        delivered: Arc<AtomicBool>,
    }

    impl Announcing {
        fn new(ready: bool) -> Self {
            Self {
                ready: Arc::new(AtomicBool::new(ready)),
                delivered: Arc::new(AtomicBool::new(false)),
            }
        }

        fn control(&self) -> Arc<FakeBrain> {
            let (ready, delivered) = (self.ready.clone(), self.delivered.clone());
            Arc::new(FakeBrain {
                requests: Mutex::new(Vec::new()),
                responder: Box::new(move |r| match r {
                    ControlRequest::Announcements { .. } => {
                        let items = if ready.load(Ordering::SeqCst)
                            && !delivered.swap(true, Ordering::SeqCst)
                        {
                            vec![Announcement {
                                speech: "La tâche est finie.".into(),
                                audio: voice(),
                                sound: Some(Sound::Success),
                            }]
                        } else {
                            vec![]
                        };
                        Ok(ControlResponse::Announcements { items })
                    }
                    other => panic!("requête inattendue : {other:?}"),
                }),
            })
        }

        fn ready(&self) {
            self.ready.store(true, Ordering::SeqCst);
        }

        fn delivered(&self) -> bool {
            self.delivered.load(Ordering::SeqCst)
        }
    }

    /// Relève rapide (20 ms), `wakes` réveils au plus.
    fn announcing(control: Arc<FakeBrain>, wakes: usize) -> Harness {
        Setup {
            wakes,
            control: Some(control),
            announce_period: Duration::from_millis(20),
            ..Setup::default()
        }
        .start()
    }

    #[tokio::test]
    async fn annonce_jouee_au_repos() {
        let brain = Announcing::new(true);
        let h = announcing(brain.control(), 0);
        h.wait_for("annonce jouée", |h| {
            h.played() == vec![success_beep(), voice_played()]
        })
        .await;
        // Relevée sous le nom par défaut.
        assert_eq!(h.announcement_requests()[0].as_deref(), Some("pc"));
        // Livrée une fois : jouée une fois.
        h.wait_for("relèves suivantes", |h| {
            h.announcement_requests().len() >= 3
        })
        .await;
        assert_eq!(h.played().len(), 2);
    }

    #[tokio::test]
    async fn annonce_jouee_ecoute_coupee() {
        let brain = Announcing::new(false);
        let h = announcing(brain.control(), 0);
        h.commands.send(TrayCommand::Mute).unwrap();
        h.wait_for("coupée", |h| h.tray.borrow().mic == MicState::Muted)
            .await;
        brain.ready();
        h.wait_for("annonce jouée", |h| {
            h.played() == vec![success_beep(), voice_played()]
        })
        .await;
    }

    #[tokio::test]
    async fn annonce_attend_la_fin_de_la_capture() {
        let brain = Announcing::new(false);
        let h = announcing(brain.control(), 1);
        h.started().await;
        // Réveil seul : la capture est en cours.
        h.feed(vec![0.0; 3_200]).await;
        h.wait_for("capture en cours", |h| h.tray.borrow().capturing)
            .await;
        h.player.finish();
        brain.ready();
        h.wait_for("annonce relevée", |_| brain.delivered()).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(h.played(), vec![wake_beep()], "rien pendant la capture");
        // Fin de la capture (abandon faute de parole), puis réponse vide : annonce.
        h.feed(vec![0.0; 70_000]).await;
        h.wait_for("annonce jouée", |h| {
            h.played() == vec![wake_beep(), success_beep(), voice_played()]
        })
        .await;
        assert_eq!(h.listens().len(), 1);
    }

    #[tokio::test]
    async fn annonce_attend_la_fin_de_la_lecture() {
        let brain = Announcing::new(false);
        let h = announcing(brain.control(), 0);
        // Jarvis parle déjà.
        h.player.playing.send_replace(true);
        brain.ready();
        h.wait_for("annonce relevée", |_| brain.delivered()).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(h.played().is_empty(), "{:?}", h.played());
        h.player.finish();
        h.wait_for("annonce jouée", |h| {
            h.played() == vec![success_beep(), voice_played()]
        })
        .await;
    }

    #[tokio::test]
    async fn reveil_coupe_l_annonce() {
        let brain = Announcing::new(true);
        let h = announcing(brain.control(), 1);
        h.wait_for("annonce jouée", |h| {
            h.played() == vec![success_beep(), voice_played()]
        })
        .await;
        h.started().await;
        // « Jarvis » pendant l'annonce : coupure, bip, capture.
        h.feed(vec![0.0; 3_200]).await;
        h.wait_for("réveil", |h| h.wakes() == 1).await;
        assert_eq!(
            h.played(),
            vec![success_beep(), voice_played(), Played::Stop, wake_beep()]
        );
        assert!(h.tray.borrow().capturing);
    }

    #[tokio::test]
    async fn pas_de_releve_hors_connexion() {
        let brain = Announcing::new(true);
        let h = Setup {
            connected: false,
            control: Some(brain.control()),
            announce_period: Duration::from_millis(20),
            ..Setup::default()
        }
        .start();
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(h.announcement_requests().is_empty());
        h.link.reconnect();
        h.wait_for("annonce jouée", |h| {
            h.played() == vec![success_beep(), voice_played()]
        })
        .await;
    }

    /// 0 : ancien cerveau (annonces inconnues) ; ensuite : nouveau cerveau.
    fn old_brain(phase: Arc<AtomicUsize>) -> Arc<FakeBrain> {
        Arc::new(FakeBrain {
            requests: Mutex::new(Vec::new()),
            responder: Box::new(move |_| {
                if phase.load(Ordering::SeqCst) == 0 {
                    Ok(ControlResponse::Error {
                        message: "requête invalide".into(),
                    })
                } else {
                    Ok(ControlResponse::Announcements { items: vec![] })
                }
            }),
        })
    }

    #[tokio::test]
    async fn ancien_cerveau_sans_annonces_ignore() {
        let h = announcing(old_brain(Arc::new(AtomicUsize::new(0))), 0);
        h.wait_for("première relève", |h| {
            h.announcement_requests().len() == 1
        })
        .await;
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(h.announcement_requests().len(), 1, "plus de relève");
        assert!(matches!(h.tray.borrow().brain, BrainLink::Connected { .. }));
    }

    #[tokio::test]
    async fn cerveau_revenu_annonces_redemandees() {
        let phase = Arc::new(AtomicUsize::new(0));
        let h = announcing(old_brain(phase.clone()), 0);
        h.wait_for("première relève", |h| {
            h.announcement_requests().len() == 1
        })
        .await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(h.announcement_requests().len(), 1);
        h.link.disconnect();
        h.wait_for("connexion perdue", |h| {
            h.tray.borrow().brain == BrainLink::Connecting
        })
        .await;
        phase.store(1, Ordering::SeqCst);
        h.link.reconnect();
        h.wait_for("relève reprise", |h| h.announcement_requests().len() >= 3)
            .await;
    }

    #[tokio::test]
    async fn annonces_relevees_sous_le_nom_de_l_oreille() {
        let brain = Announcing::new(false);
        let h = Setup {
            name: Some("salon"),
            control: Some(brain.control()),
            announce_period: Duration::from_millis(20),
            ..Setup::default()
        }
        .start();
        h.wait_for("relèves", |h| h.announcement_requests().len() >= 2)
            .await;
        assert!(
            h.announcement_requests()
                .iter()
                .all(|e| e.as_deref() == Some("salon"))
        );
    }
}
