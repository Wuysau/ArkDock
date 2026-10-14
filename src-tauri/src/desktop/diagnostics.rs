//! Bounded diagnostic aggregation for the state snapshot.
//!
//! Every backend subsystem reports through this one function so the snapshot carries a
//! single, bounded, consistently stamped list. Only the newest 128 records are kept: a
//! repeated failure, an unreachable log sink, or a burst of runtime messages can never
//! grow the snapshot without bound, and the record being written is never the one that
//! is dropped.
use super::*;

/// Retention bound for `AppSnapshot.diagnostics`.
pub(super) const MAX_DIAGNOSTICS: usize = 128;

/// The identity fields a diagnostic is stamped with.
///
/// Captured by the caller before the mutable borrow of the snapshot's diagnostic list,
/// so a diagnostic can never be attributed to a project or session that is being
/// replaced in the same statement.
pub(super) struct DiagnosticScope {
    pub(super) workspace_id: Option<String>,
    pub(super) session_id: Option<String>,
    pub(super) build_id: Option<String>,
    pub(super) backend_id: Option<String>,
}

/// Appends one record, dropping the oldest when the bound is reached.
pub(super) fn record(
    records: &mut Vec<Diagnostic>,
    scope: DiagnosticScope,
    code: &str,
    message: String,
    status: CheckStatus,
) {
    if records.len() >= MAX_DIAGNOSTICS {
        records.remove(0);
    }
    records.push(Diagnostic {
        code: code.into(),
        message,
        status,
        observed_at: now(),
        workspace_id: scope.workspace_id,
        session_id: scope.session_id,
        build_id: scope.build_id,
        backend_id: scope.backend_id,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope() -> DiagnosticScope {
        DiagnosticScope {
            workspace_id: Some("workspace".into()),
            session_id: Some("session".into()),
            build_id: Some("build".into()),
            backend_id: Some("native-previewer".into()),
        }
    }

    #[test]
    fn the_list_stays_bounded_and_keeps_the_newest_records() {
        let mut records = Vec::new();
        for index in 0..(MAX_DIAGNOSTICS + 20) {
            record(
                &mut records,
                scope(),
                "WATCH_FAILED",
                format!("record {index}"),
                CheckStatus::Fail,
            );
        }
        assert_eq!(records.len(), MAX_DIAGNOSTICS);
        assert_eq!(records.first().unwrap().message, "record 20");
        assert_eq!(
            records.last().unwrap().message,
            format!("record {}", MAX_DIAGNOSTICS + 19)
        );
        assert!(records.iter().all(|record| record.code == "WATCH_FAILED"));
    }

    #[test]
    fn every_record_is_stamped_with_the_scope_that_produced_it() {
        let mut records = Vec::new();
        record(
            &mut records,
            scope(),
            "PREVIEW_FAILED",
            "boom".into(),
            CheckStatus::Fail,
        );
        let record = &records[0];
        assert_eq!(record.workspace_id.as_deref(), Some("workspace"));
        assert_eq!(record.session_id.as_deref(), Some("session"));
        assert_eq!(record.build_id.as_deref(), Some("build"));
        assert_eq!(record.backend_id.as_deref(), Some("native-previewer"));
        assert!(record.observed_at > 0);
        assert_eq!(record.status, CheckStatus::Fail);
    }
}
