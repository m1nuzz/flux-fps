//! Commands sent from the UI thread to the Core Engine.
//!
//! UI to Engine traffic uses `tokio::sync::mpsc` with bounded capacity; the UI
//! side must use `try_send` only (the STA thread has no async context and must
//! never block).

#[derive(Debug, Clone)]
pub enum Command {
    /// Enable Game Mode isolation for a background process (CPU Sets +
    /// memory priority + EcoQoS).
    ApplyGameMode { pid: u32 },
    /// Revert Game Mode isolation for a background process.
    RemoveGameMode { pid: u32 },
    /// Trigger a low-priority standby list purge immediately.
    PurgeStandbyNow,
    /// Set the target FPS cap (None disables pacing).
    SetFpsCap(Option<f32>),
    /// UI/tray quick toggle for automatic Game Mode management.
    SetGameModeEnabled(bool),
    /// Enable/disable the adaptive FPS limit advisor.
    SetPacerEnabled(bool),
    /// Pacer aggressiveness: safety factor applied to p99 frametime.
    SetPacerSafetyFactor(f32),
    /// Memory purge trigger thresholds (settings panel).
    SetPurgeThresholds {
        commit_percent: u8,
        available_mb: u32,
        cooldown_secs: u64,
    },
    /// Replace the protected game process list.
    SetGameWhitelist(Vec<String>),
    /// Shut the engine down gracefully.
    Shutdown,
}
