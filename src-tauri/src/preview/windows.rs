//! Adapter for the exact, locally verified HarmonyOS 26.0.0.105 Windows toolchain.
//! Public OpenHarmony sources informed experiments; actual commercial binary receipts
//! are required by the native gate. No unverified version enters this adapter.
use super::build::{
    build_exit_result, build_receipt, failure_message, preview_build_args, BuildReceipt,
};
use super::control::{
    self, ControlMailbox, ControlReceiver, ControlRequest, RuntimeControlState, WireCommand,
};
use super::dependencies;
use super::frames::{parse_jpeg40_for_surface, BinaryFrame, BinaryFrameMailbox};
use super::NativePoint;
use crate::inspector::{
    InspectionReceiver, InspectionRequest, InspectorCommand, RawInspectionResponse,
    INSPECTION_TIMEOUT_MS,
};
use crate::ipc::model::{
    NativeControlAction, NativeControlPhase, NativeControlRequest, ProjectRef,
};
use crate::scheduler::BuildTask;
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::Read;
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::ptr::null_mut;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc, Mutex,
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use tungstenite::{client::IntoClientRequest, protocol::WebSocketConfig, Message};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::NetworkManagement::IpHelper::*;
use windows_sys::Win32::Networking::WinSock::AF_INET;
use windows_sys::Win32::Security::Authorization::*;
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::Storage::FileSystem::*;
use windows_sys::Win32::System::Pipes::*;

#[path = "windows_control.rs"]
mod control_worker;
#[path = "windows_process.rs"]
mod process;

const FRAME_LIMIT: usize = 1024 * 1024;
/// Build-output policy: a preview build that emits more than this much log is
/// treated as anomalous. The on-disk log itself is bounded independently by the
/// owned capture, so this is a deliberate build policy and not a disk guard.
const BUILD_LOG_LIMIT: u64 = 16 * 1024 * 1024;
/// Diagnostics handed to a failure message, newest last.
const FAILURE_TAIL: usize = 64 * 1024;
const PREVIEWER: &str = "sdk/default/openharmony/previewer/common/bin/Previewer.exe";
const HVIGOR: &str = "hvigor/bin/hvigorw.js";
pub(super) type Completed = (BuildTask, Result<(), String>);

fn exited_build_result(
    build: &process::Process,
    module: &str,
    code: u32,
    stop: &AtomicBool,
) -> Result<(), String> {
    build.drain_output(Duration::from_secs(2), stop)?;
    if build.log_size() > BUILD_LOG_LIMIT {
        return Err("BUILD_LOG_LIMIT".into());
    }
    build_exit_result(module, code, &build.log_tail(FAILURE_TAIL))
}

/// A live pinned watch worker can hand off a confirmed graph. An observably
/// exited child must instead satisfy EOF, final size and final exit status.
fn build_progress(
    build: &process::Process,
    module: &str,
    stop: &AtomicBool,
) -> Result<bool, String> {
    if let Some(code) = build.exit_code()? {
        exited_build_result(build, module, code, stop)?;
        return Ok(true);
    }
    build.check_capture_health()?;
    if build.log_size() > BUILD_LOG_LIMIT {
        return Err("BUILD_LOG_LIMIT".into());
    }
    let log = build.log_tail(FAILURE_TAIL);
    match build_receipt(module, &log) {
        BuildReceipt::Succeeded => {
            // The child may have exited while we sampled the receipt. Do not
            // let that receipt bypass its final status or output barrier.
            build.check_capture_health()?;
            if build.log_size() > BUILD_LOG_LIMIT {
                return Err("BUILD_LOG_LIMIT".into());
            }
            if let Some(code) = build.exit_code()? {
                exited_build_result(build, module, code, stop)?;
            }
            Ok(true)
        }
        BuildReceipt::Failed => Err(failure_message(&log, "PreviewBuild")),
        BuildReceipt::Pending => Ok(false),
    }
}

fn build_environment(sdk: &Path, root: &Path, run_directory: &Path) -> Vec<(String, String)> {
    // Pinned Hvigor supports BUILD_CACHE_DIR, but its wrapper's env.config takes
    // precedence. build_arguments() supplies the matching --config override.
    // A completed graph receipt precedes its final metadata writes. Job cleanup
    // or cancellation therefore may damage only this build's disposable cache,
    // never the project's pre-existing/default .hvigor/cache or the next build.
    vec![
        (
            "DEVECO_SDK_HOME".into(),
            sdk.join("sdk").to_string_lossy().into_owned(),
        ),
        (
            "DEVECO_NODE_HOME".into(),
            sdk.join("tool/node").to_string_lossy().into_owned(),
        ),
        (
            "HVIGOR_USER_HOME".into(),
            root.join(".hvigor/arkdock-home")
                .to_string_lossy()
                .into_owned(),
        ),
        (
            "BUILD_CACHE_DIR".into(),
            run_directory
                .join("hvigor-cache")
                .to_string_lossy()
                .into_owned(),
        ),
    ]
}

/// The canonical project root and the selected module directory, derived from the
/// project's own build profile. Shared by the build and the dependency preflight so
/// the profile is never resolved twice.
fn resolved_module(project: &ProjectRef) -> Result<(PathBuf, PathBuf), String> {
    let root = Path::new(&project.app_root)
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let root = local_path(&root)?;
    let profile_bytes =
        crate::workspace::read_bounded(&root.join("build-profile.json5"), 1024 * 1024)?;
    let profile_text = std::str::from_utf8(&profile_bytes).map_err(|e| e.to_string())?;
    let profile: serde_json::Value = json5::from_str(profile_text).map_err(|e| e.to_string())?;
    let source = profile
        .get("modules")
        .and_then(|v| v.as_array())
        .and_then(|modules| {
            modules
                .iter()
                .find(|module| module.get("name").and_then(|v| v.as_str()) == Some(&project.module))
        })
        .and_then(|module| module.get("srcPath"))
        .and_then(|v| v.as_str())
        .ok_or("MODULE_PATH_MISSING")?;
    let module = root
        .join(source)
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let module = local_path(&module)?;
    if !module.starts_with(&root) {
        return Err("MODULE_OUTSIDE_PROJECT".into());
    }
    Ok((root, module))
}

/// Synchronous dependency preflight for the refresh command, so a project whose
/// declared dependencies are not installed fails before any build is scheduled.
/// A project this preflight cannot resolve is left to the build path, which
/// reports exactly the failure it reports today.
pub(super) fn dependency_preflight(project: &ProjectRef) -> Result<(), String> {
    let Ok((root, module)) = resolved_module(project) else {
        return Ok(());
    };
    dependencies::preflight(&root, &module)
}

fn build_arguments(sdk: &Path, module: &str, run_directory: &Path) -> Result<Vec<String>, String> {
    let cache = run_directory
        .join("hvigor-cache")
        .to_string_lossy()
        .into_owned();
    // The pinned wrapper parses a config property using split('='); reject an
    // unrepresentable path rather than silently falling back to a user's cache.
    if !run_directory.is_absolute() || cache.contains(['=', '\0']) {
        return Err("OWNED_BUILD_CACHE_PATH_UNSUPPORTED".into());
    }
    let mut arguments = vec![sdk.join(HVIGOR).to_string_lossy().into_owned()];
    arguments.extend(preview_build_args(module)?);
    // hvigorw.js serializes --config into env.config before loading the engine.
    // This authoritative setting prevents its {} config from masking the env override.
    arguments.extend(["--config".into(), format!("build-cache-dir={cache}")]);
    Ok(arguments)
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct VerifiedProfile {
    sdk_version: String,
    files: std::collections::BTreeMap<String, String>,
}

struct WorkerContext<'a> {
    stop: &'a AtomicBool,
    frames: &'a Mutex<BinaryFrameMailbox>,
    inputs: &'a mpsc::Receiver<NativePoint>,
    messages: &'a mpsc::SyncSender<String>,
    inspections: &'a mpsc::Receiver<InspectionRequest>,
    controls: &'a Mutex<ControlMailbox>,
    control_state: &'a Mutex<RuntimeControlState>,
}

pub(super) fn verified_root(root: &Path, version: &str) -> Option<PathBuf> {
    let profile: VerifiedProfile =
        serde_json::from_str(include_str!("../../../protocol/native-profile.json")).ok()?;
    if version != profile.sdk_version || profile.files.len() != 3 {
        return None;
    }
    for candidate in std::iter::once(root).chain(root.ancestors().skip(1).take(3)) {
        if candidate.join(PREVIEWER).is_file() {
            if profile.files.iter().all(|(relative, hash)| {
                file_hash(&candidate.join(relative)).as_deref() == Some(hash.as_str())
            }) && candidate.join("tool/node/node.exe").is_file()
            {
                return candidate
                    .canonicalize()
                    .ok()
                    .and_then(|path| local_path(&path).ok());
            }
            return None;
        }
    }
    None
}

fn file_hash(path: &Path) -> Option<String> {
    let mut file = File::open(path).ok()?;
    if file.metadata().ok()?.len() > 16 * 1024 * 1024 {
        return None;
    }
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 8192];
    let mut total = 0;
    loop {
        let count = file.read(&mut buffer).ok()?;
        if count == 0 {
            break;
        }
        total += count;
        if total > 16 * 1024 * 1024 {
            return None;
        }
        hash.update(&buffer[..count]);
    }
    Some(format!("{:x}", hash.finalize()))
}

fn local_path(path: &Path) -> Result<PathBuf, String> {
    let text = path.to_str().ok_or("NATIVE_PATH_ENCODING")?;
    let plain = text.strip_prefix("\\\\?\\").unwrap_or(text);
    let bytes = plain.as_bytes();
    if bytes.len() < 3 || !bytes[0].is_ascii_alphabetic() || bytes[1] != b':' || bytes[2] != b'\\' {
        return Err("UNSUPPORTED_NATIVE_PATH: 需要 Windows 本地盘符路径".into());
    }
    Ok(PathBuf::from(plain))
}

pub(super) struct Runtime {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    inputs: mpsc::SyncSender<NativePoint>,
    inspections: mpsc::SyncSender<InspectionRequest>,
    controls: Arc<Mutex<ControlMailbox>>,
    control_state: Arc<Mutex<RuntimeControlState>>,
    task: BuildTask,
    pub completed: mpsc::Receiver<Completed>,
    pub frames: Arc<Mutex<BinaryFrameMailbox>>,
    pub messages: mpsc::Receiver<String>,
    pub failure: Arc<Mutex<Option<String>>>,
}
impl Runtime {
    pub fn launch(sdk: PathBuf, project: ProjectRef, task: BuildTask) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let frames = Arc::new(Mutex::new(BinaryFrameMailbox::new()));
        let (inputs, input_receiver) = mpsc::sync_channel(16);
        let (inspections, inspection_receiver) = mpsc::sync_channel(2);
        let (completed_tx, completed) = mpsc::sync_channel(2);
        let (message_tx, messages) = mpsc::sync_channel(16);
        let failure = Arc::new(Mutex::new(None));
        let worker_failure = failure.clone();
        let worker_stop = stop.clone();
        let worker_frames = frames.clone();
        let controls = Arc::new(Mutex::new(ControlMailbox::default()));
        let control_state = Arc::new(Mutex::new(RuntimeControlState::default()));
        let worker_controls = controls.clone();
        let worker_control_state = control_state.clone();
        let runtime_task = task.clone();
        let thread = thread::spawn(move || {
            let mut reported = false;
            let outcome = run(
                &sdk,
                &project,
                &task,
                WorkerContext {
                    stop: &worker_stop,
                    frames: &worker_frames,
                    inputs: &input_receiver,
                    messages: &message_tx,
                    inspections: &inspection_receiver,
                    controls: &worker_controls,
                    control_state: &worker_control_state,
                },
                || {
                    let _ = completed_tx.try_send((task.clone(), Ok(())));
                    reported = true;
                },
            );
            if let Err(error) = outcome {
                if !worker_stop.load(Ordering::Acquire) {
                    if reported {
                        if let Ok(mut failure) = worker_failure.lock() {
                            *failure = Some(format!("RUNTIME_FAILED: {error}"));
                        }
                    } else {
                        let _ = completed_tx.try_send((task, Err(error)));
                    }
                }
            }
            if let Ok(mut state) = worker_control_state.lock() {
                state.input_ready = false;
                state.reason = "原生预览已停止，未完成的输入无法继续。".into();
            }
            worker_stop.store(true, Ordering::Release);
            if let Ok(mut mailbox) = worker_controls.lock() {
                mailbox.cancel_all();
                if let Some((_, Some(request))) = mailbox.take_cancel() {
                    request
                        .send(request.receipt(NativeControlPhase::Cancelled, "原生预览已停止。"));
                }
            }
        });
        Self {
            stop,
            thread: Some(thread),
            inputs,
            inspections,
            controls,
            control_state,
            task: runtime_task,
            completed,
            frames,
            messages,
            failure,
        }
    }
    pub fn click(&self, point: NativePoint) -> Result<(), String> {
        // Hold the state through enqueue. A cancellation cannot mark input
        // paused and drain the old queue before this point is actually queued.
        let state = self
            .control_state
            .lock()
            .map_err(|_| "NATIVE_CONTROL_STATE")?;
        if !state.input_ready {
            return Err("NATIVE_INPUT_PAUSED".into());
        }
        if state.width != 360 || state.height != 640 {
            return Err(
                "LEGACY_INPUT_SURFACE_IDENTITY_REQUIRED: 当前画面尺寸需使用原生控制输入。".into(),
            );
        }
        if point.x >= state.width || point.y >= state.height {
            return Err("INPUT_OUTSIDE_SURFACE".into());
        }
        self.inputs
            .try_send(point)
            .map_err(|_| "INPUT_QUEUE_BUSY".into())
    }
    pub fn control(
        &self,
        request: NativeControlRequest,
        operation_id: String,
    ) -> Result<ControlReceiver, String> {
        control::validate_request(&request)?;
        if operation_id.is_empty()
            || operation_id.len() > 128
            || operation_id.chars().any(char::is_control)
        {
            return Err("NATIVE_OPERATION_ID_INVALID".into());
        }
        let state = self.control_runtime_state();
        if self.stop.load(Ordering::Acquire)
            || !control::identity_matches(&request.identity, &self.task, &state)
        {
            return Err("NATIVE_CONTROL_STALE_IDENTITY".into());
        }
        if !state.input_ready
            && !matches!(
                request.action,
                NativeControlAction::PointerCancel | NativeControlAction::CancelAll
            )
        {
            return Err(format!("NATIVE_CONTROL_PAUSED: {}", state.reason));
        }
        let (reply, response) = mpsc::sync_channel(8);
        let queued = ControlRequest {
            request,
            operation_id,
            reply,
            requested_at: super::now_ms(),
            legacy_click: false,
        };
        let cancelling = matches!(
            queued.request.action,
            NativeControlAction::PointerCancel | NativeControlAction::CancelAll
        );
        let mut mailbox = self.controls.lock().map_err(|_| "NATIVE_CONTROL_MAILBOX")?;
        {
            let mut current = self
                .control_state
                .lock()
                .map_err(|_| "NATIVE_CONTROL_STATE")?;
            if !control::identity_matches(&queued.request.identity, &self.task, &current)
                || !cancelling && !current.input_ready
            {
                return Err("NATIVE_CONTROL_CHANGED_BEFORE_ENQUEUE".into());
            }
            if cancelling {
                current.input_ready = false;
                current.reason = "正在安全取消原生输入。".into();
            }
        }
        queued.send(queued.receipt(NativeControlPhase::Queued, "已加入原生命令队列。"));
        mailbox.enqueue(queued)?;
        Ok(response)
    }
    pub fn cancel_controls(&self) {
        if let Ok(mut mailbox) = self.controls.lock() {
            if let Ok(mut state) = self.control_state.lock() {
                state.input_ready = false;
                state.reason = "正在安全取消原生输入。".into();
            }
            mailbox.cancel_all();
        }
    }
    pub fn control_runtime_state(&self) -> RuntimeControlState {
        self.control_state
            .lock()
            .map(|state| state.clone())
            .unwrap_or_else(|_| RuntimeControlState::stopped())
    }
    pub fn request_inspection(
        &self,
        command: InspectorCommand,
    ) -> Result<InspectionReceiver, String> {
        let (reply, response) = mpsc::sync_channel(1);
        self.inspections
            .try_send(InspectionRequest { command, reply })
            .map_err(|_| "INSPECTOR_QUEUE_BUSY".to_string())?;
        Ok(response)
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        self.cancel_controls();
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.thread.take() {
            let _ = worker.join();
        }
    }
}

struct Pipe {
    handle: HANDLE,
    connected: bool,
    buffer: Vec<u8>,
}
impl Pipe {
    fn create(name: &str) -> Result<Self, String> {
        let path: Vec<u16> = format!("\\\\.\\pipe\\{name}")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let sddl: Vec<u16> = "D:P(A;;GA;;;OW)".encode_utf16().chain(Some(0)).collect();
        unsafe {
            let mut descriptor = null_mut();
            if ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                null_mut(),
            ) == 0
            {
                return Err("PIPE_DACL".into());
            }
            let attributes = SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: descriptor,
                bInheritHandle: 0,
            };
            let handle = CreateNamedPipeW(
                path.as_ptr(),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_NOWAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                65_536,
                65_536,
                0,
                &attributes,
            );
            LocalFree(descriptor);
            if handle == INVALID_HANDLE_VALUE {
                return Err(format!("PIPE_CREATE: {}", std::io::Error::last_os_error()));
            }
            Ok(Self {
                handle,
                connected: false,
                buffer: Vec::new(),
            })
        }
    }
    fn receive(&mut self, pid: u32) -> Result<Vec<serde_json::Value>, String> {
        unsafe {
            if !self.connected {
                if ConnectNamedPipe(self.handle, null_mut()) == 0
                    && GetLastError() != ERROR_PIPE_CONNECTED
                {
                    return Ok(Vec::new());
                }
                let mut peer = 0;
                if GetNamedPipeClientProcessId(self.handle, &mut peer) == 0 || peer != pid {
                    return Err("PIPE_PEER_MISMATCH".into());
                }
                self.connected = true;
            }
            // Drain at most 64 KiB per tick. SDK26 Inspector trees are larger
            // than one ReadFile buffer; one read per frame tick added avoidable
            // latency. The bounded drain still yields to the frame receiver.
            let mut bytes = [0u8; 8192];
            for _ in 0..8 {
                let mut count = 0;
                if ReadFile(
                    self.handle,
                    bytes.as_mut_ptr().cast(),
                    bytes.len() as u32,
                    &mut count,
                    null_mut(),
                ) == 0
                {
                    let error = GetLastError();
                    if error == ERROR_NO_DATA {
                        break;
                    }
                    return Err(format!("PIPE_READ: {error}"));
                }
                self.buffer.extend_from_slice(&bytes[..count as usize]);
                if self.buffer.len() > FRAME_LIMIT {
                    return Err("COMMAND_PAYLOAD_LIMIT".into());
                }
                if count == 0 {
                    break;
                }
            }
        }
        if self.buffer.len() > FRAME_LIMIT {
            return Err("COMMAND_PAYLOAD_LIMIT".into());
        }
        let mut results = Vec::new();
        while let Some(end) = self.buffer.iter().position(|&byte| byte == 0) {
            let message = self.buffer.drain(..=end).collect::<Vec<_>>();
            if end > 0 {
                results.push(serde_json::from_slice(&message[..end]).map_err(|_| "COMMAND_JSON")?);
            }
            if results.len() == 32 {
                break;
            }
        }
        Ok(results)
    }
    fn inspector(&self, command: InspectorCommand) -> Result<(), String> {
        let mut bytes = serde_json::to_vec(&serde_json::json!({
            "version": "1.0.1", "type": "action", "command": command.wire_name(), "args": {}
        }))
        .map_err(|e| e.to_string())?;
        bytes.push(0);
        unsafe {
            let mut written = 0;
            if WriteFile(
                self.handle,
                bytes.as_ptr().cast(),
                bytes.len() as u32,
                &mut written,
                null_mut(),
            ) == 0
                || written as usize != bytes.len()
            {
                return Err("INSPECTOR_PIPE_BACKPRESSURE".into());
            }
        }
        Ok(())
    }
    fn control(&self, command: &WireCommand) -> Result<(), String> {
        let bytes = command.bytes()?;
        unsafe {
            let mut written = 0;
            if WriteFile(
                self.handle,
                bytes.as_ptr().cast(),
                bytes.len() as u32,
                &mut written,
                null_mut(),
            ) == 0
                || written as usize != bytes.len()
            {
                return Err("NATIVE_CONTROL_PIPE_BACKPRESSURE".into());
            }
        }
        Ok(())
    }
}
impl Drop for Pipe {
    fn drop(&mut self) {
        unsafe {
            DisconnectNamedPipe(self.handle);
            CloseHandle(self.handle);
        }
    }
}

fn tcp_owner(port: u16, pid: u32) -> bool {
    unsafe {
        let mut size = 0;
        GetExtendedTcpTable(
            null_mut(),
            &mut size,
            0,
            AF_INET as u32,
            TCP_TABLE_OWNER_PID_LISTENER,
            0,
        );
        if size == 0 || size > 4 * 1024 * 1024 {
            return false;
        }
        let mut words = vec![0u32; (size as usize).div_ceil(4)];
        if GetExtendedTcpTable(
            words.as_mut_ptr().cast(),
            &mut size,
            0,
            AF_INET as u32,
            TCP_TABLE_OWNER_PID_LISTENER,
            0,
        ) != 0
        {
            return false;
        }
        let count = words[0] as usize;
        if count > (size as usize).saturating_sub(4) / std::mem::size_of::<MIB_TCPROW_OWNER_PID>() {
            return false;
        }
        let rows =
            std::slice::from_raw_parts(words.as_ptr().add(1).cast::<MIB_TCPROW_OWNER_PID>(), count);
        rows.iter().any(|row| {
            row.dwOwningPid == pid
                && u16::from_be(row.dwLocalPort as u16) == port
                && row.dwLocalAddr == u32::from_ne_bytes([127, 0, 0, 1])
        })
    }
}

fn run(
    sdk: &Path,
    project: &ProjectRef,
    task: &BuildTask,
    context: WorkerContext<'_>,
    mut success: impl FnMut(),
) -> Result<(), String> {
    let WorkerContext {
        stop,
        frames,
        inputs,
        messages,
        inspections,
        controls,
        control_state,
    } = context;
    if project.variant != "default"
        || project.module.is_empty()
        || project.module.len() > 128
        || !project
            .module
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, b'_' | b'-'))
    {
        return Err("UNSUPPORTED_BUILD_TARGET: 仅支持 default 产品/目标与命名 Stage 模块。".into());
    }
    let (root, module) = resolved_module(project)?;
    // Fail closed before hvigor starts: a declared dependency that is not installed
    // can only produce cascading ArkTS compiler errors, never a preview.
    dependencies::preflight(&root, &module)?;
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).map_err(|e| e.to_string())?;
    let nonce = random
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let directory = std::env::temp_dir().join("ArkDock-native").join(&nonce);
    fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    let build_log = directory.join("build.log");
    let runtime_log = directory.join("previewer.log");
    // Wrapper dependencies retain the existing per-workspace home. Task metadata
    // uses a fresh app-owned parent for every build; no user-cache repair/deletion.
    fs::create_dir_all(directory.join("hvigor-cache")).map_err(|e| e.to_string())?;
    let env = build_environment(sdk, &root, &directory);
    let build_args = build_arguments(sdk, &project.module, &directory)?;
    let build_started = Instant::now();
    let build = process::Process::spawn(
        &sdk.join("tool/node/node.exe"),
        &build_args,
        &root,
        &env,
        &build_log,
    )?;
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut build_log_degraded_reported = false;
    loop {
        if stop.load(Ordering::Acquire) {
            return Ok(());
        }
        report_log_degradation(&build, "build", messages, &mut build_log_degraded_reported);
        if build_progress(&build, &project.module, stop)? {
            break;
        }
        if Instant::now() > deadline {
            return Err("BUILD_TIMEOUT: 120s".into());
        }
        thread::sleep(Duration::from_millis(50));
    }
    // A receipt can arrive between the loop's first size check and its tail
    // read. Enforce the hard bound again before accepting that graph result.
    if build.log_size() > BUILD_LOG_LIMIT {
        return Err("BUILD_LOG_LIMIT".into());
    }
    build.check_capture_health()?;
    if let Some(code) = build.exit_code()? {
        exited_build_result(&build, &project.module, code, stop)?;
    }
    report_log_degradation(&build, "build", messages, &mut build_log_degraded_reported);
    // The verified PreviewArkTS task retains an idle watch worker. Its completed graph
    // receipt is the build result; closing only this Job is a separate cleanup action.
    let build_pid = build.pid;
    drop(build);
    let build_ms = build_started.elapsed().as_millis();
    let intermediates = module.join(".preview/default/intermediates");
    let assets = intermediates.join("loader_out/default/ets");
    if fs::metadata(assets.join("modules.abc"))
        .map(|m| m.len())
        .unwrap_or(0)
        == 0
    {
        return Err("BUILD_ARTIFACT_MISSING: modules.abc".into());
    }
    let _ = messages.try_send(format!(
        "BUILD_COMPLETED: actual PreviewBuild graph succeeded; owned watch Job closed; logs {}",
        directory.display()
    ));
    let _ = messages.try_send(format!(
        "BUILD_METRICS: buildMs={build_ms}; buildPid={build_pid}; abcSha256={}",
        file_hash(&assets.join("modules.abc")).ok_or("BUILD_ARTIFACT_HASH")?
    ));
    let name = format!("ArkDock_{nonce}");
    let mut command = Pipe::create(&format!("{name}_commandPipe"))?;
    let mut trace = Pipe::create(&format!("{name}_trace"))?;
    let port_listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).map_err(|e| e.to_string())?;
    let port = port_listener
        .local_addr()
        .map_err(|e| e.to_string())?
        .port();
    drop(port_listener);
    let arg = |path: PathBuf| path.to_string_lossy().into_owned();
    let page_profile = super::configuration::page_profile_name(project)?;
    let args = vec![
        "-j".into(),
        arg(assets),
        "-arp".into(),
        arg(intermediates.join("res/default")),
        "-ljPath".into(),
        arg(intermediates.join("loader/default/loader.json")),
        "-s".into(),
        name.clone(),
        "-ts".into(),
        format!("{name}_trace"),
        "-lws".into(),
        port.to_string(),
        "-sid".into(),
        nonce.clone(),
        "-or".into(),
        "360".into(),
        "640".into(),
        "-cr".into(),
        "360".into(),
        "640".into(),
        "-device".into(),
        "phone".into(),
        "-pm".into(),
        "Stage".into(),
        "-av".into(),
        "ACE_2_0".into(),
        "-url".into(),
        "pages/Index".into(),
        "-pages".into(),
        page_profile,
        "-n".into(),
        project.module.clone(),
        "-sd".into(),
        "160".into(),
    ];
    let previewer_started = Instant::now();
    let previewer = process::Process::spawn(
        &sdk.join(PREVIEWER),
        &args,
        &sdk.join("sdk/default/openharmony/previewer/common/bin"),
        &[],
        &runtime_log,
    )?;
    let mut runtime_log_degraded_reported = false;
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut websocket = None;
    let mut control_worker = control_worker::ControlWorker::new(control_state, controls, task);
    while !stop.load(Ordering::Acquire) {
        report_log_degradation(
            &previewer,
            "previewer",
            messages,
            &mut runtime_log_degraded_reported,
        );
        if let Some(code) = previewer.exit_code()? {
            return Err(format!(
                "PREVIEWER_EXIT: {code}; {}",
                failure_tail(&previewer)
            ));
        }
        previewer.check_capture_health()?;
        for message in command.receive(previewer.pid)? {
            control_worker.route_message(&message);
        }
        trace.receive(previewer.pid)?;
        if command.connected && tcp_owner(port, previewer.pid) {
            let stream = TcpStream::connect_timeout(
                &SocketAddrV4::new(Ipv4Addr::LOCALHOST, port).into(),
                Duration::from_millis(500),
            )
            .map_err(|e| e.to_string())?;
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .map_err(|e| e.to_string())?;
            stream
                .set_write_timeout(Some(Duration::from_secs(2)))
                .map_err(|e| e.to_string())?;
            let mut request = format!("ws://127.0.0.1:{port}/{nonce}")
                .into_client_request()
                .map_err(|e| e.to_string())?;
            request.headers_mut().insert(
                "Sec-WebSocket-Protocol",
                "ws".parse().map_err(|_| "WEBSOCKET_PROTOCOL")?,
            );
            let config = WebSocketConfig::default()
                .max_message_size(Some(FRAME_LIMIT))
                .max_frame_size(Some(FRAME_LIMIT))
                .max_write_buffer_size(FRAME_LIMIT);
            let (mut socket, _) =
                tungstenite::client::client_with_config(request, stream, Some(config))
                    .map_err(|e| e.to_string())?;
            socket
                .get_mut()
                .set_nonblocking(true)
                .map_err(|e| e.to_string())?;
            websocket = Some(socket);
            break;
        }
        if Instant::now() > deadline {
            return Err("PREVIEWER_HANDSHAKE_TIMEOUT: 原生预览器未在限定时间内就绪。".into());
        }
        thread::sleep(Duration::from_millis(20));
    }
    let Some(mut socket) = websocket else {
        return Ok(());
    };
    frames.lock().map_err(|_| "FRAME_MAILBOX")?.set_stream(task);
    let mut first = true;
    let mut sequence = 0;
    let mut inspection: Option<(InspectionRequest, Instant, u64, u64)> = None;
    let mut inspection_poisoned = false;
    let mut query_id = 0u64;
    let mut held_navigation_frame: Option<(BinaryFrame, u64)> = None;
    let mut runtime_log_reported = false;
    let outcome = (|| -> Result<(), String> {
        while !stop.load(Ordering::Acquire) {
            report_log_degradation(
                &previewer,
                "previewer",
                messages,
                &mut runtime_log_degraded_reported,
            );
            if let Some(code) = previewer.exit_code()? {
                return Err(format!("PREVIEWER_EXIT: {code}"));
            }
            previewer.check_capture_health()?;
            // Runtime health comes from the child's status and this protocol. Its log
            // volume is bounded by the owned capture and never fails the run; only the
            // ArkTS page exception the runtime actually reported is fatal.
            if let Some(error) = previewer.runtime_error() {
                return Err(error);
            }
            if !runtime_log_reported && previewer.log_dropped() > 0 {
                runtime_log_reported = true;
                let _ = messages.try_send(format!(
                "LOG_CAPTURE_BOUNDED: previewer output exceeded the capture window; {} older bytes dropped, newest output retained",
                previewer.log_dropped()
            ));
            }
            for message in command.receive(previewer.pid)? {
                if message
                    .get("command")
                    .and_then(|v| v.as_str())
                    .is_some_and(|name| name == "inspector" || name == "inspectorDefault")
                {
                    if inspection.as_ref().is_some_and(|(request, _, _, _)| {
                        message.get("command").and_then(|v| v.as_str())
                            == Some(request.command.wire_name())
                    }) {
                        let (request, _, requested_at, id) =
                            inspection.take().expect("matched pending inspection");
                        let response = message
                            .get("result")
                            .cloned()
                            .ok_or("INSPECTOR_RESULT_MISSING".to_string())
                            .map(|result| RawInspectionResponse {
                                command: request.command,
                                protocol_version: message
                                    .get("version")
                                    .and_then(|value| value.as_str())
                                    .unwrap_or("")
                                    .into(),
                                result,
                                frame_sequence: sequence,
                                requested_at,
                                received_at: super::now_ms(),
                                query_id: id,
                                task: task.clone(),
                            });
                        let _ = request.reply.try_send(response);
                    }
                    continue;
                }
                // Additional commands fail their own receipts. Only an ambiguous
                // acknowledgement/release forces the owned runtime to stop.
                control_worker.receive(&message)?;
            }
            trace.receive(previewer.pid)?;
            if inspection.as_ref().is_some_and(|(_, sent, _, _)| {
                sent.elapsed() >= Duration::from_millis(INSPECTION_TIMEOUT_MS)
            }) {
                if let Some((request, _, _, _)) = inspection.take() {
                    let _ = request.reply.try_send(Err(
                        "INSPECTOR_TIMEOUT: response correlation unavailable until runtime restart"
                            .into(),
                    ));
                }
                // The wire has no request IDs. Never let a late response satisfy another query.
                inspection_poisoned = true;
            }
            if !first {
                control_worker.tick(&command, inspection.is_some(), inputs)?;
            }
            if let Some(revision) = held_navigation_frame
                .as_ref()
                .map(|(_, revision)| *revision)
            {
                if control_worker.navigation_revision() != Some(revision) {
                    held_navigation_frame = None;
                } else if control_worker.navigation_frame_ready(revision) {
                    if let Some((frame, _)) = held_navigation_frame.take() {
                        if control_worker.frame(
                            frame.metadata.width,
                            frame.metadata.height,
                            frame.metadata.sequence,
                            &frame.payload,
                        )? {
                            frames
                                .lock()
                                .map_err(|_| "FRAME_MAILBOX")?
                                .push(frame)
                                .map_err(|error| format!("FRAME_REJECTED: {error:?}"))?;
                        }
                    }
                }
            }
            if !first && inspection.is_none() && !control_worker.busy() {
                if let Ok(request) = inspections.try_recv() {
                    if inspection_poisoned {
                        let _ = request
                            .reply
                            .try_send(Err("INSPECTOR_CHANNEL_UNVERIFIED_AFTER_TIMEOUT".into()));
                    } else {
                        match command.inspector(request.command) {
                            Ok(()) => {
                                query_id = query_id.saturating_add(1);
                                inspection =
                                    Some((request, Instant::now(), super::now_ms(), query_id));
                            }
                            Err(error) => {
                                let _ = request.reply.try_send(Err(error.clone()));
                                return Err(format!("NATIVE_COMMAND_PIPE_PARTIAL_WRITE: {error}"));
                            }
                        }
                    }
                }
            }
            match socket.read() {
                Ok(Message::Binary(data)) => {
                    sequence += 1;
                    let (width, height) = control_worker.surface();
                    // During native rotation/resizing, old-dimension frames can be
                    // queued on the socket. Discard them without relabeling pixels.
                    if data.len() >= 40
                        && data.len() <= FRAME_LIMIT
                        && data[..4] == [0x12, 0x34, 0x56, 0x78]
                        && data[12..20] == data[4..12]
                        && data[20..40].iter().all(|byte| *byte == 0)
                    {
                        let actual_width =
                            u32::from_be_bytes(data[4..8].try_into().map_err(|_| "FRAME_WIDTH")?);
                        let actual_height =
                            u32::from_be_bytes(data[8..12].try_into().map_err(|_| "FRAME_HEIGHT")?);
                        if (actual_width != width || actual_height != height)
                            && control_worker.old_surface_packet(actual_width, actual_height)
                        {
                            continue;
                        }
                    }
                    let frame = parse_jpeg40_for_surface(
                        &data,
                        task,
                        sequence,
                        super::now_ms(),
                        width,
                        height,
                    )?;
                    if !control_worker.frame(width, height, sequence, &frame.payload)? {
                        if let Some(revision) = control_worker.navigation_revision() {
                            // One bounded candidate from the already observed target
                            // route may precede its command ACK on the other stream.
                            // Preserve its original identity/sequence/time unchanged.
                            held_navigation_frame = Some((frame, revision));
                        }
                        continue;
                    }
                    held_navigation_frame = None;
                    frames
                        .lock()
                        .map_err(|_| "FRAME_MAILBOX")?
                        .push(frame)
                        .map_err(|e| format!("FRAME_REJECTED: {e:?}"))?;
                    if first {
                        first = false;
                        let _ = messages.try_send(format!(
                        "PREVIEW_METRICS: previewerPid={}; launchToFirstFrameMs={}; jpegBytes={}",
                        previewer.pid,
                        previewer_started.elapsed().as_millis(),
                        data.len().saturating_sub(40)
                    ));
                        success();
                    }
                }
                Ok(Message::Close(_)) => return Err("NATIVE_SURFACE_CLOSED".into()),
                Ok(_) => {}
                Err(tungstenite::Error::Io(error))
                    if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(format!("NATIVE_TRANSPORT: {error}")),
            }
            if first && Instant::now() > deadline {
                return Err("NATIVE_FRAME_TIMEOUT: 15s".into());
            }
            thread::sleep(Duration::from_millis(16));
        }
        Ok(())
    })();
    // The process Job drops immediately after this best-effort release. Even a
    // failed pipe write cannot leave a live owned Previewer in pressed state.
    control_worker.cleanup(&command);
    outcome
}

/// Diagnostics for a failed owned process: its newest output, plus an explicit
/// note when the bounded capture dropped older output or lost its log file, so a
/// truncated log is never mistaken for the whole story.
fn report_log_degradation(
    process: &process::Process,
    name: &str,
    messages: &mpsc::SyncSender<String>,
    reported: &mut bool,
) {
    if !*reported {
        if let Some(error) = process.log_error() {
            *reported = true;
            let _ = messages.try_send(format!(
                "LOG_CAPTURE_DEGRADED: {name} disk capture unavailable; newest output stays in memory ({error})"
            ));
        }
    }
}

fn failure_tail(process: &process::Process) -> String {
    let mut tail: String = process.log_tail(FAILURE_TAIL).chars().take(4096).collect();
    if process.log_dropped() > 0 {
        tail.push_str(&format!(
            "\n[ArkDock] bounded capture dropped {} older log bytes",
            process.log_dropped()
        ));
    }
    if let Some(error) = process.log_error() {
        tail.push_str(&format!("\n[ArkDock] log file capture degraded: {error}"));
    }
    tail
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preview::frames::parse_jpeg40;
    #[test]
    fn each_build_has_private_structured_cache_environment_without_project_cache_repair() {
        let sdk = Path::new("C:/SDK space & literal");
        let root = Path::new("D:/user project & literal");
        let first_run = Path::new("C:/Temp/ArkDock-native/first-owned-run");
        let next_run = Path::new("C:/Temp/ArkDock-native/next-owned-run");
        let first = build_environment(sdk, root, first_run);
        let next = build_environment(sdk, root, next_run);
        let cache = |environment: &[(String, String)]| {
            PathBuf::from(
                &environment
                    .iter()
                    .find(|(name, _)| name == "BUILD_CACHE_DIR")
                    .unwrap()
                    .1,
            )
        };
        assert!(cache(&first).starts_with(first_run));
        assert!(cache(&next).starts_with(next_run));
        assert_ne!(cache(&first), cache(&next));
        assert!(!cache(&first).starts_with(root));
        assert_eq!(
            first
                .iter()
                .find(|(name, _)| name == "HVIGOR_USER_HOME")
                .unwrap()
                .1,
            root.join(".hvigor/arkdock-home").to_string_lossy()
        );
        // SDK/root paths with shell metacharacters are literal environment values;
        // they are neither shell fragments nor additional executable arguments.
        assert_eq!(
            first
                .iter()
                .find(|(name, _)| name == "DEVECO_SDK_HOME")
                .unwrap()
                .1,
            sdk.join("sdk").to_string_lossy()
        );
        assert_eq!(first.len(), 4);
        let arguments = build_arguments(sdk, "entry", first_run).unwrap();
        assert_eq!(
            &arguments[arguments.len() - 2..],
            &[
                "--config".to_owned(),
                format!("build-cache-dir={}", cache(&first).display())
            ]
        );
        assert!(build_arguments(sdk, "entry", Path::new("C:/Temp/unsupported=path")).is_err());
        assert!(build_arguments(sdk, "entry", Path::new("relative-run")).is_err());
    }

    #[test]
    #[ignore = "Requires the explicitly supplied licensed pinned Windows SDK and owned native-counter fixture"]
    fn real_sdk_build_uses_own_cache_while_malformed_default_metadata_is_untouched() {
        let sdk_input = std::env::var("ARKDOCK_SDK").expect("Set ARKDOCK_SDK explicitly");
        let sdk = verified_root(Path::new(&sdk_input), "26.0.0.105")
            .expect("Pinned SDK fingerprint required");
        let sdk = local_path(&sdk).unwrap();
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("examples/native-counter")
            .canonicalize()
            .unwrap();
        let root = local_path(&root).unwrap();
        let default_meta = root.join(".hvigor/cache/meta.json");
        let previous_default = fs::read(&default_meta).ok();
        let previous_written = fs::metadata(&default_meta)
            .ok()
            .and_then(|metadata| metadata.modified().ok());
        let directory = tempfile::Builder::new()
            .prefix("ArkDock-sdk-cache-proof-")
            .tempdir()
            .unwrap()
            .keep();
        fs::create_dir_all(directory.join("hvigor-cache")).unwrap();
        let log = directory.join("build.log");
        let build = process::Process::spawn(
            &sdk.join("tool/node/node.exe"),
            &build_arguments(&sdk, "entry", &directory).unwrap(),
            &root,
            &build_environment(&sdk, &root, &directory),
            &log,
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            let receipt = build_receipt("entry", &build.log_tail(FAILURE_TAIL));
            assert_ne!(
                receipt,
                BuildReceipt::Failed,
                "Real SDK failed: {}",
                failure_tail(&build)
            );
            if receipt == BuildReceipt::Succeeded {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "Real SDK build timed out: {}",
                failure_tail(&build)
            );
            if let Some(code) = build.exit_code().unwrap() {
                build
                    .drain_output(Duration::from_secs(2), &AtomicBool::new(false))
                    .unwrap();
                build_exit_result("entry", code, &build.log_tail(FAILURE_TAIL)).unwrap();
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
        let owned_meta = directory
            .join("hvigor-cache")
            .join(root.file_name().unwrap())
            .join(".hvigor/cache/meta.json");
        let metadata_deadline = Instant::now() + Duration::from_secs(3);
        let owned_json = loop {
            if let Ok(bytes) = fs::read(&owned_meta) {
                if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                    break json;
                }
            }
            assert!(
                Instant::now() < metadata_deadline,
                "SDK did not write valid metadata in own cache {}",
                owned_meta.display()
            );
            thread::sleep(Duration::from_millis(50));
        };
        assert_eq!(
            owned_json
                .get("hvigorVersion")
                .and_then(|value| value.as_str()),
            Some("6.26.8")
        );
        assert_eq!(fs::read(&default_meta).ok(), previous_default);
        assert_eq!(
            fs::metadata(&default_meta)
                .ok()
                .and_then(|metadata| metadata.modified().ok()),
            previous_written
        );
        println!("SDK_CACHE_PROOF: receipt=Succeeded; ownedMetadata={}; defaultMetadataUnchanged=true; log={}", owned_meta.display(), log.display());
        drop(build);
    }
    #[test]
    fn malformed_or_unverified_native_frames_fail_closed() {
        let task = BuildTask {
            epoch: 1,
            workspace_id: "p".into(),
            session_id: "s".into(),
            generation: 1,
            build_id: "b".into(),
            started_at: 0,
        };
        assert!(parse_jpeg40(&[], &task, 1, 0).is_err());
        let mut packet = vec![0u8; 44];
        packet[..4].copy_from_slice(&[0x12, 0x34, 0x56, 0x78]);
        assert_eq!(
            parse_jpeg40(&packet, &task, 1, 0).unwrap_err(),
            "FRAME_PROFILE_MISMATCH"
        );
        assert!(verified_root(Path::new("C:/missing"), "26.0.0.999").is_none());
    }
}
