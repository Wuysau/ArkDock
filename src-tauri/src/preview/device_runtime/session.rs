//! Serialized real-device operations on a dedicated worker. No UI thread waits on SDK tools.
use super::*;
use super::{
    deployment::Artifact,
    emulator::EmulatorManager,
    hdc::HdcSession,
    process::{check_cancel, ToolOutput},
};
use std::time::{Duration, Instant};

pub(super) fn run(context: DeviceContext, receiver: mpsc::Receiver<DeviceWork>) {
    let mut worker = Worker {
        context,
        hdc: None,
        emulator: None,
        artifact: None,
        installed: false,
        started: false,
        started_bundle: None,
        operation: 0,
        identity: None,
        active_cancel: Arc::new(AtomicBool::new(false)),
        sequence: 0,
        last_capture: Instant::now(),
        emulator_targets: std::collections::HashMap::new(),
    };
    while !worker.context.shutdown.load(Ordering::Acquire) {
        match receiver.recv_timeout(Duration::from_millis(100)) {
            Ok(work) => {
                let current = worker.snapshot();
                if work.cancel.load(Ordering::Acquire)
                    || current.operation_id != work.operation_id
                    || current.identity.as_ref() != Some(&work.request.identity)
                {
                    continue;
                }
                // Source invalidation can advance generation without replacing
                // this worker. Never reuse a prior generation's install proof.
                if worker
                    .identity
                    .as_ref()
                    .is_some_and(|old| old != &work.request.identity)
                {
                    worker.artifact = None;
                    worker.installed = false;
                    if worker
                        .identity
                        .as_ref()
                        .and_then(|old| old.target_id.as_ref())
                        != work.request.identity.target_id.as_ref()
                    {
                        worker.started = false;
                        worker.started_bundle = None;
                    }
                }
                worker.operation = work.operation_id;
                worker.identity = Some(work.request.identity.clone());
                worker.active_cancel = work.cancel.clone();
                let result = worker.execute(&work.request, &work.cancel);
                if result.is_err() {
                    worker.clear_frame();
                }
                worker.update(|state| {
                    state.busy = false;
                    if let Err(error) = result {
                        state.phase = "failed".into();
                        state.reason = error;
                        state.frame_ready = false;
                        state.foreground_verified = false;
                        state.streaming = false;
                    }
                });
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let snapshot = worker.snapshot();
                if snapshot.streaming
                    && !snapshot.busy
                    && worker.last_capture.elapsed() >= Duration::from_millis(350)
                {
                    // The adapter cancels the previous operation's token when
                    // enqueuing any new action, including stop/capture revoke.
                    let cancel = worker.active_cancel.clone();
                    if let Err(error) = worker.capture(&cancel) {
                        worker.clear_frame();
                        worker.update(|state| {
                            state.streaming = false;
                            state.frame_ready = false;
                            state.foreground_verified = false;
                            state.reason = error;
                        });
                    }
                }
            }
        }
    }
    // Dropping session managers terminates only held client/server/emulator Jobs.
}

struct Worker {
    context: DeviceContext,
    hdc: Option<HdcSession>,
    emulator: Option<EmulatorManager>,
    artifact: Option<Artifact>,
    installed: bool,
    started: bool,
    started_bundle: Option<String>,
    operation: u64,
    identity: Option<DeviceRunIdentity>,
    active_cancel: Arc<AtomicBool>,
    sequence: u64,
    last_capture: Instant,
    emulator_targets: std::collections::HashMap<String, String>,
}
impl Worker {
    fn snapshot(&self) -> DeviceRuntimeState {
        self.context
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
    fn update(&mut self, change: impl FnOnce(&mut DeviceRuntimeState)) {
        let mut state = self.context.state.lock().unwrap_or_else(|e| e.into_inner());
        if operation_current(
            &state,
            self.operation,
            self.identity.as_ref(),
            &self.context.shutdown,
            &self.active_cancel,
        ) {
            change(&mut state);
            // Select/stop-emulator deliberately transition the identity. Only
            // a guarded transition may advance the worker's own expected scope.
            self.identity = state.identity.clone();
        }
    }
    fn ensure_current(&self, cancel: &AtomicBool) -> Result<(), String> {
        check_cancel(cancel)?;
        let state = self.context.state.lock().unwrap_or_else(|e| e.into_inner());
        if !operation_current(
            &state,
            self.operation,
            self.identity.as_ref(),
            &self.context.shutdown,
            &self.active_cancel,
        ) {
            return Err("STALE_DEVICE_RUNTIME_OPERATION".into());
        }
        Ok(())
    }
    fn clear_frame(&mut self) {
        let mut state = self.context.state.lock().unwrap_or_else(|e| e.into_inner());
        if !operation_current(
            &state,
            self.operation,
            self.identity.as_ref(),
            &self.context.shutdown,
            &self.active_cancel,
        ) {
            return;
        }
        if let Ok(mut frame) = self.context.frame.lock() {
            *frame = None;
        }
        state.frame = None;
        state.frame_ready = false;
        state.foreground_verified = false;
        state.streaming = false;
    }
    fn connected(&mut self, cancel: &AtomicBool) -> Result<&mut HdcSession, String> {
        self.ensure_current(cancel)?;
        if self.hdc.is_none() {
            let session = HdcSession::connect(&self.context.hdc, cancel)?;
            self.ensure_current(cancel)?;
            self.hdc = Some(session);
        }
        Ok(self.hdc.as_mut().unwrap())
    }
    fn command(
        &mut self,
        operation: HdcOperation,
        cancel: &AtomicBool,
    ) -> Result<ToolOutput, String> {
        let plan = command_plan(operation)?;
        let result = self.connected(cancel)?.execute(&plan, cancel)?;
        self.ensure_current(cancel)?;
        Ok(result)
    }
    fn selected(&self, request: Option<&DeviceRuntimeRequest>) -> Result<String, String> {
        let state = self.snapshot();
        let target = state
            .selected_target_id
            .ok_or("DEVICE_TARGET_SELECTION_REQUIRED")?;
        if request.is_some_and(|r| r.target_id.as_ref() != Some(&target) || r.kind != state.kind) {
            return Err("STALE_DEVICE_TARGET_SELECTION".into());
        }
        Ok(target)
    }
    fn execute(
        &mut self,
        request: &DeviceRuntimeRequest,
        cancel: &AtomicBool,
    ) -> Result<(), String> {
        self.ensure_current(cancel)?;
        if self.snapshot().identity.as_ref() != Some(&request.identity) {
            return Err("STALE_DEVICE_RUNTIME_IDENTITY".into());
        }
        match request.action {
            DeviceRuntimeAction::Discover => {
                self.update(|s| {
                    s.phase = "discovering".into();
                    s.reason = "正在查询已授权的 HDC 目标…".into();
                });
                let mut targets = self.connected(cancel)?.discover(cancel)?;
                for target in &mut targets {
                    if self.emulator_targets.values().any(|key| key == &target.id) {
                        target.kind = DeviceRuntimeKind::Emulator;
                        target.reason = "本工具启动的模拟器端点已通过 HDC 观察确认。".into();
                    }
                }
                self.update(|s| {
                    s.targets = targets;
                    s.phase = "idle".into();
                    s.reason = if s.targets.is_empty() {
                        "未发现已授权目标，请连接真机或启动已有模拟器。".into()
                    } else {
                        "请选择准确的运行目标。".into()
                    };
                });
            }
            DeviceRuntimeAction::Select => {
                let id = request
                    .target_id
                    .as_deref()
                    .ok_or("DEVICE_TARGET_REQUIRED")?;
                let targets = self.connected(cancel)?.discover(cancel)?;
                if !targets
                    .iter()
                    .any(|t| t.id == id && t.connection_state == "Connected")
                {
                    return Err("DEVICE_TARGET_NOT_CONNECTED".into());
                }
                if request.kind == DeviceRuntimeKind::Unknown {
                    return Err("DEVICE_KIND_REQUIRED".into());
                }
                if request.kind == DeviceRuntimeKind::Emulator
                    && !self.emulator_targets.values().any(|key| key == id)
                {
                    // User classification remains explicit for an externally started emulator.
                    // No TCP/USB heuristic promotes an observed target into a device kind.
                }
                self.artifact = None;
                self.installed = false;
                self.started = false;
                self.started_bundle = None;
                self.clear_frame();
                self.update(|s| {
                    if let Some(identity) = &mut s.identity {
                        identity.target_id = Some(id.into());
                        identity.generation = identity.generation.saturating_add(1);
                    }
                    s.targets = targets;
                    s.selected_target_id = Some(id.into());
                    s.kind = request.kind.clone();
                    s.phase = "selected".into();
                    s.deployment_ready = false;
                    s.deployment_token = None;
                    s.frame = None;
                    s.frame_ready = false;
                    s.foreground_verified = false;
                    s.streaming = false;
                    s.reason = "目标已选择；构建并验证当前工程的 HAP。".into();
                });
            }
            DeviceRuntimeAction::Build => {
                self.selected(Some(request))?;
                self.artifact = None;
                self.installed = false;
                self.clear_frame();
                self.update(|s| {
                    s.phase = "building".into();
                    s.deployment_ready = false;
                    s.deployment_token = None;
                    s.signature_status = "verifying".into();
                    s.reason = "正在打包 HAP 并使用 SDK 验证签名…".into();
                });
                let directory = self.run_directory()?.join(fresh_id());
                let artifact = deployment::build(
                    &self.context.project,
                    &self.context.sdk,
                    &directory,
                    cancel,
                )?;
                check_cancel(cancel)?;
                if self.snapshot().identity.as_ref() != Some(&request.identity) {
                    return Err("STALE_DEVICE_RUNTIME_IDENTITY".into());
                }
                self.update(|s| {
                    s.phase = "artifactReady".into();
                    s.deployment_ready = true;
                    s.signature_status = "verifiedBySdk".into();
                    s.bundle_name = Some(artifact.bundle.clone());
                    s.module_name = Some(artifact.module.clone());
                    s.ability_name = Some(artifact.ability.clone());
                    s.artifact_sha256 = Some(artifact.sha256.clone());
                    s.deployment_token = Some(artifact.confirmation.clone());
                    s.reason = "HAP 已构建并验签；请核对目标和应用后确认安装。".into();
                });
                self.artifact = Some(artifact);
            }
            DeviceRuntimeAction::Deploy => {
                let target = self.selected(Some(request))?;
                if !request.deployment_consent {
                    return Err("DEVICE_DEPLOYMENT_CONSENT_REQUIRED".into());
                }
                let artifact = self.artifact.as_ref().ok_or("DEVICE_ARTIFACT_REQUIRED")?;
                if request.deployment_token.as_deref() != Some(&artifact.confirmation)
                    || self.snapshot().deployment_token.as_deref() != Some(&artifact.confirmation)
                    || super::super::now_ms().saturating_sub(artifact.prepared_at) > 300_000
                {
                    return Err("DEVICE_DEPLOYMENT_CONFIRMATION_EXPIRED".into());
                }
                deployment::revalidate(&self.context.project, artifact)?;
                let path = artifact.path.clone();
                let bundle = artifact.bundle.clone();
                let allowed = artifact.permitted_devices.clone();
                let id_type = artifact.device_id_type.clone();
                if !allowed.is_empty() {
                    let id = self.command(
                        if id_type.as_deref() == Some("sn") {
                            HdcOperation::SerialNumber {
                                target: target.clone(),
                            }
                        } else {
                            HdcOperation::UdId {
                                target: target.clone(),
                            }
                        },
                        cancel,
                    )?;
                    let value = parse_device_id(id.stdout_text()?, id_type.as_deref())?;
                    if !allowed.iter().any(|allowed| allowed == &value) {
                        return Err("DEVICE_SIGNING_TARGET_NOT_ALLOWED".into());
                    }
                }
                self.update(|s| {
                    s.deployment_token = None;
                    s.deployment_ready = false;
                    s.phase = "deploying".into();
                    s.reason = "正在将已验证 HAP 安装到所选目标…".into();
                });
                let result = self.command(
                    HdcOperation::Install {
                        target: target.clone(),
                        hap: path,
                    },
                    cancel,
                )?;
                require_install_receipt(&result)?;
                let info = self.command(
                    HdcOperation::BundleInfo {
                        target,
                        bundle: bundle.clone(),
                    },
                    cancel,
                )?;
                if !bundle_observed(info.stdout_text()?, &bundle) {
                    return Err("DEVICE_INSTALL_NOT_OBSERVED".into());
                }
                self.installed = true;
                self.update(|s| {
                    s.phase = "installed".into();
                    s.reason = "已确认设备中存在安装应用，可以启动。".into();
                });
            }
            DeviceRuntimeAction::Start => {
                let target = self.selected(Some(request))?;
                if !self.installed {
                    return Err("DEVICE_INSTALL_REQUIRED".into());
                }
                let a = self.artifact.as_ref().ok_or("DEVICE_ARTIFACT_REQUIRED")?;
                let op = HdcOperation::Start {
                    target,
                    bundle: a.bundle.clone(),
                    module: a.module.clone(),
                    ability: a.ability.clone(),
                };
                let bundle = a.bundle.clone();
                let result = self.command(op, cancel)?;
                require_exact_receipt(&result, "start ability successfully.")?;
                self.started = true;
                self.started_bundle = Some(bundle);
                self.update(|s| {
                    s.phase = "running".into();
                    s.reason = "启动命令已接受；允许画面读取后可查看真实设备屏幕。".into();
                });
                if request.capture_consent {
                    self.capture(cancel)?;
                    self.update(|s| s.streaming = true);
                }
            }
            DeviceRuntimeAction::Stop => {
                let target = self.selected(Some(request))?;
                self.clear_frame();
                if !self.started {
                    return Err("DEVICE_APP_NOT_STARTED_BY_ARKDOCK".into());
                }
                let bundle = self
                    .started_bundle
                    .as_ref()
                    .ok_or("DEVICE_APP_NOT_STARTED_BY_ARKDOCK")?
                    .clone();
                let result = self.command(HdcOperation::Stop { target, bundle }, cancel)?;
                require_exact_receipt(&result, "force stop process successfully.")?;
                self.started = false;
                self.started_bundle = None;
                self.update(|s| {
                    s.phase = "stopped".into();
                    s.reason = "已停止本会话启动的设备应用。".into();
                });
            }
            DeviceRuntimeAction::Capture | DeviceRuntimeAction::StreamStart => {
                self.selected(Some(request))?;
                if !request.capture_consent {
                    return Err("DEVICE_CAPTURE_CONSENT_REQUIRED".into());
                }
                self.capture(cancel)?;
                if request.action == DeviceRuntimeAction::StreamStart {
                    self.update(|s| s.streaming = true);
                }
            }
            DeviceRuntimeAction::StreamStop => {
                self.clear_frame();
                self.update(|s| {
                    s.reason = "已暂停设备画面刷新。".into();
                });
            }
            DeviceRuntimeAction::Input => self.input(request, cancel)?,
            DeviceRuntimeAction::EmulatorList => {
                if self.emulator.is_none() {
                    self.emulator = Some(EmulatorManager::from_sdk(&self.context.sdk)?);
                }
                let instances = self.emulator.as_mut().unwrap().list(cancel)?;
                self.update(|s| {
                    s.emulators = instances
                        .into_iter()
                        .map(|i| DeviceEmulator {
                            name: i.name,
                            running: i.running,
                            owned: i.owned,
                            hdc_target: i.hdc_target,
                            reason: i.reason,
                        })
                        .collect();
                    s.reason = "请选择已安装镜像的模拟器实例；首次许可需在官方工具完成。".into();
                });
            }
            DeviceRuntimeAction::EmulatorStart => {
                let name = request
                    .emulator_name
                    .as_deref()
                    .ok_or("EMULATOR_NAME_REQUIRED")?;
                if self.emulator.is_none() {
                    self.emulator = Some(EmulatorManager::from_sdk(&self.context.sdk)?);
                }
                self.update(|s| {
                    s.phase = "startingEmulator".into();
                    s.reason = "正在启动选中的官方模拟器实例…".into();
                });
                // Establish HDC ownership before the emulator can invoke its
                // own client; otherwise its launcher could own the shared server.
                self.connected(cancel)?;
                let port = available_emulator_port()?;
                let instance = self.emulator.as_mut().unwrap().start(name, port, cancel)?;
                let target = instance
                    .hdc_target
                    .clone()
                    .ok_or("EMULATOR_HDC_ENDPOINT_MISSING")?;
                self.update(|s| {
                    let value = DeviceEmulator {
                        name: instance.name,
                        running: instance.running,
                        owned: instance.owned,
                        hdc_target: instance.hdc_target,
                        reason: instance.reason,
                    };
                    if let Some(existing) = s.emulators.iter_mut().find(|item| item.name == name) {
                        *existing = value;
                    } else {
                        s.emulators.push(value);
                    }
                });
                self.connected(cancel)?.connect_target(&target, cancel)?;
                let mut targets = self.connected(cancel)?.discover(cancel)?;
                if !targets
                    .iter()
                    .any(|t| t.id == target && t.connection_state == "Connected")
                {
                    return Err("EMULATOR_TARGET_NOT_CONNECTED".into());
                }
                for item in &mut targets {
                    if item.id == target {
                        item.kind = DeviceRuntimeKind::Emulator;
                        item.reason = "本工具启动的模拟器端点已通过 HDC 观察确认。".into();
                    }
                }
                self.emulator_targets.insert(name.into(), target);
                self.update(|s| {
                    s.selected_emulator = Some(name.into());
                    s.targets = targets;
                    s.phase = "idle".into();
                    s.reason = "模拟器已连接，请选择对应 HDC 目标。".into();
                });
            }
            DeviceRuntimeAction::EmulatorStop => {
                let name = request
                    .emulator_name
                    .as_deref()
                    .ok_or("EMULATOR_NAME_REQUIRED")?;
                self.emulator
                    .as_mut()
                    .ok_or("EMULATOR_NOT_OWNED")?
                    .stop_owned(name, cancel)?;
                let target = self.emulator_targets.remove(name);
                if target.as_ref() == self.snapshot().selected_target_id.as_ref() {
                    self.clear_frame();
                    self.artifact = None;
                    self.installed = false;
                    self.started = false;
                    self.started_bundle = None;
                    self.update(|s| {
                        s.streaming = false;
                        s.frame_ready = false;
                        s.foreground_verified = false;
                        s.selected_target_id = None;
                        if let Some(id) = &mut s.identity {
                            id.target_id = None;
                            id.generation = id.generation.saturating_add(1);
                        }
                    });
                }
                self.update(|s| {
                    s.selected_emulator = None;
                    if let Some(instance) = s
                        .emulators
                        .iter_mut()
                        .find(|instance| instance.name == name)
                    {
                        instance.running = false;
                        instance.owned = false;
                        instance.hdc_target = None;
                        instance.reason = "本工具启动的实例已停止。".into();
                    }
                    if let Some(target) = &target {
                        s.targets.retain(|item| &item.id != target);
                    }
                    s.reason = "已关闭本工具启动的模拟器实例。".into();
                });
            }
            DeviceRuntimeAction::Cancel | DeviceRuntimeAction::Disconnect => {
                return Err("DEVICE_REQUEST_CANCELLED".into())
            }
        }
        Ok(())
    }

    fn run_directory(&self) -> Result<PathBuf, String> {
        let run = self
            .snapshot()
            .identity
            .ok_or("DEVICE_IDENTITY_REQUIRED")?
            .run_id;
        let path = std::env::temp_dir().join("ArkDock-device").join(run);
        fs::create_dir_all(&path).map_err(|_| "DEVICE_TEMP_UNAVAILABLE")?;
        Ok(path)
    }
    fn foreground(
        &mut self,
        target: &str,
        bundle: &str,
        cancel: &AtomicBool,
    ) -> Result<bool, String> {
        let pids = self.command(
            HdcOperation::ProcessInfo {
                target: target.into(),
                bundle: bundle.into(),
            },
            cancel,
        )?;
        let windows = self.command(
            HdcOperation::WindowInfo {
                target: target.into(),
            },
            cancel,
        )?;
        Ok(foreground_observed(
            pids.stdout_text()?,
            windows.stdout_text()?,
        ))
    }
    fn capture(&mut self, cancel: &AtomicBool) -> Result<(), String> {
        self.ensure_current(cancel)?;
        if !self.started || !self.installed {
            return Err("DEVICE_APP_NOT_STARTED".into());
        }
        let before = self.snapshot();
        let identity = before.identity.clone().ok_or("DEVICE_IDENTITY_REQUIRED")?;
        let target = self.selected(None)?;
        let bundle = self
            .artifact
            .as_ref()
            .ok_or("DEVICE_ARTIFACT_REQUIRED")?
            .bundle
            .clone();
        if self.started_bundle.as_ref() != Some(&bundle) {
            return Err("DEVICE_APP_NOT_STARTED".into());
        }
        let nonce = fresh_id();
        let path = self.run_directory()?.join(format!("{nonce}.png"));
        let sequence = self
            .sequence
            .checked_add(1)
            .ok_or("DEVICE_FRAME_SEQUENCE_LIMIT")?;
        // Keep cleanup outside the fallible capture pipeline so every failure,
        // malformed receipt and cancellation still visits both owned paths.
        let result = (|| -> Result<frame::DeviceFrame, String> {
            let foreground_before = self.foreground(&target, &bundle, cancel).unwrap_or(false);
            self.ensure_current(cancel)?;
            let captured = super::super::now_ms();
            let shot = self.command(
                HdcOperation::Screenshot {
                    target: target.clone(),
                    nonce: nonce.clone(),
                },
                cancel,
            )?;
            require_capture_receipt(&shot, &nonce)?;
            let received = self.command(
                HdcOperation::ReceiveScreenshot {
                    target: target.clone(),
                    nonce: nonce.clone(),
                    destination: path.clone(),
                },
                cancel,
            )?;
            require_transfer_receipt(&received)?;
            let foreground_after = self.foreground(&target, &bundle, cancel).unwrap_or(false);
            self.ensure_current(cancel)?;
            frame::DeviceFrame::from_file(
                &path,
                DeviceFrameMetadata {
                    run_id: identity.run_id.clone(),
                    target_id: target.clone(),
                    bundle_name: bundle,
                    sequence,
                    width: 0,
                    height: 0,
                    capture_started_at: captured,
                    received_at: super::super::now_ms(),
                    payload_length: 0,
                    foreground_verified: foreground_before && foreground_after,
                },
            )
        })();
        let cleanup = self.cleanup_capture(&target, &nonce, &path);
        let frame = match (result, cleanup) {
            (Err(error), Err(cleanup)) => return Err(format!("{error}; {cleanup}")),
            (Err(error), _) | (_, Err(error)) => return Err(error),
            (Ok(frame), Ok(())) => frame,
        };
        self.ensure_current(cancel)?;
        self.publish_frame(frame, &identity, cancel)?;
        self.sequence = sequence;
        self.last_capture = Instant::now();
        Ok(())
    }
    fn cleanup_capture(&mut self, target: &str, nonce: &str, path: &Path) -> Result<(), String> {
        let local = match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!("DEVICE_CAPTURE_LOCAL_CLEANUP: {error}")),
        };
        // Reuse the held server only; never reconnect/start a server during
        // cancellation. A short independent token permits deleting this exact
        // operation's nonce even though the original request has been revoked.
        let remote = if let Some(hdc) = self.hdc.as_mut() {
            let mut plan = command_plan(HdcOperation::RemoveScreenshot {
                target: target.into(),
                nonce: nonce.into(),
            })?;
            plan.timeout_ms = 1500;
            plan.stdout_limit = 8192;
            hdc.execute(&plan, &AtomicBool::new(false))
                .and_then(|output| {
                    if output.stdout_text()?.trim().is_empty()
                        && output.stderr.iter().all(u8::is_ascii_whitespace)
                    {
                        Ok(())
                    } else {
                        Err(format!(
                            "DEVICE_CAPTURE_REMOTE_CLEANUP: {}",
                            output.diagnostic()
                        ))
                    }
                })
        } else {
            Ok(())
        };
        match (local, remote) {
            (Err(local), Err(remote)) => Err(format!("{local}; {remote}")),
            (Err(error), _) | (_, Err(error)) => Err(error),
            _ => Ok(()),
        }
    }
    fn publish_frame(
        &mut self,
        frame: frame::DeviceFrame,
        identity: &DeviceRunIdentity,
        cancel: &AtomicBool,
    ) -> Result<(), String> {
        // All writers use state -> frame. Keep both locks until the receipt and
        // PNG agree, closing the old check-then-publish generation race.
        let mut state = self
            .context
            .state
            .lock()
            .map_err(|_| "DEVICE_STATE_UNAVAILABLE")?;
        let mut current = self
            .context
            .frame
            .lock()
            .map_err(|_| "DEVICE_FRAME_UNAVAILABLE")?;
        check_cancel(cancel)?;
        if !operation_current(
            &state,
            self.operation,
            Some(identity),
            &self.context.shutdown,
            &self.active_cancel,
        ) || state.selected_target_id.as_ref() != Some(&frame.metadata.target_id)
            || identity.run_id != frame.metadata.run_id
        {
            return Err("STALE_DEVICE_RUNTIME_OPERATION".into());
        }
        let foreground = frame.metadata.foreground_verified;
        state.frame = Some(frame.metadata.clone());
        state.frame_ready = true;
        state.foreground_verified = foreground;
        state.reason = if foreground {
            "已收到真实设备画面；图像按实际截取速度更新。".into()
        } else {
            "已收到设备全屏画面；目标应用前台身份未确认，输入暂停。".into()
        };
        *current = Some(frame);
        Ok(())
    }
    fn input(&mut self, request: &DeviceRuntimeRequest, cancel: &AtomicBool) -> Result<(), String> {
        let target = self.selected(Some(request))?;
        let input = request.input.as_ref().ok_or("DEVICE_INPUT_REQUIRED")?;
        if !request.input_consent || !request.capture_consent {
            return Err("DEVICE_INPUT_CONSENT_REQUIRED".into());
        }
        self.ensure_current(cancel)?;
        let snapshot = self.snapshot();
        let frame = snapshot
            .frame
            .as_ref()
            .ok_or("DEVICE_FRAME_REQUIRED")?
            .clone();
        if !snapshot.frame_ready
            || !snapshot.foreground_verified
            || !self.installed
            || !self.started
            || self.started_bundle.as_ref() != Some(&frame.bundle_name)
            || !frame.foreground_verified
            || frame.sequence != input.frame_sequence
            || !fresh_frame(&snapshot, &frame, super::super::now_ms())
        {
            return Err("DEVICE_INPUT_STALE_FRAME".into());
        }
        if !self.foreground(&target, &frame.bundle_name, cancel)? {
            return Err("DEVICE_INPUT_FOREGROUND_CHANGED".into());
        }
        // Window/PID probes themselves take time. Recheck freshness immediately
        // before constructing and dispatching the input operation.
        self.ensure_current(cancel)?;
        if !fresh_frame(&self.snapshot(), &frame, super::super::now_ms()) {
            return Err("DEVICE_INPUT_STALE_FRAME".into());
        }
        let point = || {
            Ok::<_, String>((
                input.x.ok_or("DEVICE_POINT_REQUIRED")?,
                input.y.ok_or("DEVICE_POINT_REQUIRED")?,
            ))
        };
        let operation = match input.kind {
            DeviceInputKind::Click => {
                let (x, y) = point()?;
                HdcOperation::Click {
                    target,
                    x,
                    y,
                    width: frame.width,
                    height: frame.height,
                }
            }
            DeviceInputKind::DoubleClick => {
                let (x, y) = point()?;
                HdcOperation::DoubleClick {
                    target,
                    x,
                    y,
                    width: frame.width,
                    height: frame.height,
                }
            }
            DeviceInputKind::LongClick => {
                let (x, y) = point()?;
                HdcOperation::LongClick {
                    target,
                    x,
                    y,
                    width: frame.width,
                    height: frame.height,
                }
            }
            DeviceInputKind::Swipe | DeviceInputKind::Drag => {
                let from = point()?;
                let to = (
                    input.end_x.ok_or("DEVICE_POINT_REQUIRED")?,
                    input.end_y.ok_or("DEVICE_POINT_REQUIRED")?,
                );
                let velocity = input.velocity.unwrap_or(800);
                if input.kind == DeviceInputKind::Swipe {
                    HdcOperation::Swipe {
                        target,
                        from,
                        to,
                        velocity,
                        width: frame.width,
                        height: frame.height,
                    }
                } else {
                    HdcOperation::Drag {
                        target,
                        from,
                        to,
                        velocity,
                        width: frame.width,
                        height: frame.height,
                    }
                }
            }
            DeviceInputKind::Text => HdcOperation::Text {
                target,
                text: input.text.clone().ok_or("DEVICE_TEXT_REQUIRED")?,
            },
            DeviceInputKind::Key => HdcOperation::KeyEvent {
                target,
                code: input.key_code.ok_or("DEVICE_KEY_REQUIRED")?,
            },
        };
        let result = self.command(operation, cancel)?;
        // UItest may return zero after printing an error; require a clean tool response.
        if result.status != 0
            || !result.stdout_text()?.trim().is_empty()
            || !result.stderr.iter().all(u8::is_ascii_whitespace)
        {
            return Err("DEVICE_INPUT_REJECTED".into());
        }
        self.update(|s| {
            s.foreground_verified = false;
            s.reason = "输入已交给设备工具；刷新画面以观察实际结果。".into();
        });
        if request.capture_consent {
            self.capture(cancel)?;
        }
        Ok(())
    }
}

fn operation_current(
    state: &DeviceRuntimeState,
    operation: u64,
    identity: Option<&DeviceRunIdentity>,
    shutdown: &AtomicBool,
    cancel: &AtomicBool,
) -> bool {
    !shutdown.load(Ordering::Acquire)
        && !cancel.load(Ordering::Acquire)
        && state.operation_id == operation
        && identity.is_some()
        && state.identity.as_ref() == identity
}

fn fresh_frame(state: &DeviceRuntimeState, frame: &DeviceFrameMetadata, now: u64) -> bool {
    state.identity.as_ref().is_some_and(|id| {
        id.run_id == frame.run_id && id.target_id.as_ref() == Some(&frame.target_id)
    }) && state.selected_target_id.as_ref() == Some(&frame.target_id)
        && state.bundle_name.as_ref() == Some(&frame.bundle_name)
        && state
            .frame
            .as_ref()
            .is_some_and(|current| current.sequence == frame.sequence)
        && now >= frame.received_at
        && frame.received_at >= frame.capture_started_at
        && now.saturating_sub(frame.capture_started_at) <= 5000
}

fn receipt_lines(output: &ToolOutput) -> Result<Vec<&str>, String> {
    if output.status != 0 || !output.stderr.iter().all(u8::is_ascii_whitespace) {
        return Err(format!(
            "DEVICE_RECEIPT_NOT_CONFIRMED: {}",
            output.diagnostic()
        ));
    }
    let lines: Vec<_> = output
        .stdout_text()?
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    if lines.iter().any(|line| {
        let text = line.to_ascii_lowercase();
        text.starts_with("error:")
            || text.starts_with("[fail]")
            || text.starts_with("[error]")
            || text.contains("failed")
            || text.contains("exception")
            || text.contains("not found")
    }) {
        return Err(format!(
            "DEVICE_RECEIPT_NOT_CONFIRMED: {}",
            output.diagnostic()
        ));
    }
    Ok(lines)
}

fn require_exact_receipt(output: &ToolOutput, expected: &str) -> Result<(), String> {
    if receipt_lines(output)? == [expected] {
        Ok(())
    } else {
        Err("DEVICE_RECEIPT_NOT_CONFIRMED".into())
    }
}

fn require_install_receipt(output: &ToolOutput) -> Result<(), String> {
    // Official hdc.md: the app result AND terminal AppMod receipt are required.
    let lines = receipt_lines(output)?;
    if lines.len() != 2
        || !matches!(lines[1], "AppMod finish" | "AppMod finish.")
        || !lines[0]
            .strip_prefix("[Info]App install path:")
            .is_some_and(|result| {
                result.rsplit_once(" msg:").is_some_and(|(path, status)| {
                    !path.is_empty() && status == "install bundle successfully."
                })
            })
    {
        return Err("DEVICE_INSTALL_RECEIPT_NOT_CONFIRMED".into());
    }
    Ok(())
}

fn require_capture_receipt(output: &ToolOutput, nonce: &str) -> Result<(), String> {
    require_exact_receipt(
        output,
        &format!("ScreenCap saved to /data/local/tmp/arkdock_{nonce}.png"),
    )
}

fn require_transfer_receipt(output: &ToolOutput) -> Result<(), String> {
    let lines = receipt_lines(output)?;
    if lines.len() != 1 {
        return Err("DEVICE_TRANSFER_RECEIPT_NOT_CONFIRMED".into());
    }
    let details = lines[0]
        .strip_prefix("FileTransfer finish, Size:")
        .ok_or("DEVICE_TRANSFER_RECEIPT_NOT_CONFIRMED")?;
    let (size, details) = details
        .split_once(',')
        .ok_or("DEVICE_TRANSFER_RECEIPT_NOT_CONFIRMED")?;
    if !size.trim().bytes().all(|b| b.is_ascii_digit())
        || size.trim().parse::<u64>().ok().filter(|n| *n > 0).is_none()
        || !details
            .trim_start()
            .strip_prefix("File count = 1,")
            .is_some_and(|timing| timing.trim_start().starts_with("time:"))
    {
        return Err("DEVICE_TRANSFER_RECEIPT_NOT_CONFIRMED".into());
    }
    Ok(())
}

fn parse_device_id(text: &str, id_type: Option<&str>) -> Result<String, String> {
    let lines: Vec<_> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    let value = match id_type {
        Some("sn") if lines.len() == 1 => lines[0],
        None | Some("udid") if lines.len() == 2 && lines[0] == "udid of current device is :" => {
            lines[1]
        }
        _ => return Err("DEVICE_ID_RECEIPT_NOT_CONFIRMED".into()),
    };
    let valid = !value.is_empty()
        && value.len() <= 128
        && match id_type {
            Some("sn") => {
                value
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
                    && !value.eq_ignore_ascii_case("unknown")
            }
            _ => value.bytes().all(|b| b.is_ascii_hexdigit()),
        };
    if valid {
        Ok(value.into())
    } else {
        Err("DEVICE_ID_RECEIPT_NOT_CONFIRMED".into())
    }
}

fn bundle_observed(text: &str, bundle: &str) -> bool {
    // BundleMgrHostImpl::DumpBundleInfo prints `${bundle}:\n` then a complete
    // JSON object. A matching name buried in malformed/error output is no proof.
    let text = text.trim();
    let json = text
        .strip_prefix(&format!("{bundle}:"))
        .map(str::trim)
        .unwrap_or(text);
    serde_json::from_str::<serde_json::Value>(json)
        .ok()
        .filter(serde_json::Value::is_object)
        .is_some_and(|value| {
            let name = value
                .get("name")
                .or_else(|| value.get("bundleName"))
                .and_then(serde_json::Value::as_str);
            name == Some(bundle)
                && value
                    .get("bundleName")
                    .is_none_or(|v| v.as_str() == Some(bundle))
        })
}

fn unsigned_decimal(value: &str) -> Option<u32> {
    (!value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()))
        .then(|| value.parse::<u32>().ok())
        .flatten()
}

fn foreground_observed(processes: &str, windows: &str) -> bool {
    let pids: Option<Vec<u32>> = processes
        .split_ascii_whitespace()
        .map(|word| unsigned_decimal(word).filter(|pid| *pid > 0))
        .collect();
    let Some(pids) = pids else { return false };
    if pids.is_empty()
        || pids.len() > 16
        || pids.iter().collect::<std::collections::HashSet<_>>().len() != pids.len()
    {
        return false;
    }
    let focus_lines: Vec<_> = windows
        .lines()
        .map(str::trim)
        .filter_map(|line| line.strip_prefix("Focus window:"))
        .collect();
    if focus_lines.len() != 1 {
        return false;
    }
    let Some(focus) = unsigned_decimal(focus_lines[0].trim()).filter(|id| *id > 0) else {
        return false;
    };
    let mut columns = None;
    let mut matching = Vec::new();
    for line in windows.lines() {
        let words: Vec<_> = line.split_ascii_whitespace().collect();
        if words.first() == Some(&"WindowName") {
            let position = |key| {
                let found: Vec<_> = words
                    .iter()
                    .enumerate()
                    .filter(|(_, word)| **word == key)
                    .map(|(index, _)| index)
                    .collect();
                (found.len() == 1).then(|| found[0])
            };
            columns = position("WinId")
                .zip(position("Pid"))
                .zip(position("DisplayId"));
            continue;
        }
        if let Some(((win, pid), display)) = columns {
            if words.get(win).and_then(|word| unsigned_decimal(word)) == Some(focus) {
                matching.push((
                    words.get(pid).and_then(|word| unsigned_decimal(word)),
                    words.get(display).and_then(|word| unsigned_decimal(word)),
                ));
            }
        }
    }
    matching.len() == 1
        && matching[0].0.is_some_and(|pid| pids.contains(&pid))
        && matching[0].1 == Some(0)
}
fn available_emulator_port() -> Result<u16, String> {
    (10000..=16555)
        .find(|port| std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, *port)).is_ok())
        .ok_or("EMULATOR_PORT_UNAVAILABLE".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn output(value: &str) -> ToolOutput {
        ToolOutput {
            status: 0,
            stdout: value.as_bytes().to_vec(),
            stderr: Vec::new(),
        }
    }
    #[test]
    fn zero_exit_and_failure_text_never_confirm_install() {
        assert!(require_install_receipt(&output("install failed")).is_err());
        assert!(require_install_receipt(&output(
            "[Info]App install path:D:/test.hap msg:install bundle successfully.\nAppMod finish\n"
        ))
        .is_ok());
        assert!(require_install_receipt(&output(
            "[Info]App install path:D:/test.hap msg:install bundle successfully.\n"
        ))
        .is_err());
        assert!(require_install_receipt(&output("install success\n")).is_err());
        assert!(require_exact_receipt(
            &output("usage: aa force-stop\n"),
            "force stop process successfully."
        )
        .is_err());
        assert!(require_exact_receipt(
            &output("start ability successfully.\n"),
            "start ability successfully."
        )
        .is_ok());
        let mut deceptive = output("start ability successfully.\n");
        deceptive.stderr = b"error: failed".to_vec();
        assert!(require_exact_receipt(&deceptive, "start ability successfully.").is_err());
    }
    #[test]
    fn capture_receipt_binds_nonce_and_transfer_requires_single_real_file() {
        let nonce = "0123456789abcdef";
        assert!(require_capture_receipt(
            &output("ScreenCap saved to /data/local/tmp/arkdock_0123456789abcdef.png\n"),
            nonce
        )
        .is_ok());
        assert!(require_capture_receipt(
            &output("ScreenCap saved to /data/local/tmp/arkdock_ffffffffffffffff.png\n"),
            nonce
        )
        .is_err());
        assert!(require_transfer_receipt(&output(
            "FileTransfer finish, Size:1111, File count = 1, time:10ms rate:0.00kB/s\n"
        ))
        .is_ok());
        for text in [
            "FileTransfer finish, Size:0, File count = 1, time:10ms",
            "FileTransfer finish, Size:11, File count = 2, time:10ms",
            "transfer success",
        ] {
            assert!(require_transfer_receipt(&output(text)).is_err());
        }
    }
    #[test]
    fn bundle_information_requires_complete_json_with_exact_optional_wrapper() {
        assert!(bundle_observed(
            "app.test:\n{\"name\":\"app.test\",\"applicationInfo\":{\"name\":\"app.test\"}}\n",
            "app.test"
        ));
        assert!(bundle_observed("{\"bundleName\":\"app.test\"}", "app.test"));
        for text in [
            "other.app:\n{\"name\":\"app.test\"}",
            "{\"name\":\"other.app\",\"bundleName\":\"app.test\"}",
            "error\n\"name\": \"app.test\",",
            "{\"applicationInfo\":{\"name\":\"app.test\"}}",
            "app.test:\n{\"name\":\"app.test\"}\n[Fail] error",
        ] {
            assert!(!bundle_observed(text, "app.test"));
        }
    }
    #[test]
    fn device_profile_ids_use_exact_udid_or_serial_receipts() {
        assert_eq!(
            parse_device_id("udid of current device is :\n23CADE0C\n", Some("udid")).unwrap(),
            "23CADE0C"
        );
        assert_eq!(
            parse_device_id("ABC123456\n", Some("sn")).unwrap(),
            "ABC123456"
        );
        for (value, kind) in [
            ("23CADE0C", "udid"),
            ("udid of current device is :\nABCD\nDEAD", "udid"),
            ("udid of current device is :\nfailed", "udid"),
            ("abc 123", "sn"),
            ("unknown", "sn"),
            ("serial: ABC123", "sn"),
        ] {
            assert!(parse_device_id(value, Some(kind)).is_err());
        }
    }
    #[test]
    fn focus_must_match_observed_pid_window_and_default_display() {
        // Exact ordering from official hidumper.md.
        let log = "----------------------------------WindowManagerService---------------------------------\nWindowName DisplayId Pid WinId Type Mode Flag ZOrd Orientation [ x y w h ]\napp 0 91 23 1 1 1 1 0 [ 0 0 720 1280 ]\nFocus window: 23\ntotal window num: 1";
        assert!(foreground_observed("91", log));
        assert!(!foreground_observed("92", log));
        assert!(!foreground_observed("91", &log.replace("app 0", "app 1")));
        assert!(!foreground_observed("91", "app 91"));
        for pids in ["error 91", "91 error", "91 91", "0 91", "+91"] {
            assert!(!foreground_observed(pids, log));
        }
        assert!(!foreground_observed(
            "91",
            &format!("{log}\nFocus window: 23")
        ));
        assert!(!foreground_observed(
            "91",
            &log.replace("Focus window: 23", "Error focus window: 23")
        ));
    }

    fn fixture() -> Worker {
        let mut state = empty_state(&ToolObservation {
            path: None,
            sha256: None,
            reason: "fixture".into(),
        });
        let identity = DeviceRunIdentity {
            run_id: "run".into(),
            workspace_id: "workspace".into(),
            target_id: Some("device".into()),
            generation: 1,
        };
        state.identity = Some(identity.clone());
        state.selected_target_id = Some("device".into());
        state.operation_id = 1;
        state.bundle_name = Some("app.test".into());
        Worker {
            context: DeviceContext {
                project: ProjectRef {
                    id: "workspace".into(),
                    name: "fixture".into(),
                    environment: Environment::WindowsLocal,
                    workspace_root: "D:/fixture".into(),
                    app_root: "D:/fixture".into(),
                    module: "entry".into(),
                    variant: "default".into(),
                    sdk_id: None,
                },
                sdk: PathBuf::from("D:/fixture-sdk"),
                hdc: PathBuf::from("D:/fixture-sdk/hdc.exe"),
                state: Arc::new(Mutex::new(state)),
                frame: Arc::new(Mutex::new(None)),
                shutdown: Arc::new(AtomicBool::new(false)),
            },
            hdc: None,
            emulator: None,
            artifact: None,
            installed: false,
            started: false,
            started_bundle: None,
            operation: 1,
            identity: Some(identity),
            active_cancel: Arc::new(AtomicBool::new(false)),
            sequence: 0,
            last_capture: Instant::now(),
            emulator_targets: std::collections::HashMap::new(),
        }
    }
    fn frame_fixture() -> frame::DeviceFrame {
        frame::DeviceFrame {
            metadata: DeviceFrameMetadata {
                run_id: "run".into(),
                target_id: "device".into(),
                bundle_name: "app.test".into(),
                sequence: 1,
                width: 1,
                height: 1,
                capture_started_at: 100,
                received_at: 120,
                payload_length: 3,
                foreground_verified: true,
            },
            png: vec![1, 2, 3],
        }
    }
    #[test]
    fn publication_is_rejected_after_operation_generation_or_cancel_changes() {
        for condition in ["operation", "generation", "run", "cancel", "shutdown"] {
            let mut worker = fixture();
            let identity = worker.identity.clone().unwrap();
            match condition {
                "operation" => worker.context.state.lock().unwrap().operation_id += 1,
                "generation" => {
                    worker
                        .context
                        .state
                        .lock()
                        .unwrap()
                        .identity
                        .as_mut()
                        .unwrap()
                        .generation += 1
                }
                "run" => {
                    worker
                        .context
                        .state
                        .lock()
                        .unwrap()
                        .identity
                        .as_mut()
                        .unwrap()
                        .run_id = "new-run".into()
                }
                "cancel" => worker.active_cancel.store(true, Ordering::Release),
                _ => worker.context.shutdown.store(true, Ordering::Release),
            }
            assert!(
                worker
                    .publish_frame(frame_fixture(), &identity, &AtomicBool::new(false))
                    .is_err(),
                "{condition}"
            );
            worker.update(|state| state.reason = "stale write".into());
            assert!(worker.context.frame.lock().unwrap().is_none());
            assert_eq!(worker.snapshot().reason, "fixture");
        }
    }
    #[test]
    fn frame_and_receipt_publish_together_and_slow_capture_never_enables_input() {
        let mut worker = fixture();
        let identity = worker.identity.clone().unwrap();
        worker
            .publish_frame(frame_fixture(), &identity, &AtomicBool::new(false))
            .unwrap();
        let snapshot = worker.snapshot();
        let frame = worker
            .context
            .frame
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .metadata
            .clone();
        assert_eq!(snapshot.frame.as_ref(), Some(&frame));
        assert!(snapshot.frame_ready);
        assert!(fresh_frame(&snapshot, &frame, 5000));
        let mut slow = frame.clone();
        slow.received_at = 6000;
        assert!(!fresh_frame(&snapshot, &slow, 6001));
        worker.clear_frame();
        assert!(worker.context.frame.lock().unwrap().is_none());
        assert!(!worker.snapshot().frame_ready);
    }
    #[test]
    fn cleanup_visits_owned_local_path_even_with_cancelled_operation() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("owned.png");
        fs::write(&path, b"fixture").unwrap();
        let mut worker = fixture();
        worker.active_cancel.store(true, Ordering::Release);
        worker
            .cleanup_capture("device", "0123456789abcdef", &path)
            .unwrap();
        assert!(!path.exists());
    }
}
