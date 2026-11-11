//! Kernel opt-in guardrails (RESEARCH_FINDINGS p.9.4.3, AGENTS.md).
//!
//! HARD RULE: the resident binary never contains kernel code. Any kernel
//! access belongs to this portable tool, is gated behind the `kernel-setup`
//! cargo feature, and is refused unless an explicit owner opt-in is present.
//!
//! This module implements the guardrails and the load/apply/unload lifecycle
//! state machine. **No driver is bundled and no driver is loaded**: the load
//! step always fails with an explicit explanation. The owner must supply a
//! legitimately signed driver separately; BYOVD has anti-cheat and security
//! consequences that must remain a deliberate, documented decision.

use std::process::Command;

/// Services whose presence means the machine is running an active anti-cheat.
/// In that state kernel access must be refused (AGENTS.md).
pub const ANTI_CHEAT_SERVICES: &[&str] = &["vgc", "vgk", "BEService", "EasyAntiCheat", "FACEIT"];

#[derive(Debug, Clone, PartialEq)]
pub struct GuardReport {
    pub hvci_enabled: Option<bool>,
    pub driver_blocklist_enabled: Option<bool>,
    pub active_anticheat: Vec<String>,
    pub problems: Vec<String>,
    pub allowed: bool,
}

fn registry_enabled(key: &str, value: &str) -> Option<bool> {
    let output = Command::new("reg.exe")
        .args(["query", key, "/v", value])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text.lines().find(|line| line.contains(value))?;
    let raw = line.split_whitespace().last()?.trim();
    match raw {
        "0x1" | "0x2" => Some(true),
        "0x0" => Some(false),
        _ => u32::from_str_radix(raw.trim_start_matches("0x"), 16)
            .ok()
            .map(|v| v != 0),
    }
}

/// HVCI / memory integrity status.
pub fn hvci_enabled() -> Option<bool> {
    registry_enabled(
        r"HKLM\SYSTEM\CurrentControlSet\Control\DeviceGuard\Scenarios\HypervisorEnforcedCodeIntegrity",
        "Enabled",
    )
}

/// Microsoft vulnerable driver blocklist status.
pub fn driver_blocklist_enabled() -> Option<bool> {
    registry_enabled(
        r"HKLM\SYSTEM\CurrentControlSet\Control\CI\Protected",
        "VerifiedAndReputablePolicyState",
    )
}

/// Active anti-cheat services detected via `sc query`.
pub fn active_anticheat_services() -> Vec<String> {
    let mut found = Vec::new();
    for service in ANTI_CHEAT_SERVICES {
        if let Ok(output) = Command::new("sc.exe").args(["query", service]).output() {
            let text = String::from_utf8_lossy(&output.stdout);
            if output.status.success() && text.contains("RUNNING") {
                found.push((*service).to_string());
            }
        }
    }
    found
}

/// Full preflight. `allowed` stays false by design: no driver is bundled.
pub fn preflight() -> GuardReport {
    let hvci = hvci_enabled();
    let blocklist = driver_blocklist_enabled();
    let anticheat = active_anticheat_services();
    let mut problems = Vec::new();

    match hvci {
        Some(true) => problems.push("HVCI is enabled; kernel writes are blocked".into()),
        Some(false) => problems
            .push("HVCI is disabled; kernel access would weaken the host security posture".into()),
        None => problems.push("HVCI state could not be determined".into()),
    }
    if blocklist != Some(true) {
        problems.push("Microsoft vulnerable driver blocklist is not confirmed enabled".into());
    }
    if !anticheat.is_empty() {
        problems.push(format!(
            "active anti-cheat services detected: {}",
            anticheat.join(", ")
        ));
    }
    problems.push("no signed kernel driver is bundled with flux-fps".into());

    GuardReport {
        hvci_enabled: hvci,
        driver_blocklist_enabled: blocklist,
        active_anticheat: anticheat,
        problems,
        allowed: false,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KernelStage {
    Idle,
    Backup,
    Loaded,
    Applied,
    Unloaded,
    Restored,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KernelAction {
    Backup,
    Load,
    Apply,
    Unload,
    Restore,
}

/// Pure state machine backing the load -> apply -> unload -> restore lifecycle
/// with a mandatory backup. Invalid transitions are rejected rather than
/// silently producing an unrecoverable state.
pub fn next_stage(stage: KernelStage, action: KernelAction) -> Option<KernelStage> {
    use KernelAction as A;
    use KernelStage as S;
    match (stage, action) {
        (S::Idle, A::Backup) => Some(S::Backup),
        (S::Backup, A::Load) => Some(S::Loaded),
        (S::Loaded, A::Apply) => Some(S::Applied),
        (S::Loaded | S::Applied, A::Unload) => Some(S::Unloaded),
        (S::Unloaded | S::Backup, A::Restore) => Some(S::Restored),
        _ => None,
    }
}

/// Attempts the kernel path. Always refused: there is no driver to load and
/// the guard report is returned to the caller for a human decision.
pub fn request_kernel_access() -> Result<(), GuardReport> {
    Err(preflight())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_requires_backup_before_load() {
        assert_eq!(
            next_stage(KernelStage::Idle, KernelAction::Backup),
            Some(KernelStage::Backup)
        );
        assert_eq!(
            next_stage(KernelStage::Backup, KernelAction::Load),
            Some(KernelStage::Loaded)
        );
        assert_eq!(
            next_stage(KernelStage::Loaded, KernelAction::Apply),
            Some(KernelStage::Applied)
        );
        assert_eq!(
            next_stage(KernelStage::Applied, KernelAction::Unload),
            Some(KernelStage::Unloaded)
        );
        assert_eq!(
            next_stage(KernelStage::Unloaded, KernelAction::Restore),
            Some(KernelStage::Restored)
        );
        assert_eq!(next_stage(KernelStage::Idle, KernelAction::Load), None);
        assert_eq!(next_stage(KernelStage::Idle, KernelAction::Apply), None);
    }

    #[test]
    fn kernel_access_is_always_refused() {
        let report = request_kernel_access().unwrap_err();
        assert!(!report.allowed);
        assert!(report
            .problems
            .iter()
            .any(|p| p.contains("no signed kernel driver")));
    }
}
