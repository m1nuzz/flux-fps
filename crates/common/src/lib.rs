//! Shared types for the flux-fps workspace: telemetry snapshots, commands,
//! configuration, the elevated-helper IPC protocol and the per-PC profile
//! (produced by flux-fps-master).

pub mod command;
pub mod config;
pub mod ipc;
pub mod profile;
pub mod telemetry;

pub use command::Command;
pub use config::{normalize_whitelist_entry, Config};
pub use ipc::{
    pipe_path, HelperReply, HelperRequest, MAX_MESSAGE_BYTES, PIPE_NAME_PREFIX, PROTOCOL_VERSION,
};
pub use profile::Profile;
pub use telemetry::{CoreInfo, FrametimeStats, PacerAdvice, TelemetrySnapshot};

/// Workspace-wide semantic version, baked into every binary.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
