//! Per-PC profile produced by `flux-fps-master.exe` and consumed by the
//! resident app. Schema-versioned for forward compatibility.

use serde::{Deserialize, Serialize};

pub const PROFILE_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    pub schema_version: u32,
    /// Per-core calibration results (best/worst cores for GPU driver affinity).
    pub cores: Vec<ProfileCore>,
    /// Recommended background isolation CPU set ids (E-cores).
    pub background_cpu_set_ids: Vec<u32>,
    /// Refined refresh rate measured via QueryDisplayConfig (e.g. 239.996).
    pub refresh_rate_hz: f32,
    /// Pacing safety factor (0.0-1.0) applied to the target interval.
    pub pacing_safety_factor: f32,
    /// Last A/B verification verdict.
    pub last_verdict: Option<Verdict>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileCore {
    pub cpu_set_id: u32,
    pub is_p_core: bool,
    /// Higher is better (normalized benchmark score).
    pub score: f32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Verdict {
    Apply,
    KeepInsignificant,
    Revert,
}
