//! ABAB session model, CSV ingestion and the Markdown benchmark report
//! (RESEARCH_FINDINGS p.10.2 / p.10.4).

use crate::stats::{decide, run_metrics, Decision, DecisionConfig, RunMetrics};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const SESSION_SCHEMA_VERSION: u32 = 1;
/// Minimum number of baseline/variant pairs (RF p.10.2: N >= 5).
pub const MIN_PAIRS: usize = 5;
/// Minimum duration of a single run in seconds (RF p.10.2).
pub const MIN_RUN_SECS: u64 = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Variant {
    /// Game Mode OFF, no profile applied.
    Baseline,
    /// Game Mode ON with the calibrated profile applied.
    Variant,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRecord {
    pub id: String,
    pub variant: Variant,
    /// CSV file name relative to the session directory.
    pub csv: String,
    pub duration_secs: u64,
    /// Max DPC duration in microseconds observed on the target cores.
    pub dpc_max_us: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub schema_version: u32,
    pub game: String,
    pub scenario: String,
    pub resolution: String,
    pub preset: String,
    pub power_plan: String,
    pub gpu_driver: String,
    /// HAGS must be fixed across all runs; it distorts GPU-side PresentMon
    /// fields (RF p.9.3).
    pub hags_enabled: bool,
    /// A 3-5 minute warmup was completed before the first measured run.
    pub warmup_done: bool,
    pub runs: Vec<RunRecord>,
}

#[derive(Debug, Clone)]
pub struct RunAnalysis {
    pub record: RunRecord,
    pub metrics: RunMetrics,
    pub frames_ms: Vec<f32>,
}

#[derive(Debug, Clone)]
pub struct Analysis {
    pub session: Session,
    pub runs: Vec<RunAnalysis>,
    pub decision: Decision,
}

/// Reads a frametime CSV. Accepts our own `frametime_ms` column, PresentMon's
/// `MsBetweenPresents`, or a headerless single numeric column.
pub fn read_frametimes(path: &Path) -> Result<Vec<f32>, String> {
    let text =
        fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let trimmed = text.trim_start_matches('\u{feff}');
    let mut lines = trimmed.lines().filter(|line| !line.trim().is_empty());
    let first = lines
        .next()
        .ok_or_else(|| format!("empty CSV: {}", path.display()))?;
    let header: Vec<String> = first
        .split(',')
        .map(|column| column.trim().trim_matches('"').to_ascii_lowercase())
        .collect();
    let column = header.iter().position(|name| {
        matches!(
            name.as_str(),
            "frametime_ms" | "msbetweenpresents" | "ms_between_presents"
        )
    });

    let data_lines: Vec<&str> = match column {
        Some(_) => lines.collect(),
        None => std::iter::once(first).chain(lines).collect(),
    };
    let index = column.unwrap_or(0);
    let mut values = Vec::new();
    for line in data_lines {
        let field = line.split(',').nth(index).unwrap_or("").trim();
        if let Ok(value) = field.parse::<f32>() {
            if value.is_finite() && value > 0.0 && value < 1000.0 {
                values.push(value);
            }
        }
    }
    if values.is_empty() {
        return Err(format!("no valid frametimes in {}", path.display()));
    }
    Ok(values)
}

/// Writes one run's raw frametimes in our canonical CSV format.
pub fn write_frametimes(path: &Path, frames_ms: &[f32]) -> Result<(), String> {
    let mut text = String::from("frametime_ms\n");
    for value in frames_ms {
        text.push_str(&format!("{value:.4}\n"));
    }
    fs::write(path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// Writes a ready-to-edit `session.toml` with the minimum ABAB protocol.
/// Fixed variables, the warmup flag and the CSV file names still have to be
/// filled in by the operator before `analyze` will accept the session.
pub fn write_template(dir: &Path, game: &str) -> Result<PathBuf, String> {
    fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let session = Session {
        schema_version: SESSION_SCHEMA_VERSION,
        game: game.to_string(),
        scenario: "built-in benchmark".into(),
        resolution: "1920x1080".into(),
        preset: "high".into(),
        power_plan: "balanced".into(),
        gpu_driver: "unknown".into(),
        hags_enabled: crate::capture::hags_enabled().unwrap_or(false),
        warmup_done: false,
        runs: (0..MIN_PAIRS * 2)
            .map(|index| RunRecord {
                id: format!("run-{:02}", index + 1),
                variant: if index % 2 == 0 {
                    Variant::Baseline
                } else {
                    Variant::Variant
                },
                csv: format!("run-{:02}.csv", index + 1),
                duration_secs: MIN_RUN_SECS,
                dpc_max_us: 0.0,
            })
            .collect(),
    };
    let text = toml::to_string_pretty(&session)
        .map_err(|e| format!("cannot serialize the session template: {e}"))?;
    let path = dir.join("session.toml");
    fs::write(&path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(path)
}

/// Validates the ABAB protocol before any statistics are computed.
pub fn validate(session: &Session) -> Result<(), String> {
    if session.schema_version != SESSION_SCHEMA_VERSION {
        return Err(format!(
            "unsupported session schema {} (expected {SESSION_SCHEMA_VERSION})",
            session.schema_version
        ));
    }
    if !session.warmup_done {
        return Err("warmup run is required before the measured ABAB pairs".into());
    }
    if session.runs.len() < MIN_PAIRS * 2 {
        return Err(format!(
            "need at least {} ABAB runs ({} pairs), got {}",
            MIN_PAIRS * 2,
            MIN_PAIRS,
            session.runs.len()
        ));
    }
    if session.runs.len() % 2 != 0 {
        return Err("ABAB runs must form complete pairs".into());
    }
    let first = session.runs[0].variant;
    for (index, run) in session.runs.iter().enumerate() {
        let expected = if index % 2 == 0 {
            first
        } else if first == Variant::Baseline {
            Variant::Variant
        } else {
            Variant::Baseline
        };
        if run.variant != expected {
            return Err(format!("run {} breaks the ABAB alternation", run.id));
        }
        if run.duration_secs < MIN_RUN_SECS {
            return Err(format!(
                "run {} is only {} s (minimum {MIN_RUN_SECS} s)",
                run.id, run.duration_secs
            ));
        }
    }
    Ok(())
}

/// Loads `session.toml`, validates the protocol and runs the full analysis.
pub fn analyze(dir: &Path) -> Result<Analysis, String> {
    let manifest = dir.join("session.toml");
    let text = fs::read_to_string(&manifest)
        .map_err(|e| format!("cannot read {}: {e}", manifest.display()))?;
    let session: Session =
        toml::from_str(&text).map_err(|e| format!("cannot parse {}: {e}", manifest.display()))?;
    validate(&session)?;

    let mut runs = Vec::with_capacity(session.runs.len());
    for record in &session.runs {
        let csv_path = dir.join(&record.csv);
        let frames_ms = read_frametimes(&csv_path)?;
        let metrics = run_metrics(&frames_ms, record.dpc_max_us)
            .ok_or_else(|| format!("run {} has no usable frametimes", record.id))?;
        runs.push(RunAnalysis {
            record: record.clone(),
            metrics,
            frames_ms,
        });
    }

    let baseline_frames: Vec<f32> = runs
        .iter()
        .filter(|r| r.record.variant == Variant::Baseline)
        .flat_map(|r| r.frames_ms.iter().copied())
        .collect();
    let variant_frames: Vec<f32> = runs
        .iter()
        .filter(|r| r.record.variant == Variant::Variant)
        .flat_map(|r| r.frames_ms.iter().copied())
        .collect();
    let baseline_metrics: Vec<RunMetrics> = runs
        .iter()
        .filter(|r| r.record.variant == Variant::Baseline)
        .map(|r| r.metrics)
        .collect();
    let variant_metrics: Vec<RunMetrics> = runs
        .iter()
        .filter(|r| r.record.variant == Variant::Variant)
        .map(|r| r.metrics)
        .collect();

    let decision = decide(
        &baseline_metrics,
        &variant_metrics,
        &baseline_frames,
        &variant_frames,
        DecisionConfig::default(),
    )?;
    Ok(Analysis {
        session,
        runs,
        decision,
    })
}

/// UTC date as YYYY-MM-DD, computed without extra dependencies.
pub fn utc_date() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = secs.div_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}")
}

/// Howard Hinnant's civil-from-days algorithm (days since 1970-01-01).
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    (if month <= 2 { y + 1 } else { y }, month, day)
}

fn verdict_label(verdict: common::Verdict) -> &'static str {
    match verdict {
        common::Verdict::Apply => "APPLY",
        common::Verdict::KeepInsignificant => "KEEP-INSIGNIFICANT",
        common::Verdict::Revert => "REVERT",
    }
}

/// Writes the Markdown report and the combined raw frametime CSV.
pub fn write_report(dir: &Path, analysis: &Analysis) -> Result<(PathBuf, PathBuf), String> {
    let date = utc_date();
    let game: String = analysis
        .session
        .game
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let report_path = dir.join(format!("bench-report-{date}-{game}.md"));
    let raw_path = dir.join(format!("frametimes-raw-{date}-{game}.csv"));

    let mut report = String::new();
    report.push_str(&format!(
        "# A/B validation report: {} ({date})\n\n",
        analysis.session.game
    ));
    report.push_str("## Fixed variables\n\n");
    report.push_str("| Variable | Value |\n|---|---|\n");
    report.push_str(&format!("| Scenario | {} |\n", analysis.session.scenario));
    report.push_str(&format!(
        "| Resolution | {} |\n",
        analysis.session.resolution
    ));
    report.push_str(&format!(
        "| Graphics preset | {} |\n",
        analysis.session.preset
    ));
    report.push_str(&format!(
        "| Power plan | {} |\n",
        analysis.session.power_plan
    ));
    report.push_str(&format!(
        "| GPU driver | {} |\n",
        analysis.session.gpu_driver
    ));
    report.push_str(&format!(
        "| HAGS | {} |\n",
        if analysis.session.hags_enabled {
            "on"
        } else {
            "off"
        }
    ));
    report.push_str(&format!(
        "| Warmup | {} |\n",
        if analysis.session.warmup_done {
            "done"
        } else {
            "skipped"
        }
    ));
    report.push_str(&format!(
        "| Runs | {} (ABAB, {} pairs) |\n\n",
        analysis.runs.len(),
        analysis.runs.len() / 2
    ));

    report.push_str("## Per-run metrics\n\n");
    report.push_str("| Run | Variant | Seconds | Frames | Avg FPS | 1% low | 0.1% low | P99 ms | sigma ms | DPC max us |\n");
    report.push_str("|---|---|---:|---:|---:|---:|---:|---:|---:|---:|\n");
    for run in &analysis.runs {
        let m = run.metrics;
        report.push_str(&format!(
            "| {} | {:?} | {} | {} | {:.1} | {:.1} | {:.1} | {:.2} | {:.2} | {:.0} |\n",
            run.record.id,
            run.record.variant,
            run.record.duration_secs,
            m.samples,
            m.avg_fps,
            m.low_1pct_fps,
            m.low_01pct_fps,
            m.p99_ms,
            m.stddev_ms,
            m.dpc_max_us
        ));
    }

    let d = &analysis.decision;
    report.push_str("\n## Statistics\n\n");
    report.push_str("| Metric | Value |\n|---|---|\n");
    report.push_str(&format!("| 1% low gain | {:+.2}% |\n", d.gain_pct));
    report.push_str(&format!(
        "| Bootstrap 95% CI | [{:+.2}%, {:+.2}%] |\n",
        d.ci_low_pct, d.ci_high_pct
    ));
    report.push_str(&format!(
        "| Welch t-test p (frametimes) | {:.6} |\n",
        d.p_welch
    ));
    report.push_str(&format!(
        "| Mann-Whitney U p (1% lows) | {:.6} |\n",
        d.p_mann_whitney
    ));
    report.push_str(&format!(
        "| Max DPC reduction | {:+.2}% |\n",
        d.dpc_reduction_pct
    ));
    report.push_str(&format!(
        "| Alpha | {:.2} |\n",
        DecisionConfig::default().alpha
    ));
    report.push_str(&format!(
        "| Gain threshold | {:+.1}% |\n",
        DecisionConfig::default().min_gain_pct
    ));

    report.push_str(&format!(
        "\n## Verdict: {}\n\n{}\n\n",
        verdict_label(d.verdict),
        d.reason
    ));
    report.push_str(
        "Limitations: real games are non-deterministic; ABAB alternation and a \
         nonparametric test reduce but do not eliminate drift. Built-in \
         benchmarks usually show a smaller gain than real matches because \
         there is less background interference. HAGS is fixed for this session \
         so GPU-side PresentMon fields are not compared.\n",
    );
    fs::write(&report_path, report)
        .map_err(|e| format!("cannot write {}: {e}", report_path.display()))?;

    let mut raw = String::from("run_id,variant,frametime_ms\n");
    for run in &analysis.runs {
        let variant = match run.record.variant {
            Variant::Baseline => "baseline",
            Variant::Variant => "variant",
        };
        for frame in &run.frames_ms {
            raw.push_str(&format!("{},{variant},{frame:.4}\n", run.record.id));
        }
    }
    fs::write(&raw_path, raw).map_err(|e| format!("cannot write {}: {e}", raw_path.display()))?;
    Ok((report_path, raw_path))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_session(runs: usize, warmup: bool) -> Session {
        Session {
            schema_version: SESSION_SCHEMA_VERSION,
            game: "CS2".into(),
            scenario: "built-in benchmark".into(),
            resolution: "1920x1080".into(),
            preset: "high".into(),
            power_plan: "balanced".into(),
            gpu_driver: "560.00".into(),
            hags_enabled: true,
            warmup_done: warmup,
            runs: (0..runs)
                .map(|i| RunRecord {
                    id: format!("run-{:02}", i + 1),
                    variant: if i % 2 == 0 {
                        Variant::Baseline
                    } else {
                        Variant::Variant
                    },
                    csv: format!("run-{:02}.csv", i + 1),
                    duration_secs: 90,
                    dpc_max_us: 300.0,
                })
                .collect(),
        }
    }

    #[test]
    fn validates_warmup_pairs_and_alternation() {
        assert!(validate(&sample_session(10, true)).is_ok());
        assert!(validate(&sample_session(8, true)).is_err());
        assert!(validate(&sample_session(10, false)).is_err());
        let mut broken = sample_session(10, true);
        broken.runs[3].variant = Variant::Baseline;
        assert!(validate(&broken).is_err());
        let mut short = sample_session(10, true);
        short.runs[0].duration_secs = 10;
        assert!(validate(&short).is_err());
    }

    #[test]
    fn csv_accepts_our_format_and_presentmon() {
        let dir = std::env::temp_dir().join(format!("flux-fps-csv-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let ours = dir.join("ours.csv");
        let theirs = dir.join("presentmon.csv");
        fs::write(&ours, "frametime_ms\n16.6\n15.9\ninvalid\n").unwrap();
        fs::write(
            &theirs,
            "Application,MsBetweenPresents,QPCTime\ncs2.exe,16.6,1\ncs2.exe,15.9,2\n",
        )
        .unwrap();
        assert_eq!(read_frametimes(&ours).unwrap(), vec![16.6, 15.9]);
        assert_eq!(read_frametimes(&theirs).unwrap(), vec![16.6, 15.9]);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn utc_date_is_well_formed() {
        let date = utc_date();
        assert_eq!(date.len(), 10);
        assert_eq!(&date[4..5], "-");
        assert_eq!(&date[7..8], "-");
    }
}
