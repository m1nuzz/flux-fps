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

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(verdict: Option<Verdict>) -> Profile {
        Profile {
            schema_version: PROFILE_SCHEMA_VERSION,
            cores: vec![
                ProfileCore {
                    cpu_set_id: 0,
                    is_p_core: true,
                    score: 1.0,
                },
                ProfileCore {
                    cpu_set_id: 1,
                    is_p_core: false,
                    score: 0.6,
                },
            ],
            background_cpu_set_ids: vec![1],
            refresh_rate_hz: 239.996,
            pacing_safety_factor: 0.909,
            last_verdict: verdict,
        }
    }

    #[test]
    fn verdict_labels_are_stable() {
        assert_eq!(Verdict::Apply.label(), "APPLY");
        assert_eq!(Verdict::KeepInsignificant.label(), "KEEP-INSIGNIFICANT");
        assert_eq!(Verdict::Revert.label(), "REVERT");
    }

    #[test]
    fn summary_reports_both_states() {
        let pending = sample(None).verdict_summary();
        assert!(pending.contains("2 cores"));
        assert!(pending.contains("no A/B verdict"));
        let applied = sample(Some(Verdict::Apply)).verdict_summary();
        assert!(applied.contains("APPLY"));
    }

    #[test]
    fn loader_ignores_missing_broken_and_future_profiles() {
        let dir = std::env::temp_dir().join(format!("flux-fps-profile-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("profile.toml");

        assert!(Profile::load(&path).is_none());
        std::fs::write(&path, "not valid toml ==").unwrap();
        assert!(Profile::load(&path).is_none());

        let good = sample(Some(Verdict::Revert));
        std::fs::write(&path, toml::to_string_pretty(&good).unwrap()).unwrap();
        let loaded = Profile::load(&path).unwrap();
        assert_eq!(loaded.last_verdict, Some(Verdict::Revert));
        assert_eq!(loaded.cores.len(), 2);

        let mut future = good;
        future.schema_version = PROFILE_SCHEMA_VERSION + 1;
        std::fs::write(&path, toml::to_string_pretty(&future).unwrap()).unwrap();
        assert!(Profile::load(&path).is_none());

        std::fs::remove_dir_all(&dir).ok();
    }
}
