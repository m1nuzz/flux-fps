//! flux-fps Core Engine: OS-level system optimization for gaming.
//!
//! HARD RULES: user-mode only in this crate; no game memory access, no DLL
//! injection, no kernel drivers. Kernel access (BYOVD) exists only in the
//! separate portable flux-fps-master tool, never in the resident binary.

pub mod cpu_isolation;
pub mod error;
pub mod memory_purge;
pub mod telemetry;

pub mod etw_dpc;

pub use error::EngineError;
