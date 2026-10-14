pub mod error;
pub mod model;

use model::{AppSnapshot, RequestScope};

/// Internal identity checks keep returning bounded strings; [`error::IpcError`] is the
/// explicit conversion boundary that turns them into the structured wire contract.
pub fn validate_scope(snapshot: &AppSnapshot, scope: &RequestScope) -> Result<(), String> {
    if scope.target_id.as_deref() != snapshot.target.as_ref().map(|t| t.id.as_str())
        || scope.session_id != snapshot.session_id
    {
        Err("STALE_REQUEST: target or preview session changed; review the current state".into())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn state(target: &str, session: &str) -> AppSnapshot {
        serde_json::from_value(json!({
            "revision":1,"phase":"untrusted",
            "target":{"id":target,"title":"Terminal","app":"terminal",
                "process":{"pid":42,"startedAt":"133700000000000001"}},
            "project":null,"candidates":[],"trusted":false,"sessionId":session,"sessionRef":null,
            "backend":{"id":"native-previewer","mode":"nativePreviewer","available":false,
                "reason":"PROTOCOL_UNVERIFIED","sdkPath":null,"sdkVersion":null,
                "canClick":false,"canSwipe":false,"canText":false,"canRotate":false},
            "build":{"generation":0,"displayedGeneration":null,"buildId":null,
                "durationMs":null,"error":null,"stale":false},
            "diagnostics":[],"dockHint":null,"layoutRequired":false,
            "display":{"mode":"expanded","requestId":0,"orbAutoHide":false,"trayAvailable":false,"reason":null},
            "motion":{"reduced":false,"intensity":"standard"},"dockConfirmation":null
        }))
        .unwrap()
    }

    #[test]
    fn delayed_requests_cannot_act_on_replacement_window_or_preview_session() {
        let scope = RequestScope {
            target_id: Some("window-a".into()),
            session_id: Some("session-a".into()),
        };
        assert!(validate_scope(&state("window-a", "session-a"), &scope).is_ok());
        assert!(validate_scope(&state("window-b", "session-a"), &scope).is_err());
        assert!(validate_scope(&state("window-a", "session-b"), &scope).is_err());
    }

    #[test]
    fn process_lifecycle_token_is_exact_on_the_wire() {
        let snapshot = state("a", "s");
        assert_eq!(
            snapshot.target.unwrap().process.started_at,
            "133700000000000001"
        );
    }
}
