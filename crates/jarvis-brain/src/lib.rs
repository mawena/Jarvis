pub mod brain;
pub mod config;
pub mod ducking;
pub mod fastpath;
pub mod intent;
pub mod listen;
pub mod llm;
pub mod logging;
pub mod mode_store;
pub mod server;
pub mod speaker;
pub mod stt;
pub mod system_skill;
#[cfg(any(test, feature = "test-util"))]
pub mod testing;
