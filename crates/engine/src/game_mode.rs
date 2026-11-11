//! Game Mode manager: tracks which background processes are currently
//! isolated and applies/removes CPU Sets + memory priority + EcoQoS.

use crate::cpu_isolation;
use crate::error::EngineError;
use crate::helper;
use crate::process::ProcessEntry;
use std::collections::{HashMap, HashSet};

/// Tracks isolated background processes across ticks.
pub struct GameModeManager {
    /// pid -> exe name of currently isolated processes.
    managed: HashMap<u32, String>,
    /// CPU set ids used for background isolation (E-cores).
    background_cpu_set_ids: Vec<u32>,
    /// Quick toggle (UI / tray): when off, ticks are skipped and existing
    /// isolation is torn down.
    enabled: bool,
}

impl GameModeManager {
    /// Builds a manager from the enumerated system cores (E-core ids only).
    pub fn new() -> Result<Self, EngineError> {
        let cores = cpu_isolation::enumerate_cpu_sets()?;
        let mut background_cpu_set_ids = cpu_isolation::e_core_ids(&cores);
        if background_cpu_set_ids.is_empty() {
            // Non-hybrid machine: fall back to all cores (isolation is then a
            // no-op, but memory priority and EcoQoS still apply).
            background_cpu_set_ids = cores.iter().map(|c| c.cpu_set_id).collect();
        }
        Ok(Self {
            managed: HashMap::new(),
            background_cpu_set_ids,
            // Game Mode stays OFF until the user toggles it: applying CPU Set
            // isolation to background processes must never happen silently at
            // startup without explicit consent.
            enabled: false,
        })
    }

    /// Whether automatic Game Mode management is currently on.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// UI quick toggle. Turning off tears down all existing isolation.
    pub fn set_enabled(&mut self, on: bool, on_error: &mut dyn FnMut(u32, &EngineError)) {
        if self.enabled && !on {
            self.reset(on_error);
        }
        self.enabled = on;
    }

    pub fn managed_pids(&self) -> Vec<u32> {
        self.managed.keys().copied().collect()
    }

    /// Applies isolation to newly appeared background processes and removes
    /// isolation from disappeared ones. Per-process errors are reported
    /// through `on_error` and never abort the tick.
    pub fn tick(
        &mut self,
        candidates: &[ProcessEntry],
        on_error: &mut dyn FnMut(u32, &EngineError),
    ) {
        if !self.enabled {
            return;
        }
        let current: HashMap<u32, String> = candidates
            .iter()
            .map(|p| (p.pid, p.exe_name.clone()))
            .collect();

        // Apply to new candidates.
        for (pid, name) in &current {
            if self.managed.contains_key(pid) {
                continue;
            }
            if let Err(e) = helper::apply_isolation(*pid, &self.background_cpu_set_ids) {
                on_error(*pid, &e);
            } else {
                self.managed.insert(*pid, name.clone());
            }
        }

        // Remove for disappeared processes.
        let gone: Vec<u32> = self
            .managed
            .keys()
            .filter(|pid| !current.contains_key(pid))
            .copied()
            .collect();
        for pid in gone {
            if let Err(e) = helper::remove_isolation(pid) {
                // Process is gone anyway; the OS cleaned up its settings.
                on_error(pid, &e);
            }
            self.managed.remove(&pid);
        }
    }

    /// Removes isolation from all managed processes (Game Mode OFF).
    pub fn reset(&mut self, on_error: &mut dyn FnMut(u32, &EngineError)) {
        for pid in self.managed.keys().copied().collect::<Vec<u32>>() {
            if let Err(e) = helper::remove_isolation(pid) {
                on_error(pid, &e);
            }
        }
        self.managed.clear();
    }

    /// Externally applied exclusions (e.g. the game itself) to keep in sync.
    pub fn is_managed(&self, pid: u32) -> bool {
        self.managed.contains_key(&pid)
    }

    /// The set of cpu set ids used for isolation (for telemetry).
    pub fn background_cpu_set_ids(&self) -> HashSet<u32> {
        self.background_cpu_set_ids.iter().copied().collect()
    }
}
