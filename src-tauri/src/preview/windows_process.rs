//! Atomic containment: a child is suspended until assigned to our kill-on-close Job.
//!
//! Child stdout and stderr are inherited as the write end of an anonymous pipe
//! that ArkDock drains continuously into a bounded sink. Capturing output this
//! way is what keeps the runtime log safe under a chatty child:
//!
//! * the sink retains a bounded window in memory and a bounded segment on disk,
//!   dropping the oldest output instead of growing without limit;
//! * the reader never waits for disk capture: its bounded non-waiting queue
//!   degrades on pressure, while memory capture and error detection continue;
//! * a log path that cannot be written degrades capture to memory only, and a
//!   write that fails mid-run stops using the file; neither invalidates a run.
//!
//! Previewer health is decided by exit status, ArkTS errors and protocol. The
//! build separately retains its hard output limit and waits for EOF after exit.
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::windows::io::{AsRawHandle, FromRawHandle, RawHandle};
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::System::JobObjects::*;
use windows_sys::Win32::System::Pipes::{CreatePipe, PeekNamedPipe};
use windows_sys::Win32::System::Threading::*;

const READ_BUFFER: usize = 16 * 1024;
/// Written by ArkDock itself when a segment is rotated. It deliberately matches
/// no SDK receipt, so a rotated build log still reports its own graph result.
const ROTATE_NOTICE: &str = "[ArkDock] log rotated: older output dropped";
const PAGE_ERROR_MARKER: &str = "[Engine Log]Error message:";
const DISK_QUEUE_CHUNKS: usize = 16;
const DISK_WORKER_LIMIT: usize = 2;
static DISK_WORKERS: AtomicUsize = AtomicUsize::new(0);
const READER_WORKER_LIMIT: usize = 8;
static READER_WORKERS: AtomicUsize = AtomicUsize::new(0);

struct ReaderPermit;
impl ReaderPermit {
    fn acquire() -> Result<Self, String> {
        READER_WORKERS
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < READER_WORKER_LIMIT).then_some(count + 1)
            })
            .map(|_| Self)
            .map_err(|_| "LOG_READER_LIMIT: prior captures have not stopped".into())
    }
}
impl Drop for ReaderPermit {
    fn drop(&mut self) {
        READER_WORKERS.fetch_sub(1, Ordering::AcqRel);
    }
}

/// A stalled filesystem call cannot be cancelled safely by Rust. Bound these
/// calls globally, including workers detached by a prior run, rather than
/// creating another thread and file handle for every retry.
struct DiskPermit;
impl DiskPermit {
    fn acquire() -> Option<Self> {
        DISK_WORKERS
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < DISK_WORKER_LIMIT).then_some(count + 1)
            })
            .ok()
            .map(|_| Self)
    }
}
impl Drop for DiskPermit {
    fn drop(&mut self) {
        DISK_WORKERS.fetch_sub(1, Ordering::AcqRel);
    }
}

#[derive(Default)]
struct DiskStatus {
    error: Mutex<Option<String>>,
    cancelled: AtomicBool,
    initialized: AtomicBool,
    finished: AtomicBool,
    pending: AtomicUsize,
}
impl DiskStatus {
    fn degrade(&self, error: String) {
        let mut diagnostic = self.error.lock().unwrap_or_else(PoisonError::into_inner);
        if diagnostic.is_none() {
            *diagnostic = Some(error);
        }
        self.cancelled.store(true, Ordering::Release);
    }
}

struct DiskCapture {
    sender: SyncSender<Vec<u8>>,
    status: Arc<DiskStatus>,
    worker: Option<JoinHandle<()>>,
}
impl DiskCapture {
    #[cfg(test)]
    fn disabled(error: String) -> Self {
        let (sender, receiver) = sync_channel(DISK_QUEUE_CHUNKS);
        drop(receiver);
        let status = Arc::new(DiskStatus::default());
        status.degrade(error);
        status.initialized.store(true, Ordering::Release);
        status.finished.store(true, Ordering::Release);
        Self {
            sender,
            status,
            worker: None,
        }
    }

    fn start(
        factory: impl FnOnce() -> Result<Box<dyn Segment>, String> + Send + 'static,
        limits: LogLimits,
    ) -> Self {
        let (sender, receiver) = sync_channel::<Vec<u8>>(DISK_QUEUE_CHUNKS);
        let status = Arc::new(DiskStatus::default());
        let worker = DiskPermit::acquire().map(|permit| {
            let status = status.clone();
            thread::spawn(move || {
                let _permit = permit;
                let segment = factory(); // All filesystem calls run only here.
                match segment {
                    Err(error) => {
                        status.degrade(error);
                        status.initialized.store(true, Ordering::Release);
                    }
                    Ok(mut segment) => {
                        status.initialized.store(true, Ordering::Release);
                        let mut on_disk = 0u64;
                        let mut window = Vec::with_capacity(limits.window);
                        while !status.cancelled.load(Ordering::Acquire) {
                            let bytes = match receiver.recv_timeout(Duration::from_millis(10)) {
                                Ok(bytes) => bytes,
                                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                                Err(_) => break,
                            };
                            // Trim before extending: even the Vec allocation
                            // cannot temporarily grow beyond the window budget.
                            if bytes.len() >= limits.window {
                                window.clear();
                                window.extend_from_slice(&bytes[bytes.len() - limits.window..]);
                            } else {
                                let excess =
                                    (window.len() + bytes.len()).saturating_sub(limits.window);
                                window.drain(..excess);
                                window.extend_from_slice(&bytes);
                            }
                            let outcome =
                                if on_disk.saturating_add(bytes.len() as u64) <= limits.budget {
                                    segment
                                        .append(&bytes)
                                        .map(|()| on_disk + bytes.len() as u64)
                                } else {
                                    let notice = format!("{ROTATE_NOTICE}\n");
                                    let notice = &notice.as_bytes()
                                        [..notice.len().min(limits.budget as usize)];
                                    let keep = window
                                        .len()
                                        .min((limits.budget as usize).saturating_sub(notice.len()));
                                    let tail = &window[window.len() - keep..];
                                    segment
                                        .rewrite(std::str::from_utf8(notice).unwrap_or(""), tail)
                                        .map(|()| (notice.len() + tail.len()) as u64)
                                };
                            match outcome {
                                Ok(written) => on_disk = written,
                                Err(error) => {
                                    status.degrade(error);
                                }
                            }
                            status.pending.fetch_sub(1, Ordering::AcqRel);
                        }
                    }
                }
                // Discard any queued chunks on cancellation. Neither the worker
                // nor its receiver owns Log or the sender, so it can always close.
                drop(receiver);
                status.finished.store(true, Ordering::Release);
            })
        });
        if worker.is_none() {
            status.degrade("LOG_WORKER_LIMIT: disk capture slots unavailable".into());
            status.initialized.store(true, Ordering::Release);
            status.finished.store(true, Ordering::Release);
        }
        Self {
            sender,
            status,
            worker,
        }
    }

    fn append(&self, bytes: &[u8]) {
        for chunk in bytes.chunks(READ_BUFFER) {
            if self.status.cancelled.load(Ordering::Acquire) {
                break;
            }
            self.status.pending.fetch_add(1, Ordering::AcqRel);
            match self.sender.try_send(chunk.to_vec()) {
                Ok(()) => {}
                Err(error) => {
                    self.status.pending.fetch_sub(1, Ordering::AcqRel);
                    self.status.degrade(match error {
                        TrySendError::Full(_) => "LOG_QUEUE_PRESSURE: disk capture disabled".into(),
                        TrySendError::Disconnected(_) => {
                            "LOG_WORKER_CLOSED: disk capture disabled".into()
                        }
                    });
                    break;
                }
            }
        }
    }
}
impl Drop for DiskCapture {
    fn drop(&mut self) {
        self.status.cancelled.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let deadline = Instant::now() + Duration::from_millis(100);
            while !worker.is_finished() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(1));
            }
            if worker.is_finished() {
                let _ = worker.join();
            }
            // A blocked disk call is detached, but retains its global permit:
            // at most two workers/file handles can survive shutdown or retries.
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct LogLimits {
    /// Bytes retained in memory after compaction. The buffer may hold up to
    /// twice this between compactions, which is the memory bound.
    pub window: usize,
    /// Bytes kept on disk per child. The documented 16 MiB log limit is this
    /// budget: exceeding it rotates output instead of failing the run.
    pub budget: u64,
}
impl Default for LogLimits {
    fn default() -> Self {
        Self {
            window: 1024 * 1024,
            budget: 16 * 1024 * 1024,
        }
    }
}

/// On-disk part of the sink. Production appends to a file; tests inject a
/// segment that fails so degradation is proven without a real disk fault.
trait Segment: Send {
    fn append(&mut self, bytes: &[u8]) -> Result<(), String>;
    fn rewrite(&mut self, notice: &str, tail: &[u8]) -> Result<(), String>;
}

struct FileSegment {
    file: File,
    path: PathBuf,
}
impl Segment for FileSegment {
    fn append(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.file
            .write_all(bytes)
            .map_err(|error| format!("LOG_WRITE: {error}"))
    }
    fn rewrite(&mut self, notice: &str, tail: &[u8]) -> Result<(), String> {
        // The sink owns this handle, so truncating and refilling it is safe:
        // nothing else, including the child, writes to the log file.
        self.file
            .set_len(0)
            .and_then(|()| self.file.seek(SeekFrom::Start(0)).map(|_| ()))
            .and_then(|()| self.file.write_all(notice.as_bytes()))
            .and_then(|()| self.file.write_all(tail))
            .map_err(|error| format!("LOG_ROTATE {}: {error}", self.path.display()))
    }
}

struct LogState {
    window: Vec<u8>,
    produced: u64,
    dropped: u64,
    error_scan: String,
    runtime_error: Option<String>,
    limits: LogLimits,
}

impl LogState {
    fn append(&mut self, bytes: &[u8]) {
        self.produced = self.produced.saturating_add(bytes.len() as u64);
        // Page errors are recognised as they arrive, so a later burst can never
        // push a fatal marker out of a bounded window before it is observed.
        self.scan_page_error(bytes);
        if self.window.len().saturating_add(bytes.len()) >= 2 * self.limits.window {
            let retained = self.limits.window.min(bytes.len());
            let keep_old = self.limits.window - retained;
            let excess = self.window.len().saturating_sub(keep_old);
            self.window.drain(..excess);
            self.dropped = self
                .dropped
                .saturating_add((excess + bytes.len() - retained) as u64);
            self.window
                .extend_from_slice(&bytes[bytes.len() - retained..]);
        } else {
            self.window.extend_from_slice(bytes);
        }
    }

    fn scan_page_error(&mut self, bytes: &[u8]) {
        if self.runtime_error.is_some() {
            return;
        }
        self.error_scan.push_str(&String::from_utf8_lossy(bytes));
        if let Some(index) = self.error_scan.find(PAGE_ERROR_MARKER) {
            self.runtime_error = Some(format!(
                "ARKTS_RUNTIME_ERROR: {}",
                self.error_scan[index..]
                    .lines()
                    .next()
                    .unwrap_or("ArkTS page exception")
            ));
            self.error_scan.clear();
            return;
        }
        // Preserve a split marker without keeping application logs in memory.
        if self.error_scan.len() > 1024 {
            let mut start = self.error_scan.len() - 1024;
            while !self.error_scan.is_char_boundary(start) {
                start += 1;
            }
            self.error_scan = self.error_scan[start..].to_owned();
        }
    }
}

/// Bounded capture of one child's output. Cheap to clone: every clone shares the
/// same sink, which is how the reader thread and the owner see the same state.
#[derive(Clone)]
pub(super) struct Log {
    state: Arc<Mutex<LogState>>,
    finished: Arc<AtomicBool>,
    cancelled: Arc<AtomicBool>,
    drain_error: Arc<Mutex<Option<String>>>,
    disk: Arc<DiskCapture>,
}

fn lock(state: &Mutex<LogState>) -> MutexGuard<'_, LogState> {
    // A panicking consumer must never stop the drain thread from reading.
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Log {
    fn open(path: &Path, limits: LogLimits) -> Self {
        let path = path.to_path_buf();
        Self::with_disk(
            DiskCapture::start(
                move || {
                    File::create(&path)
                        .map(|file| Box::new(FileSegment { file, path }) as Box<dyn Segment>)
                        .map_err(|error| format!("LOG_OPEN: {error}"))
                },
                limits,
            ),
            limits,
        )
    }

    #[cfg(test)]
    fn with_segment(
        segment: Option<Box<dyn Segment>>,
        file_error: Option<String>,
        limits: LogLimits,
    ) -> Self {
        if segment.is_none() {
            return Self::with_disk(
                DiskCapture::disabled(file_error.unwrap_or_else(|| "LOG_MEMORY_ONLY".into())),
                limits,
            );
        }
        Self::with_disk(
            DiskCapture::start(
                move || {
                    segment.ok_or_else(|| file_error.unwrap_or_else(|| "LOG_MEMORY_ONLY".into()))
                },
                limits,
            ),
            limits,
        )
    }

    fn with_disk(disk: DiskCapture, limits: LogLimits) -> Self {
        Self {
            state: Arc::new(Mutex::new(LogState {
                window: Vec::with_capacity(2 * limits.window),
                produced: 0,
                dropped: 0,
                error_scan: String::new(),
                runtime_error: None,
                limits,
            })),
            finished: Arc::new(AtomicBool::new(false)),
            cancelled: Arc::new(AtomicBool::new(false)),
            drain_error: Arc::new(Mutex::new(None)),
            disk: Arc::new(disk),
        }
    }

    fn append(&self, bytes: &[u8]) {
        lock(&self.state).append(bytes);
        // try_send never waits for disk or holds the health/window mutex.
        self.disk.append(bytes);
    }

    fn mark_finished(&self) {
        self.finished.store(true, Ordering::Release);
    }

    /// Total bytes the child has emitted, whether or not they were retained.
    pub fn produced(&self) -> u64 {
        lock(&self.state).produced
    }

    /// Bytes evicted from the memory window because the child outran the bound.
    pub fn dropped(&self) -> u64 {
        lock(&self.state).dropped
    }

    /// The newest bytes, as diagnostics. Lossy decoding keeps every byte.
    pub fn tail(&self, limit: usize) -> String {
        let state = lock(&self.state);
        let start = state.window.len().saturating_sub(limit);
        String::from_utf8_lossy(&state.window[start..]).into_owned()
    }

    /// The first ArkTS page exception seen in this child's output.
    pub fn runtime_error(&self) -> Option<String> {
        lock(&self.state).runtime_error.clone()
    }

    /// Why on-disk capture degraded, if it did. Never a failure of the run.
    pub fn file_error(&self) -> Option<String> {
        self.disk
            .status
            .error
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Whether draining has stopped, i.e. the child and every inherited handle
    /// are gone. Used to prove the reader thread is not leaked.
    #[cfg(test)]
    pub fn finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }
}

pub(super) struct Process {
    job: HANDLE,
    process: HANDLE,
    pub pid: u32,
    log: Log,
    reader: Option<JoinHandle<()>>,
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

/// CRT/CommandLineToArgvW-compatible quoting; no shell expansion takes place.
fn quote(arg: &str) -> String {
    let mut result = String::from("\"");
    let mut slashes = 0;
    for ch in arg.chars() {
        if ch == '\\' {
            slashes += 1;
        } else {
            result.push_str(&"\\".repeat(slashes * if ch == '"' { 2 } else { 1 }));
            slashes = 0;
            if ch == '"' {
                result.push('\\');
            }
            result.push(ch);
        }
    }
    result.push_str(&"\\".repeat(slashes * 2));
    result.push('"');
    result
}

/// Polling an app-owned pipe avoids an uncancellable blocking read. There is
/// one reader, so reading only the bytes reported by PeekNamedPipe cannot wait
/// for the child to produce more. The owning File closes on every exit path.
fn spawn_reader(handle: HANDLE, log: Log, permit: ReaderPermit) -> JoinHandle<()> {
    let mut reader = unsafe { File::from_raw_handle(handle as RawHandle) };
    thread::spawn(move || {
        let _permit = permit;
        let mut buffer = [0u8; READ_BUFFER];
        loop {
            if log.cancelled.load(Ordering::Acquire) {
                *log.drain_error
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner) = Some("LOG_DRAIN_CANCELLED".into());
                break;
            }
            let mut available = 0;
            let peeked = unsafe {
                PeekNamedPipe(
                    reader.as_raw_handle() as HANDLE,
                    null_mut(),
                    0,
                    null_mut(),
                    &mut available,
                    null_mut(),
                )
            };
            if peeked == 0 {
                let error = std::io::Error::last_os_error();
                if error.raw_os_error() != Some(ERROR_BROKEN_PIPE as i32) {
                    *log.drain_error
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner) =
                        Some(format!("LOG_DRAIN_READ: {error}"));
                }
                break;
            }
            if available == 0 {
                thread::sleep(Duration::from_millis(2));
                continue;
            }
            let count = buffer.len().min(available as usize);
            match reader.read(&mut buffer[..count]) {
                Ok(0) => break,
                Ok(count) => log.append(&buffer[..count]),
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    *log.drain_error
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner) =
                        Some(format!("LOG_DRAIN_READ: {error}"));
                    break;
                }
            }
        }
        log.mark_finished();
    })
}

impl Process {
    pub fn spawn(
        executable: &Path,
        args: &[String],
        cwd: &Path,
        env: &[(String, String)],
        log_path: &Path,
    ) -> Result<Self, String> {
        Self::spawn_with(executable, args, cwd, env, log_path, LogLimits::default())
    }

    pub(super) fn spawn_with(
        executable: &Path,
        args: &[String],
        cwd: &Path,
        env: &[(String, String)],
        log_path: &Path,
        limits: LogLimits,
    ) -> Result<Self, String> {
        Self::spawn_logged(executable, args, cwd, env, Log::open(log_path, limits))
    }

    fn spawn_logged(
        executable: &Path,
        args: &[String],
        cwd: &Path,
        env: &[(String, String)],
        log: Log,
    ) -> Result<Self, String> {
        if args.iter().any(|arg| arg.contains('\0'))
            || env
                .iter()
                .any(|(key, value)| key.contains(['\0', '=']) || value.contains('\0'))
        {
            return Err("INVALID_PROCESS_ARGUMENT".into());
        }
        let reader_permit = ReaderPermit::acquire()?;
        let mut result = Self {
            job: null_mut(),
            process: null_mut(),
            pid: 0,
            log,
            reader: None,
        };
        unsafe {
            result.job = CreateJobObjectW(null(), null());
            if result.job.is_null() {
                return Err(format!("JOB_CREATE: {}", std::io::Error::last_os_error()));
            }
            let mut job_limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            job_limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                result.job,
                JobObjectExtendedLimitInformation,
                (&job_limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of_val(&job_limits) as u32,
            ) == 0
            {
                return Err(format!("JOB_LIMIT: {}", std::io::Error::last_os_error()));
            }
            let mut read_end: HANDLE = null_mut();
            let mut write_end: HANDLE = null_mut();
            let inherit = SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: null_mut(),
                bInheritHandle: 1,
            };
            if CreatePipe(&mut read_end, &mut write_end, &inherit, 0) == 0 {
                return Err(format!("PROCESS_PIPE: {}", std::io::Error::last_os_error()));
            }
            // Only the child's output handle may be inherited; the read end stays
            // private so the pipe closes as soon as the child's copies are gone.
            if SetHandleInformation(read_end, HANDLE_FLAG_INHERIT, 0) == 0 {
                CloseHandle(read_end);
                CloseHandle(write_end);
                return Err(format!(
                    "PROCESS_PIPE_HANDLE: {}",
                    std::io::Error::last_os_error()
                ));
            }
            let mut bytes = 0;
            InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut bytes);
            let words = bytes.div_ceil(std::mem::size_of::<usize>());
            let mut attributes = vec![0usize; words];
            let list = attributes.as_mut_ptr().cast();
            if InitializeProcThreadAttributeList(list, 1, 0, &mut bytes) == 0 {
                CloseHandle(read_end);
                CloseHandle(write_end);
                return Err("PROCESS_ATTRIBUTE_LIST".into());
            }
            let updated = UpdateProcThreadAttribute(
                list,
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                (&write_end as *const HANDLE).cast(),
                std::mem::size_of::<HANDLE>(),
                null_mut(),
                null_mut(),
            );
            let mut startup: STARTUPINFOEXW = std::mem::zeroed();
            startup.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
            startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
            startup.StartupInfo.hStdOutput = write_end;
            startup.StartupInfo.hStdError = write_end;
            startup.lpAttributeList = list;
            let application = wide(&executable.to_string_lossy());
            let mut command = wide(
                &std::iter::once(executable.to_string_lossy().into_owned())
                    .chain(args.iter().cloned())
                    .map(|arg| quote(&arg))
                    .collect::<Vec<_>>()
                    .join(" "),
            );
            let directory = wide(&cwd.to_string_lossy());
            let mut variables = std::env::vars()
                .filter(|(key, _)| {
                    !env.iter()
                        .any(|(override_key, _)| key.eq_ignore_ascii_case(override_key))
                })
                .chain(env.iter().cloned())
                .collect::<Vec<_>>();
            variables.sort_by_key(|(key, _)| key.to_ascii_uppercase());
            let mut environment: Vec<u16> = variables
                .iter()
                .flat_map(|(key, value)| wide(&format!("{key}={value}")))
                .collect();
            environment.push(0);
            let mut info: PROCESS_INFORMATION = std::mem::zeroed();
            let created = updated != 0
                && CreateProcessW(
                    application.as_ptr(),
                    command.as_mut_ptr(),
                    null(),
                    null(),
                    1,
                    CREATE_SUSPENDED
                        | CREATE_NO_WINDOW
                        | CREATE_UNICODE_ENVIRONMENT
                        | EXTENDED_STARTUPINFO_PRESENT,
                    environment.as_ptr().cast(),
                    directory.as_ptr(),
                    &startup.StartupInfo,
                    &mut info,
                ) != 0;
            let error = std::io::Error::last_os_error();
            DeleteProcThreadAttributeList(list);
            // The child owns the only live write end from here on, which is what
            // makes the reader's end-of-file observable.
            CloseHandle(write_end);
            if !created {
                CloseHandle(read_end);
                return Err(format!("PROCESS_START: {error}"));
            }
            result.process = info.hProcess;
            result.pid = info.dwProcessId;
            if AssignProcessToJobObject(result.job, result.process) == 0 {
                TerminateProcess(result.process, 1);
                CloseHandle(info.hThread);
                CloseHandle(read_end);
                return Err(format!("JOB_ASSIGN: {}", std::io::Error::last_os_error()));
            }
            let resumed = ResumeThread(info.hThread);
            CloseHandle(info.hThread);
            if resumed == u32::MAX {
                CloseHandle(read_end);
                return Err("PROCESS_RESUME".into());
            }
            result.reader = Some(spawn_reader(read_end, result.log.clone(), reader_permit));
        }
        Ok(result)
    }

    pub fn exit_code(&self) -> Result<Option<u32>, String> {
        unsafe {
            if WaitForSingleObject(self.process, 0) == WAIT_TIMEOUT {
                return Ok(None);
            }
            let mut code = 0;
            if GetExitCodeProcess(self.process, &mut code) == 0 {
                return Err("PROCESS_EXIT_STATUS".into());
            }
            Ok(Some(code))
        }
    }

    /// Process exit is not EOF: wait for all buffered stdout/stderr to enter the
    /// memory sink before using its receipt. Disk capture is deliberately not a
    /// dependency of this barrier. Descendants retaining handles time out closed.
    pub fn drain_output(&self, timeout: Duration, stop: &AtomicBool) -> Result<(), String> {
        let deadline = Instant::now() + timeout;
        loop {
            if stop.load(Ordering::Acquire) {
                return Err("LOG_DRAIN_CANCELLED".into());
            }
            if let Some(error) = self.capture_failure() {
                return Err(error);
            }
            if self.log.finished.load(Ordering::Acquire) {
                // Acquire the completion publication before the final error
                // read: the reader can fail between our early check and EOF.
                return self.capture_failure().map_or(Ok(()), Err);
            }
            if Instant::now() >= deadline {
                return Err("LOG_DRAIN_TIMEOUT".into());
            }
            thread::sleep(Duration::from_millis(2));
        }
    }

    /// Read failures are fatal even while the child is still running: once the
    /// pipe stops being observed we cannot prove future ArkTS health. This is
    /// separate from optional disk capture degradation.
    pub fn capture_failure(&self) -> Option<String> {
        self.log
            .drain_error
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub fn check_capture_health(&self) -> Result<(), String> {
        if let Some(error) = self.capture_failure() {
            return Err(error);
        }
        if self.log.finished.load(Ordering::Acquire) && self.exit_code()?.is_none() {
            return Err("LOG_CAPTURE_ENDED: output closed while process is running".into());
        }
        Ok(())
    }

    /// Total bytes this child has emitted, whether retained or dropped.
    pub fn log_size(&self) -> u64 {
        self.log.produced()
    }

    /// Newest output, for diagnosing a failure.
    pub fn log_tail(&self, limit: usize) -> String {
        self.log.tail(limit)
    }

    pub fn runtime_error(&self) -> Option<String> {
        self.log.runtime_error()
    }

    pub fn log_dropped(&self) -> u64 {
        self.log.dropped()
    }

    pub fn log_error(&self) -> Option<String> {
        self.log.file_error()
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        unsafe {
            if !self.job.is_null() {
                TerminateJobObject(self.job, 1);
                // The reader can stop independently even if a descendant holds
                // its write end longer than job termination takes.
                CloseHandle(self.job);
            }
            if !self.process.is_null() {
                CloseHandle(self.process);
            }
        }
        self.log.cancelled.store(true, Ordering::Release);
        self.log
            .disk
            .status
            .cancelled
            .store(true, Ordering::Release);
        if let Some(reader) = self.reader.take() {
            let deadline = Instant::now() + Duration::from_millis(500);
            while !reader.is_finished() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(1));
            }
            if reader.is_finished() {
                let _ = reader.join();
            }
            // Cancellation needs no child output or disk progress. If the OS
            // does not schedule this thread within our shutdown budget, its
            // owned File and global permit stay with it until it resumes. The
            // eight-slot cap prevents retries accumulating unbounded readers.
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    static DISK_TESTS: Mutex<()> = Mutex::new(());

    fn disk_settled(log: &Log) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while !log.disk.status.finished.load(Ordering::Acquire)
            && (!log.disk.status.initialized.load(Ordering::Acquire)
                || log.disk.status.pending.load(Ordering::Acquire) > 0)
        {
            assert!(Instant::now() < deadline, "disk fixture did not settle");
            thread::sleep(Duration::from_millis(1));
        }
    }

    fn wait_exit(child: &Process) -> u32 {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(code) = child.exit_code().unwrap() {
                return code;
            }
            assert!(Instant::now() < deadline, "owned fixture did not exit");
            thread::sleep(Duration::from_millis(2));
        }
    }

    fn fixture_args() -> Vec<String> {
        vec![
            "--exact".into(),
            "preview::windows::process::tests::owned_output_child_fixture".into(),
            "--nocapture".into(),
            "--test-threads=1".into(),
        ]
    }

    /// Invoked only as this test binary's owned subprocess. No SDK, shell,
    /// global environment modification, or unrelated process is involved.
    #[test]
    fn owned_output_child_fixture() {
        let Ok(mode) = std::env::var("ARKDOCK_OWNED_LOG_FIXTURE") else {
            return;
        };
        let stdout = std::io::stdout();
        let mut stdout = stdout.lock();
        let receipt = b"\n> hvigor Finished :entry:PreviewBuild... after 1 ms\n> hvigor BUILD SUCCESSFUL in 1 s\n";
        if mode.starts_with("build-") {
            stdout.write_all(receipt).unwrap();
        }
        let rounds = if mode == "build-overlimit" { 1025 } else { 128 };
        for _ in 0..rounds {
            stdout.write_all(&[b'x'; READ_BUFFER]).unwrap();
        }
        if mode == "error" {
            stdout.write_all(b"\n[Engine Log]Error mes").unwrap();
            stdout.flush().unwrap();
            thread::sleep(Duration::from_millis(10));
            stdout
                .write_all(b"sage: fixture ArkTS exception\n")
                .unwrap();
        }
        stdout.write_all(b"\nFIXTURE_EXIT_RECEIPT\n").unwrap();
        if mode.starts_with("build-") {
            stdout.write_all(receipt).unwrap();
        }
        stdout.flush().unwrap();
        if mode == "build-fail" {
            std::process::exit(7);
        }
        if mode == "hold" || mode == "build-hold" {
            thread::sleep(Duration::from_secs(60));
        }
    }

    fn memory_child(mode: &str, directory: &Path) -> Process {
        Process::spawn_logged(
            &std::env::current_exe().unwrap(),
            &fixture_args(),
            directory,
            &[("ARKDOCK_OWNED_LOG_FIXTURE".into(), mode.into())],
            Log::with_segment(None, None, LogLimits::default()),
        )
        .unwrap()
    }

    #[test]
    fn exited_success_receipt_cannot_hide_a_final_failure_exit() {
        let directory = tempfile::tempdir().unwrap();
        let child = memory_child("build-fail", directory.path());
        assert_eq!(wait_exit(&child), 7);
        child
            .drain_output(Duration::from_secs(2), &AtomicBool::new(false))
            .unwrap();
        assert_eq!(
            super::super::build_receipt("entry", &child.log_tail(4096)),
            super::super::BuildReceipt::Succeeded
        );
        let error =
            super::super::build_progress(&child, "entry", &AtomicBool::new(false)).unwrap_err();
        assert!(error.starts_with("BUILD_FAILED: exit 7"));
    }

    #[test]
    fn exited_success_receipt_requires_draining_and_the_complete_output_limit() {
        let directory = tempfile::tempdir().unwrap();
        let child = memory_child("build-overlimit", directory.path());
        assert_eq!(wait_exit(&child), 0);
        assert_eq!(
            super::super::build_progress(&child, "entry", &AtomicBool::new(false)),
            Err("BUILD_LOG_LIMIT".into())
        );
        assert!(child.log.finished());
        assert!(child.log_size() > super::super::BUILD_LOG_LIMIT);
        assert!(child.log_tail(4096).contains("BUILD SUCCESSFUL"));
    }

    #[test]
    fn live_watch_receipt_is_allowed_but_read_failure_blocks_handoff_and_runtime_health() {
        let directory = tempfile::tempdir().unwrap();
        let child = memory_child("build-hold", directory.path());
        let deadline = Instant::now() + Duration::from_secs(3);
        while !child.log_tail(4096).contains("FIXTURE_EXIT_RECEIPT") {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(child.exit_code().unwrap(), None);
        assert_eq!(
            super::super::build_progress(&child, "entry", &AtomicBool::new(false)),
            Ok(true)
        );
        let fault = "LOG_DRAIN_READ: injected owned-pipe failure".to_owned();
        *child.log.drain_error.lock().unwrap() = Some(fault.clone());
        assert_eq!(child.capture_failure(), Some(fault.clone()));
        assert_eq!(child.check_capture_health(), Err(fault.clone()));
        assert_eq!(
            super::super::build_progress(&child, "entry", &AtomicBool::new(false)),
            Err(fault.clone())
        );
        assert_eq!(
            child.drain_output(Duration::from_secs(10), &AtomicBool::new(false)),
            Err(fault)
        );
    }

    #[test]
    fn losing_all_output_handles_while_child_lives_cannot_confirm_runtime_health() {
        let directory = tempfile::tempdir().unwrap();
        let child = memory_child("hold", directory.path());
        let deadline = Instant::now() + Duration::from_secs(3);
        while !child.log_tail(4096).contains("FIXTURE_EXIT_RECEIPT") {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(child.exit_code().unwrap(), None);
        child.log.mark_finished(); // deterministic injected EOF before process exit
        assert!(child
            .check_capture_health()
            .unwrap_err()
            .starts_with("LOG_CAPTURE_ENDED:"));
    }

    /// A segment that fails after `survive` bytes, standing in for a full or
    /// unavailable disk without needing one.
    struct FailingSegment {
        survive: usize,
        written: usize,
    }
    impl Segment for FailingSegment {
        fn append(&mut self, bytes: &[u8]) -> Result<(), String> {
            if self.written + bytes.len() > self.survive {
                return Err("LOG_WRITE: disk full".into());
            }
            self.written += bytes.len();
            Ok(())
        }
        fn rewrite(&mut self, _: &str, _: &[u8]) -> Result<(), String> {
            Err("LOG_WRITE: disk full".into())
        }
    }

    fn limits(window: usize, budget: u64) -> LogLimits {
        LogLimits { window, budget }
    }

    #[test]
    fn structured_windows_arguments_do_not_expand_shell_metacharacters() {
        assert_eq!(
            quote("C:\\path with spaces\\"),
            "\"C:\\path with spaces\\\\\""
        );
        assert_eq!(quote("a\"b"), "\"a\\\"b\"");
        assert_eq!(quote("$(whoami)&x`y"), "\"$(whoami)&x`y\"");
    }

    #[test]
    fn a_bounded_window_keeps_the_newest_output_and_counts_what_it_dropped() {
        let log = Log::with_segment(None, None, limits(1024, 64 * 1024));
        for _ in 0..8 {
            log.append(&[b'x'; 1000]);
        }
        assert_eq!(log.produced(), 8000);
        assert!(log.dropped() >= 4000, "dropped {}", log.dropped());
        assert!(log.tail(8192).len() <= 2 * 1024, "window grew unbounded");
        assert!(log.tail(8192).chars().all(|ch| ch == 'x'));
    }

    #[test]
    fn rotation_keeps_the_newest_output_inside_the_disk_budget() {
        let _serial = DISK_TESTS.lock().unwrap_or_else(PoisonError::into_inner);
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("previewer.log");
        let log = Log::open(&path, limits(4096, 16 * 1024));
        for index in 0..200 {
            log.append(format!("line {index:04} {}\n", "y".repeat(200)).as_bytes());
            disk_settled(&log);
        }
        assert!(log.produced() > 16 * 1024);
        assert!(log.dropped() > 0);
        let stored = std::fs::read_to_string(&path).unwrap();
        assert!(
            stored.len() as u64 <= 16 * 1024,
            "log file grew past its budget: {}",
            stored.len()
        );
        // The newest line survived rotation; the notice explains the gap.
        assert!(stored.contains("line 0199"), "newest output was lost");
        assert!(stored.contains(ROTATE_NOTICE));
        assert!(log.file_error().is_none());
    }

    #[test]
    fn a_failing_or_unavailable_log_file_degrades_capture_without_failing_the_child() {
        let _serial = DISK_TESTS.lock().unwrap_or_else(PoisonError::into_inner);
        let directory = tempfile::tempdir().unwrap();
        // A log path that cannot be created (missing directory, then a directory
        // in its place) still captures output in memory.
        for path in [
            directory.path().join("absent").join("previewer.log"),
            directory.path().to_path_buf(),
        ] {
            let log = Log::open(&path, limits(4096, 4096));
            disk_settled(&log);
            assert!(log.file_error().is_some());
            log.append(b"[Engine Log]Error message: kept in memory\n");
            assert!(log.tail(4096).contains("Error message"));
            assert!(log.runtime_error().is_some());
        }
        // A write that starts failing mid-run stops using the file and keeps the
        // newest output available; later appends must not panic or block.
        let log = Log::with_segment(
            Some(Box::new(FailingSegment {
                survive: 64,
                written: 0,
            })),
            None,
            limits(1024, 4096),
        );
        log.append(&[b'a'; 128]);
        disk_settled(&log);
        assert!(log.file_error().is_some());
        log.append(&[b'b'; 128]);
        assert_eq!(log.produced(), 256);
        assert!(log.tail(4096).ends_with("bbbb"));
    }

    #[test]
    fn arkts_page_error_split_across_reads_is_fatal_but_sdk_warning_is_not() {
        let log = Log::with_segment(None, None, limits(4096, 4096));
        log.append(b"[ArkCompiler] Throw error: undefined is not callable\n");
        assert!(log.runtime_error().is_none());
        log.append(b"[Engine Log]Error mes");
        assert!(log.runtime_error().is_none());
        log.append(b"sage: Cannot read property length of undefined\n");
        assert!(log
            .runtime_error()
            .unwrap()
            .starts_with("ARKTS_RUNTIME_ERROR:"));
    }

    #[test]
    fn an_early_page_error_survives_a_later_burst_that_exceeds_the_window() {
        let log = Log::with_segment(None, None, limits(1024, 4096));
        log.append(b"[Engine Log]Error message: page threw\n");
        for _ in 0..64 {
            log.append(&[b'z'; 1024]);
        }
        assert!(log.dropped() > 0);
        assert!(log
            .runtime_error()
            .unwrap()
            .starts_with("ARKTS_RUNTIME_ERROR:"));
    }

    #[test]
    fn a_real_child_floods_past_its_budget_without_blocking_and_releases_its_drain_thread() {
        let _serial = DISK_TESTS.lock().unwrap_or_else(PoisonError::into_inner);
        // The test binary is a real child that writes a known, sizeable amount to
        // stdout and exits; a tiny budget makes the flood observable.
        let executable = std::env::current_exe().unwrap();
        let expected = std::process::Command::new(&executable)
            .arg("--list")
            .output()
            .map(|output| output.stdout)
            .unwrap_or_default();
        assert!(expected.len() > 4 * 1024, "the child must produce output");
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("flood.log");
        let child = Process::spawn_with(
            &executable,
            &["--list".to_owned()],
            directory.path(),
            &[],
            &path,
            limits(4096, 8 * 1024),
        )
        .unwrap();
        let mut code = None;
        for _ in 0..2000 {
            if let Some(exit) = child.exit_code().unwrap() {
                code = Some(exit);
                break;
            }
            thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(code, Some(0), "the flooding child must still exit normally");
        child
            .drain_output(Duration::from_secs(2), &AtomicBool::new(false))
            .unwrap();
        disk_settled(&child.log);
        // Every byte was consumed, so the child never blocked on the pipe.
        assert!(child.log_size() >= expected.len() as u64);
        assert!(child.log_dropped() > 0);
        // Disk pressure may degrade capture; it never changes child health.
        let stored = std::fs::read_to_string(&path).unwrap();
        assert!(stored.len() as u64 <= 8 * 1024);
        let tail = child.log_tail(1024);
        assert_eq!(
            tail,
            String::from_utf8_lossy(&expected[expected.len().saturating_sub(1024)..]),
            "the newest output must be the output that was retained"
        );
        // Normal exit closes the child's write ends, so draining ends by itself.
        let observer = child.log.clone();
        let mut drained = false;
        for _ in 0..500 {
            if observer.finished() {
                drained = true;
                break;
            }
            thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(drained, "draining must end once the child is gone");
        drop(child);
        // Dropping the process joins the drain thread: nothing outlives it.
        assert!(observer.finished());
    }

    struct GatedSegment {
        entered: std::sync::mpsc::Sender<()>,
        release: std::sync::mpsc::Receiver<()>,
    }
    impl Segment for GatedSegment {
        fn append(&mut self, _: &[u8]) -> Result<(), String> {
            self.entered.send(()).unwrap();
            self.release.recv().unwrap();
            Ok(())
        }
        fn rewrite(&mut self, _: &str, _: &[u8]) -> Result<(), String> {
            self.append(&[])
        }
    }

    fn gated_log() -> (
        Log,
        std::sync::mpsc::Receiver<()>,
        std::sync::mpsc::Sender<()>,
    ) {
        let (entered, observe) = std::sync::mpsc::channel();
        let (release, gate) = std::sync::mpsc::channel();
        let log = Log::with_segment(
            Some(Box::new(GatedSegment {
                entered,
                release: gate,
            })),
            None,
            limits(4096, 16 * 1024),
        );
        (log, observe, release)
    }

    #[test]
    fn slow_disk_keeps_health_and_memory_nonblocking_and_bounds_queue_pressure() {
        let _serial = DISK_TESTS.lock().unwrap_or_else(PoisonError::into_inner);
        let (log, entered, release) = gated_log();
        log.append(b"initial");
        entered.recv_timeout(Duration::from_secs(2)).unwrap();
        let started = Instant::now();
        log.append(b"[Engine Log]Error mes");
        log.append(b"sage: real marker\n");
        for _ in 0..100 {
            log.append(&[b'z'; READ_BUFFER]);
        }
        assert!(log
            .runtime_error()
            .unwrap()
            .starts_with("ARKTS_RUNTIME_ERROR:"));
        assert!(log.tail(1024).ends_with("zzzz"));
        assert!(log.produced() > 1024 * 1024);
        assert!(log.dropped() > 0);
        assert!(log.file_error().unwrap().starts_with("LOG_QUEUE_PRESSURE:"));
        assert!(log.disk.status.pending.load(Ordering::Acquire) <= DISK_QUEUE_CHUNKS + 1);
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "disk blocked health reads"
        );
        release.send(()).unwrap();
        disk_settled(&log);
    }

    #[test]
    fn real_child_exit_receipt_and_split_runtime_error_drain_even_with_blocked_disk() {
        let _serial = DISK_TESTS.lock().unwrap_or_else(PoisonError::into_inner);
        let executable = std::env::current_exe().unwrap();
        let directory = tempfile::tempdir().unwrap();
        let (log, entered, release) = gated_log();
        let observer = log.clone();
        let child = Process::spawn_logged(
            &executable,
            &fixture_args(),
            directory.path(),
            &[("ARKDOCK_OWNED_LOG_FIXTURE".into(), "error".into())],
            log,
        )
        .unwrap();
        entered.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(wait_exit(&child), 0);
        child
            .drain_output(Duration::from_secs(2), &AtomicBool::new(false))
            .unwrap();
        assert!(child.log_size() > 2 * 1024 * 1024);
        assert!(child.log_tail(4096).contains("FIXTURE_EXIT_RECEIPT"));
        assert!(child
            .runtime_error()
            .unwrap()
            .starts_with("ARKTS_RUNTIME_ERROR:"));
        assert!(child
            .log_error()
            .unwrap()
            .starts_with("LOG_QUEUE_PRESSURE:"));
        assert!(observer.finished());
        let started = Instant::now();
        drop(child);
        drop(observer); // last Log drops the blocked disk capture
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "drop waited for disk"
        );
        release.send(()).unwrap();
    }

    #[test]
    fn drain_timeout_and_cancellation_are_bounded_and_drop_cleans_owned_reader() {
        let _serial = DISK_TESTS.lock().unwrap_or_else(PoisonError::into_inner);
        let directory = tempfile::tempdir().unwrap();
        let child = Process::spawn_with(
            &std::env::current_exe().unwrap(),
            &fixture_args(),
            directory.path(),
            &[("ARKDOCK_OWNED_LOG_FIXTURE".into(), "hold".into())],
            &directory.path().join("hold.log"),
            limits(4096, 8192),
        )
        .unwrap();
        assert_eq!(
            child.drain_output(Duration::from_millis(20), &AtomicBool::new(false)),
            Err("LOG_DRAIN_TIMEOUT".into())
        );
        assert_eq!(
            child.drain_output(Duration::from_secs(10), &AtomicBool::new(true)),
            Err("LOG_DRAIN_CANCELLED".into())
        );
        let observer = child.log.clone();
        let started = Instant::now();
        drop(child);
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(observer.finished(), "owned reader must close on stop");
    }

    #[test]
    fn detached_slow_disk_workers_hold_global_slots_across_retries() {
        let _serial = DISK_TESTS.lock().unwrap_or_else(PoisonError::into_inner);
        let deadline = Instant::now() + Duration::from_secs(2);
        while DISK_WORKERS.load(Ordering::Acquire) != 0 {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        }
        let (first, entered_first, release_first) = gated_log();
        let (second, entered_second, release_second) = gated_log();
        first.append(b"first");
        second.append(b"second");
        entered_first.recv_timeout(Duration::from_secs(2)).unwrap();
        entered_second.recv_timeout(Duration::from_secs(2)).unwrap();
        let started = Instant::now();
        drop(first);
        drop(second);
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(DISK_WORKERS.load(Ordering::Acquire), DISK_WORKER_LIMIT);
        for _ in 0..20 {
            let directory = tempfile::tempdir().unwrap();
            let log = Log::open(&directory.path().join("retry.log"), limits(4096, 8192));
            assert!(log.file_error().unwrap().starts_with("LOG_WORKER_LIMIT:"));
            log.append(b"[Engine Log]Error message: still detected\n");
            assert!(log.runtime_error().is_some());
            assert!(log.tail(1024).contains("still detected"));
            assert_eq!(DISK_WORKERS.load(Ordering::Acquire), DISK_WORKER_LIMIT);
        }
        release_first.send(()).unwrap();
        release_second.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while DISK_WORKERS.load(Ordering::Acquire) != 0 {
            assert!(
                Instant::now() < deadline,
                "released disk workers leaked slots"
            );
            thread::sleep(Duration::from_millis(1));
        }
    }
}
