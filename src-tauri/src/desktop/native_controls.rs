//! Native control and device IPC. Every execution revalidates the current scope.
use super::*;
use crate::preview::configuration::{compiled_pages, compiled_pages_at, profiles_for_project};
use crate::preview::control::ControlReceiver;

struct PendingControl {
    receipt: NativeControlReceipt,
    receiver: ControlReceiver,
}

#[derive(Default)]
pub(super) struct Controls {
    enabled: bool,
    identity: Option<NativeControlIdentity>,
    operations: VecDeque<NativeControlReceipt>,
    pending: Vec<PendingControl>,
    pages: Vec<String>,
    page_reason: String,
    pages_checked: bool,
    serial: u64,
}

fn same_lifecycle(a: &NativeControlIdentity, b: &NativeControlIdentity) -> bool {
    a.workspace_id == b.workspace_id
        && a.session_id == b.session_id
        && a.build_id == b.build_id
        && a.generation == b.generation
}

fn terminal(phase: &NativeControlPhase) -> bool {
    matches!(
        phase,
        NativeControlPhase::CommandAccepted
            | NativeControlPhase::Observed
            | NativeControlPhase::Failed
            | NativeControlPhase::Cancelled
            | NativeControlPhase::TimedOut
            | NativeControlPhase::Blocked
    )
}

fn input_operation(action: &NativeControlAction) -> bool {
    matches!(
        action,
        NativeControlAction::PointerDown
            | NativeControlAction::PointerMove
            | NativeControlAction::PointerUp
            | NativeControlAction::PointerCancel
            | NativeControlAction::TextCommit
            | NativeControlAction::KeyPress
    )
}

impl Controls {
    fn remember(&mut self, receipt: NativeControlReceipt) {
        if let Some(previous) = self
            .operations
            .iter_mut()
            .find(|previous| previous.operation_id == receipt.operation_id)
        {
            *previous = receipt;
        } else {
            if self.operations.len() == 32 {
                self.operations.pop_front();
            }
            self.operations.push_back(receipt);
        }
    }

    pub(super) fn reset(&mut self) {
        self.enabled = true;
        self.identity = None;
        self.pending.clear();
        self.operations.clear();
        self.pages.clear();
        self.page_reason.clear();
        self.pages_checked = false;
    }
}

impl Engine {
    fn control_identity(&self) -> Option<NativeControlIdentity> {
        let status = self.scheduler.status();
        let runtime = self.backend.control_runtime_state();
        let dimensions = runtime
            .as_ref()
            .map(|state| (state.width, state.height))
            .or_else(|| {
                self.frames
                    .lock()
                    .ok()?
                    .retained()
                    .back()
                    .map(|frame| (frame.metadata.width, frame.metadata.height))
            })
            .unwrap_or((360, 640));
        Some(NativeControlIdentity {
            workspace_id: self.snapshot.project.as_ref()?.id.clone(),
            session_id: self.snapshot.session_id.clone()?,
            build_id: status.build_id?,
            generation: status.generation,
            width: dimensions.0,
            height: dimensions.1,
        })
    }

    pub(super) fn sync_control_lifecycle(&mut self) {
        let identity = self.control_identity();
        let changed = match (&self.controls.identity, &identity) {
            (Some(old), Some(current)) => !same_lifecycle(old, current),
            (None, None) => false,
            _ => true,
        };
        if changed {
            self.backend.cancel_controls();
            self.controls.reset();
            self.controls.identity = identity;
        } else {
            self.controls.identity = identity;
        }
    }

    fn native_input_ready(&self) -> bool {
        let status = self.scheduler.status();
        self.snapshot.phase == Phase::Running
            && self.snapshot.trusted
            && self.snapshot.backend.available
            && !status.stale
            && status.error.is_none()
            && self
                .backend
                .control_runtime_state()
                .is_some_and(|runtime| runtime.input_ready)
    }

    pub(super) fn ensure_native_surface_ready(&self) -> Result<(), String> {
        if !self.native_input_ready() {
            return Err("NATIVE_CONTROL_SURFACE_PENDING: 当前表面不可交互或配置尚未确认".into());
        }
        Ok(())
    }

    fn validate_control_identity(
        &self,
        identity: &NativeControlIdentity,
        cleanup: bool,
    ) -> Result<(), String> {
        self.current_target()?;
        let current = self.control_identity().ok_or("PREVIEW_SESSION_REQUIRED")?;
        if !same_lifecycle(identity, &current) {
            return Err("STALE_CONTROL_IDENTITY".into());
        }
        if !cleanup && (identity.width != current.width || identity.height != current.height) {
            return Err("STALE_CONTROL_SURFACE".into());
        }
        if !cleanup && self.native_visible == Some(false) {
            return Err("NATIVE_CONTROL_WINDOW_HIDDEN".into());
        }
        if !cleanup {
            self.ensure_native_surface_ready()?;
        }
        if self.snapshot.project.is_none() || !self.snapshot.trusted {
            return Err("TRUST_REQUIRED".into());
        }
        Ok(())
    }

    pub(super) fn control_state(&mut self) -> NativeControlState {
        self.sync_control_lifecycle();
        self.poll_native_controls();
        let runtime = self.backend.control_runtime_state();
        if self.native_input_ready() && !self.controls.pages_checked {
            if let Some(project) = &self.snapshot.project {
                let pages = match self.backend.artifact_context() {
                    Some(artifacts) => compiled_pages_at(project, &artifacts),
                    None => compiled_pages(project),
                };
                match pages {
                    Ok(pages) => self.controls.pages = pages,
                    Err(error) => self.controls.page_reason = error,
                }
                self.controls.pages_checked = true;
            }
        }
        let available = self.snapshot.backend.available;
        let status = if available {
            NativeControlAvailability::Available
        } else {
            NativeControlAvailability::Unavailable
        };
        let experimental_reason = if available {
            "原生控制已接入，当前画面就绪后可使用。".to_owned()
        } else {
            self.snapshot.backend.reason.clone()
        };
        let mut capabilities: Vec<_> = [
            "touch",
            "text",
            "key",
            "orientation",
            "resolution",
            "colorMode",
        ]
        .into_iter()
        .map(|name| NativeControlCapability {
            name: name.into(),
            status: status.clone(),
            reason: experimental_reason.clone(),
        })
        .collect();
        let route_known = runtime
            .as_ref()
            .and_then(|state| state.current_route.as_ref())
            .is_some_and(|route| self.controls.pages.contains(route));
        capabilities.push(NativeControlCapability {
            name: "page".into(),
            status: if self.controls.pages.len() <= 1 || !route_known {
                NativeControlAvailability::Blocked
            } else {
                status
            },
            reason: if self.controls.pages.len() == 1 {
                "当前编译工程只有一个页面；保持入口页，重新加载请使用预览刷新".into()
            } else if self.controls.pages.is_empty() {
                if self.controls.page_reason.is_empty() {
                    "未接入：当前构建没有可切换的页面。".into()
                } else {
                    self.controls.page_reason.clone()
                }
            } else if !route_known {
                "当前原生路由尚未确认，暂不切换页面，以保留当前画面。".into()
            } else {
                "页面来自当前构建产物；切换结果以原生画面为准。".into()
            },
        });
        let sdk_root = self.backend.verified_sdk_root();
        let profiles = profiles_for_project(
            sdk_root.as_deref(),
            available,
            self.snapshot.project.as_ref(),
        );
        capabilities.push(NativeControlCapability {
            name: "profile".into(),
            status: if profiles
                .iter()
                .any(|p| p.status == NativeControlAvailability::Available)
            {
                NativeControlAvailability::Available
            } else {
                NativeControlAvailability::Blocked
            },
            reason: "按工程支持的设备类型选择；切换将重启预览，失败时恢复上一配置。".into(),
        });
        capabilities.push(NativeControlCapability {
            name: "ime".into(),
            status: if available {
                NativeControlAvailability::Available
            } else {
                NativeControlAvailability::Unavailable
            },
            reason: "使用本机输入法确认文字后发送到原生焦点；候选词和组合编辑在本机输入法完成。"
                .into(),
        });
        NativeControlState {
            identity: self.controls.identity.clone(),
            experimental_enabled: self.controls.enabled,
            input_ready: self.native_input_ready(),
            capabilities,
            operations: self.controls.operations.iter().cloned().collect(),
            pages: self.controls.pages.clone(),
            current_page: runtime
                .as_ref()
                .and_then(|state| state.current_route.clone()),
            profiles,
            current_profile_id: self.backend.profile().id,
            orientation: runtime
                .as_ref()
                .map(|state| state.orientation.clone())
                .unwrap_or(PreviewOrientation::Portrait),
            color_mode: runtime
                .as_ref()
                .map(|state| state.color_mode.clone())
                .unwrap_or(PreviewColorMode::Light),
            width: runtime.as_ref().map(|state| state.width).unwrap_or(360),
            height: runtime.as_ref().map(|state| state.height).unwrap_or(640),
            dpi: runtime.as_ref().map(|state| state.dpi).unwrap_or(160),
            reason: runtime
                .map(|state| state.reason)
                .unwrap_or_else(|| self.snapshot.backend.reason.clone()),
        }
    }

    pub(super) fn enable_controls(&mut self, enabled: bool) -> Result<NativeControlState, String> {
        self.sync_control_lifecycle();
        if enabled {
            let identity = self.control_identity().ok_or("PREVIEW_SESSION_REQUIRED")?;
            self.validate_control_identity(&identity, false)?;
        } else {
            self.backend.cancel_controls();
        }
        self.controls.enabled = enabled;
        Ok(self.control_state())
    }

    pub(super) fn submit_control(
        &mut self,
        request: NativeControlRequest,
    ) -> Result<NativeControlReceipt, String> {
        self.sync_control_lifecycle();
        self.poll_native_controls();
        if request.action == NativeControlAction::PointerCancel {
            return self.cancel_control(request.identity, true);
        }
        if request.action == NativeControlAction::CancelAll {
            return self.cancel_control(request.identity, false);
        }
        let cleanup = matches!(
            request.action,
            NativeControlAction::PointerUp | NativeControlAction::PointerCancel
        );
        self.validate_control_identity(&request.identity, cleanup)?;
        if matches!(
            request.action,
            NativeControlAction::TextCommit | NativeControlAction::KeyPress
        ) && !self
            .app
            .get_webview_window("main")
            .is_some_and(|window| window.is_focused().unwrap_or(false))
        {
            return Err("NATIVE_TEXT_FOCUS_REQUIRED".into());
        }
        if !cleanup && !self.controls.enabled {
            return Err("EXPERIMENTAL_CONTROL_OPT_IN_REQUIRED".into());
        }
        if request.action == NativeControlAction::SwitchProfile {
            crate::preview::control::validate_request(&request)?;
            let project = self.snapshot.project.clone().ok_or("PROJECT_REQUIRED")?;
            self.backend.set_profile(
                &project,
                request.profile_id.as_deref().ok_or("PROFILE_REQUIRED")?,
            )?;
            self.backend.cancel_controls();
            self.backend.stop();
            self.controls.reset();
            if let Ok(mut frames) = self.frames.lock() {
                frames.clear_pixels();
            }
            self.inspection_tree = None;
            self.inspection_anchor = None;
            self.scheduler.request_refresh(now());
            self.snapshot.phase = Phase::Building;
            self.publish();
            return Ok(NativeControlReceipt {
                operation_id: format!("profile-{}", now()),
                identity: request.identity,
                action: request.action,
                phase: NativeControlPhase::CommandAccepted,
                command_accepted: true,
                frame_observed: false,
                pixels_changed: false,
                configuration_confirmed: false,
                current_route: None,
                frame_sequence: None,
                requested_at: now(),
                observed_at: now(),
                reason: "已安排设备配置重启，等待新配置的真实首帧。".into(),
            });
        }
        if self.controls.pending.len() >= 16 {
            if cleanup {
                self.backend.cancel_controls();
            }
            return Err("NATIVE_CONTROL_QUEUE_BUSY: 释放操作使用独立的取消通道。".into());
        }
        if request.action == NativeControlAction::NavigatePage
            && !request
                .page
                .as_ref()
                .is_some_and(|page| self.controls.pages.contains(page))
        {
            return Err("PAGE_NOT_IN_COMPILED_ARTIFACT".into());
        }
        if request.action == NativeControlAction::NavigatePage {
            if self.controls.pages.len() <= 1 {
                return Err("SINGLE_PAGE_PROJECT: 当前工程只有入口页，不执行页面替换；重新加载请使用预览刷新".into());
            }
            let route = self
                .backend
                .control_runtime_state()
                .and_then(|state| state.current_route)
                .filter(|route| self.controls.pages.contains(route))
                .ok_or("NATIVE_CURRENT_ROUTE_UNVERIFIED: 保持当前画面，等待真实路由回读")?;
            if request.page.as_deref() == Some(route.as_str()) {
                return Err("NATIVE_PAGE_ALREADY_CURRENT: 当前页保持不变，不执行页面替换".into());
            }
        }
        if request.action == NativeControlAction::QueryRouter {
            let route = self
                .backend
                .control_runtime_state()
                .and_then(|state| state.current_route);
            return Ok(NativeControlReceipt {
                operation_id: format!("route-{}", now()),
                identity: request.identity,
                action: request.action,
                phase: NativeControlPhase::CommandAccepted,
                command_accepted: true,
                frame_observed: false,
                pixels_changed: false,
                configuration_confirmed: false,
                current_route: route,
                frame_sequence: None,
                requested_at: now(),
                observed_at: now(),
                reason: "返回最近一次原生路由通知；不伪造同步路由读取或页面呈现。".into(),
            });
        }
        if matches!(
            request.action,
            NativeControlAction::SetOrientation
                | NativeControlAction::SetResolution
                | NativeControlAction::SetColorMode
                | NativeControlAction::NavigatePage
        ) {
            self.inspection_tree = None;
            self.inspection_anchor = None;
        }
        self.controls.serial = self
            .controls
            .serial
            .checked_add(1)
            .ok_or("CONTROL_SEQUENCE_EXHAUSTED")?;
        let receipt = NativeControlReceipt {
            operation_id: format!(
                "control-{}-{}",
                request.identity.generation, self.controls.serial
            ),
            identity: request.identity.clone(),
            action: request.action.clone(),
            phase: NativeControlPhase::Queued,
            command_accepted: false,
            frame_observed: false,
            pixels_changed: false,
            configuration_confirmed: false,
            current_route: None,
            frame_sequence: None,
            requested_at: now(),
            observed_at: now(),
            reason: "请求进入本地有界队列；尚未取得原生命令回执".into(),
        };
        let receiver = match self
            .backend
            .request_control(request, receipt.operation_id.clone())
        {
            Ok(receiver) => receiver,
            Err(error) => {
                if cleanup {
                    self.backend.cancel_controls();
                }
                return Err(error);
            }
        };
        self.controls.remember(receipt.clone());
        self.controls.pending.push(PendingControl {
            receipt: receipt.clone(),
            receiver,
        });
        Ok(receipt)
    }

    pub(super) fn cancel_control(
        &mut self,
        identity: NativeControlIdentity,
        input_only: bool,
    ) -> Result<NativeControlReceipt, String> {
        self.validate_control_identity(&identity, true)?;
        let current = self.control_identity().ok_or("PREVIEW_SESSION_REQUIRED")?;
        // UI cancellation has an observed reply; lifecycle cancellation uses the
        // independent priority flag. Never let a full normal queue prevent Up.
        let mut preserved = Vec::new();
        for mut pending in std::mem::take(&mut self.controls.pending) {
            if input_only && !input_operation(&pending.receipt.action) {
                preserved.push(pending);
                continue;
            }
            pending.receipt.phase = NativeControlPhase::Cancelled;
            pending.receipt.observed_at = now();
            pending.receipt.reason = if input_only {
                "本地输入已撤销；配置操作仍在继续，原生释放由独立回执确认".into()
            } else {
                "本地操作已撤销；原生释放状态由取消操作的独立回执确认".into()
            };
            self.controls.remember(pending.receipt);
        }
        self.controls.pending = preserved;
        self.controls.serial = self
            .controls
            .serial
            .checked_add(1)
            .ok_or("CONTROL_SEQUENCE_EXHAUSTED")?;
        let receipt = NativeControlReceipt {
            operation_id: format!("cancel-{}-{}", identity.generation, self.controls.serial),
            identity,
            action: if input_only {
                NativeControlAction::PointerCancel
            } else {
                NativeControlAction::CancelAll
            },
            phase: NativeControlPhase::Queued,
            command_accepted: false,
            frame_observed: false,
            pixels_changed: false,
            configuration_confirmed: false,
            current_route: None,
            frame_sequence: None,
            requested_at: now(),
            observed_at: now(),
            reason: if input_only {
                "已请求仅取消输入并安全释放；页面和主题配置观察保持".into()
            } else {
                "已请求取消全部操作和安全释放；本地取消请求不代表原生已松开".into()
            },
        };
        let request = NativeControlRequest {
            identity: current,
            action: receipt.action.clone(),
            gesture_id: None,
            x: None,
            y: None,
            text: None,
            key: None,
            orientation: None,
            color_mode: None,
            page: None,
            profile_id: None,
            width: None,
            height: None,
            dpi: None,
        };
        let receiver = match self
            .backend
            .request_control(request, receipt.operation_id.clone())
        {
            Ok(receiver) => receiver,
            Err(error) => {
                self.backend.cancel_controls();
                return Err(error);
            }
        };
        self.controls.remember(receipt.clone());
        self.controls.pending.push(PendingControl {
            receipt: receipt.clone(),
            receiver,
        });
        Ok(receipt)
    }

    pub(super) fn poll_native_controls(&mut self) {
        let current = self.control_identity();
        let mut keep = Vec::new();
        for mut pending in std::mem::take(&mut self.controls.pending) {
            if !current
                .as_ref()
                .is_some_and(|current| same_lifecycle(current, &pending.receipt.identity))
            {
                continue;
            }
            let mut finished = false;
            loop {
                match pending.receiver.try_recv() {
                    Ok(Ok(receipt)) => {
                        if receipt.operation_id != pending.receipt.operation_id
                            || !same_lifecycle(&receipt.identity, &pending.receipt.identity)
                        {
                            continue;
                        }
                        finished = terminal(&receipt.phase);
                        pending.receipt = receipt.clone();
                        let _ = self.app.emit("arkdock:control-receipt", &receipt);
                        self.controls.remember(receipt);
                        if finished {
                            break;
                        }
                    }
                    Ok(Err(error)) => {
                        pending.receipt.phase = NativeControlPhase::Failed;
                        pending.receipt.reason = error;
                        pending.receipt.observed_at = now();
                        let _ = self.app.emit("arkdock:control-receipt", &pending.receipt);
                        self.controls.remember(pending.receipt.clone());
                        finished = true;
                        break;
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        if !terminal(&pending.receipt.phase) {
                            pending.receipt.phase = NativeControlPhase::Cancelled;
                            pending.receipt.reason = "原生控制通道已关闭，未取得完成证据".into();
                            pending.receipt.observed_at = now();
                            let _ = self.app.emit("arkdock:control-receipt", &pending.receipt);
                            self.controls.remember(pending.receipt.clone());
                        }
                        finished = true;
                        break;
                    }
                }
            }
            if !finished {
                keep.push(pending);
            }
        }
        self.controls.pending = keep;
    }

    pub(super) fn device_state(&mut self) -> DeviceRuntimeState {
        let sdk = self
            .backend
            .verified_sdk_root()
            .or_else(|| self.backend.configured_sdk());
        if self.device_sdk != sdk {
            self.device_runtime.cancel();
            self.device_runtime =
                crate::preview::device_runtime::DeviceRuntimeAdapter::new(sdk.clone());
            self.device_sdk = sdk;
        }
        let project = self
            .snapshot
            .project
            .as_ref()
            .filter(|project| {
                self.store
                    .as_ref()
                    .is_some_and(|store| store.is_trusted(project))
            })
            .cloned();
        self.device_runtime.state(project)
    }

    pub(super) fn device_action(
        &mut self,
        request: DeviceRuntimeRequest,
    ) -> Result<DeviceRuntimeState, String> {
        self.current_target()?;
        let _ = self.device_state();
        self.device_runtime.action(request)
    }
}

#[tauri::command]
pub(crate) async fn preview_control_state(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
    scope: RequestScope,
) -> Result<NativeControlState, IpcError> {
    inspection::decode_data(
        inspection::data_request(window, service, scope, DataAction::ControlState).await?,
    )
}
#[tauri::command]
pub(crate) async fn preview_control_enable(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
    scope: RequestScope,
    enabled: bool,
) -> Result<NativeControlState, IpcError> {
    inspection::decode_data(
        inspection::data_request(window, service, scope, DataAction::ControlEnable(enabled))
            .await?,
    )
}
#[tauri::command]
pub(crate) async fn preview_control_submit(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
    scope: RequestScope,
    request: NativeControlRequest,
) -> Result<NativeControlReceipt, IpcError> {
    inspection::decode_data(
        inspection::data_request(window, service, scope, DataAction::ControlSubmit(request))
            .await?,
    )
}
#[tauri::command]
pub(crate) async fn preview_control_cancel(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
    scope: RequestScope,
    identity: NativeControlIdentity,
) -> Result<NativeControlReceipt, IpcError> {
    inspection::decode_data(
        inspection::data_request(window, service, scope, DataAction::ControlCancel(identity))
            .await?,
    )
}
#[tauri::command]
pub(crate) async fn device_runtime_state(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
    scope: RequestScope,
) -> Result<DeviceRuntimeState, IpcError> {
    inspection::decode_data(
        inspection::data_request(window, service, scope, DataAction::DeviceState).await?,
    )
}
#[tauri::command]
pub(crate) async fn device_runtime_action(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
    scope: RequestScope,
    request: DeviceRuntimeRequest,
) -> Result<DeviceRuntimeState, IpcError> {
    inspection::decode_data(
        inspection::data_request(
            window,
            service,
            scope,
            DataAction::DeviceAction(Box::new(request)),
        )
        .await?,
    )
}

#[tauri::command]
pub(crate) async fn read_device_frame(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
    scope: RequestScope,
    identity: DeviceRunIdentity,
    after_sequence: u64,
) -> Result<tauri::ipc::Response, IpcError> {
    match inspection::data_request(
        window,
        service,
        scope,
        DataAction::DeviceFrame(identity, after_sequence),
    )
    .await?
    {
        inspection::DataResult::Bytes(bytes) => Ok(tauri::ipc::Response::new(bytes)),
        _ => Err("DATA_REPLY_TYPE_MISMATCH".into()),
    }
}
