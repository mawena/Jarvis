//! Annonce mDNS du cerveau (`_jarvis._tcp.local.`), TXT `id`, `v=1`, `pairing=0|1`.
// Squelette (tâche 1) : à retirer par la tâche 5.
#![allow(dead_code, unused_variables)]

pub struct Advertiser {
    // Champs (démon mdns-sd, nom complet…) : tâche 5.
}

impl Advertiser {
    pub fn start(name: &str, brain_id: &str, port: u16) -> anyhow::Result<Self> {
        todo!("tâche 5")
    }

    /// Nouvelle annonce avec `pairing=1` (fenêtre ouverte) ou `pairing=0`.
    pub fn set_pairing(&self, open: bool) -> anyhow::Result<()> {
        todo!("tâche 5")
    }

    /// Arrêt propre : retrait de l'annonce, puis du démon.
    pub fn shutdown(self) {
        todo!("tâche 5")
    }
}

/// Propriétés TXT de l'annonce.
pub fn txt_properties(brain_id: &str, pairing: bool) -> Vec<(String, String)> {
    todo!("tâche 5")
}
