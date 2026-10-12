//! Inspector queries and selection/Agent commands, isolated from desktop window actions.
use super::*;

pub(super) enum DataAction {
    InspectorCapabilities,
    ControlState,
    ControlEnable(bool),
    ControlSubmit(NativeControlRequest),
    ControlCancel(NativeControlIdentity),
    DeviceState,
    DeviceAction(DeviceRuntimeRequest),
    InspectorTree(BinaryFrameMetadata),
    SelectionCreate(SelectionDraft),
    SelectionGet(String),
    SelectionClear(Option<String>),
    SelectionInstruction(String, String),
    SelectionPrompt(String, bool),
    SelectionImage(String, bool),
    AgentList,
    AgentBind(AgentBindingDraft),
    PairCreate(AgentKind),
    PairRevoke,
    RegistrationBind(String),
    AssistedInspect,
    AssistedFill(String, String),
    DeliveryPrepare(String, Option<String>),
    DeliverySend(String),
    DeliveryStatus(String),
}
pub(super) enum DataResult {
    Json(serde_json::Value),
    Bytes(Vec<u8>),
}
pub(super) fn json_result<T: serde::Serialize>(value: T) -> Result<DataResult, String> {
    serde_json::to_value(value)
        .map(DataResult::Json)
        .map_err(|_| "DATA_SERIALIZE_FAILED".into())
}
pub(super) struct PendingInspection {
    frame: BinaryFrameMetadata,
    anchor: BinaryFrame,
    scope: RequestScope,
    started_at: u64,
    receiver: crate::inspector::InspectionReceiver,
    pub(super) reply: mpsc::Sender<Result<DataResult, String>>,
}

impl Engine {
    pub(super) fn validate_data_scope(&self, scope: &RequestScope) -> Result<(), String> {
        validate_scope(&self.snapshot, scope)?;
        self.current_target()?;
        let project = self.snapshot.project.as_ref().ok_or("PROJECT_REQUIRED")?;
        if !self.snapshot.trusted
            || !self
                .store
                .as_ref()
                .is_some_and(|store| store.is_trusted(project))
        {
            return Err("TRUST_REQUIRED".into());
        }
        Ok(())
    }
    fn validate_display_frame(&self, frame: &BinaryFrameMetadata) -> Result<BinaryFrame, String> {
        self.ensure_native_surface_ready()?;
        if self.snapshot.phase != Phase::Running
            || self.snapshot.build.stale
            || self.snapshot.build.error.is_some()
            || self.snapshot.project.as_ref().map(|p| p.id.as_str())
                != Some(frame.workspace_id.as_str())
            || self.snapshot.session_id.as_deref() != Some(frame.session_id.as_str())
            || self.snapshot.build.build_id.as_deref() != Some(frame.build_id.as_str())
            || self.snapshot.build.generation != frame.generation
        {
            return Err("STALE_SELECTION_FRAME".into());
        }
        self.frames
            .lock()
            .map_err(|_| "FRAME_UNAVAILABLE")?
            .retained
            .iter()
            .find(|f| f.metadata == *frame)
            .cloned()
            .ok_or("DISPLAYED_FRAME_EXPIRED: 画面已更新，请重新选择".into())
    }
    pub(super) fn dispatch_data(
        &mut self,
        scope: RequestScope,
        action: DataAction,
        reply: mpsc::Sender<Result<DataResult, String>>,
    ) {
        self.drain_events();
        let result = (|| {
            self.validate_data_scope(&scope)?;
            if matches!(
                &action,
                DataAction::AssistedInspect | DataAction::AssistedFill(_, _)
            ) {
                self.start_assisted(scope.clone(), action, reply.clone())?;
                return Ok(None);
            }
            if let DataAction::InspectorTree(frame) = &action {
                let anchor = self.validate_display_frame(frame)?;
                if self.inspection.is_some() {
                    return Err("INSPECTOR_QUERY_PENDING".into());
                }
                if now().saturating_sub(self.last_inspection_at) < 250 {
                    return Err("INSPECTOR_QUERY_THROTTLED".into());
                }
                let receiver = self
                    .backend
                    .request_inspection(crate::inspector::InspectorCommand::Inspector)?;
                self.last_inspection_at = now();
                self.inspection = Some(PendingInspection {
                    frame: frame.clone(),
                    anchor,
                    scope: scope.clone(),
                    started_at: now(),
                    receiver,
                    reply: reply.clone(),
                });
                return Ok(None);
            }
            let observed = now();
            let data = match action {
                DataAction::ControlState => json_result(self.control_state()),
                DataAction::ControlEnable(enabled) => json_result(self.enable_controls(enabled)?),
                DataAction::ControlSubmit(request) => json_result(self.submit_control(request)?),
                DataAction::ControlCancel(identity) => json_result(self.cancel_control(identity, false)?),
                DataAction::DeviceState => json_result(self.device_state()),
                DataAction::DeviceAction(request) => json_result(self.device_action(request)?),
                DataAction::InspectorCapabilities => {
                    json_result(crate::inspector::capabilities(&self.snapshot.backend))
                }
                DataAction::InspectorTree(_) => unreachable!(),
                DataAction::SelectionCreate(draft) => {
                    let frame = self.validate_display_frame(&draft.frame)?;
                    let node = self
                        .inspection_tree
                        .as_ref()
                        .filter(|tree| {
                            tree.workspace_id == draft.frame.workspace_id
                                && tree.session_id == draft.frame.session_id
                                && tree.build_id == draft.frame.build_id
                                && tree.generation == draft.frame.generation
                                && tree.frame_sequence <= draft.frame.sequence
                                && observed.saturating_sub(tree.received_at) <= 2000
                                && self
                                    .inspection_anchor
                                    .as_ref()
                                    .is_some_and(|anchor| anchor.payload == frame.payload)
                                && Some(tree.inspection_version) == draft.inspection_version
                        })
                        .and_then(|tree| {
                            tree.nodes
                                .iter()
                                .find(|n| Some(&n.node_id) == draft.node_id.as_ref())
                        })
                        .cloned();
                    if draft.node_id.is_some() && node.is_none() {
                        return Err("STALE_INSPECTOR_SELECTION".into());
                    }
                    json_result(
                        self.selections
                            .as_mut()
                            .ok_or("SELECTION_STORAGE_UNAVAILABLE")?
                            .create(&self.snapshot, &frame, &draft, node, observed)?,
                    )
                }
                DataAction::SelectionGet(id) => json_result(
                    self.selections
                        .as_mut()
                        .ok_or("SELECTION_STORAGE_UNAVAILABLE")?
                        .get(&self.snapshot, &id, observed)?,
                ),
                DataAction::SelectionClear(id) => {
                    self.selections
                        .as_mut()
                        .ok_or("SELECTION_STORAGE_UNAVAILABLE")?
                        .clear(&self.snapshot, id.as_deref(), observed)?;
                    json_result(true)
                }
                DataAction::SelectionInstruction(id, instruction) => json_result(
                    self.selections
                        .as_mut()
                        .ok_or("SELECTION_STORAGE_UNAVAILABLE")?
                        .update_instruction(&self.snapshot, &id, &instruction, observed)?,
                ),
                DataAction::SelectionPrompt(id, detailed) => json_result(
                    self.selections
                        .as_mut()
                        .ok_or("SELECTION_STORAGE_UNAVAILABLE")?
                        .prompt_with_detail(&self.snapshot, &id, observed, detailed)?,
                ),
                DataAction::SelectionImage(id, crop) => Ok(DataResult::Bytes(
                    self.selections
                        .as_mut()
                        .ok_or("SELECTION_STORAGE_UNAVAILABLE")?
                        .read_image(&self.snapshot, &id, crop, observed)?,
                )),
                DataAction::AgentList => json_result(self.agents.list(&self.snapshot, observed)),
                DataAction::AgentBind(draft) => {
                    json_result(self.agents.bind(&self.snapshot, &draft, observed)?)
                }
                DataAction::PairCreate(kind) => {
                    if let Some(store) = &self.selections {
                        store.revoke_delivery_grants()?;
                    }
                    json_result(self.agents.create_pair(&self.snapshot, kind, observed)?)
                }
                DataAction::PairRevoke => {
                    self.agents.revoke();
                    if let Some(store) = &self.selections {
                        store.revoke_delivery_grants()?;
                    }
                    json_result(true)
                }
                DataAction::RegistrationBind(id) => json_result(self.agents.bind_registration(
                    &self.snapshot,
                    &id,
                    observed,
                )?),
                DataAction::AssistedInspect | DataAction::AssistedFill(_, _) => unreachable!(),
                DataAction::DeliveryPrepare(id, binding) => {
                    let selection = self
                        .selections
                        .as_mut()
                        .ok_or("SELECTION_STORAGE_UNAVAILABLE")?
                        .get(&self.snapshot, &id, observed)?;
                    json_result(self.agents.prepare(
                        &self.snapshot,
                        &selection,
                        binding.as_deref(),
                        observed,
                    )?)
                }
                DataAction::DeliveryStatus(id) => {
                    json_result(self.agents.status(&self.snapshot, &id, observed)?)
                }
                DataAction::DeliverySend(id) => {
                    let prepared = self.agents.status(&self.snapshot, &id, observed)?;
                    if prepared.state != DeliveryState::Ready {
                        return Ok(Some(json_result(prepared)?));
                    }
                    let selection = self
                        .selections
                        .as_mut()
                        .ok_or("SELECTION_STORAGE_UNAVAILABLE")?
                        .get(&self.snapshot, &prepared.selection_id, observed)?;
                    let storage = self
                        .selections
                        .as_mut()
                        .ok_or("SELECTION_STORAGE_UNAVAILABLE")?;
                    storage.read_image(&self.snapshot, &selection.selection_id, false, observed)?;
                    storage.read_image(&self.snapshot, &selection.selection_id, true, observed)?;
                    let attachments = storage.authorize_delivery(
                        &self.snapshot,
                        &selection.selection_id,
                        &id,
                        observed,
                    )?;
                    json_result(self.agents.send_with_attachments(
                        &self.snapshot,
                        &selection,
                        &id,
                        attachments,
                        observed,
                    )?)
                }
            }?;
            Ok(Some(data))
        })();
        match result {
            Ok(None) => {}
            Ok(Some(data)) => {
                let _ = reply.send(Ok(data));
            }
            Err(error) => {
                let _ = reply.send(Err(error));
            }
        }
    }
    pub(super) fn poll_inspection(&mut self) {
        let Some(pending) = self.inspection.take() else {
            return;
        };
        let response = match pending.receiver.try_recv() {
            Ok(response) => response,
            Err(mpsc::TryRecvError::Empty) if now().saturating_sub(pending.started_at) < 2500 => {
                self.inspection = Some(pending);
                return;
            }
            Err(mpsc::TryRecvError::Empty) => Err("INSPECTOR_QUERY_TIMEOUT".into()),
            Err(mpsc::TryRecvError::Disconnected) => Err("INSPECTOR_QUERY_CANCELLED".into()),
        };
        let result = (|| {
            self.validate_data_scope(&pending.scope)?;
            let latest = self
                .frames
                .lock()
                .map_err(|_| "FRAME_UNAVAILABLE")?
                .retained
                .back()
                .cloned()
                .ok_or("FRAME_UNAVAILABLE")?;
            self.validate_display_frame(&latest.metadata)?;
            if latest.payload != pending.anchor.payload {
                return Err("INSPECTOR_IMAGE_CHANGED: 画面变化后须刷新组件树".into());
            }
            let tree = crate::inspector::parse_tree(&response?, &pending.frame)?;
            self.inspection_tree = Some(tree.clone());
            self.inspection_anchor = Some(pending.anchor);
            json_result(tree)
        })();
        let _ = pending.reply.send(result);
    }
}

pub(super) async fn data_request(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
    scope: RequestScope,
    action: DataAction,
) -> Result<DataResult, String> {
    check_window(&window)?;
    validate_scope(&service.snapshot()?, &scope)?;
    let sender = service.sender.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let (send, receive) = mpsc::channel();
        sender
            .try_send(Message::Data(scope, action, send))
            .map_err(|_| "REQUEST_QUEUE_BUSY".to_owned())?;
        receive
            .recv_timeout(Duration::from_secs(5))
            .map_err(|_| "DATA_REQUEST_TIMEOUT".to_owned())?
    })
    .await
    .map_err(|_| "REQUEST_WORKER_FAILED".to_owned())?
}
pub(super) fn decode_data<T: serde::de::DeserializeOwned>(result: DataResult) -> Result<T, String> {
    match result {
        DataResult::Json(value) => {
            serde_json::from_value(value).map_err(|_| "DATA_REPLY_INVALID".into())
        }
        DataResult::Bytes(_) => Err("DATA_REPLY_TYPE_MISMATCH".into()),
    }
}
#[tauri::command]
pub(crate) async fn inspector_get_capabilities(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
    scope: RequestScope,
) -> Result<InspectorCapabilities, String> {
    decode_data(data_request(window, service, scope, DataAction::InspectorCapabilities).await?)
}
#[tauri::command]
pub(crate) async fn inspector_get_tree(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
    scope: RequestScope,
    frame: BinaryFrameMetadata,
) -> Result<InspectorTree, String> {
    decode_data(data_request(window, service, scope, DataAction::InspectorTree(frame)).await?)
}
#[tauri::command]
pub(crate) async fn selection_create(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
    scope: RequestScope,
    draft: SelectionDraft,
) -> Result<SelectionContext, String> {
    decode_data(data_request(window, service, scope, DataAction::SelectionCreate(draft)).await?)
}
#[tauri::command]
pub(crate) async fn selection_get(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
    scope: RequestScope,
    selection_id: String,
) -> Result<SelectionContext, String> {
    decode_data(
        data_request(
            window,
            service,
            scope,
            DataAction::SelectionGet(selection_id),
        )
        .await?,
    )
}
#[tauri::command]
pub(crate) async fn selection_clear(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
    scope: RequestScope,
    selection_id: Option<String>,
) -> Result<bool, String> {
    decode_data(
        data_request(
            window,
            service,
            scope,
            DataAction::SelectionClear(selection_id),
        )
        .await?,
    )
}
#[tauri::command]
pub(crate) async fn selection_update_instruction(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
    scope: RequestScope,
    selection_id: String,
    user_instruction: String,
) -> Result<SelectionContext, String> {
    decode_data(
        data_request(
            window,
            service,
            scope,
            DataAction::SelectionInstruction(selection_id, user_instruction),
        )
        .await?,
    )
}
#[tauri::command]
pub(crate) async fn selection_prompt(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
    scope: RequestScope,
    selection_id: String,
    detailed: Option<bool>,
) -> Result<String, String> {
    decode_data(
        data_request(
            window,
            service,
            scope,
            DataAction::SelectionPrompt(selection_id, detailed.unwrap_or(false)),
        )
        .await?,
    )
}
#[tauri::command]
pub(crate) async fn selection_read_image(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
    scope: RequestScope,
    selection_id: String,
    crop: bool,
) -> Result<tauri::ipc::Response, String> {
    match data_request(
        window,
        service,
        scope,
        DataAction::SelectionImage(selection_id, crop),
    )
    .await?
    {
        DataResult::Bytes(bytes) => Ok(tauri::ipc::Response::new(bytes)),
        _ => Err("DATA_REPLY_TYPE_MISMATCH".into()),
    }
}
#[tauri::command]
pub(crate) async fn selection_export_image(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
    scope: RequestScope,
    selection_id: String,
    crop: bool,
) -> Result<bool, String> {
    check_window(&window)?;
    validate_scope(&service.snapshot()?, &scope)?;
    let name = if crop {
        "arkdock-selection-crop.jpg"
    } else {
        "arkdock-native-preview.jpg"
    };
    let path = rfd::AsyncFileDialog::new()
        .set_title("导出已选择的真实原生画面")
        .set_file_name(name)
        .save_file()
        .await
        .ok_or("EXPORT_CANCELLED")?;
    let bytes = match data_request(
        window,
        service,
        scope,
        DataAction::SelectionImage(selection_id, crop),
    )
    .await?
    {
        DataResult::Bytes(bytes) => bytes,
        _ => return Err("DATA_REPLY_TYPE_MISMATCH".into()),
    };
    tauri::async_runtime::spawn_blocking(move || {
        std::fs::write(path.path(), bytes).map_err(|_| "EXPORT_WRITE_FAILED".to_owned())
    })
    .await
    .map_err(|_| "EXPORT_WORKER_FAILED".to_owned())??;
    Ok(true)
}
#[tauri::command]
pub(crate) async fn agent_session_list(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
    scope: RequestScope,
) -> Result<AgentSessionList, String> {
    decode_data(data_request(window, service, scope, DataAction::AgentList).await?)
}
#[tauri::command]
pub(crate) async fn agent_session_bind(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
    scope: RequestScope,
    draft: AgentBindingDraft,
) -> Result<AgentSessionBinding, String> {
    decode_data(data_request(window, service, scope, DataAction::AgentBind(draft)).await?)
}
#[tauri::command]
pub(crate) async fn agent_delivery_prepare(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
    scope: RequestScope,
    selection_id: String,
    binding_id: Option<String>,
) -> Result<AgentDelivery, String> {
    decode_data(
        data_request(
            window,
            service,
            scope,
            DataAction::DeliveryPrepare(selection_id, binding_id),
        )
        .await?,
    )
}
#[tauri::command]
pub(crate) async fn agent_delivery_send(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
    scope: RequestScope,
    request_id: String,
) -> Result<AgentDelivery, String> {
    decode_data(data_request(window, service, scope, DataAction::DeliverySend(request_id)).await?)
}
#[tauri::command]
pub(crate) async fn agent_delivery_status(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
    scope: RequestScope,
    request_id: String,
) -> Result<AgentDelivery, String> {
    decode_data(
        data_request(
            window,
            service,
            scope,
            DataAction::DeliveryStatus(request_id),
        )
        .await?,
    )
}
