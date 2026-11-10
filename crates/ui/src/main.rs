//! flux-fps resident application entry point.
//!
//! Phase 2 milestone: console telemetry view wired to the Core Engine
//! runtime. The windows-reactor MVU frontend replaces this output in Phase 4.

use common::Command;
use std::time::Duration;

#[tokio::main]
async fn main() {
    println!(
        "flux-fps v{} starting (console telemetry mode)",
        common::VERSION
    );

    let config = common::Config::default();
    let (tx, rx) = tokio::sync::mpsc::channel(16);
    let telemetry = match engine::runtime::run(config, rx).await {
        Ok(rx) => rx,
        Err(e) => {
            eprintln!("[flux-fps] engine failed to start: {e}");
            return;
        }
    };

    // Ctrl+C sends a graceful shutdown command to the engine.
    let shutdown_tx = tx.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        println!("Ctrl+C received, shutting down...");
        let _ = shutdown_tx.try_send(Command::Shutdown);
    });

    // UI pacer: print the latest snapshot at ~2 Hz for the console view
    // (the real MVU UI updates at 30 Hz through DispatcherQueue).
    let mut ticker = tokio::time::interval(Duration::from_millis(500));
    loop {
        ticker.tick().await;
        let snapshot = telemetry.borrow().clone();
        let dpc_peak = snapshot.dpc_max_us.iter().copied().fold(0.0f32, f32::max);
        println!(
            "commit {:>3.0}% | avail {:>5} MB | managed {:>2} | max DPC {:>6.1} us | fps cap {:?}",
            snapshot.commit_percent,
            snapshot.available_mb,
            snapshot.managed_pids.len(),
            dpc_peak,
            snapshot.fps_cap,
        );
    }
}
