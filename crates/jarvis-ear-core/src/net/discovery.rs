//! Découverte du cerveau par mDNS (`_jarvis._tcp.local.`).

use super::Discovery;
use jarvis_protocol::ear::MDNS_SERVICE;
use mdns_sd::{ResolvedService, ServiceDaemon, ServiceEvent};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

/// Cerveau en appairage (`pairing=1`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundBrain {
    pub brain_id: String,
    pub name: String,
    pub addrs: Vec<SocketAddr>,
}

pub struct MdnsDiscovery {
    daemon: ServiceDaemon,
    /// Une recherche à la fois : mdns-sd ne garde qu'un destinataire par type.
    browsing: tokio::sync::Mutex<()>,
}

impl MdnsDiscovery {
    pub fn new() -> anyhow::Result<Self> {
        let daemon = ServiceDaemon::new()
            .map_err(|e| anyhow::anyhow!("démon mDNS impossible à lancer : {e}"))?;
        Ok(Self {
            daemon,
            browsing: tokio::sync::Mutex::new(()),
        })
    }

    /// Cerveaux dont la fenêtre d'appairage est ouverte (attend `wait` en entier).
    pub async fn find_pairing(&self, wait: Duration) -> Vec<FoundBrain> {
        let mut found: Vec<FoundBrain> = Vec::new();
        self.browse(wait, |service| {
            let props = properties(service);
            let id = props.get("id").cloned().unwrap_or_default();
            found.retain(|b| b.brain_id != id);
            if props.get("pairing").map(String::as_str) == Some("1")
                && let Some(addrs) =
                    match_service(&id, &props, &addresses(service), service.get_port())
            {
                found.push(FoundBrain {
                    brain_id: id,
                    name: instance_name(service.get_fullname()),
                    addrs,
                });
            }
            false
        })
        .await;
        found
    }

    /// Écoute les annonces résolues pendant `wait` au plus ; `on` rend `true` pour
    /// arrêter plus tôt.
    async fn browse(&self, wait: Duration, mut on: impl FnMut(&ResolvedService) -> bool + Send) {
        let _seul = self.browsing.lock().await;
        let events = match self.daemon.browse(MDNS_SERVICE) {
            Ok(events) => events,
            Err(e) => {
                tracing::warn!("recherche mDNS impossible : {e}");
                return;
            }
        };
        let deadline = tokio::time::Instant::now() + wait;
        while let Ok(Ok(event)) = tokio::time::timeout_at(deadline, events.recv_async()).await {
            if let ServiceEvent::ServiceResolved(service) = event
                && on(&service)
            {
                break;
            }
        }
        let _ = self.daemon.stop_browse(MDNS_SERVICE);
    }
}

impl Drop for MdnsDiscovery {
    fn drop(&mut self) {
        let _ = self.daemon.shutdown();
    }
}

#[async_trait::async_trait]
impl Discovery for MdnsDiscovery {
    /// Adresses du cerveau `brain_id` : rend dès la première annonce qui convient.
    async fn find(&self, brain_id: &str, wait: Duration) -> Vec<SocketAddr> {
        let mut found = Vec::new();
        self.browse(wait, |service| {
            match match_service(
                brain_id,
                &properties(service),
                &addresses(service),
                service.get_port(),
            ) {
                Some(addrs) => {
                    found = addrs;
                    true
                }
                None => false,
            }
        })
        .await;
        found
    }
}

fn properties(service: &ResolvedService) -> HashMap<String, String> {
    service
        .get_properties()
        .iter()
        .map(|p| (p.key().to_string(), p.val_str().to_string()))
        .collect()
}

/// IPv4 seulement : une adresse IPv6 de lien local perd sa zone dans `SocketAddr`.
fn addresses(service: &ResolvedService) -> Vec<IpAddr> {
    service
        .get_addresses_v4()
        .into_iter()
        .map(IpAddr::V4)
        .collect()
}

/// Adresses d'une annonce si elle vient du cerveau `expected_id` ; sinon (autre
/// cerveau, sans adresse), `None`.
pub fn match_service(
    expected_id: &str,
    properties: &HashMap<String, String>,
    addresses: &[IpAddr],
    port: u16,
) -> Option<Vec<SocketAddr>> {
    if expected_id.is_empty()
        || port == 0
        || properties.get("id").map(String::as_str) != Some(expected_id)
    {
        return None;
    }
    let mut addrs: Vec<SocketAddr> = addresses
        .iter()
        .filter(|ip| usable(ip))
        .map(|ip| SocketAddr::new(*ip, port))
        .collect();
    addrs.sort();
    addrs.dedup();
    (!addrs.is_empty()).then_some(addrs)
}

fn usable(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => !v4.is_unspecified() && !v4.is_multicast() && !v4.is_broadcast(),
        IpAddr::V6(v6) => !v6.is_unspecified() && !v6.is_multicast() && !v6.is_unicast_link_local(),
    }
}

/// « Charles PC._jarvis._tcp.local. » → « Charles PC » (`\.` et `\\` déséchappés).
/// Venu de n'importe quelle machine du réseau : sans caractère de contrôle ni marque
/// de direction (voir [`jarvis_protocol::printable`]).
fn instance_name(fullname: &str) -> String {
    let suffix = format!(".{MDNS_SERVICE}");
    let mut name = String::new();
    let mut chars = fullname.strip_suffix(&suffix).unwrap_or(fullname).chars();
    while let Some(c) = chars.next() {
        name.push(if c == '\\' {
            chars.next().unwrap_or(c)
        } else {
            c
        });
    }
    jarvis_protocol::printable(&name)
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    fn props(id: &str, pairing: &str) -> HashMap<String, String> {
        HashMap::from([
            ("id".to_string(), id.to_string()),
            ("v".to_string(), "1".to_string()),
            ("pairing".to_string(), pairing.to_string()),
        ])
    }

    const LAN: IpAddr = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20));

    #[test]
    fn annonce_du_bon_cerveau_retenue() {
        let other = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 5));
        let addrs = match_service("abc", &props("abc", "0"), &[other, LAN, other], 7341).unwrap();
        assert_eq!(
            addrs,
            vec![SocketAddr::new(other, 7341), SocketAddr::new(LAN, 7341)]
        );
    }

    #[test]
    fn annonce_d_un_autre_cerveau_ignoree() {
        assert_eq!(match_service("abc", &props("xyz", "0"), &[LAN], 7341), None);
        assert_eq!(match_service("abc", &HashMap::new(), &[LAN], 7341), None);
        assert_eq!(match_service("", &props("", "0"), &[LAN], 7341), None);
    }

    #[test]
    fn annonce_sans_adresse_ignoree() {
        assert_eq!(match_service("abc", &props("abc", "0"), &[], 7341), None);
        // Inutilisables : non spécifiée, lien local IPv6 (sans zone dans `SocketAddr`).
        let unusable = [
            IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            IpAddr::V6("fe80::1".parse::<Ipv6Addr>().unwrap()),
        ];
        assert_eq!(
            match_service("abc", &props("abc", "0"), &unusable, 7341),
            None
        );
        assert_eq!(match_service("abc", &props("abc", "0"), &[LAN], 0), None);
    }

    #[test]
    fn nom_d_instance_tire_du_nom_complet() {
        assert_eq!(
            instance_name("Charles PC._jarvis._tcp.local."),
            "Charles PC"
        );
        assert_eq!(instance_name("bizarre"), "bizarre");
        assert_eq!(instance_name(r"PC 2\.0._jarvis._tcp.local."), "PC 2.0");
    }

    #[test]
    fn nom_d_instance_sans_caractere_de_controle() {
        assert_eq!(
            instance_name("PC\u{1b}[31m\r\nCerveau\u{202E}lam._jarvis._tcp.local."),
            "PC[31mCerveaulam"
        );
        // Déséchappé puis nettoyé : `\` ne fait pas revenir un contrôle.
        assert_eq!(instance_name("a\\\u{7}b._jarvis._tcp.local."), "ab");
    }
}
