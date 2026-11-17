//! Anti-veille sur secteur (option `inhibit_sleep_on_ac`, spec §5.7) : tant que le
//! portable est branché, un processus `systemd-inhibit` bloque la mise en veille ; il
//! est arrêté dès que le portable est débranché ou que le cerveau s'arrête.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tracing::{info, warn};

/// Dossier des alimentations du noyau.
pub const POWER_SUPPLY_DIR: &str = "/sys/class/power_supply";

/// Fréquence de lecture de l'alimentation.
pub const CHECK_PERIOD: Duration = Duration::from_secs(30);

fn read(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

/// Le PC est-il sur secteur ? Oui si une alimentation externe (`Mains`, `USB`) est
/// branchée, si une batterie charge, ou s'il n'y a aucune batterie (PC fixe, ou
/// dossier illisible : on ne bloque alors la veille que si l'option le demande).
pub fn on_ac(dir: &Path) -> bool {
    let Ok(entries) = fs::read_dir(dir) else {
        return true;
    };
    let mut battery = false;
    for entry in entries.flatten() {
        let supply = entry.path();
        match read(&supply.join("type")).as_deref() {
            Some("Mains" | "USB") => {
                if read(&supply.join("online")).as_deref() == Some("1") {
                    return true;
                }
            }
            Some("Battery") => {
                battery = true;
                if read(&supply.join("status")).as_deref() == Some("Charging") {
                    return true;
                }
            }
            _ => {}
        }
    }
    !battery
}

/// Tient ou relâche le blocage de la veille.
pub trait Inhibitor: Send {
    fn hold(&mut self) -> io::Result<()>;
    fn release(&mut self);
    /// Le blocage tient-il encore (le processus peut être mort) ?
    fn held(&mut self) -> bool;
}

/// Blocage par `systemd-inhibit … sleep infinity` (logind).
#[derive(Default)]
pub struct SystemdInhibitor {
    child: Option<tokio::process::Child>,
}

impl Inhibitor for SystemdInhibitor {
    fn hold(&mut self) -> io::Result<()> {
        let child = tokio::process::Command::new("systemd-inhibit")
            .args([
                "--what=sleep:idle",
                "--who=Jarvis",
                "--why=Portable branché : Jarvis reste à l'écoute",
                "--mode=block",
                "sleep",
                "infinity",
            ])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        self.child = Some(child);
        Ok(())
    }

    fn release(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.start_kill();
        }
    }

    fn held(&mut self) -> bool {
        match self.child.as_mut().map(|c| c.try_wait()) {
            Some(Ok(None)) => true,
            Some(_) => {
                self.child = None;
                false
            }
            None => false,
        }
    }
}

/// Suit l'alimentation et tient le blocage quand il le faut.
pub struct SleepGuard<I: Inhibitor> {
    inhibitor: I,
    dir: PathBuf,
    /// Blocage voulu au dernier passage (pour ne journaliser que les changements).
    wanted: bool,
    /// Échec déjà signalé (pas de répétition toutes les 30 s).
    failed: bool,
}

impl<I: Inhibitor> SleepGuard<I> {
    pub fn new(inhibitor: I, dir: impl Into<PathBuf>) -> Self {
        Self {
            inhibitor,
            dir: dir.into(),
            wanted: false,
            failed: false,
        }
    }

    /// Un passage : relit l'alimentation, tient ou relâche. Renvoie l'état voulu.
    pub fn step(&mut self) -> bool {
        let wanted = on_ac(&self.dir);
        if wanted != self.wanted {
            info!(
                secteur = wanted,
                "{}",
                if wanted {
                    "portable branché : mise en veille bloquée"
                } else {
                    "portable débranché : mise en veille permise"
                }
            );
            self.wanted = wanted;
        }
        if !wanted {
            self.inhibitor.release();
            return false;
        }
        if !self.inhibitor.held() {
            match self.inhibitor.hold() {
                Ok(()) => self.failed = false,
                Err(e) if !self.failed => {
                    warn!(erreur = %e, "systemd-inhibit indisponible : veille non bloquée");
                    self.failed = true;
                }
                Err(_) => {}
            }
        }
        true
    }

    /// Tourne jusqu'à l'arrêt du cerveau (la tâche lâchée arrête le processus).
    pub async fn run(mut self, period: Duration) {
        let mut tick = tokio::time::interval(period);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            self.step();
        }
    }
}

impl<I: Inhibitor> Drop for SleepGuard<I> {
    fn drop(&mut self) {
        self.inhibitor.release();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn supply(dir: &Path, name: &str, files: &[(&str, &str)]) {
        let d = dir.join(name);
        fs::create_dir_all(&d).unwrap();
        for (file, value) in files {
            fs::write(d.join(file), format!("{value}\n")).unwrap();
        }
    }

    fn laptop(plugged: bool) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let online = if plugged { "1" } else { "0" };
        let status = if plugged { "Full" } else { "Discharging" };
        supply(dir.path(), "AC", &[("type", "Mains"), ("online", online)]);
        supply(
            dir.path(),
            "BAT0",
            &[("type", "Battery"), ("status", status)],
        );
        dir
    }

    #[test]
    fn portable_branche_ou_non() {
        assert!(on_ac(laptop(true).path()));
        assert!(!on_ac(laptop(false).path()));
    }

    #[test]
    fn chargeur_usb_c() {
        let dir = tempfile::tempdir().unwrap();
        supply(
            dir.path(),
            "BAT0",
            &[("type", "Battery"), ("status", "Full")],
        );
        supply(
            dir.path(),
            "ucsi-source-psy-1",
            &[("type", "USB"), ("online", "1")],
        );
        assert!(on_ac(dir.path()));
    }

    #[test]
    fn batterie_en_charge_sans_adaptateur_lisible() {
        let dir = tempfile::tempdir().unwrap();
        supply(
            dir.path(),
            "BAT0",
            &[("type", "Battery"), ("status", "Charging")],
        );
        assert!(on_ac(dir.path()));
    }

    #[test]
    fn pc_fixe_sans_batterie() {
        let dir = tempfile::tempdir().unwrap();
        assert!(on_ac(dir.path()));
        assert!(on_ac(&dir.path().join("absent")));
    }

    /// Faux blocage : journal des appels, panne au choix.
    #[derive(Clone, Default)]
    struct Fake {
        log: Arc<Mutex<Vec<&'static str>>>,
        held: Arc<Mutex<bool>>,
        broken: bool,
    }

    impl Inhibitor for Fake {
        fn hold(&mut self) -> io::Result<()> {
            self.log.lock().unwrap().push("hold");
            if self.broken {
                return Err(io::Error::other("absent"));
            }
            *self.held.lock().unwrap() = true;
            Ok(())
        }
        fn release(&mut self) {
            if std::mem::take(&mut *self.held.lock().unwrap()) {
                self.log.lock().unwrap().push("release");
            }
        }
        fn held(&mut self) -> bool {
            *self.held.lock().unwrap()
        }
    }

    #[test]
    fn blocage_suit_le_secteur() {
        let dir = laptop(true);
        let fake = Fake::default();
        let mut guard = SleepGuard::new(fake.clone(), dir.path());
        assert!(guard.step());
        assert!(guard.step());
        // Débranché.
        fs::write(dir.path().join("AC/online"), "0\n").unwrap();
        fs::write(dir.path().join("BAT0/status"), "Discharging\n").unwrap();
        assert!(!guard.step());
        // Rebranché.
        fs::write(dir.path().join("AC/online"), "1\n").unwrap();
        assert!(guard.step());
        drop(guard);
        assert_eq!(
            *fake.log.lock().unwrap(),
            ["hold", "release", "hold", "release"]
        );
    }

    #[test]
    fn processus_mort_relance() {
        let dir = laptop(true);
        let fake = Fake::default();
        let mut guard = SleepGuard::new(fake.clone(), dir.path());
        guard.step();
        *fake.held.lock().unwrap() = false;
        guard.step();
        assert_eq!(*fake.log.lock().unwrap(), ["hold", "hold"]);
    }

    #[test]
    fn systemd_inhibit_absent_sans_panique() {
        let dir = laptop(true);
        let fake = Fake {
            broken: true,
            ..Fake::default()
        };
        let mut guard = SleepGuard::new(fake.clone(), dir.path());
        assert!(guard.step());
        assert!(guard.step());
        assert_eq!(*fake.log.lock().unwrap(), ["hold", "hold"]);
    }
}
