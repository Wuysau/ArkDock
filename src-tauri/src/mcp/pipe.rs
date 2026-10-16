//! Authenticated, local-only duplex transport. Nonblocking I/O gives every wait
//! a deadline and lets ServerGuard cancel idle connections without leaked readers.
use super::{auth::Credential, ConnectionOwner};
use serde_json::Value;
use std::sync::Arc;
pub type Handler =
    Arc<dyn Fn(ConnectionOwner, String, Value) -> Result<Value, String> + Send + Sync>;

#[cfg(windows)]
mod native {
    use super::*;
    use crate::mcp::{auth, MAX_REQUEST, MAX_RESPONSE};
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::time::{Duration, Instant};
    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, ReadFile, WriteFile, FILE_FLAG_FIRST_PIPE_INSTANCE, OPEN_EXISTING,
        PIPE_ACCESS_DUPLEX,
    };
    use windows_sys::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, GetNamedPipeClientProcessId, SetNamedPipeHandleState,
        PIPE_NOWAIT, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE,
    };

    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    const IO_TIMEOUT: Duration = Duration::from_secs(5);
    const IDLE: Duration = Duration::from_secs(15);
    const POLL: Duration = Duration::from_millis(5);
    const MAX_CONNECTIONS: u32 = 32;

    struct Handle(HANDLE);
    unsafe impl Send for Handle {}
    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }

    fn wide(name: &str) -> Vec<u16> {
        name.encode_utf16().chain(Some(0)).collect()
    }
    fn create(credential: &Credential, first: bool) -> Result<Handle, String> {
        let flags = PIPE_ACCESS_DUPLEX
            | if first {
                FILE_FLAG_FIRST_PIPE_INSTANCE
            } else {
                0
            };
        let raw = auth::private_attributes(|attributes| unsafe {
            CreateNamedPipeW(
                wide(&credential.pipe_name).as_ptr(),
                flags,
                PIPE_TYPE_BYTE | PIPE_NOWAIT | PIPE_REJECT_REMOTE_CLIENTS,
                MAX_CONNECTIONS,
                65536,
                65536,
                0,
                attributes,
            )
        })?;
        if raw == INVALID_HANDLE_VALUE || raw.is_null() {
            Err("MCP_PIPE_UNAVAILABLE".into())
        } else {
            Ok(Handle(raw))
        }
    }

    struct Channel {
        handle: Handle,
        pending: Vec<u8>,
    }
    impl Channel {
        fn read(
            &mut self,
            maximum: usize,
            deadline: Instant,
            stop: &AtomicBool,
        ) -> Result<Value, String> {
            loop {
                if let Some(end) = self.pending.iter().position(|byte| *byte == b'\n') {
                    if end > maximum {
                        return Err("MCP_FRAME_TOO_LARGE".into());
                    }
                    let line: Vec<_> = self.pending.drain(..=end).collect();
                    return serde_json::from_slice(&line).map_err(|_| "MCP_FRAME_INVALID".into());
                }
                if self.pending.len() > maximum {
                    return Err("MCP_FRAME_TOO_LARGE".into());
                }
                if stop.load(Ordering::Acquire) {
                    return Err("MCP_SHUTDOWN".into());
                }
                if Instant::now() >= deadline {
                    return Err("MCP_PIPE_TIMEOUT".into());
                }
                let mut buffer = [0; 8192];
                let mut count = 0;
                let ok = unsafe {
                    ReadFile(
                        self.handle.0,
                        buffer.as_mut_ptr(),
                        buffer.len() as u32,
                        &mut count,
                        std::ptr::null_mut(),
                    )
                };
                if ok != 0 && count > 0 {
                    self.pending.extend_from_slice(&buffer[..count as usize]);
                    continue;
                }
                let error = unsafe { GetLastError() };
                if error != 232 {
                    return Err("MCP_DESKTOP_UNAVAILABLE".into());
                }
                std::thread::sleep(POLL);
            }
        }
        fn write(
            &self,
            value: &Value,
            maximum: usize,
            deadline: Instant,
            stop: &AtomicBool,
        ) -> Result<(), String> {
            let mut bytes = serde_json::to_vec(value).map_err(|_| "MCP_FRAME_INVALID")?;
            if bytes.len() > maximum {
                return Err("MCP_FRAME_TOO_LARGE".into());
            }
            bytes.push(b'\n');
            let mut offset = 0;
            while offset < bytes.len() {
                if stop.load(Ordering::Acquire) {
                    return Err("MCP_SHUTDOWN".into());
                }
                if Instant::now() >= deadline {
                    return Err("MCP_PIPE_TIMEOUT".into());
                }
                let mut count = 0;
                let ok = unsafe {
                    WriteFile(
                        self.handle.0,
                        bytes[offset..].as_ptr(),
                        (bytes.len() - offset) as u32,
                        &mut count,
                        std::ptr::null_mut(),
                    )
                };
                // Byte-pipe backpressure is a successful partial/zero-byte
                // write. ERROR_NO_DATA on WriteFile means the peer is closing,
                // unlike the same code from ReadFile on an empty live pipe.
                if ok == 0 {
                    return Err("MCP_DESKTOP_UNAVAILABLE".into());
                }
                offset += count as usize;
                if count == 0 {
                    std::thread::sleep(POLL);
                }
            }
            Ok(())
        }
    }

    pub struct ServerGuard {
        stop: Arc<AtomicBool>,
        worker: Option<std::thread::JoinHandle<()>>,
    }
    impl Drop for ServerGuard {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Release);
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        }
    }
    pub fn start_server(credential: Credential, handler: Handler) -> Result<ServerGuard, String> {
        let listener = create(&credential, true)?;
        let stop = Arc::new(AtomicBool::new(false));
        let running = stop.clone();
        let worker = std::thread::Builder::new()
            .name("arkdock-mcp-listener".into())
            .spawn(move || {
                let mut listener = Some(listener);
                let mut workers: Vec<std::thread::JoinHandle<()>> = Vec::new();
                while !running.load(Ordering::Acquire) {
                    let mut index = 0;
                    while index < workers.len() {
                        if workers[index].is_finished() {
                            let _ = workers.swap_remove(index).join();
                        } else {
                            index += 1;
                        }
                    }
                    if listener.is_none() && workers.len() < MAX_CONNECTIONS as usize {
                        listener = create(&credential, false).ok();
                    }
                    if let Some(pipe) = listener.as_ref() {
                        let connected =
                            unsafe { ConnectNamedPipe(pipe.0, std::ptr::null_mut()) } != 0;
                        let error = if connected {
                            0
                        } else {
                            unsafe { GetLastError() }
                        };
                        let connected = connected || error == 535;
                        if connected {
                            let channel = Channel {
                                handle: listener.take().unwrap(),
                                pending: Vec::new(),
                            };
                            let credential = credential.clone();
                            let handler = handler.clone();
                            let stop = running.clone();
                            workers.push(std::thread::spawn(move || {
                                serve(channel, credential, handler, &stop)
                            }));
                        } else if error == 232 || error == 109 {
                            // A peer may disappear before the accept poll. This
                            // instance cannot accept another client until reset.
                            listener = None;
                        }
                    }
                    std::thread::sleep(POLL);
                }
                for worker in workers {
                    let _ = worker.join();
                }
            })
            .map_err(|_| "MCP_LISTENER_FAILED")?;
        Ok(ServerGuard {
            stop,
            worker: Some(worker),
        })
    }

    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Probe {
        kind: String,
        challenge: String,
    }
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Login {
        kind: String,
        token: String,
        client_name: String,
    }
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Request {
        tool: String,
        arguments: Value,
    }

    fn serve(mut channel: Channel, credential: Credential, handler: Handler, stop: &AtomicBool) {
        let mut pid = 0;
        if unsafe { GetNamedPipeClientProcessId(channel.handle.0, &mut pid) } == 0 || pid == 0 {
            return;
        }
        let Ok(started_at) = crate::discovery::registration::process_started_at(pid) else {
            return;
        };
        let handshake = (|| -> Result<String, String> {
            let deadline = Instant::now() + IO_TIMEOUT;
            let probe: Probe = serde_json::from_value(channel.read(MAX_REQUEST, deadline, stop)?)
                .map_err(|_| "MCP_PROBE_REQUIRED")?;
            if probe.kind != "probe"
                || probe.challenge.len() != 64
                || !probe.challenge.bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                return Err("MCP_PROBE_REQUIRED".into());
            }
            channel.write(&serde_json::json!({"kind":"proof","challenge":probe.challenge,"proof":auth::proof(&credential,&probe.challenge)}), MAX_REQUEST,deadline,stop)?;
            let login: Login = serde_json::from_value(channel.read(MAX_REQUEST, deadline, stop)?)
                .map_err(|_| "MCP_AUTH_INVALID")?;
            if login.kind != "login"
                || !auth::constant_eq(
                    login.token.as_bytes(),
                    auth::hex(&credential.token).as_bytes(),
                )
                || login.client_name.trim().is_empty()
                || login.client_name.len() > 128
            {
                return Err("MCP_AUTH_INVALID".into());
            }
            channel.write(&serde_json::json!({"ok":true}), MAX_REQUEST, deadline, stop)?;
            Ok(login.client_name)
        })();
        let Ok(client_name) = handshake else {
            return;
        };
        let owner = ConnectionOwner {
            connection_id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            process: crate::ipc::model::ProcessRef {
                pid,
                started_at: started_at.to_string(),
            },
            client_name,
        };
        loop {
            let Ok(value) = channel.read(MAX_REQUEST, Instant::now() + IDLE, stop) else {
                break;
            };
            let Ok(request) = serde_json::from_value::<Request>(value) else {
                break;
            };
            if !super::super::catalog::known(&request.tool) && request.tool != "__heartbeat" {
                break;
            }
            let result = handler(owner.clone(), request.tool, request.arguments);
            let reply = match result {
                Ok(value) => serde_json::json!({"ok":true,"result":value}),
                Err(error) => serde_json::json!({"ok":false,"error":error}),
            };
            if channel
                .write(&reply, MAX_RESPONSE, Instant::now() + IO_TIMEOUT, stop)
                .is_err()
            {
                break;
            }
        }
        let _ = handler(owner, "__disconnect".into(), serde_json::json!({}));
    }

    pub struct Client {
        channel: Channel,
        stop: Arc<AtomicBool>,
    }
    impl Client {
        pub fn connect(
            credential: &Credential,
            client_name: &str,
            stop: Arc<AtomicBool>,
        ) -> Result<Self, String> {
            let raw = unsafe {
                CreateFileW(
                    wide(&credential.pipe_name).as_ptr(),
                    GENERIC_READ | GENERIC_WRITE,
                    0,
                    std::ptr::null(),
                    OPEN_EXISTING,
                    0,
                    std::ptr::null_mut(),
                )
            };
            if raw == INVALID_HANDLE_VALUE {
                return Err(
                    "MCP_DESKTOP_UNAVAILABLE: Start ArkDock to use project and task tools.".into(),
                );
            }
            let handle = Handle(raw);
            let mode = PIPE_NOWAIT;
            if unsafe {
                SetNamedPipeHandleState(handle.0, &mode, std::ptr::null(), std::ptr::null())
            } == 0
            {
                return Err("MCP_PIPE_MODE_FAILED".into());
            }
            let mut channel = Channel {
                handle,
                pending: Vec::new(),
            };
            let challenge = auth::nonce()?;
            let deadline = Instant::now() + IO_TIMEOUT;
            channel.write(
                &serde_json::json!({"kind":"probe","challenge":challenge}),
                MAX_REQUEST,
                deadline,
                &stop,
            )?;
            let proof = channel.read(MAX_REQUEST, deadline, &stop)?;
            if proof["kind"] != "proof"
                || proof["challenge"] != challenge
                || !auth::constant_eq(
                    proof["proof"].as_str().unwrap_or_default().as_bytes(),
                    auth::proof(credential, &challenge).as_bytes(),
                )
            {
                return Err("MCP_SERVER_PROOF_INVALID".into());
            }
            // No credential bytes cross the pipe until the server proves key possession.
            channel.write(&serde_json::json!({"kind":"login","token":auth::hex(&credential.token),"client_name":client_name}),MAX_REQUEST,deadline,&stop)?;
            if channel.read(MAX_REQUEST, deadline, &stop)?["ok"] != true {
                return Err("MCP_AUTH_INVALID".into());
            }
            Ok(Self { channel, stop })
        }
        pub fn call(&mut self, tool: &str, arguments: Value) -> Result<Value, String> {
            let stop = self.stop.clone();
            self.call_cancellable(tool, arguments, &stop)
        }
        pub(crate) fn set_stop(&mut self, stop: Arc<AtomicBool>) {
            self.stop = stop;
        }
        pub(crate) fn call_cancellable(
            &mut self,
            tool: &str,
            arguments: Value,
            cancelled: &AtomicBool,
        ) -> Result<Value, String> {
            let deadline = Instant::now() + IO_TIMEOUT;
            self.channel.write(
                &serde_json::json!({"tool":tool,"arguments":arguments}),
                MAX_REQUEST,
                deadline,
                cancelled,
            )?;
            let reply = self.channel.read(MAX_RESPONSE, deadline, cancelled)?;
            if reply["ok"] == true && reply.get("result").is_some() {
                Ok(reply["result"].clone())
            } else if reply["ok"] == false && reply["error"].is_string() {
                // A valid business refusal does not end the connection or its
                // context/claims. Only transport/protocol failures return Err.
                Ok(crate::mcp::tool_error(reply["error"].as_str().unwrap()))
            } else {
                Err("MCP_REPLY_INVALID".into())
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        fn credential() -> Credential {
            Credential {
                token: [0x5a; 32],
                pipe_name: format!(r"\\.\pipe\arkdock-mcp-test-{}", auth::nonce().unwrap()),
            }
        }
        #[test]
        fn real_pipe_authentication_identity_heartbeat_and_eof_cleanup() {
            let credential = credential();
            let (events, received) = std::sync::mpsc::channel();
            let guard = start_server(credential.clone(),Arc::new(move |owner,tool,args| {
                events.send((owner,tool.clone())).unwrap();
                Ok(serde_json::json!({"content":[{"type":"text","text":args.to_string()}],"tool":tool}))
            })).unwrap();
            let stop = Arc::new(AtomicBool::new(false));
            let mut client = Client::connect(&credential, "Unbranded test host", stop).unwrap();
            assert_eq!(
                client
                    .call("arkdock_context_status", serde_json::json!({}))
                    .unwrap()["tool"],
                "arkdock_context_status"
            );
            client.call("__heartbeat", serde_json::json!({})).unwrap();
            let (first, _) = received.recv_timeout(IO_TIMEOUT).unwrap();
            let (heartbeat, tool) = received.recv_timeout(IO_TIMEOUT).unwrap();
            assert_eq!(first.process.pid, std::process::id());
            assert_eq!(
                first.process.started_at,
                crate::discovery::registration::process_started_at(std::process::id())
                    .unwrap()
                    .to_string()
            );
            assert_eq!(first.client_name, "Unbranded test host");
            assert_eq!(first.connection_id, heartbeat.connection_id);
            assert_eq!(tool, "__heartbeat");
            drop(client);
            let (closed, tool) = received.recv_timeout(IO_TIMEOUT).unwrap();
            assert_eq!(closed.connection_id, first.connection_id);
            assert_eq!(tool, "__disconnect");
            drop(guard);
        }
        #[test]
        fn unproven_server_receives_no_client_credentials() {
            let credential = credential();
            let listener = create(&credential, true).unwrap();
            let worker = std::thread::spawn(move || {
                let stop = AtomicBool::new(false);
                let deadline = Instant::now() + IO_TIMEOUT;
                loop {
                    if unsafe { ConnectNamedPipe(listener.0, std::ptr::null_mut()) } != 0
                        || unsafe { GetLastError() } == 535
                    {
                        break;
                    }
                    assert!(Instant::now() < deadline);
                    std::thread::sleep(POLL);
                }
                let mut channel = Channel {
                    handle: listener,
                    pending: Vec::new(),
                };
                let probe = channel.read(MAX_REQUEST, deadline, &stop).unwrap();
                assert!(probe.get("token").is_none());
                channel.write(&serde_json::json!({"kind":"proof","challenge":probe["challenge"],"proof":"invalid"}),MAX_REQUEST,deadline,&stop).unwrap();
                assert!(channel.read(MAX_REQUEST, deadline, &stop).is_err());
            });
            let result = Client::connect(&credential, "host", Arc::new(AtomicBool::new(false)));
            assert_eq!(result.err().unwrap(), "MCP_SERVER_PROOF_INVALID");
            worker.join().unwrap();
        }
        #[test]
        fn server_guard_cancels_idle_and_partial_connections() {
            let credential = credential();
            let guard = start_server(
                credential.clone(),
                Arc::new(|_, _, _| Ok(serde_json::json!({}))),
            )
            .unwrap();
            let client =
                Client::connect(&credential, "host", Arc::new(AtomicBool::new(false))).unwrap();
            let started = Instant::now();
            drop(guard);
            assert!(started.elapsed() < Duration::from_secs(1));
            drop(client);
        }
        #[test]
        fn cancellation_closes_pending_read_without_waiting_for_timeout() {
            let credential = credential();
            let (entered, wait) = std::sync::mpsc::channel();
            let (resume, blocked) = std::sync::mpsc::channel();
            let blocked = std::sync::Mutex::new(blocked);
            let guard = start_server(
                credential.clone(),
                Arc::new(move |_, tool, _| {
                    if tool != "__disconnect" {
                        entered.send(()).unwrap();
                        blocked.lock().unwrap().recv_timeout(IO_TIMEOUT).unwrap();
                    }
                    Ok(serde_json::json!({}))
                }),
            )
            .unwrap();
            let stop = Arc::new(AtomicBool::new(false));
            let mut client = Client::connect(&credential, "host", stop.clone()).unwrap();
            let worker = std::thread::spawn(move || {
                client.call("arkdock_context_status", serde_json::json!({}))
            });
            wait.recv_timeout(IO_TIMEOUT).unwrap();
            let started = Instant::now();
            stop.store(true, Ordering::Release);
            assert_eq!(worker.join().unwrap().unwrap_err(), "MCP_SHUTDOWN");
            assert!(started.elapsed() < Duration::from_secs(1));
            resume.send(()).unwrap();
            drop(guard);
        }
    }
}

#[cfg(windows)]
pub use native::{start_server, Client, ServerGuard};

#[cfg(not(windows))]
pub struct ServerGuard;
#[cfg(not(windows))]
pub fn start_server(_credential: Credential, _handler: Handler) -> Result<ServerGuard, String> {
    Err("MCP_WINDOWS_REQUIRED".into())
}
#[cfg(not(windows))]
pub struct Client;
#[cfg(not(windows))]
impl Client {
    pub fn connect(
        _: &Credential,
        _: &str,
        _: Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<Self, String> {
        Err("MCP_WINDOWS_REQUIRED".into())
    }
    pub fn call(&mut self, _: &str, _: Value) -> Result<Value, String> {
        Err("MCP_WINDOWS_REQUIRED".into())
    }
    pub(crate) fn set_stop(&mut self, _: Arc<std::sync::atomic::AtomicBool>) {}
    pub(crate) fn call_cancellable(
        &mut self,
        _: &str,
        _: Value,
        _: &std::sync::atomic::AtomicBool,
    ) -> Result<Value, String> {
        Err("MCP_WINDOWS_REQUIRED".into())
    }
}
