//! Découverte du cerveau par mDNS (`_jarvis._tcp.local.`).
// Squelette (tâche 1) : à retirer par la tâche 5.
#![allow(dead_code, unused_variables)]

use super::Discovery;
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
    // Démon mdns-sd : tâche 5.
}

impl MdnsDiscovery {
    pub fn new() -> anyhow::Result<Self> {
        todo!("tâche 5")
    }

    /// Cerveaux dont la fenêtre d'appairage est ouverte.
    pub async fn find_pairing(&self, wait: Duration) -> Vec<FoundBrain> {
        todo!("tâche 5")
    }
}

#[async_trait::async_trait]
impl Discovery for MdnsDiscovery {
    async fn find(&self, brain_id: &str, wait: Duration) -> Vec<SocketAddr> {
        todo!("tâche 5")
    }
}

/// Adresses d'une annonce si elle vient du cerveau `expected_id` ; sinon (autre
/// cerveau, sans adresse), `None`.
pub fn match_service(
    expected_id: &str,
    properties: &HashMap<String, String>,
    addresses: &[IpAddr],
    port: u16,
) -> Option<Vec<SocketAddr>> {
    todo!("tâche 5")
}
