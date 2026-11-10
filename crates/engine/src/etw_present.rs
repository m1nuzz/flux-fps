//! Frametime source: real-time ETW `Microsoft-Windows-DxgKrnl` Present events
//! (Phase 3.3, RESEARCH_FINDINGS.md section 8.3).
//!
//! This is a *separate* session from the NT Kernel Logger DPC monitor, so both
//! can run side by side. DxgKrnl is a manifest provider: we enable it with
//! `EnableTraceEx2` using the Present keyword, then consume event id 67. Every
//! Present yields one timestamp; the delta between consecutive timestamps is
//! the frametime. No game hooks, no injection, no process handles opened.
//!
//! Layout mirrors `etw_dpc`: one OS thread owns the trace session
//! (`ProcessTrace` blocks), a second thread aggregates a 5 s sliding window and
//! publishes `common::FrametimeStats` through a `tokio::sync::watch` channel.
//! Requires elevation; the runtime treats a failure as best-effort.

use crate::error::EngineError;
use common::FrametimeStats;
use std::collections::VecDeque;
use std::mem;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;
use tokio::sync::watch;
use windows_sys::core::GUID;
use windows_sys::Win32::Foundation::ERROR_ALREADY_EXISTS;
use windows_sys::Win32::System::Diagnostics::Etw::{
    CloseTrace, ControlTraceW, EnableTraceEx2, OpenTraceW, ProcessTrace, StartTraceW,
    CONTROLTRACE_HANDLE, EVENT_CONTROL_CODE_ENABLE_PROVIDER, EVENT_RECORD,
    EVENT_TRACE_CONTROL_STOP, EVENT_TRACE_LOGFILEW, EVENT_TRACE_PROPERTIES,
    EVENT_TRACE_REAL_TIME_MODE, PROCESSTRACE_HANDLE, PROCESS_TRACE_MODE_EVENT_RECORD,
    PROCESS_TRACE_MODE_REAL_TIME, TRACE_LEVEL_INFORMATION, WNODE_FLAG_TRACED_GUID,
};
use windows_sys::Win32::System::Performance::QueryPerformanceFrequency;

/// Microsoft-Windows-DxgKrnl provider GUID.
const DXGKRNL_GUID: GUID = GUID::from_u128(0x802ec45a_1e99_4b83_9966_90a1ed4092e4);
/// Arbitrary but stable GUID identifying our private logger session.
const SESSION_GUID: GUID = GUID::from_u128(0x9c3f8e17_2ab4_4d65_b0c1_7e5a3f18d902);
const SESSION_NAME: &str = "flux-fps-present";
/// DxgKrnl "Present" event id (manifest-based provider).
const PRESENT_EVENT_ID: u16 = 67;
/// DxgKrnl "Present" keyword.
const PRESENT_KEYWORD: u64 = 0x0000_0000_0000_0010;
/// Sliding window length, seconds (RF 8.3).
const WINDOW_SECS: f32 = 5.0;
const PUBLISH_INTERVAL_MS: u64 = 200;
/// Deltas above this are stream discontinuities (alt-tab, display off), not
/// frametimes, and must not poison p99.
const MAX_FRAMETIME_US: f64 = 1_000_000.0;
/// Upper bound on the ring (5 s at 240 fps is 1200 samples).
const RING_CAPACITY: usize = 4096;
/// QPC clock client context for the WNODE header.
const WNODE_CLIENT_CONTEXT_QPC: u32 = 1;

struct State {
    last_qpc: Option<i64>,
    /// (QPC timestamp, frametime in microseconds).
    ring: VecDeque<(i64, f32)>,
}

struct Shared {
    state: Mutex<State>,
    /// Newest event timestamp, used to compute the window cutoff.
    latest_qpc: AtomicI64,
    window_ticks: i64,
    qpf: f64,
    /// Optional process filter (0 = all presenting processes).
    target_pid: AtomicU32,
}

impl Shared {
    fn new(window_secs: f32, qpf: f64) -> Self {
        Self {
            state: Mutex::new(State {
                last_qpc: None,
                ring: VecDeque::with_capacity(256),
            }),
            latest_qpc: AtomicI64::new(0),
            window_ticks: (qpf * f64::from(window_secs)) as i64,
            qpf,
            target_pid: AtomicU32::new(0),
        }
    }

    fn push(&self, ts: i64, pid: u32) {
        let filter = self.target_pid.load(Ordering::Relaxed);
        if filter != 0 && pid != filter {
            return;
        }
        self.latest_qpc.fetch_max(ts, Ordering::Relaxed);
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let Some(previous) = state.last_qpc else {
            state.last_qpc = Some(ts);
            return;
        };
        state.last_qpc = Some(ts);
        let delta_us = (ts - previous).max(0) as f64 * 1_000_000.0 / self.qpf;
        if delta_us <= 0.0 || delta_us >= MAX_FRAMETIME_US {
            return;
        }
        state.ring.push_back((ts, delta_us as f32));
        while state.ring.len() > RING_CAPACITY {
            state.ring.pop_front();
        }
    }

    fn stats(&self) -> FrametimeStats {
        let cutoff = self.latest_qpc.load(Ordering::Relaxed) - self.window_ticks;
        let Ok(mut state) = self.state.lock() else {
            return FrametimeStats::default();
        };
        while matches!(state.ring.front(), Some(&(ts, _)) if ts < cutoff) {
            state.ring.pop_front();
        }
        let span_ticks = match (state.ring.front(), state.ring.back()) {
            (Some(&(first, _)), Some(&(last, _))) => last - first,
            _ => 0,
        };
        let samples: Vec<f32> = state.ring.iter().map(|&(_, ft)| ft).collect();
        // Use the observed span once it is long enough to be meaningful; early
        // in a session fall back to the nominal window length.
        let span_secs = span_ticks.max(0) as f64 / self.qpf;
        let presents_per_sec = if span_secs >= 0.25 {
            (samples.len() as f64 / span_secs) as f32
        } else {
            samples.len() as f32 / WINDOW_SECS
        };
        aggregate(&samples, presents_per_sec)
    }
}

/// Pure aggregation so the heuristic can be unit-tested without ETW.
fn aggregate(frametimes_us: &[f32], presents_per_sec: f32) -> FrametimeStats {
    if frametimes_us.is_empty() {
        return FrametimeStats {
            presents_per_sec,
            ..FrametimeStats::default()
        };
    }
    let mut sorted = frametimes_us.to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let avg_us = sorted.iter().sum::<f32>() / sorted.len() as f32;
    // Index of the 99th percentile (the worst 1% of frames).
    let p99_index = ((sorted.len() as f32 * 0.99).ceil() as usize)
        .saturating_sub(1)
        .min(sorted.len() - 1);
    FrametimeStats {
        avg_ms: Some(avg_us / 1000.0),
        p99_ms: Some(sorted[p99_index] / 1000.0),
        presents_per_sec,
        samples: sorted.len(),
    }
}

/// Handle for a running Present monitor. Dropping it stops the session and
/// joins the worker threads.
pub struct PresentMonitor {
    stop: Arc<AtomicBool>,
    shared: Arc<Shared>,
    consumer: Option<JoinHandle<()>>,
    publisher: Option<JoinHandle<()>>,
}

impl PresentMonitor {
    /// Restricts frametimes to one presenting process (0 = all).
    pub fn set_target_pid(&self, pid: u32) {
        self.shared.target_pid.store(pid, Ordering::Relaxed);
    }

    /// Stops the ETW session and joins the worker threads.
    pub fn stop(&mut self) {
        if self.consumer.is_none() && self.publisher.is_none() {
            return;
        }
        self.stop.store(true, Ordering::SeqCst);
        // Asking the controller to stop makes ProcessTrace return. A stale or
        // already-stopped session simply yields an error we can ignore.
        let mut properties = build_properties_buffer();
        let name = to_wide(SESSION_NAME);
        unsafe {
            ControlTraceW(
                CONTROLTRACE_HANDLE { Value: 0 },
                name.as_ptr(),
                properties.as_mut_ptr() as *mut EVENT_TRACE_PROPERTIES,
                EVENT_TRACE_CONTROL_STOP,
            );
        }
        if let Some(handle) = self.consumer.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.publisher.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for PresentMonitor {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Starts the Present session. Best-effort: the caller keeps the returned
/// monitor alive for as long as frametimes are needed.
pub fn start() -> Result<(PresentMonitor, watch::Receiver<FrametimeStats>), EngineError> {
    let qpf = query_qpf()?;
    let stop = Arc::new(AtomicBool::new(false));
    let shared = Arc::new(Shared::new(WINDOW_SECS, qpf));
    let (tx, rx) = watch::channel(FrametimeStats::default());

    // The consumer owns the session. It reports successful setup before
    // blocking in ProcessTrace, so `start` returns while tracing is live.
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), EngineError>>();
    let consumer_stop = Arc::clone(&stop);
    let consumer_shared = Arc::clone(&shared);
    let consumer = std::thread::Builder::new()
        .name("flux-fps-present".into())
        .spawn(move || {
            if let Err(e) = run_session(&consumer_shared, &ready_tx) {
                let _ = ready_tx.send(Err(e));
            }
            consumer_stop.store(true, Ordering::SeqCst);
        })
        .map_err(|_| EngineError::InvalidState("failed to spawn Present consumer thread"))?;

    match ready_rx.recv() {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            let _ = consumer.join();
            return Err(e);
        }
        Err(_) => {
            let _ = consumer.join();
            return Err(EngineError::InvalidState("Present consumer thread died"));
        }
    }

    let publisher_stop = Arc::clone(&stop);
    let publisher_shared = Arc::clone(&shared);
    let publisher = std::thread::Builder::new()
        .name("flux-fps-present-pub".into())
        .spawn(move || {
            while !publisher_stop.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(PUBLISH_INTERVAL_MS));
                let _ = tx.send(publisher_shared.stats());
            }
            let _ = tx.send(publisher_shared.stats());
        })
        .map_err(|_| EngineError::InvalidState("failed to spawn Present publisher thread"))?;

    Ok((
        PresentMonitor {
            stop,
            shared,
            consumer: Some(consumer),
            publisher: Some(publisher),
        },
        rx,
    ))
}

fn query_qpf() -> Result<f64, EngineError> {
    let mut frequency: i64 = 0;
    // SAFETY: valid out pointer to a stack i64.
    unsafe { QueryPerformanceFrequency(&mut frequency) };
    if frequency <= 0 {
        return Err(EngineError::InvalidState(
            "QueryPerformanceFrequency failed",
        ));
    }
    Ok(frequency as f64)
}

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Zero-initialized properties buffer with the session name appended at
/// `LoggerNameOffset`, as required by the ETW controller ABI.
fn build_properties_buffer() -> Vec<u8> {
    let props_size = mem::size_of::<EVENT_TRACE_PROPERTIES>();
    let name_wide = to_wide(SESSION_NAME);
    let name_bytes: Vec<u8> = name_wide.iter().flat_map(|w| w.to_le_bytes()).collect();
    let mut buffer = vec![0u8; props_size + name_bytes.len()];
    buffer[props_size..].copy_from_slice(&name_bytes);

    // SAFETY: the buffer is zeroed, larger than EVENT_TRACE_PROPERTIES, and
    // allocated with suitable alignment; every field written below is public.
    let props = unsafe { &mut *(buffer.as_mut_ptr() as *mut EVENT_TRACE_PROPERTIES) };
    props.Wnode.BufferSize = buffer.len() as u32;
    props.Wnode.Flags = WNODE_FLAG_TRACED_GUID;
    props.Wnode.Guid = SESSION_GUID;
    props.Wnode.ClientContext = WNODE_CLIENT_CONTEXT_QPC;
    props.BufferSize = 64; // KB per buffer
    props.MinimumBuffers = 8;
    props.MaximumBuffers = 16;
    props.LogFileMode = EVENT_TRACE_REAL_TIME_MODE;
    props.FlushTimer = 1;
    props.LogFileNameOffset = 0; // real-time only, no log file
    props.LoggerNameOffset = props_size as u32;
    buffer
}

fn run_session(
    shared: &Arc<Shared>,
    ready: &std::sync::mpsc::Sender<Result<(), EngineError>>,
) -> Result<(), EngineError> {
    let mut properties = build_properties_buffer();
    let name_wide = to_wide(SESSION_NAME);
    let props = properties.as_mut_ptr() as *mut EVENT_TRACE_PROPERTIES;
    let mut control_handle = CONTROLTRACE_HANDLE { Value: 0 };

    // SAFETY: properties buffer matches the controller ABI; name is a
    // NUL-terminated wide string that outlives the call.
    let mut status = unsafe { StartTraceW(&mut control_handle, name_wide.as_ptr(), props) };
    if status == ERROR_ALREADY_EXISTS {
        // A crash may have left our session running; stop it and retry once.
        // Only our own named session is touched, never another tool's trace.
        unsafe {
            ControlTraceW(
                CONTROLTRACE_HANDLE { Value: 0 },
                name_wide.as_ptr(),
                props,
                EVENT_TRACE_CONTROL_STOP,
            );
        }
        status = unsafe { StartTraceW(&mut control_handle, name_wide.as_ptr(), props) };
    }
    if status != 0 {
        return Err(EngineError::Win32(status, "StartTraceW(flux-fps-present)"));
    }

    // SAFETY: control_handle is valid after a successful StartTraceW; the
    // provider GUID and keyword are compile-time constants.
    let enable_status = unsafe {
        EnableTraceEx2(
            control_handle,
            &DXGKRNL_GUID,
            EVENT_CONTROL_CODE_ENABLE_PROVIDER,
            TRACE_LEVEL_INFORMATION as u8,
            PRESENT_KEYWORD,
            0,
            0,
            std::ptr::null(),
        )
    };
    if enable_status != 0 {
        unsafe {
            ControlTraceW(
                CONTROLTRACE_HANDLE { Value: 0 },
                name_wide.as_ptr(),
                props,
                EVENT_TRACE_CONTROL_STOP,
            );
        }
        return Err(EngineError::Win32(enable_status, "EnableTraceEx2(DxgKrnl)"));
    }

    let context_ptr = Arc::as_ptr(shared) as *mut core::ffi::c_void;
    let mut logfile: EVENT_TRACE_LOGFILEW = unsafe { mem::zeroed() };
    logfile.LoggerName = name_wide.as_ptr() as *mut u16;
    logfile.Anonymous1.ProcessTraceMode =
        PROCESS_TRACE_MODE_REAL_TIME | PROCESS_TRACE_MODE_EVENT_RECORD;
    logfile.Anonymous2.EventRecordCallback = Some(on_present);
    logfile.Context = context_ptr;

    // SAFETY: logfile is zero-initialized with only valid pointers set.
    let trace: PROCESSTRACE_HANDLE = unsafe { OpenTraceW(&mut logfile) };
    if trace.Value == u64::MAX {
        unsafe {
            ControlTraceW(
                CONTROLTRACE_HANDLE { Value: 0 },
                name_wide.as_ptr(),
                props,
                EVENT_TRACE_CONTROL_STOP,
            );
        }
        return Err(EngineError::InvalidState(
            "OpenTraceW returned INVALID_PROCESSTRACE_HANDLE",
        ));
    }

    // The session is live: unblock `start()` before entering the blocking loop.
    let _ = ready.send(Ok(()));

    // SAFETY: single valid handle; blocks until the session is stopped.
    unsafe { ProcessTrace(&trace, 1, std::ptr::null(), std::ptr::null()) };
    // SAFETY: handle returned by OpenTraceW is closed exactly once here.
    unsafe { CloseTrace(trace) };
    // Ensure no orphaned session survives if ProcessTrace exited on its own.
    unsafe {
        ControlTraceW(
            CONTROLTRACE_HANDLE { Value: 0 },
            name_wide.as_ptr(),
            props,
            EVENT_TRACE_CONTROL_STOP,
        );
    }
    Ok(())
}

/// ETW event callback (runs on the consumer thread inside ProcessTrace).
///
/// SAFETY: `record` is a valid EVENT_RECORD pointer supplied by ETW for the
/// duration of the call; UserContext points at the Arc<Shared> allocation the
/// consumer thread keeps alive for the whole session.
unsafe extern "system" fn on_present(record: *mut EVENT_RECORD) {
    if record.is_null() {
        return;
    }
    let record = &*record;
    if record.EventHeader.EventDescriptor.Id != PRESENT_EVENT_ID {
        return;
    }
    let guid = &record.EventHeader.ProviderId;
    if guid.data1 != DXGKRNL_GUID.data1
        || guid.data2 != DXGKRNL_GUID.data2
        || guid.data3 != DXGKRNL_GUID.data3
        || guid.data4 != DXGKRNL_GUID.data4
    {
        return;
    }
    if let Some(shared) = record.UserContext.cast::<Shared>().as_ref() {
        shared.push(record.EventHeader.TimeStamp, record.EventHeader.ProcessId);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregate_reports_avg_and_p99() {
        // 100 samples: 1 ms, 2 ms, ... 100 ms.
        let frametimes: Vec<f32> = (1..=100).map(|ms| ms as f32 * 1000.0).collect();
        let stats = aggregate(&frametimes, 100.0);
        assert_eq!(stats.samples, 100);
        assert!((stats.avg_ms.unwrap() - 50.5).abs() < 0.01);
        assert!((stats.p99_ms.unwrap() - 99.0).abs() < 0.01);
        assert_eq!(stats.presents_per_sec, 100.0);
    }

    #[test]
    fn aggregate_handles_empty_window() {
        let stats = aggregate(&[], 0.0);
        assert_eq!(stats, FrametimeStats::default());
    }

    #[test]
    fn single_sample_p99_is_the_sample_itself() {
        let stats = aggregate(&[16_667.0], 60.0);
        assert_eq!(stats.samples, 1);
        assert!((stats.p99_ms.unwrap() - 16.667).abs() < 0.001);
    }

    #[test]
    fn stale_samples_are_pruned_from_the_window() {
        let shared = Shared::new(WINDOW_SECS, 1_000_000.0);
        {
            let mut state = shared.state.lock().expect("state lock");
            state.ring.push_back((1_000_000, 8_000.0)); // older than the window
            state.ring.push_back((6_000_000, 4_000.0)); // inside the window
        }
        shared.latest_qpc.store(10_000_000, Ordering::Relaxed);
        let stats = shared.stats();
        assert_eq!(stats.samples, 1);
        assert!((stats.avg_ms.unwrap() - 4.0).abs() < 0.001);
    }

    #[test]
    fn stream_discontinuities_are_rejected() {
        let shared = Shared::new(WINDOW_SECS, 1_000_000.0);
        // 0.01 s -> 10 ms frametime, then a 10 s gap (alt-tab). The gap must
        // not be recorded as a 10-second frame (it would wreck p99 forever).
        shared.push(1_000_000, 0);
        shared.push(1_010_000, 0);
        shared.push(11_010_000, 0);
        let state = shared.state.lock().expect("state lock");
        assert_eq!(state.ring.len(), 1);
        assert!(state.ring.back().expect("sample").1 < 20_000.0);
    }

    #[test]
    fn target_pid_filter_drops_other_processes() {
        let shared = Shared::new(WINDOW_SECS, 1_000_000.0);
        shared.target_pid.store(42, Ordering::Relaxed);
        shared.push(1_000_000, 7);
        shared.push(1_010_000, 7);
        assert_eq!(shared.stats().samples, 0);
        shared.push(1_020_000, 42);
        shared.push(1_030_000, 42);
        assert_eq!(shared.stats().samples, 1);
    }

    #[test]
    fn properties_buffer_carries_the_session_name() {
        let buffer = build_properties_buffer();
        let offset = mem::size_of::<EVENT_TRACE_PROPERTIES>();
        // SAFETY: read back the public header fields we just wrote.
        let props = unsafe { &*(buffer.as_ptr() as *const EVENT_TRACE_PROPERTIES) };
        assert_eq!(props.LoggerNameOffset as usize, offset);
        assert_eq!(props.Wnode.BufferSize as usize, buffer.len());
        let name: Vec<u16> = (0..SESSION_NAME.len())
            .map(|i| {
                let at = offset + i * 2;
                u16::from_le_bytes([buffer[at], buffer[at + 1]])
            })
            .collect();
        assert_eq!(String::from_utf16_lossy(&name), SESSION_NAME);
    }
}
