//! Effets de bord (commandes, réseau, socket) derrière un trait pour les tests.

use jarvis_protocol::{ControlRequest, ControlResponse, Mode};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

pub const OLLAMA_ADDR: &str = "127.0.0.1:11434";

pub struct Output {
    pub code: i32,
    pub stdout: String,
}

pub trait System {
    /// Lance une commande en capturant sa sortie standard.
    fn run(&self, prog: &str, args: &[&str]) -> io::Result<Output>;
    /// Lance une commande en héritant des flux ; renvoie le code de sortie.
    fn run_inherit(&self, prog: &str, args: &[&str], cwd: Option<&Path>) -> io::Result<i32>;
    /// Corps de la réponse d'un `GET` HTTP (None si injoignable ou statut != 200).
    fn http_get(&self, addr: &str, path: &str) -> Option<String>;
    /// Comme `http_get`, avec un délai plus long (une vraie recherche).
    fn http_get_slow(&self, addr: &str, path: &str) -> Option<String> {
        self.http_get(addr, path)
    }
    /// Attend sans rien faire.
    fn sleep(&self, d: Duration);
    /// Réponse `Status` du cerveau via son socket : (mode, version, modèle local).
    fn brain_status(&self) -> Option<(Mode, String, String)>;
    fn env_var(&self, name: &str) -> Option<String>;
}

pub struct RealSystem;

impl System for RealSystem {
    fn run(&self, prog: &str, args: &[&str]) -> io::Result<Output> {
        let out = Command::new(prog)
            .args(args)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()?;
        Ok(Output {
            code: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        })
    }

    fn run_inherit(&self, prog: &str, args: &[&str], cwd: Option<&Path>) -> io::Result<i32> {
        let mut cmd = Command::new(prog);
        cmd.args(args);
        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }
        Ok(cmd.status()?.code().unwrap_or(-1))
    }

    fn http_get(&self, addr: &str, path: &str) -> Option<String> {
        http_get_within(addr, path, Duration::from_secs(2))
    }

    fn http_get_slow(&self, addr: &str, path: &str) -> Option<String> {
        http_get_within(addr, path, Duration::from_secs(8))
    }

    fn sleep(&self, d: Duration) {
        std::thread::sleep(d);
    }

    fn brain_status(&self) -> Option<(Mode, String, String)> {
        let mut s = UnixStream::connect(jarvis_protocol::socket_path()).ok()?;
        s.set_read_timeout(Some(Duration::from_secs(3))).ok()?;
        let line = serde_json::to_string(&ControlRequest::Status).ok()?;
        writeln!(s, "{line}").ok()?;
        let mut reply = String::new();
        BufReader::new(s).read_line(&mut reply).ok()?;
        match serde_json::from_str(&reply).ok()? {
            ControlResponse::Status {
                mode,
                version,
                local_model,
            } => Some((mode, version, local_model)),
            _ => None,
        }
    }

    fn env_var(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }
}

fn http_get_within(addr: &str, path: &str, timeout: Duration) -> Option<String> {
    let sock = addr.to_socket_addrs().ok()?.next()?;
    let mut s = TcpStream::connect_timeout(&sock, timeout).ok()?;
    s.set_read_timeout(Some(timeout)).ok()?;
    s.set_write_timeout(Some(timeout)).ok()?;
    write!(
        s,
        "GET {path} HTTP/1.0\r\nHost: {addr}\r\nConnection: close\r\n\r\n"
    )
    .ok()?;
    let mut raw = String::new();
    s.read_to_string(&mut raw).ok()?;
    let (head, body) = raw.split_once("\r\n\r\n")?;
    head.lines()
        .next()?
        .contains(" 200")
        .then(|| body.to_string())
}

/// Faux système des tests : commandes notées, réponses préparées.
#[cfg(test)]
pub mod fake {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;

    #[derive(Default)]
    pub struct Fake {
        pub cmds: RefCell<Vec<String>>,
        pub replies: HashMap<String, (i32, String)>,
        pub http: HashMap<String, String>,
        /// Requêtes HTTP sans réponse avant que `http` réponde.
        pub http_misses: Cell<u32>,
        pub sleeps: Cell<u32>,
        pub brain: Option<(Mode, String, String)>,
        pub path: Option<String>,
    }

    impl System for Fake {
        fn run(&self, prog: &str, args: &[&str]) -> io::Result<Output> {
            let cmd = format!("{prog} {}", args.join(" "));
            self.cmds.borrow_mut().push(cmd.clone());
            let (code, stdout) = self
                .replies
                .get(&cmd)
                .cloned()
                .unwrap_or((0, String::new()));
            Ok(Output { code, stdout })
        }
        fn run_inherit(&self, prog: &str, args: &[&str], _: Option<&Path>) -> io::Result<i32> {
            self.cmds
                .borrow_mut()
                .push(format!("{prog} {}", args.join(" ")));
            Ok(0)
        }
        fn http_get(&self, _: &str, path: &str) -> Option<String> {
            if self.http_misses.get() > 0 {
                self.http_misses.set(self.http_misses.get() - 1);
                return None;
            }
            self.http.get(path).cloned()
        }
        fn sleep(&self, _: Duration) {
            self.sleeps.set(self.sleeps.get() + 1);
        }
        fn brain_status(&self) -> Option<(Mode, String, String)> {
            self.brain.clone()
        }
        fn env_var(&self, _: &str) -> Option<String> {
            self.path.clone()
        }
    }
}
