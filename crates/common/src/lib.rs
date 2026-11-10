//! Shared types for the flux-fps workspace: telemetry snapshots, commands,
//! configuration and the per-PC profile (produced by flux-fps-master).

pub mod command;
pub mod config;
pub mod profile;
pub mod telemetry;

pub use command::Command;
pub use config::Config;
pub use profile::Profile;
pub use telemetry::{CoreInfo, FrametimeStats, TelemetrySnapshot};

/// Workspace-wide semantic version, baked into every binary.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
