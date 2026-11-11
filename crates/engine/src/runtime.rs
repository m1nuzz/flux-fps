//! Core Engine runtime: tokio task wiring that combines Game Mode ticks,
//! memory purge triggers, ETW DPC monitoring and telemetry publishing.
//!
//! Bridge rules (RESEARCH_FINDINGS.md 5.3): telemetry flows out through a
//! `tokio::sync::watch` (latest value only); commands flow in through a
//! bounded `mpsc`; no blocking Win32 calls on the UI thread - the blocking
//! purge runs in `tokio::task::spawn_blocking`.

use crate::etw_dpc;
use crate::etw_present;
use crate::game_mode::GameModeManager;
use crate::helper;
use crate::memory;
use crate::pacing_advisor::{AdvisorLimits, AdvisorParams, LimitAdvisor};
use crate::telemetry::TelemetryHub;
use common::{Command, Config, FrametimeStats, PacerAdvice, TelemetrySnapshot};
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

    // Exact refresh rate of the fastest active sink (Phase 3.1). The advisor
    // uses it as the ceiling; the configured hint is the fallback.
    let max_refresh_hz = match crate::refresh::max_active_refresh_hz() {
        Ok(hz) => {
            eprintln!("[flux-fps] display refresh: {hz:.3} Hz");
            hz
        }
        Err(e) => {
            eprintln!("[flux-fps] refresh detection failed: {e}; using configured hint");
            config.refresh_rate_hint_hz
        }
    };
    let mut advisor = LimitAdvisor::new(Instant::now());
    let mut last_advice: Option<f32> = None;

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

    // ETW DxgKrnl Present monitor: best-effort frametime source (Phase 3.3).
    let mut present_monitor: Option<etw_present::PresentMonitor> = None;
    let mut present_rx: Option<watch::Receiver<FrametimeStats>> = None;
    match tokio::task::spawn_blocking(etw_present::start).await {
        Ok(Ok((monitor, rx))) => {
            present_monitor = Some(monitor);
            present_rx = Some(rx);
        }
        Ok(Err(e)) => eprintln!("[flux-fps] ETW Present monitor unavailable: {e}"),
        Err(e) => eprintln!("[flux-fps] ETW Present monitor task failed: {e}"),
    }
    let _ = &mut present_monitor; // kept alive until run() returns

    // Static per-core topology for the dashboard P/E map.
    let cpu_cores: Vec<common::CoreInfo> = crate::cpu_isolation::enumerate_cpu_sets()
        .map(|cores| {
            cores
                .iter()
                .map(|c| common::CoreInfo {
                    cpu_set_id: c.cpu_set_id,
                    efficiency_class: c.efficiency_class,
                })
                .collect()
        })
        .unwrap_or_default();

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
                    match tokio::task::spawn_blocking(helper::purge_standby).await {
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
                let candidates: Vec<crate::process::ProcessEntry> =
                    match crate::process::background_candidates(&excluded) {
                        Ok(c) => c
                            .into_iter()
                            .filter(|p| !config.is_whitelisted(&p.exe_name))
                            .collect(),
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
                let frametimes = match &present_rx {
                    Some(rx) => *rx.borrow(),
                    None => FrametimeStats::default(),
                };
                let pacer = if config.pacer_enabled {
                    advisor.update(
                        &frametimes,
                        &AdvisorParams::from_config(&config),
                        &AdvisorLimits {
                            max_fps: max_refresh_hz,
                            user_cap: config.fps_cap,
                        },
                        Instant::now(),
                    )
                } else {
                    PacerAdvice::default()
                };
                if pacer.suggested_fps != last_advice {
                    if let Some(fps) = pacer.suggested_fps {
                        eprintln!(
                            "[flux-fps] pacer advice: {fps:.1} FPS (interval {} * 100 ns, {})",
                            crate::pacing::frame_interval_100ns(fps, 1),
                            pacer.reason
                        );
                    }
                    last_advice = pacer.suggested_fps;
                }
                hub.publish(TelemetrySnapshot {
                    game_mode_active: manager.is_enabled(),
                    managed_pids: manager.managed_pids(),
                    commit_percent: stats.commit_percent,
                    available_mb: stats.available_mb,
                    dpc_max_us: dpc,
                    fps_cap: config.fps_cap,
                    cpu_cores: cpu_cores.clone(),
                    frametimes,
                    pacer,
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
                        let _ = tokio::task::spawn_blocking(helper::purge_standby).await;
                    }
                    Some(Command::SetFpsCap(cap)) => config.fps_cap = cap,
                    Some(Command::SetPacerEnabled(on)) => config.pacer_enabled = on,
                    Some(Command::SetPacerSafetyFactor(factor)) => {
                        config.pacer_safety_factor = factor.clamp(1.0, 2.0);
                    }
                    Some(Command::SetPurgeThresholds {
                        commit_percent,
                        available_mb,
                        cooldown_secs,
                    }) => {
                        config.purge_commit_threshold_percent = commit_percent;
                        config.purge_available_threshold_mb = available_mb;
                        config.purge_cooldown_secs = cooldown_secs;
                    }
                    Some(Command::SetGameWhitelist(list)) => {
                        config.game_whitelist = list;
                    }
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
