//! Opt-in, bearer-authorized loopback transport. No shell, project reads or host discovery.
mod http;
use crate::ipc::model::*;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::thread::JoinHandle;

const LEASE_MS: u64 = 10_000;
const RECEIPT_TIMEOUT_MS: u64 = 30_000;
const MAX_DELIVERIES: usize = 64;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SelectionGrant {
    pub grant_id: String,
    pub directory: String,
    pub capability_file: String,
    pub workspace_id: String,
    pub session_id: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImageAttachment {
    pub path: String,
    pub sha256: String,
    pub byte_length: u32,
    pub mime_type: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Attachments {
    pub grant: Option<SelectionGrant>,
    pub images: Vec<ImageAttachment>,
}
#[derive(Clone)]
pub struct TransportUpdate {
    pub state: DeliveryState,
    pub receipt_id: Option<String>,
    pub reason: String,
    pub transport_receipt: Option<String>,
    pub agent_acknowledgment: Option<String>,
}
struct QueuedDelivery {
    payload: Value,
    selection: SelectionContext,
    claimed: bool,
    created_at: u64,
    update: TransportUpdate,
}
struct PairState {
    pairing: AgentPairing,
    target: TargetInfo,
    preview_session: String,
    registration: Option<AgentRegistration>,
    deliveries: BTreeMap<String, QueuedDelivery>,
}
#[derive(Default)]
struct Shared {
    pair: Option<PairState>,
    receipts: BTreeMap<String, (TransportUpdate, u64)>,
    snapshot: Option<AppSnapshot>,
    observed_at: u64,
}
pub struct SessionBridge {
    shared: Arc<Mutex<Shared>>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    endpoint: String,
}

impl SessionBridge {
    pub fn start() -> Result<Self, String> {
        http::start()
    }

    pub fn pair(
        &self,
        snapshot: &AppSnapshot,
        kind: AgentKind,
        now: u64,
    ) -> Result<AgentPairing, String> {
        let project = snapshot
            .project
            .as_ref()
            .ok_or("AGENT_WORKSPACE_REQUIRED")?;
        if !snapshot.trusted || project.environment != Environment::WindowsLocal {
            return Err("AGENT_LOCAL_TRUSTED_WORKSPACE_REQUIRED".into());
        }
        let target = snapshot.target.clone().ok_or("AGENT_TARGET_REQUIRED")?;
        let session = snapshot
            .session_id
            .clone()
            .ok_or("AGENT_PREVIEW_SESSION_REQUIRED")?;
        let pairing = AgentPairing {
            pairing_id: random_id()?,
            endpoint: self.endpoint.clone(),
            token: random_id()?,
            config_path: None,
            agent_kind: kind,
            workspace_id: project.id.clone(),
            workspace_root: project.workspace_root.clone(),
            expires_at: now + 600_000,
        };
        let mut shared = self.shared.lock().map_err(|_| "AGENT_BRIDGE_UNAVAILABLE")?;
        archive_pair(&mut shared, now);
        shared.pair = Some(PairState {
            pairing: pairing.clone(),
            target,
            preview_session: session,
            registration: None,
            deliveries: BTreeMap::new(),
        });
        shared.snapshot = Some(snapshot.clone());
        shared.observed_at = now;
        Ok(pairing)
    }

    pub fn revoke(&self) {
        if let Ok(mut shared) = self.shared.lock() {
            archive_pair(&mut shared, crate::selection::now_ms());
        }
    }

    /// Actor heartbeat also revokes queued delivery when preview/trust/build scope changes.
    pub fn sync(&self, snapshot: &AppSnapshot, now: u64) {
        let Ok(mut shared) = self.shared.lock() else {
            return;
        };
        let valid = shared
            .pair
            .as_ref()
            .is_none_or(|pair| pair_scope(pair, snapshot));
        if !valid {
            archive_pair(&mut shared, now);
        }
        if let Some(pair) = &mut shared.pair {
            for delivery in pair.deliveries.values_mut() {
                if !delivery.claimed
                    && crate::selection::validate_selection(snapshot, &delivery.selection, now)
                        .is_err()
                {
                    delivery.claimed = true;
                    delivery.update = TransportUpdate {
                        state: DeliveryState::Expired,
                        receipt_id: None,
                        reason: "SELECTION_EXPIRED_BEFORE_DISPATCH".into(),
                        transport_receipt: None,
                        agent_acknowledgment: None,
                    };
                }
            }
        }
        shared.snapshot = Some(snapshot.clone());
        shared.observed_at = now;
    }

    pub fn registrations(&self, now: u64) -> Vec<AgentRegistration> {
        self.shared
            .lock()
            .ok()
            .and_then(|shared| {
                shared
                    .pair
                    .as_ref()
                    .and_then(|pair| pair.registration.clone())
            })
            .filter(|registration| registration.expires_at > now)
            .into_iter()
            .collect()
    }

    pub fn verify(
        &self,
        binding: &AgentSessionBinding,
        now: u64,
    ) -> Result<AgentRegistration, String> {
        self.registrations(now)
            .into_iter()
            .find(|r| {
                binding.transport_instance_id.as_deref() == Some(r.transport_instance_id.as_str())
                    && r.agent_session_id == binding.agent_session_id
                    && r.conversation_or_thread_id == binding.conversation_or_thread_id
                    && r.workspace_id == binding.workspace_id
                    && r.transport_kind == binding.transport_kind
            })
            .ok_or("AGENT_TRANSPORT_LEASE_EXPIRED".into())
    }

    pub fn submit(
        &self,
        binding: &AgentSessionBinding,
        delivery: &AgentDelivery,
        selection: &SelectionContext,
        attachments: Attachments,
        now: u64,
    ) -> Result<(), String> {
        let registration = self.verify(binding, now)?;
        if registration.busy {
            return Err("AGENT_BUSY_RETRY_AFTER_CURRENT_TURN".into());
        }
        let mut shared = self.shared.lock().map_err(|_| "AGENT_BRIDGE_UNAVAILABLE")?;
        let snapshot = shared
            .snapshot
            .as_ref()
            .ok_or("AGENT_RUNTIME_UNAVAILABLE")?;
        crate::selection::validate_selection(snapshot, selection, now)?;
        if now.saturating_sub(shared.observed_at) > 2000 {
            return Err("AGENT_RUNTIME_LEASE_EXPIRED".into());
        }
        let pair = shared.pair.as_mut().ok_or("AGENT_PAIR_REVOKED")?;
        if !pair.registration.as_ref().is_some_and(|r| {
            r.transport_instance_id == registration.transport_instance_id
                && r.agent_session_id == binding.agent_session_id
                && r.conversation_or_thread_id == binding.conversation_or_thread_id
                && r.expires_at > now
        }) {
            return Err("AGENT_SESSION_CHANGED_REPAIR_REQUIRED".into());
        }
        if pair.deliveries.contains_key(&delivery.request_id) {
            return Ok(());
        }
        if pair.deliveries.len() >= MAX_DELIVERIES {
            return Err("AGENT_TRANSPORT_DELIVERY_LIMIT_REPAIR_REQUIRED".into());
        }
        let payload = json!({"deliveryId":delivery.request_id,"selectionId":selection.selection_id,"prompt":delivery.prompt,
            "workspaceId":selection.workspace_id,"previewSessionId":selection.session_id,"expiresAt":selection.expires_at,
            "sessionId":binding.agent_session_id,"conversationId":binding.conversation_or_thread_id,
            "grant":attachments.grant,"images":attachments.images});
        if payload.to_string().len() > 120 * 1024 {
            return Err("AGENT_MESSAGE_TOO_LARGE".into());
        }
        pair.deliveries.insert(
            delivery.request_id.clone(),
            QueuedDelivery {
                payload,
                selection: selection.clone(),
                claimed: false,
                created_at: now,
                update: TransportUpdate {
                    state: DeliveryState::Sending,
                    receipt_id: None,
                    reason: "等待已配对通道重新核验并提交。".into(),
                    transport_receipt: None,
                    agent_acknowledgment: None,
                },
            },
        );
        Ok(())
    }

    pub fn delivery_status(&self, id: &str, now: u64) -> Option<TransportUpdate> {
        let mut shared = self.shared.lock().ok()?;
        shared.receipts.retain(|_, (_, expires)| *expires > now);
        if !shared
            .pair
            .as_ref()
            .is_some_and(|pair| pair.deliveries.contains_key(id))
        {
            return shared.receipts.get(id).map(|(receipt, _)| receipt.clone());
        }
        let delivery = shared.pair.as_mut()?.deliveries.get_mut(id)?;
        if delivery.update.state == DeliveryState::Sending
            && now.saturating_sub(delivery.created_at) > RECEIPT_TIMEOUT_MS
        {
            delivery.claimed = true;
            delivery.update = TransportUpdate {
                state: DeliveryState::Unknown,
                receipt_id: None,
                reason: "提交回执超时；结果未知，不会自动重发。".into(),
                transport_receipt: None,
                agent_acknowledgment: None,
            };
        }
        Some(delivery.update.clone())
    }
}

/// Connection authorization may disappear before the UI polls. Keep only its
/// already accepted receipt metadata; never retain a token, prompt or image grant.
fn archive_pair(shared: &mut Shared, now: u64) {
    shared.receipts.retain(|_, (_, expires)| *expires > now);
    if let Some(pair) = shared.pair.take() {
        for (id, delivery) in pair.deliveries {
            let mut update = delivery.update;
            if update.state == DeliveryState::Sending {
                update.state = if delivery.claimed {
                    DeliveryState::Unknown
                } else {
                    DeliveryState::Failed
                };
                update.reason = if delivery.claimed {
                    "通道已断开，提交结果未知；不会重发。"
                } else {
                    "通道已撤销，请求尚未交付。"
                }
                .into();
            }
            shared
                .receipts
                .insert(id, (update, now.saturating_add(60 * 60 * 1000)));
        }
    }
    while shared.receipts.len() > MAX_DELIVERIES * 2 {
        let oldest = shared
            .receipts
            .iter()
            .min_by_key(|(_, (_, expires))| *expires)
            .map(|(id, _)| id.clone());
        if let Some(id) = oldest {
            shared.receipts.remove(&id);
        } else {
            break;
        }
    }
}
impl Drop for SessionBridge {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn pair_scope(pair: &PairState, snapshot: &AppSnapshot) -> bool {
    snapshot.trusted
        && snapshot.target.as_ref().is_some_and(|target| {
            target.id == pair.target.id && target.process == pair.target.process
        })
        && snapshot.project.as_ref().is_some_and(|p| {
            p.id == pair.pairing.workspace_id && p.workspace_root == pair.pairing.workspace_root
        })
        && snapshot.session_id.as_ref() == Some(&pair.preview_session)
}
fn valid_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}
fn random_id() -> Result<String, String> {
    let mut bytes = [0u8; 32];
    #[cfg(windows)]
    getrandom::fill(&mut bytes).map_err(|_| "AGENT_RANDOM_UNAVAILABLE")?;
    #[cfg(not(windows))]
    {
        use std::io::Read;
        std::fs::File::open("/dev/urandom")
            .and_then(|mut file| file.read_exact(&mut bytes))
            .map_err(|_| "AGENT_RANDOM_UNAVAILABLE")?;
    }
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RegistrationRequest {
    transport_kind: String,
    instance_id: String,
    agent_kind: AgentKind,
    agent_version: String,
    session_id: String,
    conversation_id: String,
    workspace_root: String,
    pid: u32,
    #[serde(default)]
    workspace_id: Option<String>,
    capabilities: BTreeMap<String, bool>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PollRequest {
    instance_id: String,
    session_id: String,
    conversation_id: String,
    workspace_root: String,
    #[serde(default)]
    busy: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReceiptRequest {
    instance_id: String,
    delivery_id: String,
    session_id: String,
    conversation_id: String,
    state: DeliveryState,
    receipt_id: String,
    reason: String,
}
fn same_path(a: &str, b: &str) -> bool {
    // Resolve explicit roots only. Environment is already pinned to windowsLocal.
    let normalized = |value: &str| {
        value
            .strip_prefix("\\\\?\\")
            .unwrap_or(value)
            .replace('\\', "/")
            .trim_end_matches('/')
            .to_lowercase()
    };
    let a_normal = normalized(a);
    let b_normal = normalized(b);
    if a_normal != b_normal || a_normal.starts_with("//") {
        return false;
    }
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}
fn handle(
    shared: &mut Shared,
    token: &str,
    route: &str,
    value: Value,
    now: u64,
) -> Result<Value, String> {
    use sha2::{Digest, Sha256};
    let snapshot = shared
        .snapshot
        .as_ref()
        .ok_or("AGENT_RUNTIME_UNAVAILABLE")?;
    let pair = shared.pair.as_mut().ok_or("AGENT_PAIR_REQUIRED")?;
    let actual = Sha256::digest(token.as_bytes());
    let expected = Sha256::digest(pair.pairing.token.as_bytes());
    if actual
        .iter()
        .zip(expected.iter())
        .fold(0u8, |n, (a, b)| n | (a ^ b))
        != 0
    {
        return Err("AGENT_UNAUTHORIZED".into());
    }
    if route == "/v1/revoke" {
        if let Some(session) = value.get("sessionId").and_then(Value::as_str) {
            if pair
                .registration
                .as_ref()
                .is_some_and(|r| r.agent_session_id != session)
            {
                return Err("AGENT_SESSION_MISMATCH".into());
            }
        }
        archive_pair(shared, now);
        return Ok(json!({"accepted":true}));
    }
    if !pair_scope(pair, snapshot) || now.saturating_sub(shared.observed_at) > 2000 {
        return Err("AGENT_RUNTIME_LEASE_EXPIRED".into());
    }
    match route {
        "/v1/register" => {
            let request: RegistrationRequest =
                serde_json::from_value(value).map_err(|_| "AGENT_REGISTRATION_INVALID")?;
            if pair.registration.is_some() {
                return Err("AGENT_PAIR_ALREADY_USED".into());
            }
            if pair.pairing.expires_at <= now
                || request.agent_kind != pair.pairing.agent_kind
                || request
                    .workspace_id
                    .as_ref()
                    .is_some_and(|id| id != &pair.pairing.workspace_id)
            {
                return Err("AGENT_PAIR_EXPIRED_OR_WRONG_KIND".into());
            }
            if ![
                &request.instance_id,
                &request.session_id,
                &request.conversation_id,
                &request.agent_version,
            ]
            .iter()
            .all(|v| valid_id(v))
                || request.pid == 0
                || request.capabilities.len() > 16
                || !same_path(&request.workspace_root, &pair.pairing.workspace_root)
            {
                return Err("AGENT_REGISTRATION_SCOPE_INVALID".into());
            }
            let transport = match (&request.agent_kind, request.transport_kind.as_str()) {
                (AgentKind::Claude, "claude-channel") => AgentTransportKind::ClaudeChannel,
                (AgentKind::Codex, "codex-app-server") => AgentTransportKind::CodexAppServer,
                (AgentKind::Codex, "codex-desktop-plugin") => {
                    AgentTransportKind::CodexDesktopPlugin
                }
                _ => return Err("AGENT_TRANSPORT_UNSUPPORTED".into()),
            };
            let registration=AgentRegistration {registration_id:random_id()?,agent_kind:request.agent_kind,agent_version:request.agent_version,
                transport_kind:transport,transport_instance_id:request.instance_id,agent_session_id:request.session_id,conversation_or_thread_id:request.conversation_id,
                workspace_id:pair.pairing.workspace_id.clone(),workspace_root:pair.pairing.workspace_root.clone(),pid:request.pid,
                capabilities:AgentCapabilities {identify_current_session:true,direct_send:true,images:request.capabilities.get("images")==Some(&true),
                    file_references:request.capabilities.get("fileReferences")==Some(&true),busy_state:request.capabilities.get("busyState")==Some(&true),receipt:true,cancel_pending:false},
                observed_at:now,expires_at:now+LEASE_MS,busy:false,reason:"显式配对的本地通道；用户选择后绑定。Claude 为 Research Preview；Codex 仅当前受管理线程。".into()};
            let result = json!({"registrationId":registration.registration_id,"expiresAt":registration.expires_at});
            pair.registration = Some(registration);
            Ok(result)
        }
        "/v1/poll" => {
            let request: PollRequest =
                serde_json::from_value(value).map_err(|_| "AGENT_POLL_INVALID")?;
            let registration = pair
                .registration
                .as_mut()
                .ok_or("AGENT_REGISTRATION_REQUIRED")?;
            if registration.expires_at <= now
                || registration.transport_instance_id != request.instance_id
                || registration.agent_session_id != request.session_id
                || registration.conversation_or_thread_id != request.conversation_id
                || !same_path(&registration.workspace_root, &request.workspace_root)
            {
                return Err("AGENT_SESSION_CHANGED_REPAIR_REQUIRED".into());
            }
            registration.observed_at = now;
            registration.expires_at = now + LEASE_MS;
            registration.busy = request.busy;
            let mut deliveries = Vec::new();
            if !request.busy {
                // One outstanding hand-off at a time. Never replay after dequeue.
                if let Some(delivery) = pair.deliveries.values_mut().find(|d| !d.claimed) {
                    delivery.claimed = true;
                    if crate::selection::validate_selection(snapshot, &delivery.selection, now)
                        .is_ok()
                        && now.saturating_sub(delivery.created_at) < RECEIPT_TIMEOUT_MS
                    {
                        deliveries.push(delivery.payload.clone());
                    } else {
                        delivery.update = TransportUpdate {
                            state: DeliveryState::Expired,
                            receipt_id: None,
                            reason: "SELECTION_EXPIRED_BEFORE_DISPATCH".into(),
                            transport_receipt: None,
                            agent_acknowledgment: None,
                        };
                    }
                }
            }
            Ok(json!({"deliveries":deliveries}))
        }
        "/v1/receipt" => {
            let request: ReceiptRequest =
                serde_json::from_value(value).map_err(|_| "AGENT_RECEIPT_INVALID")?;
            let registration = pair
                .registration
                .as_ref()
                .ok_or("AGENT_REGISTRATION_REQUIRED")?;
            if registration.expires_at <= now
                || registration.transport_instance_id != request.instance_id
                || registration.agent_session_id != request.session_id
                || registration.conversation_or_thread_id != request.conversation_id
                || !valid_id(&request.receipt_id)
                || request.reason.len() > 2048
            {
                return Err("AGENT_RECEIPT_SCOPE_MISMATCH".into());
            }
            let delivery = pair
                .deliveries
                .get_mut(&request.delivery_id)
                .ok_or("AGENT_DELIVERY_NOT_FOUND")?;
            if !delivery.claimed
                || !allowed_transition(
                    &delivery.update.state,
                    &request.state,
                    registration.transport_kind.clone(),
                )
            {
                return Err("AGENT_RECEIPT_TRANSITION_INVALID".into());
            }
            if delivery.update.state == request.state
                && delivery
                    .update
                    .receipt_id
                    .as_ref()
                    .is_some_and(|id| id != &request.receipt_id)
            {
                return Err("AGENT_DUPLICATE_RECEIPT_MISMATCH".into());
            }
            let transport_receipt = if request.state == DeliveryState::TransportAccepted {
                Some(request.receipt_id.clone())
            } else {
                delivery.update.transport_receipt.clone()
            };
            let agent_acknowledgment = if request.state == DeliveryState::AgentAcknowledged {
                Some(request.receipt_id.clone())
            } else {
                delivery.update.agent_acknowledgment.clone()
            };
            delivery.update = TransportUpdate {
                state: request.state,
                receipt_id: Some(request.receipt_id),
                reason: request.reason,
                transport_receipt,
                agent_acknowledgment,
            };
            Ok(json!({"accepted":true}))
        }
        _ => Err("AGENT_ROUTE_NOT_FOUND".into()),
    }
}

fn allowed_transition(from: &DeliveryState, to: &DeliveryState, kind: AgentTransportKind) -> bool {
    use DeliveryState::*;
    if from == to {
        return true;
    }
    match (from, to) {
        (Sending, TransportAccepted | Failed | Unknown) => true,
        (Sending | TransportAccepted | Unknown, Submitted) => {
            kind == AgentTransportKind::CodexAppServer
        }
        (TransportAccepted | Submitted | Unknown, AgentAcknowledged) => true,
        (AgentAcknowledged | Submitted, Completed) => true,
        (TransportAccepted | Submitted | AgentAcknowledged | Unknown, Failed) => true,
        (TransportAccepted | Submitted | AgentAcknowledged, Unknown) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests;
