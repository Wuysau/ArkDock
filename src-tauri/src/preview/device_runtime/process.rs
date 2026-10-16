//! Owned SDK processes. Windows children cannot execute until assigned to our
//! kill-on-close Job. Protocol stdout and diagnostic stderr never share a pipe.
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct ToolOutput {
    pub status: u32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl ToolOutput {
    pub fn stdout_text(&self) -> Result<&str, String> {
        std::str::from_utf8(&self.stdout).map_err(|_| "TOOL_STDOUT_ENCODING".into())
    }

    pub fn diagnostic(&self) -> String {
        // Diagnostics are deliberately bounded, and are never an authoritative
        // protocol response. Callers must not log commands containing secrets.
        let bytes = if self.stderr.is_empty() {
            &self.stdout
        } else {
            &self.stderr
        };
        String::from_utf8_lossy(&bytes[bytes.len().saturating_sub(2048)..]).into_owned()
    }
}

pub struct ToolCommandRunner;
impl ToolCommandRunner {
    pub fn run(
        executable: &Path,
        args: &[String],
        cwd: &Path,
        env: &[(String, String)],
        timeout: Duration,
        cancel: &AtomicBool,
    ) -> Result<ToolOutput, String> {
        Self::run_limited(
            executable,
            args,
            cwd,
            env,
            timeout,
            cancel,
            1024 * 1024,
            256 * 1024,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn run_limited(
        executable: &Path,
        args: &[String],
        cwd: &Path,
        env: &[(String, String)],
        timeout: Duration,
        cancel: &AtomicBool,
        stdout_limit: usize,
        stderr_limit: usize,
    ) -> Result<ToolOutput, String> {
        check_cancel(cancel)?;
        let process =
            OwnedToolProcess::spawn(executable, args, cwd, env, stdout_limit, stderr_limit)?;
        process.wait(timeout, cancel)
    }
}

pub fn check_cancel(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Acquire) {
        Err("DEVICE_OPERATION_CANCELLED".into())
    } else {
        Ok(())
    }
}

/// CommandLineToArgvW/CRT quoting, with no cmd.exe or PowerShell evaluation.
fn windows_quote(value: &str) -> String {
    let mut result = String::from("\"");
    let mut slashes = 0;
    for ch in value.chars() {
        if ch == '\\' {
            slashes += 1;
            continue;
        }
        result.push_str(&"\\".repeat(slashes * if ch == '"' { 2 } else { 1 }));
        slashes = 0;
        if ch == '"' {
            result.push('\\');
        }
        result.push(ch);
    }
    result.push_str(&"\\".repeat(slashes * 2));
    result.push('"');
    result
}

#[cfg(windows)]
pub use win::OwnedToolProcess;

#[cfg(windows)]
mod win {
    use super::*;
    use std::fs::File;
    use std::io::Read;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, RawHandle};
    use std::ptr::{null, null_mut};
    use std::sync::atomic::AtomicUsize;
    use std::sync::{Arc, Mutex};
    use std::thread::{self, JoinHandle};
    use std::time::Instant;
    use windows_sys::Win32::Foundation::*;
    use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
    use windows_sys::Win32::System::JobObjects::*;
    use windows_sys::Win32::System::Pipes::{CreatePipe, PeekNamedPipe};
    use windows_sys::Win32::System::Threading::*;

    const MAX_PROCESSES: usize = 12;
    static PROCESSES: AtomicUsize = AtomicUsize::new(0);
    struct Permit;
    impl Permit {
        fn acquire() -> Result<Self, String> {
            PROCESSES
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                    (count < MAX_PROCESSES).then_some(count + 1)
                })
                .map(|_| Self)
                .map_err(|_| "TOOL_PROCESS_LIMIT".into())
        }
    }
    impl Drop for Permit {
        fn drop(&mut self) {
            PROCESSES.fetch_sub(1, Ordering::AcqRel);
        }
    }

    struct Handle(HANDLE);
    impl Drop for Handle {
        fn drop(&mut self) {
            if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
                unsafe {
                    CloseHandle(self.0);
                }
            }
        }
    }
    // Handles are exclusively owned; Windows allows use/close from any thread.
    unsafe impl Send for Handle {}
    unsafe impl Sync for Handle {}

    #[derive(Default)]
    struct CaptureState {
        bytes: Vec<u8>,
        error: Option<String>,
        eof: bool,
    }
    struct Capture {
        state: Arc<Mutex<CaptureState>>,
        stop: Arc<AtomicBool>,
        reader: Option<JoinHandle<()>>,
    }
    impl Capture {
        fn start(
            mut file: File,
            limit: usize,
            tail: bool,
            permit: Arc<Permit>,
        ) -> Result<Self, String> {
            let state = Arc::new(Mutex::new(CaptureState::default()));
            let stop = Arc::new(AtomicBool::new(false));
            let worker_state = state.clone();
            let worker_stop = stop.clone();
            let reader = thread::Builder::new()
                .name("device-tool-output".into())
                .spawn(move || {
                    let _permit = permit;
                    let mut buffer = [0u8; 8192];
                    loop {
                        if worker_stop.load(Ordering::Acquire) {
                            break;
                        }
                        let mut available = 0;
                        let ready = unsafe {
                            PeekNamedPipe(
                                file.as_raw_handle() as HANDLE,
                                null_mut(),
                                0,
                                null_mut(),
                                &mut available,
                                null_mut(),
                            )
                        };
                        if ready == 0 {
                            let error = std::io::Error::last_os_error();
                            let mut state = worker_state.lock().unwrap_or_else(|e| e.into_inner());
                            if error.raw_os_error() != Some(ERROR_BROKEN_PIPE as i32) {
                                state.error = Some(format!("TOOL_OUTPUT_READ: {error}"));
                            }
                            state.eof = true;
                            return;
                        }
                        if available == 0 {
                            thread::sleep(Duration::from_millis(2));
                            continue;
                        }
                        let count = buffer.len().min(available as usize);
                        match file.read(&mut buffer[..count]) {
                            Ok(0) => {
                                worker_state.lock().unwrap_or_else(|e| e.into_inner()).eof = true;
                                return;
                            }
                            Ok(count) => {
                                let mut state =
                                    worker_state.lock().unwrap_or_else(|e| e.into_inner());
                                if tail {
                                    let excess = state
                                        .bytes
                                        .len()
                                        .saturating_add(count)
                                        .saturating_sub(limit);
                                    let old = excess.min(state.bytes.len());
                                    state.bytes.drain(..old);
                                    state.bytes.extend_from_slice(
                                        &buffer[excess.saturating_sub(old)..count],
                                    );
                                } else {
                                    let room = limit.saturating_sub(state.bytes.len());
                                    state.bytes.extend_from_slice(&buffer[..count.min(room)]);
                                    if count > room {
                                        state.error = Some("TOOL_OUTPUT_LIMIT_EXCEEDED".into());
                                    }
                                }
                            }
                            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {
                                continue
                            }
                            Err(error) => {
                                let mut state =
                                    worker_state.lock().unwrap_or_else(|e| e.into_inner());
                                state.error = Some(format!("TOOL_OUTPUT_READ: {error}"));
                                state.eof = true;
                                return;
                            }
                        }
                    }
                })
                .map_err(|e| format!("TOOL_READER_START: {e}"))?;
            Ok(Self {
                state,
                stop,
                reader: Some(reader),
            })
        }
        fn bytes(&self) -> Vec<u8> {
            self.state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .bytes
                .clone()
        }
        fn health(&self) -> Result<bool, String> {
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            match &state.error {
                Some(e) => Err(e.clone()),
                None => Ok(state.eof),
            }
        }
    }
    impl Drop for Capture {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Release);
            if let Some(reader) = self.reader.take() {
                let deadline = Instant::now() + Duration::from_millis(100);
                while !reader.is_finished() && Instant::now() < deadline {
                    thread::sleep(Duration::from_millis(1));
                }
                if reader.is_finished() {
                    let _ = reader.join();
                }
                // A very delayed OS read retains its permit, bounding retries.
            }
        }
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(Some(0)).collect()
    }
    fn pipe() -> Result<(File, Handle), String> {
        let mut read = null_mut();
        let mut write = null_mut();
        let attrs = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: null_mut(),
            bInheritHandle: 1,
        };
        if unsafe { CreatePipe(&mut read, &mut write, &attrs, 0) } == 0 {
            return Err(format!(
                "TOOL_PIPE_CREATE: {}",
                std::io::Error::last_os_error()
            ));
        }
        let reader = unsafe { File::from_raw_handle(read as RawHandle) };
        let writer = Handle(write);
        if unsafe { SetHandleInformation(read, HANDLE_FLAG_INHERIT, 0) } == 0 {
            return Err("TOOL_PIPE_INHERITANCE".into());
        }
        Ok((reader, writer))
    }

    pub struct OwnedToolProcess {
        job: Handle,
        process: Handle,
        #[cfg(test)]
        pid: u32,
        stdout: Capture,
        stderr: Capture,
        _permit: Arc<Permit>,
    }
    impl OwnedToolProcess {
        pub fn spawn(
            executable: &Path,
            args: &[String],
            cwd: &Path,
            env: &[(String, String)],
            stdout_limit: usize,
            stderr_limit: usize,
        ) -> Result<Self, String> {
            Self::spawn_inner(
                executable,
                args,
                cwd,
                env,
                stdout_limit,
                stderr_limit,
                false,
            )
        }
        /// Long-lived services retain a bounded diagnostic tail, never a
        /// machine-readable command receipt. Their pipe drain runs continuously.
        pub fn spawn_service(
            executable: &Path,
            args: &[String],
            cwd: &Path,
            env: &[(String, String)],
        ) -> Result<Self, String> {
            Self::spawn_inner(executable, args, cwd, env, 64 * 1024, 64 * 1024, true)
        }
        #[allow(clippy::too_many_arguments)]
        fn spawn_inner(
            executable: &Path,
            args: &[String],
            cwd: &Path,
            env: &[(String, String)],
            stdout_limit: usize,
            stderr_limit: usize,
            tail: bool,
        ) -> Result<Self, String> {
            if !executable.is_absolute()
                || !cwd.is_absolute()
                || !executable.is_file()
                || !cwd.is_dir()
                || !(1..=16 * 1024 * 1024).contains(&stdout_limit)
                || !(1..=16 * 1024 * 1024).contains(&stderr_limit)
                || args.iter().any(|arg| arg.contains('\0'))
                || env
                    .iter()
                    .any(|(k, v)| k.is_empty() || k.contains(['\0', '=']) || v.contains('\0'))
            {
                return Err("INVALID_TOOL_PROCESS_ARGUMENT".into());
            }
            let exe = executable.to_str().ok_or("TOOL_PATH_ENCODING")?;
            let cwd = cwd.to_str().ok_or("TOOL_PATH_ENCODING")?;
            if exe.contains('\0') || cwd.contains('\0') {
                return Err("INVALID_TOOL_PROCESS_ARGUMENT".into());
            }
            let mut command = wide(
                &std::iter::once(exe.to_owned())
                    .chain(args.iter().cloned())
                    .map(|a| windows_quote(&a))
                    .collect::<Vec<_>>()
                    .join(" "),
            );
            if command.len() > 32767 {
                return Err("TOOL_COMMAND_LIMIT".into());
            }
            let permit = Arc::new(Permit::acquire()?);
            let job = Handle(unsafe { CreateJobObjectW(null(), null()) });
            if job.0.is_null() {
                return Err("TOOL_JOB_CREATE".into());
            }
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if unsafe {
                SetInformationJobObject(
                    job.0,
                    JobObjectExtendedLimitInformation,
                    (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                    std::mem::size_of_val(&limits) as u32,
                )
            } == 0
            {
                return Err("TOOL_JOB_LIMIT".into());
            }
            let (stdout_read, stdout_write) = pipe()?;
            let (stderr_read, stderr_write) = pipe()?;
            // A closed pipe is deterministic EOF for stdin. No terminal or
            // credential prompt can accidentally inherit the app's input.
            let (stdin_read, stdin_write) = pipe()?;
            drop(stdin_write);
            if unsafe {
                SetHandleInformation(
                    stdin_read.as_raw_handle() as HANDLE,
                    HANDLE_FLAG_INHERIT,
                    HANDLE_FLAG_INHERIT,
                )
            } == 0
            {
                return Err("TOOL_STDIN_INHERITANCE".into());
            }
            let handles = [
                stdout_write.0,
                stderr_write.0,
                stdin_read.as_raw_handle() as HANDLE,
            ];
            let mut size = 0;
            unsafe {
                InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut size);
            }
            let mut storage = vec![0usize; size.div_ceil(std::mem::size_of::<usize>())];
            let attrs = storage.as_mut_ptr().cast();
            if unsafe { InitializeProcThreadAttributeList(attrs, 1, 0, &mut size) } == 0 {
                return Err("TOOL_ATTRIBUTES".into());
            }
            let updated = unsafe {
                UpdateProcThreadAttribute(
                    attrs,
                    0,
                    PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                    handles.as_ptr().cast(),
                    std::mem::size_of_val(&handles),
                    null_mut(),
                    null_mut(),
                )
            };
            let mut startup: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
            startup.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
            startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES | STARTF_USESHOWWINDOW;
            startup.StartupInfo.wShowWindow = 0;
            startup.StartupInfo.hStdInput = handles[2];
            startup.StartupInfo.hStdOutput = handles[0];
            startup.StartupInfo.hStdError = handles[1];
            startup.lpAttributeList = attrs;
            let mut variables: Vec<_> = std::env::vars_os()
                .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)))
                .filter(|(k, _)| !env.iter().any(|(over, _)| k.eq_ignore_ascii_case(over)))
                .chain(env.iter().cloned())
                .collect();
            variables.sort_by_key(|(k, _)| k.to_ascii_uppercase());
            let mut environment: Vec<u16> = variables
                .iter()
                .flat_map(|(k, v)| wide(&format!("{k}={v}")))
                .collect();
            environment.push(0);
            if environment.len() == 1 {
                environment.push(0);
            }
            let mut info: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
            let created = updated != 0
                && unsafe {
                    CreateProcessW(
                        wide(exe).as_ptr(),
                        command.as_mut_ptr(),
                        null(),
                        null(),
                        1,
                        CREATE_SUSPENDED
                            | CREATE_NO_WINDOW
                            | CREATE_UNICODE_ENVIRONMENT
                            | EXTENDED_STARTUPINFO_PRESENT,
                        environment.as_ptr().cast(),
                        wide(cwd).as_ptr(),
                        &startup.StartupInfo,
                        &mut info,
                    )
                } != 0;
            let error = std::io::Error::last_os_error();
            unsafe {
                DeleteProcThreadAttributeList(attrs);
            }
            drop(stdout_write);
            drop(stderr_write);
            drop(stdin_read);
            if !created {
                return Err(format!("TOOL_PROCESS_START: {error}"));
            }
            let process = Handle(info.hProcess);
            let thread = Handle(info.hThread);
            if unsafe { AssignProcessToJobObject(job.0, process.0) } == 0 {
                unsafe {
                    TerminateProcess(process.0, 1);
                }
                return Err(format!(
                    "TOOL_JOB_ASSIGN: {}",
                    std::io::Error::last_os_error()
                ));
            }
            // Readers start before resume, so even immediate burst output is
            // observed; any failure drops the Job containing the suspended child.
            let stdout = Capture::start(stdout_read, stdout_limit, tail, permit.clone())?;
            let stderr = Capture::start(stderr_read, stderr_limit, tail, permit.clone())?;
            if unsafe { ResumeThread(thread.0) } == u32::MAX {
                return Err("TOOL_PROCESS_RESUME".into());
            }
            Ok(Self {
                job,
                process,
                #[cfg(test)]
                pid: info.dwProcessId,
                stdout,
                stderr,
                _permit: permit,
            })
        }
        #[cfg(test)]
        pub fn pid(&self) -> u32 {
            self.pid
        }
        /// Launchers may exit after spawning the actual emulator. Descendants
        /// remain in the same Job, so root-process exit does not lose ownership.
        pub fn active_processes(&self) -> Result<u32, String> {
            let mut accounting: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION =
                unsafe { std::mem::zeroed() };
            if unsafe {
                QueryInformationJobObject(
                    self.job.0,
                    JobObjectBasicAccountingInformation,
                    (&mut accounting as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
                    std::mem::size_of_val(&accounting) as u32,
                    null_mut(),
                )
            } == 0
            {
                return Err("TOOL_JOB_ACCOUNTING".into());
            }
            Ok(accounting.ActiveProcesses)
        }
        pub fn stdout(&self) -> Vec<u8> {
            self.stdout.bytes()
        }
        pub fn stderr(&self) -> Vec<u8> {
            self.stderr.bytes()
        }
        pub fn health(&self) -> Result<(), String> {
            self.stdout.health()?;
            self.stderr.health()?;
            Ok(())
        }
        pub fn exit_code(&self) -> Result<Option<u32>, String> {
            match unsafe { WaitForSingleObject(self.process.0, 0) } {
                WAIT_TIMEOUT => Ok(None),
                WAIT_OBJECT_0 => {
                    let mut code = 0;
                    if unsafe { GetExitCodeProcess(self.process.0, &mut code) } == 0 {
                        Err("TOOL_EXIT_STATUS".into())
                    } else {
                        Ok(Some(code))
                    }
                }
                _ => Err("TOOL_WAIT_FAILED".into()),
            }
        }
        pub fn wait(&self, timeout: Duration, cancel: &AtomicBool) -> Result<ToolOutput, String> {
            let deadline = Instant::now()
                .checked_add(timeout)
                .ok_or("INVALID_TOOL_TIMEOUT")?;
            loop {
                check_cancel(cancel)?;
                self.health()?;
                if let Some(status) = self.exit_code()? {
                    if self.stdout.health()? && self.stderr.health()? {
                        check_cancel(cancel)?;
                        return Ok(ToolOutput {
                            status,
                            stdout: self.stdout(),
                            stderr: self.stderr(),
                        });
                    }
                }
                if Instant::now() >= deadline {
                    return Err("TOOL_TIMEOUT".into());
                }
                thread::sleep(Duration::from_millis(5));
            }
        }
    }
    impl Drop for OwnedToolProcess {
        fn drop(&mut self) {
            unsafe {
                TerminateJobObject(self.job.0, 1);
            }
        }
    }
}

#[cfg(not(windows))]
pub struct OwnedToolProcess;
#[cfg(not(windows))]
impl OwnedToolProcess {
    pub fn spawn(
        _: &Path,
        _: &[String],
        _: &Path,
        _: &[(String, String)],
        _: usize,
        _: usize,
    ) -> Result<Self, String> {
        Err("DEVICE_RUNTIME_UNSUPPORTED_PLATFORM".into())
    }
    pub fn spawn_service(
        _: &Path,
        _: &[String],
        _: &Path,
        _: &[(String, String)],
    ) -> Result<Self, String> {
        Err("DEVICE_RUNTIME_UNSUPPORTED_PLATFORM".into())
    }
    pub fn active_processes(&self) -> Result<u32, String> {
        Err("DEVICE_RUNTIME_UNSUPPORTED_PLATFORM".into())
    }
    pub fn stdout(&self) -> Vec<u8> {
        Vec::new()
    }
    pub fn stderr(&self) -> Vec<u8> {
        Vec::new()
    }
    pub fn health(&self) -> Result<(), String> {
        Err("DEVICE_RUNTIME_UNSUPPORTED_PLATFORM".into())
    }
    pub fn exit_code(&self) -> Result<Option<u32>, String> {
        Err("DEVICE_RUNTIME_UNSUPPORTED_PLATFORM".into())
    }
    pub fn wait(&self, _: Duration, _: &AtomicBool) -> Result<ToolOutput, String> {
        Err("DEVICE_RUNTIME_UNSUPPORTED_PLATFORM".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn windows_arguments_quote_unicode_spaces_and_trailing_backslashes() {
        assert_eq!(windows_quote("中文 path\\"), "\"中文 path\\\\\"");
        assert_eq!(windows_quote("a\"b"), "\"a\\\"b\"");
    }
    #[test]
    fn cancellation_does_not_spawn_anything() {
        let result = ToolCommandRunner::run(
            Path::new("missing.exe"),
            &[],
            Path::new("."),
            &[],
            Duration::from_secs(1),
            &AtomicBool::new(true),
        );
        assert!(matches!(result, Err(e) if e == "DEVICE_OPERATION_CANCELLED"));
    }
    #[cfg(windows)]
    fn powershell() -> std::path::PathBuf {
        std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap())
            .join("System32/WindowsPowerShell/v1.0/powershell.exe")
    }
    #[cfg(windows)]
    #[test]
    fn isolated_child_keeps_stdout_stderr_and_waits_for_eof() {
        let exe = powershell();
        let output = ToolCommandRunner::run(
            &exe,
            &[
                "-NoProfile".into(),
                "-NonInteractive".into(),
                "-Command".into(),
                "[Console]::Out.Write('protocol'); [Console]::Error.Write('diagnostic'); exit 0"
                    .into(),
            ],
            exe.parent().unwrap(),
            &[],
            Duration::from_secs(10),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(output.status, 0);
        assert_eq!(output.stdout, b"protocol");
        assert_eq!(output.stderr, b"diagnostic");
    }
    #[cfg(windows)]
    #[test]
    fn isolated_child_output_limit_and_timeout_fail_closed() {
        let exe = powershell();
        let args = [
            "-NoProfile".into(),
            "-NonInteractive".into(),
            "-Command".into(),
            "[Console]::Out.Write(('x' * 4096))".into(),
        ];
        assert!(
            matches!(ToolCommandRunner::run_limited(&exe,&args,exe.parent().unwrap(),&[],Duration::from_secs(10),&AtomicBool::new(false),32,32), Err(e) if e == "TOOL_OUTPUT_LIMIT_EXCEEDED")
        );
        let args = [
            "-NoProfile".into(),
            "-NonInteractive".into(),
            "-Command".into(),
            "Start-Sleep -Seconds 30".into(),
        ];
        assert!(
            matches!(ToolCommandRunner::run(&exe,&args,exe.parent().unwrap(),&[],Duration::from_millis(100),&AtomicBool::new(false)), Err(e) if e == "TOOL_TIMEOUT")
        );
    }

    #[cfg(windows)]
    #[test]
    fn cancelling_owned_job_terminates_its_child_and_grandchild() {
        use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
        use windows_sys::Win32::System::Threading::{
            OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
        };
        let exe = powershell();
        let args = ["-NoProfile".into(), "-NonInteractive".into(), "-Command".into(),
            "$child = Start-Process -FilePath (Get-Process -Id $PID).Path -ArgumentList '-NoProfile','-NonInteractive','-Command','Start-Sleep -Seconds 30' -WindowStyle Hidden -PassThru; [Console]::Out.WriteLine($child.Id); Start-Sleep -Seconds 30".into()];
        let process =
            OwnedToolProcess::spawn(&exe, &args, exe.parent().unwrap(), &[], 4096, 4096).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let child_pid = loop {
            if let Ok(pid) = String::from_utf8_lossy(&process.stdout())
                .trim()
                .parse::<u32>()
            {
                break pid;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "fake child did not start"
            );
            std::thread::sleep(Duration::from_millis(10));
        };
        let root_handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, process.pid()) };
        let child_handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, child_pid) };
        assert!(!root_handle.is_null() && !child_handle.is_null());
        assert!(process.active_processes().unwrap() >= 2);
        assert!(process
            .wait(Duration::from_secs(2), &AtomicBool::new(true))
            .is_err());
        drop(process);
        let root_result = unsafe { WaitForSingleObject(root_handle, 5000) };
        let child_result = unsafe { WaitForSingleObject(child_handle, 5000) };
        unsafe {
            CloseHandle(root_handle);
            CloseHandle(child_handle);
        }
        assert_eq!(root_result, WAIT_OBJECT_0);
        assert_eq!(child_result, WAIT_OBJECT_0);
    }
}
