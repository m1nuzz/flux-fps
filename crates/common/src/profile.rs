//! Per-PC profile produced by `flux-fps-master.exe` and consumed by the
//! resident app. Schema-versioned for forward compatibility.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

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

impl Verdict {
    /// Stable, user-facing label used in reports and the settings panel.
    pub fn label(self) -> &'static str {
        match self {
            Self::Apply => "APPLY",
            Self::KeepInsignificant => "KEEP-INSIGNIFICANT",
            Self::Revert => "REVERT",
        }
    }
}

impl Profile {
    /// Per-user profile path: `%APPDATA%\flux-fps\profile.toml`.
    /// `flux-fps-master` writes here; the resident app only reads it.
    pub fn default_path() -> Option<PathBuf> {
        std::env::var_os("APPDATA")
            .map(|appdata| PathBuf::from(appdata).join("flux-fps").join("profile.toml"))
    }

    /// Loads a schema-compatible profile. A missing, broken or newer file is
    /// ignored: the resident app must always start with its defaults.
    pub fn load(path: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(path).ok()?;
        let profile: Self = toml::from_str(&text).ok()?;
        (profile.schema_version == PROFILE_SCHEMA_VERSION).then_some(profile)
    }

    /// One-line summary of the last A/B verdict for the settings panel.
    pub fn verdict_summary(&self) -> String {
        let details = format!("{} cores, {:.1} Hz", self.cores.len(), self.refresh_rate_hz);
        match self.last_verdict {
            Some(verdict) => format!(
                "Calibration profile: {} ({details}). Re-run A/B validation after any driver or hardware change.",
                verdict.label()
            ),
            None => format!(
                "Calibration profile loaded ({details}); no A/B verdict yet."
            ),
        }
    }
}
