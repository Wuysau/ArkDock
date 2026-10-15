//! Windows named-pipe listener for [`crate::context`].
//!
//! The pipe only transports bytes; trust comes from three facts established here:
//! the OS-proven client PID (`GetNamedPipeClientProcessId`), the client process
//! start-time lifetime token, and the pairing secret carried inside each frame.
//! Frames are bounded JSON lines; any violation drops the connection and revokes
//! its registrations, so a client recovers by reconnecting and re-announcing.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::context::{
    now_millis, parse_message, ContextHub, CONNECTION_IDLE_TIMEOUT_MS, MAX_CONNECTIONS,
    MAX_MESSAGE_BYTES,
};
use crate::ipc::model::ProcessRef;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{ReadFile, PIPE_ACCESS_INBOUND};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, GetNamedPipeClientProcessId, PIPE_TYPE_BYTE,
};
use windows_sys::Win32::System::IO::CancelIoEx;

/// Accept loop: one pipe instance per connection, at most [`MAX_CONNECTIONS`].
pub fn spawn(hub: Arc<ContextHub>) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("arkdock-context-listener".into())
        .spawn(move || accept_loop(&hub))
        .expect("context listener thread")
}

fn accept_loop(hub: &Arc<ContextHub>) {
    loop {
        // Inbound byte mode. The pipe DACL is not the authorization boundary; the
        // pairing secret plus the OS client identity are.
        let handle = unsafe {
            CreateNamedPipeW(
                PIPE_NAME_WIDE.as_ptr(),
                PIPE_ACCESS_INBOUND,
                PIPE_TYPE_BYTE,
                MAX_CONNECTIONS as u32,
                4096,
                (MAX_MESSAGE_BYTES + 1) as u32,
                0,
                std::ptr::null(),
            )
        };
        if handle == INVALID_HANDLE_VALUE || handle.is_null() {
            // Never spin on a persistent failure (for example a stale process holding
            // the name); back off instead.
            std::thread::sleep(Duration::from_millis(1_000));
            continue;
        }
        let connected = unsafe { ConnectNamedPipe(handle, std::ptr::null_mut()) };
        if connected == 0 {
            // ERROR_PIPE_CONNECTED (535): a client wrote before we listened; that
            // connection is still valid. Anything else is a real failure.
            let error = unsafe { windows_sys::Win32::Foundation::GetLastError() };
            if error != 535 {
                unsafe {
                    let _ = CloseHandle(handle);
                }
                continue;
            }
        }
        let hub = hub.clone();
        // HANDLE is a raw pointer and not Send; the thread boundary carries the isize
        // value (the same convention the windowing module uses for HWNDs).
        let handle_value = handle as isize;
        let spawned = std::thread::Builder::new()
            .name("arkdock-context-connection".into())
            .spawn(move || connection_thread(handle_value, hub));
        if spawned.is_err() {
            unsafe {
                let _ = CloseHandle(handle);
            }
        }
    }
}

fn connection_thread(handle_value: isize, hub: Arc<ContextHub>) {
    // The activity marker lets the watchdog cancel an abandoned connection; without
    // it a silent client would pin one of the bounded pipe instances forever.
    let handle = handle_value as HANDLE;
    let last_activity = Arc::new(AtomicU64::new(now_millis()));
    let finished = Arc::new(AtomicBool::new(false));
    let watchdog = {
        let last_activity = last_activity.clone();
        let finished = finished.clone();
        std::thread::spawn(move || {
            while !finished.load(Ordering::Acquire) {
                std::thread::sleep(Duration::from_millis(1_000));
                let idle = now_millis().saturating_sub(last_activity.load(Ordering::Acquire));
                if idle > CONNECTION_IDLE_TIMEOUT_MS {
                    unsafe {
                        let _ = CancelIoEx(handle_value as HANDLE, std::ptr::null_mut());
                    }
                    return;
                }
            }
        })
    };

    let mut client = ProcessRef {
        pid: 0,
        started_at: String::new(),
    };
    let mut client_pid = 0u32;
    // windows-sys BOOL: nonzero means the OS query succeeded.
    let identified = unsafe { GetNamedPipeClientProcessId(handle, &mut client_pid) } != 0
        && client_pid != 0
        && {
            match crate::discovery::registration::process_started_at(client_pid) {
                Ok(started_at) => {
                    client = ProcessRef {
                        pid: client_pid,
                        started_at: started_at.to_string(),
                    };
                    true
                }
                Err(_) => false,
            }
        };
    if identified {
        read_loop(handle, &hub, &client, &last_activity);
        hub.remove_client(&client);
    }
    finished.store(true, Ordering::Release);
    let _ = watchdog.join();
    unsafe {
        let _ = CloseHandle(handle);
    }
}

fn read_loop(
    handle: HANDLE,
    hub: &Arc<ContextHub>,
    client: &ProcessRef,
    last_activity: &AtomicU64,
) {
    let mut chunk = [0u8; 4096];
    let mut line: Vec<u8> = Vec::with_capacity(1024);
    loop {
        let mut read = 0u32;
        let ok = unsafe {
            ReadFile(
                handle,
                chunk.as_mut_ptr(),
                chunk.len() as u32,
                &mut read,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 || read == 0 {
            return; // EOF, cancellation or error: the connection is over.
        }
        last_activity.store(now_millis(), Ordering::Release);
        for byte in &chunk[..read as usize] {
            match byte {
                b'\n' => {
                    let parsed = std::str::from_utf8(&line)
                        .ok()
                        .and_then(|text| parse_message(text).ok());
                    line.clear();
                    let Some(message) = parsed else {
                        return; // Malformed frame: protocol violation.
                    };
                    if hub.apply(message, client.clone(), now_millis()).is_err() {
                        return; // Secret, session or pairing failure: disconnect.
                    }
                }
                byte => {
                    line.push(*byte);
                    if line.len() > MAX_MESSAGE_BYTES {
                        return; // Oversized frame: drop the connection.
                    }
                }
            }
        }
    }
}

/// Constant wide encoding of the fixed pipe path (`\\.\pipe\arkdock-context`).
const PIPE_NAME_WIDE: &[u16] = &[
    b'\\' as u16,
    b'\\' as u16,
    b'.' as u16,
    b'\\' as u16,
    b'p' as u16,
    b'i' as u16,
    b'p' as u16,
    b'e' as u16,
    b'\\' as u16,
    b'a' as u16,
    b'r' as u16,
    b'k' as u16,
    b'd' as u16,
    b'o' as u16,
    b'c' as u16,
    b'k' as u16,
    b'-' as u16,
    b'c' as u16,
    b'o' as u16,
    b'n' as u16,
    b't' as u16,
    b'e' as u16,
    b'x' as u16,
    b't' as u16,
    0,
];
