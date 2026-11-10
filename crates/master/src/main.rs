//! flux-fps-master: portable per-PC calibration suite.
//!
//! Phase 0 placeholder. Plans: core/HT-pair affinity sweep with frametime
//! capture, DPC/ISR profiling, standby purge responsiveness test, A/B gain
//! verification (APPLY / KEEP-INSIGNIFICANT / REVERT). The optional kernel
//! module (BYOVD, opt-in) is developed separately behind the "kernel-setup"
//! cargo feature - never in the resident binary.

fn main() {
    println!("flux-fps-master v{} (calibration suite placeholder)", common::VERSION);
}
