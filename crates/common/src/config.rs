//! Global application configuration (TOML, hot-reloadable).

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

impl Config {
    /// Per-user config path: `%APPDATA%\flux-fps\config.toml`.
    pub fn default_path() -> Option<PathBuf> {
        std::env::var_os("APPDATA")
            .map(|appdata| PathBuf::from(appdata).join("flux-fps").join("config.toml"))
    }

    /// Loads the config, falling back to defaults for a missing or broken
    /// file (the app must always start; a broken file is preserved as .bak).
    pub fn load_or_default(path: &std::path::Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(text) => match toml::from_str(&text) {
                Ok(config) => config,
                Err(e) => {
                    eprintln!("[flux-fps] config parse failed ({e}); using defaults");
                    let _ = rename_bak(path);
                    Self::default()
                }
            },
            Err(_) => Self::default(),
        }
    }

    /// Saves the config, creating parent directories as needed.
    pub fn save(&self, path: &std::path::Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = toml::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(path, text)
    }
}

fn rename_bak(path: &std::path::Path) -> std::io::Result<()> {
    let mut bak = path.as_os_str().to_owned();
    bak.push(".bak");
    std::fs::rename(path, bak)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_preserves_fields() {
        let config = Config {
            fps_cap: Some(141.0),
            purge_cooldown_secs: 30,
            ..Config::default()
        };
        let dir = std::env::temp_dir().join(format!("flux-fps-test-{}", std::process::id()));
        let path = dir.join("config.toml");
        config.save(&path).expect("save");
        let loaded = Config::load_or_default(&path);
        assert_eq!(loaded.fps_cap, Some(141.0));
        assert_eq!(loaded.purge_cooldown_secs, 30);
        assert_eq!(loaded.purge_commit_threshold_percent, 85);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn broken_file_falls_back_to_defaults() {
        let dir = std::env::temp_dir().join(format!("flux-fps-test-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("config.toml");
        std::fs::write(&path, "not [ valid toml").expect("write");
        let loaded = Config::load_or_default(&path);
        assert_eq!(loaded, Config::default());
        // Broken file preserved as .bak for the user.
        assert!(dir.join("config.toml.bak").exists());
        std::fs::remove_dir_all(&dir).ok();
    }
}
