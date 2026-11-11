//! Named Pipe transport for the elevated helper (RESEARCH_FINDINGS.md 9.1).
//!
//! The helper creates a message-mode pipe whose DACL grants access only to
//! SYSTEM, Builtin Administrators and the current user's own SID. The pipe
//! name also embeds that SID, so two logged-on sessions cannot collide.
//! Framing is one UTF-8 line per pipe message, capped at
//! [`common::MAX_MESSAGE_BYTES`].

use crate::error::EngineError;
use common::{HelperReply, HelperRequest, MAX_MESSAGE_BYTES};
use std::ffi::c_void;
use std::marker::PhantomData;
use std::mem;
use std::ptr;
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, LocalFree, ERROR_BROKEN_PIPE, ERROR_MORE_DATA, ERROR_NO_DATA,
    ERROR_PIPE_CONNECTED, GENERIC_READ, GENERIC_WRITE, HANDLE, HLOCAL, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{TokenUser, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FlushFileBuffers, ReadFile, WriteFile, FILE_ATTRIBUTE_NORMAL,
    FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    PIPE_ACCESS_DUPLEX,
};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, SetNamedPipeHandleState,
    PIPE_READMODE_MESSAGE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_MESSAGE, PIPE_UNLIMITED_INSTANCES,
    PIPE_WAIT,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows_sys::Win32::UI::Shell::IsUserAnAdmin;

/// RAII wrapper for a single Win32 handle.
struct OwnedHandle(HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
            // SAFETY: the handle is owned by this wrapper and closed once.
            unsafe { CloseHandle(self.0) };
        }
    }
}

/// RAII wrapper for a security descriptor allocated by LocalAlloc.
struct OwnedSecurityDescriptor(*mut c_void);

impl Drop for OwnedSecurityDescriptor {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: ConvertStringSecurityDescriptorToSecurityDescriptorW
            // allocates with LocalAlloc, so LocalFree is the correct release.
            unsafe { LocalFree(self.0) };
        }
    }
}

/// True when the current process has an elevated token.
pub fn is_elevated() -> bool {
    // SAFETY: no arguments, no memory access; returns a BOOL.
    unsafe { IsUserAnAdmin() != 0 }
}

/// Reads the current user's SID as a string (for example `S-1-5-21-...`).
pub fn current_user_sid() -> Result<String, EngineError> {
    let mut token: HANDLE = ptr::null_mut();
    // SAFETY: GetCurrentProcess returns a pseudo-handle; OpenProcessToken
    // writes an owned handle into `token`, closed by OwnedHandle below.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(EngineError::win32("OpenProcessToken"));
    }
    let _token = OwnedHandle(token);

    // u64 slots guarantee 8-byte alignment for the TOKEN_USER structure.
    let mut buffer = vec![0u64; 64];
    let mut needed: u32 = 0;
    // SAFETY: buffer is 512 aligned bytes; GetTokenInformation writes a
    // TOKEN_USER at its start and reports the required size in `needed`.
    let ok = unsafe {
        windows_sys::Win32::Security::GetTokenInformation(
            token,
            TokenUser,
            buffer.as_mut_ptr() as *mut c_void,
            (buffer.len() * mem::size_of::<u64>()) as u32,
            &mut needed,
        )
    };
    if ok == 0 {
        return Err(EngineError::win32("GetTokenInformation"));
    }
    // SAFETY: a successful TokenUser query places a TOKEN_USER at offset 0;
    // its User.Sid points inside the same allocation, which is still alive.
    let sid = unsafe { (*(buffer.as_ptr() as *const TOKEN_USER)).User.Sid };
    if sid.is_null() {
        return Err(EngineError::InvalidState("token user SID is null"));
    }

    let mut text: *mut u16 = ptr::null_mut();
    // SAFETY: sid is a valid PSID; the callee allocates a NUL-terminated
    // string SID that we release with LocalFree below.
    if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 || text.is_null() {
        return Err(EngineError::win32("ConvertSidToStringSidW"));
    }
    let result = wide_to_string(text);
    // SAFETY: text came from ConvertSidToStringSidW (LocalAlloc).
    unsafe { LocalFree(text as HLOCAL) };
    Ok(result)
}

/// Converts a NUL-terminated UTF-16 string into an owned String.
///
/// SAFETY: `text` must point to a readable, NUL-terminated UTF-16 buffer.
unsafe fn wide_to_string(text: *const u16) -> String {
    let mut length = 0usize;
    // SAFETY: caller guarantees a NUL terminator; the 256-char bound keeps a
    // corrupted buffer from walking unrelated memory.
    while length < 256 && unsafe { *text.add(length) } != 0 {
        length += 1;
    }
    // SAFETY: `length` words were verified readable above.
    let words = unsafe { std::slice::from_raw_parts(text, length) };
    String::from_utf16_lossy(words)
}

/// SDDL for the pipe DACL: protected DACL, GENERIC_ALL for SYSTEM and
/// Builtin Administrators, plus the interactive user's own SID.
fn sddl_for(user_sid: &str) -> String {
    format!("D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GA;;;{user_sid})")
}

/// Builds a self-relative security descriptor from a per-user SDDL string.
fn security_descriptor_for(user_sid: &str) -> Result<OwnedSecurityDescriptor, EngineError> {
    let sddl = sddl_for(user_sid);
    let wide: Vec<u16> = sddl.encode_utf16().chain(std::iter::once(0)).collect();
    let mut descriptor: *mut c_void = ptr::null_mut();
    let mut size: u32 = 0;
    // SAFETY: wide is NUL-terminated; the callee allocates a descriptor that
    // OwnedSecurityDescriptor releases after CreateNamedPipeW has copied it.
    let ok = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            wide.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            &mut size,
        )
    };
    if ok == 0 || descriptor.is_null() {
        return Err(EngineError::win32(
            "ConvertStringSecurityDescriptorToSecurityDescriptorW",
        ));
    }
    Ok(OwnedSecurityDescriptor(descriptor))
}

/// Writes one pipe message. Message mode guarantees all-or-nothing delivery.
fn write_message(handle: HANDLE, bytes: &[u8]) -> Result<(), EngineError> {
    if bytes.is_empty() || bytes.len() > MAX_MESSAGE_BYTES {
        return Err(EngineError::InvalidState("pipe message size out of range"));
    }
    let mut written: u32 = 0;
    // SAFETY: handle is a valid pipe endpoint; bytes outlives the call.
    let ok = unsafe {
        WriteFile(
            handle,
            bytes.as_ptr(),
            bytes.len() as u32,
            &mut written,
            ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(EngineError::win32("WriteFile"));
    }
    if written as usize != bytes.len() {
        return Err(EngineError::InvalidState("short pipe write"));
    }
    // Best effort: on a message-mode pipe this waits for the peer to read.
    // SAFETY: handle is a valid pipe endpoint; failures are non-fatal.
    let _ = unsafe { FlushFileBuffers(handle) };
    Ok(())
}

/// Reads one pipe message. `Ok(None)` means the peer disconnected cleanly.
fn read_message(handle: HANDLE) -> Result<Option<String>, EngineError> {
    let mut buffer = [0u8; MAX_MESSAGE_BYTES];
    let mut read: u32 = 0;
    // SAFETY: handle is a valid pipe endpoint; buffer is sized for one frame.
    let ok = unsafe {
        ReadFile(
            handle,
            buffer.as_mut_ptr(),
            buffer.len() as u32,
            &mut read,
            ptr::null_mut(),
        )
    };
    if ok == 0 {
        let code = unsafe { GetLastError() };
        return match code {
            ERROR_BROKEN_PIPE | ERROR_NO_DATA => Ok(None),
            ERROR_MORE_DATA => Err(EngineError::InvalidState(
                "pipe message exceeds frame limit",
            )),
            _ => Err(EngineError::Win32(code, "ReadFile")),
        };
    }
    if read == 0 {
        return Ok(None);
    }
    let text = String::from_utf8_lossy(&buffer[..read as usize]);
    Ok(Some(text.trim().to_string()))
}

/// Server-side pipe instance owned by the elevated helper.
pub struct PipeServer {
    listener: OwnedHandle,
}

impl PipeServer {
    /// Creates a per-user, ACL-protected pipe for the current user.
    pub fn create() -> Result<Self, EngineError> {
        let sid = current_user_sid()?;
        let path = common::pipe_path(&sid);
        let name: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
        let descriptor = security_descriptor_for(&sid)?;
        let attributes = SECURITY_ATTRIBUTES {
            nLength: mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0,
            bInheritHandle: 0,
        };
        // SAFETY: name is NUL-terminated; attributes points at a descriptor
        // that stays alive until this call returns (Windows copies the DACL).
        let handle = unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                MAX_MESSAGE_BYTES as u32,
                MAX_MESSAGE_BYTES as u32,
                0,
                &attributes,
            )
        };
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            return Err(EngineError::win32("CreateNamedPipeW"));
        }
        Ok(Self {
            listener: OwnedHandle(handle),
        })
    }

    /// Blocks until a client connects and returns one request/reply session.
    pub fn accept(&self) -> Result<PipeSession<'_>, EngineError> {
        // SAFETY: listener is a valid pipe instance; synchronous mode, so the
        // overlapped pointer must be null.
        let connected = unsafe { ConnectNamedPipe(self.listener.0, ptr::null_mut()) };
        if connected == 0 {
            let code = unsafe { GetLastError() };
            if code != ERROR_PIPE_CONNECTED {
                return Err(EngineError::Win32(code, "ConnectNamedPipe"));
            }
        }
        Ok(PipeSession {
            handle: self.listener.0,
            _server: PhantomData,
        })
    }
}

/// One client connection. Dropping it disconnects the instance so the next
/// client can be accepted without recreating the pipe.
pub struct PipeSession<'a> {
    handle: HANDLE,
    _server: PhantomData<&'a PipeServer>,
}

impl PipeSession<'_> {
    /// Reads and decodes one request; `None` means the client hung up.
    pub fn read_request(&self) -> Result<Option<HelperRequest>, EngineError> {
        match read_message(self.handle)? {
            Some(line) => Ok(HelperRequest::decode(&line)),
            None => Ok(None),
        }
    }

    /// Writes one reply.
    pub fn write_reply(&self, reply: &HelperReply) -> Result<(), EngineError> {
        write_message(self.handle, reply.encode().as_bytes())
    }
}

impl Drop for PipeSession<'_> {
    fn drop(&mut self) {
        // SAFETY: handle still belongs to the server's listener; disconnect
        // only affects this instance, which the server reuses on next accept.
        unsafe { DisconnectNamedPipe(self.handle) };
    }
}

/// Client-side connection used by the unelevated resident process.
pub struct PipeClient {
    handle: OwnedHandle,
}

impl PipeClient {
    /// Opens the current user's helper pipe. Fails fast when the helper is
    /// not running, so callers can degrade to their user-mode fallback.
    pub fn connect() -> Result<Self, EngineError> {
        let sid = current_user_sid()?;
        let path = common::pipe_path(&sid);
        let name: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: name is NUL-terminated; no security attributes are needed
        // for a client; the returned handle is owned by OwnedHandle.
        let handle = unsafe {
            CreateFileW(
                name.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                ptr::null(),
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                ptr::null_mut(),
            )
        };
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            return Err(EngineError::win32("CreateFileW"));
        }
        let mode = PIPE_READMODE_MESSAGE;
        // SAFETY: handle is a valid pipe client; the two optional state
        // parameters are null because only the read mode is being changed.
        if unsafe { SetNamedPipeHandleState(handle, &mode, ptr::null(), ptr::null()) } == 0 {
            return Err(EngineError::win32("SetNamedPipeHandleState"));
        }
        Ok(Self {
            handle: OwnedHandle(handle),
        })
    }

    /// Sends one request and waits for its reply.
    pub fn call(&self, request: &HelperRequest) -> Result<HelperReply, EngineError> {
        write_message(self.handle.0, request.encode().as_bytes())?;
        let line = read_message(self.handle.0)?
            .ok_or(EngineError::InvalidState("helper closed the pipe"))?;
        HelperReply::decode(&line).ok_or(EngineError::InvalidState("malformed helper reply"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sddl_is_per_user_and_dacl_protected() {
        let sid = "S-1-5-21-100-200-300-1001";
        let sddl = sddl_for(sid);
        assert!(sddl.starts_with("D:P"));
        assert!(sddl.ends_with(&format!("{sid})")));
        assert_eq!(sddl.matches("(A;;GA;;;").count(), 3);
        assert_ne!(sddl_for(sid), sddl_for("S-1-5-18"));
    }
}
