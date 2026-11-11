//! 24 h soak harness and the helper kill/restore regression test
//! (PHASES.md Phase 6 / RESEARCH_FINDINGS p.10).
//!
//! Samples the elevated helper pipe, the isolation journal, memory pressure
//! and our ETW session names. Nothing here runs automatically: the owner
//! starts `flux-fps-master soak` explicitly.

use common::{HelperReply, HelperRequest};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SoakSample {
    pub elapsed_secs: u64,
    pub helper_reachable: bool,
    pub isolated_pids: usize,
    pub commit_percent: f32,
    pub available_mb: u32,
    /// Number of our own ETW sessions still registered (leak detector).
    pub etw_sessions: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SoakSummary {
    pub samples: usize,
    pub helper_failures: usize,
    pub max_isolated_pids: usize,
    pub max_etw_sessions: usize,
    pub restore_ok: bool,
}

/// Pure aggregation so the soak report can be unit-tested without OS state.
pub fn summarize(samples: &[SoakSample]) -> SoakSummary {
    SoakSummary {
        samples: samples.len(),
        helper_failures: samples.iter().filter(|s| !s.helper_reachable).count(),
        max_isolated_pids: samples.iter().map(|s| s.isolated_pids).max().unwrap_or(0),
        max_etw_sessions: samples.iter().map(|s| s.etw_sessions).max().unwrap_or(0),
        restore_ok: true,
    }
}

fn helper_reachable() -> bool {
    match engine::pipe::PipeClient::connect() {
        Ok(client) => matches!(
            client.call(&HelperRequest::Ping),
            Ok(HelperReply::Pong { .. })
        ),
        Err(_) => false,
    }
}

/// Counts our own ETW sessions in `logman query -ets`. A growing count over a
/// soak indicates a session leak (the monitor failed to call ControlTrace).
fn etw_session_count() -> usize {
    let Ok(output) = Command::new("logman.exe").args(["query", "-ets"]).output() else {
        return 0;
    };
    if !output.status.success() {
        return 0;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| line.contains("flux-fps"))
        .count()
}

fn probe(elapsed_secs: u64) -> SoakSample {
    let memory = engine::memory::query_memory_stats().ok();
    SoakSample {
        elapsed_secs,
        helper_reachable: helper_reachable(),
        isolated_pids: engine::helper_state::load().len(),
        commit_percent: memory.map(|m| m.commit_percent).unwrap_or(0.0),
        available_mb: memory.map(|m| m.available_mb).unwrap_or(0),
        etw_sessions: etw_session_count(),
    }
}

fn csv_header() -> &'static str {
    "elapsed_secs,helper_reachable,isolated_pids,commit_percent,available_mb,etw_sessions\n"
}

fn csv_row(sample: &SoakSample) -> String {
    format!(
        "{},{},{},{:.2},{},{}\n",
        sample.elapsed_secs,
        u8::from(sample.helper_reachable),
        sample.isolated_pids,
        sample.commit_percent,
        sample.available_mb,
        sample.etw_sessions
    )
}

/// Runs the soak. `--hours 24` is the documented target; shorter durations are
/// allowed for smoke runs. Returns the CSV path and the aggregate summary.
pub fn run(hours: u64, interval_secs: u64, dir: &Path) -> Result<(PathBuf, SoakSummary), String> {
    if hours == 0 || interval_secs == 0 {
        return Err("hours and interval must both be positive".into());
    }
    let interval = Duration::from_secs(interval_secs);
    let total = Duration::from_secs(hours.saturating_mul(3600));
    let started = Instant::now();
    let mut samples = Vec::new();

    while started.elapsed() < total {
        thread::sleep(interval);
        samples.push(probe(started.elapsed().as_secs()));
    }

    let csv_path = dir.join(format!("soak-{}.csv", crate::session::utc_date()));
    let mut text = String::from(csv_header());
    for sample in &samples {
        text.push_str(&csv_row(sample));
    }
    fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    fs::write(&csv_path, text).map_err(|e| format!("cannot write {}: {e}", csv_path.display()))?;

    let mut summary = summarize(&samples);
    summary.restore_ok = restore_and_check();
    Ok((csv_path, summary))
}

fn restore_and_check() -> bool {
    engine::helper::restore_all().is_ok() && engine::helper_state::load().is_empty()
}

/// Locates the elevated helper process by its command line, if present.
pub fn helper_pid() -> Option<u32> {
    let script = "Get-CimInstance Win32_Process -Filter \"Name='flux-fps.exe'\" | Where-Object { $_.CommandLine -like '*--helper*' } | Select-Object -First 1 -ExpandProperty ProcessId";
    let output = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .output()
        .ok()?;
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<u32>()
        .ok()
}

/// Regression test for "restore after kill -9 of the helper" (Phase 6):
/// terminates the helper, lets Task Scheduler restart it, then verifies the
/// isolation journal is empty after a full restore.
pub fn restore_after_kill() -> Result<(), String> {
    let pid = helper_pid().ok_or("elevated helper is not running")?;
    let status = Command::new("taskkill.exe")
        .args(["/F", "/PID", &pid.to_string()])
        .status()
        .map_err(|e| format!("cannot terminate helper: {e}"))?;
    if !status.success() {
        return Err(format!("taskkill exited with {status}"));
    }
    // The scheduled task has RestartCount 3 with a 1 minute interval. Give it
    // a short window, but the restore path must work even if it never returns.
    thread::sleep(Duration::from_secs(5));
    if restore_and_check() {
        Ok(())
    } else {
        Err("isolation journal was not cleared after helper restart".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarize_counts_failures_and_extremes() {
        let samples = [
            SoakSample {
                elapsed_secs: 60,
                helper_reachable: true,
                isolated_pids: 2,
                commit_percent: 40.0,
                available_mb: 8000,
                etw_sessions: 1,
            },
            SoakSample {
                elapsed_secs: 120,
                helper_reachable: false,
                isolated_pids: 5,
                commit_percent: 55.0,
                available_mb: 6000,
                etw_sessions: 3,
            },
        ];
        let summary = summarize(&samples);
        assert_eq!(summary.samples, 2);
        assert_eq!(summary.helper_failures, 1);
        assert_eq!(summary.max_isolated_pids, 5);
        assert_eq!(summary.max_etw_sessions, 3);
        assert!(summary.restore_ok);
    }

    #[test]
    fn csv_rows_are_parseable() {
        let sample = SoakSample {
            elapsed_secs: 60,
            helper_reachable: true,
            isolated_pids: 0,
            commit_percent: 42.5,
            available_mb: 1234,
            etw_sessions: 1,
        };
        let row = csv_row(&sample);
        let fields: Vec<&str> = row.trim().split(',').collect();
        assert_eq!(fields.len(), 6);
        assert_eq!(fields[1], "1");
        assert_eq!(csv_header().split(',').count(), 6);
    }
}
