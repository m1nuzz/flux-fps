//! Frametime capture for one A/B run.
//!
//! Preferred source: our own ETW `Microsoft-Windows-DxgKrnl` Present consumer
//! in the engine crate (zero-touch, no overlays, no game process handles).
//! Fallback: PresentMon CLI, which must be supplied explicitly by the user.
//! HAGS state is recorded because GPU-side PresentMon fields are unreliable
//! when HAGS is enabled (RESEARCH_FINDINGS p.9.3).

use crate::session::read_frametimes;
use std::path::Path;
use std::process::Command;
use std::thread;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct CaptureOptions<'a> {
    pub seconds: u64,
    /// 0 = all presenting processes.
    pub target_pid: u32,
    pub output: &'a Path,
    pub presentmon: Option<&'a str>,
    pub process_name: Option<&'a str>,
}

/// Reads HwSchMode: 2 = HAGS enabled, 1 = disabled.
pub fn hags_enabled() -> Option<bool> {
    let output = Command::new("reg.exe")
        .args([
            "query",
            r"HKLM\SYSTEM\CurrentControlSet\Control\GraphicsDrivers",
            "/v",
            "HwSchMode",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text.lines().find(|line| line.contains("HwSchMode"))?;
    let value = line.split_whitespace().last()?.trim();
    match value {
        "0x2" => Some(true),
        "0x1" => Some(false),
        _ => None,
    }
}

fn capture_etw(options: &CaptureOptions<'_>) -> Result<Vec<f32>, String> {
    let (monitor, _stats) = engine::etw_present::start()
        .map_err(|e| format!("cannot start the ETW Present session: {e}"))?;
    if options.target_pid != 0 {
        monitor.set_target_pid(options.target_pid);
    }
    monitor.begin_recording();
    thread::sleep(Duration::from_secs(options.seconds));
    let deltas_us = monitor.end_recording();
    drop(monitor);
    let frames_ms: Vec<f32> = deltas_us.iter().map(|us| us / 1000.0).collect();
    if frames_ms.is_empty() {
        return Err("ETW session produced no Present events".into());
    }
    Ok(frames_ms)
}

fn capture_presentmon(options: &CaptureOptions<'_>) -> Result<Vec<f32>, String> {
    let executable = options
        .presentmon
        .ok_or("PresentMon path is required when the ETW session is unavailable")?;
    let process = options
        .process_name
        .ok_or("--process is required for PresentMon capture")?;
    let status = Command::new(executable)
        .arg("--process_name")
        .arg(process)
        .arg("--output_file")
        .arg(options.output)
        .arg("--timed")
        .arg(options.seconds.to_string())
        .status()
        .map_err(|e| format!("cannot start PresentMon: {e}"))?;
    if !status.success() {
        return Err(format!("PresentMon exited with {status}"));
    }
    read_frametimes(options.output)
}

/// Captures a run and writes the canonical frametime CSV. Falls back to
/// PresentMon only when it was explicitly configured.
pub fn capture(options: &CaptureOptions<'_>) -> Result<Vec<f32>, String> {
    if options.seconds == 0 {
        return Err("capture duration must be positive".into());
    }
    let frames_ms = match capture_etw(options) {
        Ok(frames) => frames,
        Err(etw_error) => match capture_presentmon(options) {
            Ok(frames) => {
                eprintln!("[flux-fps-master] ETW unavailable ({etw_error}); used PresentMon.");
                frames
            }
            Err(_) => {
                return Err(format!(
                    "{etw_error}; PresentMon fallback unavailable (pass --presentmon and --process)"
                ))
            }
        },
    };
    crate::session::write_frametimes(options.output, &frames_ms)?;
    Ok(frames_ms)
}
