//! Generic MCP actor boundary. All business calls observe the current desktop
//! trust/association state; the stdio process never drives a host conversation.
use super::*;
use crate::mcp::{auth::Credential, pipe::ServerGuard, ConnectionOwner};
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};

pub(super) struct McpRequest {
    pub owner: ConnectionOwner,
    pub tool: String,
    pub args: Value,
    pub reply: mpsc::Sender<Result<Value, String>>,
    pub cancelled: Arc<AtomicBool>,
}

pub(super) fn start_server(
    credential: Result<Credential, String>,
    sender: mpsc::SyncSender<Message>,
    available: bool,
    reason: String,
) -> (Option<ServerGuard>, bool, String) {
    let Ok(credential) = credential else {
        return (None, false, reason);
    };
    let handler = Arc::new(move |owner: ConnectionOwner, tool: String, args: Value| {
        let (reply, receiver) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        sender
            .try_send(Message::Mcp(Box::new(McpRequest {
                owner,
                tool,
                args,
                reply,
                cancelled: cancelled.clone(),
            })))
            .map_err(|_| "MCP_BACKEND_BUSY".to_owned())?;
        match receiver.recv_timeout(Duration::from_secs(3)) {
            Ok(result) => result,
            Err(_) => {
                cancelled.store(true, Ordering::Release);
                Err(
                    "MCP_RESULT_UNKNOWN: 后端回执超时，请查询任务状态，不要自动重试领取或回报。"
                        .into(),
                )
            }
        }
    });
    match crate::mcp::pipe::start_server(credential, handler) {
        Ok(server) => (Some(server), available, reason),
        Err(error) => (None, false, error),
    }
}

fn needs_project_authority(tool: &str) -> bool {
    matches!(
        tool,
        "arkdock_list_tasks"
            | "arkdock_claim_task"
            | "arkdock_get_task_context"
            | "arkdock_get_task_image"
            | "arkdock_report_task"
    )
}

// Admission can inspect the filesystem. Recheck cancellation after it, before
// applying a claim/update to the single actor-owned domain state.
fn execute_authorized_request<S, T>(
    state: &mut S,
    cancelled: &AtomicBool,
    admit: impl FnOnce(&mut S) -> Result<(), String>,
    apply: impl FnOnce(&mut S) -> Result<T, String>,
) -> Result<T, String> {
    if cancelled.load(Ordering::Acquire) {
        return Err("MCP_REQUEST_CANCELLED".into());
    }
    admit(state)?;
    if cancelled.load(Ordering::Acquire) {
        return Err("MCP_REQUEST_CANCELLED".into());
    }
    apply(state)
}

impl Engine {
    pub(super) fn dispatch_mcp(&mut self, request: McpRequest) {
        self.drain_events();
        let result = execute_authorized_request(
            self,
            &request.cancelled,
            |engine| engine.admit_mcp_tool(&request.tool),
            |engine| {
                engine
                    .mcp
                    .call(&request.owner, &request.tool, request.args, now())
            },
        );
        if result.is_ok()
            && matches!(
                request.tool.as_str(),
                "arkdock_connect_current_project"
                    | "arkdock_context_update"
                    | "arkdock_context_revoke"
                    | "__disconnect"
            )
        {
            self.sync_context_binding();
            self.publish();
        }
        let _ = request.reply.send(result);
    }

    fn admit_mcp_tool(&mut self, tool: &str) -> Result<(), String> {
        if needs_project_authority(tool) {
            self.current_target()?;
            let project = self.snapshot.project.as_ref().ok_or("PROJECT_REQUIRED")?;
            if !self.snapshot.trusted
                || !self
                    .store
                    .as_ref()
                    .is_some_and(|store| store.is_trusted(project))
            {
                self.device_runtime.state(None);
                self.suspend_execution();
                self.snapshot.phase = Phase::Untrusted;
                self.publish();
                return Err("TRUST_REQUIRED".into());
            }
        }
        self.mcp.sync_snapshot(&self.snapshot, now());
        Ok(())
    }

    pub(super) fn mcp_status_value(&mut self) -> Result<McpStatus, String> {
        self.mcp.sync_snapshot(&self.snapshot, now());
        let (contexts, tasks) = self.mcp.status(now());
        Ok(McpStatus {
            available: self.mcp_available,
            reason: self.mcp_reason.clone(),
            configuration: crate::mcp::configuration()?,
            contexts,
            tasks,
        })
    }

    pub(super) fn submit_mcp_task(
        &mut self,
        context: &str,
        selection: &str,
        observed: u64,
    ) -> Result<McpTaskSummary, String> {
        self.mcp.sync_snapshot(&self.snapshot, observed);
        let store = self
            .selections
            .as_mut()
            .ok_or("SELECTION_STORAGE_UNAVAILABLE")?;
        let selection = store.get(&self.snapshot, selection, observed)?;
        let screen = store.read_image(&self.snapshot, &selection.selection_id, false, observed)?;
        let crop = store.read_image(&self.snapshot, &selection.selection_id, true, observed)?;
        self.mcp
            .submit_task(context, &selection, screen, crop, observed)
    }
}

#[tauri::command]
pub(crate) async fn mcp_status(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
) -> Result<McpStatus, IpcError> {
    check_window(&window)?;
    let snapshot = service.snapshot()?;
    let scope = RequestScope {
        target_id: snapshot.target.as_ref().map(|target| target.id.clone()),
        session_id: snapshot.session_id,
    };
    inspection::decode_data(
        inspection::data_request(window, service, scope, DataAction::McpStatus).await?,
    )
}

#[tauri::command]
pub(crate) async fn mcp_revoke_context(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
    context_id: String,
) -> Result<bool, IpcError> {
    check_window(&window)?;
    if context_id.is_empty() || context_id.len() > 128 {
        return Err("INVALID_CANDIDATE".into());
    }
    let snapshot = service.snapshot()?;
    let scope = RequestScope {
        target_id: snapshot.target.as_ref().map(|target| target.id.clone()),
        session_id: snapshot.session_id,
    };
    inspection::decode_data(
        inspection::data_request(window, service, scope, DataAction::McpRevoke(context_id)).await?,
    )
}

#[tauri::command]
pub(crate) async fn mcp_submit_task(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
    scope: RequestScope,
    context_id: String,
    selection_id: String,
) -> Result<McpTaskSummary, IpcError> {
    inspection::decode_data(
        inspection::data_request(
            window,
            service,
            scope,
            DataAction::McpSubmit(context_id, selection_id),
        )
        .await?,
    )
}

#[tauri::command]
pub(crate) async fn mcp_retry_task(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
    scope: RequestScope,
    task_id: String,
    context_id: String,
) -> Result<McpTaskSummary, IpcError> {
    inspection::decode_data(
        inspection::data_request(
            window,
            service,
            scope,
            DataAction::McpRetry(task_id, context_id),
        )
        .await?,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_payload_and_claim_tools_require_live_project_authority() {
        assert!(needs_project_authority("arkdock_get_task_image"));
        assert!(needs_project_authority("arkdock_report_task"));
        assert!(!needs_project_authority("arkdock_connect_current_project"));
        assert!(!needs_project_authority("arkdock_context_status"));
        assert!(!needs_project_authority("__disconnect"));
    }
    #[test]
    fn cancellation_during_slow_admission_cannot_claim_a_task() {
        let cancelled = Arc::new(AtomicBool::new(false));
        let signal = cancelled.clone();
        let (entered, wait) = mpsc::channel();
        let (release, resume) = mpsc::channel();
        let worker = thread::spawn(move || {
            let mut claims = 0usize;
            let result = execute_authorized_request(
                &mut claims,
                &signal,
                |_| {
                    entered.send(()).unwrap();
                    resume.recv().unwrap();
                    Ok(())
                },
                |claims| {
                    *claims += 1;
                    Ok(())
                },
            );
            (result, claims)
        });
        wait.recv_timeout(Duration::from_secs(1)).unwrap();
        cancelled.store(true, Ordering::Release);
        release.send(()).unwrap();
        let (result, claims) = worker.join().unwrap();
        assert_eq!(result.unwrap_err(), "MCP_REQUEST_CANCELLED");
        assert_eq!(claims, 0);
    }

    #[test]
    fn trust_rejection_prevents_claim_and_returns_its_original_error() {
        let mut claims = 0;
        let result = execute_authorized_request(
            &mut claims,
            &AtomicBool::new(false),
            |_| Err("TRUST_REQUIRED".into()),
            |claims| {
                *claims += 1;
                Ok(())
            },
        );
        assert_eq!(result.unwrap_err(), "TRUST_REQUIRED");
        assert_eq!(claims, 0);
    }
}
