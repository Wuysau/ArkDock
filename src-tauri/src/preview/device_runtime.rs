//! HDC device execution has its own identity, consent and image transport.
//!
//! This adapter deliberately does not start a client until HDC server ownership
//! and separated stdout have a verified implementation. A PreviewBuild receipt,
//! an executable's presence, or a successful client exit cannot authorize a
//! deployment or establish that the selected application's pixels are visible.

mod plans;

pub use plans::{command_plan, parse_target_stdout, HdcCommandPlan, HdcOperation};

use crate::ipc::model::{
    DeviceRuntimeAction, DeviceRuntimeKind, DeviceRuntimeRequest, DeviceRuntimeState,
    DeviceRuntimeTarget, NativeControlIdentity,
};
use sha2::{Digest, Sha256};
use std::fs::{self, File, Metadata};
use std::io::Read;
use std::path::{Path, PathBuf};

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

/// Local selection is ephemeral. It is not an HDC connection, install grant or
/// a claim that a user's chosen device kind was proven by the runtime.
pub struct DeviceRuntimeAdapter {
    tool: ToolObservation,
    identity: Option<NativeControlIdentity>,
    targets: Vec<DeviceRuntimeTarget>,
    selected_target_id: Option<String>,
    kind: DeviceRuntimeKind,
    phase: String,
    reason: String,
}

impl DeviceRuntimeAdapter {
    /// Inspect only fixed locations below the explicitly selected SDK root.
    /// No PATH lookup, command execution, SDK mutation or device enumeration.
    pub fn new(sdk: Option<PathBuf>) -> Self {
        let tool = observe_tool(sdk.as_deref());
        Self {
            reason: tool.reason.clone(),
            tool,
            identity: None,
            targets: Vec::new(),
            selected_target_id: None,
            kind: DeviceRuntimeKind::Unknown,
            phase: "blocked".into(),
        }
    }

    /// Polling this method never starts HDC. A different preview identity
    /// invalidates every previous target selection and operation scope.
    pub fn state(&mut self, identity: Option<NativeControlIdentity>) -> DeviceRuntimeState {
        if self.identity != identity {
            self.cancel();
            self.identity = identity;
            self.phase = "blocked".into();
            self.reason = if self.identity.is_none() {
                "DEVICE_RUNTIME_SESSION_REQUIRED: 请先选择并信任一个工程。".into()
            } else {
                self.tool.reason.clone()
            };
        }
        self.snapshot()
    }

    /// IPC callers must additionally validate the current main window, project
    /// trust and actual preview identity before entering this adapter.
    pub fn action(&mut self, request: DeviceRuntimeRequest) -> Result<DeviceRuntimeState, String> {
        validate_identity(&request.identity)?;
        if self.identity.as_ref() != Some(&request.identity) {
            return Err("STALE_DEVICE_RUNTIME_IDENTITY".into());
        }
        if let Some(target_id) = &request.target_id {
            plans::validate_target_key(target_id)?;
        }
        match &request.action {
            DeviceRuntimeAction::Discover => {
                if !request.connection_consent {
                    return self.blocked(
                        "DEVICE_CONNECTION_CONSENT_REQUIRED: 需先允许设备发现；这可能启动共享 HDC 服务。",
                    );
                }
                if request.target_id.is_some() {
                    return Err("DISCOVERY_TARGET_MUST_BE_UNSET".into());
                }
                if self.tool.path.is_none() {
                    let reason = self.tool.reason.clone();
                    return self.blocked(&reason);
                }
                if !cfg!(windows) {
                    return self.blocked(
                        "DEVICE_RUNTIME_UNSUPPORTED_PLATFORM: 设备运行仅支持 Windows 本地环境。",
                    );
                }
                // Do not run `list targets` through windows_process.rs: that
                // runner combines stderr and stdout, and the installed client's
                // shared-server/auto-start lifecycle has not been verified.
                // No private `-p`, global `kill`, or default-device fallback.
                self.targets.clear();
                self.selected_target_id = None;
                self.kind = DeviceRuntimeKind::Unknown;
                // Structured code preserved: HDC_SERVER_LIFECYCLE_UNVERIFIED.
                self.blocked("未接入：暂不支持发现设备，ArkDock 不会启动 HDC 命令。")
            }
            DeviceRuntimeAction::Select => {
                if !request.connection_consent {
                    return self.blocked(
                        "DEVICE_CONNECTION_CONSENT_REQUIRED: 选择设备目标不代表允许部署。",
                    );
                }
                let id = request.target_id.as_ref().ok_or("DEVICE_TARGET_REQUIRED")?;
                let target = self
                    .targets
                    .iter()
                    .find(|target| &target.id == id)
                    .ok_or("DEVICE_TARGET_NOT_OBSERVED: 需先通过设备发现获得该目标")?;
                if target.connection_state != "Connected" {
                    return self.blocked(
                        "DEVICE_TARGET_NOT_CONNECTED: 请在设备上授权并重新连接所选目标。",
                    );
                }
                if request.kind == DeviceRuntimeKind::Unknown {
                    return self.blocked("DEVICE_KIND_REQUIRED: 请手动选择真机或模拟器。");
                }
                self.selected_target_id = Some(id.clone());
                self.kind = request.kind;
                if self.kind == DeviceRuntimeKind::Emulator {
                    self.blocked("FULL_EMULATOR_RUNTIME_UNAVAILABLE: 暂未接入模拟器运行。")
                } else {
                    self.blocked("DEVICE_DEPLOYMENT_BLOCKED: 暂未接入设备部署；当前构建产物不是可安装的 HAP。")
                }
            }
            DeviceRuntimeAction::Deploy => {
                if !request.deployment_consent {
                    return self.blocked("DEVICE_DEPLOYMENT_CONSENT_REQUIRED: 需明确允许将已签名 HAP 安装到所选设备。");
                }
                self.require_selected(&request)?;
                self.blocked("未接入：当前构建产物不是可安装的 HAP，无法部署到设备。")
            }
            DeviceRuntimeAction::Start => {
                self.require_selected(&request)?;
                self.blocked("未接入：没有可启动的已安装应用。")
            }
            DeviceRuntimeAction::Capture => {
                self.require_selected(&request)?;
                self.blocked("未接入：暂不支持截取设备画面。")
            }
            DeviceRuntimeAction::Stop => {
                self.require_selected(&request)?;
                self.blocked("未接入：ArkDock 未启动过设备应用，无需停止。")
            }
            DeviceRuntimeAction::Disconnect => {
                self.cancel();
                self.phase = "disconnected".into();
                self.reason = "已释放本地设备选择；共享 HDC 服务与其他设备会话未受影响。".into();
                Ok(self.snapshot())
            }
            DeviceRuntimeAction::Cancel => {
                self.cancel();
                Ok(self.snapshot())
            }
        }
    }

    /// Only local state is owned in the blocked implementation. Future client
    /// workers must be cancelled by their owned Job, never by HDC server name.
    pub fn cancel(&mut self) {
        self.targets.clear();
        self.selected_target_id = None;
        self.kind = DeviceRuntimeKind::Unknown;
        self.phase = "cancelled".into();
        self.reason = "设备运行请求已取消；未执行部署、输入、截屏或共享服务命令。".into();
    }

    fn require_selected(&self, request: &DeviceRuntimeRequest) -> Result<(), String> {
        let selected = self
            .selected_target_id
            .as_ref()
            .ok_or("DEVICE_TARGET_SELECTION_REQUIRED")?;
        if request.target_id.as_ref() != Some(selected) || request.kind != self.kind {
            return Err("STALE_DEVICE_TARGET_SELECTION".into());
        }
        Ok(())
    }

    fn blocked(&mut self, reason: &str) -> Result<DeviceRuntimeState, String> {
        self.phase = "blocked".into();
        self.reason = reason.into();
        Ok(self.snapshot())
    }

    fn snapshot(&self) -> DeviceRuntimeState {
        DeviceRuntimeState {
            identity: self.identity.clone(),
            kind: self.kind.clone(),
            hdc_path: self.tool.path.clone(),
            tool_sha256: self.tool.sha256.clone(),
            hdc_present: self.tool.path.is_some(),
            targets: self.targets.clone(),
            selected_target_id: self.selected_target_id.clone(),
            phase: self.phase.clone(),
            deployment_ready: false,
            signature_status: "notVerified".into(),
            foreground_verified: false,
            frame_ready: false,
            reason: self.reason.clone(),
        }
    }
}

fn validate_identity(identity: &NativeControlIdentity) -> Result<(), String> {
    for value in [
        &identity.workspace_id,
        &identity.session_id,
        &identity.build_id,
    ] {
        if value.is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
            return Err("INVALID_DEVICE_RUNTIME_IDENTITY".into());
        }
    }
    if identity.width == 0
        || identity.height == 0
        || identity.width > 4096
        || identity.height > 4096
        || u64::from(identity.width) * u64::from(identity.height) > 8_388_608
    {
        return Err("INVALID_DEVICE_RUNTIME_SURFACE".into());
    }
    Ok(())
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
            reason: "HDC_EXECUTION_UNVERIFIED: 已找到 HDC 工具，但设备发现、部署与画面暂未接入。"
                .into(),
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
