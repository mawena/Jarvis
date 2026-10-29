//! Annonce mDNS du cerveau (`_jarvis._tcp.local.`), TXT `id`, `v=1`, `pairing=0|1`.
//!
//! Nom d'hôte annoncé : `jarvis-<machine>.local.`, et non `<machine>.local.`, que
//! publie déjà avahi-daemon : deux répondeurs pour le même nom risqueraient un
//! conflit (avahi renommerait la machine).

use jarvis_protocol::ear::{MDNS_SERVICE, PROTOCOL_VERSION};
use mdns_sd::{ServiceDaemon, ServiceInfo};
use std::time::Duration;

pub struct Advertiser {
    daemon: ServiceDaemon,
    name: String,
    host: String,
    brain_id: String,
    port: u16,
    fullname: String,
    stopped: bool,
}

impl Advertiser {
    /// Lance le démon et annonce le cerveau (`pairing=0`). `name` vide : nom de la
    /// machine.
    pub fn start(name: &str, brain_id: &str, port: u16) -> anyhow::Result<Self> {
        let daemon = ServiceDaemon::new()
            .map_err(|e| anyhow::anyhow!("démon mDNS impossible à lancer : {e}"))?;
        let name = match name.trim() {
            "" => machine_name(),
            name => name.to_string(),
        };
        let mut advertiser = Self {
            daemon,
            host: format!("jarvis-{}.local.", machine_name()),
            name,
            brain_id: brain_id.to_string(),
            port,
            fullname: String::new(),
            stopped: false,
        };
        let info = advertiser.service_info(false)?;
        advertiser.fullname = info.get_fullname().to_string();
        advertiser.register(info)?;
        Ok(advertiser)
    }

    /// Nouvelle annonce avec `pairing=1` (fenêtre ouverte) ou `pairing=0`.
    pub fn set_pairing(&self, open: bool) -> anyhow::Result<()> {
        self.register(self.service_info(open)?)
    }

    /// Arrêt propre : retrait de l'annonce, puis du démon (bloque 2 s au plus).
    pub fn shutdown(mut self) {
        self.stopped = true;
        const WAIT: Duration = Duration::from_secs(1);
        if let Ok(status) = self.daemon.unregister(&self.fullname) {
            let _ = status.recv_timeout(WAIT);
        }
        if let Ok(status) = self.daemon.shutdown() {
            let _ = status.recv_timeout(WAIT);
        }
    }

    fn service_info(&self, pairing: bool) -> anyhow::Result<ServiceInfo> {
        let txt = txt_properties(&self.brain_id, pairing);
        let info = ServiceInfo::new(
            MDNS_SERVICE,
            &self.name,
            &self.host,
            (),
            self.port,
            &txt[..],
        )
        .map_err(|e| anyhow::anyhow!("annonce mDNS invalide : {e}"))?;
        Ok(info.enable_addr_auto())
    }

    fn register(&self, info: ServiceInfo) -> anyhow::Result<()> {
        self.daemon
            .register(info)
            .map_err(|e| anyhow::anyhow!("annonce mDNS impossible : {e}"))
    }
}

impl Drop for Advertiser {
    /// Filet si `shutdown` n'a pas été appelé : retrait sans attendre.
    fn drop(&mut self) {
        if !self.stopped {
            let _ = self.daemon.unregister(&self.fullname);
            let _ = self.daemon.shutdown();
        }
    }
}

/// Propriétés TXT de l'annonce.
pub fn txt_properties(brain_id: &str, pairing: bool) -> Vec<(String, String)> {
    vec![
        ("id".to_string(), brain_id.to_string()),
        ("v".to_string(), PROTOCOL_VERSION.to_string()),
        (
            "pairing".to_string(),
            if pairing { "1" } else { "0" }.to_string(),
        ),
    ]
}

/// Nom court de la machine (« charles-gamligo »), « jarvis » à défaut.
pub fn machine_name() -> String {
    host_label(&std::fs::read_to_string("/proc/sys/kernel/hostname").unwrap_or_default())
}

/// Premier label du nom d'hôte, réduit à `[a-z0-9-]`.
fn host_label(hostname: &str) -> String {
    let first = hostname.trim().split('.').next().unwrap_or_default();
    let label: String = first
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    match label.trim_matches('-') {
        "" => "jarvis".to_string(),
        label => label.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jarvis_ear_core::net::Discovery;
    use jarvis_ear_core::net::discovery::MdnsDiscovery;

    fn paire(k: &str, v: &str) -> (String, String) {
        (k.to_string(), v.to_string())
    }

    #[test]
    fn proprietes_txt_de_l_annonce() {
        assert_eq!(
            txt_properties("abc123", false),
            vec![
                paire("id", "abc123"),
                paire("v", "1"),
                paire("pairing", "0")
            ]
        );
        assert_eq!(txt_properties("abc123", true)[2], paire("pairing", "1"));
    }

    #[test]
    fn nom_d_hote_reduit_au_premier_label() {
        assert_eq!(
            host_label("charles-gamligo.cofinacorp.com\n"),
            "charles-gamligo"
        );
        assert_eq!(host_label("Mon PC_2"), "mon-pc-2");
        assert_eq!(host_label("  "), "jarvis");
    }

    /// Vrai réseau (multicast sur la machine) : `cargo test -- --ignored`.
    #[tokio::test]
    #[ignore]
    async fn annonce_puis_decouverte_sur_la_machine() {
        let id = format!("test-{}", std::process::id());
        let advertiser = Advertiser::start("Jarvis test", &id, 47341).unwrap();
        let discovery = MdnsDiscovery::new().unwrap();

        let addrs = discovery.find(&id, Duration::from_secs(5)).await;
        assert!(!addrs.is_empty(), "annonce introuvable");
        assert!(addrs.iter().all(|a| a.port() == 47341));
        // Fenêtre fermée : absent des cerveaux en appairage.
        let found = discovery.find_pairing(Duration::from_secs(2)).await;
        assert!(found.iter().all(|b| b.brain_id != id));

        advertiser.set_pairing(true).unwrap();
        let found = discovery.find_pairing(Duration::from_secs(5)).await;
        let brain = found
            .iter()
            .find(|b| b.brain_id == id)
            .expect("cerveau en appairage introuvable");
        assert_eq!(brain.name, "Jarvis test");
        assert!(!brain.addrs.is_empty());
        advertiser.shutdown();
    }
}
