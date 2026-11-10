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
    /// Shut the engine down gracefully.
    Shutdown,
}
