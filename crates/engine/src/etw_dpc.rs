//! DPC/ISR latency monitoring via the NT Kernel Logger session.
//!
//! Architecture (concept docs, RESEARCH_FINDINGS.md section 4.4):
//! - one dedicated OS thread owns the trace session (`ProcessTrace` blocks);
//! - the session enables only `EVENT_TRACE_FLAG_DPC | EVENT_TRACE_FLAG_INTERRUPT`
//!   to keep CPU overhead minimal;
//! - PerfInfoGuid events with Opcode 27 (DPC) / 28 (ISR) carry `InitialTime`
//!   in their payload; duration = Timestamp - InitialTime (100 ns units);
//! - a per-core sliding window of max DPC/ISR duration is published through a
//!   `tokio::sync::watch` channel (latest value only), so the UI never has to
//!   drain bursty callbacks.
//!
//! Requires an elevated process (SeSystemProfilePrivilege is granted to
//! administrators for the kernel logger). The session name is the canonical
//! "NT Kernel Logger": starting fails with ERROR_ALREADY_EXISTS if another
//! tool (xperf, WPR, LatencyMon) owns it - we report that instead of stealing
//! the session.

use crate::error::EngineError;
use std::collections::VecDeque;
use std::mem;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;
use tokio::sync::watch;
use windows_sys::Win32::Foundation::ERROR_ALREADY_EXISTS;
use windows_sys::Win32::System::Diagnostics::Etw::{
    CloseTrace, ControlTraceW, OpenTraceW, ProcessTrace, StartTraceW, EVENT_RECORD,
    EVENT_TRACE_CONTROL_STOP, EVENT_TRACE_FLAG_DPC, EVENT_TRACE_FLAG_INTERRUPT,
    EVENT_TRACE_LOGFILEW, EVENT_TRACE_PROPERTIES, EVENT_TRACE_REAL_TIME_MODE, PerfInfoGuid,
    SystemTraceControlGuid, CONTROLTRACE_HANDLE, PROCESSTRACE_HANDLE, WNODE_FLAG_TRACED_GUID,
};

/// Classic event type 27 = DPC completion.
const EVENT_TYPE_DPC: u8 = 27;
/// Classic event type 28 = ISR completion.
const EVENT_TYPE_ISR: u8 = 28;

const KERNEL_LOGGER_NAME: &str = "NT Kernel Logger";
/// QPC clock client context for the WNODE header.
const WNODE_CLIENT_CONTEXT_QPC: u32 = 1;
/// Maximum samples kept per core ring (enough for a 2 s window by far).
const RING_CAPACITY: usize = 8192;
/// Maximum tracked cores; events above this index are dropped.
const MAX_CORES: usize = 256;

/// Configuration for the DPC/ISR monitor.
#[derive(Debug, Clone)]
pub struct DpcMonitorConfig {
    /// Sliding window length in seconds (default 2 s).
    pub window_secs: f32,
    /// Snapshot publish interval in milliseconds (default 200 ms; the UI
    /// pacer throttles further to 30 Hz).
    pub publish_interval_ms: u64,
}

impl Default for DpcMonitorConfig {
    fn default() -> Self {
        Self { window_secs: 2.0, publish_interval_ms: 200 }
    }
}

struct Shared {
    rings: Mutex<Vec<VecDeque<(i64, f32)>>>,
    latest_timestamp_ns100: AtomicI64,
    window_ns100: i64,
}

impl Shared {
    fn new(window_secs: f32) -> Self {
        Self {
            rings: Mutex::new(vec![VecDeque::with_capacity(64); MAX_CORES]),
            latest_timestamp_ns100: AtomicI64::new(0),
            window_ns100: (window_secs * 1.0e7) as i64,
        }
    }

    fn push(&self, core: usize, timestamp: i64, duration_us: f32) {
        if core >= MAX_CORES {
            return;
        }
        self.latest_timestamp_ns100.fetch_max(timestamp, Ordering::Relaxed);
        if let Ok(mut rings) = self.rings.lock() {
            let ring = &mut rings[core];
            ring.push_back((timestamp, duration_us));
            while ring.len() > RING_CAPACITY {
                ring.pop_front();
            }
        }
    }

    /// Drains samples older than the window and returns the max duration
    /// (microseconds) per core.
    fn snapshot(&self) -> Vec<f32> {
        let cutoff = self.latest_timestamp_ns100.load(Ordering::Relaxed) - self.window_ns100;
        let mut rings = match self.rings.lock() {
            Ok(r) => r,
            Err(_) => return Vec::new(),
        };
        rings
            .iter_mut()
            .map(|ring| {
                while matches!(ring.front(), Some(&(ts, _)) if ts < cutoff) {
                    ring.pop_front();
                }
                ring.iter().map(|&(_, d)| d).fold(0.0f32, f32::max)
            })
            .collect()
    }
}

/// Handle for a running DPC/ISR monitor. Dropping it stops the session and
/// joins the worker threads.
pub struct DpcMonitor {
    stop: Arc<AtomicBool>,
    shared: Arc<Shared>,
    consumer: Option<JoinHandle<()>>,
    publisher: Option<JoinHandle<()>>,
}

impl DpcMonitor {
    /// Stops the ETW session and joins the worker threads.
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Asking the controller to stop makes the blocking ProcessTrace call
        // return. ERROR_NOT_FOUND just means the session was already gone.
        if let Ok(mut buffer) = build_properties_buffer() {
            unsafe {
                ControlTraceW(
                    CONTROLTRACE_HANDLE { Value: 0 },
                    to_wide(KERNEL_LOGGER_NAME).as_ptr(),
                    buffer.as_mut_ptr() as *mut EVENT_TRACE_PROPERTIES,
                    EVENT_TRACE_CONTROL_STOP,
                );
            }
        }
        if let Some(handle) = self.consumer.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.publisher.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for DpcMonitor {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Starts the kernel logger session and spawns the consumer/publisher pair.
///
/// Returns the monitor handle and a watch receiver yielding the per-core
/// vector of maximum DPC/ISR durations (microseconds) inside the window.
pub fn start(
    config: DpcMonitorConfig,
) -> Result<(DpcMonitor, watch::Receiver<Vec<f32>>), EngineError> {
    let stop = Arc::new(AtomicBool::new(false));
    let shared = Arc::new(Shared::new(config.window_secs));
    let (tx, rx) = watch::channel(Vec::new());

    // The consumer thread owns the session lifecycle; start errors are sent
    // back synchronously so `start` fails fast.
    let (result_tx, result_rx) = std::sync::mpsc::channel::<Result<(), EngineError>>();
    let consumer_stop = Arc::clone(&stop);
    let consumer_shared = Arc::clone(&shared);

    let consumer = std::thread::Builder::new()
        .name("flux-fps-etw".into())
        .spawn(move || {
            // Keep the context alive for the whole session: the ETW callback
            // dereferences this allocation through record.UserContext.
            let session = match run_session(&consumer_shared, &consumer_stop) {
                Ok(()) => {
                    let _ = result_tx.send(Ok(()));
                    // ProcessTrace returned (stop requested).
                    consumer_stop.store(true, Ordering::SeqCst);
                    return;
                }
                Err(e) => e,
            };
            let _ = result_tx.send(Err(session));
            consumer_stop.store(true, Ordering::SeqCst);
        })
        .map_err(|_| EngineError::InvalidState("failed to spawn consumer thread"))?;

    match result_rx.recv() {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            let _ = consumer.join();
            return Err(e);
        }
        Err(_) => {
            let _ = consumer.join();
            return Err(EngineError::InvalidState("consumer thread died"));
        }
    }

    let publisher_stop = Arc::clone(&stop);
    let publisher_shared = Arc::clone(&shared);
    let interval = Duration::from_millis(config.publish_interval_ms);
    let publisher = std::thread::Builder::new()
        .name("flux-fps-etw-pub".into())
        .spawn(move || {
            while !publisher_stop.load(Ordering::SeqCst) {
                std::thread::sleep(interval);
                let _ = tx.send(publisher_shared.snapshot());
            }
            // Final snapshot after the consumer drained its last events.
            let _ = tx.send(publisher_shared.snapshot());
        })
        .map_err(|_| EngineError::InvalidState("failed to spawn publisher thread"))?;

    Ok((
        DpcMonitor { stop, shared, consumer: Some(consumer), publisher: Some(publisher) },
        rx,
    ))
}

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Allocates the zero-initialized properties buffer with the session name
/// appended at `LoggerNameOffset`, as required by the ETW controller ABI.
fn build_properties_buffer() -> Result<Vec<u8>, EngineError> {
    let props_size = mem::size_of::<EVENT_TRACE_PROPERTIES>();
    let name_size = (KERNEL_LOGGER_NAME.len() + 1) * mem::size_of::<u16>();
    let mut buffer = vec![0u8; props_size + name_size];

    let name_wide = to_wide(KERNEL_LOGGER_NAME);
    let name_bytes: Vec<u8> = name_wide.iter().flat_map(|w| w.to_le_bytes()).collect();
    buffer[props_size..props_size + name_bytes.len()].copy_from_slice(&name_bytes);

    let props = unsafe { &mut *(buffer.as_mut_ptr() as *mut EVENT_TRACE_PROPERTIES) };
    props.Wnode.BufferSize = buffer.len() as u32;
    props.Wnode.Flags = WNODE_FLAG_TRACED_GUID;
    props.Wnode.Guid = SystemTraceControlGuid;
    props.Wnode.ClientContext = WNODE_CLIENT_CONTEXT_QPC;
    props.BufferSize = 64; // KB per trace buffer
    props.MinimumBuffers = 64;
    props.MaximumBuffers = 128;
    props.LogFileMode = EVENT_TRACE_REAL_TIME_MODE;
    props.FlushTimer = 1;
    props.EnableFlags = EVENT_TRACE_FLAG_DPC | EVENT_TRACE_FLAG_INTERRUPT;
    props.LogFileNameOffset = 0; // real-time only, no file
    props.LoggerNameOffset = props_size as u32;
    Ok(buffer)
}

fn run_session(shared: &Arc<Shared>, stop: &AtomicBool) -> Result<(), EngineError> {
    let mut properties = build_properties_buffer()?;
    let name_wide = to_wide(KERNEL_LOGGER_NAME);

    let mut control_handle = CONTROLTRACE_HANDLE { Value: 0 };
    // SAFETY: properties buffer is correctly laid out per the controller ABI;
    // the name pointer is NUL-terminated wide string.
    let status = unsafe {
        StartTraceW(
            &mut control_handle,
            name_wide.as_ptr(),
            properties.as_mut_ptr() as *mut EVENT_TRACE_PROPERTIES,
        )
    };
    if status == ERROR_ALREADY_EXISTS {
        return Err(EngineError::InvalidState(
            "NT Kernel Logger session is already active (another tracing tool owns it)",
        ));
    }
    if status != 0 {
        return Err(EngineError::Win32(status, "StartTraceW"));
    }

    // Context: raw pointer handed to the ETW callback via record.UserContext.
    let context_ptr = Arc::as_ptr(shared) as *mut core::ffi::c_void;
    let mut logfile: EVENT_TRACE_LOGFILEW = unsafe { mem::zeroed() };
    logfile.LoggerName = name_wide.as_ptr() as *mut u16;
    logfile.Anonymous1.ProcessTraceMode = EVENT_TRACE_REAL_TIME_MODE;
    logfile.Anonymous2.EventRecordCallback = Some(on_event);
    logfile.Context = context_ptr;

    // SAFETY: logfile is zero-initialized with only valid pointers set.
    let trace: PROCESSTRACE_HANDLE = unsafe { OpenTraceW(&mut logfile) };
    if trace.Value == u64::MAX {
        // INVALID_PROCESSTRACE_HANDLE: stop the session we just started.
        unsafe {
            ControlTraceW(
                CONTROLTRACE_HANDLE { Value: 0 },
                name_wide.as_ptr(),
                properties.as_mut_ptr() as *mut EVENT_TRACE_PROPERTIES,
                EVENT_TRACE_CONTROL_STOP,
            );
        }
        return Err(EngineError::win32("OpenTraceW"));
    }

    // SAFETY: single valid handle; blocking call until the session is stopped.
    let _process_status = unsafe { ProcessTrace(&trace, 1, std::ptr::null(), std::ptr::null()) };

    // SAFETY: handle returned by OpenTraceW is closed exactly once here.
    unsafe { CloseTrace(trace) };

    // Ensure the session does not survive if ProcessTrace exited on its own.
    unsafe {
        ControlTraceW(
            CONTROLTRACE_HANDLE { Value: 0 },
            name_wide.as_ptr(),
            properties.as_mut_ptr() as *mut EVENT_TRACE_PROPERTIES,
            EVENT_TRACE_CONTROL_STOP,
        );
    }

    let _ = stop; // stop flag is flipped by the caller after we return
    Ok(())
}

/// ETW event callback (runs on the consumer thread inside ProcessTrace).
///
/// SAFETY: `record` is a valid EVENT_RECORD pointer provided by ETW for the
/// duration of the call; UserContext points at the Arc<Shared> allocation
/// that the consumer thread keeps alive for the whole session.
unsafe extern "system" fn on_event(record: *mut EVENT_RECORD) {
    if record.is_null() {
        return;
    }
    let record = &*record;
    if record.EventHeader.ProviderId != PerfInfoGuid {
        return;
    }
    let opcode = record.EventHeader.EventDescriptor.Opcode;
    if opcode != EVENT_TYPE_DPC && opcode != EVENT_TYPE_ISR {
        return;
    }
    if record.UserData.is_null() || record.UserDataLength < 8 {
        return;
    }
    // Classic DPC/ISR payload starts with InitialTime (i64, 100 ns units).
    let initial_time = (record.UserData as *const i64).read_unaligned();
    let duration_us = ((record.EventHeader.TimeStamp - initial_time) / 10).max(0) as f32;
    // SAFETY: reading the single union variant carrying the processor index.
    let core = unsafe { record.BufferContext.Anonymous.ProcessorIndex } as usize;

    if let Some(shared) = record.UserContext.cast::<Shared>().as_ref() {
        shared.push(core, record.EventHeader.TimeStamp, duration_us);
    }
}
