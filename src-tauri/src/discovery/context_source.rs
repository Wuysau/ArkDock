//! Binds live agent context registrations to a docked target by process ancestry.
//!
//! A [`crate::context::ContextEntry`] is *context*, not authority. This resolver turns
//! it into discovery evidence only when the backend can prove the relationship:
//!
//! - the registration client is alive with the recorded start time (no PID reuse);
//! - the client process is an ancestor-bound descendant of the docked target window's
//!   process (bounded walk), so the registration belongs to *this* target;
//! - every path remains a source declaration, including host `roots` declarations.
//!
//! Neither an app-level MCP connection nor a descendant process proves which
//! conversation is visible. All registrations remain weak suggestions until the
//! user explicitly selects a backend-issued registration in the association flow.

use std::sync::Arc;

use crate::context::ContextHub;
use crate::ipc::model::{Evidence, EvidenceSource, TargetInfo};

use super::resolver::WorkspaceResolver;
use super::MAX_EVIDENCE_TTL_MS;

/// Evidence lives exactly as long as the heartbeat that supports it.
#[cfg(test)]
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
            if !client_alive(&target.process) {
                return Ok(Vec::new());
            }
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
        let paths: Vec<&str> = if let Some(directory) = entry.execution_directory.as_deref() {
            vec![directory]
        } else if entry.has_root_declaration() {
            entry.declared_roots().iter().map(String::as_str).collect()
        } else {
            entry.cwd.as_deref().into_iter().collect()
        };
        for root in paths {
            if root.is_empty() || root.len() > 4096 || root.contains('\0') {
                continue;
            }
            let environment = entry.environment_for_path(root);
            evidence.push(Evidence {
                source: EvidenceSource::Registration,
                target_id: Some(target.id.clone()),
                process: Some(target.process.clone()),
                session_id: Some(format!("context:{}", entry.registration_id)),
                observed_at: entry.observed_at,
                expires_at: entry.expires_at,
                environment,
                raw_path: root.to_owned(),
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
    use crate::ipc::model::{Candidate, Environment, ProcessRef, ProjectRef};

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
            connection_id: 1,
            sequence: 1,
            registration_id: "1-1".into(),
            roots,
            cwd: None,
            agent: None,
            host_session: None,
            original_project: None,
            execution_directory: None,
            environment: None,
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
    fn descendant_root_registration_never_proves_the_visible_conversation() {
        let proven = vec![(entry("session-abc-123", 7, vec!["C:\\app".into()]), true)];
        let evidence = evidence_for_target(&proven, &target(3), 1_500);
        assert_eq!(evidence.len(), 1);
        assert!(!evidence[0].verifiable);
        assert!(!evidence[0].active_scope);
        assert_eq!(evidence[0].source, EvidenceSource::Registration);
        assert_eq!(evidence[0].target_id.as_deref(), Some("target-1"));
        assert_eq!(evidence[0].process.as_ref().map(|p| p.pid), Some(3));
        assert_eq!(evidence[0].session_id.as_deref(), Some("context:1-1"));
        assert_eq!(evidence[0].raw_path, "C:\\app");
        assert_eq!(
            evidence[0].expires_at - evidence[0].observed_at,
            CONTEXT_EVIDENCE_TTL_MS
        );
        assert!(fresh(&evidence[0], 1_500));
        // A background process's roots can never drive automatic association.
        let candidate = candidate_from(&evidence[0], "C:\\app");
        assert_eq!(
            super::super::automatic_candidate(std::slice::from_ref(&candidate), &target(3), 1_500),
            None
        );
    }

    #[test]
    fn app_level_connections_and_weak_hints_never_claim_the_active_workspace() {
        // The client IS the window process: app-level MCP, many conversations.
        let proven = vec![(entry("session-abc-123", 3, vec!["C:\\app".into()]), true)];
        let evidence = evidence_for_target(&proven, &target(3), 1_500);
        assert_eq!(evidence.len(), 1);
        assert!(
            !evidence[0].verifiable,
            "host context cannot prove a visible tab"
        );
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
        assert!(evidence.iter().all(|item| !item.verifiable));
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
    fn evidence_uses_execution_directory_and_never_extends_the_registration_lease() {
        let mut source = entry("session-abc-123", 7, vec!["C:\\original".into()]);
        source.original_project = Some("C:\\original".into());
        source.execution_directory = Some("C:\\worktree".into());
        let evidence =
            evidence_for_target(&[(source.clone(), true)], &target(3), source.expires_at - 1);
        assert_eq!(evidence.len(), 1);
        assert_eq!(evidence[0].raw_path, "C:\\worktree");
        assert_eq!(evidence[0].expires_at, source.expires_at);
        assert!(!fresh(&evidence[0], source.expires_at));
        source.execution_directory = Some("/home/worktree".into());
        source.environment = Some(Environment::Wsl);
        let evidence = evidence_for_target(&[(source, true)], &target(3), 1500);
        assert_eq!(evidence[0].raw_path, "/home/worktree");
        assert_eq!(evidence[0].environment, Environment::Wsl);
    }
}
