//! Elevated helper process (`flux-fps.exe --helper`) and the client-side
//! proxies used by the unelevated resident app.
//!
//! Elevation is supplied by the Task Scheduler logon task (RunLevel Highest),
//! not by the executable manifest: the same binary must stay unelevated when
//! it runs the tray UI. Every request is idempotent, and all isolation state
//! is journalled so `--restore` can undo it after an unclean helper exit.

use crate::cpu_isolation;
use crate::error::EngineError;
use crate::helper_state;
use crate::memory_purge;
use crate::pipe::{self, PipeClient};
use common::{HelperReply, HelperRequest, PROTOCOL_VERSION};
use std::collections::BTreeSet;

/// Runs the helper's blocking accept loop until a Shutdown request arrives.
pub fn run() -> Result<(), EngineError> {
    if !pipe::is_elevated() {
        return Err(EngineError::InvalidState(
            "helper requires elevation; start it from the scheduled task",
        ));
    }
    let server = pipe::PipeServer::create()?;
    eprintln!("[flux-fps] helper listening (protocol v{PROTOCOL_VERSION})");

    // Reload the journal so a restarted helper still owns its earlier work.
    let mut isolated: BTreeSet<u32> = helper_state::load().into_iter().collect();

    loop {
        let session = match server.accept() {
            Ok(session) => session,
            Err(e) => {
                eprintln!("[flux-fps] helper accept failed: {e}");
                continue;
            }
        };
        let request = match session.read_request() {
            Ok(Some(request)) => request,
            Ok(None) => continue,
            Err(e) => {
                eprintln!("[flux-fps] helper request read failed: {e}");
                continue;
            }
        };
        let (reply, stop, changed) = dispatch(&request, &mut isolated);
        if changed {
            helper_state::save(isolated.iter().copied());
        }
        if let Err(e) = session.write_reply(&reply) {
            eprintln!("[flux-fps] helper reply failed: {e}");
        }
        drop(session);
        if stop {
            break;
        }
    }
    eprintln!("[flux-fps] helper stopped");
    Ok(())
}

/// Applies one request. Returns `(reply, stop_loop, journal_changed)`.
/// Persistence is left to the caller so unit tests never touch the disk.
fn dispatch(request: &HelperRequest, isolated: &mut BTreeSet<u32>) -> (HelperReply, bool, bool) {
    let (result, changed) = match request {
        HelperRequest::Ping => {
            return (
                HelperReply::Pong {
                    version: PROTOCOL_VERSION,
                    elevated: true,
                },
                false,
                false,
            );
        }
        HelperRequest::Shutdown => return (HelperReply::Ok, true, false),
        HelperRequest::PurgeStandbyList => (memory_purge::purge_low_priority_standby_list(), false),
        HelperRequest::ApplyIsolation { pid } => {
            if isolated.contains(pid) {
                (Ok(()), false) // already isolated; idempotent
            } else {
                match apply_isolation_fallback(*pid) {
                    Ok(()) => {
                        isolated.insert(*pid);
                        (Ok(()), true)
                    }
                    Err(e) => (Err(e), false),
                }
            }
        }
        HelperRequest::RemoveIsolation { pid } => {
            let was_isolated = isolated.remove(pid);
            let operation = if was_isolated {
                cpu_isolation::remove_background_isolation(*pid)
            } else {
                Ok(()) // nothing to undo; idempotent
            };
            (operation, was_isolated)
        }
        HelperRequest::RestoreAll => {
            let pids: Vec<u32> = isolated.iter().copied().collect();
            let was_dirty = !pids.is_empty();
            isolated.clear();
            (restore_pids(&pids), was_dirty)
        }
    };
    let reply = match result {
        Ok(()) => HelperReply::Ok,
        Err(e) => HelperReply::Error(e.to_string()),
    };
    (reply, false, changed)
}

/// CPU Set ids used for background isolation (E-cores, or all cores on a
/// non-hybrid machine). Computed in the helper so clients only send a pid.
fn background_ids() -> Result<Vec<u32>, EngineError> {
    let cores = cpu_isolation::enumerate_cpu_sets()?;
    let mut ids = cpu_isolation::e_core_ids(&cores);
    if ids.is_empty() {
        ids = cores.iter().map(|core| core.cpu_set_id).collect();
    }
    Ok(ids)
}

fn apply_isolation_fallback(pid: u32) -> Result<(), EngineError> {
    let ids = background_ids()?;
    cpu_isolation::apply_background_isolation(pid, &ids)
}

/// Removes isolation from a list of pids. A process that already exited is
/// not an error: the OS discarded its CPU Set assignment with the process.
fn restore_pids(pids: &[u32]) -> Result<(), EngineError> {
    for pid in pids {
        if let Err(e) = cpu_isolation::remove_background_isolation(*pid) {
            eprintln!("[flux-fps] restore skipped pid {pid}: {e}");
        }
    }
    Ok(())
}

/// Tries the elevated helper first; when it is not installed or not running,
/// falls back to the user-mode operation so the app still degrades cleanly.
fn call_privileged(
    request: HelperRequest,
    fallback: impl FnOnce() -> Result<(), EngineError>,
) -> Result<(), EngineError> {
    match PipeClient::connect() {
        Ok(client) => match client.call(&request) {
            Ok(HelperReply::Ok) => Ok(()),
            Ok(HelperReply::Error(message)) => Err(EngineError::Helper(message)),
            Ok(HelperReply::Pong { .. }) => {
                Err(EngineError::InvalidState("unexpected helper reply"))
            }
            Err(_) => fallback(),
        },
        Err(_) => fallback(),
    }
}

/// Purge the low-priority standby list through the helper when available.
pub fn purge_standby() -> Result<(), EngineError> {
    call_privileged(
        HelperRequest::PurgeStandbyList,
        memory_purge::purge_low_priority_standby_list,
    )
}

/// Apply background isolation to one process through the helper.
pub fn apply_isolation(pid: u32, ids: &[u32]) -> Result<(), EngineError> {
    call_privileged(HelperRequest::ApplyIsolation { pid }, || {
        cpu_isolation::apply_background_isolation(pid, ids)
    })
}

/// Remove background isolation from one process through the helper.
pub fn remove_isolation(pid: u32) -> Result<(), EngineError> {
    call_privileged(HelperRequest::RemoveIsolation { pid }, || {
        cpu_isolation::remove_background_isolation(pid)
    })
}

/// Full restore used by `--restore` and the MSI uninstall custom action.
/// Works with or without a live helper.
pub fn restore_all() -> Result<(), EngineError> {
    if let Ok(client) = PipeClient::connect() {
        if matches!(client.call(&HelperRequest::RestoreAll), Ok(HelperReply::Ok)) {
            return Ok(());
        }
    }
    let pids = helper_state::load();
    restore_pids(&pids)?;
    helper_state::clear();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ping_reports_version_and_elevation() {
        let mut isolated = BTreeSet::new();
        let (reply, stop, changed) = dispatch(&HelperRequest::Ping, &mut isolated);
        assert!(!stop);
        assert!(!changed);
        assert_eq!(
            reply,
            HelperReply::Pong {
                version: PROTOCOL_VERSION,
                elevated: true
            }
        );
    }

    #[test]
    fn shutdown_stops_the_loop_without_touching_state() {
        let mut isolated = BTreeSet::new();
        isolated.insert(77);
        let (reply, stop, changed) = dispatch(&HelperRequest::Shutdown, &mut isolated);
        assert_eq!(reply, HelperReply::Ok);
        assert!(stop);
        assert!(!changed);
        assert_eq!(isolated.len(), 1);
    }

    #[test]
    fn removing_an_unknown_pid_is_idempotent() {
        let mut isolated = BTreeSet::new();
        let (reply, stop, changed) =
            dispatch(&HelperRequest::RemoveIsolation { pid: 4242 }, &mut isolated);
        assert_eq!(reply, HelperReply::Ok);
        assert!(!stop);
        assert!(!changed);
        assert!(isolated.is_empty());
    }

    #[test]
    fn restore_on_an_empty_journal_is_a_no_op() {
        let mut isolated = BTreeSet::new();
        let (reply, stop, changed) = dispatch(&HelperRequest::RestoreAll, &mut isolated);
        assert_eq!(reply, HelperReply::Ok);
        assert!(!stop);
        assert!(!changed);
        assert!(isolated.is_empty());
    }
}
