//! Standalone acceptance of the actual SDK backend, restricted to our own fixture.
use arkdock_core::ipc::model::CheckStatus;
use arkdock_core::preview::environment::{auto_detect_sdk, inspect_environment, EnvironmentReport};
use serde::Serialize;
use serde_json::{json, Value};
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const USAGE: &str = "arkdock-native-preview --probe [--sdk <root>]\narkdock-native-preview --sdk <root> --trust-fixture --output artifacts/native-preview/<fresh-directory>";

#[derive(Debug, PartialEq, Eq)]
struct Options {
    probe: bool,
    sdk: Option<PathBuf>,
    output: Option<PathBuf>,
}

fn parse_args(args: Vec<OsString>) -> Result<Options, String> {
    if args.len() > 8 || args.iter().any(|arg| arg.to_string_lossy().len() > 8192) {
        return Err("CLI_ARGUMENT_LIMIT".into());
    }
    let mut probe = false;
    let mut trust = false;
    let mut sdk = None;
    let mut output = None;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--probe") if !probe => probe = true,
            Some("--trust-fixture") if !trust => trust = true,
            Some("--sdk") if sdk.is_none() => sdk = Some(argument_path(&mut args)?),
            Some("--output") if output.is_none() => output = Some(argument_path(&mut args)?),
            _ => return Err("CLI_UNKNOWN_OR_DUPLICATE_ARGUMENT".into()),
        }
    }
    if probe {
        if trust || output.is_some() {
            return Err("PROBE_IS_READ_ONLY: trust and output flags are unavailable".into());
        }
    } else if !trust || sdk.is_none() || output.is_none() {
        return Err("EXECUTION_REQUIRES_SDK_TRUST_FIXTURE_AND_OUTPUT".into());
    }
    Ok(Options { probe, sdk, output })
}

fn argument_path(args: &mut impl Iterator<Item = OsString>) -> Result<PathBuf, String> {
    let value = args.next().ok_or("CLI_PATH_REQUIRED")?;
    if value.is_empty() || value.to_string_lossy().starts_with("--") {
        return Err("CLI_PATH_REQUIRED".into());
    }
    Ok(PathBuf::from(value))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Step {
    id: String,
    status: CheckStatus,
    duration_ms: u64,
    evidence: Value,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Report {
    schema_version: u32,
    status: CheckStatus,
    started_at: u64,
    finished_at: u64,
    command: Vec<String>,
    environment: EnvironmentReport,
    fixture: Option<String>,
    steps: Vec<Step>,
    artifacts: Vec<String>,
    backend_messages: Vec<String>,
    backend_metrics: Vec<Value>,
}

impl Report {
    fn new(environment: EnvironmentReport) -> Self {
        Self {
            schema_version: 1,
            status: CheckStatus::Skipped,
            started_at: now_ms(),
            finished_at: 0,
            command: std::env::args_os()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect(),
            environment,
            fixture: None,
            steps: (1..=7)
                .map(|number| Step {
                    id: format!("T{number}"),
                    status: CheckStatus::Skipped,
                    duration_ms: 0,
                    evidence: json!({ "reason": "Not reached" }),
                })
                .collect(),
            artifacts: Vec::new(),
            backend_messages: Vec::new(),
            backend_metrics: Vec::new(),
        }
    }

    fn finish(&mut self) -> i32 {
        self.finished_at = now_ms();
        if self
            .steps
            .iter()
            .any(|step| step.status == CheckStatus::Fail)
        {
            self.status = CheckStatus::Fail;
            1
        } else if self
            .steps
            .iter()
            .any(|step| step.status == CheckStatus::Blocked)
        {
            self.status = CheckStatus::Blocked;
            2
        } else if self
            .steps
            .iter()
            .all(|step| step.status == CheckStatus::Pass)
        {
            self.status = CheckStatus::Pass;
            0
        } else {
            self.status = CheckStatus::Fail;
            1
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

fn create_output(root: &Path, requested: &Path) -> Result<PathBuf, String> {
    let artifacts = root.join("artifacts");
    if !artifacts.exists() {
        fs::create_dir(&artifacts).map_err(|error| format!("ARTIFACT_DIRECTORY: {error}"))?;
    }
    let artifacts = artifacts
        .canonicalize()
        .map_err(|error| error.to_string())?;
    if !artifacts.starts_with(root) {
        return Err("ARTIFACT_DIRECTORY_OUTSIDE_WORKTREE".into());
    }
    let allowed = artifacts.join("native-preview");
    if !allowed.exists() {
        fs::create_dir(&allowed).map_err(|error| error.to_string())?;
    }
    let allowed = allowed.canonicalize().map_err(|error| error.to_string())?;
    if !allowed.starts_with(root) {
        return Err("ARTIFACT_DIRECTORY_OUTSIDE_WORKTREE".into());
    }
    let candidate = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        root.join(requested)
    };
    let parent = candidate
        .parent()
        .ok_or("OUTPUT_PARENT_REQUIRED")?
        .canonicalize()
        .map_err(|error| format!("OUTPUT_PARENT_REQUIRED: {error}"))?;
    if !parent.starts_with(&allowed) {
        return Err("OUTPUT_MUST_BE_UNDER_WORKTREE_ARTIFACTS_NATIVE_PREVIEW".into());
    }
    let output = parent.join(candidate.file_name().ok_or("OUTPUT_NAME_REQUIRED")?);
    fs::create_dir(&output).map_err(|error| format!("OUTPUT_MUST_BE_FRESH: {error}"))?;
    Ok(output)
}

fn write_artifact(path: &Path, data: &[u8]) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| format!("ARTIFACT_WRITE: {error}"))?;
    file.write_all(data)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("ARTIFACT_WRITE: {error}"))
}

fn main() {
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    if args == [OsString::from("--help")] {
        println!("{USAGE}");
        return;
    }
    let options = match parse_args(args) {
        Ok(options) => options,
        Err(error) => {
            eprintln!(
                "{}",
                json!({ "status": "FAIL", "error": error, "usage": USAGE })
            );
            std::process::exit(1);
        }
    };
    let diagnostic_sdk = if options.probe {
        options.sdk.clone().or_else(auto_detect_sdk)
    } else {
        options.sdk.clone()
    };
    let environment = inspect_environment(diagnostic_sdk.as_deref());
    if options.probe {
        println!(
            "{}",
            serde_json::to_string_pretty(&environment).expect("serializable environment report")
        );
        return;
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crate is inside repository")
        .canonicalize();
    let output = root.and_then(|root| {
        create_output(&root, options.output.as_deref().expect("validated output"))
            .map_err(std::io::Error::other)
    });
    let output = match output {
        Ok(output) => output,
        Err(error) => {
            eprintln!(
                "{}",
                json!({ "status": "FAIL", "error": error.to_string() })
            );
            std::process::exit(1);
        }
    };
    let mut report = Report::new(environment);
    #[cfg(windows)]
    native::execute(&options, &output, &mut report);
    #[cfg(not(windows))]
    {
        report.steps[0].status = CheckStatus::Blocked;
        report.steps[0].evidence =
            json!({ "reason": "UNSUPPORTED_PLATFORM: Windows interactive desktop required" });
    }
    let exit_code = report.finish();
    let bytes = serde_json::to_vec_pretty(&report).expect("serializable native report");
    if let Err(error) = write_artifact(&output.join("report.json"), &bytes) {
        eprintln!("{}", json!({ "status": "FAIL", "error": error }));
        std::process::exit(1);
    }
    println!("{}", String::from_utf8_lossy(&bytes));
    std::process::exit(exit_code);
}

#[cfg(windows)]
mod native {
    use super::*;
    use arkdock_core::ipc::model::{Environment, ProjectRef};
    use arkdock_core::preview::frames::{BinaryFrame, MAX_JPEG_PAYLOAD_BYTES};
    use arkdock_core::preview::{NativePoint, NativePreviewer, PreviewBackend, PreviewInput};
    use arkdock_core::scheduler::{
        watch_project, BuildCoordinator, BuildOutcome, CompletionDisposition, WatchEvent,
        WatchGuard,
    };
    use arkdock_core::workspace::fingerprint;
    use sha2::{Digest, Sha256};
    use std::io::{Read, Seek, SeekFrom};
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    };
    use std::thread;
    use std::time::{Duration, Instant};
    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT,
    };
    use windows_sys::Win32::System::Threading::{OpenProcess, WaitForSingleObject};

    #[link(name = "user32")]
    extern "system" {
        #[link_name = "OpenInputDesktop"]
        fn open_input_desktop(flags: u32, inherit: i32, access: u32) -> HANDLE;
        #[link_name = "CloseDesktop"]
        fn close_desktop(desktop: HANDLE) -> i32;
    }

    struct Failure {
        status: CheckStatus,
        message: String,
    }
    impl Failure {
        fn fail(message: impl Into<String>) -> Self {
            Self {
                status: CheckStatus::Fail,
                message: message.into(),
            }
        }
        fn blocked(message: impl Into<String>) -> Self {
            Self {
                status: CheckStatus::Blocked,
                message: message.into(),
            }
        }
    }
    fn io(error: impl std::fmt::Display) -> Failure {
        Failure::fail(error.to_string())
    }
    fn ensure(condition: bool, message: &str) -> Result<(), Failure> {
        if condition {
            Ok(())
        } else {
            Err(Failure::fail(message))
        }
    }
    fn step(
        report: &mut Report,
        index: usize,
        action: impl FnOnce(&mut Report) -> Result<Value, Failure>,
    ) -> bool {
        let start = Instant::now();
        let result = action(report);
        report.steps[index].duration_ms = start.elapsed().as_millis() as u64;
        match result {
            Ok(evidence) => {
                report.steps[index].status = CheckStatus::Pass;
                report.steps[index].evidence = evidence;
                true
            }
            Err(error) => {
                report.steps[index].status = error.status;
                report.steps[index].evidence = json!({ "reason": error.message });
                false
            }
        }
    }

    fn desktop_and_gui_absence() -> Result<Value, Failure> {
        unsafe {
            let desktop = open_input_desktop(0, 0, 1);
            if desktop.is_null() {
                return Err(Failure::blocked(format!(
                    "INTERACTIVE_DESKTOP_UNAVAILABLE: {}",
                    GetLastError()
                )));
            }
            close_desktop(desktop);
        }
        let system_root = std::env::var_os("SystemRoot")
            .ok_or_else(|| Failure::blocked("SYSTEM_ROOT_UNAVAILABLE"))?;
        let tasklist = PathBuf::from(system_root).join("System32/tasklist.exe");
        let mut queries = Vec::new();
        for filter in ["IMAGENAME eq deveco64.exe", "IMAGENAME eq deveco.exe"] {
            queries.push(query_tasklist(&tasklist, filter)?);
        }
        Ok(
            json!({ "interactiveDesktop": true, "devecoGuiAbsent": true, "checker": tasklist, "queries": queries }),
        )
    }

    fn query_tasklist(tasklist: &Path, filter: &str) -> Result<Value, Failure> {
        let mut child = Command::new(tasklist)
            .args(["/FI", filter, "/FO", "CSV", "/NH"])
            .creation_flags(0x0800_0000)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| Failure::blocked(format!("GUI_PROCESS_CHECK_UNAVAILABLE: {error}")))?;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) => {}
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(Failure::blocked(format!(
                        "GUI_PROCESS_CHECK_WAIT: {filter}; {error}"
                    )));
                }
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Failure::blocked(format!(
                    "GUI_PROCESS_CHECK_TIMEOUT: {filter}"
                )));
            }
            thread::sleep(Duration::from_millis(20));
        }
        let output = child.wait_with_output().map_err(io)?;
        if !output.status.success()
            || output.stdout.len() > 64 * 1024
            || output.stderr.len() > 64 * 1024
        {
            return Err(Failure::blocked(format!(
                "GUI_PROCESS_CHECK_FAILED: {filter}; exit={}; stdout={}; stderr={}",
                output.status,
                String::from_utf8_lossy(&output.stdout[..output.stdout.len().min(4096)]),
                String::from_utf8_lossy(&output.stderr[..output.stderr.len().min(4096)]),
            )));
        }
        let text = String::from_utf8_lossy(&output.stdout).to_ascii_lowercase();
        if text
            .lines()
            .any(|line| line.trim_start().starts_with("\"deveco"))
        {
            return Err(Failure::blocked(format!(
                "DEVECO_GUI_RUNNING: {filter}; close it before independent acceptance"
            )));
        }
        Ok(json!({ "filter": filter, "exitCode": output.status.code(), "guiAbsent": true }))
    }

    struct SourceRestore {
        path: PathBuf,
        bytes: Vec<u8>,
        restored: bool,
    }
    impl SourceRestore {
        fn restore(&mut self) -> Result<(), Failure> {
            if !self.restored {
                fs::write(&self.path, &self.bytes).map_err(io)?;
                self.restored = true;
            }
            ensure(
                fs::read(&self.path).map_err(io)? == self.bytes,
                "SOURCE_RESTORE_MISMATCH",
            )
        }
    }
    impl Drop for SourceRestore {
        fn drop(&mut self) {
            if !self.restored {
                if let Err(error) = fs::write(&self.path, &self.bytes) {
                    eprintln!("SOURCE_RESTORE_FAILED: {error}");
                }
            }
        }
    }

    fn read_source(path: &Path, fixture: &Path) -> Result<Vec<u8>, Failure> {
        let metadata = fs::symlink_metadata(path).map_err(io)?;
        ensure(
            metadata.is_file() && !metadata.file_type().is_symlink(),
            "FIXTURE_SOURCE_MUST_BE_REGULAR_FILE",
        )?;
        ensure(
            path.canonicalize().map_err(io)?.starts_with(fixture),
            "FIXTURE_SOURCE_OUTSIDE_PROJECT",
        )?;
        ensure(metadata.len() <= 1024 * 1024, "FIXTURE_SOURCE_LIMIT")?;
        let mut bytes = Vec::new();
        fs::File::open(path)
            .map_err(io)?
            .take(1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(io)?;
        ensure(bytes.len() <= 1024 * 1024, "FIXTURE_SOURCE_LIMIT")?;
        Ok(bytes)
    }
    struct OwnedProcess {
        pid: u32,
        handle: HANDLE,
    }
    impl Drop for OwnedProcess {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.handle);
            }
        }
    }
    enum Event {
        Frame(BinaryFrame),
        Failure(String),
    }
    struct Driver {
        backend: NativePreviewer,
        coordinator: BuildCoordinator,
        project: ProjectRef,
        config_fingerprint: String,
        source: SourceRestore,
        changed: Arc<AtomicBool>,
        watch_error: Arc<Mutex<Option<String>>>,
        watcher: Option<WatchGuard>,
        pending_frame: Option<BinaryFrame>,
        last_frame: Option<BinaryFrame>,
        current_log: Option<PathBuf>,
        processes: Vec<OwnedProcess>,
        overall_deadline: Instant,
        tasks_started: u64,
    }
    impl Driver {
        fn new(options: &Options, report: &mut Report) -> Result<Self, Failure> {
            let mut backend = NativePreviewer::new(options.sdk.clone());
            let capabilities = backend.probe();
            if !capabilities.available {
                return Err(Failure::blocked(capabilities.reason));
            }
            let root = Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .expect("repo root")
                .canonicalize()
                .map_err(io)?;
            let fixture = root
                .join("examples/native-counter")
                .canonicalize()
                .map_err(io)?;
            ensure(
                fixture.starts_with(&root),
                "FIXTURE_OUTSIDE_COMPILED_WORKTREE",
            )?;
            let project = ProjectRef {
                id: format!(
                    "fixture-{:x}",
                    Sha256::digest(fixture.to_string_lossy().as_bytes())
                ),
                name: "ArkDock standalone native counter".into(),
                environment: Environment::WindowsLocal,
                workspace_root: fixture.to_string_lossy().into_owned(),
                app_root: fixture.to_string_lossy().into_owned(),
                module: "entry".into(),
                variant: "default".into(),
                sdk_id: None,
            };
            let source_path = fixture.join("entry/src/main/ets/pages/Index.ets");
            let bytes = read_source(&source_path, &fixture)?;
            let config_fingerprint = fingerprint(&project).map_err(io)?;
            let mut coordinator = BuildCoordinator::new(200);
            coordinator.set_workspace(
                &project.id,
                &format!("standalone-{}-{}", std::process::id(), now_ms()),
                now_ms(),
            );
            coordinator.request_refresh(now_ms());
            report.fixture = Some(fixture.to_string_lossy().into_owned());
            Ok(Self {
                backend,
                coordinator,
                project,
                config_fingerprint,
                source: SourceRestore {
                    path: source_path,
                    bytes,
                    restored: false,
                },
                changed: Arc::new(AtomicBool::new(false)),
                watch_error: Arc::new(Mutex::new(None)),
                watcher: None,
                pending_frame: None,
                last_frame: None,
                current_log: None,
                processes: Vec::new(),
                overall_deadline: Instant::now() + Duration::from_secs(15 * 60),
                tasks_started: 0,
            })
        }
        fn messages(&mut self, report: &mut Report) -> Result<(), Failure> {
            for message in self.backend.runtime_messages() {
                if let Some((_, directory)) = message.rsplit_once("; logs ") {
                    self.current_log = Some(PathBuf::from(directory).join("previewer.log"));
                }
                if message.starts_with("BUILD_METRICS:") || message.starts_with("PREVIEW_METRICS:")
                {
                    let fields = message
                        .split_once(": ")
                        .map(|(_, fields)| fields)
                        .unwrap_or("");
                    let mut metrics = serde_json::Map::new();
                    for field in fields.split("; ") {
                        if let Some((key, value)) = field.split_once('=') {
                            metrics.insert(
                                key.into(),
                                value
                                    .parse::<u64>()
                                    .map_or_else(|_| json!(value), |number| json!(number)),
                            );
                            if key == "previewerPid" {
                                let pid = value.parse::<u32>().map_err(io)?;
                                let handle = unsafe { OpenProcess(0x0010_0000, 0, pid) };
                                if handle.is_null() {
                                    return Err(Failure::fail("OWNED_PROCESS_HANDLE_UNAVAILABLE"));
                                }
                                self.processes.push(OwnedProcess { pid, handle });
                            }
                        }
                    }
                    report.backend_metrics.push(
                        json!({ "buildId": self.coordinator.status().build_id, "fields": metrics }),
                    );
                }
                let failed = message.starts_with("RUNTIME_FAILED:");
                report.backend_messages.push(message.clone());
                if failed {
                    self.coordinator.runtime_failed(&message);
                    return Err(Failure::fail(message));
                }
            }
            Ok(())
        }
        fn poll(&mut self, report: &mut Report) -> Result<Option<Event>, Failure> {
            if Instant::now() >= self.overall_deadline {
                return Err(Failure::fail("OVERALL_TIMEOUT: 15 minutes"));
            }
            if let Some(error) = self.watch_error.lock().map_err(io)?.take() {
                return Err(Failure::fail(error));
            }
            if self.changed.swap(false, Ordering::AcqRel) {
                self.coordinator.request_refresh(now_ms());
            }
            if let Some(task) = self.coordinator.next_task(now_ms()) {
                ensure(
                    fingerprint(&self.project).map_err(io)? == self.config_fingerprint,
                    "TRUST_FINGERPRINT_CHANGED",
                )?;
                self.backend
                    .start(&self.project, true, &task.session_id)
                    .map_err(io)?;
                self.backend.refresh(&task).map_err(io)?;
                self.tasks_started += 1;
                self.pending_frame = None;
                self.current_log = None;
            }
            if let Some((task, result)) = self.backend.completion() {
                let outcome = result.map_or_else(BuildOutcome::Failure, |()| BuildOutcome::Success);
                let disposition = self.coordinator.complete(&task, outcome.clone(), now_ms());
                if disposition == CompletionDisposition::AcceptedFailure {
                    if let BuildOutcome::Failure(error) = outcome {
                        return Ok(Some(Event::Failure(error)));
                    }
                }
            }
            self.messages(report)?;
            if let Some(frame) = self.backend.binary_frame() {
                self.pending_frame = Some(frame);
            }
            if let Some(frame) = self.pending_frame.take() {
                if self.coordinator.accept_binary_frame(&frame).is_ok() {
                    self.last_frame = Some(frame.clone());
                    return Ok(Some(Event::Frame(frame)));
                }
                if self.coordinator.is_building()
                    && frame.metadata.generation == self.coordinator.status().generation
                {
                    self.pending_frame = Some(frame);
                }
            }
            Ok(None)
        }
        fn frame(&mut self, report: &mut Report) -> Result<BinaryFrame, Failure> {
            let deadline = (Instant::now() + Duration::from_secs(180)).min(self.overall_deadline);
            loop {
                match self.poll(report)? {
                    Some(Event::Frame(frame)) => return Ok(frame),
                    Some(Event::Failure(error)) => return Err(Failure::fail(error)),
                    None => {}
                }
                if Instant::now() >= deadline {
                    return Err(Failure::fail("BUILD_FIRST_FRAME_TIMEOUT: 180 seconds"));
                }
                thread::sleep(Duration::from_millis(20));
            }
        }
        fn log_contains(&self, marker: &str) -> Result<bool, Failure> {
            let Some(path) = &self.current_log else {
                return Ok(false);
            };
            let mut file = fs::File::open(path).map_err(io)?;
            let length = file.metadata().map_err(io)?.len();
            ensure(length <= 16 * 1024 * 1024, "RUNTIME_LOG_LIMIT")?;
            file.seek(SeekFrom::Start(length.saturating_sub(64 * 1024)))
                .map_err(io)?;
            let mut text = Vec::new();
            file.take(64 * 1024).read_to_end(&mut text).map_err(io)?;
            Ok(String::from_utf8_lossy(&text).contains(marker))
        }
        fn marker(&mut self, report: &mut Report, marker: &str) -> Result<(), Failure> {
            let deadline = (Instant::now() + Duration::from_secs(10)).min(self.overall_deadline);
            while !self.log_contains(marker)? {
                if let Some(Event::Failure(error)) = self.poll(report)? {
                    return Err(Failure::fail(error));
                }
                if Instant::now() >= deadline {
                    return Err(Failure::fail(format!(
                        "NATIVE_LOG_MARKER_TIMEOUT: {marker}"
                    )));
                }
                thread::sleep(Duration::from_millis(20));
            }
            Ok(())
        }
        fn watch(&mut self) -> Result<(), Failure> {
            let changed = self.changed.clone();
            let error = self.watch_error.clone();
            let source = self.source.path.clone();
            self.watcher = Some(
                watch_project(
                    Path::new(&self.project.app_root),
                    move |event| match event {
                        WatchEvent::Changed(paths) if paths.iter().any(|path| path == &source) => {
                            changed.store(true, Ordering::Release);
                        }
                        WatchEvent::Error(message) => {
                            if let Ok(mut error) = error.lock() {
                                *error = Some(message);
                            }
                        }
                        _ => {}
                    },
                )
                .map_err(io)?,
            );
            Ok(())
        }
        fn write_source(&mut self, bytes: &[u8]) -> Result<(), Failure> {
            self.changed.store(false, Ordering::Release);
            fs::write(&self.source.path, bytes).map_err(io)
        }
        fn wait_watch(&mut self) -> Result<(), Failure> {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !self.changed.load(Ordering::Acquire) {
                if let Some(error) = self.watch_error.lock().map_err(io)?.take() {
                    return Err(Failure::fail(error));
                }
                if Instant::now() >= deadline {
                    return Err(Failure::fail("REAL_SOURCE_WATCH_TIMEOUT"));
                }
                thread::sleep(Duration::from_millis(20));
            }
            Ok(())
        }
        fn save_frame(
            &self,
            output: &Path,
            label: &str,
            frame: &BinaryFrame,
            report: &mut Report,
        ) -> Result<Value, Failure> {
            let jpeg = output.join(format!("{label}.jpg"));
            let metadata = output.join(format!("{label}.metadata.json"));
            write_artifact(&jpeg, &frame.payload).map_err(io)?;
            write_artifact(
                &metadata,
                &serde_json::to_vec_pretty(&frame.metadata).map_err(io)?,
            )
            .map_err(io)?;
            report.artifacts.extend([
                jpeg.to_string_lossy().into_owned(),
                metadata.to_string_lossy().into_owned(),
            ]);
            Ok(
                json!({ "jpeg": jpeg, "metadata": metadata, "sha256": format!("{:x}", Sha256::digest(&frame.payload)), "runtimeLog": self.current_log, "generation": frame.metadata.generation, "buildId": frame.metadata.build_id, "jpegBytes": frame.payload.len() }),
            )
        }
        fn metrics(&self) -> Result<Value, Failure> {
            let metrics = self
                .backend
                .frame_metrics()
                .ok_or_else(|| Failure::fail("FRAME_METRICS_MISSING"))?;
            ensure(
                metrics.received > 0
                    && metrics.bytes_received > 0
                    && metrics.buffered_bytes <= MAX_JPEG_PAYLOAD_BYTES,
                "FRAME_METRICS_INVALID",
            )?;
            Ok(
                json!({ "received": metrics.received, "dropped": metrics.dropped, "rejected": metrics.rejected, "bytesReceived": metrics.bytes_received, "bufferedBytes": metrics.buffered_bytes, "tasksStarted": self.tasks_started }),
            )
        }
        fn stop(&mut self) -> Result<Value, Failure> {
            self.watcher.take();
            self.backend.stop();
            self.coordinator.stop();
            self.source.restore()?;
            ensure(
                self.backend.binary_frame().is_none(),
                "STOPPED_BACKEND_EMITTED_FRAME",
            )?;
            let deadline = Instant::now() + Duration::from_secs(5);
            for process in &self.processes {
                loop {
                    let wait = unsafe { WaitForSingleObject(process.handle, 0) };
                    if wait == WAIT_OBJECT_0 {
                        break;
                    }
                    ensure(wait == WAIT_TIMEOUT, "OWNED_PROCESS_WAIT_FAILED")?;
                    if Instant::now() >= deadline {
                        return Err(Failure::fail(format!(
                            "OWNED_PREVIEWER_SURVIVED_STOP: {}",
                            process.pid
                        )));
                    }
                    thread::sleep(Duration::from_millis(20));
                }
            }
            Ok(
                json!({ "sourceRestored": true, "stoppedFramesAbsent": true, "ownedPreviewerPids": self.processes.iter().map(|process| process.pid).collect::<Vec<_>>(), "identityProof": "SYNCHRONIZE handles opened on actual Previewer receipts before stop" }),
            )
        }
    }

    const COUNTER_REGION: (u32, u32, u32, u32) = (80, 260, 200, 52);
    const REVISION_REGION: (u32, u32, u32, u32) = (60, 400, 240, 50);

    fn fixture_region_hash(
        pixels: &image::RgbImage,
        bounds: (u32, u32, u32, u32),
    ) -> Result<String, Failure> {
        ensure(
            pixels.width() == 360 && pixels.height() == 640,
            "FIXTURE_FRAME_DIMENSIONS",
        )?;
        // Fixed fixture text regions exclude the button and each other.
        let region =
            image::imageops::crop_imm(pixels, bounds.0, bounds.1, bounds.2, bounds.3).to_image();
        Ok(format!("{:x}", Sha256::digest(region.as_raw())))
    }

    fn frame_region_sha256(
        frame: &BinaryFrame,
        bounds: (u32, u32, u32, u32),
    ) -> Result<String, Failure> {
        let mut reader = image::ImageReader::with_format(
            std::io::Cursor::new(&frame.payload),
            image::ImageFormat::Jpeg,
        );
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(360);
        limits.max_image_height = Some(640);
        limits.max_alloc = Some(4 * 1024 * 1024);
        reader.limits(limits);
        let image = reader.decode().map_err(io)?;
        fixture_region_hash(&image.to_rgb8(), bounds)
    }

    pub fn execute(options: &Options, output: &Path, report: &mut Report) {
        if !step(report, 0, |_| desktop_and_gui_absence()) {
            return;
        }
        let mut driver = match Driver::new(options, report) {
            Ok(driver) => driver,
            Err(error) => {
                report.steps[1].status = error.status;
                report.steps[1].evidence = json!({ "reason": error.message });
                return;
            }
        };
        let mut updated = Vec::new();
        let passed = step(report, 1, |report| {
            let frame = driver.frame(report)?;
            driver.marker(report, "ARKDOCK_NATIVE_REVISION=FIRST")?;
            ensure(
                driver.coordinator.status().displayed_generation == Some(frame.metadata.generation),
                "FIRST_FRAME_NOT_ACCEPTED_BY_COORDINATOR",
            )?;
            driver.save_frame(output, "first", &frame, report)
        }) && step(report, 2, |report| {
            let previous = frame_region_sha256(
                driver
                    .last_frame
                    .as_ref()
                    .ok_or_else(|| Failure::fail("CURRENT_NATIVE_FRAME_REQUIRED"))?,
                COUNTER_REGION,
            )?;
            let start = Instant::now();
            driver
                .backend
                .input(PreviewInput::Click(NativePoint { x: 180, y: 360 }))
                .map_err(io)?;
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                if let Some(Event::Failure(error)) = driver.poll(report)? {
                    return Err(Failure::fail(error));
                }
                if driver.log_contains("ARKDOCK_NATIVE_COUNT=1")? {
                    if let Some(frame) = driver.last_frame.as_ref() {
                        let current = frame_region_sha256(frame, COUNTER_REGION)?;
                        if current != previous {
                            let mut evidence =
                                driver.save_frame(output, "clicked", frame, report)?;
                            evidence["inputToChangedFrameMs"] =
                                json!(start.elapsed().as_millis() as u64);
                            evidence["counterRegionBefore"] = json!(previous);
                            evidence["counterRegionAfter"] = json!(current);
                            evidence["counterRegion"] =
                                json!({ "x": 80, "y": 260, "width": 200, "height": 52 });
                            return Ok(evidence);
                        }
                    }
                }
                if Instant::now() >= deadline {
                    return Err(Failure::fail("ACTUAL_ARKUI_CLICK_RESPONSE_TIMEOUT"));
                }
                thread::sleep(Duration::from_millis(20));
            }
        }) && step(report, 3, |report| {
            driver.watch()?;
            let original = std::str::from_utf8(&driver.source.bytes).map_err(io)?;
            ensure(
                original.contains("REVISION=FIRST") && original.contains("Source revision: FIRST"),
                "FIXTURE_REVISION_MARKERS_MISSING",
            )?;
            updated = original
                .replace("REVISION=FIRST", "REVISION=SECOND")
                .replace("Source revision: FIRST", "Source revision: SECOND")
                .into_bytes();
            let old = driver
                .last_frame
                .as_ref()
                .ok_or_else(|| Failure::fail("CURRENT_FRAME_REQUIRED"))?
                .clone();
            let revision_before = frame_region_sha256(&old, REVISION_REGION)?;
            let start = Instant::now();
            driver.write_source(&updated)?;
            driver.wait_watch()?;
            driver.frame(report)?;
            driver.marker(report, "ARKDOCK_NATIVE_REVISION=SECOND")?;
            let deadline = (Instant::now() + Duration::from_secs(10)).min(driver.overall_deadline);
            let (frame, revision_after) = loop {
                if let Some(frame) = driver.last_frame.as_ref() {
                    let current = frame_region_sha256(frame, REVISION_REGION)?;
                    if frame.metadata.generation > old.metadata.generation
                        && frame.metadata.build_id != old.metadata.build_id
                        && current != revision_before
                    {
                        break (frame.clone(), current);
                    }
                }
                if let Some(Event::Failure(error)) = driver.poll(report)? {
                    return Err(Failure::fail(error));
                }
                if Instant::now() >= deadline {
                    return Err(Failure::fail("SOURCE_REVISION_PICTURE_TIMEOUT"));
                }
                thread::sleep(Duration::from_millis(20));
            };
            let mut evidence = driver.save_frame(output, "source-updated", &frame, report)?;
            evidence["saveToChangedFrameMs"] = json!(start.elapsed().as_millis() as u64);
            evidence["actualOsWatcher"] = json!(true);
            evidence["revisionRegionBefore"] = json!(revision_before);
            evidence["revisionRegionAfter"] = json!(revision_after);
            evidence["revisionRegion"] = json!({ "x": 60, "y": 400, "width": 240, "height": 50 });
            Ok(evidence)
        }) && step(report, 4, |report| {
            let last_success = driver
                .last_frame
                .as_ref()
                .ok_or_else(|| Failure::fail("LAST_SUCCESS_REQUIRED"))?
                .clone();
            let mut invalid = updated.clone();
            invalid.extend_from_slice(b"\nconst ARKDOCK_INTENTIONAL_SYNTAX_ERROR = ;\n");
            driver.write_source(&invalid)?;
            driver.wait_watch()?;
            let deadline = (Instant::now() + Duration::from_secs(180)).min(driver.overall_deadline);
            let error = loop {
                if let Some(Event::Failure(error)) = driver.poll(report)? {
                    break error;
                }
                if Instant::now() >= deadline {
                    return Err(Failure::fail("REAL_COMPILE_FAILURE_TIMEOUT"));
                }
                thread::sleep(Duration::from_millis(20));
            };
            let status = driver.coordinator.status();
            ensure(
                error.contains("BUILD_FAILED")
                    && status.stale
                    && status.displayed_generation == Some(last_success.metadata.generation)
                    && driver.last_frame.as_ref() == Some(&last_success),
                "COMPILE_FAILURE_DID_NOT_RETAIN_STALE_LAST_SUCCESS",
            )?;
            driver.write_source(&updated)?;
            driver.wait_watch()?;
            let frame = driver.frame(report)?;
            driver.marker(report, "ARKDOCK_NATIVE_REVISION=SECOND")?;
            ensure(
                !driver.coordinator.status().stale
                    && frame.metadata.generation > last_success.metadata.generation,
                "SOURCE_RECOVERY_DID_NOT_CLEAR_STALE_STATE",
            )?;
            let mut evidence = driver.save_frame(output, "recovered", &frame, report)?;
            evidence["expectedBuildFailure"] = json!(error);
            evidence["staleLastSuccessRetained"] = json!(true);
            Ok(evidence)
        });
        if passed {
            // Give the actual native producer an opportunity to replace unread frames.
            for _ in 0..10 {
                thread::sleep(Duration::from_millis(20));
            }
            step(report, 6, |report| {
                driver.messages(report)?;
                ensure(
                    report
                        .backend_metrics
                        .iter()
                        .any(|entry| entry["fields"]["buildMs"].is_number())
                        && report
                            .backend_metrics
                            .iter()
                            .any(|entry| entry["fields"]["launchToFirstFrameMs"].is_number()),
                    "ACTUAL_BACKEND_TIMINGS_MISSING",
                )?;
                driver.metrics()
            });
        }
        // Cleanup and exact source restoration run even after any failed assertion.
        step(report, 5, |_| driver.stop());
    }

    #[cfg(test)]
    mod gui_check_tests {
        use super::*;

        #[test]
        fn counter_region_ignores_button_feedback_but_detects_count_pixels() {
            // Synthetic image tests this assertion only, never ArkUI/native evidence.
            let mut pixels = image::RgbImage::from_pixel(360, 640, image::Rgb([255, 255, 255]));
            let before = fixture_region_hash(&pixels, COUNTER_REGION)
                .unwrap_or_else(|error| panic!("{}", error.message));
            pixels.put_pixel(180, 360, image::Rgb([20, 100, 240]));
            assert_eq!(
                fixture_region_hash(&pixels, COUNTER_REGION)
                    .unwrap_or_else(|error| panic!("{}", error.message)),
                before
            );
            pixels.put_pixel(180, 280, image::Rgb([20, 100, 240]));
            assert_ne!(
                fixture_region_hash(&pixels, COUNTER_REGION)
                    .unwrap_or_else(|error| panic!("{}", error.message)),
                before
            );
        }

        #[test]
        fn revision_region_ignores_counter_reset_but_detects_source_label_pixels() {
            // Synthetic assertion test only; real source refresh requires the SDK run.
            let mut pixels = image::RgbImage::from_pixel(360, 640, image::Rgb([255, 255, 255]));
            let before = fixture_region_hash(&pixels, REVISION_REGION)
                .unwrap_or_else(|error| panic!("{}", error.message));
            pixels.put_pixel(180, 280, image::Rgb([20, 100, 240]));
            assert_eq!(
                fixture_region_hash(&pixels, REVISION_REGION)
                    .unwrap_or_else(|error| panic!("{}", error.message)),
                before
            );
            pixels.put_pixel(180, 426, image::Rgb([20, 100, 240]));
            assert_ne!(
                fixture_region_hash(&pixels, REVISION_REGION)
                    .unwrap_or_else(|error| panic!("{}", error.message)),
                before
            );
        }

        fn tasklist() -> PathBuf {
            PathBuf::from(std::env::var_os("SystemRoot").expect("Windows SystemRoot"))
                .join("System32/tasklist.exe")
        }

        #[test]
        fn actual_tasklist_accepts_both_exact_deveco_process_filters() {
            for filter in ["IMAGENAME eq deveco64.exe", "IMAGENAME eq deveco.exe"] {
                match query_tasklist(&tasklist(), filter) {
                    Ok(evidence) => assert_eq!(evidence["exitCode"], 0),
                    Err(error) => assert!(
                        error.message.starts_with("DEVECO_GUI_RUNNING:"),
                        "{}",
                        error.message
                    ),
                }
            }
        }

        #[test]
        fn actual_tasklist_filter_error_retains_filter_exit_and_diagnostics() {
            let filter = "INVALID_ARKDOCK_FILTER";
            let error = query_tasklist(&tasklist(), filter).expect_err("invalid filter rejected");
            assert!(error.message.contains(filter));
            assert!(error.message.contains("exit="));
            assert!(error.message.contains("stdout="));
            assert!(error.message.contains("stderr="));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }
    #[test]
    fn execution_requires_explicit_fixture_trust_and_all_paths() {
        for invalid in [
            &[][..],
            &["--sdk", "sdk", "--output", "artifacts/native-preview/run"],
            &["--trust-fixture", "--sdk", "sdk"],
            &["--trust-fixture", "--output", "run"],
        ] {
            assert!(parse_args(args(invalid)).is_err());
        }
        let options = parse_args(args(&[
            "--sdk",
            "C:/legal sdk",
            "--trust-fixture",
            "--output",
            "artifacts/native-preview/run",
        ]))
        .unwrap();
        assert!(!options.probe);
        assert_eq!(options.sdk, Some(PathBuf::from("C:/legal sdk")));
    }
    #[test]
    fn probe_cannot_mutate_or_grant_trust() {
        assert!(parse_args(args(&["--probe"])).unwrap().probe);
        assert!(
            parse_args(args(&["--probe", "--sdk", "sdk"]))
                .unwrap()
                .probe
        );
        assert!(parse_args(args(&["--probe", "--trust-fixture"])).is_err());
        assert!(parse_args(args(&["--probe", "--output", "run"])).is_err());
    }
    #[test]
    fn arbitrary_projects_unknown_flags_and_duplicates_are_rejected() {
        for invalid in [
            &["--project", "other"][..],
            &["--probe", "--sdk"],
            &["--probe", "--sdk", "--probe"],
            &["--probe", "--probe"],
            &["--probe", "--sdk", "a", "--sdk", "b"],
        ] {
            assert!(parse_args(args(invalid)).is_err());
        }
        assert!(parse_args(vec![OsString::from("x".repeat(8193))]).is_err());
    }
    #[test]
    fn output_is_fresh_and_contained_in_owned_artifact_directory() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        assert!(create_output(&root, Path::new("outside")).is_err());
        let requested = Path::new("artifacts/native-preview/first-run");
        let output = create_output(&root, requested).unwrap();
        assert!(output.starts_with(&root));
        assert!(create_output(&root, requested).is_err());
        assert!(create_output(&root, Path::new("artifacts/native-preview/../../escaped")).is_err());
    }
}
