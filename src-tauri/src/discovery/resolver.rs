use std::collections::BTreeMap;
use std::path::Path;

use crate::ipc::model::{Candidate, Environment, Evidence, EvidenceSource, ProjectRef, TargetInfo};

use super::registration::RegistrationRegistry;
use super::{environment_name, fresh, inspect_directory, project_id, MAX_EVIDENCE_TTL_MS};

const SOURCE_INCOMPLETE_REASON: &str =
    "Evidence source resolution incomplete; automatic association is blocked";

/// A source supplies evidence only. Root inspection and the automatic decision remain centralized.
/// Implementations run inside the native backend; frontend flags are never a trusted source.
pub trait WorkspaceResolver {
    fn evidence(&self, target: &TargetInfo, now: u64) -> Result<Vec<Evidence>, String>;
}

pub struct TitleResolver;

impl WorkspaceResolver for TitleResolver {
    fn evidence(&self, target: &TargetInfo, now: u64) -> Result<Vec<Evidence>, String> {
        Ok(title_paths(&target.title)
            .into_iter()
            .filter_map(|path| {
                let environment = title_environment(&path)?;
                Some(Evidence {
                    source: EvidenceSource::Title,
                    target_id: Some(target.id.clone()),
                    process: Some(target.process.clone()),
                    session_id: None,
                    observed_at: now,
                    expires_at: now.saturating_add(MAX_EVIDENCE_TTL_MS),
                    environment,
                    raw_path: path,
                    verifiable: false,
                    active_scope: false,
                })
            })
            .collect())
    }
}

/// Default discovery uses only implemented weak sources. It cannot automatically associate a title
/// or an unauthenticated file registration, even when there is exactly one valid candidate.
pub fn resolve(target: &TargetInfo, now: u64) -> Result<Vec<Candidate>, String> {
    let title = TitleResolver;
    let registry = RegistrationRegistry::default();
    resolve_with_sources(target, now, &[&registry, &title])
}

pub fn resolve_with_sources(
    target: &TargetInfo,
    now: u64,
    sources: &[&dyn WorkspaceResolver],
) -> Result<Vec<Candidate>, String> {
    let mut merged: BTreeMap<String, Candidate> = BTreeMap::new();
    let mut errors = Vec::new();
    for source in sources {
        let evidence = match source.evidence(target, now) {
            Ok(evidence) => evidence,
            Err(error) => {
                errors.push(error);
                continue;
            }
        };
        if evidence.len() > 128 {
            errors.push("Evidence source exceeded its bounded 128-record limit".into());
            continue;
        }
        for evidence in evidence {
            if evidence.raw_path.is_empty()
                || evidence.raw_path.len() > 4096
                || evidence.raw_path.contains('\0')
            {
                continue;
            }
            let inspected = inspect_directory(
                Path::new(&evidence.raw_path),
                evidence.environment.clone(),
                evidence.clone(),
            );
            let candidates = match inspected {
                Ok(candidates) => candidates,
                Err(error) => vec![unavailable_candidate(&evidence, error)],
            };
            for candidate in candidates {
                if let Some(existing) = merged.get_mut(&candidate.id) {
                    existing.valid &= candidate.valid;
                    for evidence in candidate.evidence {
                        let serialized = serde_json::to_string(&evidence).unwrap_or_default();
                        if !existing.evidence.iter().any(|other| {
                            serde_json::to_string(other).unwrap_or_default() == serialized
                        }) {
                            existing.evidence.push(evidence);
                        }
                    }
                    for reason in candidate.reasons {
                        if !existing.reasons.contains(&reason) {
                            existing.reasons.push(reason);
                        }
                    }
                } else {
                    merged.insert(candidate.id.clone(), candidate);
                }
            }
        }
    }
    let mut candidates: Vec<_> = merged.into_values().collect();
    if !errors.is_empty() {
        for candidate in &mut candidates {
            candidate.reasons.push(SOURCE_INCOMPLETE_REASON.into());
        }
    }
    let chosen = if errors.is_empty() {
        automatic_candidate(&candidates, target, now)
    } else {
        None
    };
    for candidate in &mut candidates {
        candidate.auto_eligible = chosen.as_deref() == Some(candidate.id.as_str());
        explain_evidence(candidate, target, now);
        for error in &errors {
            candidate
                .reasons
                .push(format!("Another evidence source was unavailable: {error}"));
        }
    }
    if candidates.is_empty() && !errors.is_empty() {
        return Err(errors.join("; "));
    }
    candidates.sort_by(|a, b| {
        b.auto_eligible
            .cmp(&a.auto_eligible)
            .then_with(|| b.valid.cmp(&a.valid))
            .then_with(|| a.project.app_root.cmp(&b.project.app_root))
            .then_with(|| a.project.module.cmp(&b.project.module))
            .then_with(|| a.id.cmp(&b.id))
    });
    Ok(candidates)
}

/// Recomputes the decision from evidence; a serialized autoEligible flag cannot grant association.
/// A unique current, verifiable active scope must match both target token and PID/start time.
pub fn automatic_candidate(
    candidates: &[Candidate],
    target: &TargetInfo,
    now: u64,
) -> Option<String> {
    if candidates.iter().any(|candidate| {
        candidate
            .reasons
            .iter()
            .any(|reason| reason == SOURCE_INCOMPLETE_REASON)
    }) {
        return None;
    }
    let mut eligible = candidates.iter().filter(|candidate| {
        candidate.valid
            && matches!(candidate.project.environment, Environment::WindowsLocal)
            && candidate
                .evidence
                .iter()
                .any(|evidence| strong(evidence, target, now))
    });
    let candidate = eligible.next()?;
    if eligible.next().is_some() {
        return None;
    }
    // A second current source claiming the active scope is a conflict even if its authentication
    // is incomplete. Weak title/history observations do not claim an active scope.
    if candidates.iter().any(|other| {
        other.id != candidate.id
            && other
                .evidence
                .iter()
                .any(|evidence| evidence.active_scope && current_target(evidence, target, now))
    }) {
        return None;
    }
    Some(candidate.id.clone())
}

fn current_target(evidence: &Evidence, target: &TargetInfo, now: u64) -> bool {
    fresh(evidence, now)
        && evidence.target_id.as_deref() == Some(target.id.as_str())
        && evidence.process.as_ref().is_some_and(|process| {
            process.pid == target.process.pid && process.started_at == target.process.started_at
        })
}

fn strong(evidence: &Evidence, target: &TargetInfo, now: u64) -> bool {
    matches!(
        evidence.source,
        EvidenceSource::Registration | EvidenceSource::Uia | EvidenceSource::ProcessArguments
    ) && evidence.verifiable
        && evidence.active_scope
        && current_target(evidence, target, now)
        && matches!(evidence.environment, Environment::WindowsLocal)
        && evidence
            .session_id
            .as_ref()
            .is_some_and(|session| !session.is_empty() && session.len() <= 128)
}

fn explain_evidence(candidate: &mut Candidate, target: &TargetInfo, now: u64) {
    let has_strong = candidate
        .evidence
        .iter()
        .any(|evidence| strong(evidence, target, now));
    let reason = if candidate.auto_eligible {
        "Unique current verifiable active-scope evidence matches target token and process lifetime"
    } else if has_strong {
        "Multiple or conflicting active-scope candidates require explicit selection"
    } else {
        "Weak or unverified evidence requires explicit selection; uniqueness does not establish the active workspace"
    };
    candidate.reasons.push(reason.into());
    if candidate
        .evidence
        .iter()
        .any(|evidence| !fresh(evidence, now))
    {
        candidate.reasons.push("Expired, future-dated, or excessive-TTL evidence cannot authorize automatic association".into());
    }
    if candidate.evidence.iter().any(|evidence| {
        evidence.target_id.as_deref() != Some(target.id.as_str())
            || !evidence.process.as_ref().is_some_and(|process| {
                process.pid == target.process.pid && process.started_at == target.process.started_at
            })
    }) {
        candidate.reasons.push(
            "Evidence target token or process lifetime does not match the current target".into(),
        );
    }
}

fn unavailable_candidate(evidence: &Evidence, reason: String) -> Candidate {
    let id = project_id(
        &evidence.environment,
        &evidence.raw_path,
        "<unverified>",
        "default",
    );
    Candidate {
        id: id.clone(), project: ProjectRef { id, name: evidence.raw_path.clone(),
            environment: evidence.environment.clone(), workspace_root: evidence.raw_path.clone(),
            app_root: evidence.raw_path.clone(), module: "<unverified>".into(), variant: "default".into(), sdk_id: None },
        evidence: vec![evidence.clone()], valid: false, auto_eligible: false,
        reasons: vec![format!("Read-only project validation failed: {reason}"),
            format!("Path is retained in its {} environment; no cross-environment conversion was attempted", environment_name(&evidence.environment))],
    }
}

fn title_environment(path: &str) -> Option<Environment> {
    if path.starts_with('/') && !path.starts_with("//") {
        if std::env::var_os("WSL_DISTRO_NAME").is_some() {
            Some(Environment::Wsl)
        } else {
            None
        }
    } else {
        Some(super::path_environment(Path::new(path)))
    }
}

pub(super) fn title_paths(title: &str) -> Vec<String> {
    if title.len() > 4096 {
        return Vec::new();
    }
    let mut result = Vec::new();
    // Titles are data: extract a small set of literal absolute hints, never run a shell.
    let mut hints: Vec<&str> = title
        .split('"')
        .enumerate()
        .filter_map(|(index, part)| if index % 2 == 1 { Some(part) } else { None })
        .take(8)
        .collect();
    hints.extend(
        title
            .split(" — ")
            .flat_map(|part| part.split(" - "))
            .flat_map(|part| part.split(" | "))
            .take(8),
    );
    for hint in hints {
        let hint = hint
            .trim()
            .trim_matches(|character| character == '[' || character == ']');
        let bytes = hint.as_bytes();
        let drive = bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && (bytes[2] == b'\\' || bytes[2] == b'/');
        let uri = ["ssh://", "wsl://", "vscode-remote://"]
            .iter()
            .any(|prefix| hint.starts_with(prefix));
        if (drive || hint.starts_with("\\\\") || hint.starts_with('/') || uri)
            && !result.iter().any(|value| value == hint)
        {
            result.push(hint.to_owned());
        }
        if result.len() == 8 {
            break;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::model::ProcessRef;

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
    fn candidate(id: &str, source: EvidenceSource, reliable: bool) -> Candidate {
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
            evidence: vec![Evidence {
                source,
                target_id: Some("target-1".into()),
                process: Some(target().process),
                session_id: Some("session-1".into()),
                observed_at: 100,
                expires_at: 200,
                environment: Environment::WindowsLocal,
                raw_path: format!("C:\\{id}"),
                verifiable: reliable,
                active_scope: reliable,
            }],
            valid: true,
            auto_eligible: false,
            reasons: vec![],
        }
    }
    #[test]
    fn unique_weak_candidate_and_multiple_weak_sources_never_auto_associate() {
        let mut weak = candidate("a", EvidenceSource::Title, false);
        weak.evidence.push(
            candidate("a", EvidenceSource::History, false)
                .evidence
                .remove(0),
        );
        weak.auto_eligible = true; // Untrusted flags must not grant authority.
        assert_eq!(automatic_candidate(&[weak], &target(), 150), None);
    }
    #[test]
    fn only_unique_current_strong_scope_can_auto_associate() {
        let strong = candidate("a", EvidenceSource::Registration, true);
        assert_eq!(
            automatic_candidate(std::slice::from_ref(&strong), &target(), 150),
            Some("a".into())
        );
        assert_eq!(
            automatic_candidate(
                &[strong, candidate("b", EvidenceSource::Registration, true)],
                &target(),
                150
            ),
            None
        );
    }
    #[test]
    fn expiration_pid_reuse_target_reuse_and_future_evidence_are_rejected() {
        for mutation in 0..5 {
            let mut candidate = candidate("a", EvidenceSource::Registration, true);
            match mutation {
                0 => candidate.evidence[0].expires_at = 150,
                1 => candidate.evidence[0].process.as_mut().unwrap().started_at = "91".into(),
                2 => candidate.evidence[0].target_id = Some("reused-hwnd-token".into()),
                3 => candidate.evidence[0].observed_at = 151,
                _ => candidate.evidence[0].expires_at = 500_000,
            }
            assert_eq!(automatic_candidate(&[candidate], &target(), 150), None);
        }
    }
    #[test]
    fn invalid_project_foreign_environment_and_unproven_active_tab_are_rejected() {
        let mut candidate = candidate("a", EvidenceSource::Registration, true);
        candidate.valid = false;
        assert_eq!(
            automatic_candidate(&[candidate.clone()], &target(), 150),
            None
        );
        candidate.valid = true;
        candidate.project.environment = Environment::Wsl;
        assert_eq!(
            automatic_candidate(&[candidate.clone()], &target(), 150),
            None
        );
        candidate.project.environment = Environment::WindowsLocal;
        candidate.evidence[0].active_scope = false;
        assert_eq!(automatic_candidate(&[candidate], &target(), 150), None);
    }
    #[test]
    fn conflicting_active_claim_blocks_even_an_authenticated_candidate() {
        let strong = candidate("a", EvidenceSource::Registration, true);
        let mut conflict = candidate("b", EvidenceSource::Registration, false);
        conflict.evidence[0].active_scope = true;
        assert_eq!(
            automatic_candidate(&[strong, conflict], &target(), 150),
            None
        );
    }
    #[test]
    fn history_or_title_flags_cannot_upgrade_a_source() {
        for source in [
            EvidenceSource::Title,
            EvidenceSource::History,
            EvidenceSource::Manual,
        ] {
            assert_eq!(
                automatic_candidate(&[candidate("a", source, true)], &target(), 150),
                None
            );
        }
    }
    #[test]
    fn foreign_paths_keep_their_environment_and_titles_are_bounded() {
        assert!(matches!(
            title_environment("\\\\wsl.localhost\\Ubuntu\\home\\user\\app"),
            Some(Environment::Wsl)
        ));
        assert!(matches!(
            title_environment("ssh://host/work/app"),
            Some(Environment::Remote)
        ));
        assert_eq!(
            title_paths("\"C:\\space dir\\app\" — Terminal"),
            vec!["C:\\space dir\\app"]
        );
        assert!(title_paths(&"x".repeat(5000)).is_empty());
        assert!(title_paths("project-name — Terminal").is_empty());
    }

    #[test]
    fn wsl_unc_unavailable_candidate_retains_raw_path_and_environment() {
        let mut target = target();
        target.title = r"\\wsl.localhost\Ubuntu\home\user\app — Terminal".into();
        let candidates = resolve_with_sources(&target, 150, &[&TitleResolver]).unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].project.environment, Environment::Wsl);
        assert_eq!(
            candidates[0].project.app_root,
            r"\\wsl.localhost\Ubuntu\home\user\app"
        );
        assert!(!candidates[0].valid);
        assert!(candidates[0]
            .reasons
            .iter()
            .any(|reason| reason.contains("WSL UNC/URI inspection is unsupported")));
        assert_eq!(automatic_candidate(&candidates, &target, 150), None);
    }

    #[test]
    fn partial_source_resolution_remains_blocked_when_decision_is_recomputed() {
        struct Source {
            fail: bool,
        }
        impl WorkspaceResolver for Source {
            fn evidence(&self, _: &TargetInfo, _: u64) -> Result<Vec<Evidence>, String> {
                if self.fail {
                    return Err("Native source unavailable".into());
                }
                let mut evidence = candidate("a", EvidenceSource::Registration, true)
                    .evidence
                    .remove(0);
                evidence.raw_path = Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../tests/fixtures/harmony-app/entry/src/main/ets/pages/Index.ets")
                    .to_string_lossy()
                    .into_owned();
                Ok(vec![evidence])
            }
        }
        let candidates = resolve_with_sources(
            &target(),
            150,
            &[&Source { fail: false }, &Source { fail: true }],
        )
        .unwrap();
        assert!(candidates[0].valid);
        assert_eq!(automatic_candidate(&candidates, &target(), 150), None);
    }
}
