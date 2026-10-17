use serde_json::{json, Value};

pub const INSTRUCTIONS: &str = "ArkDock connects this MCP client to an explicitly declared project and execution directory. Call arkdock_connect_current_project with the original project, actual execution directory and environment; retain the returned contextId. The user must confirm that context in ArkDock before task data is available. A context identifies this MCP connection, not a visible conversation, and needs no host chat ID. Never infer access to chat history. Tasks and immutable screenshots are held in memory for up to one hour and disappear when ArkDock restarts. Claim a task before reading its selection or images; include its claimId in reads and reports. When the user enables automatic processing for this session, repeatedly call arkdock_wait_for_task with the saved contextId. A status of waiting is a normal bounded timeout: wait again while automatic processing remains enabled. A task result is already claimed: save taskId/claimId, read its context and images, acknowledge, use your own editing and validation tools, report completed/failed, then wait for the next task. Process sequentially. Stop on user cancellation or any error; never automatically retry an uncertain mutation or reconnect to a guessed project. This active wait does not wake an idle host or bypass its permissions, timeouts or turn limits. Disconnected claims become unknown and require an explicit user retry. The desktop application must be running for business tools.";

fn string() -> Value {
    json!({"type":"string","minLength":1,"maxLength":4096})
}
fn tool(name: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    json!({"name":name,"description":description,"inputSchema":{"type":"object","properties":properties,"required":required,"additionalProperties":false}})
}

pub fn tools() -> Value {
    let paths = json!({"originalProject":string(),"executionDirectory":string(),"environment":{"type":"string","enum":["windowsLocal","wsl","remote"]},"displayName":string(),"contextId":string()});
    let scope = json!({"contextId":string()});
    let claim = json!({"contextId":string(),"taskId":string(),"claimId":string()});
    let mut image = claim.clone();
    image["image"] = json!({"type":"string","enum":["screen","crop"]});
    let mut report = claim.clone();
    report["status"] = json!({"type":"string","enum":["acknowledged","completed","failed"]});
    report["summary"] = string();
    report["validation"] = string();
    let mut claim_task = scope.clone();
    claim_task["taskId"] = string();
    let wait = json!({"contextId":string(),"timeoutMs":{"type":"integer","minimum":0,"maximum":25000,"default":25000}});
    json!([
        tool("arkdock_connect_current_project","Declare original project and actual execution directory. Retain contextId; user confirmation in ArkDock is required.",paths.clone(), &["originalProject","executionDirectory","environment"]),
        tool("arkdock_context_update","Update an owned context's explicit project/execution declaration; material changes require confirmation.",paths,&["contextId","originalProject","executionDirectory","environment"]),
        tool("arkdock_context_status","Inspect this connection's contexts and confirmation state.",scope.clone(),&[]),
        tool("arkdock_context_revoke","Revoke an owned context and its task access.",scope.clone(),&["contextId"]),
        tool("arkdock_list_tasks","List task summaries for a confirmed owned context.",scope,&["contextId"]),
        tool("arkdock_claim_task","Claim an available task. Claim ownership is connection scoped and retries are idempotent.",claim_task,&["contextId"]),
        tool("arkdock_wait_for_task","Wait up to timeoutMs (default/max 25000) and atomically claim an available task in a confirmed context. Returns the usual task/claimId on success, or status=waiting on a normal timeout. After user-enabled processing and reporting, wait again. Errors stop the loop; no idle host wakeup is implied.",wait,&["contextId"]),
        tool("arkdock_get_task_context","Read the immutable selection of a task owned by this claim.",claim.clone(),&["contextId","taskId","claimId"]),
        tool("arkdock_get_task_image","Read a claimed task's screen or crop as a standard MCP image with its actual MIME type.",image,&["contextId","taskId","claimId","image"]),
        tool("arkdock_report_task","Acknowledge or complete/fail a claimed task, with optional summary and validation.",report,&["contextId","taskId","claimId","status"])
    ])
}

pub fn known(name: &str) -> bool {
    tools()
        .as_array()
        .unwrap()
        .iter()
        .any(|tool| tool["name"] == name)
}
