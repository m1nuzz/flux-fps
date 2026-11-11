//! flux-fps-setup: portable one-time baseline wizard.
//!
//! User-mode only. Writes the default per-user config and (re)creates the
//! elevated helper and unelevated tray logon tasks through embedded
//! PowerShell scripts. Privileged work happens in the helper, never in this
//! binary; there is no kernel code and no driver loading anywhere in this
//! tool.

use common::Config;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const REGISTER_SCRIPT: &str = include_str!("../../../scripts/Register-FluxFpsTask.ps1");
const UNREGISTER_SCRIPT: &str = include_str!("../../../scripts/Unregister-FluxFpsTask.ps1");
const HELPER_TASK: &str = "FluxFps Helper";
const TRAY_TASK: &str = "FluxFps Tray";

fn usage() {
    println!("flux-fps-setup v{}", common::VERSION);
    println!("Usage: flux-fps-setup.exe <command>");
    println!("  --apply     write the default config and register both logon tasks");
    println!("  --restore   remove both logon tasks and restore isolation state");
    println!("  --status    show config, auto-start and logon task state");
    println!("  --version   print the version");
    println!("  --help      print this help");
    println!();
    println!("The tray task is registered only when config.toml has auto_start = true.");
    println!("Run --apply and --restore from an elevated prompt (task registration");
    println!("requires administrator rights). FLUX_FPS_EXE overrides the resident");
    println!("binary path used for task registration.");
}

/// Locates the installed resident binary. The portable wizard itself is never
/// installed; it points the scheduled tasks at the MSI-installed executable.
fn resident_exe() -> Option<PathBuf> {
    if let Some(path) = env::var_os("FLUX_FPS_EXE") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    if let Ok(current) = env::current_exe() {
        let sibling = current.with_file_name("flux-fps.exe");
        if sibling.is_file() {
            return Some(sibling);
        }
    }
    if let Some(program_files) = env::var_os("ProgramFiles") {
        let installed = PathBuf::from(program_files)
            .join("flux-fps")
            .join("flux-fps.exe");
        if installed.is_file() {
            return Some(installed);
        }
    }
    None
}

/// Writes an embedded script to a temp file and runs it with PowerShell.
fn run_embedded_script(name: &str, contents: &str, args: &[String]) -> Result<(), String> {
    let path = env::temp_dir().join(format!("flux-fps-{name}.ps1"));
    fs::write(&path, contents).map_err(|e| format!("cannot write {}: {e}", path.display()))?;

    let mut command = Command::new("powershell.exe");
    command.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"]);
    command.arg(&path);
    for arg in args {
        command.arg(arg);
    }
    let status = command.status();
    let _ = fs::remove_file(&path);

    match status {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(format!("PowerShell script {name} exited with {status}")),
        Err(e) => Err(format!("cannot start PowerShell: {e}")),
    }
}

fn task_name_args() -> Vec<String> {
    vec![
        "-TaskName".to_string(),
        HELPER_TASK.to_string(),
        "-TrayTaskName".to_string(),
        TRAY_TASK.to_string(),
    ]
}

fn push_exe_arg(args: &mut Vec<String>, path: &Path) {
    args.push("-ExePath".to_string());
    args.push(path.to_string_lossy().into_owned());
}

fn apply() -> Result<(), String> {
    let config_path = Config::default_path().ok_or("APPDATA is unavailable")?;
    let existed = config_path.is_file();
    let config = Config::load_or_default(&config_path);
    if existed {
        println!("Config already present: {}", config_path.display());
    } else {
        config
            .save(&config_path)
            .map_err(|e| format!("cannot write config: {e}"))?;
        println!("Wrote default config: {}", config_path.display());
    }
    println!(
        "Auto-start: {}",
        if config.auto_start {
            "enabled (tray task will be registered)"
        } else {
            "disabled (helper task only)"
        }
    );

    match resident_exe() {
        Some(path) => {
            let mut args = task_name_args();
            // A [bool] parameter needs the colon form; passing the flag and
            // its value as two separate arguments would fail to bind.
            args.push(format!(
                "-RegisterTray:${}",
                if config.auto_start { "true" } else { "false" }
            ));
            push_exe_arg(&mut args, &path);
            run_embedded_script("register-task", REGISTER_SCRIPT, &args)?;
            println!("Logon tasks applied for {}", path.display());
        }
        None => println!("flux-fps.exe not found; install the MSI, then run --apply again."),
    }
    Ok(())
}

fn restore() -> Result<(), String> {
    let mut args = task_name_args();
    if let Some(path) = resident_exe() {
        push_exe_arg(&mut args, &path);
    }
    run_embedded_script("unregister-task", UNREGISTER_SCRIPT, &args)?;
    println!("Restore finished.");
    Ok(())
}

fn task_state(name: &str) -> &'static str {
    match Command::new("schtasks.exe")
        .args(["/Query", "/TN", name])
        .output()
    {
        Ok(output) if output.status.success() => "registered",
        Ok(_) => "not registered",
        Err(_) => "query failed",
    }
}

fn status() {
    match Config::default_path() {
        Some(path) => {
            println!(
                "Config: {} ({})",
                path.display(),
                if path.is_file() { "present" } else { "missing" }
            );
            let config = Config::load_or_default(&path);
            println!("Auto-start: {}", config.auto_start);
        }
        None => println!("Config: APPDATA is unavailable"),
    }
    println!("Helper task: {}", task_state(HELPER_TASK));
    println!("Tray task:   {}", task_state(TRAY_TASK));
    match resident_exe() {
        Some(path) => println!("Resident binary: {}", path.display()),
        None => println!("Resident binary: not found"),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("--apply") => apply(),
        Some("--restore") => restore(),
        Some("--status") => {
            status();
            Ok(())
        }
        Some("--version") => {
            println!("flux-fps-setup v{}", common::VERSION);
            Ok(())
        }
        Some("--help") | None => {
            usage();
            Ok(())
        }
        Some(other) => Err(format!("unknown option: {other}")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("[flux-fps-setup] {e}");
            ExitCode::FAILURE
        }
    }
}
