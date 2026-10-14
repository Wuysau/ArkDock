use super::*;
use crate::selection::test_support::{context, snapshot};
use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream};
use std::time::Duration;

struct Fixture {
    bridge: SessionBridge,
    pair: AgentPairing,
    snapshot: AppSnapshot,
    _directory: tempfile::TempDir,
}

#[test]
fn accepted_final_receipt_survives_immediate_revoke_before_actor_status_poll() {
    let f = Fixture::new(100, false);
    f.handle("/v1/register", f.register_body(), 100).unwrap();
    f.submit(101);
    f.handle("/v1/poll", f.poll_body(), 102).unwrap();
    f.handle("/v1/receipt", f.receipt_body("TRANSPORT_ACCEPTED"), 103)
        .unwrap();
    f.handle("/v1/receipt", f.receipt_body("UNKNOWN"), 104)
        .unwrap();
    f.handle("/v1/revoke", json!({"sessionId":"host-session"}), 105)
        .unwrap();
    let update = f
        .bridge
        .delivery_status("delivery-fixture", 106)
        .expect("accepted receipt survives revoked transport");
    assert_eq!(update.state, DeliveryState::Unknown);
    assert_eq!(update.receipt_id.as_deref(), Some("receipt-UNKNOWN"));
    assert_eq!(
        update.transport_receipt.as_deref(),
        Some("receipt-TRANSPORT_ACCEPTED")
    );
    assert!(f.bridge.registrations(106).is_empty());
    assert_eq!(
        f.handle("/v1/poll", f.poll_body(), 106).unwrap_err(),
        "AGENT_PAIR_REQUIRED"
    );
    assert!(f
        .bridge
        .delivery_status("delivery-fixture", 3_700_000)
        .is_none());
}

impl Fixture {
    fn new(now: u64, live_http: bool) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let mut state = snapshot();
        let root = directory.path().to_string_lossy().into_owned();
        let project = state.project.as_mut().unwrap();
        project.workspace_root = root.clone();
        project.app_root = root;
        let bridge = if live_http {
            SessionBridge::start().unwrap()
        } else {
            SessionBridge::detached()
        };
        // Same commit boundary as production: the credential is staged, then
        // activated. This fixture keeps no credential file.
        let prepared = bridge.prepare_pair(&state, AgentKind::Claude, now).unwrap();
        let pair = bridge.activate_pair(prepared, &state, now).unwrap();
        Self {
            bridge,
            pair,
            snapshot: state,
            _directory: directory,
        }
    }

    fn register_body(&self) -> Value {
        json!({"transportKind":"claude-channel","instanceId":"channel-fixture",
            "agentKind":"claude","agentVersion":"2.1.284-fixture","sessionId":"host-session",
            "conversationId":"host-session","workspaceRoot":self.pair.workspace_root,
            "pid":std::process::id(),"capabilities":{"identifyCurrentSession":true,
            "directSend":true,"images":true,"fileReferences":false,"busyState":false,
            "receipt":true,"cancelPending":false}})
    }

    fn poll_body(&self) -> Value {
        json!({"instanceId":"channel-fixture","sessionId":"host-session",
            "conversationId":"host-session","workspaceRoot":self.pair.workspace_root,"busy":false})
    }

    fn receipt_body(&self, state: &str) -> Value {
        json!({"instanceId":"channel-fixture","sessionId":"host-session",
            "conversationId":"host-session","deliveryId":"delivery-fixture",
            "state":state,"receiptId":format!("receipt-{state}"),"reason":"Protocol fixture only"})
    }

    fn handle(&self, route: &str, body: Value, now: u64) -> Result<Value, String> {
        handle(
            &mut self.bridge.shared.lock().unwrap(),
            &self.pair.token,
            route,
            body,
            now,
        )
    }

    fn binding(&self, now: u64) -> AgentSessionBinding {
        let registration = self.bridge.registrations(now).pop().unwrap();
        AgentSessionBinding {
            binding_id: "binding-fixture".into(),
            agent_kind: AgentKind::Claude,
            target_id: "target".into(),
            process: self.snapshot.target.as_ref().unwrap().process.clone(),
            workspace_id: self.pair.workspace_id.clone(),
            agent_session_id: registration.agent_session_id,
            conversation_or_thread_id: registration.conversation_or_thread_id,
            transport_kind: registration.transport_kind,
            writable: true,
            capabilities: registration.capabilities,
            observed_at: now,
            expires_at: now + LEASE_MS,
            reason: "Explicit fixture binding".into(),
            transport_instance_id: Some(registration.transport_instance_id),
            agent_version: Some(registration.agent_version),
            verified_at: now,
            binding_evidence: "Unit fixture, not a Claude host receipt".into(),
        }
    }

    fn submit(&self, now: u64) {
        self.submit_selection(context(now), Attachments::default(), now);
    }

    fn submit_selection(&self, selection: SelectionContext, attachments: Attachments, now: u64) {
        let binding = self.binding(now);
        let delivery = AgentDelivery {
            request_id: "delivery-fixture".into(),
            selection_id: selection.selection_id.clone(),
            binding_id: Some(binding.binding_id.clone()),
            state: DeliveryState::Sending,
            prompt: "Change the selected component; protocol fixture only".into(),
            target: Some(binding.clone()),
            receipt_id: None,
            reason: String::new(),
            created_at: now,
            updated_at: now,
            idempotency_key: "fixture-idempotency".into(),
            delivery_mode: DeliveryMode::Native,
            transport_receipt: None,
            agent_acknowledgment: None,
        };
        self.bridge
            .submit(&binding, &delivery, &selection, attachments, now)
            .unwrap();
    }

    fn http(&self, route: &str, body: Value, extra_headers: &str) -> (u16, Value) {
        let address = self.bridge.endpoint.strip_prefix("http://").unwrap();
        let encoded = body.to_string();
        let mut stream = TcpStream::connect(address).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        write!(stream, "POST {route} HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{extra_headers}\r\n{encoded}",
            self.pair.token, encoded.len()).unwrap();
        stream.shutdown(Shutdown::Write).unwrap();
        // An early rejected request can close with unread request bytes and cause
        // a Windows TCP reset after a complete response. Honor Content-Length;
        // never treat either EOF or reset as evidence that a response is complete.
        let mut header = Vec::new();
        while !header.ends_with(b"\r\n\r\n") {
            assert!(header.len() < 8192);
            let mut byte = [0u8; 1];
            stream.read_exact(&mut byte).unwrap();
            header.push(byte[0]);
        }
        let headers = String::from_utf8(header).unwrap();
        let length: usize = headers
            .lines()
            .find_map(|line| line.strip_prefix("Content-Length: "))
            .unwrap()
            .parse()
            .unwrap();
        assert!(length <= 128 * 1024);
        let mut body = vec![0u8; length];
        stream.read_exact(&mut body).unwrap();
        let code = headers.split_whitespace().nth(1).unwrap().parse().unwrap();
        (code, serde_json::from_slice(&body).unwrap())
    }
}

#[cfg(windows)]
#[test]
fn actual_rust_bridge_node_channel_and_selection_provider_roundtrip() {
    use crate::preview::frames::BinaryFrame;
    use crate::selection::SelectionStore;
    use image::{DynamicImage, ImageFormat, RgbImage};
    use std::io::{BufRead, BufReader, Cursor};
    use std::os::windows::process::CommandExt;
    use std::process::{Child, Command, Stdio};
    use std::sync::mpsc;
    use std::time::Instant;

    struct OwnChild(Child);
    impl Drop for OwnChild {
        fn drop(&mut self) {
            if self.0.try_wait().ok().flatten().is_none() {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
    }
    let now = crate::selection::now_ms();
    let mut f = Fixture::new(now, true);
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    let script = root.join("tools/agent/claude-channel.mjs");
    let pair_file = f._directory.path().join("channel-pairing.json");
    std::fs::write(&pair_file, serde_json::to_vec(&f.pair).unwrap()).unwrap();
    let command = || {
        let mut command = Command::new("node");
        command
            .arg(&script)
            .current_dir(f._directory.path())
            .env("CLAUDE_CODE_SESSION_ID", "host-session")
            .creation_flags(0x0800_0000)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    };
    let mut hook = command()
        .args(["--hook", "--pairing-file"])
        .arg(&pair_file)
        .spawn()
        .expect("Node is required for the Windows cross-language Channel test");
    hook.stdin
        .take()
        .unwrap()
        .write_all(
            json!({"hook_event_name":"SessionStart",
        "session_id":"host-session","source":"startup","cwd":f.pair.workspace_root})
            .to_string()
            .as_bytes(),
        )
        .unwrap();
    let hook_result = hook.wait_with_output().unwrap();
    assert!(
        hook_result.status.success(),
        "SessionStart fixture failed: {}",
        String::from_utf8_lossy(&hook_result.stderr)
    );

    // Real persisted SelectionStore artifacts; synthetic JPEG, never native-preview acceptance.
    let mut store = SelectionStore::open(f._directory.path()).unwrap();
    let mut image = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(RgbImage::from_pixel(360, 640, image::Rgb([12, 34, 56])))
        .write_to(&mut image, ImageFormat::Jpeg)
        .unwrap();
    let payload = image.into_inner();
    let frame = BinaryFrame {
        metadata: BinaryFrameMetadata {
            protocol_version: 1,
            workspace_id: "workspace".into(),
            session_id: "session".into(),
            build_id: "build".into(),
            generation: 1,
            sequence: 1,
            width: 360,
            height: 640,
            captured_at: now,
            rotation: 0,
            format: NativeFrameFormat::Jpeg,
            payload_length: payload.len() as u32,
        },
        payload,
    };
    let selection = store
        .create(
            &f.snapshot,
            &frame,
            &SelectionDraft {
                frame: frame.metadata.clone(),
                bounds: InspectorBounds {
                    x: 0.0,
                    y: 0.0,
                    width: 10.0,
                    height: 10.0,
                },
                node_id: None,
                inspection_version: None,
                user_instruction: "Cross-language fixture request".into(),
            },
            None,
            now,
        )
        .unwrap();
    store.sync_runtime(&f.snapshot, now).unwrap();
    let attachments = store
        .authorize_delivery(
            &f.snapshot,
            &selection.selection_id,
            "delivery-fixture",
            now,
        )
        .unwrap();
    let mut child = OwnChild(
        command()
            .arg("--pairing-file")
            .arg(&pair_file)
            .spawn()
            .unwrap(),
    );
    let stdout = child.0.stdout.take().unwrap();
    let (sender, receiver) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let message: Value = serde_json::from_str(&line.unwrap()).unwrap();
            if sender.send(message).is_err() {
                break;
            }
        }
    });
    let mut input = child.0.stdin.take().unwrap();
    let send = |input: &mut std::process::ChildStdin, id: u64, method: &str, params: Value| {
        writeln!(
            input,
            "{}",
            json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
        )
        .unwrap();
    };
    let receive = |f: &Fixture| {
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            f.bridge.sync(&f.snapshot, crate::selection::now_ms());
            match receiver.recv_timeout(Duration::from_millis(50)) {
                Ok(message) => return message,
                Err(mpsc::RecvTimeoutError::Timeout) if Instant::now() < until => {}
                error => {
                    panic!("Channel fixture did not return a complete stdio message: {error:?}")
                }
            }
        }
    };
    send(
        &mut input,
        1,
        "initialize",
        json!({"protocolVersion":"2025-06-18",
        "clientInfo":{"name":"claude-code","version":"2.1.284-fixture"}}),
    );
    assert_eq!(
        receive(&f)["result"]["capabilities"]["experimental"]["claude/channel"],
        json!({})
    );
    writeln!(
        input,
        "{}",
        json!({"jsonrpc":"2.0","method":"notifications/initialized"})
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while f
        .bridge
        .registrations(crate::selection::now_ms())
        .is_empty()
    {
        if let Some(status) = child.0.try_wait().unwrap() {
            let mut error = String::new();
            child
                .0
                .stderr
                .take()
                .unwrap()
                .read_to_string(&mut error)
                .unwrap();
            panic!("Channel exited before registration ({status}): {error}");
        }
        assert!(
            Instant::now() < deadline,
            "Channel did not register with real Rust Bridge"
        );
        f.bridge.sync(&f.snapshot, crate::selection::now_ms());
        std::thread::sleep(Duration::from_millis(10));
    }
    f.submit_selection(selection.clone(), attachments, crate::selection::now_ms());
    let notification = receive(&f);
    assert_eq!(notification["method"], "notifications/claude/channel");
    assert_eq!(
        notification["params"]["meta"]["delivery_id"],
        "delivery-fixture"
    );
    assert_eq!(notification["params"]["meta"]["session_id"], "host-session");
    let deadline = Instant::now() + Duration::from_secs(5);
    while f
        .bridge
        .delivery_status("delivery-fixture", crate::selection::now_ms())
        .unwrap()
        .state
        == DeliveryState::Sending
    {
        assert!(Instant::now() < deadline);
        f.bridge.sync(&f.snapshot, crate::selection::now_ms());
        std::thread::sleep(Duration::from_millis(10));
    }
    send(
        &mut input,
        2,
        "tools/call",
        json!({"name":"arkdock.get_selection_screenshot",
        "arguments":{"deliveryId":"delivery-fixture","selectionId":selection.selection_id}}),
    );
    let screenshot = receive(&f);
    assert_eq!(screenshot["id"], 2);
    assert_eq!(
        screenshot["result"]["content"][0]["mimeType"], "image/jpeg",
        "Image tool returned: {screenshot}"
    );
    assert!(screenshot["result"]["content"][0]["data"]
        .as_str()
        .is_some_and(|data| !data.is_empty()));
    assert_eq!(
        f.bridge
            .delivery_status("delivery-fixture", crate::selection::now_ms())
            .unwrap()
            .state,
        DeliveryState::TransportAccepted
    );
    f.snapshot.build.generation += 1;
    f.snapshot.build.stale = true;
    f.bridge.sync(&f.snapshot, crate::selection::now_ms());
    store
        .sync_runtime(&f.snapshot, crate::selection::now_ms())
        .unwrap();
    send(
        &mut input,
        3,
        "tools/call",
        json!({"name":"arkdock.get_selection",
        "arguments":{"deliveryId":"delivery-fixture","selectionId":selection.selection_id}}),
    );
    assert_eq!(receive(&f)["result"]["isError"], true);
    send(
        &mut input,
        4,
        "tools/call",
        json!({"name":"arkdock.acknowledge_delivery","arguments":{"deliveryId":"delivery-fixture"}}),
    );
    assert_ne!(receive(&f)["result"]["isError"], true);
    send(
        &mut input,
        5,
        "tools/call",
        json!({"name":"arkdock.complete_delivery","arguments":{
        "deliveryId":"delivery-fixture","success":true,"summary":"Fixture outcome after native rebuild"}}),
    );
    assert_ne!(receive(&f)["result"]["isError"], true);
    assert_eq!(
        f.bridge
            .delivery_status("delivery-fixture", crate::selection::now_ms())
            .unwrap()
            .state,
        DeliveryState::Completed
    );
    drop(input);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        assert!(
            Instant::now() < deadline,
            "Channel did not close on stdio EOF"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    reader.join().unwrap();
    assert!(f
        .bridge
        .registrations(crate::selection::now_ms())
        .is_empty());
}

#[test]
fn registration_rejects_bad_auth_wrong_kind_scope_and_reuse() {
    let f = Fixture::new(100, false);
    assert_eq!(
        handle(
            &mut f.bridge.shared.lock().unwrap(),
            "wrong-secret",
            "/v1/register",
            f.register_body(),
            100
        )
        .unwrap_err(),
        "AGENT_UNAUTHORIZED"
    );
    let mut wrong_kind = f.register_body();
    wrong_kind["agentKind"] = json!("codex");
    assert_eq!(
        f.handle("/v1/register", wrong_kind, 100).unwrap_err(),
        "AGENT_PAIR_EXPIRED_OR_WRONG_KIND"
    );
    let mut wrong_scope = f.register_body();
    wrong_scope["workspaceRoot"] = json!("missing-fixture-directory");
    assert_eq!(
        f.handle("/v1/register", wrong_scope, 100).unwrap_err(),
        "AGENT_REGISTRATION_SCOPE_INVALID"
    );
    let mut extra = f.register_body();
    extra["arbitraryCommand"] = json!("must-never-run");
    assert_eq!(
        f.handle("/v1/register", extra, 100).unwrap_err(),
        "AGENT_REGISTRATION_INVALID"
    );
    let result = f.handle("/v1/register", f.register_body(), 100).unwrap();
    assert!(result["registrationId"].is_string());
    assert_eq!(result["expiresAt"], 100 + LEASE_MS);
    assert_eq!(
        f.handle("/v1/register", f.register_body(), 101)
            .unwrap_err(),
        "AGENT_PAIR_ALREADY_USED"
    );
    assert!(!f.bridge.registrations(101)[0].capabilities.busy_state);
}

#[test]
fn current_scope_and_expired_transport_lease_cannot_be_reactivated_by_poll() {
    let f = Fixture::new(100, false);
    f.handle("/v1/register", f.register_body(), 100).unwrap();
    let binding = f.binding(101);
    let mut changed = f.poll_body();
    changed["sessionId"] = json!("different-session");
    assert_eq!(
        f.handle("/v1/poll", changed, 101).unwrap_err(),
        "AGENT_SESSION_CHANGED_REPAIR_REQUIRED"
    );
    f.bridge.sync(&f.snapshot, 100 + LEASE_MS);
    assert_eq!(
        f.handle("/v1/poll", f.poll_body(), 100 + LEASE_MS)
            .unwrap_err(),
        "AGENT_SESSION_CHANGED_REPAIR_REQUIRED"
    );
    assert_eq!(
        f.bridge.verify(&binding, 100 + LEASE_MS).err().unwrap(),
        "AGENT_TRANSPORT_LEASE_EXPIRED"
    );
    assert!(f.bridge.registrations(100 + LEASE_MS).is_empty());
}

#[test]
fn delivery_is_dequeued_once_and_receipts_require_claim_scope_and_order() {
    let f = Fixture::new(100, false);
    f.handle("/v1/register", f.register_body(), 100).unwrap();
    f.submit(101);
    f.submit(101);
    assert_eq!(
        f.handle("/v1/receipt", f.receipt_body("TRANSPORT_ACCEPTED"), 102)
            .unwrap_err(),
        "AGENT_RECEIPT_TRANSITION_INVALID"
    );
    let delivery = f.handle("/v1/poll", f.poll_body(), 102).unwrap();
    assert_eq!(delivery["deliveries"].as_array().unwrap().len(), 1);
    assert_eq!(delivery["deliveries"][0]["sessionId"], "host-session");
    assert_eq!(delivery["deliveries"][0]["previewSessionId"], "session");
    assert_eq!(
        f.handle("/v1/poll", f.poll_body(), 103).unwrap()["deliveries"],
        json!([])
    );
    assert_eq!(
        f.handle("/v1/receipt", f.receipt_body("COMPLETED"), 103)
            .unwrap_err(),
        "AGENT_RECEIPT_TRANSITION_INVALID"
    );
    let mut wrong = f.receipt_body("TRANSPORT_ACCEPTED");
    wrong["conversationId"] = json!("other-conversation");
    assert_eq!(
        f.handle("/v1/receipt", wrong, 103).unwrap_err(),
        "AGENT_RECEIPT_SCOPE_MISMATCH"
    );
    let mut extra = f.receipt_body("TRANSPORT_ACCEPTED");
    extra["workspaceRoot"] = json!(f.pair.workspace_root);
    assert_eq!(
        f.handle("/v1/receipt", extra, 103).unwrap_err(),
        "AGENT_RECEIPT_INVALID"
    );
    for (index, state) in ["TRANSPORT_ACCEPTED", "AGENT_ACKNOWLEDGED", "COMPLETED"]
        .iter()
        .enumerate()
    {
        assert_eq!(
            f.handle("/v1/receipt", f.receipt_body(state), 104 + index as u64)
                .unwrap(),
            json!({"accepted":true})
        );
    }
    assert_eq!(
        f.bridge
            .delivery_status("delivery-fixture", 107)
            .unwrap()
            .state,
        DeliveryState::Completed
    );
    assert_eq!(
        f.handle("/v1/receipt", f.receipt_body("TRANSPORT_ACCEPTED"), 107)
            .unwrap_err(),
        "AGENT_RECEIPT_TRANSITION_INVALID"
    );
}

#[test]
fn unacknowledged_timeout_is_unknown_and_never_replayed() {
    let f = Fixture::new(100, false);
    f.handle("/v1/register", f.register_body(), 100).unwrap();
    f.submit(101);
    f.handle("/v1/poll", f.poll_body(), 102).unwrap();
    let later = 101 + RECEIPT_TIMEOUT_MS + 1;
    assert_eq!(
        f.bridge
            .delivery_status("delivery-fixture", later)
            .unwrap()
            .state,
        DeliveryState::Unknown
    );
    // Refresh host/runtime leases in the fixture to isolate the no-replay rule.
    f.bridge.sync(&f.snapshot, later);
    f.bridge
        .shared
        .lock()
        .unwrap()
        .pair
        .as_mut()
        .unwrap()
        .registration
        .as_mut()
        .unwrap()
        .expires_at = later + LEASE_MS;
    assert_eq!(
        f.handle("/v1/poll", f.poll_body(), later).unwrap()["deliveries"],
        json!([])
    );
    assert_eq!(
        f.bridge
            .delivery_status("delivery-fixture", later)
            .unwrap()
            .state,
        DeliveryState::Unknown
    );
}

#[test]
fn queued_selection_rebuild_expires_and_target_change_revokes_pair() {
    let mut f = Fixture::new(100, false);
    f.handle("/v1/register", f.register_body(), 100).unwrap();
    f.submit(101);
    f.snapshot.build.generation += 1;
    f.bridge.sync(&f.snapshot, 102);
    assert_eq!(
        f.bridge
            .delivery_status("delivery-fixture", 102)
            .unwrap()
            .state,
        DeliveryState::Expired
    );
    assert_eq!(
        f.handle("/v1/poll", f.poll_body(), 102).unwrap()["deliveries"],
        json!([])
    );
    f.snapshot.target.as_mut().unwrap().process.started_at = "different-lifecycle".into();
    f.bridge.sync(&f.snapshot, 103);
    assert!(f.bridge.registrations(103).is_empty());
    assert_eq!(
        f.handle("/v1/poll", f.poll_body(), 103).unwrap_err(),
        "AGENT_PAIR_REQUIRED"
    );
}

#[test]
fn actual_http_server_enforces_browser_isolation_and_channel_wire_contract() {
    let now = crate::selection::now_ms();
    let f = Fixture::new(now, true);
    assert!(f.bridge.endpoint.starts_with("http://127.0.0.1:"));
    let (code, rejected) = f.http(
        "/v1/register",
        f.register_body(),
        "Origin: https://untrusted.invalid\r\n",
    );
    assert_eq!(code, 400);
    assert_eq!(rejected["error"], "AGENT_REQUEST_ORIGIN_REJECTED");
    let (code, registered) = f.http("/v1/register", f.register_body(), "");
    assert_eq!(code, 200);
    assert!(registered["registrationId"].is_string());
    f.submit(crate::selection::now_ms());
    let (code, polled) = f.http("/v1/poll", f.poll_body(), "");
    assert_eq!(code, 200);
    assert_eq!(polled["deliveries"][0]["deliveryId"], "delivery-fixture");
    for state in ["TRANSPORT_ACCEPTED", "AGENT_ACKNOWLEDGED", "COMPLETED"] {
        assert_eq!(
            f.http("/v1/receipt", f.receipt_body(state), ""),
            (200, json!({"accepted":true}))
        );
    }
    assert_eq!(
        f.http("/v1/revoke", json!({"sessionId":"wrong-session"}), "")
            .0,
        400
    );
    assert_eq!(
        f.http("/v1/revoke", json!({"sessionId":"host-session"}), ""),
        (200, json!({"accepted":true}))
    );
    assert_eq!(
        f.http("/v1/poll", f.poll_body(), "").1["error"],
        "AGENT_PAIR_REQUIRED"
    );
}
