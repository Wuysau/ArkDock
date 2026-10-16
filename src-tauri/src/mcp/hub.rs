//! In-memory, connection-owned MCP contexts and immutable task evidence.
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use base64::Engine;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::context::{ContextHub, ContextMessage, MAX_SESSIONS};
use crate::ipc::model::{
    AppSnapshot, Environment, McpContext, McpTaskState, McpTaskSummary, ProjectRef,
    SelectionContext, SelectionImage,
};

use super::ConnectionOwner;

const MAX_TASKS: usize = 64;
const MAX_IMAGE_BYTES: usize = 1024 * 1024;
const TASK_TTL_MS: u64 = 60 * 60 * 1000;
const MAX_REPORT_BYTES: usize = 16 * 1024;

struct ContextRecord {
    view: McpContext,
    owner: ConnectionOwner,
    sequence: u64,
    revoked: bool,
}

struct TaskRecord {
    view: McpTaskSummary,
    selection: SelectionContext,
    screenshot: Vec<u8>,
    crop: Vec<u8>,
    project: ProjectRef,
    registration_id: String,
    claim: Option<(ConnectionOwner, String)>,
    claimed_once: bool,
}

#[derive(Default)]
struct Inner {
    contexts: HashMap<String, ContextRecord>,
    tasks: Vec<TaskRecord>,
    snapshot: Option<AppSnapshot>,
}

pub struct McpHub {
    context: Arc<ContextHub>,
    token: String,
    inner: Mutex<Inner>,
}

impl McpHub {
    pub fn new(context: Arc<ContextHub>, token: [u8; 32]) -> Self {
        context.activate_token(token);
        Self {
            context,
            token: token.iter().map(|byte| format!("{byte:02x}")).collect(),
            inner: Mutex::new(Inner::default()),
        }
    }

    pub fn call(
        &self,
        owner: &ConnectionOwner,
        tool: &str,
        args: Value,
        now: u64,
    ) -> Result<Value, String> {
        if !args.is_object() {
            return Err("MCP_ARGUMENTS_INVALID".into());
        }
        validate_arguments(tool, &args)?;
        let mut inner = self.lock();
        self.refresh(&mut inner, now);
        match tool {
            "arkdock_connect_current_project" | "arkdock_context_update" => {
                let original = required(&args, "originalProject")?.to_owned();
                let execution = required(&args, "executionDirectory")?.to_owned();
                let environment: Environment = serde_json::from_value(
                    args.get("environment")
                        .cloned()
                        .ok_or("MCP_ENVIRONMENT_REQUIRED")?,
                )
                .map_err(|_| "MCP_ENVIRONMENT_INVALID")?;
                let display = optional(&args, "displayName", 128)?
                    .unwrap_or_else(|| owner.client_name.clone());
                if display.is_empty() || display.chars().any(char::is_control) {
                    return Err("MCP_DISPLAY_NAME_INVALID".into());
                }
                let requested = optional(&args, "contextId", 128)?;
                if tool == "arkdock_context_update" && requested.is_none() {
                    return Err("MCP_CONTEXT_ID_REQUIRED".into());
                }
                let id = match requested {
                    Some(id) => {
                        let previous = inner
                            .contexts
                            .get(&id)
                            .filter(|r| {
                                !r.revoked
                                    && (same_owner(&r.owner, owner)
                                        || (!r.view.connected && r.owner.process == owner.process))
                            })
                            .ok_or("MCP_CONTEXT_UNAVAILABLE")?;
                        if tool == "arkdock_context_update" && !same_owner(&previous.owner, owner) {
                            return Err("MCP_CONTEXT_UNAVAILABLE".into());
                        }
                        id
                    }
                    None => {
                        // Retired declarations are not permanent capacity reservations.
                        // Keep disconnected contexts only while immutable tasks still
                        // need their scope (notably unknown claims awaiting UI retry).
                        let tasks = &inner.tasks;
                        let retained: Vec<_> = inner
                            .contexts
                            .iter()
                            .filter(|(id, record)| {
                                record.view.connected
                                    || (!record.revoked
                                        && tasks.iter().any(|task| {
                                            task.view.context_id == **id
                                                && task.view.expires_at > now
                                                && task.view.state != McpTaskState::Expired
                                        }))
                            })
                            .map(|(id, _)| id.clone())
                            .collect();
                        inner.contexts.retain(|id, _| retained.contains(id));
                        if inner.contexts.len() >= MAX_SESSIONS {
                            return Err("MCP_CONTEXT_LIMIT".into());
                        }
                        random_id("ctx-")?
                    }
                };
                let previous = inner.contexts.get(&id);
                let sequence = previous.map_or(Ok(1), |r| next_sequence(r.sequence))?;
                let live = previous.is_some_and(|r| r.view.connected);
                let message = if live {
                    ContextMessage::Update {
                        token: self.token.clone(),
                        session: id.clone(),
                        sequence,
                        roots: vec![],
                        cwd: None,
                        agent: Some(owner.client_name.clone()),
                        host_session: None,
                        original_project: Some(original.clone()),
                        execution_directory: Some(execution.clone()),
                        environment: Some(environment.clone()),
                    }
                } else {
                    ContextMessage::Hello {
                        token: self.token.clone(),
                        session: id.clone(),
                        sequence,
                        roots: vec![],
                        cwd: None,
                        agent: Some(owner.client_name.clone()),
                        host_session: None,
                        original_project: Some(original.clone()),
                        execution_directory: Some(execution.clone()),
                        environment: Some(environment.clone()),
                    }
                };
                self.context
                    .apply(message, owner.process.clone(), owner.connection_id, now)?;
                let registration = self
                    .context
                    .live(now)
                    .into_iter()
                    .find(|r| r.session == id)
                    .ok_or("MCP_CONTEXT_REGISTRATION_MISSING")?;
                let changed = previous
                    .is_some_and(|r| r.view.registration_id != registration.registration_id);
                let view = McpContext {
                    context_id: id.clone(),
                    registration_id: registration.registration_id,
                    client_name: owner.client_name.clone(),
                    display_name: display,
                    original_project: original,
                    execution_directory: execution,
                    environment,
                    confirmed: false,
                    connected: true,
                };
                inner.contexts.insert(
                    id.clone(),
                    ContextRecord {
                        view: view.clone(),
                        owner: owner.clone(),
                        sequence,
                        revoked: false,
                    },
                );
                if changed {
                    for task in &mut inner.tasks {
                        if task.view.context_id == id && task.view.state != McpTaskState::Unknown {
                            expire(task, now);
                        }
                    }
                }
                self.refresh(&mut inner, now);
                success(
                    serde_json::to_value(&inner.contexts[&id].view)
                        .map_err(|_| "MCP_SERIALIZATION_FAILED")?,
                )
            }
            "__heartbeat" => {
                for record in inner
                    .contexts
                    .values_mut()
                    .filter(|r| same_owner(&r.owner, owner) && r.view.connected && !r.revoked)
                {
                    let sequence = next_sequence(record.sequence)?;
                    self.context.apply(
                        ContextMessage::Heartbeat {
                            token: self.token.clone(),
                            session: record.view.context_id.clone(),
                            sequence,
                        },
                        owner.process.clone(),
                        owner.connection_id,
                        now,
                    )?;
                    record.sequence = sequence;
                }
                self.refresh(&mut inner, now);
                success(json!({"ok":true}))
            }
            "__disconnect" => {
                // A connection cannot disconnect another connection, even in the same process.
                if inner.contexts.values().any(|r| same_owner(&r.owner, owner)) {
                    self.context.remove_connection(owner.connection_id);
                    self.refresh(&mut inner, now);
                }
                success(json!({"ok":true}))
            }
            "arkdock_context_status" => {
                let id = optional(&args, "contextId", 128)?;
                if let Some(id) = &id {
                    owned_context(&inner, id, owner)?;
                }
                let mut contexts: Vec<_> = inner
                    .contexts
                    .values()
                    .filter(|r| {
                        same_owner(&r.owner, owner)
                            && !r.revoked
                            && id.as_ref().is_none_or(|id| *id == r.view.context_id)
                    })
                    .map(|r| r.view.clone())
                    .collect();
                contexts.sort_by(|a, b| a.context_id.cmp(&b.context_id));
                success(
                    json!({"contexts":contexts,"queuePersistence":"memoryOnly","taskTtlMs":TASK_TTL_MS}),
                )
            }
            "arkdock_context_revoke" => {
                let id = required(&args, "contextId")?;
                owned_context(&inner, id, owner)?;
                let revoked = self.revoke_locked(&mut inner, id, now)?;
                success(json!({"revoked":revoked}))
            }
            "arkdock_list_tasks" => {
                let id = required(&args, "contextId")?;
                owned_context(&inner, id, owner)?;
                require_confirmed(&inner, id)?;
                let tasks: Vec<_> = inner
                    .tasks
                    .iter()
                    .filter(|t| t.view.context_id == id)
                    .map(|t| t.view.clone())
                    .collect();
                success(json!({"tasks":tasks}))
            }
            "arkdock_claim_task" => {
                let id = required(&args, "contextId")?;
                owned_context(&inner, id, owner)?;
                require_confirmed(&inner, id)?;
                let task_id = optional(&args, "taskId", 128)?;
                let active_claim = task_id
                    .is_none()
                    .then(|| {
                        inner.tasks.iter().position(|t| {
                            t.view.context_id == id
                                && matches!(
                                    t.view.state,
                                    McpTaskState::Claimed | McpTaskState::Acknowledged
                                )
                                && t.claim
                                    .as_ref()
                                    .is_some_and(|(claim_owner, _)| same_owner(claim_owner, owner))
                        })
                    })
                    .flatten();
                let task = inner
                    .tasks
                    .iter_mut()
                    .enumerate()
                    .find(|(index, t)| {
                        t.view.context_id == id
                            && match &task_id {
                                Some(task_id) => t.view.task_id == *task_id,
                                None => active_claim
                                    .map_or(t.view.state == McpTaskState::Pending, |active| {
                                        active == *index
                                    }),
                            }
                    })
                    .map(|(_, task)| task)
                    .ok_or("MCP_TASK_UNAVAILABLE")?;
                match task.view.state {
                    McpTaskState::Pending => {
                        task.claim = Some((owner.clone(), random_id("claim-")?));
                        task.claimed_once = true;
                        task.view.state = McpTaskState::Claimed;
                        task.view.updated_at = now;
                    }
                    McpTaskState::Claimed
                    | McpTaskState::Acknowledged
                    | McpTaskState::Completed
                    | McpTaskState::Failed
                        if task
                            .claim
                            .as_ref()
                            .is_some_and(|(claim_owner, _)| same_owner(claim_owner, owner)) => {}
                    _ => return Err("MCP_TASK_UNAVAILABLE".into()),
                }
                success(
                    json!({"task":task.view,"claimId":task.claim.as_ref().ok_or("MCP_CLAIM_MISSING")?.1}),
                )
            }
            "arkdock_get_task_context" | "arkdock_get_task_image" | "arkdock_report_task" => {
                let id = required(&args, "contextId")?;
                owned_context(&inner, id, owner)?;
                require_confirmed(&inner, id)?;
                let task_id = required(&args, "taskId")?;
                let claim_id = required(&args, "claimId")?;
                let task = inner
                    .tasks
                    .iter_mut()
                    .find(|t| t.view.context_id == id && t.view.task_id == task_id)
                    .ok_or("MCP_TASK_UNAVAILABLE")?;
                if !task.claim.as_ref().is_some_and(|(claim_owner, claim)| {
                    same_owner(claim_owner, owner) && claim == claim_id
                }) || matches!(
                    task.view.state,
                    McpTaskState::Expired | McpTaskState::Unknown | McpTaskState::Pending
                ) {
                    return Err("MCP_CLAIM_INVALID".into());
                }
                match tool {
                    "arkdock_get_task_context" => {
                        success(json!({"task":task.view,"selection":task.selection}))
                    }
                    "arkdock_get_task_image" => {
                        let (descriptor, bytes) = match required(&args, "image")? {
                            "screen" => (&task.selection.screenshot, &task.screenshot),
                            "crop" => (&task.selection.crop, &task.crop),
                            _ => return Err("MCP_IMAGE_INVALID".into()),
                        };
                        Ok(
                            json!({"content":[{"type":"image","mimeType":mime_type(descriptor)?,"data":base64::engine::general_purpose::STANDARD.encode(bytes)}],"isError":false}),
                        )
                    }
                    _ => {
                        let state = match required(&args, "status")? {
                            "acknowledged" => McpTaskState::Acknowledged,
                            "completed" => McpTaskState::Completed,
                            "failed" => McpTaskState::Failed,
                            _ => return Err("MCP_REPORT_STATUS_INVALID".into()),
                        };
                        let summary = optional(&args, "summary", MAX_REPORT_BYTES)?;
                        let validation = optional(&args, "validation", MAX_REPORT_BYTES)?;
                        if matches!(
                            task.view.state,
                            McpTaskState::Completed | McpTaskState::Failed
                        ) {
                            if task.view.state != state
                                || task.view.summary != summary
                                || task.view.validation != validation
                            {
                                return Err("MCP_REPORT_CONFLICT".into());
                            }
                        } else if task.view.state != state
                            || task.view.summary != summary
                            || task.view.validation != validation
                        {
                            task.view.state = state;
                            task.view.summary = summary;
                            task.view.validation = validation;
                            task.view.updated_at = now;
                        }
                        success(json!({"task":task.view}))
                    }
                }
            }
            _ => Err("MCP_TOOL_UNKNOWN".into()),
        }
    }

    pub fn sync_snapshot(&self, snapshot: &AppSnapshot, now: u64) {
        let mut inner = self.lock();
        inner.snapshot = Some(snapshot.clone());
        self.refresh(&mut inner, now);
    }

    pub fn submit_task(
        &self,
        context_id: &str,
        selection: &SelectionContext,
        screenshot: Vec<u8>,
        crop: Vec<u8>,
        now: u64,
    ) -> Result<McpTaskSummary, String> {
        let mut inner = self.lock();
        self.refresh(&mut inner, now);
        require_confirmed(&inner, context_id)?;
        let snapshot = inner.snapshot.as_ref().ok_or("MCP_PROJECT_UNAVAILABLE")?;
        crate::selection::validate_selection(snapshot, selection, now)?;
        if serde_json::to_vec(selection)
            .map_err(|_| "MCP_SELECTION_INVALID")?
            .len()
            > MAX_IMAGE_BYTES
        {
            return Err("MCP_SELECTION_TOO_LARGE".into());
        }
        validate_image(&selection.screenshot, &screenshot)?;
        validate_image(&selection.crop, &crop)?;
        let project = snapshot.project.clone().ok_or("MCP_PROJECT_UNAVAILABLE")?;
        let registration_id = inner
            .contexts
            .get(context_id)
            .ok_or("MCP_CONTEXT_UNAVAILABLE")?
            .view
            .registration_id
            .clone();
        // Repeated UI delivery of the same selection to the same logical context is idempotent.
        if let Some(task) = inner.tasks.iter().find(|t| {
            t.view.context_id == context_id
                && t.selection == *selection
                && t.view.state != McpTaskState::Expired
        }) {
            return Ok(task.view.clone());
        }
        inner
            .tasks
            .retain(|t| t.view.expires_at > now && t.view.state != McpTaskState::Expired);
        if inner.tasks.len() >= MAX_TASKS {
            return Err("MCP_TASK_LIMIT".into());
        }
        let view = McpTaskSummary {
            task_id: random_id("task-")?,
            context_id: context_id.into(),
            selection_id: selection.selection_id.clone(),
            state: McpTaskState::Pending,
            created_at: now,
            updated_at: now,
            expires_at: now.checked_add(TASK_TTL_MS).ok_or("MCP_TIME_OVERFLOW")?,
            summary: None,
            validation: None,
        };
        inner.tasks.push(TaskRecord {
            view: view.clone(),
            selection: selection.clone(),
            screenshot,
            crop,
            project,
            registration_id,
            claim: None,
            claimed_once: false,
        });
        Ok(view)
    }

    pub fn retry_task(
        &self,
        task_id: &str,
        context_id: &str,
        now: u64,
    ) -> Result<McpTaskSummary, String> {
        let mut inner = self.lock();
        self.refresh(&mut inner, now);
        let task = inner
            .tasks
            .iter()
            .find(|t| t.view.task_id == task_id)
            .ok_or("MCP_TASK_UNAVAILABLE")?;
        require_confirmed(&inner, context_id)?;
        if inner.snapshot.as_ref().and_then(|s| s.project.as_ref()) != Some(&task.project) {
            return Err("MCP_TASK_PROJECT_MISMATCH".into());
        }
        let registration = inner
            .contexts
            .get(context_id)
            .ok_or("MCP_CONTEXT_UNAVAILABLE")?
            .view
            .registration_id
            .clone();
        let task = inner
            .tasks
            .iter_mut()
            .find(|t| t.view.task_id == task_id)
            .ok_or("MCP_TASK_UNAVAILABLE")?;
        if !matches!(
            task.view.state,
            McpTaskState::Unknown | McpTaskState::Failed
        ) || task.screenshot.is_empty()
            || task.crop.is_empty()
        {
            return Err("MCP_TASK_RETRY_UNAVAILABLE".into());
        }
        task.claim = None;
        task.view.context_id = context_id.into();
        task.registration_id = registration;
        task.view.state = McpTaskState::Pending;
        task.view.updated_at = now;
        task.view.summary = None;
        task.view.validation = None;
        Ok(task.view.clone())
    }

    pub fn status(&self, now: u64) -> (Vec<McpContext>, Vec<McpTaskSummary>) {
        let mut inner = self.lock();
        self.refresh(&mut inner, now);
        let mut contexts: Vec<_> = inner
            .contexts
            .values()
            .filter(|r| !r.revoked)
            .map(|r| r.view.clone())
            .collect();
        contexts.sort_by(|a, b| a.context_id.cmp(&b.context_id));
        (
            contexts,
            inner.tasks.iter().map(|t| t.view.clone()).collect(),
        )
    }

    /// A user/target lifecycle change revokes evidence, including disconnected
    /// claims. Mere transport loss uses __disconnect instead and stays retryable.
    pub fn invalidate_tasks(&self, now: u64) {
        let mut inner = self.lock();
        for task in &mut inner.tasks {
            expire(task, now);
        }
    }

    pub fn revoke_context(&self, context_id: &str, now: u64) -> Result<bool, String> {
        let mut inner = self.lock();
        self.refresh(&mut inner, now);
        self.revoke_locked(&mut inner, context_id, now)
    }

    fn revoke_locked(&self, inner: &mut Inner, id: &str, now: u64) -> Result<bool, String> {
        let record = inner
            .contexts
            .get_mut(id)
            .ok_or("MCP_CONTEXT_UNAVAILABLE")?;
        if record.revoked {
            return Ok(false);
        }
        if record.view.connected {
            let sequence = next_sequence(record.sequence)?;
            self.context.apply(
                ContextMessage::Revoke {
                    token: self.token.clone(),
                    session: id.into(),
                    sequence,
                },
                record.owner.process.clone(),
                record.owner.connection_id,
                now,
            )?;
            record.sequence = sequence;
        }
        record.revoked = true;
        record.view.confirmed = false;
        record.view.connected = false;
        invalidate_context_tasks(inner, id, now);
        Ok(true)
    }

    fn refresh(&self, inner: &mut Inner, now: u64) {
        let live = self.context.live(now);
        for record in inner.contexts.values_mut() {
            record.view.connected = !record.revoked
                && live.iter().any(|entry| {
                    entry.session == record.view.context_id
                        && entry.registration_id == record.view.registration_id
                        && entry.connection_id == record.owner.connection_id
                        && entry.client == record.owner.process
                });
            record.view.confirmed = record.view.connected
                && inner.snapshot.as_ref().is_some_and(|snapshot| {
                    snapshot.trusted
                        && snapshot
                            .context_source
                            .as_ref()
                            .and_then(|s| s.bound_registration_id.as_deref())
                            == Some(record.view.registration_id.as_str())
                        && snapshot
                            .project
                            .as_ref()
                            .is_some_and(|project| context_matches(&record.view, project))
                });
        }
        for task in &mut inner.tasks {
            if task.view.state == McpTaskState::Expired {
                continue;
            }
            if task.view.expires_at <= now || task.view.created_at > now {
                expire(task, now);
                continue;
            }
            let Some(context) = inner.contexts.get(&task.view.context_id) else {
                expire(task, now);
                continue;
            };
            if context.revoked {
                expire(task, now);
                continue;
            }
            if task.view.state == McpTaskState::Unknown {
                // Reconnection and selection of a new context never grant access. Only the
                // explicit UI retry below can attach fresh, confirmed ownership.
                if inner.snapshot.as_ref().is_some_and(|snapshot| {
                    snapshot
                        .project
                        .as_ref()
                        .is_some_and(|project| !snapshot.trusted || project != &task.project)
                }) {
                    expire(task, now);
                }
                continue;
            }
            if context.view.registration_id != task.registration_id {
                expire(task, now);
                continue;
            }
            if !context.view.connected {
                if matches!(
                    task.view.state,
                    McpTaskState::Claimed | McpTaskState::Acknowledged
                ) {
                    task.view.state = McpTaskState::Unknown;
                    task.view.updated_at = now;
                    task.claim = None;
                } else if task.view.state == McpTaskState::Pending {
                    expire(task, now);
                }
                continue;
            }
            let Some(snapshot) = inner.snapshot.as_ref() else {
                expire(task, now);
                continue;
            };
            if !snapshot.trusted
                || snapshot.project.as_ref() != Some(&task.project)
                || !context.view.confirmed
            {
                expire(task, now);
                continue;
            }
            if task.view.state == McpTaskState::Pending
                && !task.claimed_once
                && (snapshot.build.build_id.as_deref() != Some(&task.selection.build_id)
                    || snapshot.build.generation != task.selection.generation
                    || snapshot.build.stale
                    || snapshot.session_id.as_deref() != Some(&task.selection.session_id))
            {
                expire(task, now);
            }
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn next_sequence(sequence: u64) -> Result<u64, String> {
    sequence
        .checked_add(1)
        .ok_or_else(|| "MCP_SEQUENCE_EXHAUSTED".into())
}
fn same_owner(a: &ConnectionOwner, b: &ConnectionOwner) -> bool {
    a.connection_id == b.connection_id && a.process == b.process
}
fn owned_context<'a>(
    inner: &'a Inner,
    id: &str,
    owner: &ConnectionOwner,
) -> Result<&'a ContextRecord, String> {
    inner
        .contexts
        .get(id)
        .filter(|r| !r.revoked && same_owner(&r.owner, owner))
        .ok_or_else(|| "MCP_CONTEXT_UNAVAILABLE".into())
}
fn require_confirmed(inner: &Inner, id: &str) -> Result<(), String> {
    if inner
        .contexts
        .get(id)
        .is_some_and(|r| r.view.connected && r.view.confirmed && !r.revoked)
    {
        Ok(())
    } else {
        Err("MCP_CONTEXT_CONFIRMATION_REQUIRED".into())
    }
}
fn context_matches(context: &McpContext, project: &ProjectRef) -> bool {
    if context.environment != project.environment {
        return false;
    }
    if project.environment == Environment::WindowsLocal {
        let normalize = |path: &str| {
            let value = path.replace('/', "\\");
            let value = value
                .strip_prefix("\\\\?\\UNC\\")
                .map(|unc| format!("\\\\{unc}"))
                .or_else(|| value.strip_prefix("\\\\?\\").map(str::to_owned))
                .unwrap_or(value);
            value.trim_end_matches('\\').to_ascii_lowercase()
        };
        let execution = normalize(&context.execution_directory);
        let app = normalize(&project.app_root);
        execution == normalize(&project.workspace_root)
            || execution == app
            || execution.starts_with(&format!("{app}\\"))
    } else {
        let execution = context.execution_directory.trim_end_matches('/');
        let app = project.app_root.trim_end_matches('/');
        execution == project.workspace_root.trim_end_matches('/')
            || execution == app
            || execution.starts_with(&format!("{app}/"))
    }
}
fn expire(task: &mut TaskRecord, now: u64) {
    task.view.state = McpTaskState::Expired;
    task.view.updated_at = now;
    task.claim = None;
    task.screenshot.clear();
    task.crop.clear();
}
fn invalidate_context_tasks(inner: &mut Inner, id: &str, now: u64) {
    for task in &mut inner.tasks {
        if task.view.context_id == id {
            expire(task, now);
        }
    }
}
fn random_id(prefix: &str) -> Result<String, String> {
    let mut bytes = [0; 24];
    getrandom::fill(&mut bytes).map_err(|_| "MCP_RANDOM_UNAVAILABLE")?;
    Ok(format!(
        "{prefix}{}",
        bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    ))
}
fn required<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty() && v.len() <= 4096)
        .ok_or_else(|| "MCP_ARGUMENT_REQUIRED".into())
}
fn optional(args: &Value, key: &str, limit: usize) -> Result<Option<String>, String> {
    match args.get(key) {
        None => Ok(None),
        Some(Value::String(value)) if value.len() <= limit => Ok(Some(value.clone())),
        _ => Err("MCP_ARGUMENT_INVALID".into()),
    }
}
fn validate_arguments(tool: &str, args: &Value) -> Result<(), String> {
    let allowed: &[&str] = match tool {
        "arkdock_connect_current_project" | "arkdock_context_update" => &[
            "contextId",
            "originalProject",
            "executionDirectory",
            "environment",
            "displayName",
        ],
        "arkdock_context_status" | "arkdock_context_revoke" | "arkdock_list_tasks" => {
            &["contextId"]
        }
        "arkdock_claim_task" => &["contextId", "taskId"],
        "arkdock_get_task_context" => &["contextId", "taskId", "claimId"],
        "arkdock_get_task_image" => &["contextId", "taskId", "claimId", "image"],
        "arkdock_report_task" => &[
            "contextId",
            "taskId",
            "claimId",
            "status",
            "summary",
            "validation",
        ],
        "__heartbeat" | "__disconnect" => &[],
        _ => return Err("MCP_TOOL_UNKNOWN".into()),
    };
    if args
        .as_object()
        .is_some_and(|object| object.keys().all(|key| allowed.contains(&key.as_str())))
    {
        Ok(())
    } else {
        Err("MCP_ARGUMENTS_INVALID".into())
    }
}
fn success(value: Value) -> Result<Value, String> {
    Ok(
        json!({"content":[{"type":"text","text":serde_json::to_string(&value).map_err(|_|"MCP_SERIALIZATION_FAILED")?}],"structuredContent":value,"isError":false}),
    )
}
fn mime_type(image: &SelectionImage) -> Result<&'static str, String> {
    match image.format.as_str() {
        "jpeg" | "jpg" => Ok("image/jpeg"),
        "png" => Ok("image/png"),
        _ => Err("MCP_IMAGE_FORMAT_INVALID".into()),
    }
}
fn validate_image(descriptor: &SelectionImage, bytes: &[u8]) -> Result<(), String> {
    mime_type(descriptor)?;
    let digest = format!("{:x}", Sha256::digest(bytes));
    if bytes.is_empty()
        || bytes.len() > MAX_IMAGE_BYTES
        || bytes.len() != descriptor.byte_length as usize
        || descriptor.sha256 != digest
        || descriptor.width == 0
        || descriptor.height == 0
    {
        return Err("MCP_IMAGE_EVIDENCE_INVALID".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::model::{ContextSourceStatus, McpTaskState, ProcessRef};

    fn owner(connection_id: u64, name: &str) -> ConnectionOwner {
        ConnectionOwner {
            connection_id,
            process: ProcessRef {
                pid: 42,
                started_at: "1".into(),
            },
            client_name: name.into(),
        }
    }

    fn connect(hub: &McpHub, owner: &ConnectionOwner) -> McpContext {
        connect_at(hub, owner, 100)
    }

    fn connect_at(hub: &McpHub, owner: &ConnectionOwner, now: u64) -> McpContext {
        let result = hub.call(owner, "arkdock_connect_current_project", json!({
            "originalProject":"C:\\repo", "executionDirectory":"C:\\worktree", "environment":"windowsLocal"
            }), now).unwrap();
        serde_json::from_value(result["structuredContent"].clone()).unwrap()
    }

    fn confirmed(hub: &McpHub, context: &McpContext) -> AppSnapshot {
        confirmed_at(hub, context, 101)
    }

    fn confirmed_at(hub: &McpHub, context: &McpContext, now: u64) -> AppSnapshot {
        let mut snapshot = crate::selection::test_support::snapshot();
        snapshot.project.as_mut().unwrap().workspace_root = "C:\\worktree".into();
        snapshot.project.as_mut().unwrap().app_root = "C:\\worktree".into();
        snapshot.context_source = Some(ContextSourceStatus {
            paired: true,
            active_sessions: 1,
            last_event_at: 100,
            reason: None,
            registrations: vec![],
            bound_registration_id: Some(context.registration_id.clone()),
        });
        hub.sync_snapshot(&snapshot, now);
        snapshot
    }

    fn task(hub: &McpHub, context: &McpContext) -> McpTaskSummary {
        hub.submit_task(
            &context.context_id,
            &crate::selection::test_support::context(100),
            vec![0xff, 0xd8, 0xff, 0xd9],
            vec![0xff, 0xd8, 0xff, 0xd9],
            102,
        )
        .unwrap()
    }

    fn claim(
        hub: &McpHub,
        owner: &ConnectionOwner,
        context: &McpContext,
        task: &McpTaskSummary,
    ) -> Value {
        hub.call(
            owner,
            "arkdock_claim_task",
            json!({"contextId":context.context_id,"taskId":task.task_id}),
            103,
        )
        .unwrap()["structuredContent"]
            .clone()
    }

    #[test]
    fn explicit_project_detach_revokes_disconnected_claim_evidence() {
        let hub = McpHub::new(Arc::new(ContextHub::new()), [1; 32]);
        let owner = owner(1, "Generic client");
        let context = connect(&hub, &owner);
        let mut snapshot = confirmed(&hub, &context);
        let task = task(&hub, &context);
        claim(&hub, &owner, &context, &task);
        hub.call(&owner, "__disconnect", json!({}), 104).unwrap();
        snapshot.project = None;
        snapshot.trusted = false;
        snapshot
            .context_source
            .as_mut()
            .unwrap()
            .bound_registration_id = None;
        hub.sync_snapshot(&snapshot, 105);
        assert_eq!(hub.status(105).1[0].state, McpTaskState::Unknown);
        // Explicit Detach must revoke even after disconnect erased the active
        // project. Reconfirming that same project cannot restore old evidence.
        hub.invalidate_tasks(106);
        let reconnected = connect_at(&hub, &owner, 107);
        confirmed_at(&hub, &reconnected, 108);
        assert!(hub
            .retry_task(&task.task_id, &reconnected.context_id, 109)
            .is_err());
        let inner = hub.lock();
        assert_eq!(inner.tasks[0].view.state, McpTaskState::Expired);
        assert!(inner.tasks[0].screenshot.is_empty() && inner.tasks[0].crop.is_empty());
        assert!(inner.tasks[0].claim.is_none());
    }

    #[test]
    fn generic_clients_and_multiple_contexts_are_isolated_even_on_one_connection() {
        let hub = McpHub::new(Arc::new(ContextHub::new()), [7; 32]);
        let a = owner(1, "unlisted client A");
        let b = owner(2, "unlisted client B");
        let first = connect(&hub, &a);
        let second = connect(&hub, &a);
        let third = connect(&hub, &b);
        assert_ne!(first.context_id, second.context_id);
        assert_ne!(second.context_id, third.context_id);
        let result = hub
            .call(&a, "arkdock_context_status", json!({}), 101)
            .unwrap();
        assert_eq!(
            result["structuredContent"]["contexts"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert!(hub
            .call(
                &b,
                "arkdock_list_tasks",
                json!({"contextId":first.context_id}),
                101
            )
            .is_err());
        confirmed(&hub, &first);
        let task = task(&hub, &first);
        assert!(hub
            .call(
                &a,
                "arkdock_claim_task",
                json!({"contextId":second.context_id,"taskId":task.task_id}),
                103
            )
            .is_err());
        let claimed = claim(&hub, &a, &first, &task);
        assert_eq!(claimed, claim(&hub, &a, &first, &task));
        assert_eq!(
            claimed,
            hub.call(
                &a,
                "arkdock_claim_task",
                json!({"contextId":first.context_id}),
                103
            )
            .unwrap()["structuredContent"]
        );
        let ui = serde_json::to_string(&hub.status(103)).unwrap();
        assert!(!ui.contains(claimed["claimId"].as_str().unwrap()));
    }

    #[test]
    fn digest_validation_and_immutable_claim_survive_same_project_rebuild() {
        let hub = McpHub::new(Arc::new(ContextHub::new()), [7; 32]);
        let a = owner(1, "unknown brand");
        let context = connect(&hub, &a);
        let mut snapshot = confirmed(&hub, &context);
        let selection = crate::selection::test_support::context(100);
        assert!(hub
            .submit_task(
                &context.context_id,
                &selection,
                vec![0; 4],
                vec![0xff, 0xd8, 0xff, 0xd9],
                102
            )
            .is_err());
        let task = task(&hub, &context);
        let claimed = claim(&hub, &a, &context, &task);
        snapshot.build.generation += 1;
        snapshot.build.build_id = Some("new-build".into());
        snapshot.build.stale = true;
        hub.sync_snapshot(&snapshot, 104);
        let args = json!({"contextId":context.context_id,"taskId":task.task_id,"claimId":claimed["claimId"],"image":"screen"});
        let image = hub.call(&a, "arkdock_get_task_image", args, 105).unwrap();
        assert_eq!(image["content"][0]["type"], "image");
        assert_eq!(image["content"][0]["data"], "/9j/2Q==");
        snapshot.trusted = false;
        hub.sync_snapshot(&snapshot, 106);
        assert!(hub.call(&a, "arkdock_get_task_context", json!({"contextId":context.context_id,"taskId":task.task_id,"claimId":claimed["claimId"]}), 107).is_err());
    }

    #[test]
    fn unclaimed_build_change_expires_and_disconnect_never_requeues() {
        let hub = McpHub::new(Arc::new(ContextHub::new()), [7; 32]);
        let a = owner(1, "unknown brand");
        let context = connect(&hub, &a);
        let mut snapshot = confirmed(&hub, &context);
        let pending = task(&hub, &context);
        snapshot.build.generation += 1;
        hub.sync_snapshot(&snapshot, 103);
        assert_eq!(hub.status(104).1[0].state, McpTaskState::Expired);
        assert!(hub
            .call(
                &a,
                "arkdock_claim_task",
                json!({"contextId":context.context_id,"taskId":pending.task_id}),
                104
            )
            .is_err());
        snapshot.build.generation = 1;
        hub.sync_snapshot(&snapshot, 105);
        let task = task(&hub, &context);
        let claimed = claim(&hub, &a, &context, &task);
        hub.call(&a, "__disconnect", json!({}), 106).unwrap();
        assert_eq!(
            hub.status(107)
                .1
                .iter()
                .find(|item| item.task_id == task.task_id)
                .unwrap()
                .state,
            McpTaskState::Unknown
        );
        assert!(hub.call(&a, "arkdock_report_task", json!({"contextId":context.context_id,"taskId":task.task_id,"claimId":claimed["claimId"],"status":"completed"}), 108).is_err());
    }

    #[test]
    fn explicit_retry_rotates_proof_and_rejects_late_reports() {
        let hub = McpHub::new(Arc::new(ContextHub::new()), [7; 32]);
        let a = owner(1, "first client");
        let first = connect(&hub, &a);
        confirmed(&hub, &first);
        let task = task(&hub, &first);
        let old = claim(&hub, &a, &first, &task);
        hub.call(&a, "__disconnect", json!({}), 104).unwrap();
        assert!(hub
            .retry_task(&task.task_id, &first.context_id, 105)
            .is_err());
        let b = owner(2, "another generic client");
        let second = connect_at(&hub, &b, 105);
        let mut snapshot = confirmed_at(&hub, &second, 106);
        snapshot.build.generation += 1;
        snapshot.build.build_id = Some("own-edits-build".into());
        hub.sync_snapshot(&snapshot, 106);
        let retried = hub
            .retry_task(&task.task_id, &second.context_id, 107)
            .unwrap();
        assert_eq!(retried.state, McpTaskState::Pending);
        let new = hub
            .call(
                &b,
                "arkdock_claim_task",
                json!({"contextId":second.context_id,"taskId":task.task_id}),
                108,
            )
            .unwrap()["structuredContent"]
            .clone();
        assert_ne!(new["claimId"], old["claimId"]);
        let report = json!({"contextId":second.context_id,"taskId":task.task_id,"claimId":new["claimId"],"status":"completed","summary":"Updated selected component","validation":"Unit tests pass"});
        let result = hub
            .call(&b, "arkdock_report_task", report.clone(), 109)
            .unwrap();
        assert_eq!(
            result,
            hub.call(&b, "arkdock_report_task", report.clone(), 110)
                .unwrap()
        );
        let mut late = report;
        late["claimId"] = old["claimId"].clone();
        assert!(hub.call(&b, "arkdock_report_task", late, 111).is_err());
        let mut conflict = json!({"contextId":second.context_id,"taskId":task.task_id,"claimId":new["claimId"],"status":"failed"});
        assert!(hub
            .call(&b, "arkdock_report_task", conflict.clone(), 111)
            .is_err());
        conflict["summary"] = "a".repeat(MAX_REPORT_BYTES + 1).into();
        assert!(hub.call(&b, "arkdock_report_task", conflict, 112).is_err());
        assert!(hub.call(&a,"arkdock_get_task_context",json!({"contextId":first.context_id,"taskId":task.task_id,"claimId":old["claimId"]}),112).is_err());
    }

    #[test]
    fn repeated_connect_is_idempotent_and_material_update_requires_new_confirmation() {
        let hub = McpHub::new(Arc::new(ContextHub::new()), [7; 32]);
        let a = owner(1, "generic client");
        let context = connect(&hub, &a);
        confirmed(&hub, &context);
        let task = task(&hub, &context);
        let repeat = json!({"contextId":context.context_id,"originalProject":"C:\\repo","executionDirectory":"C:\\worktree","environment":"windowsLocal"});
        let same = hub
            .call(&a, "arkdock_connect_current_project", repeat.clone(), 103)
            .unwrap();
        assert_eq!(
            same["structuredContent"]["registrationId"],
            context.registration_id
        );
        assert_eq!(same["structuredContent"]["confirmed"], true);
        assert!(hub
            .call(
                &owner(2, "another connection"),
                "arkdock_connect_current_project",
                repeat.clone(),
                104
            )
            .is_err());
        let mut update = repeat;
        update["executionDirectory"] = "C:\\other".into();
        let changed = hub.call(&a, "arkdock_context_update", update, 105).unwrap();
        assert_ne!(
            changed["structuredContent"]["registrationId"],
            context.registration_id
        );
        assert_eq!(changed["structuredContent"]["confirmed"], false);
        assert_eq!(
            hub.status(106)
                .1
                .iter()
                .find(|t| t.task_id == task.task_id)
                .unwrap()
                .state,
            McpTaskState::Expired
        );
    }

    #[test]
    fn reconnect_is_only_same_process_and_requires_fresh_confirmation() {
        let hub = McpHub::new(Arc::new(ContextHub::new()), [7; 32]);
        let a = owner(1, "generic client");
        let context = connect(&hub, &a);
        let mut snapshot = confirmed(&hub, &context);
        let task = task(&hub, &context);
        claim(&hub, &a, &context, &task);
        hub.call(&a, "__disconnect", json!({}), 104).unwrap();
        let args = json!({"contextId":context.context_id,"originalProject":"C:\\repo","executionDirectory":"C:\\worktree","environment":"windowsLocal"});
        let mut foreign = owner(2, "other process");
        foreign.process.started_at = "different lifetime".into();
        assert!(hub
            .call(
                &foreign,
                "arkdock_connect_current_project",
                args.clone(),
                105
            )
            .is_err());
        let reconnected = hub
            .call(
                &owner(2, "same process"),
                "arkdock_connect_current_project",
                args,
                106,
            )
            .unwrap();
        let reconnected: McpContext =
            serde_json::from_value(reconnected["structuredContent"].clone()).unwrap();
        assert_ne!(reconnected.registration_id, context.registration_id);
        assert!(!reconnected.confirmed);
        assert!(hub
            .retry_task(&task.task_id, &reconnected.context_id, 107)
            .is_err());
        snapshot
            .context_source
            .as_mut()
            .unwrap()
            .bound_registration_id = Some(reconnected.registration_id.clone());
        hub.sync_snapshot(&snapshot, 107);
        assert!(hub
            .retry_task(&task.task_id, &reconnected.context_id, 108)
            .is_ok());
    }

    #[test]
    fn same_selection_with_changed_instruction_creates_distinct_immutable_tasks() {
        let hub = McpHub::new(Arc::new(ContextHub::new()), [7; 32]);
        let a = owner(1, "generic client");
        let context = connect(&hub, &a);
        confirmed(&hub, &context);
        let first = task(&hub, &context);
        assert_eq!(first.task_id, task(&hub, &context).task_id);
        let mut changed = crate::selection::test_support::context(100);
        changed.user_instruction = "Change selected component size".into();
        let second = hub
            .submit_task(
                &context.context_id,
                &changed,
                vec![0xff, 0xd8, 0xff, 0xd9],
                vec![0xff, 0xd8, 0xff, 0xd9],
                102,
            )
            .unwrap();
        assert_ne!(first.task_id, second.task_id);
        let proof = claim(&hub, &a, &context, &first);
        let result = hub.call(&a,"arkdock_get_task_context",json!({"contextId":context.context_id,"taskId":first.task_id,"claimId":proof["claimId"]}),104).unwrap();
        assert_eq!(
            result["structuredContent"]["selection"]["userInstruction"],
            "Change selected component color"
        );
    }

    #[test]
    fn declared_windows_paths_match_extended_roots_and_app_children_with_boundaries() {
        for (execution, root, expected) in [
            (r"\\?\C:\worktree", r"C:\worktree", true),
            (r"\\?\C:\worktree\entry", r"C:\worktree", true),
            (r"C:\worktree\entry", r"C:\worktree", true),
            (r"C:\worktree-sibling", r"C:\worktree", false),
            (r"\\?\UNC\server\repo\entry", r"\\server\repo", true),
            (r"\\server\repo-sibling", r"\\server\repo", false),
        ] {
            let hub = McpHub::new(Arc::new(ContextHub::new()), [7; 32]);
            let a = owner(1, "generic client");
            let result = hub.call(&a,"arkdock_connect_current_project",json!({"originalProject":"C:\\repo","executionDirectory":execution,"environment":"windowsLocal"}),100).unwrap();
            let context: McpContext =
                serde_json::from_value(result["structuredContent"].clone()).unwrap();
            let mut snapshot = confirmed(&hub, &context);
            snapshot.project.as_mut().unwrap().workspace_root = root.into();
            snapshot.project.as_mut().unwrap().app_root = root.into();
            hub.sync_snapshot(&snapshot, 102);
            assert_eq!(hub.status(103).0[0].confirmed, expected, "{execution}");
        }
    }

    #[test]
    fn undeclared_arguments_fail_closed_for_every_domain_tool() {
        let hub = McpHub::new(Arc::new(ContextHub::new()), [7; 32]);
        let a = owner(1, "generic client");
        for tool in [
            "arkdock_connect_current_project",
            "arkdock_context_update",
            "arkdock_context_status",
            "arkdock_context_revoke",
            "arkdock_list_tasks",
            "arkdock_claim_task",
            "arkdock_get_task_context",
            "arkdock_get_task_image",
            "arkdock_report_task",
            "__heartbeat",
            "__disconnect",
        ] {
            assert_eq!(
                hub.call(&a, tool, json!({"trusted":true}), 100),
                Err("MCP_ARGUMENTS_INVALID".into()),
                "{tool}"
            );
        }
    }

    #[test]
    fn revoked_and_empty_disconnected_contexts_do_not_exhaust_capacity() {
        let hub = McpHub::new(Arc::new(ContextHub::new()), [7; 32]);
        let a = owner(1, "generic client");
        for index in 0..MAX_SESSIONS + 8 {
            let now = 100 + index as u64 * 2;
            let context = connect_at(&hub, &a, now);
            assert!(hub.revoke_context(&context.context_id, now + 1).unwrap());
        }
        let disconnected = connect_at(&hub, &a, 200);
        hub.call(&a, "__disconnect", json!({}), 201).unwrap();
        let replacement = connect_at(&hub, &owner(2, "new connection"), 202);
        assert_ne!(disconnected.context_id, replacement.context_id);
        assert_eq!(hub.status(203).0.len(), 1);
        assert_eq!(hub.lock().contexts.len(), 1);
    }

    #[test]
    fn active_contexts_and_disconnected_unknown_tasks_keep_the_capacity_limit() {
        let hub = McpHub::new(Arc::new(ContextHub::new()), [7; 32]);
        let a = owner(1, "generic client");
        let context = connect(&hub, &a);
        confirmed(&hub, &context);
        let task = task(&hub, &context);
        let old = claim(&hub, &a, &context, &task);
        hub.call(&a, "__disconnect", json!({}), 104).unwrap();
        let b = owner(2, "replacement client");
        for _ in 0..MAX_SESSIONS - 1 {
            connect_at(&hub, &b, 105);
        }
        let args = json!({"originalProject":"C:\\repo","executionDirectory":"C:\\worktree","environment":"windowsLocal"});
        assert_eq!(
            hub.call(&b, "arkdock_connect_current_project", args.clone(), 106),
            Err("MCP_CONTEXT_LIMIT".into())
        );
        assert_eq!(hub.status(106).1[0].state, McpTaskState::Unknown);
        assert!(!hub.lock().tasks[0].screenshot.is_empty());
        assert!(hub.call(&a,"arkdock_get_task_context",json!({"contextId":context.context_id,"taskId":task.task_id,"claimId":old["claimId"]}),106).is_err());
        let active = McpHub::new(Arc::new(ContextHub::new()), [7; 32]);
        for _ in 0..MAX_SESSIONS {
            connect(&active, &a);
        }
        assert_eq!(
            active.call(&a, "arkdock_connect_current_project", args, 101),
            Err("MCP_CONTEXT_LIMIT".into())
        );
    }

    #[test]
    fn build_expired_tasks_release_queue_capacity_before_their_original_ttl() {
        let hub = McpHub::new(Arc::new(ContextHub::new()), [7; 32]);
        let a = owner(1, "generic client");
        let context = connect(&hub, &a);
        let mut snapshot = confirmed(&hub, &context);
        for index in 0..MAX_TASKS {
            let mut selection = crate::selection::test_support::context(100);
            selection.user_instruction = format!("Old build task {index}");
            hub.submit_task(
                &context.context_id,
                &selection,
                vec![0xff, 0xd8, 0xff, 0xd9],
                vec![0xff, 0xd8, 0xff, 0xd9],
                102,
            )
            .unwrap();
        }
        snapshot.build.generation = 2;
        snapshot.build.displayed_generation = Some(2);
        snapshot.build.build_id = Some("new-build".into());
        hub.sync_snapshot(&snapshot, 103);
        assert!(hub
            .status(104)
            .1
            .iter()
            .all(|task| task.state == McpTaskState::Expired && task.expires_at > 104));
        let mut current = crate::selection::test_support::context(104);
        current.generation = 2;
        current.build_id = "new-build".into();
        let new = hub
            .submit_task(
                &context.context_id,
                &current,
                vec![0xff, 0xd8, 0xff, 0xd9],
                vec![0xff, 0xd8, 0xff, 0xd9],
                105,
            )
            .unwrap();
        assert_eq!(new.state, McpTaskState::Pending);
        assert_eq!(hub.status(106).1.len(), 1);
        assert_eq!(hub.status(106).1[0].task_id, new.task_id);
    }

    #[test]
    fn task_queue_is_bounded_and_ttl_destroys_image_access() {
        let hub = McpHub::new(Arc::new(ContextHub::new()), [7; 32]);
        let a = owner(1, "generic client");
        let context = connect(&hub, &a);
        confirmed(&hub, &context);
        for index in 0..MAX_TASKS {
            let mut selection = crate::selection::test_support::context(100);
            selection.user_instruction = format!("Task {index}");
            hub.submit_task(
                &context.context_id,
                &selection,
                vec![0xff, 0xd8, 0xff, 0xd9],
                vec![0xff, 0xd8, 0xff, 0xd9],
                102,
            )
            .unwrap();
        }
        let selection = crate::selection::test_support::context(100);
        assert_eq!(
            hub.submit_task(
                &context.context_id,
                &selection,
                vec![0xff, 0xd8, 0xff, 0xd9],
                vec![0xff, 0xd8, 0xff, 0xd9],
                102
            ),
            Err("MCP_TASK_LIMIT".into())
        );
        hub.status(102 + TASK_TTL_MS);
        let inner = hub.lock();
        assert!(inner
            .tasks
            .iter()
            .all(|task| task.view.state == McpTaskState::Expired
                && task.screenshot.is_empty()
                && task.crop.is_empty()
                && task.claim.is_none()));
    }
}
