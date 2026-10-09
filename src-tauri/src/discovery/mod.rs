//! Bounded, read-only workspace discovery. Discovery never authorizes execution.

mod inspector;
pub mod registration;
mod resolver;
#[cfg(all(windows, feature = "desktop"))]
pub(crate) mod windows_tabs;

pub use inspector::{inspect_directory, path_environment, project_id};
pub use resolver::{
    automatic_candidate, resolve, resolve_with_sources, TitleResolver, WorkspaceResolver,
};

use crate::ipc::model::{Environment, Evidence};

/// Registration and active-scope evidence expires quickly; timestamps are Unix milliseconds.
pub const MAX_EVIDENCE_TTL_MS: u64 = 120_000;

pub(crate) fn environment_name(environment: &Environment) -> &'static str {
    match environment {
        Environment::WindowsLocal => "windowsLocal",
        Environment::Wsl => "wsl",
        Environment::Remote => "remote",
    }
}

pub(crate) fn fresh(evidence: &Evidence, now: u64) -> bool {
    evidence.observed_at <= now
        && evidence.expires_at > now
        && evidence.expires_at > evidence.observed_at
        && evidence.expires_at - evidence.observed_at <= MAX_EVIDENCE_TTL_MS
}
