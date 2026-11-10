//! Low-priority standby list purge via NtSetSystemInformation
//! (SystemMemoryListInformation = 80, command MemoryPurgeLowPriorityStandbyList = 4).
//!
//! Requires the process token to hold SeProfileSingleProcessPrivilege and the
//! process to run elevated. Full purge (command 3) and FlushModifiedList
//! (command 5) are intentionally NOT exposed - see RESEARCH_FINDINGS.md 3.3.

use crate::error::EngineError;
use std::mem;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::Security::{
    AdjustTokenPrivileges, LookupPrivilegeValueW, LUID_AND_ATTRIBUTES, SE_PRIVILEGE_ENABLED,
    TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

// NtSetSystemInformation is not exposed by windows-sys; declare it directly.
// SAFETY ABI: documented NT-native function signature (ntdll.dll).
#[link(name = "ntdll")]
extern "system" {
    fn NtSetSystemInformation(
        SystemInformationClass: i32,
        SystemInformation: *mut core::ffi::c_void,
        SystemInformationLength: u32,
    ) -> i32;
}

/// SystemInformationClass for memory list commands.
const SYSTEM_MEMORY_LIST_INFORMATION: i32 = 80;
/// Purges only priority-0 standby pages (safe subset).
const MEMORY_PURGE_LOW_PRIORITY_STANDBY_LIST: u32 = 4;

/// RAII wrapper closing the token handle on drop.
struct TokenHandle(HANDLE);

impl Drop for TokenHandle {
    fn drop(&mut self) {
        // SAFETY: handle is owned by this wrapper and closed exactly once.
        unsafe { CloseHandle(self.0) };
    }
}

/// Purges the low-priority standby list.
///
/// SAFETY: NtSetSystemInformation with SystemMemoryListInformation expects a
/// pointer to a single u32 command value and its size (4 bytes).
pub fn purge_low_priority_standby_list() -> Result<(), EngineError> {
    enable_profile_single_process_privilege()?;
    let command = MEMORY_PURGE_LOW_PRIORITY_STANDBY_LIST;
    // SAFETY: SystemMemoryListInformation expects a single u32 command value
    // plus its size; the privilege has been enabled just above.
    let status = unsafe {
        NtSetSystemInformation(
            SYSTEM_MEMORY_LIST_INFORMATION,
            &command as *const u32 as *mut core::ffi::c_void,
            mem::size_of_val(&command) as u32,
        )
    };
    if status != 0 {
        return Err(EngineError::NtStatus(status));
    }
    Ok(())
}

/// Enables SeProfileSingleProcessPrivilege on the current process token.
fn enable_profile_single_process_privilege() -> Result<(), EngineError> {
    let mut token: HANDLE = std::ptr::null_mut();
    // SAFETY: GetCurrentProcess returns a pseudo-handle; OpenProcessToken
    // writes an owned handle into `token` (closed by TokenHandle).
    if unsafe {
        OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY, &mut token)
    } == 0
    {
        return Err(EngineError::win32("OpenProcessToken"));
    }
    let _token = TokenHandle(token);

    let name: Vec<u16> = "SeProfileSingleProcessPrivilege\0"
        .encode_utf16()
        .collect();
    let mut luid = LUID_AND_ATTRIBUTES {
        // SAFETY: LUID is a plain POD; zeroing it is a valid initialization.
        Luid: unsafe { mem::zeroed() },
        Attributes: SE_PRIVILEGE_ENABLED,
    };
    // SAFETY: name is a NUL-terminated wide string; luid is a valid out param.
    if unsafe {
        LookupPrivilegeValueW(std::ptr::null(), name.as_ptr(), &mut luid.Luid)
    } == 0
    {
        return Err(EngineError::win32("LookupPrivilegeValueW"));
    }

    let tp = TOKEN_PRIVILEGES {
        PrivilegeCount: 1,
        Privileges: [luid],
    };
    // SAFETY: tp is a well-formed one-entry TOKEN_PRIVILEGES; the last two
    // arguments are optional (NULL, 0) per the documented ABI.
    if unsafe {
        AdjustTokenPrivileges(token, 0, &tp, 0, std::ptr::null_mut(), std::ptr::null_mut())
    } == 0
    {
        return Err(EngineError::win32("AdjustTokenPrivileges"));
    }
    // AdjustTokenPrivileges succeeds even if the privilege cannot be assigned;
    // the actual NtSetSystemInformation call then fails with
    // STATUS_PRIVILEGE_NOT_HELD, which the caller surfaces.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Requires elevation (run manually as admin: `cargo test -- --ignored`).
    /// Purges the low-priority standby list once and expects STATUS_SUCCESS.
    /// Modifies system memory state; hence ignored by default.
    #[test]
    #[ignore = "requires elevation and modifies system memory state"]
    fn purge_succeeds_when_elevated() {
        purge_low_priority_standby_list().expect("purge must succeed when elevated");
    }
}
