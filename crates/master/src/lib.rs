//! flux-fps-master: portable per-PC calibration and A/B validation suite.
//!
//! This crate is deliberately separate from the resident `flux-fps` binary.
//! It performs calibration, raw frametime capture, ABAB statistics, long soak
//! tests and kernel opt-in guardrails. Nothing here runs automatically: every
//! command is started explicitly by the machine owner.
//!
//! HARD RULE: no kernel driver is bundled and none is loaded. The kernel
//! lifecycle in [`guard`] is a documented, always-refused state machine.

pub mod calibration;
pub mod capture;
pub mod guard;
pub mod session;
pub mod soak;
pub mod stats;
