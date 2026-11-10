//! Global application configuration (TOML, hot-reloadable).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Commit charge percentage that triggers a standby purge.
    pub purge_commit_threshold_percent: u8,
    /// Minimum available memory (MB) that triggers a standby purge.
    pub purge_available_threshold_mb: u32,
    /// Cooldown between purges, seconds.
    pub purge_cooldown_secs: u64,
    /// Target FPS cap (None disables pacing).
    pub fps_cap: Option<f32>,
    /// Target refresh rate hint; the master tool refines this per display.
    pub refresh_rate_hint_hz: f32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            purge_commit_threshold_percent: 85,
            purge_available_threshold_mb: 2048,
            purge_cooldown_secs: 60,
            fps_cap: None,
            refresh_rate_hint_hz: 240.0,
        }
    }
}
