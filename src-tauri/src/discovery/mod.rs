//! Bounded, read-only workspace discovery. Discovery never authorizes execution.

/// Candidate display semantics are consumed by the desktop session coordinator. The
/// module stays compiled and tested without the desktop feature so its evidence rules
/// are covered by the base test run; `dead_code` is allowed only in that configuration.
#[cfg_attr(not(feature = "desktop"), allow(dead_code))]
pub(crate) mod classification;
/// Live agent context registrations bound to a docked target by process ancestry.
#[cfg_attr(not(feature = "desktop"), allow(dead_code))]
pub(crate) mod context_source;
/// Windows-only parent-PID snapshot for the ancestry binding. Compiled for every
/// Windows build: the ancestry rule runs in the base test configuration too.
#[cfg(windows)]
pub(crate) mod context_source_windows;
mod inspector;
pub mod registration;
mod resolver;
#[cfg(all(windows, feature = "desktop"))]
pub(crate) mod windows_tabs;

pub use inspector::{inspect_directory, path_environment, project_id};
pub use resolver::{
    automatic_candidate, current_active_evidence, current_target_evidence, resolution_complete,
    resolve, resolve_with_sources, TitleResolver, WorkspaceResolver,
};

use crate::ipc::model::{Environment, Evidence};

/// Registration and active-scope evidence expires quickly; timestamps are Unix milliseconds.
pub const MAX_EVIDENCE_TTL_MS: u64 = 120_000;

/// A workspace observation is only trustworthy inside this window; the frontend and the
/// backend both use this single bound.
pub const WORKSPACE_OBSERVATION_TTL_MS: u64 = 15_000;

/// Candidates are constructed by read-only inspection before display semantics exist.
/// [`classification::apply`] always replaces this placeholder before a snapshot is
/// published, so a partially built candidate can never reach the frontend with a
/// guessed category.
pub(crate) use classification::PENDING;

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
