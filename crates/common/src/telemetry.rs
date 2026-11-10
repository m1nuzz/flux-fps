//! Telemetry snapshot exchanged between the Core Engine and the UI.
//!
//! Published through `tokio::sync::watch` (latest value only - the UI never
//! queues bursts), throttled to 30 Hz by the pacer task.

#[derive(Debug, Clone, Default)]
pub struct TelemetrySnapshot {
    pub game_mode_active: bool,
    pub managed_pids: Vec<u32>,
    /// Commit charge percentage (0-100).
    pub commit_percent: f32,
    /// Available physical memory, MB.
    pub available_mb: u32,
    /// Max DPC duration (microseconds) observed in the 2 s window, per core.
    pub dpc_max_us: Vec<f32>,
    /// Current FPS cap target, if pacing is active.
    pub fps_cap: Option<f32>,
}
