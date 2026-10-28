use std::process::{Command, Stdio};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    pub success: bool,
    pub stdout: String,
}

pub trait CommandRunner: Send + Sync {
    fn run(&self, program: &str, args: &[String]) -> std::io::Result<CommandOutput>;
    fn spawn_detached(&self, program: &str, args: &[String]) -> std::io::Result<()>;
}

/// Real runner backed by `std::process::Command`.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemRunner;

impl CommandRunner for SystemRunner {
    fn run(&self, program: &str, args: &[String]) -> std::io::Result<CommandOutput> {
        let out = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .output()?;
        Ok(CommandOutput {
            success: out.status.success(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        })
    }

    fn spawn_detached(&self, program: &str, args: &[String]) -> std::io::Result<()> {
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        // Reap the child in a background thread so it never becomes a zombie.
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    }
}

#[cfg(any(test, feature = "test-util"))]
mod fake {
    use super::{CommandOutput, CommandRunner};
    use std::collections::HashMap;
    use std::sync::Mutex;

    type Call = (String, Vec<String>);

    /// Test double recording calls and returning scripted outputs.
    #[derive(Debug, Default)]
    pub struct FakeRunner {
        calls: Mutex<Vec<Call>>,
        responses: Mutex<HashMap<String, CommandOutput>>,
    }

    impl FakeRunner {
        pub fn new() -> Self {
            Self::default()
        }

        /// Program the output returned for `program`.
        pub fn respond(&self, program: &str, output: CommandOutput) {
            self.responses
                .lock()
                .unwrap()
                .insert(program.to_string(), output);
        }

        /// All calls so far (run and spawn_detached), in order.
        pub fn calls(&self) -> Vec<Call> {
            self.calls.lock().unwrap().clone()
        }

        fn record(&self, program: &str, args: &[String]) {
            self.calls
                .lock()
                .unwrap()
                .push((program.to_string(), args.to_vec()));
        }
    }

    impl CommandRunner for FakeRunner {
        fn run(&self, program: &str, args: &[String]) -> std::io::Result<CommandOutput> {
            self.record(program, args);
            Ok(self
                .responses
                .lock()
                .unwrap()
                .get(program)
                .cloned()
                .unwrap_or(CommandOutput {
                    success: true,
                    stdout: String::new(),
                }))
        }

        fn spawn_detached(&self, program: &str, args: &[String]) -> std::io::Result<()> {
            self.record(program, args);
            Ok(())
        }
    }
}

#[cfg(any(test, feature = "test-util"))]
pub use fake::FakeRunner;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_runner_enregistre_les_appels() {
        let r = FakeRunner::new();
        r.run("a", &["1".into()]).unwrap();
        r.spawn_detached("b", &[]).unwrap();
        assert_eq!(
            r.calls(),
            vec![
                ("a".to_string(), vec!["1".to_string()]),
                ("b".to_string(), vec![])
            ]
        );
    }

    #[test]
    fn fake_runner_reponse_programmee() {
        let r = FakeRunner::new();
        r.respond(
            "wmctrl",
            CommandOutput {
                success: false,
                stdout: String::new(),
            },
        );
        assert!(!r.run("wmctrl", &[]).unwrap().success);
        assert!(r.run("other", &[]).unwrap().success);
    }

    #[test]
    fn system_runner_execute_true() {
        assert!(SystemRunner.run("true", &[]).unwrap().success);
    }
}
