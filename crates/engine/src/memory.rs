//! Memory pressure statistics and the purge decision function.
//!
//! The purge is pressure-based, never timer-based (RESEARCH_FINDINGS.md 3.3):
//! commit charge over threshold OR available memory under threshold, with a
//! cooldown, and never twice too fast.

use common::Config;
use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

/// Snapshot of system memory state relevant to purge decisions.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MemoryStats {
    /// Memory load percentage (0-100), as reported by GlobalMemoryStatusEx.
    pub commit_percent: f32,
    /// Available physical memory in megabytes.
    pub available_mb: u32,
    /// Total physical memory in megabytes.
    pub total_mb: u32,
}

/// Queries current memory statistics.
pub fn query_memory_stats() -> Result<MemoryStats, crate::EngineError> {
    let mut status = MEMORYSTATUSEX {
        dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
        dwMemoryLoad: 0,
        ullTotalPhys: 0,
        ullAvailPhys: 0,
        ullTotalPageFile: 0,
        ullAvailPageFile: 0,
        ullTotalVirtual: 0,
        ullAvailVirtual: 0,
        ullAvailExtendedVirtual: 0,
    };
    // SAFETY: status is a correctly initialized MEMORYSTATUSEX (dwLength set).
    if unsafe { GlobalMemoryStatusEx(&mut status) } == 0 {
        return Err(crate::EngineError::win32("GlobalMemoryStatusEx"));
    }
    const MB: u64 = 1024 * 1024;
    Ok(MemoryStats {
        commit_percent: status.dwMemoryLoad as f32,
        available_mb: (status.ullAvailPhys / MB) as u32,
        total_mb: (status.ullTotalPhys / MB) as u32,
    })
}

/// Pure decision function: should a low-priority standby purge run now?
///
/// `elapsed_since_last` is `None` when no purge has happened yet this session.
pub fn should_purge(
    stats: &MemoryStats,
    config: &Config,
    elapsed_since_last: Option<std::time::Duration>,
) -> bool {
    let pressure = stats.commit_percent >= f32::from(config.purge_commit_threshold_percent)
        || stats.available_mb <= config.purge_available_threshold_mb;
    if !pressure {
        return false;
    }
    match elapsed_since_last {
        // No purge yet: only fire when memory is actually tight.
        None => true,
        Some(elapsed) => elapsed.as_secs() >= config.purge_cooldown_secs,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> Config {
        Config {
            purge_commit_threshold_percent: 85,
            purge_available_threshold_mb: 2048,
            purge_cooldown_secs: 60,
            ..Config::default()
        }
    }

    fn stats(commit: f32, available_mb: u32) -> MemoryStats {
        MemoryStats {
            commit_percent: commit,
            available_mb,
            total_mb: 32768,
        }
    }

    #[test]
    fn no_pressure_means_no_purge() {
        assert!(!should_purge(&stats(50.0, 8192), &config(), None));
        assert!(!should_purge(
            &stats(50.0, 8192),
            &config(),
            Some(std::time::Duration::from_secs(9999))
        ));
    }

    #[test]
    fn pressure_without_prior_purge_fires() {
        assert!(should_purge(&stats(90.0, 8192), &config(), None));
        assert!(should_purge(&stats(50.0, 1024), &config(), None));
    }

    #[test]
    fn cooldown_blocks_immediate_repurge() {
        assert!(!should_purge(
            &stats(90.0, 1024),
            &config(),
            Some(std::time::Duration::from_secs(30))
        ));
    }

    #[test]
    fn cooldown_elapsed_allows_purge() {
        assert!(should_purge(
            &stats(90.0, 1024),
            &config(),
            Some(std::time::Duration::from_secs(61))
        ));
    }
}
