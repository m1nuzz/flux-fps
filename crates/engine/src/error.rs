//! Engine error type.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum EngineError {
    /// A Win32 call failed (carries the GetLastError value).
    #[error("win32 error {0}: {1}")]
    Win32(u32, &'static str),

    /// NtSetSystemInformation returned a non-zero NTSTATUS.
    #[error("NTSTATUS 0x{0:08X}")]
    NtStatus(i32),

    /// Process disappeared or is not accessible.
    #[error("process {0} not accessible")]
    ProcessInaccessible(u32),

    /// The elevated helper rejected a request.
    #[error("helper: {0}")]
    Helper(String),

    #[error("invalid system state: {0}")]
    InvalidState(&'static str),
}

impl EngineError {
    /// Builds an error from the current GetLastError value.
    pub fn win32(api: &'static str) -> Self {
        let code = unsafe { windows_sys::Win32::Foundation::GetLastError() };
        EngineError::Win32(if code == 0 { 1 } else { code }, api)
    }
}
