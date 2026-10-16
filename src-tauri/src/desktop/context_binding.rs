//! Explicit context selection and lease enforcement. A context is a suggestion,
//! never an assertion about the conversation visible in a host window.
use super::*;
use crate::context::ContextEntry;

pub(super) fn matches_project(entry: &ContextEntry, project: &ProjectRef) -> bool {
    let matches = |path: &str| {
        entry.environment_for_path(path) == project.environment
            && association::workspace_path_matches(project, path)
    };
    if let Some(path) = entry.execution_directory.as_deref() {
        return matches(path);
    }
    if !entry.roots.is_empty() {
        return entry.roots.iter().any(|root| matches(root));
    }
    entry.cwd.as_deref().is_some_and(matches)
}

fn binding_is_live(binding: &ContextEntry, live: &[ContextEntry]) -> bool {
    live.iter().any(|entry| {
        entry.registration_id == binding.registration_id
            && entry.session == binding.session
            && entry.client == binding.client
    })
}

pub(super) fn binding_for_selection(
    source: AssociationSource,
    previous: Option<&ContextEntry>,
    candidates: &[Candidate],
    project: &ProjectRef,
    live: &[ContextEntry],
) -> Result<Option<ContextEntry>, String> {
    // A successful independent directory selection intentionally opts out of a
    // context lease. Read-only inspection never calls this or changes the old pin.
    if source == AssociationSource::Directory {
        return Ok(None);
    }
    if let Some(previous) = previous.filter(|entry| matches_project(entry, project)) {
        if !binding_is_live(previous, live) {
            return Err("STALE_SESSION".into());
        }
        return Ok(Some(previous.clone()));
    }
    if source == AssociationSource::Candidate {
        return selection_binding(candidates, project, live);
    }
    Ok(None)
}

/// A click on a normal candidate must not turn context evidence into an
/// untracked manual directory. Keep the exact backend generation or ask the user
/// to disambiguate the registration cards; never guess the most recent session.
pub(super) fn selection_binding(
    candidates: &[Candidate],
    project: &ProjectRef,
    live: &[ContextEntry],
) -> Result<Option<ContextEntry>, String> {
    let mut ids = Vec::new();
    for evidence in candidates
        .iter()
        .filter(|candidate| candidate.project.id == project.id)
        .flat_map(|candidate| &candidate.evidence)
    {
        if evidence.source != EvidenceSource::Registration {
            continue;
        }
        if let Some(id) = evidence
            .session_id
            .as_deref()
            .and_then(|session| session.strip_prefix("context:"))
        {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
    }
    if ids.is_empty() {
        return Ok(None);
    }
    let mut matches = live.iter().filter(|entry| {
        ids.contains(&entry.registration_id.as_str()) && matches_project(entry, project)
    });
    let entry = matches
        .next()
        .ok_or("STALE_SESSION: this context candidate expired; select a current registration")?;
    if matches.next().is_some() {
        return Err("WORKSPACE_CONFLICT: multiple sessions suggest this project; choose its Agent registration card".into());
    }
    Ok(Some(entry.clone()))
}

impl Engine {
    pub(super) fn context_status(&self) -> ContextSourceStatus {
        let events = self.context.last_event();
        let entries = self.context.live(now());
        ContextSourceStatus {
            paired: self.context.paired(),
            active_sessions: entries.len() as u32,
            last_event_at: events.as_ref().map(|(_, at)| *at).unwrap_or(0),
            reason: events.map(|(reason, _)| reason),
            bound_registration_id: self
                .context_binding
                .as_ref()
                .map(|entry| entry.registration_id.clone()),
            registrations: entries
                .iter()
                .map(|entry| ContextRegistration {
                    id: entry.registration_id.clone(),
                    session: entry.session.clone(),
                    host_session: entry.host_session.clone(),
                    agent: entry.agent.clone(),
                    original_project: entry.original_project.clone(),
                    execution_directory: entry.execution_directory.clone(),
                    suggested_directory: entry
                        .execution_directory
                        .is_none()
                        .then(|| entry.effective_execution_directory().map(str::to_owned))
                        .flatten(),
                    hint_source: entry.execution_directory.is_none().then(|| {
                        if entry.has_root_declaration() {
                            "roots".into()
                        } else {
                            "startupDirectory".into()
                        }
                    }),
                    environment: entry.execution_environment(),
                })
                .collect(),
        }
    }

    // This runs before watcher/build/frame events, including while untrusted or
    // stopped: a context lease must not depend on a preview SessionRef existing.
    pub(super) fn sync_context_binding(&mut self) {
        let live = self.context.live(now());
        if self
            .context_binding
            .as_ref()
            .is_some_and(|binding| !binding_is_live(binding, &live))
        {
            // Transport loss preserves disconnected task evidence for an
            // explicit retry. It must not be treated as a user project revoke.
            self.replace_preview_association(None);
            self.context_confirmation_required = true;
            self.snapshot.candidates.clear();
            self.snapshot.phase = if self.snapshot.target.is_some() {
                Phase::SelectProject
            } else {
                Phase::Idle
            };
            self.snapshot.workspace_observation = None;
            self.diagnostic(
                "CONTEXT_BINDING_ENDED",
                "连接的会话已断开、撤销或更换工程，请重新确认连接。".into(),
                CheckStatus::Blocked,
            );
            self.publish();
        } else {
            let status = self.context_status();
            if self.snapshot.context_source.as_ref() != Some(&status) {
                self.publish();
            }
        }
    }

    pub(super) fn require_live_context_binding(&mut self) -> Result<(), String> {
        let was_bound = self.context_binding.is_some();
        self.sync_context_binding();
        if was_bound && self.context_binding.is_none() {
            return Err(
                "STALE_SESSION: the selected context changed; reconnect its project".into(),
            );
        }
        Ok(())
    }

    pub(super) fn select_context_project(&mut self, id: &str) -> Result<(), String> {
        self.current_target()?;
        let entry = self
            .context
            .live(now())
            .into_iter()
            .find(|entry| entry.registration_id == id)
            .ok_or("STALE_SESSION: this context registration is no longer available")?;
        if self.context_binding.as_ref().is_some_and(|current| {
            current.registration_id == entry.registration_id
                && current.client == entry.client
                && current.session == entry.session
        }) && self
            .snapshot
            .project
            .as_ref()
            .is_some_and(|project| matches_project(&entry, project))
        {
            // Confirmation of the same live association is idempotent. Do not
            // restart Previewer or expire pending tasks by replacing its epoch.
            return Ok(());
        }
        if entry.execution_environment() != Environment::WindowsLocal {
            return Err("UNSUPPORTED_ENVIRONMENT: WSL/remote context is retained but native preview is unavailable".into());
        }
        let path = absolute_directory_input(entry.effective_execution_directory().ok_or(
            "INVALID_DIRECTORY_PATH: choose a context with an explicit execution directory",
        )?)?;
        // Confirming a replacement MCP connection is a context handover, not
        // an explicit Detach. Preserve disconnected task evidence until the
        // resolved project can be compared or the user explicitly retries it.
        self.replace_preview_association(None);
        self.snapshot.candidates.clear();
        self.snapshot.workspace_observation = None;
        self.context_binding = Some(entry);
        if let Err(error) = self.inspect_manual_directory(path, true, AssociationSource::Context) {
            self.close_session();
            self.snapshot.phase = Phase::SelectProject;
            return Err(error);
        }
        self.require_live_context_binding()
    }
}

#[tauri::command]
pub(crate) async fn context_select_project(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
    scope: RequestScope,
    registration_id: String,
) -> Result<AppSnapshot, IpcError> {
    if registration_id.is_empty() || registration_id.len() > 128 {
        return Err("INVALID_CANDIDATE".into());
    }
    request(
        window,
        service,
        scope,
        Action::ContextSelect(registration_id),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{parse_message, HEARTBEAT_TTL_MS};

    fn register(
        hub: &ContextHub,
        session: &str,
        connection: u64,
        sequence: u64,
        kind: &str,
        execution: &str,
        at: u64,
    ) {
        let message = serde_json::json!({
            "kind": kind, "token": "aa".repeat(32), "session": session, "sequence": sequence,
            "roots": [], "originalProject": "C:\\repo", "executionDirectory": execution,
            "environment": "windowsLocal", "hostSession": "host-chat"
        });
        hub.apply(
            parse_message(&message.to_string()).unwrap(),
            ProcessRef {
                pid: 10,
                started_at: "1".into(),
            },
            connection,
            at,
        )
        .unwrap();
    }

    #[test]
    fn selected_context_survives_heartbeat_but_not_background_replacement_or_material_update() {
        let hub = ContextHub::new();
        hub.activate_token([0xaa; 32]);
        register(
            &hub,
            "context-session-1",
            1,
            1,
            "hello",
            "C:\\worktree-a",
            100,
        );
        let bound = hub.live(101).remove(0);
        register(
            &hub,
            "context-session-2",
            2,
            1,
            "hello",
            "C:\\background",
            102,
        );
        assert!(binding_is_live(&bound, &hub.live(103)));
        let heartbeat = serde_json::json!({"kind":"heartbeat","token":"aa".repeat(32),"session":"context-session-1","sequence":2});
        hub.apply(
            parse_message(&heartbeat.to_string()).unwrap(),
            bound.client.clone(),
            1,
            104,
        )
        .unwrap();
        assert!(binding_is_live(&bound, &hub.live(105)));
        register(
            &hub,
            "context-session-1",
            1,
            3,
            "update",
            "C:\\worktree-b",
            106,
        );
        assert!(!binding_is_live(&bound, &hub.live(107)));
    }

    #[test]
    fn disconnect_expiry_and_reconnect_invalidate_a_pin_even_without_a_preview_session() {
        let hub = ContextHub::new();
        hub.activate_token([0xaa; 32]);
        register(
            &hub,
            "context-session-1",
            1,
            1,
            "hello",
            "C:\\worktree",
            100,
        );
        let bound = hub.live(101).remove(0);
        assert!(!binding_is_live(&bound, &hub.live(100 + HEARTBEAT_TTL_MS)));
        register(
            &hub,
            "context-session-1",
            2,
            2,
            "hello",
            "C:\\worktree",
            20_000,
        );
        assert!(!binding_is_live(&bound, &hub.live(20_001)));
        let reconnected = hub.live(20_001).remove(0);
        hub.remove_connection(2);
        assert!(!binding_is_live(&reconnected, &hub.live(20_002)));
    }

    #[test]
    fn normal_candidate_selection_keeps_context_lease_and_rejects_stale_or_ambiguous_sessions() {
        let hub = ContextHub::new();
        hub.activate_token([0xaa; 32]);
        register(
            &hub,
            "context-session-1",
            1,
            1,
            "hello",
            "C:\\worktree",
            100,
        );
        let entry = hub.live(101).remove(0);
        let project = ProjectRef {
            id: "project".into(),
            name: "project".into(),
            environment: Environment::WindowsLocal,
            workspace_root: "C:\\worktree".into(),
            app_root: "C:\\worktree".into(),
            module: "entry".into(),
            variant: "default".into(),
            sdk_id: None,
        };
        let evidence = |id: &str| Evidence {
            source: EvidenceSource::Registration,
            target_id: None,
            process: None,
            session_id: Some(format!("context:{id}")),
            observed_at: 100,
            expires_at: 15_100,
            environment: Environment::WindowsLocal,
            raw_path: "C:\\worktree".into(),
            verifiable: false,
            active_scope: false,
        };
        let mut candidate = Candidate {
            id: project.id.clone(),
            project: project.clone(),
            evidence: vec![evidence(&entry.registration_id)],
            valid: true,
            auto_eligible: false,
            reasons: Vec::new(),
            category: crate::discovery::PENDING,
            category_expires_at: 0,
        };
        assert_eq!(
            selection_binding(&[candidate.clone()], &project, &hub.live(101))
                .unwrap()
                .unwrap()
                .registration_id,
            entry.registration_id
        );
        assert!(selection_binding(&[candidate.clone()], &project, &[]).is_err());
        register(
            &hub,
            "context-session-2",
            2,
            1,
            "hello",
            "C:\\worktree",
            102,
        );
        let second = hub
            .live(103)
            .into_iter()
            .find(|item| item.session != entry.session)
            .unwrap();
        candidate.evidence.push(evidence(&second.registration_id));
        assert!(selection_binding(&[candidate], &project, &hub.live(103)).is_err());
    }

    #[test]
    fn directory_selection_is_independent_while_multi_root_candidates_keep_the_selected_lease() {
        let hub = ContextHub::new();
        hub.activate_token([0xaa; 32]);
        register(
            &hub,
            "context-session-1",
            1,
            1,
            "hello",
            "C:\\worktree",
            100,
        );
        let mut entry = hub.live(101).remove(0);
        let project = ProjectRef {
            id: "p".into(),
            name: "p".into(),
            environment: Environment::WindowsLocal,
            workspace_root: "C:\\worktree".into(),
            app_root: "C:\\worktree".into(),
            module: "entry".into(),
            variant: "default".into(),
            sdk_id: None,
        };
        assert!(binding_for_selection(
            AssociationSource::Directory,
            Some(&entry),
            &[],
            &project,
            &[]
        )
        .unwrap()
        .is_none());
        assert!(binding_for_selection(
            AssociationSource::Automatic,
            Some(&entry),
            &[],
            &project,
            &[]
        )
        .is_err());
        entry.execution_directory = None;
        entry.roots = vec!["C:\\elsewhere".into(), "C:\\worktree".into()];
        assert!(entry.effective_execution_directory().is_none());
        assert!(matches_project(&entry, &project));
        assert!(binding_for_selection(
            AssociationSource::Context,
            Some(&entry),
            &[],
            &project,
            &[entry.clone()]
        )
        .unwrap()
        .is_some());
    }
}
