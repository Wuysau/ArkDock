//! Pre-docking pairing for the agent context source.
//!
//! The context source must be pairable BEFORE any project or preview session exists
//! (otherwise discovery could never bootstrap: no project to trust, no session to
//! scope with). These commands therefore validate the window only — not
//! [`super::Engine::validate_data_scope`] — and never touch execution trust. The
//! credential is persisted to both the user-chosen file and app-owned state
//! before activation, so pairing survives an ordinary application restart.

use std::sync::Arc;

use tauri::Manager;

use crate::context::ContextHub;
use crate::context_pairing_store::ContextPairingStore;
use crate::ipc::model::{ContextPairing, IpcError};

use super::{check_window, Message, Service};

/// Managed handle shared by the pairing commands, the pipe listener and the engine.
pub(crate) struct ContextHubHandle {
    hub: Arc<ContextHub>,
    pairing: Arc<ContextPairingStore>,
}

impl ContextHubHandle {
    pub(crate) fn new(hub: Arc<ContextHub>, pairing: Arc<ContextPairingStore>) -> Self {
        Self { hub, pairing }
    }
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
    let path = rfd::AsyncFileDialog::new()
        .set_title("保存 ArkDock 上下文源配对凭证（只交给本机 Agent 工具）")
        .set_file_name("arkdock-context-pairing.json")
        .save_file()
        .await
        .ok_or(IpcError::from("CONTEXT_PAIRING_SAVE_CANCELLED"))?;
    let handle = window.state::<ContextHubHandle>();
    let pairing = handle
        .pairing
        .create(path.path(), &handle.hub)
        .map_err(IpcError::from)?;
    request_publish(&service);
    Ok(pairing)
}

#[tauri::command]
pub(crate) async fn context_pair_revoke(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
) -> Result<bool, IpcError> {
    check_window(&window)?;
    let handle = window.state::<ContextHubHandle>();
    // Revocation clears the pairing and every live registration; existing pipe
    // connections fail on their next frame and recover only with a new pairing.
    let revoked = handle.pairing.revoke(&handle.hub).map_err(IpcError::from)?;
    request_publish(&service);
    Ok(revoked)
}
