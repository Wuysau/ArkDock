#![cfg(all(windows, feature = "desktop"))]
//! Spawn the actual application executable exclusively with --mcp. No desktop,
//! device, bundle, or release command participates in these protocol fixtures.
use arkdock_core::mcp::{auth, pipe, ConnectionOwner};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(5);

struct Host {
    child: Child,
    stdin: Option<ChildStdin>,
    replies: mpsc::Receiver<Value>,
    stderr: Option<std::thread::JoinHandle<String>>,
}

impl Host {
    fn start(appdata: &std::path::Path) -> Self {
        let executable = std::env::var_os("ARKDOCK_MCP_TEST_EXE")
            .unwrap_or_else(|| env!("CARGO_BIN_EXE_arkdock").into());
        let mut child = Command::new(executable)
            .arg("--mcp")
            .env("APPDATA", appdata)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();
        let (send, replies) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let value = serde_json::from_str::<Value>(&line.unwrap())
                    .expect("stdout contains only MCP JSON frames");
                if send.send(value).is_err() {
                    break;
                }
            }
        });
        let stderr = Some(std::thread::spawn(move || {
            let mut text = String::new();
            stderr.read_to_string(&mut text).unwrap();
            text
        }));
        Self {
            child,
            stdin,
            replies,
            stderr,
        }
    }
    fn send(&mut self, frame: Value) {
        writeln!(self.stdin.as_mut().unwrap(), "{frame}").unwrap();
        self.stdin.as_mut().unwrap().flush().unwrap();
    }
    fn reply(&self, id: u64) -> Value {
        let reply = self
            .replies
            .recv_timeout(TIMEOUT)
            .expect("bounded MCP reply");
        assert_eq!(reply["jsonrpc"], "2.0");
        assert_eq!(reply["id"], id);
        reply
    }
    fn initialize(&mut self) {
        self.send(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","clientInfo":{"name":"Generic process fixture","version":"1"},"capabilities":{}}}));
        let reply = self.reply(1);
        assert_eq!(reply["result"]["serverInfo"]["name"], "ArkDock");
        assert!(reply["result"]["instructions"]
            .as_str()
            .unwrap()
            .contains("contextId"));
        self.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
    }
    fn close(&mut self) {
        self.stdin.take();
        let deadline = Instant::now() + TIMEOUT;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            assert!(Instant::now() < deadline, "MCP process exits on stdin EOF");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(self.stderr.take().unwrap().join().unwrap().is_empty());
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn actual_executable_discovers_tools_offline_and_reports_desktop_unavailable() {
    let directory = tempfile::tempdir().unwrap();
    let mut host = Host::start(directory.path());
    host.initialize();
    host.send(json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}));
    let reply = host.reply(2);
    assert_eq!(reply["result"]["tools"].as_array().unwrap().len(), 9);
    assert!(!directory.path().join("dev.arkdock.desktop").exists());
    host.send(json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"arkdock_context_status","arguments":{}}}));
    let unavailable = host.reply(3);
    assert_eq!(unavailable["result"]["isError"], true);
    assert!(unavailable["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("MCP_DESKTOP_UNAVAILABLE"));
    host.close();
}

#[test]
fn actual_executable_forwards_native_identity_and_cancels_inflight_call() {
    let directory = tempfile::tempdir().unwrap();
    let credential = auth::load_or_create(&directory.path().join("dev.arkdock.desktop")).unwrap();
    let (events, received) = mpsc::channel::<(ConnectionOwner, String)>();
    let (resume, blocked) = mpsc::channel();
    let blocked = Mutex::new(blocked);
    let server = pipe::start_server(
        credential,
        Arc::new(move |owner, tool, _| {
            events.send((owner, tool.clone())).unwrap();
            if tool == "arkdock_claim_task" {
                blocked.lock().unwrap().recv_timeout(TIMEOUT).unwrap();
            }
            Ok(json!({"content":[{"type":"image","mimeType":"image/png","data":"iVBORw0KGgo="}]}))
        }),
    )
    .unwrap();
    let mut host = Host::start(directory.path());
    host.initialize();
    host.send(json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"arkdock_get_task_image","arguments":{"contextId":"ctx","taskId":"task","claimId":"claim","image":"crop"}}}));
    assert_eq!(host.reply(2)["result"]["content"][0]["type"], "image");
    let (owner, tool) = received.recv_timeout(TIMEOUT).unwrap();
    assert_eq!(owner.process.pid, host.child.id());
    assert_eq!(owner.client_name, "Generic process fixture");
    assert_eq!(tool, "arkdock_get_task_image");
    host.send(json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"arkdock_claim_task","arguments":{"contextId":"ctx"}}}));
    let (claim_owner, tool) = received.recv_timeout(TIMEOUT).unwrap();
    assert_eq!(claim_owner.connection_id, owner.connection_id);
    assert_eq!(tool, "arkdock_claim_task");
    host.send(json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":3}}));
    let cancelled = host.reply(3);
    assert_eq!(cancelled["error"]["code"], -32800);
    assert!(cancelled.get("result").is_none());
    let disconnect_started = Instant::now();
    resume.send(()).unwrap();
    let (disconnect_owner, tool) = received.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(disconnect_started.elapsed() < Duration::from_secs(1));
    assert_eq!(disconnect_owner.connection_id, owner.connection_id);
    assert_eq!(tool, "__disconnect");
    host.close();
    drop(server);
}

#[test]
fn business_rejection_keeps_context_claim_and_heartbeat_on_same_connection() {
    let directory = tempfile::tempdir().unwrap();
    let credential = auth::load_or_create(&directory.path().join("dev.arkdock.desktop")).unwrap();
    let (events, received) = mpsc::channel::<(ConnectionOwner, String)>();
    let claimed = Mutex::new(false);
    let server=pipe::start_server(credential,Arc::new(move |owner,tool,_| {
        events.send((owner,tool.clone())).unwrap();
        match tool.as_str() {
            "arkdock_connect_current_project" => Ok(json!({"content":[{"type":"text","text":"connected"}],"structuredContent":{"contextId":"fixture-context"}})),
            "arkdock_claim_task" => { *claimed.lock().unwrap()=true; Ok(json!({"content":[],"structuredContent":{"claimId":"fixture-claim"}})) },
            "arkdock_list_tasks" => Err("MCP_CONTEXT_CONFIRMATION_REQUIRED".into()),
            "arkdock_context_status" => Ok(json!({"content":[],"structuredContent":{"contextId":"fixture-context","claimActive":*claimed.lock().unwrap()}})),
            "__disconnect" => { *claimed.lock().unwrap()=false; Ok(json!({"content":[]})) },
            "__heartbeat" => Ok(json!({"content":[]})),
            _ => Err("MCP_TOOL_UNKNOWN".into()),
        }
    })).unwrap();
    let mut host = Host::start(directory.path());
    host.initialize();
    let mut owner = None;
    for (id, tool) in [
        (2, "arkdock_connect_current_project"),
        (3, "arkdock_claim_task"),
        (4, "arkdock_list_tasks"),
        (5, "arkdock_context_status"),
    ] {
        host.send(json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":tool,"arguments":{"contextId":"fixture-context"}}}));
        let reply = host.reply(id);
        let (current, event) = received.recv_timeout(TIMEOUT).unwrap();
        assert_eq!(event, tool, "business rejection must not disconnect");
        if let Some(owner) = owner {
            assert_eq!(current.connection_id, owner);
        } else {
            owner = Some(current.connection_id);
        }
        if tool == "arkdock_list_tasks" {
            assert_eq!(reply["result"]["isError"], true);
            assert!(reply["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("CONFIRMATION_REQUIRED"));
        }
        if tool == "arkdock_context_status" {
            assert_eq!(reply["result"]["structuredContent"]["claimActive"], true);
        }
    }
    let (heartbeat, tool) = received.recv_timeout(Duration::from_secs(6)).unwrap();
    assert_eq!(tool, "__heartbeat");
    assert_eq!(Some(heartbeat.connection_id), owner);
    host.close();
    let (disconnected, tool) = received.recv_timeout(TIMEOUT).unwrap();
    assert_eq!(tool, "__disconnect");
    assert_eq!(Some(disconnected.connection_id), owner);
    drop(server);
}
