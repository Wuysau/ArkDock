use super::adapters;
use super::bridge::{Attachments, PreparedPairing, SessionBridge};
use super::AgentTransport;
use crate::ipc::model::*;
use crate::selection::{format_compact_prompt, format_prompt, validate_selection};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

const BINDING_TTL_MS: u64 = 60_000;
const MAX_BINDINGS: usize = 16;
const MAX_DELIVERIES: usize = 32;

struct DeliveryRecord {
    delivery: AgentDelivery,
    selection: SelectionContext,
}

#[derive(Default)]
pub struct AgentRouter {
    bindings: BTreeMap<String, AgentSessionBinding>,
    deliveries: BTreeMap<String, DeliveryRecord>,
    serial: u64,
    bridge: Option<SessionBridge>,
    /// Credential staged by [`Self::prepare_pair`] and awaiting its durable
    /// write. Not a second pairing state: the bridge's live pairing remains the
    /// only authority, and a staged token authorizes nothing until activation.
    pending_pair: Option<PreparedPairing>,
    #[cfg(test)]
    verified_adapter: Option<Box<dyn adapters::VerifiedAdapter>>,
}

impl AgentRouter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn list(&mut self, snapshot: &AppSnapshot, now: u64) -> AgentSessionList {
        self.sync(snapshot, now);
        self.cleanup(now);
        let registrations = self
            .bridge
            .as_ref()
            .map(|bridge| bridge.discover_sessions(now))
            .unwrap_or_default();
        let bindings: Vec<AgentSessionBinding> = self
            .bindings
            .values()
            .filter(|binding| validate_binding(snapshot, binding, now).is_ok())
            .cloned()
            .map(|mut binding| {
                if binding.transport_instance_id.is_some() {
                    if let Some(registration) = registrations.iter().find(|r| {
                        Some(&r.transport_instance_id) == binding.transport_instance_id.as_ref()
                    }) {
                        binding.verified_at = registration.observed_at;
                    } else {
                        binding.writable = false;
                        binding.reason = "连接已失效，需要重新配对。".into();
                    }
                }
                binding
            })
            .collect();
        AgentSessionList {
            adapters: [AgentKind::Codex, AgentKind::Claude]
                .into_iter()
                .map(|kind| {
                    registrations
                        .iter()
                        .find(|r| r.agent_kind == kind)
                        .map(|r| AgentAdapterStatus {
                            agent_kind: kind.clone(),
                            capabilities: r.capabilities.clone(),
                            status: if bindings.iter().any(|b| {
                                b.writable
                                    && b.transport_instance_id.as_ref()
                                        == Some(&r.transport_instance_id)
                            }) {
                                DeliveryState::Ready
                            } else {
                                DeliveryState::SessionUnverified
                            },
                            reason: r.reason.clone(),
                        })
                        .unwrap_or_else(|| adapters::status(kind))
                })
                .collect(),
            bindings,
            registrations,
        }
    }

    pub fn bind(
        &mut self,
        snapshot: &AppSnapshot,
        draft: &AgentBindingDraft,
        now: u64,
    ) -> Result<AgentSessionBinding, String> {
        self.cleanup(now);
        if !valid_id(&draft.agent_session_id) || !valid_id(&draft.conversation_or_thread_id) {
            return Err("AGENT_SESSION_ID_INVALID".into());
        }
        let target = snapshot.target.as_ref().ok_or("AGENT_TARGET_REQUIRED")?;
        let workspace = snapshot
            .project
            .as_ref()
            .ok_or("AGENT_WORKSPACE_REQUIRED")?;
        // Explicitly changing a conversation revokes older bindings for this
        // Agent/window; the old request can never follow the new conversation.
        self.bindings.retain(|_, binding| {
            binding.agent_kind != draft.agent_kind || binding.target_id != target.id
        });
        if self.bindings.len() >= MAX_BINDINGS {
            return Err("AGENT_BINDING_LIMIT".into());
        }
        self.serial = self.serial.checked_add(1).ok_or("AGENT_REQUEST_LIMIT")?;
        let binding = AgentSessionBinding {
            binding_id: format!("binding-{now:x}-{:x}", self.serial),
            agent_kind: draft.agent_kind.clone(),
            target_id: target.id.clone(),
            process: target.process.clone(),
            workspace_id: workspace.id.clone(),
            agent_session_id: draft.agent_session_id.clone(),
            conversation_or_thread_id: draft.conversation_or_thread_id.clone(),
            transport_kind: AgentTransportKind::Unsupported,
            transport_instance_id: None,
            agent_version: None,
            verified_at: now,
            binding_evidence: "手动声明，不构成宿主授权".into(),
            writable: false,
            capabilities: adapters::no_capabilities(),
            observed_at: now,
            expires_at: now
                .checked_add(BINDING_TTL_MS)
                .ok_or("AGENT_TIMESTAMP_INVALID")?,
            reason: "用户显式声明的会话 ID；没有宿主当前活动线程和写入权限证明，不能直接发送。"
                .into(),
        };
        self.bindings
            .insert(binding.binding_id.clone(), binding.clone());
        Ok(binding)
    }

    pub fn prepare(
        &mut self,
        snapshot: &AppSnapshot,
        selection: &SelectionContext,
        binding_id: Option<&str>,
        now: u64,
    ) -> Result<AgentDelivery, String> {
        self.cleanup(now);
        validate_selection(snapshot, selection, now)?;
        if selection.user_instruction.trim().is_empty() {
            return Err("AGENT_INSTRUCTION_REQUIRED".into());
        }
        let binding = if let Some(id) = binding_id {
            let binding = self.bindings.get(id).ok_or("AGENT_BINDING_NOT_FOUND")?;
            validate_binding(snapshot, binding, now)?;
            Some(binding.clone())
        } else {
            None
        };
        // The deterministic request includes the immutable selection, instruction
        // and precise target thread. Repeated clicks get the same status/receipt.
        let identity = serde_json::to_vec(&(
            &selection.selection_id,
            &selection.workspace_id,
            format!(
                "{:x}",
                Sha256::digest(selection.user_instruction.as_bytes())
            ),
            binding.as_ref().map(|b| {
                (
                    &b.agent_kind,
                    &b.agent_session_id,
                    &b.conversation_or_thread_id,
                )
            }),
            DeliveryMode::Native,
        ))
        .map_err(|_| "AGENT_CONTEXT_INVALID")?;
        let request_id = format!("delivery-{:x}", Sha256::digest(identity));
        if let Some(existing) = self.deliveries.get(&request_id) {
            return Ok(existing.delivery.clone());
        }
        if self.deliveries.len() >= MAX_DELIVERIES {
            return Err("AGENT_DELIVERY_LIMIT_WAIT_FOR_SELECTION_EXPIRY".into());
        }
        let (state, reason) = availability(binding.as_ref());
        let delivery = AgentDelivery {
            request_id: request_id.clone(),
            selection_id: selection.selection_id.clone(),
            binding_id: binding.as_ref().map(|binding| binding.binding_id.clone()),
            state,
            prompt: format_prompt(selection),
            target: binding,
            receipt_id: None,
            reason,
            created_at: now,
            updated_at: now,
            idempotency_key: request_id.clone(),
            delivery_mode: DeliveryMode::Native,
            transport_receipt: None,
            agent_acknowledgment: None,
        };
        self.deliveries.insert(
            request_id,
            DeliveryRecord {
                delivery: delivery.clone(),
                selection: selection.clone(),
            },
        );
        Ok(delivery)
    }

    pub fn status(
        &mut self,
        snapshot: &AppSnapshot,
        request_id: &str,
        now: u64,
    ) -> Result<AgentDelivery, String> {
        self.sync(snapshot, now);
        let record = self
            .deliveries
            .get_mut(request_id)
            .ok_or("AGENT_DELIVERY_NOT_FOUND")?;
        // No metadata or prompt from a previous workspace is returned.
        validate_delivery_scope(snapshot, &record.selection)?;
        if let Some(update) = self
            .bridge
            .as_ref()
            .and_then(|bridge| bridge.get_delivery_status(request_id, now))
        {
            record.delivery.state = update.state;
            record.delivery.reason = update.reason;
            record.delivery.receipt_id = update.receipt_id.clone();
            record.delivery.transport_receipt = update.transport_receipt;
            record.delivery.agent_acknowledgment = update.agent_acknowledgment;
            record.delivery.updated_at = now;
        }
        if matches!(
            record.delivery.state,
            DeliveryState::Ready
                | DeliveryState::Unsupported
                | DeliveryState::SessionUnverified
                | DeliveryState::Expired
        ) && validate_selection(snapshot, &record.selection, now).is_err()
        {
            record.delivery.state = DeliveryState::Expired;
            record.delivery.reason = "选择已过期、构建已改变或预览已失效；请重新选择。".into();
            record.delivery.updated_at = now;
        }
        if let Some(binding) = &record.delivery.target {
            if matches!(
                binding.transport_kind,
                AgentTransportKind::ClaudeChannel
                    | AgentTransportKind::CodexAppServer
                    | AgentTransportKind::CodexDesktopPlugin
            ) && self
                .bridge
                .as_ref()
                .is_none_or(|bridge| bridge.verify_session(binding, now).is_err())
            {
                if record.delivery.state == DeliveryState::Sending {
                    record.delivery.state = DeliveryState::Unknown;
                    record.delivery.reason = "连接已失效，提交结果未知；不会自动重发。".into();
                } else if record.delivery.state == DeliveryState::Ready {
                    record.delivery.state = DeliveryState::SessionUnverified;
                    record.delivery.reason = "通道心跳或授权已失效，请重新配对。".into();
                }
            }
            if !self
                .bindings
                .get(&binding.binding_id)
                .is_some_and(|current| current == binding)
                || validate_binding(snapshot, binding, now).is_err()
            {
                if !matches!(
                    record.delivery.state,
                    DeliveryState::Ready
                        | DeliveryState::Unsupported
                        | DeliveryState::SessionUnverified
                        | DeliveryState::Sending
                ) {
                    return Ok(record.delivery.clone());
                }
                record.delivery.state = if matches!(
                    record.delivery.state,
                    DeliveryState::Sending | DeliveryState::Unknown
                ) {
                    DeliveryState::Unknown
                } else {
                    DeliveryState::SessionUnverified
                };
                record.delivery.reason = "绑定已过期、线程已切换或目标进程生命周期已改变。".into();
                record.delivery.updated_at = now;
            }
        }
        Ok(record.delivery.clone())
    }

    pub fn send(
        &mut self,
        snapshot: &AppSnapshot,
        request_id: &str,
        now: u64,
    ) -> Result<AgentDelivery, String> {
        let current = self.status(snapshot, request_id, now)?;
        if current.state != DeliveryState::Ready {
            return Ok(current);
        }
        let Some(binding) = current.target.as_ref() else {
            return Ok(current);
        };
        let (state, reason) = availability(Some(binding));
        let record = self
            .deliveries
            .get_mut(request_id)
            .ok_or("AGENT_DELIVERY_NOT_FOUND")?;
        record.delivery.state = state;
        record.delivery.reason = reason;
        record.delivery.updated_at = now;
        if matches!(
            binding.transport_kind,
            AgentTransportKind::ClaudeChannel
                | AgentTransportKind::CodexAppServer
                | AgentTransportKind::CodexDesktopPlugin
        ) {
            record.delivery.state = DeliveryState::SessionUnverified;
            record.delivery.reason = "必须经 send_with_attachments 校验附件后提交。".into();
        }
        #[cfg(test)]
        if record.delivery.state == DeliveryState::Ready {
            if let Some(adapter) = &self.verified_adapter {
                if let Err(error) = adapter.verify(binding) {
                    record.delivery.state = DeliveryState::SessionUnverified;
                    record.delivery.reason = error;
                } else {
                    record.delivery.state = DeliveryState::Sending;
                    match adapter.submit(binding, request_id, &record.delivery.prompt) {
                        Ok(receipt)
                            if receipt.request_id == request_id
                                && receipt.thread_id == binding.conversation_or_thread_id
                                && valid_id(&receipt.receipt_id) =>
                        {
                            record.delivery.state = DeliveryState::Submitted;
                            record.delivery.receipt_id = Some(receipt.receipt_id);
                            record.delivery.reason =
                                "目标会话确认接受了消息；不代表修改已完成。".into();
                        }
                        Ok(_) => {
                            record.delivery.state = DeliveryState::Failed;
                            record.delivery.reason =
                                "提交回执与本次请求不一致；请检查目标会话。".into();
                        }
                        Err(error) => {
                            record.delivery.state = DeliveryState::Failed;
                            record.delivery.reason = error;
                        }
                    }
                }
            } else {
                record.delivery.state = DeliveryState::Unsupported;
                record.delivery.reason = "未接入：当前通道不支持向该会话提交。".into();
            }
        }
        Ok(record.delivery.clone())
    }

    pub fn send_checked(
        &mut self,
        snapshot: &AppSnapshot,
        selection: &SelectionContext,
        request_id: &str,
        now: u64,
    ) -> Result<AgentDelivery, String> {
        validate_selection(snapshot, selection, now)?;
        let record = self
            .deliveries
            .get(request_id)
            .ok_or("AGENT_DELIVERY_NOT_FOUND")?;
        if &record.selection != selection {
            return Err("AGENT_SELECTION_CHANGED".into());
        }
        self.send(snapshot, request_id, now)
    }

    pub fn sync(&mut self, snapshot: &AppSnapshot, now: u64) {
        if let Some(bridge) = &self.bridge {
            bridge.sync(snapshot, now);
        }
    }

    /// Stages a new pairing credential for `credential_path` without changing the
    /// live pairing, and returns the exact credential to persist.
    ///
    /// The caller persists the credential and then calls [`Self::activate_pair`]
    /// with the returned `pairing_id`, or [`Self::discard_pair`] when the write
    /// failed. Nothing the previous pairing could do is lost before that.
    pub fn prepare_pair(
        &mut self,
        snapshot: &AppSnapshot,
        kind: AgentKind,
        credential_path: &str,
        now: u64,
    ) -> Result<AgentPairing, String> {
        if credential_path.is_empty()
            || credential_path.len() > 4096
            || credential_path.contains('\0')
        {
            return Err("AGENT_PAIR_PATH_INVALID".into());
        }
        if self.bridge.is_none() {
            self.bridge = Some(SessionBridge::start()?);
        }
        let mut prepared = self
            .bridge
            .as_ref()
            .ok_or("AGENT_BRIDGE_UNAVAILABLE")?
            .prepare_pair(snapshot, kind, now)?;
        prepared.set_config_path(credential_path.to_owned());
        let pairing = prepared.pairing().clone();
        // A newer preparation replaces an unwritten one; the older credential was
        // never authoritative, so dropping it revokes nothing.
        self.pending_pair = Some(prepared);
        Ok(pairing)
    }

    /// Makes a staged credential authoritative, replacing the previous pairing.
    ///
    /// Only a credential this router staged may be activated, and the project,
    /// session, target and trust identity must be unchanged since preparation. A
    /// request for a superseded credential leaves the current staging intact.
    pub fn activate_pair(
        &mut self,
        pairing_id: &str,
        snapshot: &AppSnapshot,
        now: u64,
    ) -> Result<AgentPairing, String> {
        self.commit_pair(pairing_id, snapshot, now, || Ok(()))
    }

    /// The actor calls this only with an already staged credential. Scope and
    /// expiry are checked under the same bridge transaction as its file swap.
    pub(crate) fn commit_pair(
        &mut self,
        pairing_id: &str,
        snapshot: &AppSnapshot,
        now: u64,
        commit: impl FnOnce() -> Result<(), String>,
    ) -> Result<AgentPairing, String> {
        let Some(pending) = self.pending_pair.as_ref() else {
            return Err("AGENT_PAIR_PREPARE_REQUIRED".into());
        };
        if pending.pairing().pairing_id != pairing_id {
            return Err("AGENT_PAIR_PREPARE_SUPERSEDED".into());
        }
        let prepared = self
            .pending_pair
            .take()
            .expect("the pending credential was just checked");
        let pairing = self
            .bridge
            .as_ref()
            .ok_or("AGENT_BRIDGE_UNAVAILABLE")?
            .commit_pair(prepared, snapshot, now, commit)?;
        // Declared sessions and their evidence belonged to the replaced pairing.
        self.bindings.clear();
        Ok(pairing)
    }

    /// Abandons a staged credential whose write failed, leaving the live pairing
    /// exactly as it was.
    pub fn discard_pair(&mut self, pairing_id: &str) {
        if self
            .pending_pair
            .as_ref()
            .is_some_and(|prepared| prepared.pairing().pairing_id == pairing_id)
        {
            self.pending_pair = None;
        }
    }

    pub fn revoke(&mut self) {
        self.pending_pair = None;
        if let Some(bridge) = &self.bridge {
            bridge.revoke();
        }
        self.bindings.clear();
    }

    pub fn bind_registration(
        &mut self,
        snapshot: &AppSnapshot,
        id: &str,
        now: u64,
    ) -> Result<AgentSessionBinding, String> {
        self.sync(snapshot, now);
        let registration = self
            .bridge
            .as_ref()
            .ok_or("AGENT_PAIR_REQUIRED")?
            .registrations(now)
            .into_iter()
            .find(|r| r.registration_id == id)
            .ok_or("AGENT_REGISTRATION_EXPIRED")?;
        let draft = AgentBindingDraft {
            agent_kind: registration.agent_kind.clone(),
            agent_session_id: registration.agent_session_id.clone(),
            conversation_or_thread_id: registration.conversation_or_thread_id.clone(),
        };
        if let Some(binding) = self.bindings.values().find(|binding| {
            binding.transport_instance_id.as_ref() == Some(&registration.transport_instance_id)
                && binding.agent_session_id == registration.agent_session_id
                && binding.conversation_or_thread_id == registration.conversation_or_thread_id
                && validate_binding(snapshot, binding, now).is_ok()
        }) {
            return Ok(binding.clone());
        }
        let mut binding = self.bind(snapshot, &draft, now)?;
        binding.transport_kind = registration.transport_kind;
        binding.transport_instance_id = Some(registration.transport_instance_id);
        binding.agent_version = Some(registration.agent_version);
        binding.writable = true;
        binding.capabilities = registration.capabilities;
        binding.expires_at = now.saturating_add(8 * 60 * 60 * 1000);
        binding.verified_at = registration.observed_at;
        binding.binding_evidence="用户确认配对的专用通道实例、工程根和会话；每次提交由通道再次核验。窗口身份不证明会话 ID。".into();
        binding.reason = registration.reason;
        self.bindings
            .insert(binding.binding_id.clone(), binding.clone());
        Ok(binding)
    }

    pub fn send_with_attachments(
        &mut self,
        snapshot: &AppSnapshot,
        selection: &SelectionContext,
        id: &str,
        attachments: Attachments,
        now: u64,
    ) -> Result<AgentDelivery, String> {
        validate_selection(snapshot, selection, now)?;
        let current = self.status(snapshot, id, now)?;
        let record = self.deliveries.get(id).ok_or("AGENT_DELIVERY_NOT_FOUND")?;
        if &record.selection != selection {
            return Err("AGENT_SELECTION_CHANGED".into());
        }
        if current.state != DeliveryState::Ready {
            return Ok(current);
        }
        let binding = current.target.as_ref().ok_or("AGENT_BINDING_REQUIRED")?;
        let result = self
            .bridge
            .as_ref()
            .ok_or("AGENT_TRANSPORT_UNAVAILABLE")?
            .submit_message(binding, &current, selection, attachments, now);
        let record = self
            .deliveries
            .get_mut(id)
            .ok_or("AGENT_DELIVERY_NOT_FOUND")?;
        match result {
            Ok(()) => {
                record.delivery.state = DeliveryState::Sending;
                record.delivery.reason = "通道正在核验目标会话；尚未取得提交回执。".into();
            }
            Err(reason) => {
                // Busy rejects before queueing; caller may explicitly retry after the active turn.
                record.delivery.state = if reason == "AGENT_BUSY_RETRY_AFTER_CURRENT_TURN" {
                    DeliveryState::Ready
                } else {
                    DeliveryState::SessionUnverified
                };
                record.delivery.reason = reason;
            }
        }
        record.delivery.updated_at = now;
        Ok(record.delivery.clone())
    }

    pub fn assisted_delivery(
        &mut self,
        snapshot: &AppSnapshot,
        selection: &SelectionContext,
        result: Option<AssistedFill>,
        now: u64,
    ) -> Result<AgentDelivery, String> {
        validate_selection(snapshot, selection, now)?;
        if selection.user_instruction.trim().is_empty() {
            return Err("AGENT_INSTRUCTION_REQUIRED".into());
        }
        let target = snapshot.target.as_ref().ok_or("AGENT_TARGET_REQUIRED")?;
        let identity = serde_json::to_vec(&(
            &selection.selection_id,
            &selection.user_instruction,
            &target.id,
            &target.process,
            DeliveryMode::Assisted,
        ))
        .map_err(|_| "AGENT_CONTEXT_INVALID")?;
        let id = format!("delivery-{:x}", Sha256::digest(identity));
        if let Some(record) = self.deliveries.get_mut(&id) {
            if let Some(result) = result {
                if record.delivery.state == DeliveryState::Sending {
                    record.delivery.state = result.state;
                    record.delivery.reason = result.reason;
                    record.delivery.updated_at = now;
                }
            }
            return Ok(record.delivery.clone());
        }
        if self.deliveries.len() >= MAX_DELIVERIES {
            return Err("AGENT_DELIVERY_LIMIT_WAIT_FOR_SELECTION_EXPIRY".into());
        }
        let delivery = AgentDelivery {
            request_id: id.clone(),
            selection_id: selection.selection_id.clone(),
            binding_id: None,
            state: DeliveryState::Sending,
            prompt: format_compact_prompt(selection),
            target: None,
            receipt_id: None,
            reason: "等待目标输入框填写结果；不会自动提交。".into(),
            created_at: now,
            updated_at: now,
            idempotency_key: id.clone(),
            delivery_mode: DeliveryMode::Assisted,
            transport_receipt: None,
            agent_acknowledgment: None,
        };
        self.deliveries.insert(
            id,
            DeliveryRecord {
                delivery: delivery.clone(),
                selection: selection.clone(),
            },
        );
        Ok(delivery)
    }

    pub fn finish_assisted(
        &mut self,
        id: &str,
        result: AssistedFill,
        now: u64,
    ) -> Result<AgentDelivery, String> {
        let record = self
            .deliveries
            .get_mut(id)
            .ok_or("AGENT_DELIVERY_NOT_FOUND")?;
        if record.delivery.delivery_mode != DeliveryMode::Assisted
            || record.delivery.state != DeliveryState::Sending
        {
            return Ok(record.delivery.clone());
        }
        record.delivery.state = result.state;
        record.delivery.reason = result.reason;
        record.delivery.updated_at = now;
        Ok(record.delivery.clone())
    }

    fn cleanup(&mut self, now: u64) {
        self.bindings
            .retain(|_, binding| binding.expires_at > now && binding.observed_at <= now);
        self.deliveries.retain(|_, record| {
            // A successful edit may rebuild immediately; keep its receipt history
            // independently of the ten-minute image/context grant.
            let retain_until = if matches!(
                record.delivery.state,
                DeliveryState::Ready
                    | DeliveryState::Unsupported
                    | DeliveryState::SessionUnverified
                    | DeliveryState::Expired
            ) {
                record.selection.expires_at
            } else {
                record.delivery.created_at.saturating_add(60 * 60 * 1000)
            };
            retain_until > now && record.selection.created_at <= now
        });
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 256 && !id.chars().any(char::is_control)
}

fn validate_binding(
    snapshot: &AppSnapshot,
    binding: &AgentSessionBinding,
    now: u64,
) -> Result<(), String> {
    if snapshot.project.as_ref().map(|project| project.id.as_str())
        != Some(binding.workspace_id.as_str())
    {
        return Err("AGENT_WORKSPACE_MISMATCH".into());
    }
    if snapshot
        .target
        .as_ref()
        .is_none_or(|target| target.id != binding.target_id || target.process != binding.process)
    {
        return Err("AGENT_TARGET_LIFECYCLE_MISMATCH".into());
    }
    if binding.observed_at > now || binding.expires_at <= now {
        return Err("AGENT_BINDING_EXPIRED".into());
    }
    Ok(())
}

fn validate_delivery_scope(
    snapshot: &AppSnapshot,
    selection: &SelectionContext,
) -> Result<(), String> {
    if snapshot.project.as_ref().map(|project| project.id.as_str())
        != Some(selection.workspace_id.as_str())
        || snapshot.session_id.as_deref() != Some(selection.session_id.as_str())
    {
        return Err("AGENT_SELECTION_SCOPE_MISMATCH".into());
    }
    Ok(())
}

fn availability(binding: Option<&AgentSessionBinding>) -> (DeliveryState, String) {
    let Some(binding) = binding else {
        return (DeliveryState::Unsupported, "未注册已核验的当前 Agent 会话写入通道。可以复制提示词、导出截图或通过显式授权的只读 MCP 获取选择上下文。".into());
    };
    if binding.transport_kind == AgentTransportKind::Unsupported {
        return (
            DeliveryState::Unsupported,
            adapters::status(binding.agent_kind.clone()).reason,
        );
    }
    if !binding.writable || !binding.capabilities.identify_current_session {
        return (
            DeliveryState::SessionUnverified,
            "当前活动会话及写入权限尚未核验。".into(),
        );
    }
    if !binding.capabilities.direct_send
        || !binding.capabilities.receipt
        || !binding.capabilities.images
    {
        return (
            DeliveryState::Unsupported,
            "适配器没有核验直接发送、截图和提交回执能力。".into(),
        );
    }
    (
        DeliveryState::Ready,
        "当前会话具有已核验的发送能力；提交前再次核验。".into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::pairing::{encode, CredentialStore, FileCredentials};
    use crate::selection::test_support::{context, snapshot};
    use std::path::{Path, PathBuf};
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    struct TestAdapter {
        calls: Arc<AtomicUsize>,
        actual_thread: String,
        receipt_thread: String,
    }
    impl adapters::VerifiedAdapter for TestAdapter {
        fn verify(&self, binding: &AgentSessionBinding) -> Result<(), String> {
            if binding.conversation_or_thread_id != self.actual_thread {
                Err("TEST_ONLY_THREAD_CHANGED".into())
            } else {
                Ok(())
            }
        }
        fn submit(
            &self,
            _: &AgentSessionBinding,
            request_id: &str,
            _: &str,
        ) -> Result<adapters::Receipt, String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(adapters::Receipt {
                request_id: request_id.into(),
                thread_id: self.receipt_thread.clone(),
                receipt_id: "test-only-receipt".into(),
            })
        }
    }

    fn draft(thread: &str) -> AgentBindingDraft {
        AgentBindingDraft {
            agent_kind: AgentKind::Codex,
            agent_session_id: "agent-session".into(),
            conversation_or_thread_id: thread.into(),
        }
    }

    fn verified_router(
        actual_thread: &str,
        receipt_thread: &str,
    ) -> (AgentRouter, String, Arc<AtomicUsize>) {
        let mut router = AgentRouter::new();
        let binding = router.bind(&snapshot(), &draft("thread"), 100).unwrap();
        let mutable = router.bindings.get_mut(&binding.binding_id).unwrap();
        mutable.transport_kind = AgentTransportKind::ExplicitBridge;
        mutable.writable = true;
        mutable.capabilities.identify_current_session = true;
        mutable.capabilities.direct_send = true;
        mutable.capabilities.receipt = true;
        mutable.capabilities.images = true;
        let calls = Arc::new(AtomicUsize::new(0));
        router.verified_adapter = Some(Box::new(TestAdapter {
            calls: calls.clone(),
            actual_thread: actual_thread.into(),
            receipt_thread: receipt_thread.into(),
        }));
        (router, binding.binding_id, calls)
    }

    #[test]
    fn product_name_and_manual_binding_never_grant_write_access() {
        let mut router = AgentRouter::new();
        let state = snapshot();
        let list = router.list(&state, 100);
        assert!(list
            .adapters
            .iter()
            .all(|adapter| adapter.status == DeliveryState::Unsupported
                && !adapter.capabilities.direct_send));
        let binding = router.bind(&state, &draft("thread"), 100).unwrap();
        assert!(!binding.writable);
        let prepared = router
            .prepare(&state, &context(100), Some(&binding.binding_id), 101)
            .unwrap();
        assert_eq!(prepared.state, DeliveryState::Unsupported);
        let sent = router
            .send_checked(&state, &context(100), &prepared.request_id, 102)
            .unwrap();
        assert_eq!(sent.state, DeliveryState::Unsupported);
        assert!(sent.receipt_id.is_none());
    }

    #[test]
    fn wrong_workspace_or_target_lifecycle_cannot_prepare_or_read_prompt() {
        let mut router = AgentRouter::new();
        let state = snapshot();
        let binding = router.bind(&state, &draft("thread"), 100).unwrap();
        let prepared = router
            .prepare(&state, &context(100), Some(&binding.binding_id), 101)
            .unwrap();
        let mut other = state.clone();
        other.project.as_mut().unwrap().id = "other".into();
        assert!(router
            .prepare(&other, &context(100), Some(&binding.binding_id), 101)
            .is_err());
        assert_eq!(
            router
                .status(&other, &prepared.request_id, 101)
                .unwrap_err(),
            "AGENT_SELECTION_SCOPE_MISMATCH"
        );
        other = state;
        other.target.as_mut().unwrap().process.started_at = "new-process".into();
        assert_eq!(
            router
                .prepare(&other, &context(100), Some(&binding.binding_id), 101)
                .unwrap_err(),
            "AGENT_TARGET_LIFECYCLE_MISMATCH"
        );
        assert_eq!(
            router
                .status(&other, &prepared.request_id, 102)
                .unwrap()
                .state,
            DeliveryState::SessionUnverified
        );
    }

    #[test]
    fn active_thread_switch_and_expired_binding_never_submit() {
        let (mut router, id, calls) = verified_router("different-active-thread", "thread");
        let selection = context(100);
        let prepared = router
            .prepare(&snapshot(), &selection, Some(&id), 101)
            .unwrap();
        assert_eq!(
            router
                .send_checked(&snapshot(), &selection, &prepared.request_id, 102)
                .unwrap()
                .state,
            DeliveryState::SessionUnverified
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        let (mut router, id, calls) = verified_router("thread", "thread");
        let prepared = router
            .prepare(&snapshot(), &selection, Some(&id), 101)
            .unwrap();
        router.bind(&snapshot(), &draft("new-thread"), 102).unwrap();
        assert_eq!(
            router
                .send_checked(&snapshot(), &selection, &prepared.request_id, 103)
                .unwrap()
                .state,
            DeliveryState::SessionUnverified
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        let (mut router, id, calls) = verified_router("thread", "thread");
        let prepared = router
            .prepare(&snapshot(), &selection, Some(&id), 101)
            .unwrap();
        assert_eq!(
            router
                .send_checked(&snapshot(), &selection, &prepared.request_id, 60_100)
                .unwrap()
                .state,
            DeliveryState::SessionUnverified
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn duplicate_request_has_one_actual_test_submit_and_one_matching_receipt() {
        // Fake adapter verifies the state machine only, not Codex/Claude delivery.
        let (mut router, id, calls) = verified_router("thread", "thread");
        let selection = context(100);
        let first = router
            .prepare(&snapshot(), &selection, Some(&id), 101)
            .unwrap();
        let second = router
            .prepare(&snapshot(), &selection, Some(&id), 102)
            .unwrap();
        assert_eq!(first.request_id, second.request_id);
        let submitted = router
            .send_checked(&snapshot(), &selection, &first.request_id, 103)
            .unwrap();
        assert_eq!(submitted.state, DeliveryState::Submitted);
        assert_eq!(
            router
                .send_checked(&snapshot(), &selection, &first.request_id, 104)
                .unwrap()
                .receipt_id,
            submitted.receipt_id
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn submitted_history_survives_expected_rebuild_and_image_expiration() {
        let (mut router, id, calls) = verified_router("thread", "thread");
        let selection = context(100);
        let prepared = router
            .prepare(&snapshot(), &selection, Some(&id), 101)
            .unwrap();
        router
            .send_checked(&snapshot(), &selection, &prepared.request_id, 102)
            .unwrap();
        // Simulate an Agent edit rebuilding the same preview. Image access expires,
        // but the independently obtained delivery receipt remains truthful.
        let mut changed = snapshot();
        changed.build.generation += 1;
        let receipt = router.status(&changed, &prepared.request_id, 103).unwrap();
        assert_eq!(receipt.state, DeliveryState::Submitted);
        assert_eq!(receipt.receipt_id.as_deref(), Some("test-only-receipt"));
        router.cleanup(selection.expires_at + 1);
        assert!(router.deliveries.contains_key(&prepared.request_id));
        assert!(router
            .send_checked(&changed, &selection, &prepared.request_id, 104)
            .is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn wrong_receipt_and_requirement_change_never_report_submitted() {
        let (mut router, id, calls) = verified_router("thread", "other-thread");
        let selection = context(100);
        let prepared = router
            .prepare(&snapshot(), &selection, Some(&id), 101)
            .unwrap();
        let mut changed = selection.clone();
        changed.user_instruction = "A different requirement".into();
        assert_eq!(
            router
                .send_checked(&snapshot(), &changed, &prepared.request_id, 102)
                .unwrap_err(),
            "AGENT_SELECTION_CHANGED"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        let failed = router
            .send_checked(&snapshot(), &selection, &prepared.request_id, 103)
            .unwrap();
        assert_eq!(failed.state, DeliveryState::Failed);
        assert!(failed.receipt_id.is_none());
        assert_eq!(
            router
                .send_checked(&snapshot(), &selection, &prepared.request_id, 104)
                .unwrap()
                .state,
            DeliveryState::Failed
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "A mismatched/lost receipt cannot authorize a second submit"
        );
        assert_eq!(
            router
                .status(&snapshot(), &prepared.request_id, selection.expires_at)
                .unwrap()
                .state,
            DeliveryState::Failed
        );
    }

    #[test]
    fn rapid_real_bridge_receipts_survive_first_status_rebuild_and_selection_expiry() {
        use std::io::{Read, Write};
        use std::net::TcpStream;
        use std::time::Duration;
        fn post(pair: &AgentPairing, route: &str, body: serde_json::Value) -> serde_json::Value {
            let address = pair.endpoint.strip_prefix("http://").unwrap();
            let bytes = body.to_string();
            let mut stream = TcpStream::connect(address).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            write!(stream, "POST {route} HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{bytes}", pair.token, bytes.len()).unwrap();
            let mut headers = Vec::new();
            while !headers.ends_with(b"\r\n\r\n") {
                assert!(headers.len() < 8192);
                let mut byte = [0u8; 1];
                stream.read_exact(&mut byte).unwrap();
                headers.push(byte[0]);
            }
            let headers = String::from_utf8(headers).unwrap();
            assert!(
                headers.starts_with("HTTP/1.1 200"),
                "Fixture HTTP request was rejected"
            );
            let length: usize = headers
                .lines()
                .find_map(|line| line.strip_prefix("Content-Length: "))
                .unwrap()
                .parse()
                .unwrap();
            assert!(length <= 128 * 1024);
            let mut bytes = vec![0u8; length];
            stream.read_exact(&mut bytes).unwrap();
            serde_json::from_slice(&bytes).unwrap()
        }
        let directory = tempfile::tempdir().unwrap();
        let now = crate::selection::now_ms();
        let mut state = snapshot();
        state.project.as_mut().unwrap().workspace_root =
            directory.path().to_string_lossy().into_owned();
        let mut router = AgentRouter::new();
        // Production commit boundary: the credential is durable before the
        // pairing becomes authoritative.
        let credentials = directory.path().join("arkdock-pairing.json");
        let pair = router
            .prepare_pair(
                &state,
                AgentKind::Claude,
                &credentials.to_string_lossy(),
                now,
            )
            .unwrap();
        crate::agent::pairing::write_credential(&credentials, &pair).unwrap();
        let pair = router.activate_pair(&pair.pairing_id, &state, now).unwrap();
        let registration = post(
            &pair,
            "/v1/register",
            serde_json::json!({
            "transportKind":"claude-channel","instanceId":"router-fixture","agentKind":"claude",
            "agentVersion":"fixture","sessionId":"host-session","conversationId":"host-session",
            "workspaceRoot":pair.workspace_root,"pid":std::process::id(),"capabilities":{"images":true}}),
        );
        let binding = router
            .bind_registration(
                &state,
                registration["registrationId"].as_str().unwrap(),
                now,
            )
            .unwrap();
        let selection = context(now);
        let prepared = router
            .prepare(&state, &selection, Some(&binding.binding_id), now)
            .unwrap();
        router
            .send_with_attachments(
                &state,
                &selection,
                &prepared.request_id,
                Attachments::default(),
                now,
            )
            .unwrap();
        let poll = post(
            &pair,
            "/v1/poll",
            serde_json::json!({"instanceId":"router-fixture",
            "sessionId":"host-session","conversationId":"host-session","workspaceRoot":pair.workspace_root,"busy":false}),
        );
        assert_eq!(poll["deliveries"][0]["deliveryId"], prepared.request_id);
        for (state, receipt) in [
            ("TRANSPORT_ACCEPTED", "transport-fixture"),
            ("AGENT_ACKNOWLEDGED", "ack-fixture"),
            ("COMPLETED", "complete-fixture"),
        ] {
            assert_eq!(
                post(
                    &pair,
                    "/v1/receipt",
                    serde_json::json!({"instanceId":"router-fixture",
                "sessionId":"host-session","conversationId":"host-session","deliveryId":prepared.request_id,
                "state":state,"receiptId":receipt,"reason":"Protocol fixture only"})
                )["accepted"],
                true
            );
        }
        let result = router
            .status(&state, &prepared.request_id, crate::selection::now_ms())
            .unwrap();
        assert_eq!(result.state, DeliveryState::Completed);
        assert_eq!(
            result.transport_receipt.as_deref(),
            Some("transport-fixture")
        );
        assert_eq!(result.agent_acknowledgment.as_deref(), Some("ack-fixture"));
        assert_eq!(result.receipt_id.as_deref(), Some("complete-fixture"));
        state.build.generation += 1;
        state.build.stale = true;
        let after_rebuild = router
            .status(&state, &prepared.request_id, crate::selection::now_ms())
            .unwrap();
        assert_eq!(after_rebuild.state, DeliveryState::Completed);
        router.list(&state, selection.expires_at + 1);
        let after_expiry = router
            .status(&state, &prepared.request_id, selection.expires_at + 1)
            .unwrap();
        assert_eq!(after_expiry.state, DeliveryState::Completed);
        assert_eq!(
            after_expiry.agent_acknowledgment.as_deref(),
            Some("ack-fixture")
        );
    }

    /// Injects a deterministic write failure. The real store is exercised by the
    /// directory cases below, which need no injection.
    struct FailingStore {
        /// What an interrupted same-directory write leaves behind: a sibling
        /// temporary file, never a truncated credential.
        partial: bool,
    }
    impl CredentialStore for FailingStore {
        fn write(&self, path: &Path, bytes: &[u8]) -> Result<(), String> {
            if self.partial {
                std::fs::write(path.with_extension("tmp"), &bytes[..bytes.len() / 2]).unwrap();
            }
            Err("PAIRING_WRITE_FAILED".into())
        }
    }

    struct PairFixture {
        router: AgentRouter,
        state: AppSnapshot,
        directory: tempfile::TempDir,
        now: u64,
    }

    impl PairFixture {
        fn new() -> Self {
            let directory = tempfile::tempdir().unwrap();
            let mut state = snapshot();
            let root = directory.path().to_string_lossy().into_owned();
            let project = state.project.as_mut().unwrap();
            project.workspace_root = root.clone();
            project.app_root = root;
            Self {
                router: AgentRouter::new(),
                state,
                directory,
                now: 1_000,
            }
        }

        fn path(&self, name: &str) -> PathBuf {
            self.directory.path().join(name)
        }

        /// Stages a credential exactly as the desktop actor does before writing.
        fn stage(&mut self, path: &Path) -> AgentPairing {
            self.router
                .prepare_pair(
                    &self.state,
                    AgentKind::Claude,
                    &path.to_string_lossy(),
                    self.now,
                )
                .unwrap()
        }

        fn persist(
            &self,
            pairing: &AgentPairing,
            store: &dyn CredentialStore,
            path: &Path,
        ) -> Result<(), String> {
            store.write(path, &encode(pairing)?)
        }

        /// The complete production sequence with an injectable store.
        fn commit(
            &mut self,
            store: &dyn CredentialStore,
            path: &Path,
        ) -> Result<AgentPairing, String> {
            let pairing = self.stage(path);
            match self.persist(&pairing, store, path) {
                Ok(()) => self.activate(&pairing),
                Err(error) => {
                    self.router.discard_pair(&pairing.pairing_id);
                    Err(error)
                }
            }
        }

        fn activate(&mut self, pairing: &AgentPairing) -> Result<AgentPairing, String> {
            self.router
                .activate_pair(&pairing.pairing_id, &self.state, self.now)
        }

        /// An empty register body can never succeed, so `AGENT_REGISTRATION_INVALID`
        /// proves the bearer check and the live scope accepted this token without
        /// changing any state; anything else means it grants nothing here.
        fn authorizes(&self, pairing: &AgentPairing) -> bool {
            self.router
                .bridge
                .as_ref()
                .expect("a pairing attempt starts the bridge")
                .authorize(
                    &pairing.token,
                    "/v1/register",
                    serde_json::json!({}),
                    self.now,
                )
                .err()
                .as_deref()
                == Some("AGENT_REGISTRATION_INVALID")
        }

        fn stored(&self, path: &Path) -> AgentPairing {
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
        }
    }

    #[test]
    fn first_pairing_persists_exactly_the_activated_credential() {
        let mut fixture = PairFixture::new();
        let path = fixture.path("arkdock-pairing.json");
        let pairing = fixture.commit(&FileCredentials, &path).unwrap();
        // The file an Agent reads and the pairing the backend accepts are one
        // credential, and the report names the file it was saved to.
        assert_eq!(fixture.stored(&path), pairing);
        assert_eq!(
            pairing.config_path.as_deref(),
            Some(path.to_string_lossy().as_ref())
        );
        assert!(fixture.authorizes(&pairing));
    }

    #[test]
    fn transaction_scope_expiry_and_cancellation_leave_the_old_file_and_token_usable() {
        use crate::agent::pairing::{stage_credential, CommitPermit};
        use std::time::Duration;
        for rejection in ["scope", "expiry", "cancel"] {
            let mut fixture = PairFixture::new();
            let path = fixture.path("pairing.json");
            let first = fixture.commit(&FileCredentials, &path).unwrap();
            let prepared = fixture.stage(&path);
            let staged = stage_credential(&path, &prepared).unwrap();
            let permit = CommitPermit::new(Duration::from_secs(2));
            let mut state = fixture.state.clone();
            let mut observed = fixture.now;
            if rejection == "scope" {
                state.session_id = Some("changed".into());
            }
            if rejection == "expiry" {
                observed = prepared.expires_at;
            }
            if rejection == "cancel" {
                permit.cancel();
            }
            let error = fixture
                .router
                .commit_pair(&prepared.pairing_id, &state, observed, || {
                    staged.commit(&permit).map(|_| ())
                })
                .unwrap_err();
            assert!(matches!(
                error.as_str(),
                "AGENT_PAIR_SCOPE_CHANGED" | "AGENT_PAIR_EXPIRED" | "PAIRING_WRITE_TIMEOUT"
            ));
            assert_eq!(fixture.stored(&path), first);
            assert!(fixture.authorizes(&first));
            assert!(!fixture.authorizes(&prepared));
        }
    }

    #[test]
    fn superseded_writer_at_the_same_path_cannot_clobber_the_committed_retry() {
        use crate::agent::pairing::{stage_credential, CommitPermit};
        use std::time::Duration;
        let mut fixture = PairFixture::new();
        let path = fixture.path("pairing.json");
        let first = fixture.commit(&FileCredentials, &path).unwrap();
        let a = fixture.stage(&path);
        let late = stage_credential(&path, &a).unwrap();
        let b = fixture.stage(&path);
        let retry = stage_credential(&path, &b).unwrap();
        let permit = CommitPermit::new(Duration::from_secs(2));
        let mut backup = None;
        let live = fixture
            .router
            .commit_pair(&b.pairing_id, &fixture.state, fixture.now, || {
                backup = retry.commit(&permit)?;
                Ok(())
            })
            .unwrap();
        if let Some(backup) = backup {
            std::fs::remove_file(backup).unwrap();
        }
        assert_eq!(
            fixture
                .router
                .commit_pair(&a.pairing_id, &fixture.state, fixture.now, || {
                    late.commit(&CommitPermit::new(Duration::from_secs(2)))
                        .map(|_| ())
                })
                .unwrap_err(),
            "AGENT_PAIR_PREPARE_REQUIRED"
        );
        assert_eq!(fixture.stored(&path), live);
        assert!(fixture.authorizes(&live));
        assert!(!fixture.authorizes(&first));
        assert!(!fixture.authorizes(&a));
        assert_eq!(
            std::fs::read_dir(fixture.directory.path()).unwrap().count(),
            1
        );
    }

    #[test]
    fn replacing_a_pairing_revokes_only_the_previous_credential() {
        let mut fixture = PairFixture::new();
        let first_path = fixture.path("first.json");
        let second_path = fixture.path("second.json");
        let first = fixture.commit(&FileCredentials, &first_path).unwrap();
        let second = fixture.commit(&FileCredentials, &second_path).unwrap();
        assert!(fixture.authorizes(&second));
        assert!(!fixture.authorizes(&first));
        assert_ne!(first.token, second.token);
        // The earlier file keeps the credential it was written with; it is simply
        // no longer a credential the backend accepts.
        assert_eq!(fixture.stored(&first_path), first);
    }

    #[test]
    fn failed_credential_write_keeps_the_previous_pairing_usable() {
        let mut fixture = PairFixture::new();
        let path = fixture.path("arkdock-pairing.json");
        let first = fixture.commit(&FileCredentials, &path).unwrap();
        // A staged credential whose bytes never reach the file.
        let staged = fixture.stage(&path);
        let error = fixture
            .persist(&staged, &FailingStore { partial: true }, &path)
            .unwrap_err();
        assert_eq!(error, "PAIRING_WRITE_FAILED");
        fixture.router.discard_pair(&staged.pairing_id);
        assert!(fixture.authorizes(&first));
        assert_eq!(fixture.stored(&path), first);
        assert!(!fixture.authorizes(&staged));
        assert!(!path.with_extension("tmp").to_string_lossy().is_empty());
    }

    #[test]
    fn unwritable_directory_and_unreplaceable_target_never_disturb_the_live_pairing() {
        let mut fixture = PairFixture::new();
        let path = fixture.path("arkdock-pairing.json");
        let first = fixture.commit(&FileCredentials, &path).unwrap();
        let missing = fixture
            .directory
            .path()
            .join("absent")
            .join("arkdock-pairing.json");
        assert_eq!(
            fixture.commit(&FileCredentials, &missing).unwrap_err(),
            "PAIRING_WRITE_FAILED"
        );
        let occupied = fixture.path("occupied.json");
        std::fs::create_dir(&occupied).unwrap();
        assert_eq!(
            fixture.commit(&FileCredentials, &occupied).unwrap_err(),
            "PAIRING_REPLACE_FAILED"
        );
        assert!(fixture.authorizes(&first));
        assert_eq!(fixture.stored(&path), first);
    }

    #[test]
    fn staged_credential_grants_nothing_until_it_is_activated() {
        let mut fixture = PairFixture::new();
        let path = fixture.path("arkdock-pairing.json");
        let first = fixture.commit(&FileCredentials, &path).unwrap();
        let staged_path = fixture.path("staged.json");
        let staged = fixture.stage(&staged_path);
        fixture
            .persist(&staged, &FileCredentials, &staged_path)
            .unwrap();
        // The saved credential is complete, and still grants nothing: only
        // activation installs a token the backend validates.
        assert_eq!(fixture.stored(&staged_path), staged);
        assert!(!fixture.authorizes(&staged));
        assert!(fixture.authorizes(&first));
    }

    #[test]
    fn project_session_target_or_trust_change_cancels_a_staged_activation() {
        let mutations: [fn(&mut AppSnapshot); 6] = [
            |state| state.project.as_mut().unwrap().id = "other-workspace".into(),
            |state| state.project.as_mut().unwrap().workspace_root = "C:/other".into(),
            |state| state.session_id = Some("other-session".into()),
            |state| state.target.as_mut().unwrap().id = "other-target".into(),
            |state| {
                state.target.as_mut().unwrap().process.started_at = "lifecycle-2".into();
            },
            |state| state.trusted = false,
        ];
        for mutate in mutations {
            let mut fixture = PairFixture::new();
            let path = fixture.path("arkdock-pairing.json");
            let first = fixture.commit(&FileCredentials, &path).unwrap();
            let staged = fixture.stage(&path);
            fixture.persist(&staged, &FileCredentials, &path).unwrap();
            mutate(&mut fixture.state);
            let error = fixture.activate(&staged).unwrap_err();
            assert!(
                matches!(
                    error.as_str(),
                    "AGENT_PAIR_SCOPE_CHANGED" | "AGENT_LOCAL_TRUSTED_WORKSPACE_REQUIRED"
                ),
                "unexpected rejection: {error}"
            );
            // The previous pairing is untouched: it is what the bridge still holds.
            assert!(fixture.authorizes(&first));
            assert!(!fixture.authorizes(&staged));
            assert_eq!(
                fixture.activate(&staged).unwrap_err(),
                "AGENT_PAIR_PREPARE_REQUIRED"
            );
        }
    }

    #[test]
    fn concurrent_pairing_requests_leave_exactly_one_authorized_credential() {
        let mut fixture = PairFixture::new();
        let first_path = fixture.path("first.json");
        let a_path = fixture.path("a.json");
        let b_path = fixture.path("b.json");
        let first = fixture.commit(&FileCredentials, &first_path).unwrap();
        // Two preparations race before either is written.
        let a = fixture.stage(&a_path);
        let b = fixture.stage(&b_path);
        fixture.persist(&b, &FileCredentials, &b_path).unwrap();
        // The superseded preparation can no longer be activated.
        assert_eq!(
            fixture.activate(&a).unwrap_err(),
            "AGENT_PAIR_PREPARE_SUPERSEDED"
        );
        assert!(fixture.authorizes(&first));
        let activated = fixture.activate(&b).unwrap();
        assert!(fixture.authorizes(&activated));
        assert!(!fixture.authorizes(&first));
        assert!(!fixture.authorizes(&a));
        assert_eq!(fixture.stored(&b_path), activated);
    }

    #[test]
    fn restart_grants_nothing_from_a_persisted_credential() {
        let mut fixture = PairFixture::new();
        let path = fixture.path("arkdock-pairing.json");
        let pairing = fixture.commit(&FileCredentials, &path).unwrap();
        // A restarted backend holds no live pairing, so the credential file - the
        // only survivor of a crash - authorizes nothing on its own.
        let restarted = SessionBridge::detached();
        restarted.sync(&fixture.state, fixture.now);
        assert_eq!(
            restarted
                .authorize(
                    &pairing.token,
                    "/v1/register",
                    serde_json::json!({}),
                    fixture.now
                )
                .unwrap_err(),
            "AGENT_PAIR_REQUIRED"
        );
        // Pairing again from the surviving workspace succeeds normally.
        let mut recovered = PairFixture::new();
        let recovered_path = recovered.path("arkdock-pairing.json");
        let again = recovered.commit(&FileCredentials, &recovered_path).unwrap();
        assert!(recovered.authorizes(&again));
    }

    #[test]
    fn revoke_drops_a_staged_credential_and_the_live_pairing() {
        let mut fixture = PairFixture::new();
        let path = fixture.path("arkdock-pairing.json");
        let first = fixture.commit(&FileCredentials, &path).unwrap();
        let staged = fixture.stage(&path);
        fixture.router.revoke();
        assert!(!fixture.authorizes(&first));
        assert_eq!(
            fixture.activate(&staged).unwrap_err(),
            "AGENT_PAIR_PREPARE_REQUIRED"
        );
    }

    #[test]
    fn retry_after_a_failed_pairing_leaves_no_duplicate_state() {
        let mut fixture = PairFixture::new();
        let path = fixture.path("arkdock-pairing.json");
        let first = fixture.commit(&FileCredentials, &path).unwrap();
        assert_eq!(
            fixture
                .commit(&FailingStore { partial: true }, &path)
                .unwrap_err(),
            "PAIRING_WRITE_FAILED"
        );
        assert!(fixture.authorizes(&first));
        // The retry replaces both the file and the live pairing, without a second
        // live credential and without the discarded staging resurfacing.
        let retried = fixture.commit(&FileCredentials, &path).unwrap();
        assert_eq!(fixture.stored(&path), retried);
        assert!(fixture.authorizes(&retried));
        assert!(!fixture.authorizes(&first));
        assert_eq!(
            fixture.activate(&first).unwrap_err(),
            "AGENT_PAIR_PREPARE_REQUIRED"
        );
    }

    #[test]
    fn pairing_failures_never_expose_the_token() {
        let mut fixture = PairFixture::new();
        let path = fixture.path("arkdock-pairing.json");
        let first = fixture.commit(&FileCredentials, &path).unwrap();
        let staged = fixture.stage(&path);
        let mut errors = vec![fixture
            .persist(&staged, &FailingStore { partial: false }, &path)
            .unwrap_err()];
        fixture.router.discard_pair(&staged.pairing_id);
        let current = fixture.stage(&path);
        errors.push(fixture.activate(&staged).unwrap_err());
        fixture.state.session_id = Some("other-session".into());
        errors.push(fixture.activate(&current).unwrap_err());
        for error in errors {
            for token in [&first.token, &staged.token, &current.token] {
                assert!(
                    !error.contains(token.as_str()),
                    "a pairing error leaked a credential: {error}"
                );
            }
        }
    }
}
