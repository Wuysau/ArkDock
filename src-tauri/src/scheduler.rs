//! Generation and lifecycle checks are independent of the native transport.
//! This coordinator issues descriptors, never shell commands or synthetic build results.
use crate::ipc::model::{BuildStatus, NativeFrame};
use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Component, Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuildTask {
    pub epoch: u64,
    pub workspace_id: String,
    pub session_id: String,
    pub generation: u64,
    pub build_id: String,
    pub started_at: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BuildOutcome {
    Success,
    Failure(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompletionDisposition {
    AcceptedSuccess,
    AcceptedFailure,
    RejectedStale,
    RejectedUnknown,
    RejectedStopped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameReject {
    Stopped,
    NoSuccessfulBuild,
    WrongSession,
    WrongBuild,
    WrongGeneration,
    OldSequence,
    InvalidMetadata,
    InvalidDimensions,
    InvalidRotation,
    PayloadTooLarge,
    InvalidEncoding,
    ImageDimensionsMismatch,
}

#[derive(Debug)]
struct WorkspaceEpoch {
    workspace_id: String,
    session_id: String,
}

#[derive(Debug)]
pub struct BuildCoordinator {
    debounce_ms: u64,
    epoch: u64,
    workspace: Option<WorkspaceEpoch>,
    generation: u64,
    pending_at: Option<u64>,
    active: Option<BuildTask>,
    last_success: Option<BuildTask>,
    displayed_generation: Option<u64>,
    latest_build_id: Option<String>,
    duration_ms: Option<u64>,
    error: Option<String>,
    last_frame_sequence: Option<u64>,
}

impl BuildCoordinator {
    pub fn new(debounce_ms: u64) -> Self {
        Self {
            debounce_ms,
            epoch: 0,
            workspace: None,
            generation: 0,
            pending_at: None,
            active: None,
            last_success: None,
            displayed_generation: None,
            latest_build_id: None,
            duration_ms: None,
            error: None,
            last_frame_sequence: None,
        }
    }

    /// Selecting even the same identity closes the old execution epoch.
    /// The caller requests the initial refresh after trust and backend gates pass.
    pub fn set_workspace(&mut self, workspace_id: &str, session_id: &str, _now_ms: u64) {
        self.stop();
        if self.epoch == u64::MAX {
            return;
        }
        self.generation = 0;
        self.workspace = Some(WorkspaceEpoch {
            workspace_id: workspace_id.to_owned(),
            session_id: session_id.to_owned(),
        });
    }

    /// A batch of source changes creates one generation and restarts the debounce.
    pub fn request_refresh(&mut self, now_ms: u64) -> Option<u64> {
        self.workspace.as_ref()?;
        // Exhaustion is a closed lifecycle, never wrap and reuse a generation.
        let Some(generation) = self.generation.checked_add(1) else {
            self.stop();
            return None;
        };
        self.generation = generation;
        self.pending_at = Some(now_ms.saturating_add(self.debounce_ms));
        self.last_frame_sequence = None;
        Some(generation)
    }

    pub fn next_task(&mut self, now_ms: u64) -> Option<BuildTask> {
        if self.active.is_some() || now_ms < self.pending_at? {
            return None;
        }
        let workspace = self.workspace.as_ref()?;
        let task = BuildTask {
            epoch: self.epoch,
            workspace_id: workspace.workspace_id.clone(),
            session_id: workspace.session_id.clone(),
            generation: self.generation,
            build_id: format!("epoch-{}-generation-{}", self.epoch, self.generation),
            started_at: now_ms,
        };
        self.pending_at = None;
        self.latest_build_id = Some(task.build_id.clone());
        self.error = None;
        self.duration_ms = None;
        self.active = Some(task.clone());
        Some(task)
    }

    pub fn complete(
        &mut self,
        task: &BuildTask,
        outcome: BuildOutcome,
        now_ms: u64,
    ) -> CompletionDisposition {
        let Some(workspace) = self.workspace.as_ref() else {
            return CompletionDisposition::RejectedStopped;
        };
        if task.epoch != self.epoch
            || task.workspace_id != workspace.workspace_id
            || task.session_id != workspace.session_id
        {
            return CompletionDisposition::RejectedStale;
        }
        if self.active.as_ref() != Some(task) {
            return CompletionDisposition::RejectedUnknown;
        }
        // Release only the exact active task, so an old callback cannot release a new task.
        self.active = None;
        if task.generation != self.generation {
            return CompletionDisposition::RejectedStale;
        }
        self.duration_ms = Some(now_ms.saturating_sub(task.started_at));
        match outcome {
            BuildOutcome::Success => {
                self.last_success = Some(task.clone());
                self.last_frame_sequence = None;
                self.error = None;
                CompletionDisposition::AcceptedSuccess
            }
            BuildOutcome::Failure(error) => {
                self.error = Some(error.chars().take(4096).collect());
                CompletionDisposition::AcceptedFailure
            }
        }
    }

    pub fn accept_frame(&mut self, frame: &NativeFrame) -> Result<(), FrameReject> {
        let workspace = self.workspace.as_ref().ok_or(FrameReject::Stopped)?;
        if frame.session_id != workspace.session_id {
            return Err(FrameReject::WrongSession);
        }
        if frame.generation != self.generation {
            return Err(FrameReject::WrongGeneration);
        }
        let success = self
            .last_success
            .as_ref()
            .ok_or(FrameReject::NoSuccessfulBuild)?;
        if frame.build_id != success.build_id {
            return Err(FrameReject::WrongBuild);
        }
        if frame.generation != success.generation {
            return Err(FrameReject::WrongGeneration);
        }
        if self
            .last_frame_sequence
            .is_some_and(|sequence| frame.sequence <= sequence)
        {
            return Err(FrameReject::OldSequence);
        }
        validate_frame(frame)?;
        self.last_frame_sequence = Some(frame.sequence);
        self.displayed_generation = Some(frame.generation);
        Ok(())
    }

    pub fn status(&self) -> BuildStatus {
        let displayed_generation = self.displayed_generation;
        BuildStatus {
            generation: self.generation,
            displayed_generation,
            build_id: self.latest_build_id.clone(),
            duration_ms: self.duration_ms,
            error: self.error.clone(),
            stale: displayed_generation
                .is_some_and(|generation| generation != self.generation || self.error.is_some()),
        }
    }

    pub fn is_building(&self) -> bool {
        self.active.is_some()
    }

    pub fn runtime_failed(&mut self, message: &str) {
        if self.workspace.is_some() {
            self.last_success = None;
            self.error = Some(message.chars().take(4096).collect());
        }
    }

    pub fn stop(&mut self) {
        // Saturation never reopens an exhausted epoch.
        self.epoch = self.epoch.saturating_add(1);
        self.workspace = None;
        self.pending_at = None;
        self.active = None;
        self.last_success = None;
        self.displayed_generation = None;
        self.latest_build_id = None;
        self.duration_ms = None;
        self.error = None;
        self.last_frame_sequence = None;
    }
}

pub const MAX_FRAME_BASE64_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_FRAME_DIMENSION: u32 = 8192;
const MAX_FRAME_PIXELS: u64 = 16 * 1024 * 1024;

/// Bound IPC memory and compare IHDR dimensions before accepting native image data.
/// The image decoder must still check PNG CRCs and complete image validity.
pub fn validate_frame(frame: &NativeFrame) -> Result<(), FrameReject> {
    if frame.session_id.is_empty()
        || frame.session_id.len() > 256
        || frame.build_id.is_empty()
        || frame.build_id.len() > 256
        || frame.generation == 0
    {
        return Err(FrameReject::InvalidMetadata);
    }
    if frame.width == 0
        || frame.height == 0
        || frame.width > MAX_FRAME_DIMENSION
        || frame.height > MAX_FRAME_DIMENSION
        || u64::from(frame.width) * u64::from(frame.height) > MAX_FRAME_PIXELS
    {
        return Err(FrameReject::InvalidDimensions);
    }
    if !matches!(frame.rotation, 0 | 90 | 180 | 270) {
        return Err(FrameReject::InvalidRotation);
    }
    let bytes = frame.png_base64.as_bytes();
    if bytes.len() > MAX_FRAME_BASE64_BYTES {
        return Err(FrameReject::PayloadTooLarge);
    }
    if bytes.len() < 44 || bytes.len() % 4 != 0 {
        return Err(FrameReject::InvalidEncoding);
    }
    let padding = bytes.iter().rev().take_while(|&&byte| byte == b'=').count();
    if padding > 2
        || bytes[..bytes.len() - padding]
            .iter()
            .any(|&byte| base64_value(byte).is_none())
    {
        return Err(FrameReject::InvalidEncoding);
    }
    let mut header = [0u8; 33];
    let mut written = 0;
    for chunk in bytes[..44].chunks_exact(4) {
        let a = base64_value(chunk[0]).ok_or(FrameReject::InvalidEncoding)?;
        let b = base64_value(chunk[1]).ok_or(FrameReject::InvalidEncoding)?;
        let c = base64_value(chunk[2]).ok_or(FrameReject::InvalidEncoding)?;
        let d = base64_value(chunk[3]).ok_or(FrameReject::InvalidEncoding)?;
        header[written] = (a << 2) | (b >> 4);
        header[written + 1] = (b << 4) | (c >> 2);
        header[written + 2] = (c << 6) | d;
        written += 3;
    }
    if header[..8] != [137, 80, 78, 71, 13, 10, 26, 10]
        || header[8..12] != [0, 0, 0, 13]
        || &header[12..16] != b"IHDR"
    {
        return Err(FrameReject::InvalidEncoding);
    }
    let width = u32::from_be_bytes(
        header[16..20]
            .try_into()
            .map_err(|_| FrameReject::InvalidEncoding)?,
    );
    let height = u32::from_be_bytes(
        header[20..24]
            .try_into()
            .map_err(|_| FrameReject::InvalidEncoding)?,
    );
    if width != frame.width || height != frame.height {
        return Err(FrameReject::ImageDimensionsMismatch);
    }
    Ok(())
}

fn base64_value(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

#[derive(Clone, Debug)]
pub enum WatchEvent {
    Changed(Vec<PathBuf>),
    Error(String),
}

/// Ownership keeps the operating-system watcher alive; dropping closes only this watch.
pub struct WatchGuard {
    watcher: RecommendedWatcher,
    root: PathBuf,
}

impl Drop for WatchGuard {
    fn drop(&mut self) {
        let _ = self.watcher.unwatch(&self.root);
    }
}

pub fn watch_project<F>(root: &Path, callback: F) -> Result<WatchGuard, String>
where
    F: Fn(WatchEvent) + Send + 'static,
{
    let root = root
        .canonicalize()
        .map_err(|error| format!("WATCH_ROOT: {error}"))?;
    if !root.is_dir() {
        return Err("WATCH_ROOT: project root is not a directory".into());
    }
    let watched_root = root.clone();
    let mut watcher = RecommendedWatcher::new(
        move |result: notify::Result<notify::Event>| match result {
            Ok(event)
                if matches!(
                    event.kind,
                    EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
                ) =>
            {
                let paths: Vec<_> = event
                    .paths
                    .into_iter()
                    .filter(|path| is_source_path(&watched_root, path))
                    .collect();
                if !paths.is_empty() {
                    callback(WatchEvent::Changed(paths));
                }
            }
            Ok(_) => {}
            Err(error) => callback(WatchEvent::Error(format!("WATCH_ERROR: {error}"))),
        },
        Config::default().with_follow_symlinks(false),
    )
    .map_err(|error| format!("WATCH_START: {error}"))?;
    watcher
        .watch(&root, RecursiveMode::Recursive)
        .map_err(|error| format!("WATCH_START: {error}"))?;
    Ok(WatchGuard { watcher, root })
}

pub fn is_source_path(root: &Path, path: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(root) else {
        return false;
    };
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return false;
        };
        let name = name.to_string_lossy().to_ascii_lowercase();
        if matches!(
            name.as_str(),
            ".git"
                | "node_modules"
                | "oh_modules"
                | "build"
                | ".hvigor"
                | ".cache"
                | "cache"
                | "caches"
                | "temp"
                | "tmp"
                | ".tmp"
                | ".preview"
                | ".arkdock"
        ) || name.starts_with(".~")
            || name.ends_with('~')
            || [".tmp", ".temp", ".swp", ".swo", ".bak"]
                .iter()
                .any(|suffix| name.ends_with(suffix))
        {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn coordinator() -> BuildCoordinator {
        let mut coordinator = BuildCoordinator::new(100);
        coordinator.set_workspace("project", "session", 0);
        coordinator
    }

    fn frame(task: &BuildTask, sequence: u64) -> NativeFrame {
        NativeFrame {
            session_id: task.session_id.clone(), build_id: task.build_id.clone(),
            generation: task.generation, sequence, width: 1, height: 1,
            captured_at: 1, rotation: 0,
            png_base64: "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jF9sAAAAASUVORK5CYII=".into(),
        }
    }

    #[test]
    fn multiple_changes_debounce_into_one_latest_task() {
        let mut c = coordinator();
        assert_eq!(c.request_refresh(0), Some(1));
        assert_eq!(c.request_refresh(50), Some(2));
        assert!(c.next_task(149).is_none());
        let task = c.next_task(150).unwrap();
        assert_eq!(task.generation, 2);
        assert!(c.next_task(500).is_none());
    }

    #[test]
    fn change_during_build_runs_next_task_and_rejects_old_completion() {
        let mut c = coordinator();
        c.request_refresh(0);
        let old = c.next_task(100).unwrap();
        c.request_refresh(101);
        assert!(c.next_task(300).is_none());
        assert_eq!(
            c.complete(&old, BuildOutcome::Success, 300),
            CompletionDisposition::RejectedStale
        );
        assert_eq!(c.status().displayed_generation, None);
        let next = c.next_task(300).unwrap();
        assert_eq!(next.generation, 2);
        assert_eq!(
            c.complete(&old, BuildOutcome::Success, 301),
            CompletionDisposition::RejectedUnknown
        );
        assert!(c.next_task(400).is_none());
    }

    #[test]
    fn failure_retains_last_success_and_marks_it_stale() {
        let mut c = coordinator();
        c.request_refresh(0);
        let first = c.next_task(100).unwrap();
        assert_eq!(
            c.complete(&first, BuildOutcome::Success, 120),
            CompletionDisposition::AcceptedSuccess
        );
        assert!(c.accept_frame(&frame(&first, 1)).is_ok());
        c.request_refresh(150);
        assert!(c.status().stale);
        assert_eq!(
            c.accept_frame(&frame(&first, 2)),
            Err(FrameReject::WrongGeneration)
        );
        let second = c.next_task(250).unwrap();
        assert_eq!(
            c.complete(&second, BuildOutcome::Failure("compile failed".into()), 280),
            CompletionDisposition::AcceptedFailure
        );
        let status = c.status();
        assert_eq!(status.displayed_generation, Some(1));
        assert_eq!(status.error.as_deref(), Some("compile failed"));
        assert_eq!(status.duration_ms, Some(30));
        assert!(status.stale);
    }

    #[test]
    fn successful_build_does_not_claim_a_frame_was_displayed() {
        let mut c = coordinator();
        c.request_refresh(0);
        let task = c.next_task(100).unwrap();
        c.complete(&task, BuildOutcome::Success, 120);
        assert_eq!(c.status().displayed_generation, None);
        c.accept_frame(&frame(&task, 1)).unwrap();
        assert_eq!(c.status().displayed_generation, Some(1));
        c.request_refresh(150);
        let second = c.next_task(250).unwrap();
        c.complete(&second, BuildOutcome::Success, 280);
        assert_eq!(c.status().displayed_generation, Some(1));
        assert!(c.status().stale);
        c.accept_frame(&frame(&second, 1)).unwrap();
        assert_eq!(c.status().displayed_generation, Some(2));
        assert!(!c.status().stale);
    }

    #[test]
    fn cross_session_and_out_of_order_frames_are_rejected() {
        let mut c = coordinator();
        c.request_refresh(0);
        let task = c.next_task(100).unwrap();
        c.complete(&task, BuildOutcome::Success, 120);
        let mut wrong = frame(&task, 1);
        wrong.session_id = "different".into();
        assert_eq!(c.accept_frame(&wrong), Err(FrameReject::WrongSession));
        assert!(c.accept_frame(&frame(&task, 2)).is_ok());
        assert_eq!(
            c.accept_frame(&frame(&task, 1)),
            Err(FrameReject::OldSequence)
        );
        assert_eq!(
            c.accept_frame(&frame(&task, 2)),
            Err(FrameReject::OldSequence)
        );
    }

    #[test]
    fn runtime_exit_keeps_picture_stale_and_rejects_pending_frames_until_rebuild() {
        let mut c = coordinator();
        c.request_refresh(0);
        let task = c.next_task(100).unwrap();
        c.complete(&task, BuildOutcome::Success, 120);
        c.accept_frame(&frame(&task, 1)).unwrap();
        c.runtime_failed("PREVIEWER_EXIT: 1");
        assert!(c.status().stale);
        assert_eq!(c.status().displayed_generation, Some(1));
        assert_eq!(
            c.accept_frame(&frame(&task, 2)),
            Err(FrameReject::NoSuccessfulBuild)
        );
        c.request_refresh(200);
        let next = c.next_task(300).unwrap();
        c.complete(&next, BuildOutcome::Success, 320);
        c.accept_frame(&frame(&next, 1)).unwrap();
        assert!(!c.status().stale);
        assert_eq!(c.status().error, None);
    }

    #[test]
    fn stopping_and_reselecting_same_workspace_closes_old_epoch() {
        let mut c = coordinator();
        c.request_refresh(0);
        let old = c.next_task(100).unwrap();
        c.stop();
        assert_eq!(
            c.complete(&old, BuildOutcome::Success, 120),
            CompletionDisposition::RejectedStopped
        );
        assert_eq!(c.request_refresh(120), None);
        c.set_workspace("project", "session", 130);
        c.request_refresh(130);
        let new = c.next_task(230).unwrap();
        assert_ne!(new.epoch, old.epoch);
        assert_ne!(new.build_id, old.build_id);
        assert_eq!(
            c.complete(&old, BuildOutcome::Success, 240),
            CompletionDisposition::RejectedStale
        );
        assert_eq!(
            c.complete(&new, BuildOutcome::Success, 250),
            CompletionDisposition::AcceptedSuccess
        );
    }

    #[test]
    fn watcher_ignores_generated_and_dependency_paths_only_under_root() {
        let root = Path::new("/project/build/app");
        assert!(is_source_path(
            root,
            Path::new("/project/build/app/entry/src/Index.ets")
        ));
        for path in [
            "build/result.abc",
            "oh_modules/x.ets",
            "node_modules/x.js",
            ".git/config",
            ".hvigor/cache",
            "entry/x.ets.tmp",
            "cache/a",
        ] {
            assert!(!is_source_path(root, &root.join(path)), "{path}");
        }
        assert!(!is_source_path(root, Path::new("/other/Index.ets")));
    }

    #[test]
    fn invalid_png_dimensions_and_payload_are_rejected() {
        let mut c = coordinator();
        c.request_refresh(0);
        let task = c.next_task(100).unwrap();
        let mut f = frame(&task, 1);
        f.session_id.clear();
        assert_eq!(validate_frame(&f), Err(FrameReject::InvalidMetadata));
        f.session_id = task.session_id.clone();
        f.width = 8193;
        assert_eq!(validate_frame(&f), Err(FrameReject::InvalidDimensions));
        f.width = 2;
        assert_eq!(
            validate_frame(&f),
            Err(FrameReject::ImageDimensionsMismatch)
        );
        f.width = 1;
        f.rotation = 45;
        assert_eq!(validate_frame(&f), Err(FrameReject::InvalidRotation));
        f.rotation = 0;
        f.png_base64 = "not a png".into();
        assert_eq!(validate_frame(&f), Err(FrameReject::InvalidEncoding));
    }
}
