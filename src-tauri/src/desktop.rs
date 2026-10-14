//! A single backend worker owns sessions. Hooks/watchers only enqueue lightweight messages.
#[cfg(not(windows))]
use crate::discovery::resolve;
use crate::discovery::{
    automatic_candidate, current_active_evidence, inspect_directory, path_environment,
    resolution_complete, WORKSPACE_OBSERVATION_TTL_MS,
};
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
mod agent_delivery;
mod association;
mod diagnostics;
mod frame_delivery;
mod inspection;
mod native_controls;
mod session;
mod visibility;
use association::{workspace_observation, AssociationDecision};
use frame_delivery::FrameDelivery;
use inspection::{DataAction, DataResult, PendingInspection};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{Emitter, Manager};

enum Action {
    BeginDrag,
    Rediscover,
    Select(String),
    Directory(PathBuf),
    InspectDirectory(PathBuf),
    Detach,
    Trust(String, bool),
    Sdk(PathBuf),
    Refresh,
    Stop,
    BinaryFrames(u32, String),
    LegacyFrames(u32, String),
    Arrange(bool),
    MaximizePair,
    RestorePair,
    Display(DisplayMode, u64),
    OrbAutoHide(bool),
    Motion(MotionSettings),
    DisplaySnapshot,
    Click {
        build_id: String,
        generation: u64,
        point: NativePoint,
    },
}
enum Message {
    Metrics(RequestScope, mpsc::Sender<Result<MotionMetrics, String>>),
    Action(
        RequestScope,
        Action,
        mpsc::Sender<Result<AppSnapshot, String>>,
    ),
    Data(
        RequestScope,
        DataAction,
        mpsc::Sender<Result<DataResult, String>>,
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
    selections: Option<crate::selection::SelectionStore>,
    agents: crate::agent::AgentRouter,
    assisted: Arc<Mutex<crate::agent::assisted::AssistedController>>,
    assisted_busy: Arc<std::sync::atomic::AtomicBool>,
    pending_assisted: Option<agent_delivery::PendingAssisted>,
    pending_pair: Option<agent_delivery::PendingPair>,
    assisted_offer_scope: Option<RequestScope>,
    inspection: Option<PendingInspection>,
    inspection_tree: Option<InspectorTree>,
    inspection_anchor: Option<BinaryFrame>,
    last_inspection_at: u64,
    last_selection_sync: u64,
    controls: native_controls::Controls,
    device_runtime: crate::preview::device_runtime::DeviceRuntimeAdapter,
    device_sdk: Option<PathBuf>,
    native_visible: Option<bool>,
    last_visibility_sync: u64,
    last_visibility_emit: u64,
    own_handle: isize,
    associated_manually: bool,
    next_workspace_probe: Instant,
    /// Wall-clock instant at which candidate display semantics may change; `0` means
    /// nothing is scheduled. Set from the published snapshot, never from the frontend.
    next_candidate_recheck_at: u64,
}

#[derive(Clone, Copy)]
enum DiscoveryMode {
    Explicit,
    Lightweight,
}

const WORKSPACE_PROBE_INTERVAL: Duration = Duration::from_secs(5);
/// When a candidate deadline has already passed without changing the displayed
/// category, recheck on this bounded interval instead of every actor tick.
const CANDIDATE_RECHECK_INTERVAL_MS: u64 = 1_000;

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
fn app_data_selection_store(
    app: &tauri::AppHandle,
) -> Result<crate::selection::SelectionStore, String> {
    app.path()
        .app_data_dir()
        .map_err(|_| "SELECTION_APP_DATA_UNAVAILABLE".to_owned())
        .and_then(|path| crate::selection::SelectionStore::open(&path))
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
        workspace_observation: None,
        trusted: false,
        session_id: None,
        session_ref: None,
        backend,
        build: BuildCoordinator::new(300).status(),
        diagnostics: Vec::new(),
        dock_hint: None,
        layout_required: false,
        pair_layout: Some(PairLayoutStatus::default()),
        display: DisplayStatus {
            mode: DisplayMode::Expanded,
            request_id: 0,
            orb_auto_hide: false,
            tray_available: false,
            reason: Some("Native display controller has not confirmed availability".into()),
        },
        motion: MotionSettings {
            reduced: false,
            intensity: MotionIntensity::Standard,
        },
        dock_confirmation: None,
    }
}

fn frame_changes_state(snapshot: &AppSnapshot, build: &BuildStatus) -> bool {
    snapshot.phase != Phase::Running || snapshot.build != *build
}

fn absolute_directory_input(value: &str) -> Result<PathBuf, String> {
    if value.len() > 4096 {
        return Err("INVALID_DIRECTORY_PATH: enter a bounded absolute directory path".into());
    }
    let value = value.trim();
    if value.is_empty() || value.len() > 4096 || value.chars().any(char::is_control) {
        return Err("INVALID_DIRECTORY_PATH: enter a bounded absolute directory path".into());
    }
    let path = PathBuf::from(value);
    let environment = path_environment(&path);
    let explicit_uri = ["wsl://", "ssh://", "vscode-remote://"]
        .iter()
        .any(|prefix| {
            value.strip_prefix(prefix).is_some_and(|suffix| {
                suffix
                    .split_once('/')
                    .is_some_and(|(host, remainder)| !host.is_empty() && !remainder.is_empty())
            })
        });
    if environment == Environment::WindowsLocal
        && value.starts_with('/')
        && !value.starts_with("//")
    {
        return Err(
            "UNSUPPORTED_ENVIRONMENT: an unqualified POSIX path is not a Windows directory".into(),
        );
    }
    if !path.is_absolute() && !explicit_uri {
        return Err("ABSOLUTE_DIRECTORY_REQUIRED: relative paths are not accepted".into());
    }
    Ok(path)
}

impl Engine {
    fn sync_visibility(&mut self) {
        let Some(window) = self.app.get_webview_window("main") else {
            return;
        };
        let visible = visibility::visible(self.own_handle);
        if !visible {
            self.backend.cancel_controls();
        }
        if self.native_visible != Some(visible)
            || now().saturating_sub(self.last_visibility_emit) >= 1000
        {
            self.native_visible = Some(visible);
            self.last_visibility_emit = now();
            let _ = window.emit("arkdock:visibility", PreviewVisibility { visible });
        }
    }
    fn publish(&mut self) {
        self.sync_control_lifecycle();
        self.snapshot.revision += 1;
        self.snapshot.build = self.scheduler.status();
        // Candidate categories are backend truth: they are recomputed for the snapshot
        // that is emitted, never derived again by the frontend.
        self.grade_candidates();
        if let Some(selections) = &self.selections {
            let _ = selections.sync_runtime(&self.snapshot, now());
        }
        if let Ok(mut state) = self.output.lock() {
            *state = self.snapshot.clone();
        }
        let _ = self.app.emit("arkdock:state", &self.snapshot);
    }
    /// Recomputes candidate display semantics and records when they may move.
    /// Returns whether any candidate's visible category changed.
    fn grade_candidates(&mut self) -> bool {
        let Some(target) = self.snapshot.target.clone() else {
            self.next_candidate_recheck_at = 0;
            return false;
        };
        let observed = now();
        let observation = self.snapshot.workspace_observation.clone();
        let applied = crate::discovery::classification::apply(
            &mut self.snapshot.candidates,
            &target,
            observation.as_ref(),
            observed,
        );
        self.next_candidate_recheck_at = if applied.next_recheck_at == 0 {
            0
        } else if applied.next_recheck_at > observed {
            applied.next_recheck_at
        } else {
            observed.saturating_add(CANDIDATE_RECHECK_INTERVAL_MS)
        };
        applied.changed
    }
    /// Expired evidence must stop presenting a "current workspace" candidate even when
    /// no window event or file change arrives to publish another snapshot.
    fn refresh_candidate_semantics(&mut self) {
        if self.next_candidate_recheck_at == 0 || now() < self.next_candidate_recheck_at {
            return;
        }
        if self.grade_candidates() {
            self.publish();
        }
    }
    fn diagnostic(&mut self, code: &str, message: String, status: CheckStatus) {
        // The scope is captured before the mutable borrow so a diagnostic is attributed
        // to the project/session that produced it, never to one being replaced.
        let scope = diagnostics::DiagnosticScope {
            workspace_id: self.snapshot.project.as_ref().map(|p| p.id.clone()),
            session_id: self.snapshot.session_id.clone(),
            build_id: self.scheduler.status().build_id,
            backend_id: Some(self.snapshot.backend.id.clone()),
        };
        diagnostics::record(&mut self.snapshot.diagnostics, scope, code, message, status);
    }
    fn apply_window(&mut self, update: WindowUpdate) {
        // Display surfaces are independent of project execution and preview delivery.
        self.snapshot.display = update.display;
        self.snapshot.dock_confirmation = update.dock_confirmation;
        if update.exit_requested {
            self.app.exit(0);
            return;
        }
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
        self.snapshot.pair_layout = Some(update.pair_layout);
        if changed {
            self.close_session();
            self.snapshot.candidates.clear();
            self.snapshot.workspace_observation = None;
            if self.snapshot.target.is_some() {
                if let Err(error) = self.discover(DiscoveryMode::Explicit) {
                    self.discovery_failed(&error);
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
    fn discover(&mut self, mode: DiscoveryMode) -> Result<(), String> {
        let target = self.current_target()?;
        self.next_workspace_probe = Instant::now() + WORKSPACE_PROBE_INTERVAL;
        if matches!(mode, DiscoveryMode::Explicit) && self.snapshot.project.is_none() {
            self.snapshot.phase = Phase::Discovering;
            self.publish();
        }
        let observed = now();
        #[cfg(windows)]
        let (mut candidates, observation, source_error) = {
            let controller = self.controller.as_ref().ok_or("WINDOWING_UNAVAILABLE")?;
            let scope = controller.discovery_scope(&target.id)?;
            let tabs = match mode {
                DiscoveryMode::Explicit => {
                    WindowsTabResolver::collect(scope.handle, &target, observed)
                }
                DiscoveryMode::Lightweight => {
                    let Some(tabs) =
                        WindowsTabResolver::collect_lightweight(scope.handle, &target, observed)
                    else {
                        return Ok(());
                    };
                    tabs
                }
            };
            // HWND equality is insufficient: verify the controller's current lifecycle again.
            controller.discovery_scope(&target.id)?;
            let candidates = resolve_with_sources(
                &target,
                observed,
                &[&tabs, &RegistrationRegistry::default(), &TitleResolver],
            );
            match candidates {
                Ok(candidates) => (candidates, tabs.observation(), None),
                Err(error) => (
                    Vec::new(),
                    Some(WorkspaceObservation {
                        status: WorkspaceStatus::Unknown,
                        project_name: tabs
                            .observation()
                            .and_then(|observation| observation.project_name),
                        active_path: None,
                        reason: format!(
                            "Current workspace metadata could not be verified: {error}"
                        ),
                        observed_at: observed,
                    }),
                    Some(error),
                ),
            }
        };
        #[cfg(not(windows))]
        let (mut candidates, observation, source_error) = match resolve(&target, observed) {
            Ok(candidates) => (candidates, None, None),
            Err(error) => (Vec::new(), None, Some(error)),
        };
        self.current_target()?;
        let current_observation =
            workspace_observation(&candidates, &target, observed, observation.clone());
        if matches!(mode, DiscoveryMode::Lightweight) {
            // A poll carries the user's manual choice and historical suggestions forward
            // without re-inspecting every old root; neither becomes automatically eligible.
            association::carry_forward_previous_candidates(
                &self.snapshot.candidates,
                &mut candidates,
                &target,
                &current_observation,
            );
        }
        // History is separate from current-window evidence, including when its path matches.
        if let Some(store) = self
            .store
            .as_ref()
            .filter(|_| matches!(mode, DiscoveryMode::Explicit))
        {
            let mut inspected_roots: Vec<(Environment, String, Vec<Candidate>)> = Vec::new();
            for project in store.history() {
                let evidence = Evidence {
                    source: EvidenceSource::History,
                    target_id: None,
                    process: None,
                    session_id: None,
                    observed_at: observed,
                    expires_at: observed + 30_000,
                    environment: project.environment.clone(),
                    raw_path: project.app_root.clone(),
                    verifiable: false,
                    active_scope: false,
                };
                if !inspected_roots.iter().any(|(environment, root, _)| {
                    environment == &project.environment && root == &project.app_root
                }) {
                    let items = inspect_directory(
                        Path::new(&project.app_root),
                        project.environment.clone(),
                        evidence,
                    )
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|item| {
                        // Cache only selections that actually exist in the bounded history,
                        // not every variant inspected in every root (at most 64 records).
                        store.history().iter().any(|selected| {
                            selected.environment == project.environment
                                && selected.app_root == project.app_root
                                && selected.id == item.project.id
                                && selected.module == item.project.module
                                && selected.variant == item.project.variant
                        })
                    })
                    .collect();
                    inspected_roots.push((
                        project.environment.clone(),
                        project.app_root.clone(),
                        items,
                    ));
                }
                if let Some((_, _, items)) =
                    inspected_roots.iter().find(|(environment, root, _)| {
                        environment == &project.environment && root == &project.app_root
                    })
                {
                    // A historical selection is one module/variant, not every module in its
                    // application directory. Removed/library selections cannot create new history.
                    for item in items.iter().filter(|item| {
                        item.project.id == project.id
                            && item.project.module == project.module
                            && item.project.variant == project.variant
                    }) {
                        if !candidates.iter().any(|c| c.id == item.id) {
                            candidates.push(item.clone());
                        }
                    }
                }
            }
        }
        self.snapshot.workspace_observation = Some(workspace_observation(
            &candidates,
            &target,
            observed,
            observation,
        ));
        self.snapshot.candidates = candidates;
        self.reconcile_workspace(&target, observed)?;
        if let Some(error) = source_error {
            return Err(error);
        }
        Ok(())
    }
    /// Applies the association policy to the current candidate set.
    ///
    /// The decision itself lives in `association::decide`; this method only performs the
    /// side effects it names, in order, so a release always closes the old session before
    /// a replacement can be associated.
    fn reconcile_workspace(&mut self, target: &TargetInfo, observed: u64) -> Result<(), String> {
        let decision = association::decide(
            &self.snapshot.candidates,
            target,
            self.snapshot.workspace_observation.as_ref(),
            self.snapshot.project.as_ref(),
            self.associated_manually,
            observed,
        );
        let rebind = match decision {
            AssociationDecision::Hold => {
                self.refresh_binding_evidence();
                // Fresh metadata for the same workspace must not restart a preview or lift Stop.
                return Ok(());
            }
            AssociationDecision::AwaitSelection => {
                self.snapshot.phase = Phase::SelectProject;
                return Ok(());
            }
            AssociationDecision::Associate { project_id } => Some(project_id),
            AssociationDecision::Release { reason, rebind } => {
                // Stop watcher/build/runtime before the old workspace can emit another frame.
                self.close_session();
                self.snapshot.phase = Phase::SelectProject;
                self.diagnostic(
                    reason,
                    "Current workspace evidence changed or is no longer reliable; the previous association was suspended".into(),
                    CheckStatus::Blocked,
                );
                rebind
            }
        };
        let Some(id) = rebind else {
            return Ok(());
        };
        let project = self
            .snapshot
            .candidates
            .iter()
            .find(|candidate| candidate.id == id)
            .ok_or("CANDIDATE_DISAPPEARED")?
            .project
            .clone();
        self.associate(project, false)
    }
    /// Rebinds only the evidence of an association that is still supported by current
    /// host evidence.
    fn refresh_binding_evidence(&mut self) {
        let Some(project_id) = self.snapshot.project.as_ref().map(|p| p.id.clone()) else {
            return;
        };
        let Some(previous) = self
            .snapshot
            .session_ref
            .as_ref()
            .map(|session| session.binding_evidence.clone())
        else {
            return;
        };
        let refreshed = association::refreshed_binding_evidence(
            &self.snapshot.candidates,
            &project_id,
            &previous,
            self.associated_manually,
        );
        if let Some(session) = &mut self.snapshot.session_ref {
            session.binding_evidence = refreshed;
        }
    }
    fn discovery_failed(&mut self, error: &str) {
        association::revoke_unverified_evidence(&mut self.snapshot.candidates);
        self.snapshot.workspace_observation = Some(WorkspaceObservation {
            status: WorkspaceStatus::Unknown,
            project_name: None,
            active_path: None,
            reason: format!("Current workspace metadata could not be verified: {error}"),
            observed_at: now(),
        });
        if !self.associated_manually {
            self.close_session();
            self.snapshot.phase = Phase::SelectProject;
        }
    }
    fn probe_workspace(&mut self) {
        if self.snapshot.target.is_none() || Instant::now() < self.next_workspace_probe {
            return;
        }
        #[cfg(windows)]
        {
            // The resolver returns early for other hosts and never performs generic tab scanning
            // here. Codex uses a bounded, noninteractive metadata-only scan every five seconds.
            let previous = self.snapshot.workspace_observation.clone();
            if let Err(error) = self.discover(DiscoveryMode::Lightweight) {
                self.discovery_failed(&error);
                self.diagnostic("DISCOVERY_FAILED", error, CheckStatus::Blocked);
            }
            if self.snapshot.workspace_observation != previous {
                self.publish();
            }
        }
        #[cfg(not(windows))]
        {
            self.next_workspace_probe = Instant::now() + WORKSPACE_PROBE_INTERVAL;
        }
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
        let observed = now();
        if explicitly_selected
            && association::conflicts_with_active_workspace(
                &project,
                self.snapshot.workspace_observation.as_ref(),
                observed,
            )
        {
            return Err("WORKSPACE_CONFLICT: the host's current project differs from this directory; rediscover its current workspace".into());
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
        self.current_target()?;
        let binding_evidence = association::binding_evidence(
            &self.snapshot.candidates,
            &project.id,
            self.snapshot
                .session_ref
                .as_ref()
                .map(|session| session.binding_evidence.as_slice())
                .unwrap_or_default(),
            explicitly_selected.then(|| evidence.clone()),
            self.associated_manually,
        );
        let manual = association::is_manual_association(
            explicitly_selected,
            self.associated_manually,
            self.snapshot
                .project
                .as_ref()
                .map(|current| current.id.as_str()),
            &project.id,
        );
        self.close_session();
        self.associated_manually = manual;
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
    fn inspect_manual_directory(
        &mut self,
        path: PathBuf,
        select_unique: bool,
    ) -> Result<(), String> {
        let target = self.current_target()?;
        let observed = now();
        let environment = path_environment(&path);
        if environment == Environment::WindowsLocal && !path.is_dir() {
            return Err("DIRECTORY_REQUIRED: select or enter an existing directory".into());
        }
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
            // Validation alone proves a directory, not the host's active workspace.
            active_scope: false,
        };
        let inspected = inspect_directory(&path, environment, evidence)?;
        self.current_target()?;
        let unique =
            (inspected.len() == 1 && inspected[0].valid).then(|| inspected[0].project.clone());
        {
            let additional = inspected
                .iter()
                .filter(|candidate| {
                    !self
                        .snapshot
                        .candidates
                        .iter()
                        .any(|existing| existing.id == candidate.id)
                })
                .count();
            if self.snapshot.candidates.len().saturating_add(additional) > 256 {
                return Err(
                    "CANDIDATE_LIMIT: rediscover before inspecting more directories".into(),
                );
            }
            for mut candidate in inspected {
                if let Some(existing) = self
                    .snapshot
                    .candidates
                    .iter_mut()
                    .find(|existing| existing.id == candidate.id)
                {
                    // The read-only inspection refreshes the directory's validity, without
                    // replacing current host evidence with a manual current-workspace claim.
                    existing.valid = candidate.valid;
                    existing.evidence.retain(|evidence| {
                        evidence.source != EvidenceSource::Manual || evidence.active_scope
                    });
                    existing.evidence.append(&mut candidate.evidence);
                } else {
                    self.snapshot.candidates.push(candidate);
                }
            }
        }
        if select_unique {
            if let Some(project) = unique {
                self.associate(project, true)?;
            } else if self.snapshot.project.is_none() {
                self.snapshot.phase = Phase::SelectProject;
            }
        } else if self.snapshot.project.is_none() {
            self.snapshot.phase = Phase::SelectProject;
        }
        Ok(())
    }
    fn handle(&mut self, scope: RequestScope, action: Action) -> Result<AppSnapshot, String> {
        validate_scope(&self.snapshot, &scope)?;
        match action {
            Action::Display(mode, request_id) => {
                if request_id == 0 || request_id > 9_007_199_254_740_991 {
                    return Err("INVALID_DISPLAY_REQUEST_ID".into());
                }
                self.controller
                    .as_ref()
                    .ok_or("WINDOWING_UNAVAILABLE")?
                    .set_display_mode(mode, request_id)?;
                self.drain_events();
            }
            Action::OrbAutoHide(enabled) => {
                self.controller
                    .as_ref()
                    .ok_or("WINDOWING_UNAVAILABLE")?
                    .set_orb_auto_hide(enabled)?;
                self.drain_events();
            }
            Action::Motion(preferences) => {
                self.controller
                    .as_ref()
                    .ok_or("WINDOWING_UNAVAILABLE")?
                    .set_motion_preferences(preferences.clone())?;
                self.snapshot.motion = preferences;
                self.drain_events();
            }
            Action::DisplaySnapshot => self.drain_events(),
            Action::BeginDrag => self
                .controller
                .as_ref()
                .ok_or("UNSUPPORTED_PLATFORM")?
                .begin_drag()?,
            Action::Rediscover => {
                if let Err(error) = self.discover(DiscoveryMode::Explicit) {
                    self.discovery_failed(&error);
                    self.publish();
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
                self.inspect_manual_directory(path, true)?;
            }
            Action::InspectDirectory(path) => {
                self.inspect_manual_directory(path, false)?;
            }
            Action::Detach => {
                if let Some(controller) = &self.controller {
                    controller.detach();
                    self.detaching = true;
                }
                self.close_session();
                self.snapshot.target = None;
                self.snapshot.candidates.clear();
                self.snapshot.workspace_observation = None;
                self.snapshot.phase = Phase::Idle;
                self.snapshot.layout_required = false;
                self.snapshot.dock_confirmation = None;
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
                self.backend.cancel_controls();
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
            Action::Stop => self.stop_execution(),
            Action::BinaryFrames(version, consumer_id) => {
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
                    .enable(version, session, &consumer_id)?;
            }
            Action::LegacyFrames(version, consumer_id) => {
                if version != BINARY_FRAME_PROTOCOL_VERSION {
                    return Err("PROTOCOL_VERSION_UNSUPPORTED".into());
                }
                self.frames
                    .lock()
                    .map_err(|_| "FRAME_UNAVAILABLE")?
                    .disable(&consumer_id)?;
            }
            Action::Arrange(consent) => {
                self.current_target()?;
                self.controller
                    .as_ref()
                    .ok_or("UNSUPPORTED_PLATFORM")?
                    .arrange_side_by_side(consent)?;
            }
            Action::MaximizePair => {
                let target = self.current_target()?;
                self.controller
                    .as_ref()
                    .ok_or("UNSUPPORTED_PLATFORM")?
                    .maximize_pair(&target.id)?;
                self.drain_events();
            }
            Action::RestorePair => {
                let target = self.current_target()?;
                self.controller
                    .as_ref()
                    .ok_or("UNSUPPORTED_PLATFORM")?
                    .restore_pair(&target.id)?;
                self.drain_events();
            }
            Action::Click {
                build_id,
                generation,
                point,
            } => {
                self.current_target()?;
                self.ensure_native_surface_ready()?;
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
        self.probe_workspace();
        self.refresh_candidate_semantics();
        if !self.associated_manually && self.snapshot.project.is_some() {
            // The claim must still be supported by a current automatic candidate; the
            // freshness gate itself lives in the association policy.
            let target = self.snapshot.target.clone();
            let project_id = self
                .snapshot
                .project
                .as_ref()
                .map(|project| project.id.clone());
            let expired = match (target, project_id) {
                (Some(target), Some(project_id)) => !association::association_still_current(
                    &self.snapshot.candidates,
                    &target,
                    &project_id,
                    now(),
                ),
                _ => false,
            };
            if expired {
                self.close_session();
                self.snapshot.phase = Phase::SelectProject;
                self.snapshot.workspace_observation = Some(WorkspaceObservation {
                    status: WorkspaceStatus::Unknown,
                    project_name: None,
                    active_path: None,
                    reason: "The previous active workspace evidence expired; rediscover or select a directory".into(),
                    observed_at: now(),
                });
                self.publish();
            }
        }
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
                    .map(|frames| frames.wants_legacy_delivery(&frame.metadata.session_id))
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
        && window
            .app_handle()
            .config()
            .build
            .dev_url
            .as_ref()
            .is_some_and(|configured| {
                configured.scheme() == "http"
                    && configured.host_str() == Some("127.0.0.1")
                    && configured.origin() == url.origin()
            });
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
) -> Result<AppSnapshot, IpcError> {
    check_window(&window)?;
    let sender = service.sender.clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<AppSnapshot, IpcError> {
        let (send, receive) = mpsc::channel();
        sender
            .try_send(Message::Action(scope, action, send))
            .map_err(|_| IpcError::from("REQUEST_QUEUE_BUSY"))?;
        let result = receive
            .recv_timeout(Duration::from_secs(15))
            .map_err(|_| IpcError::from("REQUEST_TIMEOUT"))?;
        result.map_err(IpcError::from)
    })
    .await
    .map_err(|_| "REQUEST_WORKER_FAILED")?
}
#[tauri::command]
async fn snapshot(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
) -> Result<AppSnapshot, IpcError> {
    check_window(&window)?;
    service.snapshot().map_err(IpcError::from)
}
#[tauri::command]
async fn minimize_window(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
    scope: RequestScope,
) -> Result<AppSnapshot, IpcError> {
    check_window(&window)?;
    let snapshot = service.snapshot()?;
    validate_scope(&snapshot, &scope)?;
    window.minimize().map_err(|error| error.to_string())?;
    Ok(snapshot)
}
#[tauri::command]
async fn collapse_to_orb(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
    scope: RequestScope,
    request_id: u64,
) -> Result<AppSnapshot, IpcError> {
    request(
        window,
        service,
        scope,
        Action::Display(DisplayMode::Orb, request_id),
    )
    .await
}
#[tauri::command]
async fn expand_from_orb(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
    scope: RequestScope,
    request_id: u64,
) -> Result<AppSnapshot, IpcError> {
    request(
        window,
        service,
        scope,
        Action::Display(DisplayMode::Expanded, request_id),
    )
    .await
}
#[tauri::command]
async fn hide_to_tray(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
    scope: RequestScope,
    request_id: u64,
) -> Result<AppSnapshot, IpcError> {
    request(
        window,
        service,
        scope,
        Action::Display(DisplayMode::Hidden, request_id),
    )
    .await
}
#[tauri::command]
async fn show_from_tray(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
    scope: RequestScope,
    request_id: u64,
) -> Result<AppSnapshot, IpcError> {
    request(
        window,
        service,
        scope,
        Action::Display(DisplayMode::Expanded, request_id),
    )
    .await
}
#[tauri::command]
async fn set_orb_auto_hide(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
    scope: RequestScope,
    enabled: bool,
) -> Result<AppSnapshot, IpcError> {
    request(window, service, scope, Action::OrbAutoHide(enabled)).await
}
#[tauri::command]
async fn set_motion_preferences(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
    scope: RequestScope,
    preferences: MotionSettings,
) -> Result<AppSnapshot, IpcError> {
    request(window, service, scope, Action::Motion(preferences)).await
}
#[tauri::command]
async fn get_display_mode(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
    scope: RequestScope,
) -> Result<AppSnapshot, IpcError> {
    request(window, service, scope, Action::DisplaySnapshot).await
}
#[tauri::command]
async fn get_motion_metrics(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
    scope: RequestScope,
) -> Result<MotionMetrics, IpcError> {
    check_window(&window)?;
    let sender = service.sender.clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<MotionMetrics, IpcError> {
        let (send, receive) = mpsc::channel();
        sender
            .try_send(Message::Metrics(scope, send))
            .map_err(|_| IpcError::from("REQUEST_QUEUE_BUSY"))?;
        let result = receive
            .recv_timeout(Duration::from_secs(5))
            .map_err(|_| IpcError::from("REQUEST_TIMEOUT"))?;
        result.map_err(IpcError::from)
    })
    .await
    .map_err(|_| "REQUEST_WORKER_FAILED")?
}
#[tauri::command]
async fn close_window(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
    scope: RequestScope,
) -> Result<AppSnapshot, IpcError> {
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
) -> Result<AppSnapshot, IpcError> {
    let snapshot = request(window.clone(), service, scope, Action::BeginDrag).await?;
    window.start_dragging().map_err(|e| e.to_string())?;
    Ok(snapshot)
}
#[tauri::command]
async fn maximize_pair(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
    scope: RequestScope,
) -> Result<AppSnapshot, IpcError> {
    request(window, service, scope, Action::MaximizePair).await
}
#[tauri::command]
async fn restore_pair(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
    scope: RequestScope,
) -> Result<AppSnapshot, IpcError> {
    request(window, service, scope, Action::RestorePair).await
}
#[tauri::command]
async fn rediscover(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
    scope: RequestScope,
) -> Result<AppSnapshot, IpcError> {
    request(window, service, scope, Action::Rediscover).await
}
#[tauri::command]
async fn select_candidate(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
    scope: RequestScope,
    candidate_id: String,
) -> Result<AppSnapshot, IpcError> {
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
) -> Result<AppSnapshot, IpcError> {
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
async fn inspect_directory_path(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
    scope: RequestScope,
    path: String,
) -> Result<AppSnapshot, IpcError> {
    check_window(&window)?;
    validate_scope(&service.snapshot()?, &scope)?;
    let directory = absolute_directory_input(&path)?;
    request(window, service, scope, Action::InspectDirectory(directory)).await
}
#[tauri::command]
async fn configure_sdk(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
    scope: RequestScope,
) -> Result<AppSnapshot, IpcError> {
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
) -> Result<AppSnapshot, IpcError> {
    request(window, service, scope, Action::Detach).await
}
#[tauri::command]
async fn set_trust(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
    scope: RequestScope,
    project_id: String,
    trusted: bool,
) -> Result<AppSnapshot, IpcError> {
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
) -> Result<AppSnapshot, IpcError> {
    request(window, service, scope, Action::Refresh).await
}
#[tauri::command]
async fn stop_preview(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Service>,
    scope: RequestScope,
) -> Result<AppSnapshot, IpcError> {
    request(window, service, scope, Action::Stop).await
}

/// Binary frame commands share the same error vocabulary as every other command; the
/// protocol shape additionally carries the expected and received frame protocol version.
fn protocol_error(
    error: impl Into<IpcError>,
    received_protocol_version: u32,
) -> PreviewProtocolError {
    let error = error.into();
    PreviewProtocolError {
        code: crate::ipc::error::wire_name(&error.code),
        message: error.message,
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
    consumer_id: String,
) -> Result<AppSnapshot, PreviewProtocolError> {
    request(
        window,
        service,
        scope,
        Action::BinaryFrames(protocol_version, consumer_id),
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
    consumer_id: String,
) -> Result<AppSnapshot, PreviewProtocolError> {
    request(
        window,
        service,
        scope,
        Action::LegacyFrames(protocol_version, consumer_id),
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
    consumer_id: String,
) -> Result<tauri::ipc::Response, PreviewProtocolError> {
    let result = (|| -> Result<Vec<u8>, IpcError> {
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
            .take_current(&snapshot, &consumer_id)
            .map_err(IpcError::from)
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
) -> Result<AppSnapshot, IpcError> {
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
) -> Result<AppSnapshot, IpcError> {
    if build_id.len() > 256
        || x >= crate::preview::frames::MAX_SURFACE_DIMENSION
        || y >= crate::preview::frames::MAX_SURFACE_DIMENSION
    {
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
            collapse_to_orb,
            expand_from_orb,
            hide_to_tray,
            show_from_tray,
            set_orb_auto_hide,
            set_motion_preferences,
            get_display_mode,
            get_motion_metrics,
            minimize_window,
            close_window,
            maximize_pair,
            restore_pair,
            begin_drag,
            rediscover,
            select_candidate,
            select_directory,
            inspect_directory_path,
            configure_sdk,
            detach,
            set_trust,
            refresh_preview,
            stop_preview,
            arrange_side_by_side,
            click_preview,
            enable_binary_preview_frames,
            disable_binary_preview_frames,
            read_preview_frame,
            native_controls::preview_control_state,
            native_controls::preview_control_enable,
            native_controls::preview_control_submit,
            native_controls::preview_control_cancel,
            native_controls::device_runtime_state,
            native_controls::device_runtime_action,
            inspection::inspector_get_capabilities,
            inspection::inspector_get_tree,
            inspection::selection_create,
            inspection::selection_get,
            inspection::selection_clear,
            inspection::selection_update_instruction,
            inspection::selection_prompt,
            inspection::selection_read_image,
            inspection::selection_export_image,
            agent_delivery::agent_session_list,
            agent_delivery::agent_session_bind,
            agent_delivery::agent_delivery_prepare,
            agent_delivery::agent_delivery_send,
            agent_delivery::agent_delivery_status,
            agent_delivery::agent_pair_create,
            agent_delivery::agent_pair_revoke,
            agent_delivery::agent_registration_bind,
            agent_delivery::agent_assisted_inspect,
            agent_delivery::agent_assisted_fill,
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
                let selection_result = app_data_selection_store(&handle);
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
                    selections: None,
                    agents: crate::agent::AgentRouter::new(),
                    assisted: Arc::new(Mutex::new(
                        crate::agent::assisted::AssistedController::default(),
                    )),
                    assisted_busy: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                    pending_assisted: None,
                    pending_pair: None,
                    assisted_offer_scope: None,
                    inspection: None,
                    inspection_tree: None,
                    inspection_anchor: None,
                    last_inspection_at: 0,
                    last_selection_sync: 0,
                    controls: native_controls::Controls::default(),
                    device_runtime: crate::preview::device_runtime::DeviceRuntimeAdapter::new(None),
                    device_sdk: None,
                    native_visible: None,
                    last_visibility_sync: 0,
                    last_visibility_emit: 0,
                    own_handle,
                    associated_manually: false,
                    next_workspace_probe: Instant::now() + WORKSPACE_PROBE_INTERVAL,
                    next_candidate_recheck_at: 0,
                };
                match selection_result {
                    Ok(store) => engine.selections = Some(store),
                    Err(error) => engine.diagnostic(
                        "SELECTION_STORAGE_UNAVAILABLE",
                        error,
                        CheckStatus::Blocked,
                    ),
                }
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
                        Ok(Message::Metrics(scope, reply)) => {
                            engine.drain_events();
                            let result = validate_scope(&engine.snapshot, &scope).and_then(|()| {
                                engine
                                    .controller
                                    .as_ref()
                                    .ok_or_else(|| "WINDOWING_UNAVAILABLE".to_owned())?
                                    .motion_metrics()
                            });
                            let _ = reply.send(result);
                        }
                        Ok(Message::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                            engine.cancel_assisted();
                            engine.cancel_pairing();
                            break;
                        }
                        Ok(Message::Action(scope, action, reply)) => {
                            engine.cancel_assisted();
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
                        Ok(Message::Data(scope, action, reply)) => {
                            engine.dispatch_data(scope, action, reply)
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                    }
                    engine.drain_events();
                    engine.tick();
                    engine.sync_control_lifecycle();
                    engine.poll_native_controls();
                    engine.poll_inspection();
                    engine.poll_assisted();
                    engine.poll_pair();
                    engine.agents.sync(&engine.snapshot, now());
                    if now().saturating_sub(engine.last_visibility_sync) >= 100 {
                        engine.last_visibility_sync = now();
                        engine.sync_visibility();
                    }
                    if now().saturating_sub(engine.last_selection_sync) >= 2000 {
                        engine.last_selection_sync = now();
                        if let Some(selections) = &mut engine.selections {
                            let _ = selections.cleanup(now());
                            let _ = selections.sync_runtime(&engine.snapshot, now());
                        }
                    }
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
