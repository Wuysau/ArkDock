//! A single backend worker owns sessions. Hooks/watchers only enqueue lightweight messages.
#[cfg(not(windows))]
use crate::discovery::resolve;
use crate::discovery::{automatic_candidate, inspect_directory, path_environment};
#[cfg(windows)]
use crate::discovery::{
    registration::RegistrationRegistry, resolve_with_sources, windows_tabs::WindowsTabResolver,
    TitleResolver,
};
use crate::ipc::model::*;
use crate::ipc::validate_scope;
use crate::preview::frames::{BinaryFrame, BINARY_FRAME_PROTOCOL_VERSION};
use crate::preview::{NativePoint, NativePreviewer, PreviewBackend, PreviewInput};
use crate::scheduler::{
    watch_project, BuildCoordinator, BuildOutcome, CompletionDisposition, WatchEvent, WatchGuard,
};
use crate::windowing::{WindowController, WindowUpdate};
use crate::workspace::WorkspaceStore;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{Emitter, Manager};

enum Action {
    BeginDrag,
    Rediscover,
    Select(String),
    Directory(PathBuf),
    Detach,
    Trust(String, bool),
    Sdk(PathBuf),
    Refresh,
    Stop,
    BinaryFrames(u32),
    LegacyFrames(u32),
    Arrange(bool),
    Click {
        build_id: String,
        generation: u64,
        point: NativePoint,
    },
}
enum Message {
    Action(
        RequestScope,
        Action,
        mpsc::Sender<Result<AppSnapshot, String>>,
    ),
    Shutdown,
}
#[derive(Default)]
struct PendingWatch {
    changed: bool,
    error: Option<String>,
}
impl PendingWatch {
    fn push(&mut self, event: WatchEvent) {
        match event {
            WatchEvent::Changed(_) => self.changed = true,
            WatchEvent::Error(error) => self.error = Some(error.chars().take(4096).collect()),
        }
    }
}
struct Service {
    sender: mpsc::SyncSender<Message>,
    state: Arc<Mutex<AppSnapshot>>,
    worker: Mutex<Option<JoinHandle<()>>>,
    frames: Arc<Mutex<FrameDelivery>>,
}

#[derive(Default)]
struct FrameDelivery {
    binary_enabled: bool,
    binary_session: Option<String>,
    latest: Option<BinaryFrame>,
}
impl FrameDelivery {
    fn enable(&mut self, protocol_version: u32, session_id: &str) -> Result<(), String> {
        if protocol_version != BINARY_FRAME_PROTOCOL_VERSION {
            return Err("PROTOCOL_VERSION_UNSUPPORTED".into());
        }
        if session_id.is_empty() || session_id.len() > 256 {
            return Err("PREVIEW_SESSION_REQUIRED".into());
        }
        self.binary_enabled = true;
        self.binary_session = Some(session_id.to_owned());
        Ok(())
    }
    fn push(&mut self, frame: BinaryFrame) {
        self.latest = Some(frame);
    }
    fn clear(&mut self) {
        self.latest = None;
        self.binary_enabled = false;
        self.binary_session = None;
    }
    fn take_current(&mut self, state: &AppSnapshot) -> Result<Vec<u8>, String> {
        if !self.binary_enabled {
            return Err("BINARY_TRANSPORT_NOT_ENABLED".into());
        }
        if self.binary_session.as_deref() != state.session_id.as_deref() {
            self.clear();
            return Err("STALE_SESSION".into());
        }
        let Some(frame) = self.latest.as_ref() else {
            return Ok(Vec::new());
        };
        let metadata = &frame.metadata;
        if !state.trusted
            || state.phase != Phase::Running
            || state.build.stale
            || state.session_id.as_deref() != Some(&metadata.session_id)
            || state.project.as_ref().map(|project| project.id.as_str())
                != Some(metadata.workspace_id.as_str())
            || state.build.build_id.as_deref() != Some(&metadata.build_id)
            || state.build.generation != metadata.generation
            || state.build.displayed_generation != Some(metadata.generation)
        {
            self.latest = None;
            return Err("STALE_FRAME".into());
        }
        self.latest.take().ok_or("FRAME_UNAVAILABLE")?.packet()
    }
}
impl Service {
    fn snapshot(&self) -> Result<AppSnapshot, String> {
        self.state
            .lock()
            .map(|s| s.clone())
            .map_err(|_| "STATE_UNAVAILABLE".into())
    }
    fn shutdown(&self) {
        // Sender remains connected until the worker exits; send is bounded and does not run on a hook.
        let _ = self.sender.send(Message::Shutdown);
        if let Ok(mut worker) = self.worker.lock() {
            if let Some(worker) = worker.take() {
                let _ = worker.join();
            }
        }
    }
}

struct Engine {
    app: tauri::AppHandle,
    output: Arc<Mutex<AppSnapshot>>,
    snapshot: AppSnapshot,
    controller: Option<WindowController>,
    store: Option<WorkspaceStore>,
    backend: NativePreviewer,
    scheduler: BuildCoordinator,
    watcher: Option<WatchGuard>,
    window_pending: Arc<Mutex<Option<WindowUpdate>>>,
    watch_pending: Arc<Mutex<PendingWatch>>,
    serial: u64,
    detaching: bool,
    frames: Arc<Mutex<FrameDelivery>>,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn initial(backend: BackendStatus) -> AppSnapshot {
    AppSnapshot {
        revision: 0,
        phase: if cfg!(windows) {
            Phase::Idle
        } else {
            Phase::Unsupported
        },
        target: None,
        project: None,
        candidates: Vec::new(),
        trusted: false,
        session_id: None,
        session_ref: None,
        backend,
        build: BuildCoordinator::new(300).status(),
        diagnostics: Vec::new(),
        dock_hint: None,
        layout_required: false,
    }
}

fn frame_changes_state(snapshot: &AppSnapshot, build: &BuildStatus) -> bool {
    snapshot.phase != Phase::Running || snapshot.build != *build
}

/// Close execution at the gate itself: tick must never inherit an untrusted live Job.
fn execution_gate(trusted: bool, available: bool, reason: &str, suspend: impl FnOnce()) -> Phase {
    if !trusted {
        suspend();
        Phase::Untrusted
    } else if !available {
        suspend();
        if reason.starts_with("SDK_MISSING") {
            Phase::SdkMissing
        } else {
            Phase::ProtocolBlocked
        }
    } else {
        Phase::Building
    }
}

impl Engine {
    fn publish(&mut self) {
        self.snapshot.revision += 1;
        self.snapshot.build = self.scheduler.status();
        if let Ok(mut state) = self.output.lock() {
            *state = self.snapshot.clone();
        }
        let _ = self.app.emit("arkdock:state", &self.snapshot);
    }
    fn diagnostic(&mut self, code: &str, message: String, status: CheckStatus) {
        if self.snapshot.diagnostics.len() >= 128 {
            self.snapshot.diagnostics.remove(0);
        }
        self.snapshot.diagnostics.push(Diagnostic {
            code: code.into(),
            message,
            status,
            observed_at: now(),
            workspace_id: self.snapshot.project.as_ref().map(|p| p.id.clone()),
            session_id: self.snapshot.session_id.clone(),
            build_id: self.scheduler.status().build_id,
            backend_id: Some(self.snapshot.backend.id.clone()),
        });
    }
    fn suspend_execution(&mut self) {
        if let Ok(mut frames) = self.frames.lock() {
            frames.clear();
        }
        self.watcher.take();
        self.backend.stop();
        self.scheduler.stop();
        self.watch_pending = Arc::new(Mutex::new(PendingWatch::default()));
        self.snapshot.session_id = None;
        self.snapshot.session_ref = None;
        self.snapshot.trusted = false;
    }
    fn close_session(&mut self) {
        self.suspend_execution();
        self.snapshot.project = None;
    }
    fn apply_window(&mut self, update: WindowUpdate) {
        if self.detaching {
            if update.target.is_some() {
                return;
            }
            self.detaching = false;
        }
        let changed = self.snapshot.target.as_ref().map(|t| (&t.id, &t.process))
            != update.target.as_ref().map(|t| (&t.id, &t.process));
        self.snapshot.target = update.target;
        self.snapshot.dock_hint = update.dock_hint;
        self.snapshot.layout_required = update.layout_required;
        if changed {
            self.close_session();
            self.snapshot.candidates.clear();
            if self.snapshot.target.is_some() {
                if let Err(error) = self.discover() {
                    self.snapshot.phase = Phase::SelectProject;
                    self.diagnostic("DISCOVERY_FAILED", error, CheckStatus::Fail);
                }
            } else {
                self.snapshot.phase = Phase::Idle;
            }
        }
        self.publish();
    }
    fn current_target(&self) -> Result<TargetInfo, String> {
        let target = self.snapshot.target.clone().ok_or("TARGET_REQUIRED")?;
        if self
            .controller
            .as_ref()
            .and_then(WindowController::current_target)
            .as_ref()
            .is_none_or(|current| current.id != target.id || current.process != target.process)
        {
            return Err("STALE_TARGET: reattach to the current window".into());
        }
        Ok(target)
    }
    fn discover(&mut self) -> Result<(), String> {
        let target = self.current_target()?;
        self.snapshot.phase = Phase::Discovering;
        self.publish();
        let observed = now();
        #[cfg(windows)]
        {
            let controller = self.controller.as_ref().ok_or("WINDOWING_UNAVAILABLE")?;
            let scope = controller.discovery_scope(&target.id)?;
            let tabs = WindowsTabResolver::collect(scope.handle, &target, observed);
            controller.discovery_scope(&target.id)?;
            self.snapshot.candidates = resolve_with_sources(
                &target,
                observed,
                &[&tabs, &RegistrationRegistry::default(), &TitleResolver],
            )?;
        }
        #[cfg(not(windows))]
        {
            self.snapshot.candidates = resolve(&target, observed)?;
        }
        // History is a hint. It never upgrades to current-session reliable evidence.
        if let Some(store) = &self.store {
            for project in store.history() {
                let evidence = Evidence {
                    source: EvidenceSource::History,
                    target_id: Some(target.id.clone()),
                    process: Some(target.process.clone()),
                    session_id: None,
                    observed_at: observed,
                    expires_at: observed + 30_000,
                    environment: project.environment.clone(),
                    raw_path: project.app_root.clone(),
                    verifiable: false,
                    active_scope: false,
                };
                if let Ok(items) = inspect_directory(
                    Path::new(&project.app_root),
                    project.environment.clone(),
                    evidence,
                ) {
                    for item in items {
                        if !self.snapshot.candidates.iter().any(|c| c.id == item.id) {
                            self.snapshot.candidates.push(item);
                        }
                    }
                }
            }
        }
        if let Some(id) = automatic_candidate(&self.snapshot.candidates, &target, observed) {
            let project = self
                .snapshot
                .candidates
                .iter()
                .find(|c| c.id == id)
                .ok_or("CANDIDATE_DISAPPEARED")?
                .project
                .clone();
            self.associate(project, false)?;
        } else {
            self.snapshot.phase = Phase::SelectProject;
        }
        Ok(())
    }
    fn associate(
        &mut self,
        mut project: ProjectRef,
        explicitly_selected: bool,
    ) -> Result<(), String> {
        let target = self.current_target()?;
        if project.environment != Environment::WindowsLocal {
            return Err("UNSUPPORTED_ENVIRONMENT: WSL/remote preview is not implemented".into());
        }
        // Re-read candidate directory before selecting it; candidate id is not an authorization credential.
        let evidence = Evidence {
            source: EvidenceSource::Manual,
            target_id: self.snapshot.target.as_ref().map(|t| t.id.clone()),
            process: self.snapshot.target.as_ref().map(|t| t.process.clone()),
            session_id: None,
            observed_at: now(),
            expires_at: now() + 30_000,
            environment: project.environment.clone(),
            raw_path: project.app_root.clone(),
            verifiable: true,
            active_scope: true,
        };
        let validated = inspect_directory(
            Path::new(&project.app_root),
            project.environment.clone(),
            evidence.clone(),
        )?;
        if !validated
            .iter()
            .any(|c| c.valid && c.project.id == project.id)
        {
            return Err("PROJECT_CHANGED_OR_INVALID".into());
        }
        let mut binding_evidence = self
            .snapshot
            .candidates
            .iter()
            .find(|c| c.project.id == project.id)
            .map(|c| c.evidence.clone())
            .unwrap_or_default();
        if explicitly_selected {
            binding_evidence.push(evidence);
        }
        self.close_session();
        self.serial += 1;
        let session = format!("{}-{}-{}", std::process::id(), now(), self.serial);
        project.sdk_id = self.snapshot.backend.sdk_path.as_ref().map(|path| {
            let mut hash = Sha256::new();
            hash.update(path.as_bytes());
            hash.update(
                self.snapshot
                    .backend
                    .sdk_version
                    .as_deref()
                    .unwrap_or("unknown")
                    .as_bytes(),
            );
            format!("sdk-{:x}", hash.finalize())
        });
        self.scheduler.set_workspace(&project.id, &session, now());
        self.snapshot.session_id = Some(session.clone());
        self.snapshot.project = Some(project.clone());
        self.snapshot.session_ref = Some(SessionRef {
            id: session.clone(),
            project: project.clone(),
            target,
            binding_evidence,
            sdk_id: project.sdk_id.clone(),
        });
        if let Some(store) = &mut self.store {
            store.remember(&project)?;
        }
        self.snapshot.trusted = self
            .store
            .as_ref()
            .is_some_and(|store| store.is_trusted(&project));
        let pending = self.watch_pending.clone();
        match watch_project(Path::new(&project.app_root), move |event| {
            if let Ok(mut pending) = pending.lock() {
                pending.push(event);
            }
        }) {
            Ok(watcher) => self.watcher = Some(watcher),
            Err(error) => self.diagnostic("WATCH_FAILED", error, CheckStatus::Fail),
        }
        self.scheduler.request_refresh(now());
        self.gate_preview();
        Ok(())
    }
    fn gate_preview(&mut self) {
        let Some(project) = self.snapshot.project.as_ref() else {
            return;
        };
        self.snapshot.trusted = self
            .store
            .as_ref()
            .is_some_and(|store| store.is_trusted(project));
        let reason = self.snapshot.backend.reason.clone();
        self.snapshot.phase = execution_gate(
            self.snapshot.trusted,
            self.snapshot.backend.available,
            &reason,
            || self.suspend_execution(),
        );
    }
    fn handle(&mut self, scope: RequestScope, action: Action) -> Result<AppSnapshot, String> {
        validate_scope(&self.snapshot, &scope)?;
        match action {
            Action::BeginDrag => self
                .controller
                .as_ref()
                .ok_or("UNSUPPORTED_PLATFORM")?
                .begin_drag()?,
            Action::Rediscover => {
                if let Err(error) = self.discover() {
                    self.snapshot.phase = Phase::SelectProject;
                    return Err(error);
                }
            }
            Action::Select(id) => {
                self.current_target()?;
                let project = self
                    .snapshot
                    .candidates
                    .iter()
                    .find(|c| c.id == id && c.valid)
                    .ok_or("INVALID_CANDIDATE")?
                    .project
                    .clone();
                self.associate(project, true)?;
            }
            Action::Directory(path) => {
                let target = self.current_target()?;
                let observed = now();
                let environment = path_environment(&path);
                let evidence = Evidence {
                    source: EvidenceSource::Manual,
                    target_id: Some(target.id),
                    process: Some(target.process),
                    session_id: None,
                    observed_at: observed,
                    expires_at: observed + 30_000,
                    environment: environment.clone(),
                    raw_path: path.to_string_lossy().into_owned(),
                    verifiable: true,
                    active_scope: true,
                };
                self.snapshot.candidates = inspect_directory(&path, environment, evidence)?;
                if self.snapshot.candidates.len() == 1 && self.snapshot.candidates[0].valid {
                    self.associate(self.snapshot.candidates[0].project.clone(), true)?;
                } else {
                    self.snapshot.phase = Phase::SelectProject;
                }
            }
            Action::Detach => {
                if let Some(controller) = &self.controller {
                    controller.detach();
                    self.detaching = true;
                }
                self.close_session();
                self.snapshot.target = None;
                self.snapshot.candidates.clear();
                self.snapshot.phase = Phase::Idle;
                self.snapshot.layout_required = false;
            }
            Action::Trust(id, trusted) => {
                self.current_target()?;
                let project = self
                    .snapshot
                    .project
                    .as_ref()
                    .filter(|p| p.id == id)
                    .ok_or("STALE_PROJECT")?
                    .clone();
                self.store
                    .as_mut()
                    .ok_or("PERSISTENCE_UNAVAILABLE")?
                    .set_trust(&project, trusted)?;
                if !trusted {
                    self.suspend_execution();
                    self.gate_preview();
                } else {
                    self.associate(project, false)?;
                }
            }
            Action::Sdk(path) => {
                self.store
                    .as_mut()
                    .ok_or("PERSISTENCE_UNAVAILABLE")?
                    .set_sdk_path(path)?;
                self.backend.stop();
                self.backend =
                    NativePreviewer::new(self.store.as_ref().and_then(WorkspaceStore::sdk_path));
                self.snapshot.backend = self.backend.probe();
                if let Some(project) = self.snapshot.project.clone() {
                    self.associate(project, false)?;
                }
            }
            Action::Refresh => {
                self.current_target()?;
                if self.snapshot.phase == Phase::Stopped {
                    let project = self.snapshot.project.clone().ok_or("PROJECT_REQUIRED")?;
                    self.associate(project, false)?;
                }
                self.gate_preview();
                if !self.snapshot.trusted {
                    return Err("TRUST_REQUIRED".into());
                }
                if !self.snapshot.backend.available {
                    return Err(self.snapshot.backend.reason.clone());
                }
                self.scheduler.request_refresh(now());
            }
            Action::Stop => {
                if let Ok(mut frames) = self.frames.lock() {
                    frames.clear();
                }
                self.watcher.take();
                self.watch_pending = Arc::new(Mutex::new(PendingWatch::default()));
                self.backend.stop();
                self.scheduler.stop();
                self.snapshot.session_id = None;
                self.snapshot.session_ref = None;
                self.snapshot.phase = Phase::Stopped;
            }
            Action::BinaryFrames(version) => {
                if !self.snapshot.trusted {
                    return Err("TRUST_REQUIRED".into());
                }
                let session = self
                    .snapshot
                    .session_id
                    .as_deref()
                    .ok_or("PREVIEW_SESSION_REQUIRED")?;
                self.frames
                    .lock()
                    .map_err(|_| "FRAME_UNAVAILABLE")?
                    .enable(version, session)?;
            }
            Action::LegacyFrames(version) => {
                if version != BINARY_FRAME_PROTOCOL_VERSION {
                    return Err("PROTOCOL_VERSION_UNSUPPORTED".into());
                }
                self.frames.lock().map_err(|_| "FRAME_UNAVAILABLE")?.clear();
            }
            Action::Arrange(consent) => {
                self.current_target()?;
                self.controller
                    .as_ref()
                    .ok_or("UNSUPPORTED_PLATFORM")?
                    .arrange_side_by_side(consent)?;
            }
            Action::Click {
                build_id,
                generation,
                point,
            } => {
                self.current_target()?;
                let status = self.scheduler.status();
                if self.snapshot.phase != Phase::Running
                    || status.stale
                    || status.build_id.as_deref() != Some(build_id.as_str())
                    || status.displayed_generation != Some(generation)
                    || generation != status.generation
                    || !self.snapshot.backend.can_click
                {
                    return Err("STALE_INPUT: current native surface required".into());
                }
                let project = self.snapshot.project.as_ref().ok_or("PROJECT_REQUIRED")?;
                if !self
                    .store
                    .as_ref()
                    .is_some_and(|store| store.is_trusted(project))
                {
                    return Err("TRUST_REQUIRED".into());
                }
                self.backend.input(PreviewInput::Click(point))?;
            }
        }
        self.publish();
        Ok(self.snapshot.clone())
    }
    fn drain_events(&mut self) {
        let window = self.window_pending.lock().ok().and_then(|mut p| p.take());
        if let Some(window) = window {
            self.apply_window(window);
        }
        let pending = self
            .watch_pending
            .lock()
            .ok()
            .map(|mut p| std::mem::take(&mut *p));
        if let Some(pending) = pending {
            if pending.changed {
                self.scheduler.request_refresh(now());
                self.gate_preview();
            }
            let had_error = pending.error.is_some();
            if let Some(error) = pending.error {
                self.diagnostic("WATCH_FAILED", error, CheckStatus::Fail);
            }
            if pending.changed || had_error {
                self.publish();
            }
        }
    }
    fn tick(&mut self) {
        if !self.snapshot.trusted || !self.snapshot.backend.available {
            return;
        }
        // Re-check configuration trust immediately before any future enabled adapter runs.
        let Some(project) = self.snapshot.project.clone() else {
            return;
        };
        if !self
            .store
            .as_ref()
            .is_some_and(|store| store.is_trusted(&project))
        {
            self.suspend_execution();
            self.snapshot.phase = Phase::Untrusted;
            self.publish();
            return;
        }
        if let Some(task) = self.scheduler.next_task(now()) {
            self.snapshot.phase = Phase::Building;
            self.publish();
            let result = self
                .backend
                .start(&project, true, &task.session_id)
                .and_then(|()| self.backend.refresh(&task));
            // Observe changes queued during synchronous adapter work before committing its result.
            self.drain_events();
            match result {
                Ok(()) => { /* Async completion is accepted only after actual build and native frame. */
                }
                Err(error) => {
                    if self
                        .scheduler
                        .complete(&task, BuildOutcome::Failure(error.clone()), now())
                        == CompletionDisposition::AcceptedFailure
                    {
                        self.snapshot.phase = if self.scheduler.status().stale {
                            Phase::Stale
                        } else {
                            Phase::Crashed
                        };
                        self.diagnostic("PREVIEW_FAILED", error, CheckStatus::Fail);
                    }
                }
            }
            self.publish();
        }
        if let Some((task, result)) = self.backend.completion() {
            self.drain_events();
            let outcome = result.map_or_else(BuildOutcome::Failure, |()| BuildOutcome::Success);
            let disposition = self.scheduler.complete(&task, outcome.clone(), now());
            if disposition == CompletionDisposition::AcceptedFailure {
                self.snapshot.phase = if self.scheduler.status().stale {
                    Phase::Stale
                } else {
                    Phase::Crashed
                };
                if let BuildOutcome::Failure(error) = outcome {
                    self.diagnostic("PREVIEW_FAILED", error, CheckStatus::Fail);
                }
            }
            self.publish();
        }
        for message in self.backend.runtime_messages() {
            if message.starts_with("RUNTIME_FAILED:") {
                self.backend.stop();
                self.scheduler.runtime_failed(&message);
                self.snapshot.phase = if self.scheduler.status().stale {
                    Phase::Stale
                } else {
                    Phase::Crashed
                };
                self.diagnostic("NATIVE_RUNTIME_FAILED", message, CheckStatus::Fail);
            } else {
                self.diagnostic("NATIVE_RUNTIME", message, CheckStatus::Pass);
            }
            self.publish();
        }
        if let Some(frame) = self.backend.binary_frame() {
            self.drain_events();
            if self.scheduler.accept_binary_frame(&frame).is_ok() {
                let changed = frame_changes_state(&self.snapshot, &self.scheduler.status());
                self.snapshot.phase = Phase::Running;
                // Pixels travel separately. Later frames must not re-render the whole tool.
                if changed {
                    self.publish();
                }
                let legacy = self
                    .frames
                    .lock()
                    .map(|frames| {
                        !frames.binary_enabled
                            || frames.binary_session.as_deref()
                                != Some(frame.metadata.session_id.as_str())
                    })
                    .unwrap_or(false);
                if legacy {
                    #[cfg(windows)]
                    match frame.legacy_png() {
                        Ok(legacy_frame) => {
                            let _ = self.app.emit("arkdock:frame", legacy_frame);
                        }
                        Err(error) => {
                            self.diagnostic("FRAME_COMPATIBILITY_FAILED", error, CheckStatus::Fail)
                        }
                    }
                }
                if let Ok(mut frames) = self.frames.lock() {
                    frames.push(frame);
                }
            }
        }
    }
}

fn check_window(window: &tauri::WebviewWindow) -> Result<(), String> {
    if window.label() != "main" {
        return Err("UNAUTHORIZED_WINDOW".into());
    }
    let url = window.url().map_err(|error| error.to_string())?;
    let local_asset = (url.scheme() == "tauri" && url.host_str() == Some("localhost"))
        || (matches!(url.scheme(), "http" | "https") && url.host_str() == Some("tauri.localhost"));
    let development = cfg!(debug_assertions)
        && url.scheme() == "http"
        && url.host_str() == Some("127.0.0.1")
        && url.port() == Some(1420);
    if !local_asset && !development {
        return Err("UNAUTHORIZED_ORIGIN".into());
    }
    Ok(())
}
async fn request(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
    scope: RequestScope,
    action: Action,
) -> Result<AppSnapshot, String> {
    check_window(&window)?;
    let sender = service.sender.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let (send, receive) = mpsc::channel();
        sender
            .try_send(Message::Action(scope, action, send))
            .map_err(|_| "REQUEST_QUEUE_BUSY".to_owned())?;
        receive
            .recv_timeout(Duration::from_secs(15))
            .map_err(|_| "REQUEST_TIMEOUT".to_owned())?
    })
    .await
    .map_err(|_| "REQUEST_WORKER_FAILED".to_owned())?
}
#[tauri::command]
async fn snapshot(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
) -> Result<AppSnapshot, String> {
    check_window(&window)?;
    service.snapshot()
}
#[tauri::command]
async fn minimize_window(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
    scope: RequestScope,
) -> Result<AppSnapshot, String> {
    check_window(&window)?;
    let snapshot = service.snapshot()?;
    validate_scope(&snapshot, &scope)?;
    window.minimize().map_err(|error| error.to_string())?;
    Ok(snapshot)
}
#[tauri::command]
async fn close_window(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
    scope: RequestScope,
) -> Result<AppSnapshot, String> {
    check_window(&window)?;
    let snapshot = service.snapshot()?;
    validate_scope(&snapshot, &scope)?;
    // Closing the owned main window follows RunEvent::Exit and shuts down its worker/Job.
    window.close().map_err(|error| error.to_string())?;
    Ok(snapshot)
}
#[tauri::command]
async fn begin_drag(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
    scope: RequestScope,
) -> Result<AppSnapshot, String> {
    let snapshot = request(window.clone(), service, scope, Action::BeginDrag).await?;
    window.start_dragging().map_err(|e| e.to_string())?;
    Ok(snapshot)
}
#[tauri::command]
async fn rediscover(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
    scope: RequestScope,
) -> Result<AppSnapshot, String> {
    request(window, service, scope, Action::Rediscover).await
}
#[tauri::command]
async fn select_candidate(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
    scope: RequestScope,
    candidate_id: String,
) -> Result<AppSnapshot, String> {
    if candidate_id.len() > 256 {
        return Err("INVALID_CANDIDATE".into());
    }
    request(window, service, scope, Action::Select(candidate_id)).await
}
#[tauri::command]
async fn select_directory(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
    scope: RequestScope,
) -> Result<AppSnapshot, String> {
    check_window(&window)?;
    validate_scope(&service.snapshot()?, &scope)?;
    let path = rfd::AsyncFileDialog::new()
        .set_title("选择实际鸿蒙工程目录")
        .pick_folder()
        .await
        .ok_or("DIRECTORY_SELECTION_CANCELLED")?;
    request(
        window,
        service,
        scope,
        Action::Directory(path.path().to_path_buf()),
    )
    .await
}
#[tauri::command]
async fn configure_sdk(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
    scope: RequestScope,
) -> Result<AppSnapshot, String> {
    check_window(&window)?;
    validate_scope(&service.snapshot()?, &scope)?;
    let path = rfd::AsyncFileDialog::new()
        .set_title("选择合法安装的 SDK 目录")
        .pick_folder()
        .await
        .ok_or("SDK_SELECTION_CANCELLED")?;
    request(
        window,
        service,
        scope,
        Action::Sdk(path.path().to_path_buf()),
    )
    .await
}
#[tauri::command]
async fn detach(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
    scope: RequestScope,
) -> Result<AppSnapshot, String> {
    request(window, service, scope, Action::Detach).await
}
#[tauri::command]
async fn set_trust(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
    scope: RequestScope,
    project_id: String,
    trusted: bool,
) -> Result<AppSnapshot, String> {
    if project_id.len() > 256 {
        return Err("INVALID_PROJECT".into());
    }
    request(window, service, scope, Action::Trust(project_id, trusted)).await
}
#[tauri::command]
async fn refresh_preview(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
    scope: RequestScope,
) -> Result<AppSnapshot, String> {
    request(window, service, scope, Action::Refresh).await
}
#[tauri::command]
async fn stop_preview(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
    scope: RequestScope,
) -> Result<AppSnapshot, String> {
    request(window, service, scope, Action::Stop).await
}

fn protocol_error(message: String, received_protocol_version: u32) -> PreviewProtocolError {
    PreviewProtocolError {
        code: message
            .split(':')
            .next()
            .unwrap_or("PREVIEW_ERROR")
            .to_owned(),
        message,
        expected_protocol_version: BINARY_FRAME_PROTOCOL_VERSION,
        received_protocol_version,
    }
}

#[tauri::command]
async fn enable_binary_preview_frames(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
    scope: RequestScope,
    protocol_version: u32,
) -> Result<AppSnapshot, PreviewProtocolError> {
    request(
        window,
        service,
        scope,
        Action::BinaryFrames(protocol_version),
    )
    .await
    .map_err(|error| protocol_error(error, protocol_version))
}

#[tauri::command]
async fn disable_binary_preview_frames(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
    scope: RequestScope,
    protocol_version: u32,
) -> Result<AppSnapshot, PreviewProtocolError> {
    request(
        window,
        service,
        scope,
        Action::LegacyFrames(protocol_version),
    )
    .await
    .map_err(|error| protocol_error(error, protocol_version))
}

#[tauri::command]
async fn read_preview_frame(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
    scope: RequestScope,
    protocol_version: u32,
) -> Result<tauri::ipc::Response, PreviewProtocolError> {
    let result = (|| {
        check_window(&window)?;
        if protocol_version != BINARY_FRAME_PROTOCOL_VERSION {
            return Err("PROTOCOL_VERSION_UNSUPPORTED".into());
        }
        // Keep state and the one unread frame locked in this order for the scoped read.
        let snapshot = service.state.lock().map_err(|_| "STATE_UNAVAILABLE")?;
        validate_scope(&snapshot, &scope)?;
        service
            .frames
            .lock()
            .map_err(|_| "FRAME_UNAVAILABLE")?
            .take_current(&snapshot)
    })();
    result
        .map(tauri::ipc::Response::new)
        .map_err(|error| protocol_error(error, protocol_version))
}
#[tauri::command]
async fn arrange_side_by_side(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
    scope: RequestScope,
    consent: bool,
) -> Result<AppSnapshot, String> {
    request(window, service, scope, Action::Arrange(consent)).await
}

#[tauri::command]
async fn click_preview(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
    scope: RequestScope,
    build_id: String,
    generation: u64,
    x: u32,
    y: u32,
) -> Result<AppSnapshot, String> {
    if build_id.len() > 256 || x >= 360 || y >= 640 {
        return Err("INVALID_NATIVE_INPUT".into());
    }
    request(
        window,
        service,
        scope,
        Action::Click {
            build_id,
            generation,
            point: NativePoint { x, y },
        },
    )
    .await
}

pub fn run() {
    let app = tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            snapshot,
            minimize_window,
            close_window,
            begin_drag,
            rediscover,
            select_candidate,
            select_directory,
            configure_sdk,
            detach,
            set_trust,
            refresh_preview,
            stop_preview,
            arrange_side_by_side,
            click_preview,
            enable_binary_preview_frames,
            disable_binary_preview_frames,
            read_preview_frame
        ])
        .on_page_load(|webview, payload| {
            if webview.label() == "main"
                && payload.event() == tauri::webview::PageLoadEvent::Started
            {
                if let Some(service) = webview.app_handle().try_state::<Service>() {
                    if let Ok(mut frames) = service.frames.lock() {
                        frames.clear();
                    }
                }
            }
        })
        .setup(|app| {
            let (sender, receiver) = mpsc::sync_channel(256);
            let store_result = app
                .path()
                .app_data_dir()
                .map_err(|e| e.to_string())
                .and_then(|path| WorkspaceStore::open(&path));
            let sdk = store_result
                .as_ref()
                .ok()
                .and_then(WorkspaceStore::sdk_path)
                .or_else(crate::preview::environment::auto_detect_sdk);
            let backend = NativePreviewer::new(sdk);
            let state = Arc::new(Mutex::new(initial(backend.capabilities())));
            let service_state = state.clone();
            let frames = Arc::new(Mutex::new(FrameDelivery::default()));
            let service_frames = frames.clone();
            let handle = app.handle().clone();
            let window_pending = Arc::new(Mutex::new(None));
            let main = app
                .get_webview_window("main")
                .ok_or("MAIN_WINDOW_MISSING")?;
            #[cfg(windows)]
            let own_handle = main.hwnd()?.0 as isize;
            #[cfg(not(windows))]
            let own_handle = {
                let _ = main;
                0
            };
            let worker = thread::spawn(move || {
                let mut engine = Engine {
                    app: handle,
                    output: state,
                    snapshot: initial(backend.capabilities()),
                    controller: None,
                    store: None,
                    backend,
                    scheduler: BuildCoordinator::new(300),
                    watcher: None,
                    window_pending: window_pending.clone(),
                    watch_pending: Arc::new(Mutex::new(PendingWatch::default())),
                    serial: 0,
                    detaching: false,
                    frames,
                };
                match store_result {
                    Ok(store) => engine.store = Some(store),
                    Err(error) => {
                        engine.diagnostic("SETTINGS_UNAVAILABLE", error, CheckStatus::Fail)
                    }
                }
                match WindowController::start(own_handle, move |update| {
                    if let Ok(mut pending) = window_pending.lock() {
                        *pending = Some(update);
                    }
                }) {
                    Ok(controller) => engine.controller = Some(controller),
                    Err(error) => {
                        engine.diagnostic("WINDOWING_UNAVAILABLE", error, CheckStatus::Blocked)
                    }
                }
                engine.publish();
                loop {
                    match receiver.recv_timeout(Duration::from_millis(16)) {
                        Ok(Message::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Ok(Message::Action(scope, action, reply)) => {
                            engine.drain_events();
                            let result = engine.handle(scope, action);
                            if let Err(error) = &result {
                                engine.diagnostic(
                                    "REQUEST_FAILED",
                                    error.clone(),
                                    CheckStatus::Fail,
                                );
                                engine.publish();
                            }
                            let _ = reply.send(result);
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                    }
                    engine.drain_events();
                    engine.tick();
                }
                engine.close_session();
                engine.controller.take();
            });
            app.manage(Service {
                sender,
                state: service_state,
                worker: Mutex::new(Some(worker)),
                frames: service_frames,
            });
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("ArkDock desktop startup failed");
    app.run(|handle, event| {
        if matches!(event, tauri::RunEvent::Exit) {
            handle.state::<Service>().shutdown();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_teardown_releases_pixels_and_restores_legacy_delivery() {
        let mut delivery = FrameDelivery::default();
        delivery.enable(1, "s").unwrap();
        delivery.clear();
        assert!(!delivery.binary_enabled);
    }

    #[test]
    fn binary_delivery_is_bounded_versioned_and_rejects_a_replaced_session() {
        use crate::preview::frames::BinaryFrame;
        let mut state = initial(crate::preview::probe(None));
        state.session_id = Some("s".into());
        state.trusted = true;
        state.phase = Phase::Running;
        state.build.build_id = Some("b".into());
        state.build.generation = 1;
        state.build.displayed_generation = Some(1);
        state.project = Some(ProjectRef {
            id: "w".into(),
            name: "test".into(),
            environment: Environment::WindowsLocal,
            workspace_root: "test".into(),
            app_root: "test".into(),
            module: "entry".into(),
            variant: "default".into(),
            sdk_id: None,
        });
        let frame = |sequence| BinaryFrame {
            metadata: BinaryFrameMetadata {
                protocol_version: 1,
                workspace_id: "w".into(),
                session_id: "s".into(),
                build_id: "b".into(),
                generation: 1,
                sequence,
                width: 360,
                height: 640,
                captured_at: 1,
                rotation: 0,
                format: NativeFrameFormat::Jpeg,
                payload_length: 4,
            },
            payload: vec![0xff, 0xd8, 0xff, 0xd9],
        };
        let mut delivery = FrameDelivery::default();
        assert_eq!(
            delivery.enable(2, "s").unwrap_err(),
            "PROTOCOL_VERSION_UNSUPPORTED"
        );
        assert_eq!(
            delivery.enable(1, "").unwrap_err(),
            "PREVIEW_SESSION_REQUIRED"
        );
        delivery.enable(1, "s").unwrap();
        delivery.push(frame(1));
        delivery.push(frame(2));
        let packet = delivery.take_current(&state).unwrap();
        let length = u32::from_le_bytes(packet[..4].try_into().unwrap()) as usize;
        let metadata: BinaryFrameMetadata = serde_json::from_slice(&packet[4..4 + length]).unwrap();
        assert_eq!(metadata.sequence, 2);
        assert_eq!(packet[4 + length..], [0xff, 0xd8, 0xff, 0xd9]);
        assert!(delivery.take_current(&state).unwrap().is_empty());
        delivery.push(frame(3));
        state.session_id = Some("new".into());
        assert_eq!(delivery.take_current(&state).unwrap_err(), "STALE_SESSION");
        delivery.clear();
        assert_eq!(
            delivery.take_current(&state).unwrap_err(),
            "BINARY_TRANSPORT_NOT_ENABLED"
        );
    }

    #[test]
    fn fingerprint_change_closes_active_execution_before_entering_untrusted() {
        let mut scheduler = BuildCoordinator::new(0);
        scheduler.set_workspace("workspace", "session", 1);
        scheduler.request_refresh(1);
        let old = scheduler.next_task(1).unwrap();
        let mut closed = false;
        let phase = execution_gate(false, true, "verified", || {
            scheduler.stop();
            closed = true;
        });
        assert!(closed);
        assert_eq!(phase, Phase::Untrusted);
        assert!(!scheduler.is_building());
        assert_eq!(
            scheduler.complete(&old, BuildOutcome::Success, 2),
            CompletionDisposition::RejectedStopped
        );
    }

    #[test]
    fn source_only_change_keeps_the_trusted_execution_open() {
        let mut closed = false;
        assert_eq!(
            execution_gate(true, true, "verified", || closed = true),
            Phase::Building
        );
        assert!(!closed);
    }

    #[test]
    fn only_first_frame_or_changed_build_publishes_a_snapshot() {
        let mut snapshot = initial(crate::preview::probe(None));
        snapshot.phase = Phase::Building;
        let mut build = snapshot.build.clone();
        build.displayed_generation = Some(1);
        assert!(frame_changes_state(&snapshot, &build));

        snapshot.phase = Phase::Running;
        snapshot.build = build.clone();
        assert!(!frame_changes_state(&snapshot, &build));

        build.generation += 1;
        build.displayed_generation = Some(build.generation);
        assert!(frame_changes_state(&snapshot, &build));

        snapshot.build = build.clone();
        snapshot.phase = Phase::Stale;
        assert!(frame_changes_state(&snapshot, &build));
    }
}
