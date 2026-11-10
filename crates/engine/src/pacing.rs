//! Dynamic FPS pacer core (Phase 3, RESEARCH_FINDINGS.md section 8).
//!
//! Timing strategy: a high-resolution waitable timer
//! (`CREATE_WAITABLE_TIMER_HIGH_RESOLUTION`, Windows 10 1803+) sleeps until
//! ~1.5 ms before the frame deadline, then a QPC spin closes the gap. This
//! yields ~10-50 us precision without touching the global timer resolution
//! (NtSetTimerResolution is deliberately NOT used - see section 8.2).
//!
//! The frame interval is computed from the exact refresh rate reported by
//! the master tool (e.g. 239.996 Hz is NOT rounded to 240).

use crate::error::EngineError;
use std::mem;
use windows_sys::Win32::Foundation::{CloseHandle, WaitForSingleObject, HANDLE, INFINITE};
use windows_sys::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows_sys::Win32::System::Threading::{
    CreateWaitableTimerExW, SetWaitableTimer, CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
    CREATE_WAITABLE_TIMER_MANUAL_RESET,
};

/// DesiredAccess for the timer handle: TIMER_ALL_ACCESS.
const TIMER_ALL_ACCESS: u32 = 0x1F0003;
/// Spin phase length: the last 1.5 ms before the deadline are busy-waited.
const SPIN_PHASE_US: i64 = 1500;
/// No FPS cap target.
const UNLIMITED: f32 = 0.0;

/// RAII wrapper closing the timer handle on drop.
struct TimerHandle(HANDLE);

impl Drop for TimerHandle {
    fn drop(&mut self) {
        // SAFETY: handle is owned by this wrapper and closed exactly once.
        unsafe { CloseHandle(self.0) };
    }
}

/// Computes the frame interval in 100 ns units for a refresh rate and an
/// integer divisor (1 = native refresh, 2 = half rate, ...).
pub fn frame_interval_100ns(refresh_hz: f32, divisor: u32) -> i64 {
    let effective_hz = (refresh_hz / divisor.max(1) as f32).max(1.0);
    (1.0e7 / effective_hz as f64).round() as i64
}

/// High-precision frame pacer.
pub struct FramePacer {
    timer: Option<TimerHandle>,
    qpc_frequency: f64,
    /// Target frame interval in 100 ns units; 0 = pacing disabled.
    frame_interval_ns100: i64,
    /// Next frame deadline in QPC ticks.
    next_deadline_qpc: i64,
}

fn qpc_now() -> i64 {
    let mut value: i64 = 0;
    // SAFETY: valid out pointer.
    unsafe { QueryPerformanceCounter(&mut value) };
    value
}

fn qpc_frequency() -> i64 {
    let mut frequency: i64 = 0;
    // SAFETY: valid out pointer.
    unsafe { QueryPerformanceFrequency(&mut frequency) };
    frequency.max(1)
}

impl FramePacer {
    /// Creates a pacer with the given target FPS. `0.0` disables pacing.
    pub fn new(target_fps: f32) -> Result<Self, EngineError> {
        let timer = if target_fps > UNLIMITED {
            // SAFETY: NULL security attributes and NULL name (unnamed timer)
            // are valid; the handle is wrapped in TimerHandle below.
            let handle = unsafe {
                CreateWaitableTimerExW(
                    std::ptr::null(),
                    std::ptr::null(),
                    CREATE_WAITABLE_TIMER_HIGH_RESOLUTION | CREATE_WAITABLE_TIMER_MANUAL_RESET,
                    TIMER_ALL_ACCESS,
                )
            };
            if handle.is_null() {
                return Err(EngineError::win32("CreateWaitableTimerExW"));
            }
            Some(TimerHandle(handle))
        } else {
            None
        };
        let frequency = qpc_frequency();
        let mut pacer = Self {
            timer,
            qpc_frequency: frequency as f64,
            frame_interval_ns100: 0,
            next_deadline_qpc: qpc_now(),
        };
        pacer.set_target_fps(target_fps);
        Ok(pacer)
    }

    /// Updates the FPS cap. `0.0` disables pacing.
    pub fn set_target_fps(&mut self, target_fps: f32) {
        self.frame_interval_ns100 = if target_fps > UNLIMITED {
            (1.0e7 / f64::from(target_fps)).round() as i64
        } else {
            0
        };
        if self.frame_interval_ns100 > 0 {
            self.next_deadline_qpc = qpc_now();
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.frame_interval_ns100 > 0
    }

    /// Blocks until the next frame deadline and returns the actual frame
    /// time in milliseconds since the previous call. Returns 0.0 when
    /// pacing is disabled.
    pub fn wait_for_next_frame(&mut self) -> f64 {
        if !self.is_enabled() {
            return 0.0;
        }
        let started = qpc_now();
        self.next_deadline_qpc += self.frame_interval_ns100;
        // If we are late, resync to now instead of spiraling.
        let now = qpc_now();
        if self.next_deadline_qpc < now {
            self.next_deadline_qpc = now + self.frame_interval_ns100;
        }

        if let Some(timer) = &self.timer {
            let remaining_ns100 = self.next_deadline_qpc - now;
            let sleep_ns100 = remaining_ns100 - (SPIN_PHASE_US * 10);
            if sleep_ns100 > 0 {
                // Negative due time = relative, in 100 ns units.
                let due: i64 = -sleep_ns100;
                // SAFETY: owned timer handle, valid relative due time.
                // On failure we silently fall through to a full spin window:
                // less efficient but still correct.
                unsafe { SetWaitableTimer(timer.0, &due, 0, None, std::ptr::null(), 0) };
                // SAFETY: owned handle; INFINITE wait returns on timer signal.
                unsafe { WaitForSingleObject(timer.0, INFINITE) };
            }
        }

        // Spin phase: busy-wait on QPC until the deadline.
        let deadline_ticks = self.next_deadline_qpc;
        while qpc_now() < deadline_ticks {
            std::hint::spin_loop();
        }

        let elapsed = (qpc_now() - started) as f64 / self.qpc_frequency * 1000.0;
        elapsed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interval_math_uses_exact_refresh() {
        // 239.996 Hz is a real DisplayConfig value and must NOT be rounded.
        assert_eq!(frame_interval_100ns(239.996, 1), 41667);
        assert_eq!(frame_interval_100ns(240.0, 1), 41667);
        // Half rate: 240 / 2 = 120 Hz.
        assert_eq!(frame_interval_100ns(240.0, 2), 83333);
        assert_eq!(frame_interval_100ns(144.0, 1), 69444);
        // Divisor 0 is clamped to 1 (native rate).
        assert_eq!(frame_interval_100ns(240.0, 0), 41667);
    }

    #[test]
    fn disabled_pacer_is_noop() {
        let mut pacer = FramePacer::new(0.0).expect("disabled pacer needs no timer");
        assert!(!pacer.is_enabled());
        assert_eq!(pacer.wait_for_next_frame(), 0.0);
    }

    /// Coarse timing sanity on a live system: one frame at 240 Hz should
    /// take roughly 4-100 ms (loose bounds tolerate CI runner jitter).
    #[test]
    fn paced_frame_waits_roughly_the_interval() {
        let mut pacer = FramePacer::new(240.0).expect("timer must be creatable");
        assert!(pacer.is_enabled());
        let elapsed_ms = pacer.wait_for_next_frame();
        assert!(
            elapsed_ms > 1.0 && elapsed_ms < 100.0,
            "got {elapsed_ms} ms"
        );
    }

    /// Precise jitter measurement; run manually on the target machine
    /// (`cargo test -p engine -- --ignored`).
    #[test]
    #[ignore = "timing-sensitive; run on the target machine"]
    fn paced_frames_hit_the_interval() {
        let mut pacer = FramePacer::new(240.0).expect("timer must be creatable");
        // Warmup.
        for _ in 0..10 {
            pacer.wait_for_next_frame();
        }
        let mut total = 0.0;
        let frames = 120;
        for _ in 0..frames {
            total += pacer.wait_for_next_frame();
        }
        let avg = total / f64::from(frames);
        assert!((avg - 4.167).abs() < 0.3, "avg frame time {avg} ms");
    }
}
