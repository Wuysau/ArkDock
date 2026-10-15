//! Bounded SDK discovery and the exact locally verified Windows Previewer profile.
pub mod build;
pub mod configuration;
pub mod control;
pub mod dependencies;
pub mod device_runtime;
pub mod environment;
pub mod frames;
#[cfg(windows)]
mod windows;
use crate::ipc::model::{
    BackendStatus, CheckStatus, Diagnostic, Environment, NativeFrame, ProjectRef,
};
use crate::scheduler::{validate_frame, BuildTask, FrameReject};
use std::collections::VecDeque;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::Child;
use std::time::{SystemTime, UNIX_EPOCH};

const BACKEND_ID: &str = "native-previewer";
const MANIFEST_LIMIT: u64 = 64 * 1024;
const SDK_MAX_DIRECTORIES: usize = 64;
const SDK_MAX_ENTRIES: usize = 1024;
const SDK_MAX_DEPTH: usize = 4;
const DIAGNOSTIC_LIMIT: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativePoint {
    pub x: u32,
    pub y: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PreviewInput {
    Click(NativePoint),
    Swipe {
        from: NativePoint,
        to: NativePoint,
        duration_ms: u32,
    },
    Text(String),
    Rotate(u32),
}

pub trait PreviewBackend: Send {
    fn probe(&mut self) -> BackendStatus;
    fn capabilities(&self) -> BackendStatus;
    fn start(
        &mut self,
        project: &ProjectRef,
        trusted: bool,
        session_id: &str,
    ) -> Result<(), String>;
    fn refresh(&mut self, task: &BuildTask) -> Result<(), String>;
    fn frame(&mut self) -> Option<NativeFrame>;
    fn binary_frame(&mut self) -> Option<frames::BinaryFrame>;
    fn completion(&mut self) -> Option<(BuildTask, Result<(), String>)>;
    fn input(&mut self, input: PreviewInput) -> Result<(), String>;
    fn stop(&mut self);
    fn diagnostics(&self) -> Vec<Diagnostic>;
}

pub struct NativePreviewer {
    sdk: Option<PathBuf>,
    status: BackendStatus,
    workspace_id: Option<String>,
    session_id: Option<String>,
    diagnostics: VecDeque<Diagnostic>,
    owned_child: Option<OwnedChild>,
    mailbox: FrameMailbox,
    project: Option<ProjectRef>,
    #[cfg(windows)]
    runtime: Option<windows::Runtime>,
}

impl NativePreviewer {
    pub fn request_control(
        &self,
        request: crate::ipc::model::NativeControlRequest,
        operation_id: String,
    ) -> Result<control::ControlReceiver, String> {
        #[cfg(windows)]
        {
            self.runtime
                .as_ref()
                .ok_or("PREVIEW_SESSION_REQUIRED")?
                .control(request, operation_id)
        }
        #[cfg(not(windows))]
        {
            let _ = (request, operation_id);
            Err("UNSUPPORTED_PLATFORM".into())
        }
    }

    pub fn cancel_controls(&self) {
        #[cfg(windows)]
        if let Some(runtime) = &self.runtime {
            runtime.cancel_controls();
        }
    }

    pub fn control_runtime_state(&self) -> Option<control::RuntimeControlState> {
        #[cfg(windows)]
        {
            self.runtime
                .as_ref()
                .map(|runtime| runtime.control_runtime_state())
        }
        #[cfg(not(windows))]
        {
            None
        }
    }

    pub fn configured_sdk(&self) -> Option<PathBuf> {
        self.sdk.clone()
    }

    /// Synchronous dependency preflight for the refresh command: a project whose
    /// declared dependencies are not installed fails before a build is scheduled,
    /// so the user gets the structured cause instead of the ArkTS cascade it causes.
    pub fn preflight(&self, project: &ProjectRef) -> Result<(), String> {
        #[cfg(windows)]
        {
            windows::dependency_preflight(project)
        }
        #[cfg(not(windows))]
        {
            let _ = project;
            Ok(())
        }
    }

    pub fn request_inspection(
        &self,
        command: crate::inspector::InspectorCommand,
    ) -> Result<
        std::sync::mpsc::Receiver<Result<crate::inspector::RawInspectionResponse, String>>,
        String,
    > {
        #[cfg(windows)]
        {
            self.runtime
                .as_ref()
                .ok_or("PREVIEW_SESSION_REQUIRED")?
                .request_inspection(command)
        }
        #[cfg(not(windows))]
        {
            let _ = command;
            Err("UNSUPPORTED_PLATFORM".into())
        }
    }

    pub fn new(sdk: Option<PathBuf>) -> Self {
        let status = probe(sdk.as_deref());
        Self {
            sdk,
            status,
            workspace_id: None,
            session_id: None,
            diagnostics: VecDeque::new(),
            owned_child: None,
            mailbox: FrameMailbox::new(),
            project: None,
            #[cfg(windows)]
            runtime: None,
        }
    }

    fn reject(&mut self, message: String, build_id: Option<String>) -> Result<(), String> {
        let code = message
            .split(':')
            .next()
            .unwrap_or("PREVIEW_BLOCKED")
            .to_owned();
        if self.diagnostics.len() == DIAGNOSTIC_LIMIT {
            self.diagnostics.pop_front();
        }
        self.diagnostics.push_back(Diagnostic {
            code,
            message: message.clone(),
            status: CheckStatus::Blocked,
            observed_at: now_ms(),
            workspace_id: self.workspace_id.clone(),
            session_id: self.session_id.clone(),
            build_id,
            backend_id: Some(BACKEND_ID.into()),
        });
        Err(message)
    }
}

impl PreviewBackend for NativePreviewer {
    fn probe(&mut self) -> BackendStatus {
        self.status = probe(self.sdk.as_deref());
        self.status.clone()
    }

    fn capabilities(&self) -> BackendStatus {
        self.status.clone()
    }

    fn start(
        &mut self,
        project: &ProjectRef,
        trusted: bool,
        session_id: &str,
    ) -> Result<(), String> {
        if self.workspace_id.as_deref() != Some(project.id.as_str())
            || self.session_id.as_deref() != Some(session_id)
        {
            self.stop();
        }
        self.workspace_id = Some(project.id.clone());
        self.session_id = Some(session_id.to_owned());
        if !trusted {
            return self.reject(
                "TRUST_REQUIRED: project execution has not been authorized".into(),
                None,
            );
        }
        if session_id.is_empty() || session_id.len() > 256 {
            return self.reject(
                "INVALID_SESSION: an active bounded session token is required".into(),
                None,
            );
        }
        if project.environment != Environment::WindowsLocal {
            return self.reject(
                "UNSUPPORTED_ENVIRONMENT: native preview supports only Windows-local projects"
                    .into(),
                None,
            );
        }
        self.probe();
        if !self.status.available {
            return self.reject(self.status.reason.clone(), None);
        }
        self.project = Some(project.clone());
        Ok(())
    }

    fn refresh(&mut self, task: &BuildTask) -> Result<(), String> {
        if self.session_id.as_deref() != Some(task.session_id.as_str())
            || self.workspace_id.as_deref() != Some(task.workspace_id.as_str())
        {
            return self.reject(
                "STALE_SESSION: refresh does not belong to the current workspace session".into(),
                Some(task.build_id.clone()),
            );
        }
        #[cfg(windows)]
        if self.status.available {
            let sdk = self
                .sdk
                .as_deref()
                .and_then(|sdk| {
                    windows::verified_root(sdk, self.status.sdk_version.as_deref().unwrap_or(""))
                })
                .ok_or("SDK_CHANGED: reselect the verified SDK")?;
            let project = self.project.clone().ok_or("PROJECT_REQUIRED")?;
            self.runtime.take();
            self.runtime = Some(windows::Runtime::launch(sdk, project, task.clone()));
            return Ok(());
        }
        self.reject(self.status.reason.clone(), Some(task.build_id.clone()))
    }

    fn frame(&mut self) -> Option<NativeFrame> {
        #[cfg(windows)]
        if self.runtime.is_some() {
            return self
                .binary_frame()
                .and_then(|frame| frame.legacy_png().ok());
        }
        self.mailbox.take()
    }

    fn binary_frame(&mut self) -> Option<frames::BinaryFrame> {
        #[cfg(windows)]
        if let Some(runtime) = &self.runtime {
            return runtime
                .frames
                .lock()
                .ok()
                .and_then(|mut frames| frames.take());
        }
        None
    }

    fn completion(&mut self) -> Option<(BuildTask, Result<(), String>)> {
        #[cfg(windows)]
        if let Some(runtime) = &self.runtime {
            return runtime.completed.try_recv().ok();
        }
        None
    }

    fn input(&mut self, input: PreviewInput) -> Result<(), String> {
        #[cfg(windows)]
        if let (Some(runtime), PreviewInput::Click(point)) = (&self.runtime, &input) {
            return runtime.click(*point);
        }
        let _ = input;
        self.reject(self.status.reason.clone(), None)
    }

    fn stop(&mut self) {
        self.mailbox.clear();
        #[cfg(windows)]
        self.runtime.take();
        self.project = None;
        self.workspace_id = None;
        self.session_id = None;
        // This handle can only refer to a child created by this application's adapter.
        // No PID lookup, process enumeration, or external-process termination is used.
        if let Some(mut child) = self.owned_child.take() {
            if let Err(error) = child.stop() {
                let _ = self.reject(format!("PREVIEW_CLEANUP: {error}"), None);
            }
        }
    }

    fn diagnostics(&self) -> Vec<Diagnostic> {
        self.diagnostics.iter().cloned().collect()
    }
}

impl NativePreviewer {
    pub fn frame_metrics(&self) -> Option<frames::BinaryFrameMetrics> {
        #[cfg(windows)]
        if let Some(runtime) = &self.runtime {
            return runtime.frames.lock().ok().map(|frames| frames.metrics());
        }
        None
    }
    pub fn runtime_messages(&mut self) -> Vec<String> {
        #[cfg(windows)]
        if let Some(runtime) = &self.runtime {
            if let Some(failure) = runtime
                .failure
                .lock()
                .ok()
                .and_then(|mut failure| failure.take())
            {
                return vec![failure];
            }
            return runtime.messages.try_iter().take(16).collect();
        }
        Vec::new()
    }
}

impl Drop for NativePreviewer {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Scan only the explicitly supplied SDK root. No PATH search or execution occurs.
pub fn probe(sdk: Option<&Path>) -> BackendStatus {
    probe_for_platform(sdk, cfg!(windows))
}

fn probe_for_platform(sdk: Option<&Path>, windows: bool) -> BackendStatus {
    let inspection = sdk.map(inspect_sdk);
    let detected = inspection.as_ref().and_then(|result| result.as_ref().ok());
    let reason = if !windows {
        "UNSUPPORTED_PLATFORM: native Windows Previewer requires a Windows desktop".to_owned()
    } else {
        match inspection.as_ref() {
            None => "SDK_MISSING: select a legally obtained local SDK; no SDK root is configured".into(),
            Some(Err(error)) => format!("SDK_MISSING: {error}"),
            Some(Ok(_)) => "PROTOCOL_UNVERIFIED: SDK files are present, but this SDK version's build, named-pipe, frame and input protocol has not been verified".into(),
        }
    };
    #[cfg(windows)]
    let verified_root = detected.and_then(|sdk| windows::verified_root(&sdk.root, &sdk.version));
    #[cfg(windows)]
    let dependency_error = verified_root.as_ref().and_then(|root| {
        let report = environment::inspect_environment(Some(root));
        report.execution_blocker().map(|check| {
            format!(
                "SDK_DEPENDENCY_UNAVAILABLE: {}: {}",
                check.dependency, check.detail
            )
        })
    });
    #[cfg(windows)]
    let available = windows && verified_root.is_some() && dependency_error.is_none();
    #[cfg(windows)]
    let reason = dependency_error.unwrap_or(reason);
    #[cfg(not(windows))]
    let available = false;
    BackendStatus {
        id: BACKEND_ID.into(),
        mode: "nativePreviewer".into(),
        available,
        reason: if available {
            "HarmonyOS 26.0.0.105：Stage phone 360×640 点击基线；新增官方控制命令为未验收实验入口，需单独启用；PreviewBuild 刷新重启运行时".into()
        } else {
            reason
        },
        sdk_path: detected.map(|sdk| sdk.root.to_string_lossy().into_owned()),
        sdk_version: detected.map(|sdk| sdk.version.clone()),
        can_click: available,
        can_swipe: false,
        can_text: false,
        can_rotate: false,
    }
}

struct SdkInspection {
    root: PathBuf,
    version: String,
}

/// Recognized package layouts are bounded; unsupported layouts ask for a narrower root.
/// Package metadata is an observation, never an executable path or a legal/authenticity claim.
fn inspect_sdk(sdk: &Path) -> Result<SdkInspection, String> {
    let root = sdk
        .canonicalize()
        .map_err(|error| format!("configured SDK is inaccessible ({error})"))?;
    if !root.is_dir() {
        return Err("configured SDK root is not a directory".into());
    }
    let mut queue = VecDeque::from([(root.clone(), 0)]);
    let mut directories = 0;
    let mut entries_seen = 0;
    let mut ets_versions = Vec::new();
    let mut previewer_packages = Vec::new();
    while let Some((directory, depth)) = queue.pop_front() {
        directories += 1;
        if directories > SDK_MAX_DIRECTORIES {
            return Err(
                "SDK probe directory limit reached; select a specific SDK version root".into(),
            );
        }
        let manifest = directory.join("oh-uni-package.json");
        if let Some((component, version)) = read_package_manifest(&root, &manifest) {
            match component.as_str() {
                "ets" => ets_versions.push(version),
                "previewer" => previewer_packages.push((directory.clone(), version)),
                _ => {}
            }
        }
        if depth == SDK_MAX_DEPTH {
            continue;
        }
        let entries = fs::read_dir(&directory)
            .map_err(|error| format!("SDK directory cannot be inspected ({error})"))?;
        let mut next = Vec::new();
        for entry in entries {
            entries_seen += 1;
            if entries_seen > SDK_MAX_ENTRIES {
                return Err(
                    "SDK probe entry limit reached; select a specific SDK version root".into(),
                );
            }
            let entry = entry
                .map_err(|error| format!("SDK directory entry cannot be inspected ({error})"))?;
            let file_type = entry
                .file_type()
                .map_err(|error| format!("SDK directory entry cannot be inspected ({error})"))?;
            // Do not walk symlinks/junctions out of the selected root.
            if file_type.is_dir()
                && !file_type.is_symlink()
                && sdk_directory_name(&entry.file_name().to_string_lossy())
            {
                if let Ok(path) = entry.path().canonicalize() {
                    if path.starts_with(&root) {
                        next.push(path);
                    }
                }
            }
        }
        next.sort();
        queue.extend(next.into_iter().map(|path| (path, depth + 1)));
    }
    for (directory, version) in previewer_packages {
        if !ets_versions.contains(&version) {
            continue;
        }
        for relative in [
            "Previewer.exe",
            "previewer.exe",
            "bin/Previewer.exe",
            "bin/previewer.exe",
            "common/bin/Previewer.exe",
        ] {
            if is_windows_previewer(&root, &directory.join(relative)) {
                return Ok(SdkInspection { root, version });
            }
        }
    }
    Err("no matching ets/previewer package manifests and Windows Previewer binary were found in the supported bounded layout".into())
}

fn sdk_directory_name(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "sdk" | "default" | "openharmony" | "windows" | "ets" | "previewer" | "bin"
    ) || (name.as_bytes().first().is_some_and(u8::is_ascii_digit)
        && name.len() <= 64
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_')))
}

fn read_package_manifest(root: &Path, path: &Path) -> Option<(String, String)> {
    let path = path.canonicalize().ok()?;
    if !path.starts_with(root) {
        return None;
    }
    let mut file = File::open(path).ok()?;
    if !file.metadata().ok()?.is_file() || file.metadata().ok()?.len() > MANIFEST_LIMIT {
        return None;
    }
    let mut data = String::new();
    Read::by_ref(&mut file)
        .take(MANIFEST_LIMIT + 1)
        .read_to_string(&mut data)
        .ok()?;
    if data.len() as u64 > MANIFEST_LIMIT {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(&data).ok()?;
    let component = value.get("path")?.as_str()?;
    let version = value.get("version")?.as_str()?;
    if !matches!(component, "ets" | "previewer")
        || version.is_empty()
        || version.len() > 128
        || !version
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
    {
        return None;
    }
    Some((component.into(), version.into()))
}

fn is_windows_previewer(root: &Path, path: &Path) -> bool {
    let Ok(path) = path.canonicalize() else {
        return false;
    };
    if !path.starts_with(root) {
        return false;
    }
    let Ok(mut file) = File::open(path) else {
        return false;
    };
    let Ok(metadata) = file.metadata() else {
        return false;
    };
    if !metadata.is_file() || metadata.len() < 70 || metadata.len() > 512 * 1024 * 1024 {
        return false;
    }
    let mut dos = [0u8; 64];
    if file.read_exact(&mut dos).is_err() || &dos[..2] != b"MZ" {
        return false;
    }
    let offset = u32::from_le_bytes([dos[60], dos[61], dos[62], dos[63]]) as u64;
    if offset < 64
        || offset > metadata.len().saturating_sub(6)
        || file.seek(SeekFrom::Start(offset)).is_err()
    {
        return false;
    }
    let mut pe = [0u8; 6];
    if file.read_exact(&mut pe).is_err() || &pe[..4] != b"PE\0\0" {
        return false;
    }
    matches!(u16::from_le_bytes([pe[4], pe[5]]), 0x8664 | 0xaa64)
}

/// The queue contains at most one bounded frame; producers replace unread old frames.
#[derive(Default)]
pub struct FrameMailbox {
    stream: Option<(String, String, u64)>,
    latest: Option<NativeFrame>,
    last_sequence: Option<u64>,
}

impl FrameMailbox {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_stream(&mut self, session_id: &str, build_id: &str, generation: u64) {
        self.clear();
        self.stream = Some((session_id.into(), build_id.into(), generation));
    }

    pub fn push(&mut self, frame: NativeFrame) -> Result<(), FrameReject> {
        let (session_id, build_id, generation) =
            self.stream.as_ref().ok_or(FrameReject::Stopped)?;
        if &frame.session_id != session_id {
            return Err(FrameReject::WrongSession);
        }
        if &frame.build_id != build_id {
            return Err(FrameReject::WrongBuild);
        }
        if frame.generation != *generation {
            return Err(FrameReject::WrongGeneration);
        }
        if self
            .last_sequence
            .is_some_and(|sequence| frame.sequence <= sequence)
        {
            return Err(FrameReject::OldSequence);
        }
        validate_frame(&frame)?;
        self.last_sequence = Some(frame.sequence);
        self.latest = Some(frame);
        Ok(())
    }

    pub fn take(&mut self) -> Option<NativeFrame> {
        self.latest.take()
    }

    pub fn clear(&mut self) {
        self.stream = None;
        self.latest = None;
        self.last_sequence = None;
    }
}

/// Viewport coordinates use CSS pixels. PNG width/height describe the unrotated
/// native surface; rotation describes clockwise presentation of that surface.
pub fn map_input_point(
    x: f64,
    y: f64,
    viewport_width: f64,
    viewport_height: f64,
    frame_width: u32,
    frame_height: u32,
    rotation: u32,
) -> Option<NativePoint> {
    if [x, y, viewport_width, viewport_height]
        .iter()
        .any(|value| !value.is_finite())
        || viewport_width <= 0.0
        || viewport_height <= 0.0
        || frame_width == 0
        || frame_height == 0
        || frame_width > 8192
        || frame_height > 8192
        || !matches!(rotation, 0 | 90 | 180 | 270)
    {
        return None;
    }
    let (display_width, display_height) = if matches!(rotation, 90 | 270) {
        (f64::from(frame_height), f64::from(frame_width))
    } else {
        (f64::from(frame_width), f64::from(frame_height))
    };
    let scale = (viewport_width / display_width).min(viewport_height / display_height);
    let width = display_width * scale;
    let height = display_height * scale;
    let left = (viewport_width - width) / 2.0;
    let top = (viewport_height - height) / 2.0;
    if x < left || y < top || x >= left + width || y >= top + height {
        return None;
    }
    let rx = (x - left) / width;
    let ry = (y - top) / height;
    let (nx, ny) = match rotation {
        0 => (rx, ry),
        90 => (ry, 1.0 - rx),
        180 => (1.0 - rx, 1.0 - ry),
        270 => (1.0 - ry, rx),
        _ => return None,
    };
    Some(NativePoint {
        x: ((nx * f64::from(frame_width)).floor() as u32).min(frame_width - 1),
        y: ((ny * f64::from(frame_height)).floor() as u32).min(frame_height - 1),
    })
}

/// Only a Child returned by this application's future verified adapter is owned.
/// No public command/argv launch surface is exposed. Process-tree containment
/// must be added with the verified Windows adapter before enabling execution.
pub struct OwnedChild {
    child: Child,
    stopped: bool,
}

impl OwnedChild {
    pub fn from_spawned(child: Child) -> Self {
        Self {
            child,
            stopped: false,
        }
    }

    pub fn stop(&mut self) -> std::io::Result<()> {
        if self.stopped {
            return Ok(());
        }
        if self.child.try_wait()?.is_none() {
            self.child.kill()?;
        }
        self.child.wait()?;
        self.stopped = true;
        Ok(())
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::model::Environment;

    fn project() -> ProjectRef {
        ProjectRef {
            id: "project".into(),
            name: "App".into(),
            environment: Environment::WindowsLocal,
            workspace_root: "C:\\App".into(),
            app_root: "C:\\App".into(),
            module: "entry".into(),
            variant: "default".into(),
            sdk_id: None,
        }
    }

    #[test]
    fn no_sdk_and_unknown_protocol_never_start_a_backend() {
        let mut backend = NativePreviewer::new(None);
        assert!(!backend.capabilities().available);
        assert_eq!(
            backend.start(&project(), false, "session"),
            Err("TRUST_REQUIRED: project execution has not been authorized".into())
        );
        assert!(backend.start(&project(), true, "session").is_err());
        assert!(backend.frame().is_none());
        assert!(!backend.capabilities().can_click);
        backend.stop();
        assert!(backend.frame().is_none());
    }

    #[test]
    fn sdk_probe_has_distinct_platform_missing_and_protocol_reasons() {
        assert!(probe_for_platform(None, false)
            .reason
            .starts_with("UNSUPPORTED_PLATFORM:"));
        assert!(probe_for_platform(None, true)
            .reason
            .starts_with("SDK_MISSING:"));
        let sdk = tempfile::tempdir().unwrap();
        let ets = sdk.path().join("ets");
        let previewer = sdk.path().join("previewer");
        fs::create_dir_all(&ets).unwrap();
        fs::create_dir_all(&previewer).unwrap();
        fs::write(
            ets.join("oh-uni-package.json"),
            r#"{"path":"ets","version":"4.0.0.1"}"#,
        )
        .unwrap();
        fs::write(
            previewer.join("oh-uni-package.json"),
            r#"{"path":"previewer","version":"4.0.0.1"}"#,
        )
        .unwrap();
        let mut pe = vec![0u8; 128];
        pe[0..2].copy_from_slice(b"MZ");
        pe[60..64].copy_from_slice(&64u32.to_le_bytes());
        pe[64..68].copy_from_slice(b"PE\0\0");
        pe[68..70].copy_from_slice(&0x8664u16.to_le_bytes());
        fs::write(previewer.join("Previewer.exe"), pe).unwrap();
        let status = probe_for_platform(Some(sdk.path()), true);
        assert!(status.reason.starts_with("PROTOCOL_UNVERIFIED:"));
        assert!(!status.available);
        assert_eq!(status.sdk_version.as_deref(), Some("4.0.0.1"));
        fs::write(
            ets.join("oh-uni-package.json"),
            r#"{"path":"ets","version":"5.0.0.1"}"#,
        )
        .unwrap();
        assert!(probe_for_platform(Some(sdk.path()), true)
            .reason
            .starts_with("SDK_MISSING:"));
    }

    #[test]
    fn malformed_or_large_sdk_manifests_do_not_enable_execution() {
        let sdk = tempfile::tempdir().unwrap();
        fs::write(
            sdk.path().join("oh-uni-package.json"),
            vec![b'x'; MANIFEST_LIMIT as usize + 1],
        )
        .unwrap();
        assert!(
            read_package_manifest(sdk.path(), &sdk.path().join("oh-uni-package.json")).is_none()
        );
        fs::write(
            sdk.path().join("oh-uni-package.json"),
            r#"{"path":"../../outside","version":"4.0.0.1"}"#,
        )
        .unwrap();
        assert!(
            read_package_manifest(sdk.path(), &sdk.path().join("oh-uni-package.json")).is_none()
        );
        assert!(!probe_for_platform(Some(sdk.path()), true).available);
    }

    #[test]
    fn letterbox_rejects_margins_and_rotates_to_native_coordinates() {
        assert_eq!(map_input_point(10.0, 50.0, 300.0, 200.0, 100, 200, 0), None);
        assert_eq!(
            map_input_point(150.0, 100.0, 300.0, 200.0, 100, 200, 0),
            Some(NativePoint { x: 50, y: 100 })
        );
        assert_eq!(
            map_input_point(50.0, 100.0, 200.0, 100.0, 100, 200, 90),
            None
        );
        assert_eq!(
            map_input_point(50.0, 25.0, 200.0, 100.0, 100, 200, 90),
            Some(NativePoint { x: 25, y: 150 })
        );
        assert_eq!(
            map_input_point(25.0, 50.0, 100.0, 200.0, 100, 200, 180),
            Some(NativePoint { x: 75, y: 150 })
        );
        assert_eq!(
            map_input_point(50.0, 25.0, 200.0, 100.0, 100, 200, 270),
            Some(NativePoint { x: 75, y: 50 })
        );
        assert!(map_input_point(f64::NAN, 1.0, 200.0, 100.0, 100, 200, 0).is_none());
    }

    #[test]
    fn latest_frame_mailbox_bounds_backpressure_and_closes_stream() {
        let mut mailbox = FrameMailbox::new();
        mailbox.set_stream("session", "build", 3);
        let frame = |sequence| {
            NativeFrame { session_id: "session".into(), build_id: "build".into(), generation: 3, sequence, width: 1, height: 1, captured_at: 1, rotation: 0, png_base64: "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jF9sAAAAASUVORK5CYII=".into() }
        };
        mailbox.push(frame(1)).unwrap();
        mailbox.push(frame(2)).unwrap();
        assert_eq!(mailbox.take().unwrap().sequence, 2);
        assert!(mailbox.take().is_none());
        assert_eq!(mailbox.push(frame(1)), Err(FrameReject::OldSequence));
        let mut wrong = frame(3);
        wrong.session_id = "other".into();
        assert_eq!(mailbox.push(wrong), Err(FrameReject::WrongSession));
        mailbox.clear();
        assert_eq!(mailbox.push(frame(4)), Err(FrameReject::Stopped));
    }
}
