//! Binds live agent context registrations to a docked target by process ancestry.
//!
//! A [`crate::context::ContextEntry`] is *context*, not authority. This resolver turns
//! it into discovery evidence only when the backend can prove the relationship:
//!
//! - the registration client is alive with the recorded start time (no PID reuse);
//! - the client process is an ancestor-bound descendant of the docked target window's
//!   process (bounded walk), so the registration belongs to *this* target;
//! - the workspace path is a host `roots` declaration. A spawn-time working directory
//!   is only a weak hint and can never prove the active workspace on its own.
//!
//! Strength is decided here, never taken from the client: `verifiable` reflects the
//! OS-proven connection identity plus the host root declaration; `active_scope`
//! additionally requires the registration to be conversation-scoped. An MCP client
//! that *is* the target window's process (an app-level connection serving many
//! conversations) never proves which conversation is visible, so it stays a
//! suggestion. Multiple concurrent sessions or roots fail closed: the existing
//! uniqueness gates in `automatic_candidate` then require an explicit choice.

use std::sync::Arc;

use crate::context::ContextHub;
use crate::ipc::model::{Environment, Evidence, EvidenceSource, TargetInfo};

use super::resolver::WorkspaceResolver;
use super::{path_environment, MAX_EVIDENCE_TTL_MS};

/// Evidence lives exactly as long as the heartbeat that supports it.
pub const CONTEXT_EVIDENCE_TTL_MS: u64 = crate::context::HEARTBEAT_TTL_MS;

/// Bounded ancestry walk; deeper chains are rejected instead of guessed.
pub const ANCESTRY_MAX_DEPTH: usize = 8;

pub struct ContextSourceResolver {
    hub: Arc<ContextHub>,
}

impl ContextSourceResolver {
    pub fn new(hub: Arc<ContextHub>) -> Self {
        Self { hub }
    }
}

impl WorkspaceResolver for ContextSourceResolver {
    fn evidence(&self, target: &TargetInfo, now: u64) -> Result<Vec<Evidence>, String> {
        #[cfg(windows)]
        {
            let entries = self.hub.live(now);
            if entries.is_empty() {
                return Ok(Vec::new());
            }
            let mut proven: Vec<(crate::context::ContextEntry, bool)> = Vec::new();
            {
                let parents = super::context_source_windows::parent_map();
                for entry in entries {
                    if !client_alive(&entry.client) {
                        continue;
                    }
                    let lineage = match parents.as_ref() {
                        Some(map) => ancestry_binds_with(
                            entry.client.pid,
                            target.process.pid,
                            &|pid: u32| map.get(&pid).copied(),
                        ),
                        None => false,
                    };
                    proven.push((entry, lineage));
                }
            }
            Ok(evidence_for_target(&proven, target, now))
        }
        #[cfg(not(windows))]
        {
            // Ancestry cannot be proven off-Windows; registrations stay context-only.
            let _ = (&self.hub, target, now);
            Ok(Vec::new())
        }
    }
}

#[cfg(windows)]
fn client_alive(client: &crate::ipc::model::ProcessRef) -> bool {
    super::registration::process_started_at(client.pid)
        .map(|started_at| started_at.to_string())
        .is_ok_and(|started_at| started_at == client.started_at)
}

/// Whether `descendant` is `ancestor` itself or one of its bounded descendants,
/// according to the injected parent function. Pure and platform-independent so the
/// binding rule is tested without real processes.
pub fn ancestry_binds_with(
    descendant: u32,
    ancestor: u32,
    parent_of: &dyn Fn(u32) -> Option<u32>,
) -> bool {
    let mut current = descendant;
    for _ in 0..ANCESTRY_MAX_DEPTH {
        if current == ancestor {
            return true;
        }
        match parent_of(current) {
            Some(parent) if parent != current => current = parent,
            _ => return false,
        }
    }
    false
}

/// The evidence one target derives from its proven registrations. Pure: the same rule
/// runs on Windows resolution and in the cross-platform test suite.
pub fn evidence_for_target(
    proven: &[(crate::context::ContextEntry, bool)],
    target: &TargetInfo,
    now: u64,
) -> Vec<Evidence> {
    let mut evidence = Vec::new();
    for (entry, lineage) in proven {
        if !fresh_of(entry, now) {
            continue;
        }
        // A registration bound to another window's process tree never surfaces as
        // this target's evidence; it belongs to whichever target its lineage matches.
        if !*lineage {
            continue;
        }
        // Conversation scope requires an own agent process inside this target's tree.
        // A registration whose client *is* the window process is an app-level
        // connection; it carries real context but cannot prove the visible conversation.
        let session_scoped = *lineage && entry.client.pid != target.process.pid;
        if entry.has_root_declaration() {
            for root in entry.declared_roots() {
                if root.is_empty() || root.len() > 4096 || root.contains('\0') {
                    continue;
                }
                let environment = path_environment(std::path::Path::new(root));
                // A declared root is a host workspace claim: OS-proven connection plus
                // the host's own declaration. Only the conversation-scope gate and the
                // environment decide whether it can drive automatic association.
                // One session declaring several workspaces is ambiguous: fail closed to
                // a suggestion instead of letting either root claim the session.
                let active_scope = session_scoped
                    && entry.declared_roots().len() == 1
                    && matches!(environment, Environment::WindowsLocal);
                evidence.push(Evidence {
                    source: EvidenceSource::Registration,
                    target_id: Some(target.id.clone()),
                    process: Some(target.process.clone()),
                    session_id: Some(entry.session.clone()),
                    observed_at: now,
                    expires_at: now.saturating_add(CONTEXT_EVIDENCE_TTL_MS),
                    environment,
                    raw_path: root.clone(),
                    verifiable: true,
                    active_scope,
                });
            }
        } else if let Some(cwd) = entry.cwd.as_ref() {
            // A spawn-time working directory is a hint from the connection, never a
            // host claim about the current conversation's workspace.
            let environment = path_environment(std::path::Path::new(cwd));
            evidence.push(Evidence {
                source: EvidenceSource::Registration,
                target_id: Some(target.id.clone()),
                process: Some(target.process.clone()),
                session_id: Some(entry.session.clone()),
                observed_at: now,
                expires_at: now.saturating_add(CONTEXT_EVIDENCE_TTL_MS),
                environment,
                raw_path: cwd.clone(),
                verifiable: false,
                active_scope: false,
            });
        }
    }
    evidence
}

fn fresh_of(entry: &crate::context::ContextEntry, now: u64) -> bool {
    entry.observed_at <= now
        && entry.expires_at > now
        && entry.expires_at - entry.observed_at <= MAX_EVIDENCE_TTL_MS
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::ContextEntry;
    use crate::discovery::fresh;
    use crate::ipc::model::{Candidate, ProcessRef, ProjectRef};

    /// Builds a minimal valid candidate around one evidence record so the decision
    /// core (`automatic_candidate`) can be asserted end-to-end without real paths.
    fn candidate_from(evidence: &Evidence, root: &str) -> Candidate {
        let id = super::super::project_id(&evidence.environment, root, "ctx-app", "default");
        Candidate {
            id: id.clone(),
            project: ProjectRef {
                id,
                name: "ctx-app".into(),
                environment: evidence.environment.clone(),
                workspace_root: root.into(),
                app_root: root.into(),
                module: "entry".into(),
                variant: "default".into(),
                sdk_id: None,
            },
            evidence: vec![evidence.clone()],
            valid: true,
            auto_eligible: false,
            reasons: Vec::new(),
            category: crate::discovery::PENDING,
            category_expires_at: 0,
        }
    }

    fn target(pid: u32) -> TargetInfo {
        TargetInfo {
            id: "target-1".into(),
            title: "Terminal".into(),
            app: "terminal".into(),
            process: ProcessRef {
                pid,
                started_at: "90".into(),
            },
        }
    }

    fn entry(session: &str, client_pid: u32, roots: Vec<String>) -> ContextEntry {
        ContextEntry {
            session: session.into(),
            client: ProcessRef {
                pid: client_pid,
                started_at: "1000".into(),
            },
            roots,
            cwd: None,
            agent: None,
            observed_at: 1_000,
            expires_at: 1_000 + CONTEXT_EVIDENCE_TTL_MS,
        }
    }

    /// parent_of: 7 -> 5 -> 3 -> 1(root); 9 is a root process; 11 -> 1.
    fn parent_of(pid: u32) -> Option<u32> {
        match pid {
            7 => Some(5),
            5 => Some(3),
            3 => Some(1),
            11 => Some(1),
            _ => None,
        }
    }

    #[test]
    fn ancestry_walk_binds_descendants_within_the_depth_limit() {
        assert!(ancestry_binds_with(7, 3, &parent_of));
        assert!(ancestry_binds_with(7, 7, &parent_of));
        assert!(!ancestry_binds_with(9, 3, &parent_of));
        // Deeper than the bound fails closed even when a longer chain would match.
        let deep = |pid: u32| Some(pid + 1);
        assert!(!ancestry_binds_with(
            1,
            1 + ANCESTRY_MAX_DEPTH as u32,
            &deep
        ));
        let cyclic = |pid: u32| if pid == 2 { Some(1) } else { Some(2) };
        assert!(!ancestry_binds_with(1, 3, &cyclic));
    }

    #[test]
    fn a_unique_session_scoped_root_declaration_becomes_strong_evidence() {
        let proven = vec![(entry("session-abc-123", 7, vec!["C:\\app".into()]), true)];
        let evidence = evidence_for_target(&proven, &target(3), 1_500);
        assert_eq!(evidence.len(), 1);
        assert!(evidence[0].verifiable);
        assert!(evidence[0].active_scope);
        assert_eq!(evidence[0].source, EvidenceSource::Registration);
        assert_eq!(evidence[0].target_id.as_deref(), Some("target-1"));
        assert_eq!(evidence[0].process.as_ref().map(|p| p.pid), Some(3));
        assert_eq!(evidence[0].session_id.as_deref(), Some("session-abc-123"));
        assert_eq!(evidence[0].raw_path, "C:\\app");
        assert_eq!(
            evidence[0].expires_at - evidence[0].observed_at,
            CONTEXT_EVIDENCE_TTL_MS
        );
        assert!(fresh(&evidence[0], 1_500));
        // And the existing decision core accepts it as the automatic candidate.
        let candidate = candidate_from(&evidence[0], "C:\\app");
        assert_eq!(
            super::super::automatic_candidate(std::slice::from_ref(&candidate), &target(3), 1_500),
            Some(candidate.id.clone())
        );
    }

    #[test]
    fn app_level_connections_and_weak_hints_never_claim_the_active_workspace() {
        // The client IS the window process: app-level MCP, many conversations.
        let proven = vec![(entry("session-abc-123", 3, vec!["C:\\app".into()]), true)];
        let evidence = evidence_for_target(&proven, &target(3), 1_500);
        assert_eq!(evidence.len(), 1);
        assert!(evidence[0].verifiable, "host context stays verifiable");
        assert!(
            !evidence[0].active_scope,
            "app-level context is not conversation scope"
        );

        // A cwd-only hint is weak even when session scoped.
        let mut hint = entry("session-abc-123", 7, Vec::new());
        hint.cwd = Some("C:\\fallback".into());
        let evidence = evidence_for_target(&[(hint, true)], &target(3), 1_500);
        assert_eq!(evidence.len(), 1);
        assert!(!evidence[0].verifiable);
        assert!(!evidence[0].active_scope);
    }

    #[test]
    fn foreign_lineage_ambiguous_roots_and_non_windows_roots_fail_closed() {
        // Not this target's process tree: nothing is emitted for this target.
        let proven = vec![(entry("session-abc-123", 9, vec!["C:\\app".into()]), false)];
        assert!(evidence_for_target(&proven, &target(3), 1_500).is_empty());

        // Two declared roots in one session: real context, but no single active scope.
        let proven = vec![(
            entry(
                "session-abc-123",
                7,
                vec!["C:\\app".into(), "C:\\other".into()],
            ),
            true,
        )];
        let evidence = evidence_for_target(&proven, &target(3), 1_500);
        assert_eq!(evidence.len(), 2);
        assert!(evidence.iter().all(|item| item.verifiable));
        assert!(evidence.iter().all(|item| !item.active_scope));

        // A WSL root keeps its environment and can never drive Windows association.
        let proven = vec![(
            entry(
                "session-abc-123",
                7,
                vec![r"\\wsl.localhost\Ubuntu\app".into()],
            ),
            true,
        )];
        let evidence = evidence_for_target(&proven, &target(3), 1_500);
        assert_eq!(evidence[0].environment, Environment::Wsl);
        assert!(!evidence[0].active_scope);
    }

    #[test]
    fn expired_entries_are_dropped_even_before_the_hub_prunes() {
        let mut stale = entry("session-abc-123", 7, vec!["C:\\app".into()]);
        stale.expires_at = 1_400;
        let evidence = evidence_for_target(&[(stale, true)], &target(3), 1_500);
        assert!(evidence.is_empty());
    }

    #[test]
    fn parent_maps_are_bounded_snapshots() {
        // The Windows-only snapshot helper stays optional: resolution treats a missing
        // snapshot as "no lineage", never as "assume lineage".
        #[cfg(all(windows, feature = "desktop"))]
        {
            let map = super::super::context_source_windows::parent_map();
            if let Some(map) = map.as_ref() {
                assert!(map.iter().take(64).all(|(pid, parent)| pid != parent));
            }
        }
    }
}
