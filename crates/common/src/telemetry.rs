//! Telemetry snapshot exchanged between the Core Engine and the UI.
//!
//! Published through `tokio::sync::watch` (latest value only - the UI never
//! queues bursts), throttled to 30 Hz by the pacer task.

/// One logical core for the dashboard P/E map.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CoreInfo {
    pub cpu_set_id: u32,
    /// 0 = E-core, 1 = P-core (Intel Thread Director classes).
    pub efficiency_class: u8,
}

/// Aggregate frametime statistics from the ETW Present monitor (Phase 3.3).
///
/// All frametime values are milliseconds; `None` means no Present events have
/// been observed in the sliding window yet.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct FrametimeStats {
    /// Mean frametime over the sliding window, ms.
    pub avg_ms: Option<f32>,
    /// 99th percentile frametime, ms - the "worst 1%" of frames.
    pub p99_ms: Option<f32>,
    /// Presents observed per second in the sliding window.
    pub presents_per_sec: f32,
    /// Number of frametime samples in the window.
    pub samples: usize,
}

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
    /// Static per-core topology for the P/E map (populated at engine start).
    pub cpu_cores: Vec<CoreInfo>,
    /// Latest frametime aggregate from the DxgKrnl Present session.
    pub frametimes: FrametimeStats,
}
