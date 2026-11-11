//! flux-fps Core Engine: OS-level system optimization for gaming.
//!
//! HARD RULES: user-mode only in this crate; no game memory access, no DLL
//! injection, no kernel drivers. Kernel access (BYOVD) exists only in the
//! separate portable flux-fps-master tool, never in the resident binary.
//!
//! Privileged user-mode operations are delegated through `helper`/`pipe` to
//! an elevated copy of the resident binary started by Task Scheduler.

pub mod cpu_isolation;
pub mod error;
pub mod etw_dpc;
pub mod etw_present;
pub mod game_mode;
pub mod helper;
pub mod helper_state;
pub mod memory;
pub mod memory_purge;
pub mod pacing;
pub mod pacing_advisor;
pub mod pipe;
pub mod process;
pub mod refresh;
pub mod runtime;
pub mod telemetry;

pub use error::EngineError;
