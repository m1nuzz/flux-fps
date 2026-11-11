//! Statistical core of the A/B validation harness (RESEARCH_FINDINGS p.10).
//!
//! Everything here is pure and deterministic so it can be unit-tested in CI
//! without elevation, ETW sessions or real games. Frametimes are in
//! milliseconds; the "1% low" metric is the FPS equivalent of the 99th
//! percentile frametime (the worst 1% of frames).

use common::Verdict;

/// Frametimes longer than this are stream discontinuities (alt-tab, loading),
/// not frames. Matches the ETW Present consumer in the engine crate.
const MAX_FRAMETIME_MS: f32 = 1000.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RunMetrics {
    pub samples: usize,
    pub avg_fps: f32,
    /// FPS derived from the 99th percentile frametime.
    pub low_1pct_fps: f32,
    /// FPS derived from the 99.9th percentile frametime.
    pub low_01pct_fps: f32,
    pub p95_ms: f32,
    pub p99_ms: f32,
    pub stddev_ms: f32,
    pub dpc_max_us: f32,
}

/// Drops invalid deltas and returns a sorted copy.
pub fn sanitize(frametimes_ms: &[f32]) -> Vec<f32> {
    let mut sorted: Vec<f32> = frametimes_ms
        .iter()
        .copied()
        .filter(|v| v.is_finite() && *v > 0.0 && *v < MAX_FRAMETIME_MS)
        .collect();
    sorted.sort_by(f32::total_cmp);
    sorted
}

/// Nearest-rank percentile on an ascending slice (0.0 < q <= 1.0).
pub fn percentile(sorted: &[f32], q: f64) -> f32 {
    debug_assert!(!sorted.is_empty());
    let index = ((sorted.len() as f64 * q).ceil() as usize)
        .saturating_sub(1)
        .min(sorted.len() - 1);
    sorted[index]
}

/// Aggregates one run. `dpc_max_us` comes from the DPC/ISR ETW monitor.
pub fn run_metrics(frametimes_ms: &[f32], dpc_max_us: f32) -> Option<RunMetrics> {
    let sorted = sanitize(frametimes_ms);
    if sorted.is_empty() {
        return None;
    }
    let n = sorted.len() as f64;
    let mean = sorted.iter().map(|v| f64::from(*v)).sum::<f64>() / n;
    let variance = if sorted.len() > 1 {
        sorted
            .iter()
            .map(|v| (f64::from(*v) - mean).powi(2))
            .sum::<f64>()
            / (n - 1.0)
    } else {
        0.0
    };
    let p99 = f64::from(percentile(&sorted, 0.99));
    let p999 = f64::from(percentile(&sorted, 0.999));
    Some(RunMetrics {
        samples: sorted.len(),
        avg_fps: (1000.0 / mean) as f32,
        low_1pct_fps: (1000.0 / p99) as f32,
        low_01pct_fps: (1000.0 / p999) as f32,
        p95_ms: percentile(&sorted, 0.95),
        p99_ms: p99 as f32,
        stddev_ms: variance.sqrt() as f32,
        dpc_max_us,
    })
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WelchResult {
    pub t: f64,
    pub df: f64,
    pub p_two_sided: f64,
}

fn mean_and_variance(values: &[f64]) -> Option<(f64, f64)> {
    if values.len() < 2 {
        return None;
    }
    let n = values.len() as f64;
    let mean = values.iter().sum::<f64>() / n;
    let variance = values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (n - 1.0);
    Some((mean, variance))
}

/// Welch's t-test for unequal variances (RF p.10.3).
pub fn welch_t_test(a: &[f64], b: &[f64]) -> Option<WelchResult> {
    let (mean_a, var_a) = mean_and_variance(a)?;
    let (mean_b, var_b) = mean_and_variance(b)?;
    let na = a.len() as f64;
    let nb = b.len() as f64;
    let sa = var_a / na;
    let sb = var_b / nb;
    let denom = sa + sb;
    if denom <= 0.0 {
        return Some(WelchResult {
            t: 0.0,
            df: na + nb - 2.0,
            p_two_sided: 1.0,
        });
    }
    let t = (mean_a - mean_b) / denom.sqrt();
    let df = denom.powi(2) / (sa.powi(2) / (na - 1.0) + sb.powi(2) / (nb - 1.0));
    let x = df / (df + t.powi(2));
    Some(WelchResult {
        t,
        df,
        p_two_sided: regularized_incomplete_beta(df / 2.0, 0.5, x),
    })
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MannWhitneyResult {
    pub u: f64,
    pub z: f64,
    pub p_two_sided: f64,
}

/// Mann-Whitney U with average ranks for ties and a tie-corrected normal
/// approximation. Used on the heavy-tailed 1% low distribution (RF p.10.3).
pub fn mann_whitney_u(a: &[f64], b: &[f64]) -> Option<MannWhitneyResult> {
    if a.is_empty() || b.is_empty() {
        return None;
    }
    let mut tagged: Vec<(f64, usize)> = a
        .iter()
        .map(|&v| (v, 0))
        .chain(b.iter().map(|&v| (v, 1)))
        .collect();
    tagged.sort_by(|x, y| x.0.total_cmp(&y.0));

    let mut rank_sum_a = 0.0;
    let mut tie_term = 0.0;
    let mut i = 0;
    while i < tagged.len() {
        let mut j = i + 1;
        while j < tagged.len() && tagged[j].0 == tagged[i].0 {
            j += 1;
        }
        let average_rank = (i + j + 1) as f64 / 2.0;
        for entry in &tagged[i..j] {
            if entry.1 == 0 {
                rank_sum_a += average_rank;
            }
        }
        let tie_size = (j - i) as f64;
        tie_term += tie_size.powi(3) - tie_size;
        i = j;
    }

    let n1 = a.len() as f64;
    let n2 = b.len() as f64;
    let n = n1 + n2;
    let u1 = rank_sum_a - n1 * (n1 + 1.0) / 2.0;
    let mean_u = n1 * n2 / 2.0;
    let variance = (n1 * n2 / 12.0) * ((n + 1.0) - tie_term / (n * (n - 1.0)));
    if variance <= 0.0 {
        return None;
    }
    let z = (u1 - mean_u).abs().max(0.0) - 0.5;
    let z = z.max(0.0) / variance.sqrt();
    Some(MannWhitneyResult {
        u: u1,
        z,
        p_two_sided: erfc(z / std::f64::consts::SQRT_2),
    })
}

/// Deterministic xorshift64* so bootstrap results are reproducible run to run.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed | 1)
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound as u64) as usize
    }
}

/// Bootstrap percentile CI of the 1% low gain (10k resamples by default).
pub fn bootstrap_gain_ci(
    baseline: &[f64],
    variant: &[f64],
    resamples: usize,
) -> Option<(f64, f64)> {
    if baseline.is_empty() || variant.is_empty() || resamples == 0 {
        return None;
    }
    let mut rng = Rng::new(0x4658_5850_5F41_4221);
    let mut gains: Vec<f64> = Vec::with_capacity(resamples);
    for _ in 0..resamples {
        let base_mean = resampled_mean(baseline, &mut rng);
        let variant_mean = resampled_mean(variant, &mut rng);
        if base_mean <= 0.0 {
            continue;
        }
        gains.push((variant_mean - base_mean) / base_mean * 100.0);
    }
    if gains.is_empty() {
        return None;
    }
    gains.sort_by(f64::total_cmp);
    let low = gains[((gains.len() as f64 * 0.025).floor() as usize).min(gains.len() - 1)];
    let high = gains[((gains.len() as f64 * 0.975).floor() as usize).min(gains.len() - 1)];
    Some((low, high))
}

fn resampled_mean(values: &[f64], rng: &mut Rng) -> f64 {
    let mut sum = 0.0;
    for _ in 0..values.len() {
        sum += values[rng.below(values.len())];
    }
    sum / values.len() as f64
}

#[derive(Debug, Clone, Copy)]
pub struct DecisionConfig {
    /// Minimum 1% low gain in percent required for APPLY (RF p.10.3: +5%).
    pub min_gain_pct: f64,
    /// Significance level (RF p.10.3: 0.05).
    pub alpha: f64,
    /// Bootstrap resamples (RF p.10.3: 10_000).
    pub resamples: usize,
}

impl Default for DecisionConfig {
    fn default() -> Self {
        Self {
            min_gain_pct: 5.0,
            alpha: 0.05,
            resamples: 10_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Decision {
    pub verdict: Verdict,
    pub gain_pct: f64,
    pub p_welch: f64,
    pub p_mann_whitney: f64,
    pub ci_low_pct: f64,
    pub ci_high_pct: f64,
    pub dpc_reduction_pct: f64,
    pub reason: &'static str,
}

fn mean_f64(values: &[f64]) -> f64 {
    values.iter().sum::<f64>() / values.len() as f64
}

/// Full ABAB analysis: distributions, significance, bootstrap CI and the
/// auto-verdict written into the profile (RF p.10.3 / p.10.4).
pub fn decide(
    baseline_runs: &[RunMetrics],
    variant_runs: &[RunMetrics],
    baseline_frames: &[f32],
    variant_frames: &[f32],
    config: DecisionConfig,
) -> Result<Decision, String> {
    if baseline_runs.is_empty() || variant_runs.is_empty() {
        return Err("both baseline and variant need at least one run".into());
    }
    let baseline_low: Vec<f64> = baseline_runs
        .iter()
        .map(|m| f64::from(m.low_1pct_fps))
        .collect();
    let variant_low: Vec<f64> = variant_runs
        .iter()
        .map(|m| f64::from(m.low_1pct_fps))
        .collect();
    let base_mean = mean_f64(&baseline_low);
    if base_mean <= 0.0 {
        return Err("baseline 1% low is zero".into());
    }
    let gain_pct = (mean_f64(&variant_low) - base_mean) / base_mean * 100.0;

    let frames_a: Vec<f64> = sanitize(baseline_frames)
        .iter()
        .map(|v| f64::from(*v))
        .collect();
    let frames_b: Vec<f64> = sanitize(variant_frames)
        .iter()
        .map(|v| f64::from(*v))
        .collect();
    let p_welch = welch_t_test(&frames_a, &frames_b)
        .map(|r| r.p_two_sided)
        .unwrap_or(1.0);
    let p_mann_whitney = mann_whitney_u(&baseline_low, &variant_low)
        .map(|r| r.p_two_sided)
        .unwrap_or(1.0);
    let (ci_low_pct, ci_high_pct) =
        bootstrap_gain_ci(&baseline_low, &variant_low, config.resamples)
            .unwrap_or((gain_pct, gain_pct));

    let base_dpc = mean_f64(
        &baseline_runs
            .iter()
            .map(|m| f64::from(m.dpc_max_us))
            .collect::<Vec<_>>(),
    );
    let variant_dpc = mean_f64(
        &variant_runs
            .iter()
            .map(|m| f64::from(m.dpc_max_us))
            .collect::<Vec<_>>(),
    );
    let dpc_reduction_pct = if base_dpc > 0.0 {
        (base_dpc - variant_dpc) / base_dpc * 100.0
    } else {
        0.0
    };

    let verdict = if gain_pct < 0.0 {
        Verdict::Revert
    } else if dpc_reduction_pct < -10.0 {
        // A large DPC regression means the profile increased interference,
        // so a positive frametime gain is not trustworthy (RF p.10.1).
        Verdict::Revert
    } else if gain_pct >= config.min_gain_pct && p_mann_whitney < config.alpha && ci_low_pct > 0.0 {
        Verdict::Apply
    } else {
        Verdict::KeepInsignificant
    };

    let reason = match verdict {
        Verdict::Apply => "1% low gain verified (p < alpha, bootstrap CI above zero)",
        Verdict::KeepInsignificant => "gain below threshold or not statistically verified",
        Verdict::Revert => {
            if gain_pct < 0.0 {
                "1% low regressed; revert the profile"
            } else {
                "DPC latency increased; mechanism check failed"
            }
        }
    };

    Ok(Decision {
        verdict,
        gain_pct,
        p_welch,
        p_mann_whitney,
        ci_low_pct,
        ci_high_pct,
        dpc_reduction_pct,
        reason,
    })
}

// --- Special functions (no external crates) -------------------------------

/// Lanczos log-gamma, |error| < 1e-13 for z > 0.
fn ln_gamma(z: f64) -> f64 {
    const COEFFICIENTS: [f64; 8] = [
        0.999_999_999_999_809_9,
        676.520_368_121_885_1,
        -1_259.139_216_722_402_8,
        771.323_428_777_653_1,
        -176.615_029_162_140_6,
        12.507_343_278_686_905,
        -0.138_571_095_265_720_1,
        9.984_369_578_019_572e-6,
    ];
    if z < 0.5 {
        // Reflection formula: Gamma(z)Gamma(1-z) = pi / sin(pi*z).
        return (std::f64::consts::PI / (std::f64::consts::PI * z).sin()).ln() - ln_gamma(1.0 - z);
    }
    let z = z - 1.0;
    let mut series = COEFFICIENTS[0];
    for (i, coefficient) in COEFFICIENTS.iter().enumerate().skip(1) {
        series += *coefficient / (z + i as f64);
    }
    let t = z + 7.5;
    0.5 * (2.0 * std::f64::consts::PI).ln() + (z + 0.5) * t.ln() - t + series.ln()
}

/// Continued fraction expansion for the incomplete beta function.
fn beta_continued_fraction(a: f64, b: f64, x: f64) -> f64 {
    const MAX_ITERATIONS: usize = 300;
    const EPSILON: f64 = 3.0e-14;
    let qab = a + b;
    let qap = a + 1.0;
    let qam = a - 1.0;
    let mut c = 1.0;
    let mut d = 1.0 - qab * x / qap;
    if d.abs() < 1.0e-30 {
        d = 1.0e-30;
    }
    d = 1.0 / d;
    let mut h = d;
    for m in 1..=MAX_ITERATIONS {
        let m2 = 2 * m;
        let aa = m as f64 * (b - m as f64) * x / ((qam + m2 as f64) * (a + m2 as f64));
        d = 1.0 + aa * d;
        if d.abs() < 1.0e-30 {
            d = 1.0e-30;
        }
        c = 1.0 + aa / c;
        if c.abs() < 1.0e-30 {
            c = 1.0e-30;
        }
        d = 1.0 / d;
        h *= d * c;
        let aa = -(a + m as f64) * (qab + m as f64) * x / ((a + m2 as f64) * (qap + m2 as f64));
        d = 1.0 + aa * d;
        if d.abs() < 1.0e-30 {
            d = 1.0e-30;
        }
        c = 1.0 + aa / c;
        if c.abs() < 1.0e-30 {
            c = 1.0e-30;
        }
        d = 1.0 / d;
        let delta = d * c;
        h *= delta;
        if (delta - 1.0).abs() < EPSILON {
            break;
        }
    }
    h
}

/// Regularized incomplete beta I_x(a, b).
fn regularized_incomplete_beta(a: f64, b: f64, x: f64) -> f64 {
    if x <= 0.0 {
        return 0.0;
    }
    if x >= 1.0 {
        return 1.0;
    }
    let front =
        (ln_gamma(a + b) - ln_gamma(a) - ln_gamma(b) + a * x.ln() + b * (1.0 - x).ln()).exp();
    if x < (a + 1.0) / (a + b + 2.0) {
        front * beta_continued_fraction(a, b, x) / a
    } else {
        1.0 - front * beta_continued_fraction(b, a, 1.0 - x) / b
    }
}

/// Complementary error function, Abramowitz & Stegun 7.1.26.
fn erfc(x: f64) -> f64 {
    let z = x.abs();
    let t = 1.0 / (1.0 + 0.327_591_1 * z);
    let result =
        (((((1.061_405_429 * t - 1.453_152_027) * t) + 1.421_413_741) * t - 0.284_496_736) * t
            + 0.254_829_592)
            * t
            * (-z * z).exp();
    if x < 0.0 {
        2.0 - result
    } else {
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_drops_outliers_and_sorts() {
        let input = [16.0, 0.0, 2000.0, 8.0, f32::NAN, 12.0];
        assert_eq!(sanitize(&input), vec![8.0, 12.0, 16.0]);
    }

    #[test]
    fn percentile_uses_nearest_rank() {
        let sorted: Vec<f32> = (1..=100).map(|v| v as f32).collect();
        assert_eq!(percentile(&sorted, 0.99), 99.0);
        assert_eq!(percentile(&sorted, 0.999), 100.0);
        assert_eq!(percentile(&sorted, 0.01), 1.0);
    }

    #[test]
    fn metrics_match_expected_values() {
        let frames = vec![10.0; 1000];
        let m = run_metrics(&frames, 250.0).unwrap();
        assert_eq!(m.samples, 1000);
        assert!((m.avg_fps - 100.0).abs() < 0.001);
        assert!((m.low_1pct_fps - 100.0).abs() < 0.001);
        assert!(m.stddev_ms.abs() < 0.001);
        assert_eq!(m.dpc_max_us, 250.0);
    }

    #[test]
    fn welch_detects_separated_distributions() {
        let a: Vec<f64> = (0..60).map(|v| 10.0 + (v % 5) as f64 * 0.1).collect();
        let b: Vec<f64> = (0..60).map(|v| 16.0 + (v % 5) as f64 * 0.1).collect();
        let result = welch_t_test(&a, &b).unwrap();
        assert!(result.p_two_sided < 0.001, "p = {}", result.p_two_sided);
        let same = welch_t_test(&a, &a).unwrap();
        assert!((same.p_two_sided - 1.0).abs() < 1e-9);
    }

    #[test]
    fn mann_whitney_handles_ties_and_separation() {
        let same = mann_whitney_u(&[1.0, 1.0, 1.0], &[1.0, 1.0, 1.0]).unwrap();
        assert!(same.p_two_sided > 0.05);
        let separated =
            mann_whitney_u(&[1.0, 2.0, 3.0, 4.0, 5.0], &[6.0, 7.0, 8.0, 9.0, 10.0]).unwrap();
        assert!(
            separated.p_two_sided < 0.05,
            "p = {}",
            separated.p_two_sided
        );
    }

    #[test]
    fn bootstrap_is_deterministic_and_ordered() {
        let a = vec![100.0, 102.0, 98.0, 101.0, 99.0];
        let b = vec![110.0, 112.0, 108.0, 111.0, 109.0];
        let first = bootstrap_gain_ci(&a, &b, 2000).unwrap();
        let second = bootstrap_gain_ci(&a, &b, 2000).unwrap();
        assert_eq!(first, second);
        assert!(first.0 <= first.1);
        assert!(first.0 > 5.0);
    }

    #[test]
    fn decision_applies_only_verified_gains() {
        let cfg = DecisionConfig {
            resamples: 2000,
            ..DecisionConfig::default()
        };
        let baseline = vec![
            RunMetrics {
                samples: 100,
                avg_fps: 100.0,
                low_1pct_fps: 60.0,
                low_01pct_fps: 50.0,
                p95_ms: 15.0,
                p99_ms: 16.67,
                stddev_ms: 2.0,
                dpc_max_us: 300.0
            };
            5
        ];
        let mut variant = baseline.clone();
        for run in &mut variant {
            run.low_1pct_fps = 66.0;
            run.dpc_max_us = 240.0;
        }
        let frames_a = vec![16.0; 500];
        let frames_b = vec![15.0; 500];
        let decision = decide(&baseline, &variant, &frames_a, &frames_b, cfg).unwrap();
        assert_eq!(decision.verdict, Verdict::Apply, "{decision:?}");
        assert!(decision.gain_pct > 9.0);

        let regressed = decide(&variant, &baseline, &frames_b, &frames_a, cfg).unwrap();
        assert_eq!(regressed.verdict, Verdict::Revert);
    }

    #[test]
    fn decision_keeps_insignificant_gains() {
        let cfg = DecisionConfig {
            resamples: 2000,
            ..DecisionConfig::default()
        };
        let baseline = vec![
            RunMetrics {
                samples: 100,
                avg_fps: 100.0,
                low_1pct_fps: 60.0,
                low_01pct_fps: 50.0,
                p95_ms: 15.0,
                p99_ms: 16.67,
                stddev_ms: 2.0,
                dpc_max_us: 300.0
            };
            5
        ];
        let mut variant = baseline.clone();
        for run in &mut variant {
            run.low_1pct_fps = 61.0;
        }
        let frames = vec![16.0; 500];
        let decision = decide(&baseline, &variant, &frames, &frames, cfg).unwrap();
        assert_eq!(decision.verdict, Verdict::KeepInsignificant, "{decision:?}");
    }
}
