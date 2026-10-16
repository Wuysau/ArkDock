pub mod error;
pub mod model;

use model::{AppSnapshot, DeviceRunIdentity, RequestScope};

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

/// Device runs survive Previewer stop/restart. Admission still pins the host
/// target, trusted project and (when supplied) device workspace. The manager
/// subsequently compares the complete DeviceRunIdentity before action/read.
pub fn validate_device_scope(
    snapshot: &AppSnapshot,
    scope: &RequestScope,
    identity: Option<&DeviceRunIdentity>,
) -> Result<(), String> {
    if snapshot.target.is_none()
        || scope.target_id.as_deref() != snapshot.target.as_ref().map(|target| target.id.as_str())
    {
        return Err("STALE_DEVICE_TARGET_SCOPE".into());
    }
    let project = snapshot.project.as_ref().ok_or("PROJECT_REQUIRED")?;
    if !snapshot.trusted {
        return Err("TRUST_REQUIRED".into());
    }
    if identity.is_some_and(|identity| identity.workspace_id != project.id) {
        return Err("STALE_DEVICE_RUNTIME_IDENTITY".into());
    }
    Ok(())
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

    #[test]
    fn device_scope_survives_preview_restart_but_not_host_project_or_trust_changes() {
        let mut current = state("host", "preview-new");
        current.trusted = true;
        current.project = Some(serde_json::from_value(json!({"id":"project", "name":"fixture", "environment":"windowsLocal", "workspaceRoot":"fixture", "appRoot":"fixture", "module":"entry", "variant":"default", "sdkId":null})).unwrap());
        let scope = RequestScope {
            target_id: Some("host".into()),
            session_id: Some("preview-old".into()),
        };
        let identity = DeviceRunIdentity {
            run_id: "device-run".into(),
            workspace_id: "project".into(),
            target_id: Some("device".into()),
            generation: 2,
        };
        assert!(validate_device_scope(&current, &scope, Some(&identity)).is_ok());
        assert!(
            validate_device_scope(&current, &scope, None).is_ok(),
            "state discovery needs no prior device identity"
        );
        assert!(
            validate_scope(&current, &scope).is_err(),
            "ordinary IPC keeps its Previewer-session gate"
        );
        current.session_id = None;
        assert!(validate_device_scope(&current, &scope, Some(&identity)).is_ok());
        current.trusted = false;
        assert!(validate_device_scope(&current, &scope, Some(&identity)).is_err());
        current.trusted = true;
        current.project.as_mut().unwrap().id = "other-project".into();
        assert!(validate_device_scope(&current, &scope, Some(&identity)).is_err());
        current.project.as_mut().unwrap().id = "project".into();
        current.target.as_mut().unwrap().id = "replacement-host".into();
        assert!(validate_device_scope(&current, &scope, Some(&identity)).is_err());
        current.target = None;
        assert!(validate_device_scope(
            &current,
            &RequestScope {
                target_id: None,
                session_id: None
            },
            None
        )
        .is_err());
    }
}
