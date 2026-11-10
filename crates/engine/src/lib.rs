//! flux-fps Core Engine: OS-level system optimization for gaming.
//!
//! HARD RULES: user-mode only in this crate; no game memory access, no DLL
//! injection, no kernel drivers. Kernel access (BYOVD) exists only in the
//! separate portable flux-fps-master tool, never in the resident binary.

pub mod cpu_isolation;
pub mod error;
pub mod etw_dpc;
pub mod game_mode;
pub mod memory;
pub mod memory_purge;
pub mod pacing;
pub mod process;
pub mod runtime;
pub mod telemetry;

pub use error::EngineError;
