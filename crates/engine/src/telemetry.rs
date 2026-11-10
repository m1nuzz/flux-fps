//! Latest-value telemetry publishing for the UI.
//!
//! Engine -> UI traffic uses `tokio::sync::watch` (only the newest snapshot is
//! retained) so the UI never has to drain bursty ETW callbacks. A separate
//! 30 Hz pacer task forwards snapshots into the UI thread.

use common::TelemetrySnapshot;
use tokio::sync::watch;

pub struct TelemetryHub {
    tx: watch::Sender<TelemetrySnapshot>,
}

impl TelemetryHub {
    /// Creates the hub and returns it together with the UI-side receiver.
    pub fn new() -> (Self, watch::Receiver<TelemetrySnapshot>) {
        let (tx, rx) = watch::channel(TelemetrySnapshot::default());
        (Self { tx }, rx)
    }

    /// Publishes a new snapshot; receivers observe it on their next borrow.
    pub fn publish(&self, snapshot: TelemetrySnapshot) {
        let _ = self.tx.send(snapshot);
    }

    /// Borrows the latest snapshot.
    pub fn latest(&self) -> TelemetrySnapshot {
        self.tx.borrow().clone()
    }
}
