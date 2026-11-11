//! flux-fps-master: portable per-PC calibration and A/B validation CLI.
//!
//! Every command is explicit; this tool never runs at logon, never installs
//! anything and never starts a background service. Kernel access is refused
//! by design (see the `guard` command).

use common::Verdict;
use flux_fps_master::{calibration, capture, guard, session, soak};
use std::path::PathBuf;
use std::process::ExitCode;
use std::str::FromStr;

const HELP: &str = "\
flux-fps-master - portable calibration and A/B validation

USAGE:
    flux-fps-master <command> [options]

COMMANDS:
    calibrate       Benchmark logical cores, refresh rate and purge response;
                    write a per-PC profile TOML.
    init-session    Write an ABAB session.toml template (10 runs / 5 pairs).
    capture         Record one run's raw frametimes to a canonical CSV.
    analyze         Validate an ABAB session, compute statistics, write the
                    Markdown report and the combined raw CSV.
    compare         Diff two profiles after a driver or app update.
    soak            Long-running helper/ETW/memory stability harness.
    restore-test    Kill the elevated helper and verify isolation is restored.
    guard           Show kernel opt-in preflight results (always refused).
    --version       Print the tool version.
    --help          Show this help.

COMMON OPTIONS:
    calibrate     --out <file>                     (default profile.toml)
    init-session  --dir <dir> --game <name>       (default . / game)
    capture       --seconds <n> --out <csv>
                  --pid <n> --presentmon <exe> --process <name>
    analyze       --dir <dir> --profile <file>
    compare       --old <file> --new <file>
    soak          --hours <n> --interval <n> --dir <dir>
                                                  (default 24 / 60 / .)
    restore-test  (no options)

All output is written to the current directory unless --dir/--out is given.
";

fn flag_value(options: &[String], flag: &str) -> Option<String> {
    let mut iter = options.iter();
    while let Some(arg) = iter.next() {
        if arg == flag {
            return iter.next().cloned();
        }
        if let Some(rest) = arg.strip_prefix(&format!("{flag}=")) {
            return Some(rest.to_string());
        }
    }
    None
}

fn required(options: &[String], flag: &str) -> Result<String, String> {
    flag_value(options, flag).ok_or_else(|| format!("missing required option {flag}"))
}

fn number<T>(options: &[String], flag: &str, default: Option<T>) -> Result<T, String>
where
    T: FromStr,
    T::Err: std::fmt::Display,
{
    match flag_value(options, flag) {
        Some(text) => text
            .parse::<T>()
            .map_err(|e| format!("invalid value for {flag}: {e}")),
        None => default.ok_or_else(|| format!("missing required option {flag}")),
    }
}

fn path_or(options: &[String], flag: &str, default: &str) -> PathBuf {
    PathBuf::from(flag_value(options, flag).unwrap_or_else(|| default.to_string()))
}

fn verdict_label(verdict: Verdict) -> &'static str {
    match verdict {
        Verdict::Apply => "APPLY",
        Verdict::KeepInsignificant => "KEEP-INSIGNIFICANT",
        Verdict::Revert => "REVERT",
    }
}

fn cmd_calibrate(options: &[String]) -> Result<(), String> {
    let out = path_or(options, "--out", "profile.toml");
    let (profile, report) = calibration::calibrate()?;
    println!("Logical processors scored: {}", report.cores.len());
    for core in &report.cores {
        println!(
            "  cpu set {:>3}  {}  score {:.3}",
            core.cpu_set_id,
            if core.is_p_core { "P-core" } else { "E-core" },
            core.score
        );
    }
    println!("Refresh rate: {:.3} Hz", report.refresh_hz);
    println!(
        "Background CPU sets: {} (E-cores preferred)",
        report.background_cpu_set_ids.len()
    );
    if report.purge_available {
        println!(
            "Standby purge: +{} MB available in {} ms",
            report.purge_gain_mb, report.purge_duration_ms
        );
    } else {
        println!("Standby purge: unavailable (elevated helper is not reachable)");
    }
    println!("Pacing safety factor: {:.4}", profile.pacing_safety_factor);
    calibration::write_profile(&out, &profile)?;
    println!("Profile written to {}", out.display());
    Ok(())
}

fn cmd_init_session(options: &[String]) -> Result<(), String> {
    let dir = path_or(options, "--dir", ".");
    let game = flag_value(options, "--game").unwrap_or_else(|| "game".to_string());
    let path = session::write_template(&dir, &game)?;
    println!("Session template written to {}", path.display());
    println!(
        "Edit the fixed variables, set warmup_done = true after a 3-5 minute \
         warmup, and point each run at its captured CSV (minimum {} s per run).",
        session::MIN_RUN_SECS
    );
    Ok(())
}

fn cmd_capture(options: &[String]) -> Result<(), String> {
    let seconds = number::<u64>(options, "--seconds", None)?;
    let out = PathBuf::from(required(options, "--out")?);
    let pid = number::<u32>(options, "--pid", Some(0))?;
    let presentmon = flag_value(options, "--presentmon");
    let process = flag_value(options, "--process");
    match capture::hags_enabled() {
        Some(true) => println!("HAGS: enabled (GPU-side PresentMon fields are not compared)"),
        Some(false) => println!("HAGS: disabled"),
        None => println!("HAGS: state unknown"),
    }
    let capture_options = capture::CaptureOptions {
        seconds,
        target_pid: pid,
        output: out.as_path(),
        presentmon: presentmon.as_deref(),
        process_name: process.as_deref(),
    };
    let frames_ms = capture::capture(&capture_options)?;
    println!(
        "Captured {} frames in {} s -> {}",
        frames_ms.len(),
        seconds,
        out.display()
    );
    Ok(())
}

fn cmd_analyze(options: &[String]) -> Result<(), String> {
    let dir = path_or(options, "--dir", ".");
    let profile_path = flag_value(options, "--profile").map(PathBuf::from);
    let analysis = session::analyze(&dir)?;
    let decision = &analysis.decision;
    println!(
        "{}: {} runs, 1% low gain {:+.2}% (CI {:+.2}%..{:+.2}%)",
        analysis.session.game,
        analysis.runs.len(),
        decision.gain_pct,
        decision.ci_low_pct,
        decision.ci_high_pct
    );
    println!(
        "Welch p {:.6}; Mann-Whitney p {:.6}; DPC reduction {:+.2}%",
        decision.p_welch, decision.p_mann_whitney, decision.dpc_reduction_pct
    );
    let (report, raw) = session::write_report(&dir, &analysis)?;
    println!("Report: {}", report.display());
    println!("Raw frametimes: {}", raw.display());
    if let Some(path) = profile_path.as_deref() {
        calibration::update_verdict(path, decision.verdict)?;
        println!("Profile verdict updated: {}", path.display());
    }
    println!(
        "VERDICT: {} - {}",
        verdict_label(decision.verdict),
        decision.reason
    );
    Ok(())
}

fn cmd_compare(options: &[String]) -> Result<(), String> {
    let old = PathBuf::from(required(options, "--old")?);
    let new = PathBuf::from(required(options, "--new")?);
    let old_profile = calibration::load_profile(&old)?;
    let new_profile = calibration::load_profile(&new)?;
    println!("{}", calibration::compare(&old_profile, &new_profile));
    Ok(())
}

fn cmd_soak(options: &[String]) -> Result<(), String> {
    let hours = number::<u64>(options, "--hours", Some(24))?;
    let interval = number::<u64>(options, "--interval", Some(60))?;
    let dir = path_or(options, "--dir", ".");
    println!(
        "Starting {hours} h soak (one sample every {interval} s). Press Ctrl+C to stop early."
    );
    let (csv, summary) = soak::run(hours, interval, &dir)?;
    println!("Soak CSV: {}", csv.display());
    println!(
        "Samples {}; helper failures {}; max isolated pids {}; max ETW sessions {}; restore {}",
        summary.samples,
        summary.helper_failures,
        summary.max_isolated_pids,
        summary.max_etw_sessions,
        if summary.restore_ok { "ok" } else { "FAILED" }
    );
    if summary.helper_failures > 0 || !summary.restore_ok {
        return Err("soak found helper or restore failures".into());
    }
    Ok(())
}

fn cmd_guard() -> Result<(), String> {
    let report = guard::preflight();
    println!("Kernel opt-in preflight (portable master tool only):");
    println!(
        "  HVCI: {}",
        match report.hvci_enabled {
            Some(true) => "enabled",
            Some(false) => "disabled",
            None => "unknown",
        }
    );
    println!(
        "  Vulnerable driver blocklist: {}",
        match report.driver_blocklist_enabled {
            Some(true) => "enabled",
            Some(false) => "disabled",
            None => "unknown",
        }
    );
    println!(
        "  Active anti-cheat services: {}",
        if report.active_anticheat.is_empty() {
            "none detected".to_string()
        } else {
            report.active_anticheat.join(", ")
        }
    );
    for problem in &report.problems {
        println!("  - {problem}");
    }
    println!(
        "  Allowed: {} (no signed driver is bundled; kernel access is refused)",
        report.allowed
    );
    // The refusal is explicit rather than a silent no-op.
    if guard::request_kernel_access().is_ok() {
        return Err("kernel guard unexpectedly allowed access".into());
    }
    Ok(())
}

fn dispatch(command: &str, options: &[String]) -> Result<(), String> {
    match command {
        "calibrate" => cmd_calibrate(options),
        "init-session" => cmd_init_session(options),
        "capture" => cmd_capture(options),
        "analyze" => cmd_analyze(options),
        "compare" => cmd_compare(options),
        "soak" => cmd_soak(options),
        "restore-test" => soak::restore_after_kill(),
        "guard" => cmd_guard(),
        _ => Err(format!(
            "unknown command '{command}'; run flux-fps-master --help"
        )),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(command) = args.first() else {
        println!("{HELP}");
        return ExitCode::SUCCESS;
    };
    let result = match command.as_str() {
        "--version" | "-V" => {
            println!("flux-fps-master v{}", common::VERSION);
            Ok(())
        }
        "--help" | "-h" | "help" => {
            println!("{HELP}");
            Ok(())
        }
        other => dispatch(other, &args[1..]),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}
