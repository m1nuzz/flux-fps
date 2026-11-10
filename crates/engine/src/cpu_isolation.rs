//! CPU Sets-based background process isolation.
//!
//! Strategy: NEVER pin the game (that fights Intel Thread Director). Instead,
//! background processes are pushed onto E-cores via soft affinity (CPU Sets),
//! with low memory priority and EcoQoS. The game keeps unrestricted scheduling.

use crate::error::EngineError;
use std::mem;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::SystemInformation::{
    CpuSetInformation, GetSystemCpuSetInformation, SYSTEM_CPU_SET_INFORMATION,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, ProcessMemoryPriority, ProcessPowerThrottling,
    SetProcessDefaultCpuSets, SetProcessInformation, PROCESS_INFORMATION_CLASS,
    PROCESS_POWER_THROTTLING_STATE, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_SET_LIMITED_INFORMATION,
};

/// Efficiency class of a CPU set on hybrid CPUs.
pub const EFFICIENCY_CLASS_E_CORE: u8 = 0;
pub const EFFICIENCY_CLASS_P_CORE: u8 = 1;

/// Memory priority for background processes (low: their pages are evicted
/// from standby before the game's pages).
const MEMORY_PRIORITY_LOW: u8 = 1;

/// PROCESS_INFORMATION_CLASS values (Win32 enum, NOT the Nt PROCESSINFOCLASS).
const PROCESS_MEMORY_PRIORITY: PROCESS_INFORMATION_CLASS = ProcessMemoryPriority;
const PROCESS_POWER_THROTTLING: PROCESS_INFORMATION_CLASS = ProcessPowerThrottling;

const PROCESS_POWER_THROTTLING_EXECUTION_STATE: u32 = 0x1;
const PROCESS_POWER_THROTTLING_CURRENT_VERSION: u32 = 1;

/// One CPU set (logical processor) discovered via GetSystemCpuSetInformation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CoreDescriptor {
    pub cpu_set_id: u32,
    pub group: u16,
    pub efficiency_class: u8,
    pub is_p_core: bool,
}

/// RAII wrapper closing the process handle on drop.
struct ProcessHandle(HANDLE);

impl Drop for ProcessHandle {
    fn drop(&mut self) {
        // SAFETY: handle is owned by this wrapper and closed exactly once.
        unsafe { CloseHandle(self.0) };
    }
}

/// Enumerates all CPU sets of the current system.
///
/// SAFETY: GetSystemCpuSetInformation writes variable-size records; records
/// are walked by advancing through the buffer by each record's `Size` field.
pub fn enumerate_cpu_sets() -> Result<Vec<CoreDescriptor>, EngineError> {
    let process = unsafe { GetCurrentProcess() };
    let mut needed: u32 = 0;

    // SAFETY: NULL buffer is documented: call returns required size.
    if unsafe { GetSystemCpuSetInformation(std::ptr::null_mut(), 0, &mut needed, process, 0) } == 0
        && needed == 0
    {
        return Err(EngineError::win32("GetSystemCpuSetInformation(size)"));
    }

    let mut buffer = vec![0u8; needed as usize];
    let mut returned: u32 = 0;
    // SAFETY: buffer is allocated with the size returned by the first call.
    if unsafe {
        GetSystemCpuSetInformation(
            buffer.as_mut_ptr() as *mut SYSTEM_CPU_SET_INFORMATION,
            needed,
            &mut returned,
            process,
            0,
        )
    } == 0
    {
        return Err(EngineError::win32("GetSystemCpuSetInformation"));
    }

    let mut cores = Vec::new();
    let mut offset = 0usize;
    while offset + mem::size_of::<SYSTEM_CPU_SET_INFORMATION>() <= returned as usize {
        // SAFETY: offset walks the buffer by each record's own Size field,
        // which guarantees alignment to the record start.
        let record =
            unsafe { &*(buffer.as_ptr().add(offset) as *const SYSTEM_CPU_SET_INFORMATION) };
        if record.Size == 0 {
            break;
        }
        // CPU_SET_INFORMATION_TYPE == CpuSetInformation (0) means this record
        // is a CpuSet member.
        if record.Type == CpuSetInformation {
            // SAFETY: reading the single variant of this union.
            let cpu_set = unsafe { &record.Anonymous.CpuSet };
            cores.push(CoreDescriptor {
                cpu_set_id: cpu_set.Id,
                group: cpu_set.Group,
                efficiency_class: cpu_set.EfficiencyClass,
                is_p_core: cpu_set.EfficiencyClass != EFFICIENCY_CLASS_E_CORE,
            });
        }
        offset += record.Size as usize;
    }

    if cores.is_empty() {
        return Err(EngineError::InvalidState("no CPU sets returned"));
    }
    Ok(cores)
}

/// Returns CPU set ids classified as E-cores.
pub fn e_core_ids(cores: &[CoreDescriptor]) -> Vec<u32> {
    cores
        .iter()
        .filter(|c| !c.is_p_core)
        .map(|c| c.cpu_set_id)
        .collect()
}

/// Enables Game Mode isolation for a background process:
/// CPU Sets (default sets) + low memory priority + EcoQoS power throttling.
pub fn apply_background_isolation(pid: u32, cpu_set_ids: &[u32]) -> Result<(), EngineError> {
    let handle = open_process_limited(pid)?;
    set_default_cpu_sets(handle.0, cpu_set_ids)?;
    set_memory_priority(handle.0, MEMORY_PRIORITY_LOW)?;
    set_power_throttling(handle.0, true)?;
    Ok(())
}

/// Disables Game Mode isolation: clears default CPU Sets, restores normal
/// memory priority and turns EcoQoS off (ControlMask=0x1, StateMask=0x0).
pub fn remove_background_isolation(pid: u32) -> Result<(), EngineError> {
    let handle = open_process_limited(pid)?;
    set_default_cpu_sets(handle.0, &[])?;
    set_memory_priority(handle.0, 4)?; // MEMORY_PRIORITY_NORMAL
    set_power_throttling(handle.0, false)?;
    Ok(())
}

fn open_process_limited(pid: u32) -> Result<ProcessHandle, EngineError> {
    // SAFETY: constants and arguments match the documented OpenProcess ABI.
    let handle = unsafe {
        OpenProcess(
            PROCESS_SET_LIMITED_INFORMATION | PROCESS_QUERY_LIMITED_INFORMATION,
            0,
            pid,
        )
    };
    if handle.is_null() {
        return Err(EngineError::ProcessInaccessible(pid));
    }
    Ok(ProcessHandle(handle))
}

fn set_default_cpu_sets(handle: HANDLE, cpu_set_ids: &[u32]) -> Result<(), EngineError> {
    let (ptr, count) = if cpu_set_ids.is_empty() {
        (std::ptr::null(), 0u32)
    } else {
        (cpu_set_ids.as_ptr(), cpu_set_ids.len() as u32)
    };
    // SAFETY: ptr points to `count` valid u32 values (or NULL/0 to reset).
    if unsafe { SetProcessDefaultCpuSets(handle, ptr, count) } == 0 {
        return Err(EngineError::win32("SetProcessDefaultCpuSets"));
    }
    Ok(())
}

fn set_memory_priority(handle: HANDLE, priority: u8) -> Result<(), EngineError> {
    // SAFETY: class PROCESS_MEMORY_PRIORITY expects exactly one BYTE.
    if unsafe {
        SetProcessInformation(
            handle,
            PROCESS_MEMORY_PRIORITY,
            &priority as *const u8 as *mut core::ffi::c_void,
            mem::size_of::<u8>() as u32,
        )
    } == 0
    {
        return Err(EngineError::win32("SetProcessInformation(MemoryPriority)"));
    }
    Ok(())
}

fn set_power_throttling(handle: HANDLE, enable: bool) -> Result<(), EngineError> {
    let state = PROCESS_POWER_THROTTLING_STATE {
        Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        // OFF: ControlMask=0x1, StateMask=0x0 (a zero ControlMask is a no-op).
        ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_STATE,
        StateMask: if enable {
            PROCESS_POWER_THROTTLING_EXECUTION_STATE
        } else {
            0
        },
    };
    // SAFETY: class PROCESS_POWER_THROTTLING expects PROCESS_POWER_THROTTLING_STATE.
    if unsafe {
        SetProcessInformation(
            handle,
            PROCESS_POWER_THROTTLING,
            &state as *const _ as *mut core::ffi::c_void,
            mem::size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
        )
    } == 0
    {
        return Err(EngineError::win32("SetProcessInformation(PowerThrottling)"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Live-system test: CPU set enumeration must succeed on any Windows 10+
    /// machine and return a non-empty, consistent core list.
    #[test]
    fn enumerate_returns_consistent_cores() {
        let cores = enumerate_cpu_sets().expect("enumeration must succeed on live Windows");
        assert!(!cores.is_empty());
        // CPU set ids are unique.
        let mut ids: Vec<u32> = cores.iter().map(|c| c.cpu_set_id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), cores.len(), "cpu set ids must be unique");
    }

    /// e_core_ids must be a subset of the enumerated ids and only contain
    /// non-P-core entries.
    #[test]
    fn e_core_ids_are_valid_subset() {
        let cores = enumerate_cpu_sets().expect("enumeration must succeed on live Windows");
        let e_ids = e_core_ids(&cores);
        for id in &e_ids {
            let core = cores
                .iter()
                .find(|c| c.cpu_set_id == *id)
                .expect("e-core id must map back to a core");
            assert!(!core.is_p_core);
            assert_eq!(core.efficiency_class, EFFICIENCY_CLASS_E_CORE);
        }
    }

    /// Isolation apply/remove round-trip on the current process. Safe: all
    /// three mechanisms are soft settings that we immediately revert.
    #[test]
    fn isolation_round_trip_on_current_process() {
        let cores = enumerate_cpu_sets().expect("enumeration must succeed on live Windows");
        // Use E-cores when present, otherwise the first core (non-hybrid CI
        // machines) - the point is that settings apply and revert cleanly.
        let targets = match e_core_ids(&cores).first() {
            Some(id) => vec![*id],
            None => vec![cores[0].cpu_set_id],
        };
        let pid = std::process::id();
        apply_background_isolation(pid, &targets).expect("apply must succeed for own process");
        remove_background_isolation(pid).expect("remove must succeed for own process");
    }
}
