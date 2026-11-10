//! Core Engine runtime: tokio task wiring that combines Game Mode ticks,
//! memory purge triggers, ETW DPC monitoring and telemetry publishing.
//!
//! Bridge rules (RESEARCH_FINDINGS.md 5.3): telemetry flows out through a
//! `tokio::sync::watch` (latest value only); commands flow in through a
//! bounded `mpsc`; no blocking Win32 calls on the UI thread - the blocking
//! purge runs in `tokio::task::spawn_blocking`.

use crate::etw_dpc;
use crate::game_mode::GameModeManager;
use crate::memory;
use crate::memory_purge;
use crate::telemetry::TelemetryHub;
use common::{Command, Config, TelemetrySnapshot};
use std::collections::HashSet;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, watch};

/// Starts the engine runtime and returns the UI-side telemetry receiver.
///
/// `commands` is the UI-side sender's counterpart; the runtime owns it. The
/// ETW DPC monitor starts best-effort: without elevation (or when another
/// tool owns the kernel logger) telemetry simply lacks DPC data.
pub async fn run(
    mut config: Config,
    mut commands: mpsc::Receiver<Command>,
) -> Result<watch::Receiver<TelemetrySnapshot>, crate::EngineError> {
    let (hub, telemetry_rx) = TelemetryHub::new();
    let mut manager = GameModeManager::new()?;

    // ETW DPC monitor: best-effort, admin-only. The monitor handle must stay
    // alive for the whole runtime, otherwise its Drop stops the session.
    let mut dpc_monitor: Option<etw_dpc::DpcMonitor> = None;
    let mut dpc_rx: Option<watch::Receiver<Vec<f32>>> = None;
    match tokio::task::spawn_blocking(move || etw_dpc::start(etw_dpc::DpcMonitorConfig::default()))
        .await
    {
        Ok(Ok((monitor, rx))) => {
            dpc_monitor = Some(monitor);
            dpc_rx = Some(rx);
        }
        Ok(Err(e)) => eprintln!("[flux-fps] ETW DPC monitor unavailable: {e}"),
        Err(e) => eprintln!("[flux-fps] ETW DPC monitor task failed: {e}"),
    }
    let _ = &mut dpc_monitor; // kept alive until run() returns

    let mut last_purge: Option<Instant> = None;
    let mut tick = tokio::time::interval(Duration::from_secs(1));

    loop {
        tokio::select! {
            _ = tick.tick() => {
                // 1. Memory check + pressure-based purge.
                let stats = match memory::query_memory_stats() {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("[flux-fps] memory stats failed: {e}");
                        continue;
                    }
                };
                let elapsed = last_purge.map(|t| t.elapsed());
                if memory::should_purge(&stats, &config, elapsed) {
                    last_purge = Some(Instant::now());
                    match tokio::task::spawn_blocking(memory_purge::purge_low_priority_standby_list)
                        .await
                    {
                        Ok(Ok(())) => eprintln!(
                            "[flux-fps] standby purge done (commit {}%, avail {} MB)",
                            stats.commit_percent, stats.available_mb
                        ),
                        Ok(Err(e)) => eprintln!("[flux-fps] standby purge failed: {e}"),
                        Err(e) => eprintln!("[flux-fps] purge task join failed: {e}"),
                    }
                }

                // 2. Game Mode tick (scan + apply/remove).
                let mut excluded: HashSet<u32> = HashSet::new();
                excluded.insert(std::process::id());
                let candidates = match crate::process::background_candidates(&excluded) {
                    Ok(c) => c,
                    Err(e) => {
                        eprintln!("[flux-fps] process scan failed: {e}");
                        continue;
                    }
                };
                manager.tick(&candidates, &mut |pid, e| {
                    eprintln!("[flux-fps] game mode error for pid {pid}: {e}")
                });

                // 3. Publish telemetry snapshot.
                let dpc = match &dpc_rx {
                    Some(rx) => rx.borrow().clone(),
                    None => Vec::new(),
                };
                hub.publish(TelemetrySnapshot {
                    game_mode_active: manager.is_enabled(),
                    managed_pids: manager.managed_pids(),
                    commit_percent: stats.commit_percent,
                    available_mb: stats.available_mb,
                    dpc_max_us: dpc,
                    fps_cap: config.fps_cap,
                });
            }
            command = commands.recv() => {
                match command {
                    Some(Command::ApplyGameMode { pid }) | Some(Command::RemoveGameMode { pid }) => {
                        // One-off manual control: remove is a no-op for
                        // unmanaged pids; apply is handled on the next tick
                        // via the exclusion set in a future revision.
                        let _ = pid;
                    }
                    Some(Command::PurgeStandbyNow) => {
                        last_purge = Some(Instant::now());
                        let _ = tokio::task::spawn_blocking(
                            memory_purge::purge_low_priority_standby_list,
                        ).await;
                    }
                    Some(Command::SetFpsCap(cap)) => config.fps_cap = cap,
                    Some(Command::SetGameModeEnabled(on)) => {
                        manager.set_enabled(on, &mut |pid, e| {
                            eprintln!("[flux-fps] game mode toggle error for pid {pid}: {e}")
                        });
                    }
                    Some(Command::Shutdown) | None => {
                        manager.reset(&mut |pid, e| {
                            eprintln!("[flux-fps] game mode reset error for pid {pid}: {e}")
                        });
                        hub.publish(TelemetrySnapshot::default());
                        break;
                    }
                }
            }
        }
    }
    Ok(telemetry_rx)
}
