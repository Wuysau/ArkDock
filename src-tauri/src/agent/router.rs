use super::adapters;
use crate::ipc::model::*;
use crate::selection::{format_prompt, validate_selection};
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
    #[cfg(test)]
    verified_adapter: Option<Box<dyn adapters::VerifiedAdapter>>,
}

impl AgentRouter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn list(&mut self, snapshot: &AppSnapshot, now: u64) -> AgentSessionList {
        self.cleanup(now);
        let bindings = self
            .bindings
            .values()
            .filter(|binding| validate_binding(snapshot, binding, now).is_ok())
            .cloned()
            .collect();
        AgentSessionList {
            adapters: vec![
                adapters::status(AgentKind::Codex),
                adapters::status(AgentKind::Claude),
            ],
            bindings,
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
        let identity =
            serde_json::to_vec(&(selection, &binding)).map_err(|_| "AGENT_CONTEXT_INVALID")?;
        let request_id = format!("delivery-{:x}", Sha256::digest(identity));
        if let Some(existing) = self.deliveries.get(&request_id) {
            return Ok(existing.delivery.clone());
        }
        if self.deliveries.len() >= MAX_DELIVERIES {
            let id = self
                .deliveries
                .values()
                .filter(|record| record.delivery.state != DeliveryState::Sending)
                .min_by_key(|record| record.delivery.created_at)
                .map(|record| record.delivery.request_id.clone())
                .ok_or("AGENT_DELIVERY_LIMIT")?;
            self.deliveries.remove(&id);
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
        let record = self
            .deliveries
            .get_mut(request_id)
            .ok_or("AGENT_DELIVERY_NOT_FOUND")?;
        // No metadata or prompt from a previous workspace is returned.
        validate_delivery_scope(snapshot, &record.selection)?;
        if validate_selection(snapshot, &record.selection, now).is_err() {
            record.delivery.state = DeliveryState::Expired;
            record.delivery.reason = "选择已过期、构建已改变或预览已失效；请重新选择。".into();
            record.delivery.updated_at = now;
        }
        if let Some(binding) = &record.delivery.target {
            if !self
                .bindings
                .get(&binding.binding_id)
                .is_some_and(|current| current == binding)
                || validate_binding(snapshot, binding, now).is_err()
            {
                if record.delivery.state == DeliveryState::Expired {
                    return Ok(record.delivery.clone());
                }
                record.delivery.state = DeliveryState::SessionUnverified;
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
        if matches!(
            current.state,
            DeliveryState::Submitted
                | DeliveryState::Sending
                | DeliveryState::Expired
                | DeliveryState::SessionUnverified
                | DeliveryState::Failed
        ) {
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
        // There is intentionally no production submit path before an actual
        // current-session transport is implemented and independently verified.
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
                            record.delivery.reason = "AGENT_RECEIPT_MISMATCH".into();
                        }
                        Err(error) => {
                            record.delivery.state = DeliveryState::Failed;
                            record.delivery.reason = error;
                        }
                    }
                }
            } else {
                record.delivery.state = DeliveryState::Unsupported;
                record.delivery.reason = "AGENT_ADAPTER_NOT_IMPLEMENTED".into();
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

    fn cleanup(&mut self, now: u64) {
        self.bindings
            .retain(|_, binding| binding.expires_at > now && binding.observed_at <= now);
        self.deliveries.retain(|_, record| {
            record.selection.expires_at > now && record.selection.created_at <= now
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
    use crate::selection::test_support::{context, snapshot};
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
            DeliveryState::Expired
        );
    }
}
