//! Atomic containment: a child is suspended until assigned to our kill-on-close Job.
use std::fs::{self, File};
use std::os::windows::io::AsRawHandle;
use std::path::Path;
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::System::JobObjects::*;
use windows_sys::Win32::System::Threading::*;

pub(super) struct Process {
    job: HANDLE,
    process: HANDLE,
    pub pid: u32,
    log: File,
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

impl Process {
    pub fn spawn(
        executable: &Path,
        args: &[String],
        cwd: &Path,
        env: &[(String, String)],
        log_path: &Path,
    ) -> Result<Self, String> {
        if args.iter().any(|arg| arg.contains('\0'))
            || env
                .iter()
                .any(|(key, value)| key.contains(['\0', '=']) || value.contains('\0'))
        {
            return Err("INVALID_PROCESS_ARGUMENT".into());
        }
        let log = File::create(log_path).map_err(|e| format!("PROCESS_LOG: {e}"))?;
        let mut result = Self {
            job: null_mut(),
            process: null_mut(),
            pid: 0,
            log,
        };
        unsafe {
            result.job = CreateJobObjectW(null(), null());
            if result.job.is_null() {
                return Err(format!("JOB_CREATE: {}", std::io::Error::last_os_error()));
            }
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                result.job,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of_val(&limits) as u32,
            ) == 0
            {
                return Err(format!("JOB_LIMIT: {}", std::io::Error::last_os_error()));
            }
            let mut output = null_mut();
            if DuplicateHandle(
                GetCurrentProcess(),
                result.log.as_raw_handle(),
                GetCurrentProcess(),
                &mut output,
                0,
                1,
                DUPLICATE_SAME_ACCESS,
            ) == 0
            {
                return Err(format!(
                    "PROCESS_LOG_HANDLE: {}",
                    std::io::Error::last_os_error()
                ));
            }
            // Only this one output handle may be inherited, irrespective of other threads.
            let mut bytes = 0;
            InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut bytes);
            let words = bytes.div_ceil(std::mem::size_of::<usize>());
            let mut attributes = vec![0usize; words];
            let list = attributes.as_mut_ptr().cast();
            if InitializeProcThreadAttributeList(list, 1, 0, &mut bytes) == 0 {
                CloseHandle(output);
                return Err("PROCESS_ATTRIBUTE_LIST".into());
            }
            let updated = UpdateProcThreadAttribute(
                list,
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                (&output as *const HANDLE).cast(),
                std::mem::size_of::<HANDLE>(),
                null_mut(),
                null_mut(),
            );
            let mut startup: STARTUPINFOEXW = std::mem::zeroed();
            startup.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
            startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
            startup.StartupInfo.hStdOutput = output;
            startup.StartupInfo.hStdError = output;
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
            CloseHandle(output);
            if !created {
                return Err(format!("PROCESS_START: {error}"));
            }
            result.process = info.hProcess;
            result.pid = info.dwProcessId;
            if AssignProcessToJobObject(result.job, result.process) == 0 {
                TerminateProcess(result.process, 1);
                CloseHandle(info.hThread);
                return Err(format!("JOB_ASSIGN: {}", std::io::Error::last_os_error()));
            }
            let resumed = ResumeThread(info.hThread);
            CloseHandle(info.hThread);
            if resumed == u32::MAX {
                return Err("PROCESS_RESUME".into());
            }
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

    pub fn log_size(&self) -> u64 {
        self.log.metadata().map(|m| m.len()).unwrap_or(u64::MAX)
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        unsafe {
            if !self.job.is_null() {
                TerminateJobObject(self.job, 1);
                if !self.process.is_null() {
                    WaitForSingleObject(self.process, 2000);
                }
                CloseHandle(self.job);
            }
            if !self.process.is_null() {
                CloseHandle(self.process);
            }
        }
    }
}

pub(super) fn bounded_log(path: &Path) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let mut text = String::new();
    if let Ok(mut file) = fs::File::open(path) {
        let length = file.metadata().map(|m| m.len()).unwrap_or(0);
        let _ = file.seek(SeekFrom::Start(length.saturating_sub(64 * 1024)));
        let _ = file.take(64 * 1024).read_to_string(&mut text);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn structured_windows_arguments_do_not_expand_shell_metacharacters() {
        assert_eq!(
            quote("C:\\path with spaces\\"),
            "\"C:\\path with spaces\\\\\""
        );
        assert_eq!(quote("a\"b"), "\"a\\\"b\"");
        assert_eq!(quote("$(whoami)&x`y"), "\"$(whoami)&x`y\"");
    }
}
