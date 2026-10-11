//! User-confirmed pairing and asynchronous UIA work, isolated from Previewer actor work.
use super::inspection::{data_request, decode_data, json_result};
use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

enum AssistedResult {
    Offer(AssistedOffer),
    Fill(AssistedFill),
}
pub(super) struct PendingAssisted {
    scope: RequestScope,
    selection: Option<SelectionContext>,
    delivery_id: Option<String>,
    cancelled: Arc<AtomicBool>,
    started_at: u64,
    receiver: mpsc::Receiver<Result<AssistedResult, String>>,
    reply: mpsc::Sender<Result<DataResult, String>>,
}
impl Engine {
    pub(super) fn cancel_assisted(&mut self) {
        self.assisted_offer_scope = None;
        if let Some(pending) = &self.pending_assisted {
            pending.cancelled.store(true, Ordering::Release);
        }
        if let Ok(mut controller) = self.assisted.try_lock() {
            controller.invalidate();
        }
    }
    pub(super) fn start_assisted(
        &mut self,
        scope: RequestScope,
        action: DataAction,
        reply: mpsc::Sender<Result<DataResult, String>>,
    ) -> Result<(), String> {
        if self.pending_assisted.is_some() || self.assisted_busy.load(Ordering::Acquire) {
            return Err("ASSISTED_REQUEST_PENDING".into());
        }
        let target = self.current_target()?;
        #[cfg(windows)]
        let handle = self
            .controller
            .as_ref()
            .ok_or("WINDOWING_UNAVAILABLE")?
            .discovery_scope(&target.id)?
            .handle;
        #[cfg(not(windows))]
        let handle = 0;
        let (selection, delivery, confirmation) = match action {
            DataAction::AssistedInspect => {
                self.assisted_offer_scope = None;
                (None, None, None)
            }
            DataAction::AssistedFill(confirmation, id) => {
                if self.assisted_offer_scope.take().as_ref() != Some(&scope) {
                    return Err("ASSISTED_SCOPE_CHANGED_RECONFIRM_REQUIRED".into());
                }
                let selection = self
                    .selections
                    .as_mut()
                    .ok_or("SELECTION_STORAGE_UNAVAILABLE")?
                    .get(&self.snapshot, &id, now())?;
                let delivery =
                    self.agents
                        .assisted_delivery(&self.snapshot, &selection, None, now())?;
                if delivery.state != DeliveryState::Sending {
                    let _ = reply.send(json_result(delivery));
                    return Ok(());
                }
                (Some(selection), Some(delivery), Some(confirmation))
            }
            _ => return Err("ASSISTED_ACTION_INVALID".into()),
        };
        let prompt = delivery.as_ref().map(|d| d.prompt.clone());
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancel = cancelled.clone();
        let controller = self.assisted.clone();
        let busy = self.assisted_busy.clone();
        let caller = self.own_handle;
        let (sender, receiver) = mpsc::channel();
        busy.store(true, Ordering::Release);
        thread::spawn(move || {
            let result = (|| {
                let mut controller = controller.lock().map_err(|_| "ASSISTED_UNAVAILABLE")?;
                controller.set_cancellation(cancel);
                if let Some(id) = confirmation {
                    controller
                        .fill(
                            handle,
                            &target,
                            caller,
                            &id,
                            prompt.as_deref().ok_or("ASSISTED_PROMPT_REQUIRED")?,
                            now(),
                        )
                        .map(AssistedResult::Fill)
                } else {
                    controller
                        .inspect(handle, &target, caller, now())
                        .map(AssistedResult::Offer)
                }
            })();
            busy.store(false, Ordering::Release);
            let _ = sender.send(result);
        });
        self.pending_assisted = Some(PendingAssisted {
            scope,
            selection,
            delivery_id: delivery.map(|d| d.request_id),
            cancelled,
            started_at: now(),
            receiver,
            reply,
        });
        Ok(())
    }
    pub(super) fn poll_assisted(&mut self) {
        let Some(pending) = self.pending_assisted.take() else {
            return;
        };
        let valid = self.validate_data_scope(&pending.scope).and_then(|()| {
            if pending.cancelled.load(Ordering::Acquire) {
                return Err("ASSISTED_CANCELLED".into());
            }
            if let Some(selection) = &pending.selection {
                crate::selection::validate_selection(&self.snapshot, selection, now())
            } else {
                Ok(())
            }
        });
        let result = if let Err(error) = valid {
            Err(error)
        } else {
            match pending.receiver.try_recv() {
                Ok(result) => result,
                Err(mpsc::TryRecvError::Empty)
                    if now().saturating_sub(pending.started_at) < 2500 =>
                {
                    self.pending_assisted = Some(pending);
                    return;
                }
                Err(_) => Err("ASSISTED_TIMEOUT_OR_CANCELLED".into()),
            }
        };
        pending.cancelled.store(true, Ordering::Release);
        let result = match (pending.delivery_id, result) {
            (Some(id), Ok(AssistedResult::Fill(result))) => self
                .agents
                .finish_assisted(&id, result, now())
                .and_then(json_result),
            (Some(id), Err(error)) => self
                .agents
                .finish_assisted(
                    &id,
                    AssistedFill {
                        state: DeliveryState::Unknown,
                        receipt_id: None,
                        reason: format!("{error}；填写结果需人工核验，不会自动重试或提交。"),
                    },
                    now(),
                )
                .and_then(json_result),
            (None, Ok(AssistedResult::Offer(offer))) => {
                self.assisted_offer_scope = Some(pending.scope);
                json_result(offer)
            }
            (_, Err(error)) => Err(error),
            _ => Err("ASSISTED_REPLY_INVALID".into()),
        };
        let _ = pending.reply.send(result);
    }
}

#[tauri::command]
pub(crate) async fn agent_pair_create(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
    scope: RequestScope,
    agent_kind: AgentKind,
) -> Result<AgentPairing, String> {
    check_window(&window)?;
    validate_scope(&service.snapshot()?, &scope)?;
    let path = rfd::AsyncFileDialog::new()
        .set_title("保存本次 Agent 配对凭证（仅交给指定本地 Agent）")
        .set_file_name("arkdock-pairing.json")
        .save_file()
        .await
        .ok_or("PAIRING_SAVE_CANCELLED")?;
    let mut pairing: AgentPairing = decode_data(
        data_request(window, service, scope, DataAction::PairCreate(agent_kind)).await?,
    )?;
    pairing.config_path = Some(path.path().to_string_lossy().into_owned());
    let bytes = serde_json::to_vec_pretty(&pairing).map_err(|_| "PAIRING_SERIALIZE_FAILED")?;
    tauri::async_runtime::spawn_blocking(move || {
        std::fs::write(path.path(), bytes).map_err(|_| "PAIRING_WRITE_FAILED".to_owned())
    })
    .await
    .map_err(|_| "PAIRING_WORKER_FAILED".to_owned())??;
    Ok(pairing)
}
#[tauri::command]
pub(crate) async fn agent_pair_revoke(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
    scope: RequestScope,
) -> Result<bool, String> {
    decode_data(data_request(window, service, scope, DataAction::PairRevoke).await?)
}
#[tauri::command]
pub(crate) async fn agent_registration_bind(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
    scope: RequestScope,
    registration_id: String,
) -> Result<AgentSessionBinding, String> {
    decode_data(
        data_request(
            window,
            service,
            scope,
            DataAction::RegistrationBind(registration_id),
        )
        .await?,
    )
}
#[tauri::command]
pub(crate) async fn agent_assisted_inspect(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
    scope: RequestScope,
) -> Result<AssistedOffer, String> {
    decode_data(data_request(window, service, scope, DataAction::AssistedInspect).await?)
}
#[tauri::command]
pub(crate) async fn agent_assisted_fill(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
    scope: RequestScope,
    confirmation_id: String,
    selection_id: String,
) -> Result<AgentDelivery, String> {
    decode_data(
        data_request(
            window,
            service,
            scope,
            DataAction::AssistedFill(confirmation_id, selection_id),
        )
        .await?,
    )
}
