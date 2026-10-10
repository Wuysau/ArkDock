//! Adapter for the exact, locally verified HarmonyOS 26.0.0.105 Windows toolchain.
//! Public OpenHarmony sources informed experiments; actual commercial binary receipts
//! are required by the native gate. No unverified version enters this adapter.
use super::build::{build_exit_result, build_receipt, preview_build_args, BuildReceipt};
use super::frames::{parse_jpeg40, BinaryFrameMailbox};
use super::NativePoint;
use crate::inspector::{
    InspectionReceiver, InspectionRequest, InspectorCommand, RawInspectionResponse,
    INSPECTION_TIMEOUT_MS,
};
use crate::ipc::model::ProjectRef;
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

#[path = "windows_process.rs"]
mod process;

const FRAME_LIMIT: usize = 1024 * 1024;
const LOG_LIMIT: u64 = 16 * 1024 * 1024;
const PREVIEWER: &str = "sdk/default/openharmony/previewer/common/bin/Previewer.exe";
const HVIGOR: &str = "hvigor/bin/hvigorw.js";
pub(super) type Completed = (BuildTask, Result<(), String>);

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
        return Err(
            "UNSUPPORTED_NATIVE_PATH: verified profile requires a Windows-local drive".into(),
        );
    }
    Ok(PathBuf::from(plain))
}

pub(super) struct Runtime {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    inputs: mpsc::SyncSender<NativePoint>,
    inspections: mpsc::SyncSender<InspectionRequest>,
    pub completed: mpsc::Receiver<Completed>,
    pub frames: Arc<Mutex<BinaryFrameMailbox>>,
    pub messages: mpsc::Receiver<String>,
}
impl Runtime {
    pub fn launch(sdk: PathBuf, project: ProjectRef, task: BuildTask) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let frames = Arc::new(Mutex::new(BinaryFrameMailbox::new()));
        let (inputs, input_receiver) = mpsc::sync_channel(16);
        let (inspections, inspection_receiver) = mpsc::sync_channel(2);
        let (completed_tx, completed) = mpsc::sync_channel(2);
        let (message_tx, messages) = mpsc::sync_channel(16);
        let worker_stop = stop.clone();
        let worker_frames = frames.clone();
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
                },
                || {
                    let _ = completed_tx.try_send((task.clone(), Ok(())));
                    reported = true;
                },
            );
            if let Err(error) = outcome {
                if !worker_stop.load(Ordering::Acquire) {
                    if reported {
                        let _ = message_tx.try_send(format!("RUNTIME_FAILED: {error}"));
                    } else {
                        let _ = completed_tx.try_send((task, Err(error)));
                    }
                }
            }
        });
        Self {
            stop,
            thread: Some(thread),
            inputs,
            inspections,
            completed,
            frames,
            messages,
        }
    }
    pub fn click(&self, point: NativePoint) -> Result<(), String> {
        if point.x >= 360 || point.y >= 640 {
            return Err("INPUT_OUTSIDE_SURFACE".into());
        }
        self.inputs
            .try_send(point)
            .map_err(|_| "INPUT_QUEUE_BUSY".into())
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
    fn mouse(&self, command: &str, point: NativePoint) -> Result<(), String> {
        let mut bytes = serde_json::to_vec(&serde_json::json!({ "version": "1.0.1", "type": "action", "command": command, "args": { "x": point.x, "y": point.y } })).map_err(|e| e.to_string())?;
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
                return Err("INPUT_PIPE_BACKPRESSURE".into());
            }
        }
        Ok(())
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
    } = context;
    if project.variant != "default"
        || project.module.is_empty()
        || project.module.len() > 128
        || !project
            .module
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, b'_' | b'-'))
    {
        return Err("UNSUPPORTED_BUILD_TARGET: verified profile supports default product/target and named Stage modules".into());
    }
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
    // Each workspace owns its wrapper cache; parallel worktrees never share its daemon state.
    let home = root.join(".hvigor/arkdock-home");
    let env = vec![
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
            home.to_string_lossy().into_owned(),
        ),
    ];
    let mut build_args = vec![sdk.join(HVIGOR).to_string_lossy().into_owned()];
    build_args.extend(preview_build_args(&project.module)?);
    let build_started = Instant::now();
    let build = process::Process::spawn(
        &sdk.join("tool/node/node.exe"),
        &build_args,
        &root,
        &env,
        &build_log,
    )?;
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        if stop.load(Ordering::Acquire) {
            return Ok(());
        }
        if build.log_size() > LOG_LIMIT {
            return Err("BUILD_LOG_LIMIT".into());
        }
        let log = process::bounded_log(&build_log);
        match build_receipt(&project.module, &log) {
            BuildReceipt::Succeeded => break,
            BuildReceipt::Failed => {
                return Err(format!(
                    "BUILD_FAILED: PreviewBuild; {}",
                    log.chars()
                        .rev()
                        .take(4096)
                        .collect::<String>()
                        .chars()
                        .rev()
                        .collect::<String>()
                ));
            }
            BuildReceipt::Pending => {}
        }
        if let Some(code) = build.exit_code()? {
            // Cached PreviewArkTS may exit naturally. The graph can finish between
            // our previous log read and this exit observation; re-read after the barrier.
            build_exit_result(&project.module, code, &process::bounded_log(&build_log))?;
            break;
        }
        if Instant::now() > deadline {
            return Err("BUILD_TIMEOUT: 120s".into());
        }
        thread::sleep(Duration::from_millis(50));
    }
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
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut websocket = None;
    while !stop.load(Ordering::Acquire) {
        if let Some(code) = previewer.exit_code()? {
            return Err(format!(
                "PREVIEWER_EXIT: {code}; {}",
                process::bounded_log(&runtime_log)
                    .chars()
                    .take(4096)
                    .collect::<String>()
            ));
        }
        command.receive(previewer.pid)?;
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
            return Err(
                "PREVIEWER_HANDSHAKE_TIMEOUT: private pipe and owned loopback listener required"
                    .into(),
            );
        }
        thread::sleep(Duration::from_millis(20));
    }
    let Some(mut socket) = websocket else {
        return Ok(());
    };
    frames.lock().map_err(|_| "FRAME_MAILBOX")?.set_stream(task);
    let mut first = true;
    let mut sequence = 0;
    let mut release = None;
    let mut inspection: Option<(InspectionRequest, Instant, u64, u64)> = None;
    let mut inspection_poisoned = false;
    let mut query_id = 0u64;
    while !stop.load(Ordering::Acquire) {
        if let Some(code) = previewer.exit_code()? {
            return Err(format!("PREVIEWER_EXIT: {code}"));
        }
        if previewer.log_size() > LOG_LIMIT {
            return Err("PREVIEWER_LOG_LIMIT".into());
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
            if message.get("result") == Some(&serde_json::Value::Bool(false)) {
                return Err("NATIVE_COMMAND_REJECTED".into());
            }
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
        if !first && inspection.is_none() {
            if let Ok(request) = inspections.try_recv() {
                if inspection_poisoned {
                    let _ = request
                        .reply
                        .try_send(Err("INSPECTOR_CHANNEL_UNVERIFIED_AFTER_TIMEOUT".into()));
                } else {
                    match command.inspector(request.command) {
                        Ok(()) => {
                            query_id = query_id.saturating_add(1);
                            inspection = Some((request, Instant::now(), super::now_ms(), query_id));
                        }
                        Err(error) => {
                            let _ = request.reply.try_send(Err(error));
                        }
                    }
                }
            }
        }
        if let Some((point, due)) = release {
            if Instant::now() >= due {
                command.mouse("MouseRelease", point)?;
                release = None;
            }
        } else if !first {
            if let Ok(point) = inputs.try_recv() {
                command.mouse("MousePress", point)?;
                release = Some((point, Instant::now() + Duration::from_millis(80)));
            }
        }
        match socket.read() {
            Ok(Message::Binary(data)) => {
                sequence += 1;
                let frame = parse_jpeg40(&data, task, sequence, super::now_ms())?;
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
}

#[cfg(test)]
mod tests {
    use super::*;
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
