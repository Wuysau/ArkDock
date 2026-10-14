//! User-confirmed pairing and asynchronous UIA work, isolated from Previewer actor work.
use super::inspection::{data_request, decode_data, json_result};
use super::*;
use crate::agent::pairing::{CommitPermit, StagedCredential};
use std::sync::atomic::{AtomicBool, Ordering};

enum AssistedResult {
    Offer(AssistedOffer),
    Fill(AssistedFill),
}

/// A pairing whose credential file is being written.
///
/// The credential is staged before the write and activated only after it is
/// confirmed durable, so a failed save leaves the previous credential usable and
/// a credential file that was never activated grants nothing.
pub(super) struct PendingPair {
    scope: RequestScope,
    pairing: AgentPairing,
    permit: Arc<CommitPermit>,
    receiver: mpsc::Receiver<Result<StagedCredential, String>>,
    reply: mpsc::Sender<Result<DataResult, String>>,
}

/// The write must settle while the calling command is still waiting, otherwise
/// the command reports its own timeout and the report would be ambiguous.
const PAIR_COMMIT_TIMEOUT_MS: u64 = 4_000;

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
    /// Stages a pairing credential and starts persisting it.
    ///
    /// Nothing the current pairing authorizes changes here; the reply is sent
    /// from [`Self::poll_pair`] once the write settles.
    pub(super) fn start_pair(
        &mut self,
        scope: RequestScope,
        kind: AgentKind,
        credentials: String,
        permit: Arc<CommitPermit>,
        reply: mpsc::Sender<Result<DataResult, String>>,
    ) -> Result<(), String> {
        if self.pending_pair.is_some() {
            return Err("AGENT_PAIRING_IN_PROGRESS".into());
        }
        if permit.expired() {
            return Err("PAIRING_WRITE_TIMEOUT".into());
        }
        let now = now();
        let pairing = self
            .agents
            .prepare_pair(&self.snapshot, kind, &credentials, now)?;
        let (sender, receiver) = mpsc::channel();
        let written = pairing.clone();
        // Off the actor thread: an unreachable or very slow directory must not
        // stall preview work, and activation waits for this result only.
        thread::spawn(move || {
            let _ = sender.send(crate::agent::pairing::stage_credential(
                std::path::Path::new(&credentials),
                &written,
            ));
        });
        self.pending_pair = Some(PendingPair {
            scope,
            pairing,
            permit,
            receiver,
            reply,
        });
        Ok(())
    }

    pub(super) fn poll_pair(&mut self) {
        let Some(pending) = self.pending_pair.take() else {
            return;
        };
        let written = if pending.permit.expired() {
            Err("PAIRING_WRITE_TIMEOUT".into())
        } else {
            match pending.receiver.try_recv() {
                Ok(result) => result,
                Err(mpsc::TryRecvError::Empty) if !pending.permit.expired() => {
                    self.pending_pair = Some(pending);
                    return;
                }
                Err(mpsc::TryRecvError::Empty) => Err("PAIRING_WRITE_TIMEOUT".into()),
                Err(mpsc::TryRecvError::Disconnected) => Err("PAIRING_WRITE_FAILED".into()),
            }
        };
        let id = pending.pairing.pairing_id.clone();
        // The write is the commit point: only a durable credential may become
        // authoritative, and every failure leaves the previous pairing in place.
        let mut backup = None;
        let result = written.and_then(|staged| {
            self.validate_data_scope(&pending.scope)?;
            // Serialization is fallible, so it also precedes the commit point.
            let reply = json_result(pending.pairing.clone())?;
            // An explicit replacement revokes prior delivery grants. Their
            // cleanup failure rejects the replacement before touching either
            // the credential file or the current bearer token.
            if let Some(store) = &self.selections {
                store.revoke_delivery_grants()?;
            }
            self.agents.commit_pair(&id, &self.snapshot, now(), || {
                backup = staged.commit(&pending.permit)?;
                Ok(())
            })?;
            Ok(reply)
        });
        if result.is_err() {
            pending.permit.cancel();
            self.agents.discard_pair(&id);
        }
        let _ = pending.reply.send(result);
        if let Some(backup) = backup {
            let _ = thread::Builder::new()
                .name("arkdock-pair-cleanup".into())
                .spawn(move || {
                    let _ = std::fs::remove_file(backup);
                });
        }
    }

    pub(super) fn cancel_pairing(&mut self) {
        if let Some(pending) = self.pending_pair.take() {
            pending.permit.cancel();
            self.agents.discard_pair(&pending.pairing.pairing_id);
            let _ = pending.reply.send(Err("PAIRING_CANCELLED".into()));
        }
    }

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
    let credentials = path.path().to_string_lossy().into_owned();
    // Staged, persisted, then activated inside the actor. A save that fails
    // leaves the previously paired credential usable, and the reply only
    // reports a pairing the backend actually holds.
    decode_data(
        data_request(
            window,
            service,
            scope,
            DataAction::PairCreate(
                agent_kind,
                credentials,
                Arc::new(CommitPermit::new(Duration::from_millis(
                    PAIR_COMMIT_TIMEOUT_MS,
                ))),
            ),
        )
        .await?,
    )
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
