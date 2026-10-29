//! Le service systemd utilisateur `jarvis-ear`, vu de l'oreille elle-même : ne pas lancer
//! une deuxième oreille à côté du service, et mettre le service en pause pendant
//! l'enrôlement (sinon il entendrait les prises « Jarvis » et y réagirait).

use std::ffi::OsString;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Nom de l'unité, tel que `systemctl --user` l'accepte.
pub const UNIT: &str = "jarvis-ear";

/// `systemctl --user …` (remplaçable dans les tests).
pub trait Systemctl: Send + Sync {
    /// Lance `systemctl --user <args>` ; vrai si la commande réussit (code 0).
    fn run(&self, args: &[&str]) -> bool;
}

/// Le vrai `systemctl`, sorties ignorées. Absent ou sans gestionnaire de session : faux.
pub struct RealSystemctl;

impl Systemctl for RealSystemctl {
    fn run(&self, args: &[&str]) -> bool {
        Command::new("systemctl")
            .arg("--user")
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }
}

/// Le service de l'oreille tourne.
pub fn is_active(systemctl: &dyn Systemctl) -> bool {
    systemctl.run(&["is-active", "--quiet", UNIT])
}

/// Processus lancé par systemd : il pose `INVOCATION_ID` et `JOURNAL_STREAM` pour ses
/// services.
pub fn started_by_systemd(var: impl Fn(&str) -> Option<OsString>) -> bool {
    ["INVOCATION_ID", "JOURNAL_STREAM"]
        .iter()
        .any(|name| var(name).is_some_and(|v| !v.is_empty()))
}

/// Faut-il refuser de démarrer ? Oui si le service tourne déjà et que ce processus n'est
/// pas lui (lancé à la main), sauf `--force` : deux oreilles entendraient tout en double.
pub fn refuse_second_ear(service_active: bool, started_by_systemd: bool, force: bool) -> bool {
    service_active && !started_by_systemd && !force
}

/// Message affiché quand une deuxième oreille est refusée.
pub const ALREADY_RUNNING: &str = "L'oreille tourne déjà en service (jarvis-ear.service) : une deuxième oreille entendrait tout en double.
  Journal :       jarvisctl logs --oreille -f
  Pour la lancer à la main, arrête d'abord le service : jarvisctl stop oreille
  Pour passer outre : jarvis-ear --force";

/// Pause du service pendant l'enrôlement. Le service est relancé une seule fois, par
/// [`EarPause::resume`] ou à la destruction (fin normale, erreur ou panique).
pub struct EarPause {
    systemctl: Arc<dyn Systemctl>,
    /// Relancer à la fin : le service tournait, ou il est activé (premier enrôlement :
    /// il était sauté faute de mot d'éveil, la relance le fait démarrer).
    restart: bool,
    resumed: AtomicBool,
}

/// Issue de la relance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resume {
    /// Rien à relancer (service ni actif ni activé), ou déjà relancé.
    Nothing,
    Started,
    Failed,
}

impl EarPause {
    /// Arrête le service s'il tourne (en le disant), et note s'il faudra le relancer.
    /// Erreur si le service tourne et ne s'arrête pas : l'enrôlement serait faussé.
    pub fn begin(systemctl: Arc<dyn Systemctl>, say: &mut dyn FnMut(&str)) -> anyhow::Result<Self> {
        let active = is_active(&*systemctl);
        if active {
            if !systemctl.run(&["stop", UNIT]) {
                anyhow::bail!(
                    "impossible d'arrêter le service jarvis-ear, qui entendrait les prises : \
                     arrête-le avec `jarvisctl stop oreille`, puis relance l'enrôlement"
                );
            }
            say(
                "Service jarvis-ear arrêté pendant l'enrôlement (il n'entendra pas les prises) ; \
                 il redémarre à la fin.",
            );
        }
        let restart = active || systemctl.run(&["is-enabled", "--quiet", UNIT]);
        Ok(Self {
            systemctl,
            restart,
            resumed: AtomicBool::new(false),
        })
    }

    /// Relance le service (une seule fois, même appelée plusieurs fois).
    pub fn resume(&self) -> Resume {
        if !self.restart || self.resumed.swap(true, Ordering::SeqCst) {
            return Resume::Nothing;
        }
        if self.systemctl.run(&["start", UNIT]) {
            Resume::Started
        } else {
            Resume::Failed
        }
    }

    /// Relance et l'annonce sur la sortie standard.
    pub fn resume_and_report(&self) {
        match self.resume() {
            Resume::Nothing => {}
            Resume::Started => println!("Service jarvis-ear relancé."),
            Resume::Failed => eprintln!(
                "Le service jarvis-ear n'a pas pu être relancé : lance `jarvisctl start oreille`."
            ),
        }
    }
}

impl Drop for EarPause {
    fn drop(&mut self) {
        self.resume_and_report();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// `systemctl` simulé : réponses par commande, commandes notées.
    #[derive(Default)]
    pub struct FakeSystemctl {
        pub calls: Mutex<Vec<String>>,
        pub ok: Vec<&'static str>,
    }

    impl Systemctl for FakeSystemctl {
        fn run(&self, args: &[&str]) -> bool {
            let cmd = args.join(" ");
            self.calls.lock().unwrap().push(cmd.clone());
            self.ok.contains(&cmd.as_str())
        }
    }

    fn fake(ok: &[&'static str]) -> Arc<FakeSystemctl> {
        Arc::new(FakeSystemctl {
            calls: Mutex::default(),
            ok: ok.to_vec(),
        })
    }

    fn calls(f: &FakeSystemctl) -> Vec<String> {
        f.calls.lock().unwrap().clone()
    }

    const ACTIVE: &str = "is-active --quiet jarvis-ear";
    const ENABLED: &str = "is-enabled --quiet jarvis-ear";
    const STOP: &str = "stop jarvis-ear";
    const START: &str = "start jarvis-ear";

    #[test]
    fn service_actif_arrete_puis_relance() {
        let sc = fake(&[ACTIVE, STOP, START]);
        let mut said = Vec::new();
        let pause = EarPause::begin(sc.clone(), &mut |l| said.push(l.to_string())).unwrap();
        assert_eq!(calls(&sc), vec![ACTIVE, STOP]);
        assert_eq!(said.len(), 1);
        assert!(said[0].contains("arrêté pendant l'enrôlement"), "{said:?}");
        drop(pause);
        assert_eq!(calls(&sc), vec![ACTIVE, STOP, START]);
    }

    #[test]
    fn premier_enrolement_service_active_mais_saute_demarre_a_la_fin() {
        let sc = fake(&[ENABLED, START]);
        let mut said = Vec::new();
        let pause = EarPause::begin(sc.clone(), &mut |l| said.push(l.to_string())).unwrap();
        assert!(said.is_empty(), "rien n'est arrêté");
        assert_eq!(pause.resume(), Resume::Started);
        drop(pause);
        assert_eq!(calls(&sc), vec![ACTIVE, ENABLED, START]);
    }

    #[test]
    fn service_ni_actif_ni_active_jamais_demarre() {
        let sc = fake(&[]);
        let pause = EarPause::begin(sc.clone(), &mut |_| {}).unwrap();
        assert_eq!(pause.resume(), Resume::Nothing);
        drop(pause);
        assert_eq!(calls(&sc), vec![ACTIVE, ENABLED]);
    }

    #[test]
    fn relance_une_seule_fois_signal_puis_destruction() {
        let sc = fake(&[ACTIVE, STOP, START]);
        let pause = EarPause::begin(sc.clone(), &mut |_| {}).unwrap();
        // Ctrl-C : la tâche du signal relance, puis la destruction ne refait rien.
        assert_eq!(pause.resume(), Resume::Started);
        assert_eq!(pause.resume(), Resume::Nothing);
        drop(pause);
        let starts = calls(&sc).iter().filter(|c| *c == START).count();
        assert_eq!(starts, 1);
    }

    #[test]
    fn relance_aussi_quand_l_enrolement_echoue() {
        fn enroll(sc: Arc<FakeSystemctl>) -> anyhow::Result<()> {
            let _pause = EarPause::begin(sc, &mut |_| {})?;
            anyhow::bail!("le micro ne délivre pas d'audio")
        }
        let sc = fake(&[ACTIVE, STOP, START]);
        assert!(enroll(sc.clone()).is_err());
        assert_eq!(calls(&sc).last().map(String::as_str), Some(START));

        // Panique comprise.
        let sc = fake(&[ACTIVE, STOP, START]);
        let inner = sc.clone();
        let res = std::panic::catch_unwind(move || {
            let _pause = EarPause::begin(inner, &mut |_| {}).unwrap();
            panic!("boum");
        });
        assert!(res.is_err());
        assert_eq!(calls(&sc).last().map(String::as_str), Some(START));
    }

    #[test]
    fn arret_impossible_refuse_l_enrolement() {
        let sc = fake(&[ACTIVE]);
        let err = EarPause::begin(sc.clone(), &mut |_| {})
            .err()
            .unwrap()
            .to_string();
        assert!(err.contains("jarvisctl stop oreille"), "{err}");
        assert_eq!(calls(&sc), vec![ACTIVE, STOP]);
    }

    #[test]
    fn relance_en_echec_signalee() {
        let sc = fake(&[ACTIVE, STOP]);
        let pause = EarPause::begin(sc, &mut |_| {}).unwrap();
        assert_eq!(pause.resume(), Resume::Failed);
    }

    #[test]
    fn detection_de_systemd() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(k, _)| *k == name)
                    .map(|(_, v)| OsString::from(v))
            }
        };
        assert!(started_by_systemd(env(&[("INVOCATION_ID", "4f2a")])));
        assert!(started_by_systemd(env(&[("JOURNAL_STREAM", "8:12345")])));
        assert!(!started_by_systemd(env(&[])));
        assert!(!started_by_systemd(env(&[("INVOCATION_ID", "")])));
        assert!(!started_by_systemd(env(&[("HOME", "/home/charles")])));
    }

    #[test]
    fn deuxieme_oreille_refusee_seulement_a_la_main_sans_force() {
        // (service actif, lancé par systemd, --force) → refus
        for (active, systemd, force, refuse) in [
            (true, false, false, true),
            (true, true, false, false),
            (true, false, true, false),
            (true, true, true, false),
            (false, false, false, false),
            (false, true, false, false),
            (false, false, true, false),
        ] {
            assert_eq!(
                refuse_second_ear(active, systemd, force),
                refuse,
                "actif={active} systemd={systemd} force={force}"
            );
        }
    }

    #[test]
    fn service_actif_lu_par_systemctl() {
        assert!(is_active(&*fake(&[ACTIVE])));
        assert!(!is_active(&*fake(&[])));
    }
}
