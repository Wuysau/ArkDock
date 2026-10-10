//! Native Inspector receipt experiment, restricted to the repository's own fixture.
use arkdock_core::inspector::{InspectorCommand, RawInspectionResponse};
use arkdock_core::ipc::model::{Environment, ProjectRef};
use arkdock_core::preview::frames::BinaryFrame;
use arkdock_core::preview::{NativePoint, NativePreviewer, PreviewBackend, PreviewInput};
use arkdock_core::scheduler::BuildTask;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
fn frame(backend: &mut NativePreviewer, deadline: Instant) -> Result<BinaryFrame, String> {
    loop {
        if let Some(frame) = backend.binary_frame() {
            return Ok(frame);
        }
        if let Some((_, Err(error))) = backend.completion() {
            return Err(error);
        }
        for message in backend.runtime_messages() {
            println!("{message}");
            if message.starts_with("RUNTIME_FAILED:") {
                return Err(message);
            }
        }
        if Instant::now() > deadline {
            return Err("PROBE_FRAME_TIMEOUT".into());
        }
        std::thread::sleep(Duration::from_millis(16));
    }
}
fn query(
    backend: &mut NativePreviewer,
    command: InspectorCommand,
) -> Result<RawInspectionResponse, String> {
    let response = backend.request_inspection(command)?;
    response
        .recv_timeout(Duration::from_secs(3))
        .map_err(|error| format!("PROBE_RESPONSE: {error}"))?
}
fn save_response(
    output: &Path,
    name: &str,
    response: Result<RawInspectionResponse, String>,
    displayed: &BinaryFrame,
) -> Result<Value, String> {
    let receipt = match response {
        Ok(response) => {
            let parsed = if response.command == InspectorCommand::Inspector {
                match arkdock_core::inspector::parse_tree(&response, &displayed.metadata) {
                    Ok(tree) => {
                        fs::write(
                            output.join(format!("{name}-tree.json")),
                            serde_json::to_vec_pretty(&tree).map_err(|e| e.to_string())?,
                        )
                        .map_err(|e| e.to_string())?;
                        json!({"status":"PASS","nodeCount":tree.nodes.len()})
                    }
                    Err(error) => json!({"status":"BLOCKED","error":error}),
                }
            } else {
                json!({"status":"SKIPPED","reason":"Default attributes are not a live component tree"})
            };
            json!({"command":response.command.wire_name(),"protocolVersion":response.protocol_version, "result":response.result,"parse":parsed,
            "requestedAt":response.requested_at, "receivedAt":response.received_at,
            "queryMs": response.received_at.saturating_sub(response.requested_at),
            "frameSequence":response.frame_sequence,"queryId":response.query_id,
            "workspaceId":response.task.workspace_id,"sessionId":response.task.session_id,
            "buildId":response.task.build_id,"generation":response.task.generation,
            "exactFrameMatch":false})
        }
        Err(error) => json!({"error":error}),
    };
    fs::write(
        output.join(format!("{name}.json")),
        serde_json::to_vec_pretty(&receipt).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    println!(
        "{}",
        json!({"receipt":name,"queryMs":receipt.get("queryMs"),"resultBytes":receipt.get("result").and_then(|v|v.as_str()).map(str::len),"error":receipt.get("error")})
    );
    Ok(receipt)
}
fn save_frame(output: &Path, name: &str, frame: &BinaryFrame) -> Result<(), String> {
    fs::write(output.join(format!("{name}.jpg")), &frame.payload).map_err(|e| e.to_string())?;
    fs::write(
        output.join(format!("{name}-metadata.json")),
        serde_json::to_vec_pretty(&frame.metadata).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}
fn execute() -> Result<(), String> {
    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    if arguments.len() != 3 || arguments[0] != "--trust-fixture" {
        return Err(
            "Usage: arkdock-inspector-probe --trust-fixture <sdk-root> <fresh-output-name>".into(),
        );
    }
    let sdk = PathBuf::from(&arguments[1]);
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or("ROOT")?
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let name = arguments[2].to_str().ok_or("OUTPUT_ENCODING")?;
    if name.is_empty()
        || name.len() > 128
        || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return Err("OUTPUT_NAME".into());
    }
    let parent = root.join("artifacts/native-preview");
    fs::create_dir_all(&parent).map_err(|e| e.to_string())?;
    if !parent
        .canonicalize()
        .map_err(|e| e.to_string())?
        .starts_with(&root)
    {
        return Err("OUTPUT_OUTSIDE_WORKTREE".into());
    }
    let output = parent.join(name);
    fs::create_dir(&output).map_err(|e| format!("OUTPUT_MUST_BE_FRESH: {e}"))?;
    let fixture = root
        .join("examples/native-counter")
        .canonicalize()
        .map_err(|e| e.to_string())?;
    if !fixture.starts_with(&root) {
        return Err("FIXTURE_OUTSIDE_WORKTREE".into());
    }
    let project = ProjectRef {
        id: format!(
            "inspector-fixture-{:x}",
            Sha256::digest(fixture.to_string_lossy().as_bytes())
        ),
        name: "ArkDock Inspector native counter".into(),
        environment: Environment::WindowsLocal,
        workspace_root: fixture.to_string_lossy().into_owned(),
        app_root: fixture.to_string_lossy().into_owned(),
        module: "entry".into(),
        variant: "default".into(),
        sdk_id: None,
    };
    let task = BuildTask {
        epoch: 1,
        workspace_id: project.id.clone(),
        session_id: format!("inspector-probe-{}-{}", std::process::id(), now_ms()),
        generation: 1,
        build_id: "inspector-baseline".into(),
        started_at: now_ms(),
    };
    let mut backend = NativePreviewer::new(Some(sdk.clone()));
    let environment = arkdock_core::preview::environment::inspect_environment(Some(&sdk));
    fs::write(
        output.join("environment.json"),
        serde_json::to_vec_pretty(&environment).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let capability = backend.probe();
    if !capability.available {
        return Err(capability.reason);
    }
    backend.start(&project, true, &task.session_id)?;
    backend.refresh(&task)?;
    let first = frame(&mut backend, Instant::now() + Duration::from_secs(140))?;
    save_frame(&output, "before", &first)?;
    let mut receipts = Vec::new();
    receipts.push(save_response(
        &output,
        "inspector-before",
        query(&mut backend, InspectorCommand::Inspector),
        &first,
    )?);
    receipts.push(save_response(
        &output,
        "inspector-default",
        query(&mut backend, InspectorCommand::Default),
        &first,
    )?);
    let click_start = now_ms();
    backend.input(PreviewInput::Click(NativePoint { x: 180, y: 361 }))?;
    std::thread::sleep(Duration::from_millis(200));
    let after = frame(&mut backend, Instant::now() + Duration::from_secs(5))?;
    save_frame(&output, "after", &after)?;
    receipts.push(save_response(
        &output,
        "inspector-after",
        query(&mut backend, InspectorCommand::Inspector),
        &after,
    )?);
    let settle_until = Instant::now() + Duration::from_secs(1);
    let mut settled = after.clone();
    while Instant::now() < settle_until {
        if let Some(frame) = backend.binary_frame() {
            settled = frame;
        }
        std::thread::sleep(Duration::from_millis(16));
    }
    save_frame(&output, "settled", &settled)?;
    receipts.push(save_response(
        &output,
        "inspector-settled",
        query(&mut backend, InspectorCommand::Inspector),
        &settled,
    )?);
    for index in 0..12 {
        receipts.push(save_response(
            &output,
            &format!("inspector-repeat-{index:02}"),
            query(&mut backend, InspectorCommand::Inspector),
            &settled,
        )?);
    }
    let messages = backend.runtime_messages();
    for message in &messages {
        println!("{message}");
    }
    backend.stop();
    let available = receipts
        .iter()
        .all(|receipt| receipt.get("error").is_none())
        && receipts
            .iter()
            .filter(|receipt| receipt["command"] == "inspector")
            .count()
            == 15
        && receipts
            .iter()
            .filter(|receipt| receipt["command"] == "inspector")
            .all(|receipt| receipt["parse"]["status"] == "PASS");
    let report = json!({"status":if available{"PASS"}else{"BLOCKED"},"startedAt":task.started_at,"finishedAt":now_ms(),"sdkVersion":capability.sdk_version,"command":["arkdock-inspector-probe","--trust-fixture","<sdk-root>",name],"receipts":receipts,"clickStartedAt":click_start,"firstFrame":first.metadata,"afterFrame":after.metadata,"backendMessages":messages,"exactFrameMatch":false,"reason":if available{"Native response requires field/geometry validation before product admission"}else{"Actual commercial Inspector returned no usable tree"}});
    fs::write(
        output.join("report.json"),
        serde_json::to_vec_pretty(&report).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    println!("{}", json!({"status":report["status"],"evidence":output}));
    Ok(())
}
fn main() {
    if let Err(error) = execute() {
        eprintln!("{}", json!({"status":"BLOCKED","error":error}));
        std::process::exit(2);
    }
}
