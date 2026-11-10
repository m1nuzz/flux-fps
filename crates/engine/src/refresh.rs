//! Exact refresh rate detection via `QueryDisplayConfig` (Phase 3.1, RF p.8.1).
//!
//! The pacer needs the *exact* active mode refresh rate (e.g. 239.996 Hz,
//! not a rounded 240) to compute frame deadlines without long-term drift.
//! `EnumDisplaySettings`/`IDXGIOutput::GetDesc` give rounded values; the
//! CCD path here returns the rational fraction as configured.

use crate::error::EngineError;
use windows_sys::Win32::Devices::Display::{
    GetDisplayConfigBufferSizes, QueryDisplayConfig, DISPLAYCONFIG_MODE_INFO,
    DISPLAYCONFIG_MODE_INFO_TYPE_TARGET, DISPLAYCONFIG_PATH_INFO,
    DISPLAYCONFIG_SCANLINE_ORDERING_PROGRESSIVE, QDC_ONLY_ACTIVE_PATHS,
};
use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::Graphics::Gdi::DISPLAYCONFIG_PATH_ACTIVE;

/// Converts a `DISPLAYCONFIG_RATIONAL` fraction to Hz.
///
/// Returns `None` for a zero denominator (invalid mode data). The result
/// keeps full single-precision fidelity: 239996/1000 -> 239.996 Hz.
pub fn refresh_hz(numerator: u32, denominator: u32) -> Option<f32> {
    if denominator == 0 {
        None
    } else {
        Some(numerator as f32 / denominator as f32)
    }
}

/// Highest progressive refresh rate (Hz) across all active display paths.
///
/// Interlaced modes are skipped (their `vSyncFreq` describes field rate, not
/// frame rate). If several monitors are active the maximum is returned - the
/// pacer targets the fastest sink.
pub fn max_active_refresh_hz() -> Result<f32, EngineError> {
    let (mut path_count, mut mode_count) = query_buffer_sizes()?;
    let mut paths: Vec<DISPLAYCONFIG_PATH_INFO> = Vec::new();
    let mut modes: Vec<DISPLAYCONFIG_MODE_INFO> = Vec::new();

    // Sizes can change between the sizing call and the data call (mode
    // changes, RDP reconnect); retry once on that well-known race.
    for attempt in 0..2 {
        paths.resize(path_count as usize, unsafe { std::mem::zeroed() });
        modes.resize(mode_count as usize, unsafe { std::mem::zeroed() });
        // SAFETY: buffers sized exactly as reported by the sizing call.
        let err = unsafe {
            QueryDisplayConfig(
                QDC_ONLY_ACTIVE_PATHS,
                &mut path_count,
                paths.as_mut_ptr(),
                &mut mode_count,
                modes.as_mut_ptr(),
                std::ptr::null_mut(),
            )
        };
        if err == ERROR_SUCCESS {
            break;
        }
        if attempt == 0 {
            // Re-query the (now larger) required sizes and try again.
            let (p, m) = query_buffer_sizes()?;
            path_count = p;
            mode_count = m;
            continue;
        }
        return Err(EngineError::Win32(
            err,
            "QueryDisplayConfig(QDC_ONLY_ACTIVE_PATHS)",
        ));
    }

    // Only the first path_count/mode_count entries are valid.
    paths.truncate(path_count as usize);
    modes.truncate(mode_count as usize);

    let active: Vec<u64> = paths
        .iter()
        .filter(|p| p.flags & DISPLAYCONFIG_PATH_ACTIVE != 0)
        .map(|p| {
            // adapterId (LUID) + target id identifies a target uniquely.
            (p.targetInfo.adapterId.LowPart as u64) << 32 | p.targetInfo.id as u64
        })
        .collect();

    let mut best: Option<f32> = None;
    for mode in &modes {
        if mode.infoType != DISPLAYCONFIG_MODE_INFO_TYPE_TARGET {
            continue;
        }
        let key = (mode.adapterId.LowPart as u64) << 32 | mode.id as u64;
        if !active.contains(&key) {
            continue;
        }
        // SAFETY: infoType == TARGET guarantees the union's targetMode arm.
        let signal = unsafe { mode.Anonymous.targetMode }.targetVideoSignalInfo;
        if signal.scanLineOrdering != DISPLAYCONFIG_SCANLINE_ORDERING_PROGRESSIVE {
            continue;
        }
        if let Some(hz) = refresh_hz(signal.vSyncFreq.Numerator, signal.vSyncFreq.Denominator) {
            best = Some(best.map_or(hz, |b: f32| b.max(hz)));
        }
    }

    best.ok_or(EngineError::InvalidState(
        "no active progressive display path found",
    ))
}

fn query_buffer_sizes() -> Result<(u32, u32), EngineError> {
    let mut path_count = 0u32;
    let mut mode_count = 0u32;
    // SAFETY: pure sizing query; data pointers are not dereferenced.
    let err = unsafe {
        GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut path_count, &mut mode_count)
    };
    if err != ERROR_SUCCESS {
        return Err(EngineError::Win32(
            err,
            "GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS)",
        ));
    }
    Ok((path_count, mode_count))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rational_to_hz() {
        assert_eq!(refresh_hz(240, 1), Some(240.0));
        assert_eq!(refresh_hz(60, 1), Some(60.0));
        assert_eq!(refresh_hz(144_000, 1_000), Some(144.0));
        // Exact non-integer mode rates survive (no 240 rounding of 239.996).
        let hz = refresh_hz(239_996, 1_000).expect("valid fraction");
        assert!((hz - 239.996).abs() < 1e-3, "got {hz}");
        // Rational approximations used by some drivers: 165.000976...
        let hz = refresh_hz(337_865, 2_048).expect("valid fraction");
        assert!((hz - 165.0).abs() < 0.01, "got {hz}");
    }

    #[test]
    fn invalid_fraction_is_none() {
        assert_eq!(refresh_hz(240, 0), None);
        assert_eq!(refresh_hz(0, 0), None);
    }
}
