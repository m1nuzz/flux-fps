//! Per-PC calibration: logical-core throughput sweep, exact refresh rate,
//! background CPU Set selection and standby-purge responsiveness.
//!
//! All work here is user-mode. The sweep uses thread affinity on the tool's
//! own thread only; it never touches a game process.

use common::{Profile, ProfileCore, Verdict, PROFILE_SCHEMA_VERSION};
use engine::cpu_isolation::{e_core_ids, enumerate_cpu_sets};
use engine::memory::query_memory_stats;
use std::fs;
use std::hint::black_box;
use std::path::Path;
use std::time::Instant;
use windows_sys::Win32::System::Threading::{GetCurrentThread, SetThreadAffinityMask};

const WORKLOAD_ITERATIONS: u64 = 2_000_000;

/// Deterministic integer workload; the result is consumed through black_box
/// so the optimizer cannot remove the loop.
fn workload() -> u64 {
    let mut accumulator: u64 = 0x9E37_79B9_7F4A_7C15;
    for step in 0..WORKLOAD_ITERATIONS {
        accumulator = accumulator
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(step);
    }
    accumulator
}

/// Measures one logical processor. Indexes above the 64-bit affinity mask
/// (processor groups) are reported as unavailable rather than guessed.
fn benchmark_logical_processor(index: usize) -> Option<f32> {
    if index >= usize::BITS as usize {
        return None;
    }
    // SAFETY: GetCurrentThread returns a pseudo handle valid for this thread.
    // The previous mask is restored before returning, including on the error
    // path of the workload itself (there is none - the workload cannot fail).
    let handle = unsafe { GetCurrentThread() };
    let previous = unsafe { SetThreadAffinityMask(handle, 1usize << index) };
    if previous == 0 {
        return None;
    }
    let started = Instant::now();
    let result = workload();
    let elapsed = started.elapsed().as_secs_f32();
    // SAFETY: restoring the mask returned by the first successful call.
    unsafe { SetThreadAffinityMask(handle, previous) };
    black_box(result);
    if elapsed <= 0.0 {
        return None;
    }
    Some(WORKLOAD_ITERATIONS as f32 / elapsed)
}

#[derive(Debug, Clone)]
pub struct CalibrationReport {
    pub cores: Vec<ProfileCore>,
    pub refresh_hz: f32,
    pub background_cpu_set_ids: Vec<u32>,
    /// Available memory gained by one low-priority standby purge, MB.
    pub purge_gain_mb: i64,
    pub purge_duration_ms: u128,
    pub purge_available: bool,
}

/// Runs the full calibration and returns the profile plus a printable report.
pub fn calibrate() -> Result<(Profile, CalibrationReport), String> {
    let descriptors = enumerate_cpu_sets().map_err(|e| format!("CPU set enumeration: {e}"))?;
    let mut scores = Vec::with_capacity(descriptors.len());
    for index in 0..descriptors.len() {
        scores.push(benchmark_logical_processor(index));
    }
    let peak = scores.iter().flatten().copied().fold(0.0_f32, f32::max);
    let cores: Vec<ProfileCore> = descriptors
        .iter()
        .zip(scores.iter())
        .map(|(core, score)| ProfileCore {
            cpu_set_id: core.cpu_set_id,
            is_p_core: core.is_p_core,
            score: score
                .map(|s| if peak > 0.0 { s / peak } else { 0.0 })
                .unwrap_or(0.0),
        })
        .collect();

    let refresh_hz = max_active_refresh_hz_safe();
    let background_cpu_set_ids = e_core_ids(&descriptors);
    let (purge_available, purge_gain_mb, purge_duration_ms) = purge_responsiveness();

    let profile = Profile {
        schema_version: PROFILE_SCHEMA_VERSION,
        cores: cores.clone(),
        background_cpu_set_ids: background_cpu_set_ids.clone(),
        refresh_rate_hz: refresh_hz,
        // 1/1.10 leaves roughly 10% headroom, matching Config's p99 factor.
        pacing_safety_factor: 1.0 / 1.10,
        last_verdict: None,
    };
    Ok((
        profile,
        CalibrationReport {
            cores,
            refresh_hz,
            background_cpu_set_ids,
            purge_gain_mb,
            purge_duration_ms,
            purge_available,
        },
    ))
}

fn max_active_refresh_hz_safe() -> f32 {
    match engine::refresh::max_active_refresh_hz() {
        Ok(hz) if hz > 0.0 => hz,
        _ => 60.0,
    }
}

/// One low-priority standby purge; absence of privilege is reported, not fatal.
fn purge_responsiveness() -> (bool, i64, u128) {
    let before = match query_memory_stats() {
        Ok(stats) => i64::from(stats.available_mb),
        Err(_) => return (false, 0, 0),
    };
    let started = Instant::now();
    if engine::helper::purge_standby().is_err() {
        return (false, 0, started.elapsed().as_millis());
    }
    let after = query_memory_stats()
        .map(|stats| i64::from(stats.available_mb))
        .unwrap_or(before);
    (true, after - before, started.elapsed().as_millis())
}

pub fn write_profile(path: &Path, profile: &Profile) -> Result<(), String> {
    let text =
        toml::to_string_pretty(profile).map_err(|e| format!("cannot serialize profile: {e}"))?;
    fs::write(path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

pub fn load_profile(path: &Path) -> Result<Profile, String> {
    let text =
        fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    toml::from_str(&text).map_err(|e| format!("cannot parse {}: {e}", path.display()))
}

/// Stamps the latest A/B verdict into an existing profile.
pub fn update_verdict(path: &Path, verdict: Verdict) -> Result<(), String> {
    let mut profile = load_profile(path)?;
    profile.last_verdict = Some(verdict);
    write_profile(path, &profile)
}

/// Compares two profiles after a driver or application update (RF p.10.4).
pub fn compare(a: &Profile, b: &Profile) -> String {
    let mut lines = Vec::new();
    lines.push(format!(
        "Schema: v{} vs v{}",
        a.schema_version, b.schema_version
    ));
    lines.push(format!(
        "Refresh rate: {:.3} Hz vs {:.3} Hz (delta {:+.3})",
        a.refresh_rate_hz,
        b.refresh_rate_hz,
        b.refresh_rate_hz - a.refresh_rate_hz
    ));
    lines.push(format!(
        "Pacing safety factor: {:.3} vs {:.3}",
        a.pacing_safety_factor, b.pacing_safety_factor
    ));
    lines.push(format!(
        "Background CPU sets: {} vs {}",
        a.background_cpu_set_ids.len(),
        b.background_cpu_set_ids.len()
    ));
    lines.push(format!(
        "Verdict: {:?} vs {:?}",
        a.last_verdict, b.last_verdict
    ));
    let mut regressions = 0;
    for core_a in &a.cores {
        if let Some(core_b) = b
            .cores
            .iter()
            .find(|core| core.cpu_set_id == core_a.cpu_set_id)
        {
            let delta = core_b.score - core_a.score;
            if delta < -0.10 {
                regressions += 1;
                lines.push(format!(
                    "REGRESSION: cpu set {} score {:.2} -> {:.2}",
                    core_a.cpu_set_id, core_a.score, core_b.score
                ));
            }
        }
    }
    lines.push(format!("Core regressions (< -0.10): {regressions}"));
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(cores: &[(u32, f32)], verdict: Option<Verdict>) -> Profile {
        Profile {
            schema_version: PROFILE_SCHEMA_VERSION,
            cores: cores
                .iter()
                .map(|&(id, score)| ProfileCore {
                    cpu_set_id: id,
                    is_p_core: true,
                    score,
                })
                .collect(),
            background_cpu_set_ids: vec![4, 5],
            refresh_rate_hz: 239.996,
            pacing_safety_factor: 0.909,
            last_verdict: verdict,
        }
    }

    #[test]
    fn profile_round_trip_preserves_verdict() {
        let dir = std::env::temp_dir().join(format!("flux-fps-profile-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("profile.toml");
        let original = profile(&[(0, 1.0), (1, 0.8)], Some(Verdict::Apply));
        write_profile(&path, &original).unwrap();
        let loaded = load_profile(&path).unwrap();
        assert_eq!(loaded.last_verdict, Some(Verdict::Apply));
        assert_eq!(loaded.cores.len(), 2);
        assert!((loaded.refresh_rate_hz - 239.996).abs() < 0.001);
        update_verdict(&path, Verdict::Revert).unwrap();
        assert_eq!(
            load_profile(&path).unwrap().last_verdict,
            Some(Verdict::Revert)
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn compare_reports_core_regressions() {
        let a = profile(&[(0, 1.0), (1, 0.9)], None);
        let b = profile(&[(0, 0.7), (1, 0.9)], None);
        let report = compare(&a, &b);
        assert!(report.contains("REGRESSION: cpu set 0"));
        assert!(report.contains("Core regressions (< -0.10): 1"));
    }
}
