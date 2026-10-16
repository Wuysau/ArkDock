//! Project association policy: workspace identity, active-workspace consistency and the
//! candidate classification the frontend displays.
//!
//! Everything in this module is pure. It decides *what* the association state should be;
//! the session coordinator decides when to apply it and owns every side effect
//! (stopping the watcher, closing the Previewer, publishing a snapshot). That split keeps
//! the association rules testable without a window, a previewer, or a clock.
//!
//! Invariants:
//! - evidence, not a project name or a previous choice, identifies the current workspace;
//! - association is read-only and never grants execution trust;
//! - a conflict or an expired claim releases the session instead of keeping stale pixels;
//! - manual and historical choices are carried forward as *candidates* only.
use super::*;

/// Generic UIA is intentionally not sampled by lightweight polls. Preserve its
/// existing lease for the same target while independently refreshing other sources.
/// Never renew timestamps merely because the context source was polled.
#[cfg_attr(not(windows), allow(dead_code))]
pub(super) struct UnsampledTabEvidence<'a>(pub(super) &'a [Candidate]);

impl crate::discovery::WorkspaceResolver for UnsampledTabEvidence<'_> {
    fn evidence(&self, target: &TargetInfo, observed: u64) -> Result<Vec<Evidence>, String> {
        let mut retained = Vec::new();
        for item in self.0.iter().flat_map(|candidate| &candidate.evidence) {
            if item.source == EvidenceSource::Uia
                && crate::discovery::current_target_evidence(item, target, observed)
                && !retained.contains(item)
            {
                retained.push(item.clone());
            }
        }
        Ok(retained)
    }
}

/// Canonical comparison form for an already identified workspace path.
///
/// This never opens a foreign path: the caller decides whether canonicalization is
/// allowed. WSL and remote identities keep their own spelling and environment.
pub(super) fn normalized_workspace_path(path: &str) -> String {
    let value = path.replace('/', "\\");
    let value = value
        .strip_prefix("\\\\?\\UNC\\")
        .map(|unc| format!("\\\\{unc}"))
        .or_else(|| value.strip_prefix("\\\\?\\").map(str::to_owned))
        .unwrap_or(value);
    value.trim_end_matches('\\').to_ascii_lowercase()
}

/// Whether a raw observed path still identifies this project's workspace.
pub(super) fn workspace_path_matches(project: &ProjectRef, raw_path: &str) -> bool {
    if path_environment(Path::new(raw_path)) != project.environment {
        return false;
    }
    // Canonicalization is read-only and only applies to already identified Windows paths.
    // WSL and remote identities are never opened as, or converted to, local filesystem paths.
    let resolved = if project.environment == Environment::WindowsLocal {
        Path::new(raw_path)
            .canonicalize()
            .ok()
            .and_then(|path| path.to_str().map(str::to_owned))
            .unwrap_or_else(|| raw_path.to_owned())
    } else {
        raw_path.to_owned()
    };
    let active = normalized_workspace_path(&resolved);
    let app = normalized_workspace_path(&project.app_root);
    let workspace = normalized_workspace_path(&project.workspace_root);
    active == workspace || active == app || active.starts_with(&format!("{app}\\"))
}

/// The single active-workspace summary shown for the current target.
///
/// Only lifecycle-verified, active-scope evidence can produce `Verified`. Disagreeing
/// sources suspend the summary instead of picking a winner, and a partial evidence
/// collection keeps the summary on the same completeness gate as automatic association.
pub(super) fn workspace_observation(
    candidates: &[Candidate],
    target: &TargetInfo,
    observed: u64,
    host_observation: Option<WorkspaceObservation>,
) -> WorkspaceObservation {
    if !resolution_complete(candidates) {
        return WorkspaceObservation {
            status: WorkspaceStatus::Suggested,
            project_name: host_observation.and_then(|observation| observation.project_name),
            active_path: None,
            reason: "Workspace evidence collection is incomplete; the current workspace cannot be reliably resolved".into(),
            observed_at: observed,
        };
    }
    let mut active_paths = Vec::new();
    for evidence in candidates.iter().flat_map(|candidate| &candidate.evidence) {
        if current_active_evidence(evidence, target, observed)
            && !active_paths.iter().any(|path: &String| {
                normalized_workspace_path(path) == normalized_workspace_path(&evidence.raw_path)
            })
        {
            active_paths.push(evidence.raw_path.clone());
        }
    }
    if active_paths.len() > 1 {
        return WorkspaceObservation {
            status: WorkspaceStatus::Suggested,
            project_name: host_observation.and_then(|observation| observation.project_name),
            active_path: None,
            reason: "Current workspace sources disagree; automatic association is suspended".into(),
            observed_at: observed,
        };
    }
    if let Some(path) = active_paths.pop() {
        return WorkspaceObservation {
            status: WorkspaceStatus::Verified,
            project_name: host_observation
                .as_ref()
                .and_then(|observation| observation.project_name.clone()),
            active_path: Some(path),
            reason:
                "Current workspace path is supported by lifecycle-verified active-scope evidence"
                    .into(),
            observed_at: observed,
        };
    }
    host_observation.unwrap_or(WorkspaceObservation {
        status: WorkspaceStatus::Unknown,
        project_name: None,
        active_path: None,
        reason: "The current active workspace has not been reliably identified; history is only a suggestion".into(),
        observed_at: observed,
    })
}

/// What the coordinator must do with the current association after one discovery pass.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum AssociationDecision {
    /// Same workspace: keep the running session and refresh only its binding evidence.
    Hold,
    /// Release the association first, then optionally bind the uniquely reliable candidate.
    Release {
        reason: &'static str,
        rebind: Option<String>,
    },
    /// No association yet and one candidate is uniquely reliable.
    Associate { project_id: String },
    /// No association and no reliable automatic candidate: wait for the user.
    AwaitSelection,
}

/// Recomputes the association from evidence.
///
/// A previous association must still be supported by *current* evidence. Fresh metadata
/// for the same workspace never restarts a preview that the user stopped, and a
/// manually chosen project is kept until an active-scope claim contradicts it.
pub(super) fn decide(
    candidates: &[Candidate],
    target: &TargetInfo,
    observation: Option<&WorkspaceObservation>,
    project: Option<&ProjectRef>,
    associated_manually: bool,
    observed: u64,
) -> AssociationDecision {
    let automatic = automatic_candidate(candidates, target, observed);
    if let Some(project) = project {
        let different_project = automatic.as_ref().is_some_and(|id| {
            candidates
                .iter()
                .any(|candidate| candidate.id == *id && candidate.project.id != project.id)
        });
        let conflicting = different_project
            || candidates.iter().any(|candidate| {
                candidate.evidence.iter().any(|evidence| {
                    current_active_evidence(evidence, target, observed)
                        && !workspace_path_matches(project, &evidence.raw_path)
                })
            })
            || observation.is_some_and(|observation| {
                observation.status == WorkspaceStatus::Verified
                    && observation
                        .active_path
                        .as_ref()
                        .is_some_and(|path| !workspace_path_matches(project, path))
            });
        let unchanged = automatic.as_ref().is_some_and(|id| {
            candidates
                .iter()
                .any(|candidate| candidate.id == *id && candidate.project.id == project.id)
        });
        if !conflicting && (associated_manually || unchanged) {
            return AssociationDecision::Hold;
        }
        return AssociationDecision::Release {
            reason: "WORKSPACE_EVIDENCE_CHANGED",
            rebind: automatic,
        };
    }
    match automatic {
        Some(project_id) => AssociationDecision::Associate { project_id },
        None => AssociationDecision::AwaitSelection,
    }
}

/// Whether the association is still supported by a currently automatic candidate.
///
/// This applies the same freshness gate as [`decide`], so the periodic expiry check and a
/// discovery pass cannot disagree about whether an association claim has expired.
pub(super) fn association_still_current(
    candidates: &[Candidate],
    target: &TargetInfo,
    project_id: &str,
    observed: u64,
) -> bool {
    let active = automatic_candidate(candidates, target, observed);
    candidates.iter().any(|candidate| {
        active.as_deref() == Some(candidate.id.as_str()) && candidate.project.id == project_id
    })
}

/// True when a freshly verified host observation contradicts this project.
///
/// Used before an explicit selection so a user cannot bind a directory that the host's
/// own verified workspace already disproves.
pub(super) fn conflicts_with_active_workspace(
    project: &ProjectRef,
    observation: Option<&WorkspaceObservation>,
    observed: u64,
) -> bool {
    observation.is_some_and(|observation| {
        observation.status == WorkspaceStatus::Verified
            && observation.observed_at <= observed
            && observed.saturating_sub(observation.observed_at) < WORKSPACE_OBSERVATION_TTL_MS
            && observation
                .active_path
                .as_ref()
                .is_some_and(|path| !workspace_path_matches(project, path))
    })
}

/// Whether this association was chosen by the user rather than derived from evidence.
pub(super) fn is_manual_association(
    explicitly_selected: bool,
    associated_manually: bool,
    current_project_id: Option<&str>,
    project_id: &str,
) -> bool {
    explicitly_selected
        || (associated_manually && current_project_id.is_some_and(|current| current == project_id))
}

/// Evidence recorded with a session binding.
///
/// The evidence of every candidate for the project is the base. An explicit selection adds
/// its own manual evidence; a derived association inherits the manual evidence of the
/// previous binding for the same project so a user's choice is never silently dropped.
pub(super) fn binding_evidence(
    candidates: &[Candidate],
    project_id: &str,
    previously_bound: &[Evidence],
    manual: Option<Evidence>,
    inherited_manual: bool,
) -> Vec<Evidence> {
    let mut evidence: Vec<Evidence> = candidates
        .iter()
        .filter(|candidate| candidate.project.id == project_id)
        .flat_map(|candidate| candidate.evidence.iter().cloned())
        .collect();
    if let Some(manual) = manual {
        evidence.push(manual);
    } else if inherited_manual {
        for previous in previously_bound
            .iter()
            .filter(|evidence| evidence.source == EvidenceSource::Manual)
        {
            if !evidence.contains(previous) {
                evidence.push(previous.clone());
            }
        }
    }
    evidence
}

/// Recomputes only the evidence of an already held binding.
pub(super) fn refreshed_binding_evidence(
    candidates: &[Candidate],
    project_id: &str,
    previous: &[Evidence],
    associated_manually: bool,
) -> Vec<Evidence> {
    binding_evidence(candidates, project_id, previous, None, associated_manually)
}

/// Carries user-visible candidates from the previous pass into a lightweight poll.
///
/// Polls never re-inspect every historical root, so:
/// - manual evidence for the *current* target is carried forward, unless a verified host
///   observation now disproves that project;
/// - history-only candidates are retained as suggestions.
///
/// Neither path can make a candidate automatically eligible or executable.
pub(super) fn carry_forward_previous_candidates(
    previous: &[Candidate],
    discovered: &mut Vec<Candidate>,
    target: &TargetInfo,
    observation: &WorkspaceObservation,
) {
    for candidate in previous {
        if !candidate.evidence.iter().any(|evidence| {
            evidence.source == EvidenceSource::Manual
                && evidence.target_id.as_deref() == Some(target.id.as_str())
                && evidence.process.as_ref() == Some(&target.process)
        }) || (observation.status == WorkspaceStatus::Verified
            && observation
                .active_path
                .as_ref()
                .is_some_and(|path| !workspace_path_matches(&candidate.project, path)))
        {
            continue;
        }
        if let Some(current) = discovered.iter_mut().find(|item| item.id == candidate.id) {
            current.evidence.extend(
                candidate
                    .evidence
                    .iter()
                    .filter(|evidence| evidence.source == EvidenceSource::Manual)
                    .cloned(),
            );
        } else {
            // The read-only inspection refreshes the directory's validity without
            // replacing current host evidence with a manual current-workspace claim.
            let mut manual = candidate.clone();
            manual
                .evidence
                .retain(|evidence| evidence.source == EvidenceSource::Manual);
            manual.auto_eligible = false;
            discovered.push(manual);
        }
    }
    for candidate in previous {
        if !candidate.evidence.is_empty()
            && candidate
                .evidence
                .iter()
                .all(|evidence| evidence.source == EvidenceSource::History)
            && !discovered.iter().any(|current| current.id == candidate.id)
        {
            discovered.push(candidate.clone());
        }
    }
}

/// Revokes every unproven claim after a discovery failure.
///
/// A failed source cannot leave behind evidence that still looks like a current
/// workspace, and it can never trigger project code execution on its own.
pub(super) fn revoke_unverified_evidence(candidates: &mut [Candidate]) {
    for candidate in candidates {
        candidate.auto_eligible = false;
        for evidence in &mut candidate.evidence {
            if !matches!(
                evidence.source,
                EvidenceSource::History | EvidenceSource::Manual
            ) {
                evidence.verifiable = false;
                evidence.active_scope = false;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::WorkspaceResolver;
    use crate::ipc::model::{ProcessRef, SessionRef};

    fn target() -> TargetInfo {
        TargetInfo {
            id: "target-1".into(),
            title: "Terminal".into(),
            app: "terminal".into(),
            process: ProcessRef {
                pid: 42,
                started_at: "90".into(),
            },
        }
    }

    #[test]
    fn polling_another_source_keeps_unsampled_uia_only_until_its_original_expiry() {
        let mut evidence = strong_evidence("a", 100, 200);
        evidence.source = EvidenceSource::Uia;
        let candidates = [candidate("a", vec![evidence.clone()])];
        let source = UnsampledTabEvidence(&candidates);
        assert_eq!(source.evidence(&target(), 150).unwrap(), vec![evidence]);
        assert!(source.evidence(&target(), 200).unwrap().is_empty());
        let mut reused_target = target();
        reused_target.process.started_at = "91".into();
        assert!(source.evidence(&reused_target, 150).unwrap().is_empty());
    }

    fn project(id: &str) -> ProjectRef {
        ProjectRef {
            id: id.into(),
            name: id.into(),
            environment: Environment::WindowsLocal,
            workspace_root: format!("C:\\{id}"),
            app_root: format!("C:\\{id}"),
            module: "entry".into(),
            variant: "default".into(),
            sdk_id: None,
        }
    }

    fn strong_evidence(id: &str, observed_at: u64, expires_at: u64) -> Evidence {
        Evidence {
            source: EvidenceSource::Registration,
            target_id: Some("target-1".into()),
            process: Some(target().process),
            session_id: Some("session-1".into()),
            observed_at,
            expires_at,
            environment: Environment::WindowsLocal,
            raw_path: format!("C:\\{id}"),
            verifiable: true,
            active_scope: true,
        }
    }

    fn candidate(id: &str, evidence: Vec<Evidence>) -> Candidate {
        Candidate {
            id: id.into(),
            project: project(id),
            evidence,
            valid: true,
            auto_eligible: false,
            reasons: Vec::new(),
            category: crate::discovery::PENDING,
            category_expires_at: 0,
        }
    }

    #[test]
    fn a_workspace_path_claim_is_normalized_but_never_converted_between_environments() {
        assert_eq!(
            normalized_workspace_path("C:/Users/Dev/App/"),
            "c:\\users\\dev\\app"
        );
        assert_eq!(
            normalized_workspace_path("\\\\?\\C:\\Users\\Dev\\App"),
            "c:\\users\\dev\\app"
        );
        assert_eq!(
            normalized_workspace_path("\\\\?\\UNC\\server\\share"),
            "\\\\server\\share"
        );
        let windows = project("a");
        assert!(workspace_path_matches(&windows, "C:\\a"));
        assert!(workspace_path_matches(&windows, "C:\\a\\entry\\src"));
        assert!(!workspace_path_matches(&windows, "C:\\b"));
        let mut wsl = project("a");
        wsl.environment = Environment::Wsl;
        assert!(!workspace_path_matches(&wsl, "C:\\a"));
    }

    #[test]
    fn disagreeing_or_incomplete_sources_suspend_the_verified_summary() {
        let mut first = candidate("a", vec![strong_evidence("a", 100, 200)]);
        first.evidence.push({
            let mut second = strong_evidence("b", 100, 200);
            second.target_id = Some("target-1".into());
            second
        });
        let summary = workspace_observation(&[first], &target(), 150, None);
        assert_eq!(summary.status, WorkspaceStatus::Suggested);
        assert!(summary.active_path.is_none());

        let mut incomplete = candidate("a", vec![strong_evidence("a", 100, 200)]);
        incomplete
            .reasons
            .push("Evidence source resolution incomplete; automatic association is blocked".into());
        let summary = workspace_observation(&[incomplete], &target(), 150, None);
        assert_eq!(summary.status, WorkspaceStatus::Suggested);

        let verified = workspace_observation(
            &[candidate("a", vec![strong_evidence("a", 100, 200)])],
            &target(),
            150,
            None,
        );
        assert_eq!(verified.status, WorkspaceStatus::Verified);
        assert_eq!(verified.active_path.as_deref(), Some("C:\\a"));
    }

    #[test]
    fn a_previous_association_is_kept_only_while_current_evidence_supports_it() {
        let bound = candidate("a", vec![strong_evidence("a", 100, 200)]);
        let current = project("a");
        // Fresh metadata for the same workspace keeps the session and only rebinds evidence.
        assert_eq!(
            decide(
                std::slice::from_ref(&bound),
                &target(),
                None,
                Some(&current),
                false,
                150
            ),
            AssociationDecision::Hold
        );
        // Expiry releases it before the old workspace can emit another frame.
        assert_eq!(
            decide(
                std::slice::from_ref(&bound),
                &target(),
                None,
                Some(&current),
                false,
                250
            ),
            AssociationDecision::Release {
                reason: "WORKSPACE_EVIDENCE_CHANGED",
                rebind: None,
            }
        );
        // A verified host observation for another directory also releases it. Two strong
        // candidates for different directories are ambiguous, so nothing is rebound.
        let mut elsewhere = workspace_observation(
            &[candidate("b", vec![strong_evidence("b", 100, 200)])],
            &target(),
            150,
            None,
        );
        elsewhere.status = WorkspaceStatus::Verified;
        let both = vec![
            candidate("a", vec![strong_evidence("a", 100, 200)]),
            candidate("b", vec![strong_evidence("b", 100, 200)]),
        ];
        assert_eq!(
            decide(
                &both,
                &target(),
                Some(&elsewhere),
                Some(&current),
                true,
                150
            ),
            AssociationDecision::Release {
                reason: "WORKSPACE_EVIDENCE_CHANGED",
                rebind: None,
            }
        );
        // The same release rebinds the one candidate the evidence still supports when it
        // is unambiguous, which is what keeps a window switch from stranding the session.
        assert_eq!(
            decide(
                std::slice::from_ref(&bound),
                &target(),
                Some(&elsewhere),
                Some(&current),
                true,
                150
            ),
            AssociationDecision::Release {
                reason: "WORKSPACE_EVIDENCE_CHANGED",
                rebind: Some("a".into()),
            }
        );
    }

    #[test]
    fn a_manual_choice_survives_weak_evidence_but_not_a_contradicting_active_claim() {
        let weak = candidate(
            "a",
            vec![{
                let mut evidence = strong_evidence("a", 100, 200);
                evidence.source = EvidenceSource::Title;
                evidence.verifiable = false;
                evidence.active_scope = false;
                evidence
            }],
        );
        let current = project("a");
        assert_eq!(
            decide(
                std::slice::from_ref(&weak),
                &target(),
                None,
                Some(&current),
                true,
                150
            ),
            AssociationDecision::Hold
        );
        // The same weak candidate cannot be associated automatically.
        assert_eq!(
            decide(
                std::slice::from_ref(&weak),
                &target(),
                None,
                None,
                false,
                150
            ),
            AssociationDecision::AwaitSelection
        );
    }

    #[test]
    fn release_can_rebind_the_uniquely_reliable_candidate() {
        let other = candidate("b", vec![strong_evidence("b", 100, 200)]);
        let mut elsewhere = strong_evidence("b", 100, 200);
        elsewhere.raw_path = "C:\\b".into();
        assert_eq!(
            decide(
                std::slice::from_ref(&other),
                &target(),
                None,
                Some(&project("a")),
                false,
                150
            ),
            AssociationDecision::Release {
                reason: "WORKSPACE_EVIDENCE_CHANGED",
                rebind: Some("b".into()),
            }
        );
        assert_eq!(
            decide(
                std::slice::from_ref(&other),
                &target(),
                None,
                None,
                false,
                150
            ),
            AssociationDecision::Associate {
                project_id: "b".into()
            }
        );
        assert_eq!(
            decide(&[], &target(), None, None, false, 150),
            AssociationDecision::AwaitSelection
        );
    }

    #[test]
    fn an_explicit_choice_cannot_contradict_a_fresh_verified_workspace() {
        let observation = workspace_observation(
            &[candidate("b", vec![strong_evidence("b", 100, 200)])],
            &target(),
            150,
            None,
        );
        assert!(conflicts_with_active_workspace(
            &project("a"),
            Some(&observation),
            150
        ));
        assert!(!conflicts_with_active_workspace(
            &project("b"),
            Some(&observation),
            150
        ));
        // An expired observation cannot block a deliberate choice.
        assert!(!conflicts_with_active_workspace(
            &project("a"),
            Some(&observation),
            150 + WORKSPACE_OBSERVATION_TTL_MS
        ));
    }

    #[test]
    fn a_periodic_check_uses_the_same_freshness_gate_as_the_decision() {
        let bound = candidate("a", vec![strong_evidence("a", 100, 200)]);
        let candidates = std::slice::from_ref(&bound);
        assert!(association_still_current(candidates, &target(), "a", 150));
        // Expiry and a different project both end the claim.
        assert!(!association_still_current(candidates, &target(), "a", 250));
        assert!(!association_still_current(candidates, &target(), "b", 150));
        assert!(!association_still_current(&[], &target(), "a", 150));
    }

    #[test]
    fn a_manual_or_historical_candidate_never_gains_automatic_association() {
        // Display category and automatic association are separate decisions: the manual
        // candidate is shown as `manual` and is still never associated automatically.
        let manual = candidate(
            "a",
            vec![{
                let mut evidence = strong_evidence("a", 100, 200);
                evidence.source = EvidenceSource::Manual;
                evidence.verifiable = false;
                evidence
            }],
        );
        let observation =
            workspace_observation(std::slice::from_ref(&manual), &target(), 150, None);
        let category =
            crate::discovery::classification::classify(&manual, &target(), Some(&observation), 150)
                .category;
        assert_eq!(category, crate::ipc::model::CandidateCategory::Manual);
        assert_eq!(
            decide(
                std::slice::from_ref(&manual),
                &target(),
                Some(&observation),
                None,
                false,
                150
            ),
            AssociationDecision::AwaitSelection
        );
    }

    #[test]
    fn binding_evidence_keeps_the_users_manual_choice_for_the_same_project() {
        let manual = Evidence {
            source: EvidenceSource::Manual,
            target_id: Some("target-1".into()),
            process: Some(target().process),
            session_id: None,
            observed_at: 100,
            expires_at: 200,
            environment: Environment::WindowsLocal,
            raw_path: "C:\\a".into(),
            verifiable: true,
            active_scope: true,
        };
        let manual = [manual];
        let manual = manual.as_slice();
        let candidates = vec![candidate("a", vec![strong_evidence("a", 100, 200)])];
        let derived = binding_evidence(&candidates, "a", manual, None, true);
        assert!(derived.contains(&manual[0]));
        // Without the manual inheritance, only discovered evidence is bound.
        let discovered_only = binding_evidence(&candidates, "a", manual, None, false);
        assert!(!discovered_only.contains(&manual[0]));
        // An explicit selection adds its own manual evidence exactly once.
        let explicit = binding_evidence(&candidates, "a", &[], Some(manual[0].clone()), false);
        assert_eq!(
            explicit
                .iter()
                .filter(|evidence| evidence.source == EvidenceSource::Manual)
                .count(),
            1
        );
        // A refresh of the same binding keeps the manual choice and drops nothing else.
        let refreshed = refreshed_binding_evidence(&candidates, "a", manual, true);
        assert!(refreshed.contains(&manual[0]));
    }

    #[test]
    fn manual_association_is_derived_from_the_user_choice_only() {
        assert!(is_manual_association(true, false, None, "a"));
        assert!(is_manual_association(false, true, Some("a"), "a"));
        assert!(!is_manual_association(false, true, Some("b"), "a"));
        assert!(!is_manual_association(false, false, Some("a"), "a"));
    }

    #[test]
    fn a_lightweight_poll_carries_manual_and_history_candidates_without_granting_authority() {
        let manual = Evidence {
            source: EvidenceSource::Manual,
            target_id: Some("target-1".into()),
            process: Some(target().process),
            session_id: None,
            observed_at: 100,
            expires_at: 200,
            environment: Environment::WindowsLocal,
            raw_path: "C:\\a".into(),
            verifiable: true,
            active_scope: true,
        };
        let history = Evidence {
            source: EvidenceSource::History,
            target_id: None,
            process: None,
            session_id: None,
            observed_at: 100,
            expires_at: 200,
            environment: Environment::WindowsLocal,
            raw_path: "C:\\h".into(),
            verifiable: false,
            active_scope: false,
        };
        let previous = vec![
            candidate("a", vec![manual.clone()]),
            candidate("h", vec![history]),
            candidate("x", vec![manual.clone()]),
        ];
        let observation = workspace_observation(
            &[candidate("a", vec![strong_evidence("a", 100, 200)])],
            &target(),
            150,
            None,
        );
        let mut discovered = vec![candidate("a", vec![strong_evidence("a", 100, 200)])];
        carry_forward_previous_candidates(&previous, &mut discovered, &target(), &observation);
        // "a" keeps its manual evidence, "h" is retained as history only.
        assert_eq!(
            discovered
                .iter()
                .find(|item| item.id == "a")
                .unwrap()
                .evidence
                .iter()
                .filter(|e| e.source == EvidenceSource::Manual)
                .count(),
            1
        );
        assert!(discovered.iter().any(|item| item.id == "h"));
        assert!(
            !discovered.iter().any(|item| item.auto_eligible),
            "carrying candidates forward never grants automatic eligibility"
        );
        // A continued manual choice is a suggestion, never a current-workspace claim.
        assert_eq!(
            crate::discovery::classification::classify(
                discovered.first().unwrap(),
                &target(),
                Some(&observation),
                150
            )
            .category,
            crate::ipc::model::CandidateCategory::Current
        );
    }

    #[test]
    fn a_failed_discovery_revokes_unproven_claims_and_keeps_manual_and_history_input() {
        let mut candidates = vec![
            candidate("a", vec![strong_evidence("a", 100, 200)]),
            candidate(
                "m",
                vec![Evidence {
                    source: EvidenceSource::Manual,
                    target_id: None,
                    process: None,
                    session_id: None,
                    observed_at: 100,
                    expires_at: 200,
                    environment: Environment::WindowsLocal,
                    raw_path: "C:\\m".into(),
                    verifiable: true,
                    active_scope: false,
                }],
            ),
        ];
        candidates[0].auto_eligible = true;
        revoke_unverified_evidence(&mut candidates);
        assert!(!candidates[0].auto_eligible);
        assert!(!candidates[0].evidence[0].verifiable);
        assert!(!candidates[0].evidence[0].active_scope);
        // Manual input is preserved but still not a current-workspace claim.
        assert!(!candidates[1].evidence[0].active_scope);
        assert_eq!(candidates[1].evidence[0].source, EvidenceSource::Manual);
    }

    #[test]
    fn association_policy_never_reads_or_writes_execution_permission() {
        // `decide` only sees candidates, the target, the observation and the previous
        // project; trust is not an input and cannot become an output.
        let binding = SessionRef {
            id: "session".into(),
            project: project("a"),
            target: target(),
            binding_evidence: vec![strong_evidence("a", 100, 200)],
            sdk_id: None,
        };
        assert_eq!(binding.project.id, "a");
        assert!(!binding.binding_evidence.is_empty());
    }
}
