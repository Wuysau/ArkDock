//! Local Windows duplex named-pipe transport with server key proof, OS client identity and ACKs.
//! A successful write is not registration acceptance; every applied frame receives
//! a bounded acknowledgement. Failed authentication/protocol checks close only the
//! owning connection and revoke its leases.
use crate::context::{now_millis, parse_message, ContextHub, MAX_CONNECTIONS, MAX_MESSAGE_BYTES};
use crate::ipc::model::ProcessRef;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{ReadFile, WriteFile, PIPE_ACCESS_DUPLEX};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, GetNamedPipeClientProcessId, PIPE_REJECT_REMOTE_CLIENTS,
    PIPE_TYPE_BYTE,
};
use windows_sys::Win32::System::Threading::{GetCurrentThreadId, OpenThread, THREAD_TERMINATE};
use windows_sys::Win32::System::IO::CancelSynchronousIo;

static NEXT_CONNECTION_ID: AtomicU64 = AtomicU64::new(1);

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ProbeFrame {
    kind: String,
    challenge: String,
}

fn respond_to_probe(handle: HANDLE, hub: &ContextHub, line: &[u8]) -> bool {
    let result = serde_json::from_slice::<ProbeFrame>(line)
        .map_err(|_| "CONTEXT_PROBE_REQUIRED".to_owned())
        .and_then(|probe| {
            if probe.kind != "probe" {
                return Err("CONTEXT_PROBE_REQUIRED".into());
            }
            hub.server_proof(&probe.challenge).map(|proof| {
                serde_json::json!({"kind":"proof","challenge":probe.challenge,"proof":proof})
            })
        });
    match result {
        Ok(value) => write_json(handle, &value),
        Err(error) => {
            write_ack(handle, None, None, Err(error));
            false
        }
    }
}
#[cfg(feature = "desktop")]
pub fn spawn(hub: Arc<ContextHub>) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("arkdock-context-listener".into())
        .spawn(move || accept_loop(&hub))
        .expect("context listener thread")
}
fn create_pipe(name: &[u16]) -> HANDLE {
    unsafe {
        CreateNamedPipeW(
            name.as_ptr(),
            PIPE_ACCESS_DUPLEX,
            PIPE_TYPE_BYTE | PIPE_REJECT_REMOTE_CLIENTS,
            MAX_CONNECTIONS as u32,
            4096,
            (MAX_MESSAGE_BYTES + 1) as u32,
            0,
            std::ptr::null(),
        )
    }
}
#[cfg(feature = "desktop")]
fn accept_loop(hub: &Arc<ContextHub>) {
    let name: Vec<u16> = crate::context::CONTEXT_PIPE_NAME
        .encode_utf16()
        .chain(Some(0))
        .collect();
    loop {
        let handle = create_pipe(&name);
        if handle == INVALID_HANDLE_VALUE || handle.is_null() {
            std::thread::sleep(Duration::from_millis(1000));
            continue;
        }
        if !connect_pipe(handle) {
            unsafe {
                CloseHandle(handle);
            }
            continue;
        }
        let hub = hub.clone();
        let handle_value = handle as isize;
        if std::thread::Builder::new()
            .name("arkdock-context-connection".into())
            .spawn(move || {
                connection_thread(
                    handle_value,
                    hub,
                    crate::context::CONNECTION_IDLE_TIMEOUT_MS,
                )
            })
            .is_err()
        {
            unsafe {
                CloseHandle(handle);
            }
        }
    }
}
fn connect_pipe(handle: HANDLE) -> bool {
    unsafe {
        ConnectNamedPipe(handle, std::ptr::null_mut()) != 0
            || windows_sys::Win32::Foundation::GetLastError() == 535
    }
}
fn connection_thread(handle_value: isize, hub: Arc<ContextHub>, idle_timeout_ms: u64) {
    let handle = handle_value as HANDLE;
    let Ok(connection_id) =
        NEXT_CONNECTION_ID
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
    else {
        unsafe {
            CloseHandle(handle);
        }
        return;
    };
    // ReadFile/WriteFile are synchronous. CancelSynchronousIo targets this worker;
    // CancelIoEx on the pipe does not provide the synchronous timeout guarantee.
    let worker = unsafe { OpenThread(THREAD_TERMINATE, 0, GetCurrentThreadId()) };
    if worker.is_null() {
        unsafe {
            CloseHandle(handle);
        }
        return;
    }
    let worker_value = worker as isize;
    let last_activity = Arc::new(AtomicU64::new(now_millis()));
    let finished = Arc::new(AtomicBool::new(false));
    let watchdog = {
        let last_activity = last_activity.clone();
        let finished = finished.clone();
        std::thread::spawn(move || {
            while !finished.load(Ordering::Acquire) {
                std::thread::sleep(Duration::from_millis(50));
                if now_millis().saturating_sub(last_activity.load(Ordering::Acquire))
                    > idle_timeout_ms
                {
                    // Repeat until the worker exits: cancellation may race the next read.
                    unsafe {
                        CancelSynchronousIo(worker_value as HANDLE);
                    }
                }
            }
        })
    };
    let mut pid = 0u32;
    if unsafe { GetNamedPipeClientProcessId(handle, &mut pid) } != 0 && pid != 0 {
        if let Ok(started_at) = crate::discovery::registration::process_started_at(pid) {
            let client = ProcessRef {
                pid,
                started_at: started_at.to_string(),
            };
            read_loop(
                handle,
                &hub,
                &client,
                connection_id,
                &last_activity,
                idle_timeout_ms,
            );
        }
    }
    hub.remove_connection(connection_id);
    finished.store(true, Ordering::Release);
    let _ = watchdog.join();
    unsafe {
        CloseHandle(worker);
        CloseHandle(handle);
    }
}
fn read_loop(
    handle: HANDLE,
    hub: &ContextHub,
    client: &ProcessRef,
    connection_id: u64,
    last_activity: &AtomicU64,
    idle_timeout_ms: u64,
) {
    let mut chunk = [0u8; 4096];
    let mut line = Vec::with_capacity(1024);
    let mut proof_sent = false;
    loop {
        if now_millis().saturating_sub(last_activity.load(Ordering::Acquire)) > idle_timeout_ms {
            return;
        }
        let mut read = 0u32;
        if unsafe {
            ReadFile(
                handle,
                chunk.as_mut_ptr(),
                chunk.len() as u32,
                &mut read,
                std::ptr::null_mut(),
            )
        } == 0
            || read == 0
        {
            return;
        }
        for byte in &chunk[..read as usize] {
            if *byte == b'\n' {
                if !proof_sent {
                    if !respond_to_probe(handle, hub, &line) {
                        return;
                    }
                    line.clear();
                    proof_sent = true;
                    // A single pre-authentication probe does not extend the idle timer.
                    continue;
                }
                let message = std::str::from_utf8(&line)
                    .map_err(|_| "CONTEXT_FRAME_INVALID".to_owned())
                    .and_then(parse_message);
                line.clear();
                let message = match message {
                    Ok(message) => message,
                    Err(error) => {
                        write_ack(handle, None, None, Err(error));
                        return;
                    }
                };
                let session = message.session().to_owned();
                let sequence = message.sequence();
                let result = hub.apply(message, client.clone(), connection_id, now_millis());
                let accepted = result.is_ok();
                if !write_ack(handle, Some(&session), Some(sequence), result) || !accepted {
                    return;
                }
                // Only a complete accepted frame counts as activity, not slow partial bytes.
                last_activity.store(now_millis(), Ordering::Release);
            } else {
                line.push(*byte);
                if line.len() > MAX_MESSAGE_BYTES {
                    write_ack(handle, None, None, Err("CONTEXT_FRAME_TOO_LARGE".into()));
                    return;
                }
            }
        }
    }
}
fn write_ack(
    handle: HANDLE,
    session: Option<&str>,
    sequence: Option<u64>,
    result: Result<(), String>,
) -> bool {
    let mut value = serde_json::json!({"kind":"ack","ok":result.is_ok()});
    if let Some(session) = session.filter(|s| s.len() <= 128) {
        value["session"] = session.into();
    }
    if let Some(sequence) = sequence {
        value["sequence"] = sequence.into();
    }
    if let Err(error) = result {
        value["error"] = error.into();
    }
    write_json(handle, &value)
}
fn write_json(handle: HANDLE, value: &serde_json::Value) -> bool {
    let mut bytes = value.to_string().into_bytes();
    bytes.push(b'\n');
    let mut offset = 0usize;
    while offset < bytes.len() {
        let mut written = 0u32;
        if unsafe {
            WriteFile(
                handle,
                bytes[offset..].as_ptr(),
                (bytes.len() - offset) as u32,
                &mut written,
                std::ptr::null_mut(),
            )
        } == 0
            || written == 0
        {
            return false;
        }
        offset += written as usize;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{File, OpenOptions};
    use std::io::{BufRead, BufReader, Write};
    struct Connection {
        stream: BufReader<File>,
        done: std::sync::mpsc::Receiver<()>,
        worker: Option<std::thread::JoinHandle<()>>,
    }
    impl Connection {
        fn open(hub: Arc<ContextHub>, idle_ms: u64) -> Self {
            let expected = hub.server_proof(&"1".repeat(64)).unwrap();
            let mut connection = Self::open_unproven(hub, idle_ms);
            let proof = connection.probe(&"1".repeat(64));
            assert_eq!(proof["kind"], "proof");
            assert_eq!(proof["challenge"], "1".repeat(64));
            assert_eq!(proof["proof"], expected);
            connection
        }
        fn open_unproven(hub: Arc<ContextHub>, idle_ms: u64) -> Self {
            static TEST_ID: AtomicU64 = AtomicU64::new(1);
            let path = format!(
                "\\\\.\\pipe\\arkdock-context-test-{}-{}",
                std::process::id(),
                TEST_ID.fetch_add(1, Ordering::Relaxed)
            );
            let name: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
            let handle = create_pipe(&name);
            assert!(!handle.is_null() && handle != INVALID_HANDLE_VALUE);
            let handle_value = handle as isize;
            let (tx, done) = std::sync::mpsc::channel();
            let worker = std::thread::spawn(move || {
                assert!(connect_pipe(handle_value as HANDLE));
                connection_thread(handle_value, hub, idle_ms);
                let _ = tx.send(());
            });
            let client = OpenOptions::new()
                .read(true)
                .write(true)
                .open(path)
                .unwrap();
            Self {
                stream: BufReader::new(client),
                done,
                worker: Some(worker),
            }
        }
        fn probe(&mut self, challenge: &str) -> serde_json::Value {
            writeln!(
                self.stream.get_mut(),
                "{}",
                serde_json::json!({"kind":"probe","challenge":challenge})
            )
            .unwrap();
            let mut line = String::new();
            self.stream.read_line(&mut line).unwrap();
            serde_json::from_str(&line).unwrap()
        }
        fn send(&mut self, kind: &str, session: &str, sequence: u64) -> serde_json::Value {
            let mut value = serde_json::json!({"kind":kind,"token":"aa".repeat(32),"session":session,"sequence":sequence});
            if kind == "hello" || kind == "update" {
                value["roots"] = serde_json::json!(["C:\\fixture-project"]);
            }
            writeln!(self.stream.get_mut(), "{value}").unwrap();
            let mut line = String::new();
            self.stream.read_line(&mut line).unwrap();
            serde_json::from_str(&line).unwrap()
        }
        fn finished(&mut self) {
            self.done
                .recv_timeout(Duration::from_secs(3))
                .expect("bounded production connection cleanup");
            self.worker.take().unwrap().join().unwrap();
        }
    }
    fn hub() -> Arc<ContextHub> {
        let hub = Arc::new(ContextHub::new());
        hub.activate_token([0xaa; 32]);
        hub
    }
    #[test]
    fn real_pipe_ack_replay_and_connection_owned_disconnect() {
        let hub = hub();
        let mut first = Connection::open(hub.clone(), 2000);
        let mut second = Connection::open(hub.clone(), 2000);
        let ack = first.send("hello", "session-first-123", 1);
        assert_eq!(ack["ok"], true);
        assert_eq!(ack["sequence"], 1);
        assert_eq!(second.send("hello", "session-second-12", 1)["ok"], true);
        assert_eq!(hub.live(now_millis()).len(), 2);
        let rejection = first.send("heartbeat", "session-first-123", 1);
        assert_eq!(rejection["ok"], false);
        assert_eq!(rejection["error"], "CONTEXT_SEQUENCE_REPLAY");
        first.finished();
        let live = hub.live(now_millis());
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].session, "session-second-12");
        assert_eq!(second.send("heartbeat", "session-second-12", 2)["ok"], true);
        assert_eq!(second.send("revoke", "session-second-12", 3)["ok"], true);
        assert!(hub.live(now_millis()).is_empty());
        drop(second.stream);
        second.done.recv_timeout(Duration::from_secs(3)).unwrap();
        second.worker.take().unwrap().join().unwrap();
    }
    #[test]
    fn real_pipe_invalid_token_is_rejected_without_registration() {
        let hub = hub();
        let mut connection = Connection::open(hub.clone(), 1000);
        writeln!(connection.stream.get_mut(),"{}",serde_json::json!({"kind":"hello","token":"wrong-secret","session":"session-invalid-1","sequence":1})).unwrap();
        let mut line = String::new();
        connection.stream.read_line(&mut line).unwrap();
        assert!(line.contains("CONTEXT_TOKEN_INVALID"));
        assert!(!line.contains("wrong-secret"));
        connection.finished();
        assert!(hub.live(now_millis()).is_empty());
    }
    #[test]
    fn real_pipe_partial_bytes_do_not_renew_idle_lease() {
        let hub = hub();
        let mut connection = Connection::open(hub.clone(), 200);
        assert_eq!(connection.send("hello", "session-partial-1", 1)["ok"], true);
        connection
            .stream
            .get_mut()
            .write_all(b"{\"kind\":")
            .unwrap();
        connection.finished();
        assert!(hub.live(now_millis()).is_empty());
    }
    #[test]
    fn real_pipe_material_update_ack_carries_the_applied_context() {
        let hub = hub();
        let mut connection = Connection::open(hub.clone(), 1000);
        assert_eq!(connection.send("hello", "session-update-12", 1)["ok"], true);
        let previous = hub.live(now_millis()).remove(0).registration_id;
        let frame = serde_json::json!({"kind":"update","token":"aa".repeat(32),"session":"session-update-12","sequence":2,
            "roots":["C:\\host"],"cwd":"C:\\tool","originalProject":"C:\\repo","executionDirectory":"/home/worktree",
            "environment":"wsl","hostSession":"actual-host-session"});
        writeln!(connection.stream.get_mut(), "{frame}").unwrap();
        let mut line = String::new();
        connection.stream.read_line(&mut line).unwrap();
        let ack: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(ack["ok"], true);
        assert_eq!(ack["sequence"], 2);
        let updated = hub.live(now_millis()).remove(0);
        assert_ne!(updated.registration_id, previous);
        assert_eq!(
            updated.effective_execution_directory(),
            Some("/home/worktree")
        );
        assert_eq!(updated.original_project.as_deref(), Some("C:\\repo"));
        assert_eq!(updated.host_session.as_deref(), Some("actual-host-session"));
        assert_eq!(
            updated.execution_environment(),
            crate::ipc::model::Environment::Wsl
        );
        assert_eq!(
            connection.send("revoke", "session-update-12", 3)["ok"],
            true
        );
        drop(connection.stream);
        connection
            .done
            .recv_timeout(Duration::from_secs(3))
            .unwrap();
        connection.worker.take().unwrap().join().unwrap();
        assert!(hub.live(now_millis()).is_empty());
    }
    #[test]
    fn real_pipe_oversized_frame_cleans_up_registration() {
        let hub = hub();
        let mut connection = Connection::open(hub.clone(), 1000);
        assert_eq!(connection.send("hello", "session-oversized", 1)["ok"], true);
        connection
            .stream
            .get_mut()
            .write_all(&vec![b' '; MAX_MESSAGE_BYTES + 1])
            .unwrap();
        let mut line = String::new();
        connection.stream.read_line(&mut line).unwrap();
        assert!(line.contains("CONTEXT_FRAME_TOO_LARGE"));
        connection.finished();
        assert!(hub.live(now_millis()).is_empty());
    }
    #[test]
    fn real_pipe_requires_one_strict_probe_before_registration() {
        let hub = hub();
        let mut missing = Connection::open_unproven(hub.clone(), 1000);
        let reply = missing.send("hello", "session-no-probe1", 1);
        assert_eq!(reply["error"], "CONTEXT_PROBE_REQUIRED");
        missing.finished();
        assert!(hub.live(now_millis()).is_empty());

        let mut extra = Connection::open_unproven(hub.clone(), 1000);
        writeln!(
            extra.stream.get_mut(),
            "{}",
            serde_json::json!({"kind":"probe","challenge":"1".repeat(64),"token":"must-not-echo"})
        )
        .unwrap();
        let mut line = String::new();
        extra.stream.read_line(&mut line).unwrap();
        assert!(line.contains("CONTEXT_PROBE_REQUIRED"));
        assert!(!line.contains("must-not-echo"));
        extra.finished();

        let mut repeated = Connection::open(hub.clone(), 1000);
        assert_eq!(
            repeated.probe(&"2".repeat(64))["error"],
            "CONTEXT_FRAME_INVALID"
        );
        repeated.finished();
        assert!(hub.live(now_millis()).is_empty());
    }
    #[test]
    fn real_pipe_missing_pairing_produces_no_proof_or_credential() {
        let hub = Arc::new(ContextHub::new());
        let mut connection = Connection::open_unproven(hub.clone(), 1000);
        let reply = connection.probe(&"1".repeat(64));
        assert_eq!(reply["error"], "CONTEXT_PAIRING_MISSING");
        assert!(reply.get("proof").is_none());
        assert!(reply.get("token").is_none());
        connection.finished();
        assert!(hub.live(now_millis()).is_empty());
    }
    #[test]
    fn real_pipe_unauthed_probe_does_not_register_or_keep_socket_alive() {
        let hub = hub();
        let mut connection = Connection::open(hub.clone(), 200);
        assert!(hub.live(now_millis()).is_empty());
        connection.finished();
        assert!(hub.live(now_millis()).is_empty());
    }
}
