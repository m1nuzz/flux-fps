//! Process enumeration via the Toolhelp32 snapshot API.

use std::mem;

/// A running process as reported by the system snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessEntry {
    pub pid: u32,
    pub exe_name: String,
}

/// Critical system processes that must NEVER be touched by Game Mode.
/// Optimizing any of these can destabilize or brick the session.
pub const CRITICAL_PROCESSES: &[&str] = &[
    "system",
    "registry",
    "memory compression",
    "csrss.exe",
    "dwm.exe",
    "wininit.exe",
    "winlogon.exe",
    "services.exe",
    "lsass.exe",
    "smss.exe",
    "svchost.exe",
    "explorer.exe",
    "msmpeng.exe", // Windows Defender
    "securityhealthservice.exe",
    "wmiprvse.exe",
    "searchindexer.exe",
    "fontdrvhost.exe",
    "sihost.exe",
    "ctfmon.exe",
    "conhost.exe",
];

/// Lists all user and system processes (pid != 0 and != 4, which are the
/// Idle and System pseudo-processes).
pub fn list_processes() -> Result<Vec<ProcessEntry>, crate::EngineError> {
    // SAFETY: TH32CS_SNAPPROCESS with pid 0 creates a snapshot of all
    // processes; the returned handle is closed on all paths below.
    let snapshot = unsafe {
        windows_sys::Win32::System::Diagnostics::ToolHelp::CreateToolhelp32Snapshot(
            windows_sys::Win32::System::Diagnostics::ToolHelp::TH32CS_SNAPPROCESS,
            0,
        )
    };
    if snapshot == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
        return Err(crate::EngineError::win32("CreateToolhelp32Snapshot"));
    }
    struct SnapshotGuard(windows_sys::Win32::Foundation::HANDLE);
    impl Drop for SnapshotGuard {
        fn drop(&mut self) {
            // SAFETY: owned snapshot handle, closed exactly once.
            unsafe { windows_sys::Win32::Foundation::CloseHandle(self.0) };
        }
    }
    let _guard = SnapshotGuard(snapshot);

    let mut entry = zeroed_process_entry32w();
    entry.dwSize =
        mem::size_of::<windows_sys::Win32::System::Diagnostics::ToolHelp::PROCESSENTRY32W>() as u32;

    let mut processes = Vec::new();
    // SAFETY: entry.dwSize is set to the struct size; iteration follows the
    // documented Toolhelp32 first/next pattern.
    unsafe {
        if windows_sys::Win32::System::Diagnostics::ToolHelp::Process32FirstW(snapshot, &mut entry)
            != 0
        {
            loop {
                if entry.th32ProcessID != 0 && entry.th32ProcessID != 4 {
                    processes.push(ProcessEntry {
                        pid: entry.th32ProcessID,
                        exe_name: wide_to_string(&entry.szExeFile),
                    });
                }
                if windows_sys::Win32::System::Diagnostics::ToolHelp::Process32NextW(
                    snapshot, &mut entry,
                ) == 0
                {
                    break;
                }
            }
        }
    }
    Ok(processes)
}

/// Returns processes eligible for Game Mode isolation: everything except
/// the critical system list and the excluded pids.
pub fn background_candidates(
    excluded_pids: &std::collections::HashSet<u32>,
) -> Result<Vec<ProcessEntry>, crate::EngineError> {
    Ok(list_processes()?
        .into_iter()
        .filter(|p| !excluded_pids.contains(&p.pid))
        .filter(|p| {
            let lower = p.exe_name.to_ascii_lowercase();
            !CRITICAL_PROCESSES.contains(&lower.as_str())
        })
        .collect())
}

fn wide_to_string(wide: &[u16]) -> String {
    let len = wide.iter().position(|&c| c == 0).unwrap_or(wide.len());
    String::from_utf16_lossy(&wide[..len])
}

// PROCESSENTRY32W contains a large fixed-size array and no Default impl in
// windows-sys; zeroed initialization is required before setting dwSize.
fn zeroed_process_entry32w() -> windows_sys::Win32::System::Diagnostics::ToolHelp::PROCESSENTRY32W {
    // SAFETY: PROCESSENTRY32W is a plain POD C struct; all-zero bytes are a
    // valid pre-initialization state (dwSize is set right after).
    unsafe { mem::zeroed() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// Live-system test: the snapshot lists processes and includes this very
    /// test binary.
    #[test]
    fn snapshot_contains_own_process() {
        let own = std::process::id();
        let all = list_processes().expect("snapshot must succeed on live Windows");
        assert!(!all.is_empty());
        assert!(
            all.iter().any(|p| p.pid == own),
            "own test process must appear in the snapshot"
        );
    }

    /// Critical processes are never returned as background candidates.
    #[test]
    fn critical_processes_are_excluded() {
        let all = list_processes().expect("snapshot must succeed on live Windows");
        let excluded: HashSet<u32> = HashSet::new();
        let candidates = background_candidates(&excluded).expect("candidates must build");
        let candidate_names: HashSet<String> = candidates
            .iter()
            .map(|p| p.exe_name.to_ascii_lowercase())
            .collect();
        for critical in CRITICAL_PROCESSES {
            assert!(
                !candidate_names.contains(*critical),
                "critical process {critical} must never be a candidate"
            );
        }
        // Sanity: filtering actually removed something (svchost.exe always runs).
        assert!(candidates.len() < all.len() || all.iter().all(|p| p.pid == 0));
    }
}
