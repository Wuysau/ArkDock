//! Project-scoped device runtime. Tools run on one cancellable worker, never Engine.
mod deployment;
pub(crate) mod emulator;
mod frame;
pub(crate) mod hdc;
mod plans;
pub(crate) mod process;
mod session;
pub use plans::{command_plan, parse_target_stdout, HdcCommandPlan, HdcOperation};

use crate::ipc::model::*;
use sha2::{Digest, Sha256};
use std::fs::{self, File, Metadata};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc, Mutex,
};

const TOOL_SIZE_LIMIT: u64 = 32 * 1024 * 1024;
const HDC_CANDIDATES: [&str; 5] = [
    "hdc.exe",
    "toolchains/hdc.exe",
    "openharmony/toolchains/hdc.exe",
    "default/openharmony/toolchains/hdc.exe",
    "sdk/default/openharmony/toolchains/hdc.exe",
];
#[derive(Clone)]
struct ToolObservation {
    path: Option<String>,
    sha256: Option<String>,
    reason: String,
}

pub(crate) struct DeviceWork {
    request: DeviceRuntimeRequest,
    cancel: Arc<AtomicBool>,
    operation_id: u64,
}
pub(crate) struct DeviceContext {
    project: ProjectRef,
    sdk: PathBuf,
    hdc: PathBuf,
    state: Arc<Mutex<DeviceRuntimeState>>,
    frame: Arc<Mutex<Option<frame::DeviceFrame>>>,
    shutdown: Arc<AtomicBool>,
}

pub struct DeviceRuntimeAdapter {
    tool: ToolObservation,
    sdk: Option<PathBuf>,
    project: Option<ProjectRef>,
    state: Arc<Mutex<DeviceRuntimeState>>,
    frame: Arc<Mutex<Option<frame::DeviceFrame>>>,
    sender: Option<mpsc::SyncSender<DeviceWork>>,
    shutdown: Arc<AtomicBool>,
    active_cancel: Option<Arc<AtomicBool>>,
}

impl DeviceRuntimeAdapter {
    pub fn new(sdk: Option<PathBuf>) -> Self {
        let tool = observe_tool(sdk.as_deref());
        Self {
            state: Arc::new(Mutex::new(empty_state(&tool))),
            tool,
            sdk,
            project: None,
            frame: Arc::new(Mutex::new(None)),
            sender: None,
            shutdown: Arc::new(AtomicBool::new(false)),
            active_cancel: None,
        }
    }

    /// Project/trust lifecycle only; Previewer rebuilds never change device identity.
    pub fn state(&mut self, project: Option<ProjectRef>) -> DeviceRuntimeState {
        if self.project != project {
            self.cancel();
            self.project = project;
            let mut state = empty_state(&self.tool);
            if let Some(project) = &self.project {
                state.identity = Some(DeviceRunIdentity {
                    run_id: fresh_id(),
                    workspace_id: project.id.clone(),
                    target_id: None,
                    generation: 1,
                });
                state.phase = "idle".into();
                state.reason = "选择运行目标；设备运行不依赖 Previewer 构建或画面。".into();
            }
            self.state = Arc::new(Mutex::new(state));
            self.frame = Arc::new(Mutex::new(None));
        }
        self.snapshot()
    }

    pub fn snapshot(&self) -> DeviceRuntimeState {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn start_worker(&mut self) -> Result<(), String> {
        if self.sender.is_some() {
            return Ok(());
        }
        let sdk = self.sdk.clone().ok_or("HDC_MISSING: 请选择本机 SDK。")?;
        let hdc = PathBuf::from(self.tool.path.as_ref().ok_or(self.tool.reason.as_str())?);
        let project = self.project.clone().ok_or("PROJECT_REQUIRED")?;
        self.shutdown = Arc::new(AtomicBool::new(false));
        let context = DeviceContext {
            project,
            sdk,
            hdc,
            state: self.state.clone(),
            frame: self.frame.clone(),
            shutdown: self.shutdown.clone(),
        };
        let (sender, receiver) = mpsc::sync_channel(8);
        std::thread::Builder::new()
            .name("arkdock-device".into())
            .spawn(move || session::run(context, receiver))
            .map_err(|_| "DEVICE_WORKER_UNAVAILABLE")?;
        self.sender = Some(sender);
        Ok(())
    }

    pub fn action(&mut self, request: DeviceRuntimeRequest) -> Result<DeviceRuntimeState, String> {
        let snapshot = self.snapshot();
        if snapshot.identity.as_ref() != Some(&request.identity) {
            return Err("STALE_DEVICE_RUNTIME_IDENTITY".into());
        }
        if let Some(target) = &request.target_id {
            plans::validate_target_key(target)?;
        }
        if matches!(
            request.action,
            DeviceRuntimeAction::Cancel | DeviceRuntimeAction::Disconnect
        ) {
            self.cancel();
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(identity) = &mut state.identity {
                identity.generation += 1;
                identity.target_id = None;
            }
            state.targets.clear();
            state.selected_target_id = None;
            state.busy = false;
            state.streaming = false;
            state.frame = None;
            state.frame_ready = false;
            state.foreground_verified = false;
            state.deployment_token = None;
            state.phase = "disconnected".into();
            state.reason = "已断开本工具的设备会话。".into();
            return Ok(state.clone());
        }
        if !request.connection_consent {
            return Err("DEVICE_CONNECTION_CONSENT_REQUIRED".into());
        }
        if matches!(
            request.action,
            DeviceRuntimeAction::Capture | DeviceRuntimeAction::StreamStart
        ) && !request.capture_consent
        {
            return Err("DEVICE_CAPTURE_CONSENT_REQUIRED".into());
        }
        if request.action == DeviceRuntimeAction::Input && !request.input_consent {
            return Err("DEVICE_INPUT_CONSENT_REQUIRED".into());
        }
        if snapshot.busy
            && !matches!(
                request.action,
                DeviceRuntimeAction::Stop | DeviceRuntimeAction::StreamStop
            )
        {
            return Err("DEVICE_OPERATION_BUSY".into());
        }
        if let Some(cancel) = self.active_cancel.take() {
            cancel.store(true, Ordering::Release);
        }
        self.start_worker()?;
        let cancel = Arc::new(AtomicBool::new(false));
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.operation_id = state
            .operation_id
            .checked_add(1)
            .ok_or("DEVICE_OPERATION_LIMIT")?;
        state.busy = true;
        state.reason = "正在执行设备操作…".into();
        let operation_id = state.operation_id;
        if let Err(error) = self.sender.as_ref().unwrap().try_send(DeviceWork {
            request,
            cancel: cancel.clone(),
            operation_id,
        }) {
            state.busy = false;
            return Err(format!("DEVICE_QUEUE_BUSY: {error}"));
        }
        self.active_cancel = Some(cancel);
        Ok(state.clone())
    }

    pub fn read_frame(&self, identity: &DeviceRunIdentity, after: u64) -> Result<Vec<u8>, String> {
        // Keep the state epoch pinned until the matching packet is copied.
        // Publishers use the same state -> frame lock order.
        let state = self.state.lock().map_err(|_| "DEVICE_STATE_UNAVAILABLE")?;
        if state.identity.as_ref() != Some(identity) {
            return Err("STALE_DEVICE_RUNTIME_IDENTITY".into());
        }
        let frames = self.frame.lock().map_err(|_| "DEVICE_FRAME_UNAVAILABLE")?;
        let frame = frames.as_ref().filter(|frame| {
            state.frame_ready
                && state.frame.as_ref() == Some(&frame.metadata)
                && frame.metadata.run_id == identity.run_id
                && frame.metadata.sequence > after
                && Some(&frame.metadata.target_id) == identity.target_id.as_ref()
        });
        frame
            .map(frame::DeviceFrame::encode)
            .transpose()
            .map(|v| v.unwrap_or_default())
    }

    pub fn cancel(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        if let Some(cancel) = self.active_cancel.take() {
            cancel.store(true, Ordering::Release);
        }
        self.sender.take();
        if let Ok(mut frame) = self.frame.lock() {
            *frame = None;
        }
    }
}
impl Drop for DeviceRuntimeAdapter {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn fresh_id() -> String {
    #[cfg(windows)]
    {
        let mut bytes = [0u8; 16];
        if getrandom::fill(&mut bytes).is_ok() {
            return bytes.iter().map(|b| format!("{b:02x}")).collect();
        }
    }
    static SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    format!(
        "{}-{}-{}",
        std::process::id(),
        super::now_ms(),
        SERIAL.fetch_add(1, Ordering::Relaxed)
    )
}
fn empty_state(tool: &ToolObservation) -> DeviceRuntimeState {
    DeviceRuntimeState {
        identity: None,
        kind: DeviceRuntimeKind::Unknown,
        hdc_path: tool.path.clone(),
        tool_sha256: tool.sha256.clone(),
        hdc_present: tool.path.is_some(),
        targets: Vec::new(),
        selected_target_id: None,
        phase: "idle".into(),
        deployment_ready: false,
        signature_status: "notVerified".into(),
        foreground_verified: false,
        frame_ready: false,
        reason: tool.reason.clone(),
        busy: false,
        operation_id: 0,
        bundle_name: None,
        module_name: None,
        ability_name: None,
        artifact_sha256: None,
        deployment_token: None,
        frame: None,
        streaming: false,
        emulators: Vec::new(),
        selected_emulator: None,
    }
}

fn observe_tool(sdk: Option<&Path>) -> ToolObservation {
    let observed = (|| -> Result<(String, String), String> {
        let sdk = sdk.ok_or("HDC_MISSING: 尚未选择本地 SDK。")?;
        if !sdk.is_absolute() {
            return Err("HDC_SDK_ROOT_NOT_ABSOLUTE".into());
        }
        reject_reparse_ancestors(sdk)?;
        let root = sdk
            .canonicalize()
            .map_err(|error| format!("HDC_SDK_INACCESSIBLE: SDK 目录无法访问（{error}）"))?;
        if !root.is_dir() {
            return Err("HDC_SDK_NOT_DIRECTORY".into());
        }
        let mut candidates = Vec::new();
        for relative in HDC_CANDIDATES {
            let path = root.join(relative);
            match fs::symlink_metadata(&path) {
                Ok(_) => {
                    reject_reparse_ancestors(&path)?;
                    let path = path.canonicalize().map_err(|error| {
                        format!("HDC_FILE_INACCESSIBLE: SDK 文件无法访问（{error}）")
                    })?;
                    if !path.starts_with(&root) {
                        return Err("HDC_FILE_OUTSIDE_SDK".into());
                    }
                    if !candidates.contains(&path) {
                        candidates.push(path);
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(format!(
                        "HDC_FILE_INACCESSIBLE: SDK 文件无法访问（{error}）"
                    ))
                }
            }
        }
        if candidates.len() > 1 {
            return Err(
                "HDC_TOOL_AMBIGUOUS: 找到多个 hdc.exe，请指定具体的 SDK 工具链目录。".into(),
            );
        }
        let path = candidates
            .first()
            .ok_or("HDC_MISSING: 在 SDK 工具链目录中未找到 hdc.exe。")?;
        let mut file = File::open(path)
            .map_err(|error| format!("HDC_FILE_OPEN: 无法打开 SDK 文件（{error}）"))?;
        let metadata = file
            .metadata()
            .map_err(|error| format!("HDC_FILE_METADATA: 无法读取 SDK 文件信息（{error}）"))?;
        if !metadata.is_file()
            || is_reparse(&metadata)
            || metadata.len() < 2
            || metadata.len() > TOOL_SIZE_LIMIT
        {
            return Err("HDC_FILE_REJECTED: 需要常规的可执行文件。".into());
        }
        let mut header = [0u8; 2];
        file.read_exact(&mut header)
            .map_err(|error| format!("HDC_FILE_HEADER: 无法读取文件头（{error}）"))?;
        if header != *b"MZ" {
            return Err("HDC_FILE_NOT_WINDOWS_EXECUTABLE".into());
        }
        let mut hash = Sha256::new();
        hash.update(header);
        let mut total = 2u64;
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let read = file
                .read(&mut buffer)
                .map_err(|error| format!("HDC_FILE_READ: 读取 SDK 文件失败（{error}）"))?;
            if read == 0 {
                break;
            }
            total += read as u64;
            if total > TOOL_SIZE_LIMIT {
                return Err("HDC_FILE_LIMIT_EXCEEDED".into());
            }
            hash.update(&buffer[..read]);
        }
        if total != metadata.len() {
            return Err("HDC_FILE_CHANGED_DURING_OBSERVATION".into());
        }
        let path = path
            .to_str()
            .ok_or("HDC_PATH_ENCODING_UNSUPPORTED")?
            .to_owned();
        Ok((path, format!("{:x}", hash.finalize())))
    })();
    match observed {
        Ok((path, sha256)) => ToolObservation {
            path: Some(path),
            sha256: Some(sha256),
            reason: "已找到 HDC 工具；连接后可发现设备、构建部署并查看运行画面。".into(),
        },
        Err(reason) => ToolObservation {
            path: None,
            sha256: None,
            reason,
        },
    }
}

fn reject_reparse_ancestors(path: &Path) -> Result<(), String> {
    for ancestor in path.ancestors() {
        if ancestor.as_os_str().is_empty() {
            continue;
        }
        let metadata = fs::symlink_metadata(ancestor)
            .map_err(|error| format!("HDC_PATH_INACCESSIBLE: 路径无法访问（{error}）"))?;
        if is_reparse(&metadata) {
            return Err("HDC_REPARSE_PATH_REJECTED".into());
        }
    }
    Ok(())
}

fn is_reparse(metadata: &Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;

    fn adapter_with_frame() -> (DeviceRuntimeAdapter, DeviceRunIdentity) {
        let adapter = DeviceRuntimeAdapter::new(None);
        let identity = DeviceRunIdentity {
            run_id: "run".into(),
            workspace_id: "project".into(),
            target_id: Some("target".into()),
            generation: 1,
        };
        let metadata = DeviceFrameMetadata {
            run_id: "run".into(),
            target_id: "target".into(),
            bundle_name: "app.test".into(),
            sequence: 7,
            width: 1,
            height: 1,
            capture_started_at: 1,
            received_at: 2,
            payload_length: 3,
            foreground_verified: true,
        };
        {
            let mut state = adapter.state.lock().unwrap();
            state.identity = Some(identity.clone());
            state.frame = Some(metadata.clone());
            state.frame_ready = true;
        }
        *adapter.frame.lock().unwrap() = Some(frame::DeviceFrame {
            metadata,
            png: vec![1, 2, 3],
        });
        (adapter, identity)
    }

    #[test]
    fn device_packet_requires_current_epoch_and_published_frame() {
        let (adapter, identity) = adapter_with_frame();
        assert!(adapter
            .read_frame(&identity, 6)
            .unwrap()
            .starts_with(b"ADDF"));
        assert!(adapter.read_frame(&identity, 7).unwrap().is_empty());
        adapter
            .state
            .lock()
            .unwrap()
            .frame
            .as_mut()
            .unwrap()
            .sequence = 8;
        assert!(adapter.read_frame(&identity, 6).unwrap().is_empty());
        adapter
            .state
            .lock()
            .unwrap()
            .identity
            .as_mut()
            .unwrap()
            .generation += 1;
        assert_eq!(
            adapter.read_frame(&identity, 0).unwrap_err(),
            "STALE_DEVICE_RUNTIME_IDENTITY"
        );
    }
}
