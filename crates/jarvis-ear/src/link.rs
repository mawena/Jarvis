//! Lien réseau de l'oreille : le trait [`Link`] (vrai `EarLink` de jarvis-ear-core,
//! faux lien dans les tests) et la supervision qui appaire seule l'oreille du PC par
//! le socket Unix, puis la réappaire si le cerveau refuse son jeton.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::anyhow;
use jarvis_ear_core::net::{EarLink, LinkDown, LinkEvent, LinkState, Pairing, PairingStore};
use jarvis_protocol::ear::{EarMessage, RejectReason};
use jarvis_protocol::{ControlRequest, ControlResponse};
use tokio::sync::{mpsc, watch};

use crate::client::{Brain, ClientError};

/// Nom de l'oreille du PC dans le registre du cerveau.
pub const LOCAL_NAME: &str = "PC";

/// Délai entre deux essais d'appairage local (cerveau pas encore démarré).
pub const LOCAL_RETRY: Duration = Duration::from_secs(2);

/// Liaison avec le cerveau, vue de la boucle de l'oreille.
#[async_trait::async_trait]
pub trait Link: Send + Sync + 'static {
    /// Échoue tout de suite hors connexion : rien n'est gardé.
    fn send(&self, message: EarMessage) -> Result<(), LinkDown>;
    /// Son du micro ; jamais mis en attente hors connexion.
    fn send_audio(&self, utterance: u32, samples: &[i16]) -> Result<(), LinkDown>;
    /// Nouvelle tentative de connexion, tout de suite.
    fn nudge(&self);
    fn state(&self) -> watch::Receiver<LinkState>;
    /// Envoie ce qui est déjà accepté, puis ferme.
    async fn close(&self);
}

#[async_trait::async_trait]
impl Link for EarLink {
    fn send(&self, message: EarMessage) -> Result<(), LinkDown> {
        EarLink::send(self, message)
    }
    fn send_audio(&self, utterance: u32, samples: &[i16]) -> Result<(), LinkDown> {
        EarLink::send_audio(self, utterance, samples)
    }
    fn nudge(&self) {
        EarLink::nudge(self)
    }
    fn state(&self) -> watch::Receiver<LinkState> {
        EarLink::state(self)
    }
    async fn close(&self) {
        EarLink::close(self).await
    }
}

/// Appairage de l'oreille du PC par le socket Unix (qui vaut preuve, sans code) ;
/// connexion ensuite à `127.0.0.1:<port>` avec l'empreinte reçue.
pub async fn pair_local(control: &dyn Brain, name: &str) -> anyhow::Result<Pairing> {
    let request = ControlRequest::PairLocal { name: name.into() };
    match control.request(request).await {
        Ok(ControlResponse::PairedLocal {
            ear_id,
            token,
            port,
            fingerprint,
            brain_id,
            brain_name,
        }) => Ok(Pairing {
            brain_id,
            brain_name: jarvis_protocol::printable(&brain_name),
            ear_id,
            token,
            fingerprint,
            addresses: vec![SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port)],
            manual: None,
        }),
        Ok(ControlResponse::Error { message }) => {
            Err(anyhow!("appairage local refusé : {message}"))
        }
        Ok(other) => Err(anyhow!(
            "réponse inattendue à l'appairage local : {other:?}"
        )),
        Err(ClientError::BrainUnreachable) => Err(anyhow!("cerveau injoignable")),
        Err(e) => Err(anyhow!("appairage local impossible : {e}")),
    }
}

/// Appairage de l'oreille du PC (seulement la boucle locale, pas d'adresse manuelle).
pub fn is_local(pairing: &Pairing) -> bool {
    pairing.manual.is_none()
        && !pairing.addresses.is_empty()
        && pairing.addresses.iter().all(|a| a.ip().is_loopback())
}

/// Lance un lien pour un appairage.
pub type Spawner = Arc<dyn Fn(Pairing) -> (Arc<dyn Link>, mpsc::Receiver<LinkEvent>) + Send + Sync>;

/// Oreille du PC : appairage automatique par le socket Unix.
pub struct LocalPairing {
    pub control: Arc<dyn Brain>,
    pub name: String,
    pub retry: Duration,
}

/// Lien supervisé : remplacé par un nouveau après un réappairage local. Ses états et
/// ses événements sont ceux du lien en cours.
pub struct Supervised {
    inner: Arc<Mutex<Option<Arc<dyn Link>>>>,
    state: watch::Receiver<LinkState>,
    shutdown: watch::Sender<bool>,
}

impl Supervised {
    /// `pairing` absent : appairage local d'abord (`local` requis).
    pub fn spawn(
        pairing: Option<Pairing>,
        local: Option<LocalPairing>,
        store: PairingStore,
        spawner: Spawner,
    ) -> (Self, mpsc::Receiver<LinkEvent>) {
        let inner = Arc::new(Mutex::new(None));
        let (state_tx, state) = watch::channel(LinkState::Connecting);
        let (events_tx, events) = mpsc::channel(64);
        let (shutdown, shutdown_rx) = watch::channel(false);
        let task = Supervisor {
            pairing,
            local,
            store,
            spawner,
            inner: inner.clone(),
            state: state_tx,
            events: events_tx,
            shutdown: shutdown_rx,
        };
        tokio::spawn(task.run());
        (
            Supervised {
                inner,
                state,
                shutdown,
            },
            events,
        )
    }

    fn current(&self) -> Option<Arc<dyn Link>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

#[async_trait::async_trait]
impl Link for Supervised {
    fn send(&self, message: EarMessage) -> Result<(), LinkDown> {
        self.current().ok_or(LinkDown)?.send(message)
    }
    fn send_audio(&self, utterance: u32, samples: &[i16]) -> Result<(), LinkDown> {
        self.current()
            .ok_or(LinkDown)?
            .send_audio(utterance, samples)
    }
    fn nudge(&self) {
        if let Some(link) = self.current() {
            link.nudge();
        }
    }
    fn state(&self) -> watch::Receiver<LinkState> {
        self.state.clone()
    }
    async fn close(&self) {
        let link = self.inner.lock().unwrap_or_else(|e| e.into_inner()).take();
        self.shutdown.send_replace(true);
        if let Some(link) = link {
            link.close().await;
        }
    }
}

struct Supervisor {
    pairing: Option<Pairing>,
    local: Option<LocalPairing>,
    store: PairingStore,
    spawner: Spawner,
    inner: Arc<Mutex<Option<Arc<dyn Link>>>>,
    state: watch::Sender<LinkState>,
    events: mpsc::Sender<LinkEvent>,
    shutdown: watch::Receiver<bool>,
}

/// Refus qui appellent un nouvel appairage local (le registre du cerveau a changé).
fn needs_new_pairing(state: &LinkState) -> bool {
    matches!(
        state,
        LinkState::Rejected(RejectReason::UnknownEar | RejectReason::BadToken)
    )
}

impl Supervisor {
    async fn run(mut self) {
        loop {
            let pairing = match self.pairing.take() {
                Some(pairing) => pairing,
                None => match self.pair_locally().await {
                    Some(pairing) => pairing,
                    None => return,
                },
            };
            let (link, mut events) = (self.spawner)(pairing);
            let mut link_state = link.state();
            *self.inner.lock().unwrap_or_else(|e| e.into_inner()) = Some(link.clone());
            let mut state = link_state.borrow_and_update().clone();
            loop {
                if self.local.is_some() && needs_new_pairing(&state) {
                    tracing::warn!("jeton de l'oreille du PC refusé : nouvel appairage local");
                    self.inner.lock().unwrap_or_else(|e| e.into_inner()).take();
                    self.state.send_replace(LinkState::Connecting);
                    break;
                }
                self.state.send_replace(state.clone());
                tokio::select! {
                    changed = link_state.changed() => {
                        if changed.is_err() {
                            return;
                        }
                        state = link_state.borrow_and_update().clone();
                    }
                    event = events.recv() => match event {
                        Some(event) => {
                            let _ = self.events.send(event).await;
                        }
                        None => return,
                    },
                    _ = self.shutdown.changed() => return,
                }
            }
            // L'ancien lien s'arrête ici (lâché).
            drop(link);
        }
    }

    /// Essaie l'appairage local jusqu'à ce que le cerveau réponde ; `None` à l'arrêt.
    async fn pair_locally(&mut self) -> Option<Pairing> {
        let local = self.local.as_ref()?;
        let mut warned = false;
        loop {
            match pair_local(local.control.as_ref(), &local.name).await {
                Ok(pairing) => {
                    let store = self.store.clone();
                    let saved = pairing.clone();
                    match tokio::task::spawn_blocking(move || store.save(&saved)).await {
                        Ok(Ok(())) => {}
                        Ok(Err(e)) => tracing::warn!("appairage local non enregistré : {e}"),
                        Err(e) => tracing::warn!("appairage local non enregistré : {e}"),
                    }
                    tracing::info!("oreille du PC appairée à « {} »", pairing.brain_name);
                    return Some(pairing);
                }
                Err(e) if !warned => {
                    tracing::warn!("{e:#} : nouvel essai toutes les {:?}", local.retry);
                    warned = true;
                }
                Err(e) => tracing::debug!("{e:#}"),
            }
            tokio::select! {
                () = tokio::time::sleep(local.retry) => {}
                _ = self.shutdown.changed() => return None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jarvis_protocol::{EarId, Fingerprint};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Faux socket de contrôle : refuse `fail` fois, puis appaire (jeton `t1`, `t2`…).
    struct FakeControl {
        fail: AtomicUsize,
        calls: AtomicUsize,
        names: Mutex<Vec<String>>,
    }

    impl FakeControl {
        fn new(fail: usize) -> Arc<Self> {
            Arc::new(FakeControl {
                fail: AtomicUsize::new(fail),
                calls: AtomicUsize::new(0),
                names: Mutex::new(Vec::new()),
            })
        }
    }

    #[async_trait::async_trait]
    impl Brain for FakeControl {
        async fn request(&self, req: ControlRequest) -> Result<ControlResponse, ClientError> {
            let ControlRequest::PairLocal { name } = req else {
                panic!("requête inattendue : {req:?}");
            };
            self.names.lock().unwrap().push(name);
            if self
                .fail
                .try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                .is_ok()
            {
                return Err(ClientError::BrainUnreachable);
            }
            let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            Ok(ControlResponse::PairedLocal {
                ear_id: EarId::new("e1"),
                token: format!("t{n}"),
                port: 7341,
                fingerprint: Fingerprint::of_der(b"cert"),
                brain_id: "b1".into(),
                brain_name: "PC de Charles".into(),
            })
        }
    }

    struct FakeLink {
        state: watch::Sender<LinkState>,
    }

    #[async_trait::async_trait]
    impl Link for FakeLink {
        fn send(&self, _: EarMessage) -> Result<(), LinkDown> {
            Ok(())
        }
        fn send_audio(&self, _: u32, _: &[i16]) -> Result<(), LinkDown> {
            Ok(())
        }
        fn nudge(&self) {}
        fn state(&self) -> watch::Receiver<LinkState> {
            self.state.subscribe()
        }
        async fn close(&self) {}
    }

    /// Fabrique de faux liens : retient les appairages reçus et les liens créés.
    #[derive(Default)]
    struct Spawned {
        pairings: Mutex<Vec<Pairing>>,
        links: Mutex<Vec<Arc<FakeLink>>>,
        events: Mutex<Vec<mpsc::Sender<LinkEvent>>>,
    }

    fn spawner(spawned: Arc<Spawned>) -> Spawner {
        Arc::new(move |pairing| {
            let link = Arc::new(FakeLink {
                state: watch::channel(LinkState::Connecting).0,
            });
            let (tx, rx) = mpsc::channel(8);
            spawned.pairings.lock().unwrap().push(pairing);
            spawned.links.lock().unwrap().push(link.clone());
            spawned.events.lock().unwrap().push(tx);
            (link as Arc<dyn Link>, rx)
        })
    }

    async fn wait_for(what: &str, cond: impl Fn() -> bool) {
        for _ in 0..300 {
            if cond() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("délai dépassé : {what}");
    }

    fn local(control: Arc<FakeControl>) -> Option<LocalPairing> {
        Some(LocalPairing {
            control,
            name: LOCAL_NAME.into(),
            retry: Duration::from_millis(20),
        })
    }

    #[tokio::test]
    async fn appairage_local_automatique() {
        let dir = tempfile::tempdir().unwrap();
        let store = PairingStore::new(dir.path().join("ear"));
        // Le cerveau ne répond pas encore deux fois : l'oreille réessaie.
        let control = FakeControl::new(2);
        let spawned = Arc::new(Spawned::default());
        let (link, _events) = Supervised::spawn(
            None,
            local(control.clone()),
            store.clone(),
            spawner(spawned.clone()),
        );
        wait_for("lien lancé", || {
            spawned.pairings.lock().unwrap().len() == 1
        })
        .await;
        let pairing = spawned.pairings.lock().unwrap()[0].clone();
        assert_eq!(pairing.token, "t1");
        assert_eq!(pairing.brain_id, "b1");
        assert_eq!(pairing.fingerprint, Fingerprint::of_der(b"cert"));
        assert_eq!(pairing.addresses, vec!["127.0.0.1:7341".parse().unwrap()]);
        assert!(is_local(&pairing));
        assert_eq!(*control.names.lock().unwrap(), ["PC", "PC", "PC"]);
        // Enregistré pour le prochain démarrage.
        assert_eq!(store.load().unwrap(), Some(pairing));
        // L'état est celui du lien en cours.
        spawned.links.lock().unwrap()[0]
            .state
            .send_replace(LinkState::Unreachable);
        let mut state = link.state();
        tokio::time::timeout(
            Duration::from_secs(3),
            state.wait_for(|s| *s == LinkState::Unreachable),
        )
        .await
        .unwrap()
        .unwrap();
    }

    #[tokio::test]
    async fn jeton_refuse_reappairage_local() {
        let dir = tempfile::tempdir().unwrap();
        let store = PairingStore::new(dir.path().join("ear"));
        let control = FakeControl::new(0);
        let old = pair_local(control.as_ref(), LOCAL_NAME).await.unwrap();
        store.save(&old).unwrap();
        let spawned = Arc::new(Spawned::default());
        let (link, mut events) = Supervised::spawn(
            Some(old),
            local(control.clone()),
            store.clone(),
            spawner(spawned.clone()),
        );
        wait_for("premier lien", || spawned.links.lock().unwrap().len() == 1).await;
        let first = spawned.links.lock().unwrap()[0].clone();
        first
            .state
            .send_replace(LinkState::Rejected(RejectReason::BadToken));
        wait_for("second lien", || spawned.links.lock().unwrap().len() == 2).await;
        assert_eq!(spawned.pairings.lock().unwrap()[1].token, "t2");
        assert_eq!(store.load().unwrap().unwrap().token, "t2");
        // Pas d'« appairage refusé » montré pendant le réappairage local.
        assert_ne!(
            *link.state().borrow(),
            LinkState::Rejected(RejectReason::BadToken)
        );
        // Les événements du nouveau lien arrivent par le lien supervisé.
        let tx = spawned.events.lock().unwrap()[1].clone();
        tx.send(LinkEvent::Disconnected).await.unwrap();
        let event = tokio::time::timeout(Duration::from_secs(3), events.recv())
            .await
            .unwrap();
        assert_eq!(event, Some(LinkEvent::Disconnected));
    }

    #[tokio::test]
    async fn oreille_distante_refusee_reste_refusee() {
        let dir = tempfile::tempdir().unwrap();
        let store = PairingStore::new(dir.path().join("ear"));
        let control = FakeControl::new(0);
        let mut pairing = pair_local(control.as_ref(), LOCAL_NAME).await.unwrap();
        pairing.addresses = vec!["192.168.1.20:7341".parse().unwrap()];
        assert!(!is_local(&pairing));
        let spawned = Arc::new(Spawned::default());
        let (link, _events) =
            Supervised::spawn(Some(pairing), None, store, spawner(spawned.clone()));
        wait_for("lien", || spawned.links.lock().unwrap().len() == 1).await;
        spawned.links.lock().unwrap()[0]
            .state
            .send_replace(LinkState::Rejected(RejectReason::UnknownEar));
        let mut state = link.state();
        tokio::time::timeout(
            Duration::from_secs(3),
            state.wait_for(|s| matches!(s, LinkState::Rejected(_))),
        )
        .await
        .unwrap()
        .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(spawned.links.lock().unwrap().len(), 1, "pas de réappairage");
        assert_eq!(control.calls.load(Ordering::SeqCst), 1);
    }
}
