//! Agent de code : Claude Code lancé en arrière-plan sur une branche à part d'un
//! projet connu, suivi, arrêté, puis annoncé à la fin.

pub mod claude;
pub mod git;
pub mod phrases;
pub mod projects;
pub mod registry;
pub mod settings;
pub mod supervisor;
#[cfg(any(test, feature = "test-util"))]
pub mod testing;
pub mod types;

pub use settings::{AgentSettings, DEFAULT_ALLOWED_TOOLS, DENIED_TOOLS, Project};
pub use types::{AgentCommand, EndReason, Progress, RunResult, TaskId, TaskRecord, TaskState};
