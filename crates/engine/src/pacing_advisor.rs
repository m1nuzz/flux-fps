//! Adaptive FPS limit advisor (Phase 3.4, RESEARCH_FINDINGS.md 8.3).
//!
//! The resident app does not pace game frames: it only *advises* a limit for
//! the game's own frame limiter / driver overlay (direct pacing is reserved
//! for the optional dxgi-proxy). The heuristic consumes the 5 s frametime
//! aggregate from `etw_present`:
//!
//! 1. sustainable limit = 1000 / (p99 frametime * safety factor);
//! 2. clamp to [min_fps, min(detected refresh, user cap)];
//! 3. require N consecutive windows with the same target before acting
//!    (prevents a single loading hitch from lowering the limit);
//! 4. move the published limit towards the target by at most 1 FPS/second,
//!    avoiding the micro-stutter caused by abrupt limiter changes.
//!
//! This module is pure logic - no Win32, no clocks it reads itself - so the
//! whole heuristic is unit-testable with synthetic frametimes.

use common::{Config, FrametimeStats, PacerAdvice};
use std::time::Instant;

/// Minimum samples in the sliding window before advising anything.
const MIN_SAMPLES: usize = 60;
/// Relative tolerance for two consecutive targets to count as "the same".
const STABLE_TOLERANCE: f32 = 0.02;
/// Absolute part of the stability tolerance, FPS.
const STABLE_TOLERANCE_FPS: f32 = 0.5;
/// Number of consecutive windows required before a target is trusted.
const DEFAULT_STABLE_WINDOWS: u32 = 3;
/// Hard gradient limit from RF 8.3 (FPS per second).
const DEFAULT_MAX_CHANGE_PER_SEC: f32 = 1.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AdvisorParams {
    /// >= 1.0; higher is more conservative.
    pub safety_factor: f32,
    /// Consecutive stable windows required.
    pub stable_windows: u32,
    /// Maximum limit change per second, FPS.
    pub max_change_per_sec: f32,
    /// Lower bound for a suggested limit, FPS.
    pub min_fps: f32,
}

impl AdvisorParams {
    /// Builds sanitized parameters from user configuration.
    pub fn from_config(config: &Config) -> Self {
        Self {
            safety_factor: config.pacer_safety_factor.clamp(1.0, 2.0),
            stable_windows: DEFAULT_STABLE_WINDOWS,
            max_change_per_sec: DEFAULT_MAX_CHANGE_PER_SEC,
            min_fps: config.pacer_min_fps.clamp(30, 240) as f32,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AdvisorLimits {
    /// Exact detected refresh rate of the fastest active display, Hz.
    pub max_fps: f32,
    /// Optional hard cap set by the user.
    pub user_cap: Option<f32>,
}

#[derive(Debug)]
pub struct LimitAdvisor {
    current: Option<f32>,
    candidate: Option<f32>,
    stable_count: u32,
    last_update: Instant,
}

impl LimitAdvisor {
    pub fn new(now: Instant) -> Self {
        Self {
            current: None,
            candidate: None,
            stable_count: 0,
            last_update: now,
        }
    }

    /// Feeds one frametime aggregate and returns the current recommendation.
    ///
    /// `now` is supplied by the caller so the gradient is deterministic in
    /// tests; in the runtime it is the 1 Hz engine tick timestamp.
    pub fn update(
        &mut self,
        stats: &FrametimeStats,
        params: &AdvisorParams,
        limits: &AdvisorLimits,
        now: Instant,
    ) -> PacerAdvice {
        let elapsed = now
            .saturating_duration_since(self.last_update)
            .as_secs_f32();
        self.last_update = now;

        let Some(p99_ms) = stats.p99_ms else {
            self.candidate = None;
            self.stable_count = 0;
            return PacerAdvice {
                suggested_fps: self.current,
                observed_fps: stats.presents_per_sec,
                reason: "no Present data",
                ..PacerAdvice::default()
            };
        };
        if stats.samples < MIN_SAMPLES || p99_ms <= 0.0 {
            self.candidate = None;
            self.stable_count = 0;
            return PacerAdvice {
                suggested_fps: self.current,
                observed_fps: stats.presents_per_sec,
                reason: "insufficient samples",
                ..PacerAdvice::default()
            };
        }

        // A user cap and the exact refresh rate both bound the advice; the
        // floor keeps the advisor from suggesting unusable low limits.
        let ceiling = limits
            .max_fps
            .min(limits.user_cap.unwrap_or(f32::MAX).max(params.min_fps))
            .max(params.min_fps);
        let sustainable = (1000.0 / (p99_ms * params.safety_factor)).clamp(params.min_fps, ceiling);

        let stable = self.candidate.is_some_and(|c| {
            (c - sustainable).abs() <= (c * STABLE_TOLERANCE).max(STABLE_TOLERANCE_FPS)
        });
        if stable {
            self.stable_count = self.stable_count.saturating_add(1);
        } else {
            self.candidate = Some(sustainable);
            self.stable_count = 1;
        }

        if self.stable_count < params.stable_windows {
            return PacerAdvice {
                suggested_fps: self.current,
                sustainable_fps: Some(sustainable),
                observed_fps: stats.presents_per_sec,
                stable_windows: self.stable_count,
                reason: "waiting for stable windows",
            };
        }

        let max_delta = params.max_change_per_sec * elapsed.max(0.0);
        let next = match self.current {
            Some(current) => current + (sustainable - current).clamp(-max_delta, max_delta),
            None => sustainable,
        };
        self.current = Some(next);
        let reason = if (next - sustainable).abs() < 0.05 {
            "limit reached"
        } else {
            "gradient step"
        };
        PacerAdvice {
            suggested_fps: Some(next),
            sustainable_fps: Some(sustainable),
            observed_fps: stats.presents_per_sec,
            stable_windows: self.stable_count,
            reason,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn stats(p99_ms: f32, fps: f32) -> FrametimeStats {
        FrametimeStats {
            avg_ms: Some(1000.0 / fps),
            p99_ms: Some(p99_ms),
            presents_per_sec: fps,
            samples: 600,
        }
    }

    fn params() -> AdvisorParams {
        AdvisorParams {
            safety_factor: 1.1,
            stable_windows: 3,
            max_change_per_sec: 1.0,
            min_fps: 30.0,
        }
    }

    fn limits() -> AdvisorLimits {
        AdvisorLimits {
            max_fps: 240.0,
            user_cap: None,
        }
    }

    #[test]
    fn no_data_never_produces_a_limit() {
        let start = Instant::now();
        let mut advisor = LimitAdvisor::new(start);
        let advice = advisor.update(
            &FrametimeStats::default(),
            &params(),
            &limits(),
            start + Duration::from_secs(1),
        );
        assert_eq!(advice.suggested_fps, None);
        assert_eq!(advice.reason, "no Present data");
    }

    #[test]
    fn sparse_windows_are_ignored() {
        let start = Instant::now();
        let mut advisor = LimitAdvisor::new(start);
        let mut sparse = stats(8.0, 120.0);
        sparse.samples = 4;
        let advice = advisor.update(
            &sparse,
            &params(),
            &limits(),
            start + Duration::from_secs(1),
        );
        assert_eq!(advice.reason, "insufficient samples");
        assert_eq!(advice.suggested_fps, None);
    }

    #[test]
    fn requires_three_stable_windows_before_advising() {
        let start = Instant::now();
        let mut advisor = LimitAdvisor::new(start);
        let sample = stats(8.0, 120.0);
        for second in 1..3 {
            let advice = advisor.update(
                &sample,
                &params(),
                &limits(),
                start + Duration::from_secs(second),
            );
            assert_eq!(advice.reason, "waiting for stable windows");
            assert_eq!(advice.suggested_fps, None);
        }
        let advice = advisor.update(
            &sample,
            &params(),
            &limits(),
            start + Duration::from_secs(3),
        );
        // 1000 / (8 ms * 1.1) = 113.636 FPS.
        let suggested = advice.suggested_fps.expect("limit after 3 stable windows");
        assert!((suggested - 113.636).abs() < 0.1, "got {suggested}");
        assert_eq!(advice.reason, "limit reached");
    }

    #[test]
    fn limit_changes_at_most_one_fps_per_second() {
        let start = Instant::now();
        let mut advisor = LimitAdvisor::new(start);
        let slow = stats(8.0, 120.0);
        for second in 1..=3 {
            advisor.update(
                &slow,
                &params(),
                &limits(),
                start + Duration::from_secs(second),
            );
        }
        let baseline = advisor
            .update(&slow, &params(), &limits(), start + Duration::from_secs(4))
            .suggested_fps
            .expect("baseline");
        // A much faster game: target jumps to 1000 / (4 * 1.1) = 227.27 FPS.
        let fast = stats(4.0, 230.0);
        // Two windows build stability; the third one applies the first step.
        for second in 5..=6 {
            advisor.update(
                &fast,
                &params(),
                &limits(),
                start + Duration::from_secs(second),
            );
        }
        let stepped = advisor
            .update(&fast, &params(), &limits(), start + Duration::from_secs(7))
            .suggested_fps
            .expect("step");
        assert!(stepped > baseline, "limit should rise towards the target");
        assert!(
            stepped - baseline <= 1.01,
            "moved {} FPS in 1 s",
            stepped - baseline
        );
    }

    #[test]
    fn refresh_rate_and_user_cap_bound_the_target() {
        let start = Instant::now();
        let fast = stats(2.0, 500.0);
        let mut params_small = params();
        params_small.stable_windows = 1;

        let mut advisor = LimitAdvisor::new(start);
        let advice = advisor.update(
            &fast,
            &params_small,
            &AdvisorLimits {
                max_fps: 239.996,
                user_cap: None,
            },
            start + Duration::from_secs(1),
        );
        assert!((advice.suggested_fps.unwrap() - 239.996).abs() < 0.01);

        let mut advisor = LimitAdvisor::new(start);
        let advice = advisor.update(
            &fast,
            &params_small,
            &AdvisorLimits {
                max_fps: 239.996,
                user_cap: Some(45.0),
            },
            start + Duration::from_secs(1),
        );
        assert!((advice.suggested_fps.unwrap() - 45.0).abs() < 0.01);
    }

    #[test]
    fn min_fps_floor_protects_against_hitches() {
        let start = Instant::now();
        let mut params_small = params();
        params_small.stable_windows = 1;
        let mut advisor = LimitAdvisor::new(start);
        let hitch = stats(100.0, 10.0);
        let advice = advisor.update(
            &hitch,
            &params_small,
            &limits(),
            start + Duration::from_secs(1),
        );
        assert_eq!(advice.suggested_fps, Some(30.0));
    }

    #[test]
    fn config_sanitizes_untrusted_values() {
        let config = Config {
            pacer_safety_factor: 99.0,
            pacer_min_fps: 1,
            ..Config::default()
        };
        let params = AdvisorParams::from_config(&config);
        assert_eq!(params.safety_factor, 2.0);
        assert_eq!(params.min_fps, 30.0);
        assert_eq!(params.max_change_per_sec, 1.0);
    }
}
