//! Pre-docking pairing for the agent context source.
//!
//! The context source must be pairable BEFORE any project or preview session exists
//! (otherwise discovery could never bootstrap: no project to trust, no session to
//! scope with). These commands therefore validate the window only — not
//! [`super::Engine::validate_data_scope`] — and never touch execution trust. The
//! credential is persisted to a user-chosen file first and activated in the hub
//! second, so the reply never reports a pairing the backend does not hold.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use tauri::Manager;

use crate::context::{ContextHub, CONTEXT_PIPE_NAME};
use crate::ipc::model::{ContextPairing, IpcError};

use super::{check_window, Message, Service};

/// Managed handle shared by the pairing commands, the pipe listener and the engine.
pub(crate) struct ContextHubHandle(pub(crate) Arc<ContextHub>);

/// The credential file an agent tool points its MCP server at. The secret is read
/// from this file at runtime — it never travels on argv, in logs, or into Git.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ContextPairingFile<'a> {
    version: u32,
    pipe_name: &'a str,
    credential: &'a str,
    created_at: u64,
}

const PAIR_COMMIT_TIMEOUT_MS: u64 = 4_000;

fn hub(window: &tauri::WebviewWindow) -> Result<Arc<ContextHub>, IpcError> {
    Ok(window.state::<ContextHubHandle>().inner().0.clone())
}

/// Pairing state feeds the published snapshot; republish right away so the strip
/// does not wait up to one probe interval to reflect it.
fn request_publish(service: &Service) {
    let _ = service.sender.send(Message::Publish);
}

#[tauri::command]
pub(crate) async fn context_pair_create(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
) -> Result<ContextPairing, IpcError> {
    check_window(&window)?;
    let hub = hub(&window)?;
    let path = rfd::AsyncFileDialog::new()
        .set_title("保存 ArkDock 上下文源配对凭证（只交给本机 Agent 工具）")
        .set_file_name("arkdock-context-pairing.json")
        .save_file()
        .await
        .ok_or(IpcError::from("CONTEXT_PAIRING_SAVE_CANCELLED"))?;
    let pairing_file = path.path().to_string_lossy().into_owned();
    // Persist first, activate second: a failed write leaves any previous pairing
    // untouched, and the reply only reports a pairing the backend actually holds.
    let (credential, bytes) = crate::context::new_pairing().map_err(IpcError::from)?;
    let file = ContextPairingFile {
        version: 1,
        pipe_name: CONTEXT_PIPE_NAME,
        credential: &credential,
        created_at: crate::context::now_millis(),
    };
    let encoded = serde_json::to_vec_pretty(&file)
        .map_err(|_| IpcError::from("CONTEXT_PAIRING_SERIALIZE_FAILED"))?;
    let staged = crate::agent::pairing::stage_bytes(Path::new(&pairing_file), &encoded)
        .map_err(IpcError::from)?;
    let permit = Arc::new(crate::agent::pairing::CommitPermit::new(
        Duration::from_millis(PAIR_COMMIT_TIMEOUT_MS),
    ));
    if let Some(backup) = staged.commit(&permit).map_err(IpcError::from)? {
        let _ = std::fs::remove_file(backup);
    }
    hub.activate_token(bytes);
    request_publish(&service);
    Ok(ContextPairing {
        pipe_name: CONTEXT_PIPE_NAME.to_owned(),
        pairing_file,
    })
}

#[tauri::command]
pub(crate) async fn context_pair_revoke(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
) -> Result<bool, IpcError> {
    check_window(&window)?;
    let hub = hub(&window)?;
    // Revocation clears the pairing and every live registration; existing pipe
    // connections fail on their next frame and recover only with a new pairing.
    let revoked = hub.revoke_token();
    request_publish(&service);
    Ok(revoked)
}
