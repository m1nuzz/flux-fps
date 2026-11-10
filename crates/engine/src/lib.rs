//! flux-fps Core Engine: OS-level system optimization for gaming.
//!
//! HARD RULES: user-mode only in this crate; no game memory access, no DLL
//! injection, no kernel drivers. Kernel access (BYOVD) exists only in the
//! separate portable flux-fps-master tool, never in the resident binary.

pub mod cpu_isolation;
pub mod error;
pub mod memory_purge;
pub mod telemetry;

pub mod etw_dpc {
    //! DPC/ISR latency monitoring via the NT Kernel Logger session.
    //!
    //! Planned (Phase 2, step 4): StartTraceW + EnableTraceEx2 with
    //! EVENT_TRACE_FLAG_DPC | EVENT_TRACE_FLAG_INTERRUPT; parse PerfInfoGuid
    //! events 27/28; duration = Timestamp - InitialTime.
}

pub use error::EngineError;
