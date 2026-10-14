//! Backend-owned candidate display semantics.
//!
//! The frontend renders labels, hints and groups from the category that this module
//! assigns to every candidate in a snapshot. It never recomputes evidence freshness,
//! target identity or process lifetime, so a stale React render cannot invent a
//! "current workspace" claim that the backend does not hold.
//!
//! The category is display semantics only:
//! - it never grants execution trust (`workspace.rs` owns trust);
//! - it never grants automatic association (`automatic_candidate` owns that);
//! - historical and manual candidates cannot reach [`CandidateCategory::Current`],
//!   because only lifecycle-verified active-scope evidence can.
//!
//! Every input that could change the answer is re-evaluated on each snapshot and the
//! earliest deadline at which the answer can move is reported as `categoryExpiresAt`.

use crate::ipc::model::{
    Candidate, CandidateCategory, Evidence, EvidenceSource, TargetInfo, WorkspaceObservation,
    WorkspaceStatus,
};

use super::{current_active_evidence, current_target_evidence, WORKSPACE_OBSERVATION_TTL_MS};

/// Why a candidate holds its category. Kept next to the rules so the reason and the
/// rule cannot drift; the wire contract exposes the category, not this prose.
const CURRENT: &str =
    "Lifecycle-verified active-scope evidence matches this target window and process lifetime";
const CURRENT_UNPROVEN_OBSERVATION: &str =
    "Active-scope evidence matches the target, but the workspace observation is not verified and fresh";
const SUGGESTED_CURRENT: &str =
    "Evidence belongs to this target, but it does not prove the host's active workspace";
const WEAK: &str = "Weak or unverified evidence requires explicit confirmation";
const MANUAL: &str =
    "The directory passed a read-only inspection and waits for explicit confirmation";
const HISTORY: &str =
    "A previously associated project; history does not identify this window's workspace";

/// Candidates are constructed by read-only inspection before display semantics exist.
/// [`apply`] always replaces this placeholder before a snapshot is published, so a
/// partially built candidate can never reach the frontend with a guessed category.
pub(crate) const PENDING: CandidateCategory = CandidateCategory::Suggested;

/// One candidate's display semantics.
///
/// `reason` is the rule that produced the category. It is not sent on the wire: the
/// contract exposes the category and the frontend owns its own wording.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CandidateSemantics {
    pub(crate) category: CandidateCategory,
    /// Unix milliseconds at which the category may change. `0` means no scheduled change.
    pub(crate) expires_at: u64,
    #[allow(dead_code)]
    pub(crate) reason: &'static str,
}

/// Classifies one candidate. Pure: no state, no side effects, no inspection.
pub(crate) fn classify(
    candidate: &Candidate,
    target: &TargetInfo,
    observation: Option<&WorkspaceObservation>,
    now: u64,
) -> CandidateSemantics {
    let current: Vec<&Evidence> = candidate
        .evidence
        .iter()
        .filter(|evidence| {
            // Manual and history evidence is an input, never a claim about the host's
            // current workspace, so it cannot make a candidate look "current".
            !matches!(
                evidence.source,
                EvidenceSource::Manual | EvidenceSource::History
            ) && current_target_evidence(evidence, target, now)
        })
        .collect();
    let strong = current
        .iter()
        .any(|evidence| current_active_evidence(evidence, target, now));
    let observation_fresh_and_verified = observation.is_none_or(|observation| {
        observation.status == WorkspaceStatus::Verified
            && observation.observed_at <= now
            && now.saturating_sub(observation.observed_at) < WORKSPACE_OBSERVATION_TTL_MS
    });
    let category = if observation_fresh_and_verified && strong {
        CandidateCategory::Current
    } else if !current.is_empty() {
        // The target match is real but the active-workspace claim is not.
        CandidateCategory::Suggested
    } else if candidate
        .evidence
        .iter()
        .any(|evidence| evidence.source == EvidenceSource::Manual)
    {
        CandidateCategory::Manual
    } else if candidate
        .evidence
        .iter()
        .any(|evidence| evidence.source == EvidenceSource::History)
    {
        CandidateCategory::History
    } else {
        CandidateCategory::Suggested
    };
    let reason = match category {
        CandidateCategory::Current => CURRENT,
        CandidateCategory::Suggested if !current.is_empty() => {
            if strong {
                CURRENT_UNPROVEN_OBSERVATION
            } else {
                SUGGESTED_CURRENT
            }
        }
        CandidateCategory::Suggested => WEAK,
        CandidateCategory::Manual => MANUAL,
        CandidateCategory::History => HISTORY,
    };
    let expires_at = deadline(&category, candidate, observation, now);
    CandidateSemantics {
        category,
        expires_at,
        reason,
    }
}

/// The earliest instant at which this candidate's category can move.
fn deadline(
    category: &CandidateCategory,
    candidate: &Candidate,
    observation: Option<&WorkspaceObservation>,
    now: u64,
) -> u64 {
    let mut deadline = 0u64;
    // Fresh evidence only expires; expired evidence never becomes fresh again.
    for evidence in candidate
        .evidence
        .iter()
        .filter(|evidence| evidence.expires_at > now && evidence.observed_at <= now)
    {
        deadline = earliest(deadline, evidence.expires_at);
    }
    if *category == CandidateCategory::Current {
        if let Some(observation) = observation {
            // A verified observation stops being current after the shared TTL.
            deadline = earliest(
                deadline,
                observation
                    .observed_at
                    .saturating_add(WORKSPACE_OBSERVATION_TTL_MS),
            );
        }
    }
    deadline
}

fn earliest(current: u64, candidate: u64) -> u64 {
    if current == 0 {
        candidate
    } else {
        current.min(candidate)
    }
}

/// The result of one classification pass over the snapshot's candidates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Applied {
    /// True when at least one candidate's displayed category changed.
    pub(crate) changed: bool,
    /// Earliest instant the display may change; `0` when nothing is scheduled.
    pub(crate) next_recheck_at: u64,
}

/// Assigns display semantics to every candidate and reports when the snapshot's
/// candidate display can next change.
///
/// The snapshot is the only source of truth for what the frontend displays, so this
/// runs inside snapshot publication rather than on a frontend timer.
pub(crate) fn apply(
    candidates: &mut [Candidate],
    target: &TargetInfo,
    observation: Option<&WorkspaceObservation>,
    now: u64,
) -> Applied {
    let mut next = 0u64;
    let mut changed = false;
    for candidate in candidates.iter_mut() {
        let semantics = classify(candidate, target, observation, now);
        changed |= candidate.category != semantics.category;
        candidate.category = semantics.category;
        candidate.category_expires_at = semantics.expires_at;
        if semantics.expires_at > 0 {
            next = earliest(next, semantics.expires_at);
        }
    }
    Applied {
        changed,
        next_recheck_at: next,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::model::{Environment, ProcessRef, ProjectRef};

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

    fn candidate(id: &str, evidence: Vec<Evidence>) -> Candidate {
        Candidate {
            id: id.into(),
            project: ProjectRef {
                id: id.into(),
                name: id.into(),
                environment: Environment::WindowsLocal,
                workspace_root: format!("C:\\{id}"),
                app_root: format!("C:\\{id}"),
                module: "entry".into(),
                variant: "default".into(),
                sdk_id: None,
            },
            evidence,
            valid: true,
            auto_eligible: false,
            reasons: Vec::new(),
            category: CandidateCategory::Suggested,
            category_expires_at: 0,
        }
    }

    fn evidence(source: EvidenceSource, active_scope: bool, process: ProcessRef) -> Evidence {
        evidence_at(source, active_scope, process, 100, 200)
    }

    fn evidence_at(
        source: EvidenceSource,
        active_scope: bool,
        process: ProcessRef,
        observed_at: u64,
        expires_at: u64,
    ) -> Evidence {
        Evidence {
            source,
            target_id: Some("target-1".into()),
            process: Some(process),
            session_id: Some("session-1".into()),
            observed_at,
            expires_at,
            environment: Environment::WindowsLocal,
            raw_path: "C:\\project".into(),
            verifiable: active_scope,
            active_scope,
        }
    }

    fn verified(observed_at: u64) -> WorkspaceObservation {
        WorkspaceObservation {
            status: WorkspaceStatus::Verified,
            project_name: Some("project".into()),
            active_path: Some("C:\\project".into()),
            reason: String::new(),
            observed_at,
        }
    }

    fn process() -> ProcessRef {
        target().process
    }

    #[test]
    fn only_fresh_verified_observation_and_strong_evidence_reach_current() {
        let strong = candidate(
            "a",
            vec![evidence(EvidenceSource::Registration, true, process())],
        );
        assert_eq!(
            classify(&strong, &target(), Some(&verified(120)), 150).category,
            CandidateCategory::Current
        );
        // The same evidence, viewed after the observation TTL, is only a suggestion.
        let late = candidate(
            "a",
            vec![evidence_at(
                EvidenceSource::Registration,
                true,
                process(),
                20_000,
                20_100,
            )],
        );
        assert_eq!(
            classify(&late, &target(), Some(&verified(5_000)), 20_050).category,
            CandidateCategory::Suggested
        );
        // A suggested observation also demotes it.
        let mut suggested = verified(120);
        suggested.status = WorkspaceStatus::Suggested;
        assert_eq!(
            classify(&strong, &target(), Some(&suggested), 150).category,
            CandidateCategory::Suggested
        );
        // With no observation at all, strong evidence alone is still current.
        assert_eq!(
            classify(&strong, &target(), None, 150).category,
            CandidateCategory::Current
        );
    }

    #[test]
    fn weak_history_and_manual_evidence_never_reach_current() {
        let cases = [
            (EvidenceSource::Title, false, CandidateCategory::Suggested),
            (
                EvidenceSource::Registration,
                false,
                CandidateCategory::Suggested,
            ),
            (EvidenceSource::Manual, true, CandidateCategory::Manual),
            (EvidenceSource::History, false, CandidateCategory::History),
        ];
        for (source, active_scope, expected) in cases {
            let label = format!("{source:?}");
            let item = candidate("a", vec![evidence(source, active_scope, process())]);
            assert_eq!(
                classify(&item, &target(), Some(&verified(120)), 150).category,
                expected,
                "{label}"
            );
        }
    }

    #[test]
    fn expired_pid_reuse_and_session_change_are_re_evaluated() {
        let mut item = candidate(
            "a",
            vec![evidence(EvidenceSource::Registration, true, process())],
        );
        let observation = verified(120);
        assert_eq!(
            classify(&item, &target(), Some(&observation), 150).category,
            CandidateCategory::Current
        );
        // Evidence expiry.
        assert_eq!(
            classify(&item, &target(), Some(&observation), 200).category,
            CandidateCategory::Suggested
        );
        // Future-dated evidence is not current.
        item.evidence[0].observed_at = 151;
        assert_eq!(
            classify(&item, &target(), Some(&observation), 150).category,
            CandidateCategory::Suggested
        );
        item.evidence[0].observed_at = 100;
        // PID reuse: same window title, different process lifetime.
        let mut reused = target();
        reused.process.started_at = "91".into();
        assert_eq!(
            classify(&item, &reused, Some(&observation), 150).category,
            CandidateCategory::Suggested
        );
        // Excessively long TTL cannot extend the claim.
        item.evidence[0].expires_at = 500_000;
        assert_eq!(
            classify(&item, &target(), Some(&observation), 150).category,
            CandidateCategory::Suggested
        );
    }

    #[test]
    fn apply_is_the_only_writer_and_reports_the_earliest_deadline() {
        let mut candidates = vec![
            candidate(
                "a",
                vec![evidence(EvidenceSource::Registration, true, process())],
            ),
            candidate(
                "b",
                vec![{
                    let mut manual = evidence(EvidenceSource::Manual, false, process());
                    manual.expires_at = 300;
                    manual.target_id = None;
                    manual.process = None;
                    manual
                }],
            ),
        ];
        let next = apply(&mut candidates, &target(), Some(&verified(120)), 150);
        assert_eq!(candidates[0].category, CandidateCategory::Current);
        assert_eq!(candidates[1].category, CandidateCategory::Manual);
        // 200 comes from the first candidate's evidence, 270 from the observation.
        assert_eq!(next.next_recheck_at, 200);
        assert!(next.changed);
        assert_eq!(candidates[0].category_expires_at, 200);
        assert_eq!(candidates[1].category_expires_at, 300);
        // Nothing scheduled once everything expired, and the change is reported.
        let expired = apply(&mut candidates, &target(), None, 400);
        assert_eq!(expired.next_recheck_at, 0);
        assert!(expired.changed);
        assert_eq!(candidates[0].category, CandidateCategory::Suggested);
        // A second pass over unchanged inputs is not a change.
        assert!(!apply(&mut candidates, &target(), None, 400).changed);
    }

    #[test]
    fn each_category_documents_a_distinct_rule() {
        let rules = [
            CURRENT,
            CURRENT_UNPROVEN_OBSERVATION,
            SUGGESTED_CURRENT,
            WEAK,
            MANUAL,
            HISTORY,
        ];
        let unique: std::collections::BTreeSet<&str> = rules.into_iter().collect();
        assert_eq!(
            unique.len(),
            rules.len(),
            "every rule needs its own wording"
        );
        assert!(rules.iter().all(|rule| rule.len() > 30));
    }

    #[test]
    fn current_category_does_not_grant_automatic_association() {
        let mut candidates = vec![candidate(
            "a",
            vec![evidence(EvidenceSource::Registration, true, process())],
        )];
        apply(&mut candidates, &target(), Some(&verified(120)), 150);
        assert_eq!(candidates[0].category, CandidateCategory::Current);
        // Display semantics never write the association flag.
        assert!(!candidates[0].auto_eligible);
        assert!(candidates[0].valid);
    }
}
