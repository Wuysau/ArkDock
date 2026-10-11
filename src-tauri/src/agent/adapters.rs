use super::bridge::{Attachments, SessionBridge, TransportUpdate};
use crate::ipc::model::*;

/// Transport ownership is independent of a desktop window or product label.
/// Implementations must revalidate their owned host session at actual submission.
pub trait AgentTransport {
    fn discover_sessions(&self, now: u64) -> Vec<AgentRegistration>;
    fn verify_session(
        &self,
        binding: &AgentSessionBinding,
        now: u64,
    ) -> Result<AgentRegistration, String>;
    fn submit_message(
        &self,
        binding: &AgentSessionBinding,
        delivery: &AgentDelivery,
        selection: &SelectionContext,
        attachments: Attachments,
        now: u64,
    ) -> Result<(), String>;
    fn get_delivery_status(&self, id: &str, now: u64) -> Option<TransportUpdate>;
    fn dispose(&self);
}
impl AgentTransport for SessionBridge {
    fn discover_sessions(&self, now: u64) -> Vec<AgentRegistration> {
        self.registrations(now)
    }
    fn verify_session(
        &self,
        binding: &AgentSessionBinding,
        now: u64,
    ) -> Result<AgentRegistration, String> {
        self.verify(binding, now)
    }
    fn submit_message(
        &self,
        binding: &AgentSessionBinding,
        delivery: &AgentDelivery,
        selection: &SelectionContext,
        attachments: Attachments,
        now: u64,
    ) -> Result<(), String> {
        self.submit(binding, delivery, selection, attachments, now)
    }
    fn get_delivery_status(&self, id: &str, now: u64) -> Option<TransportUpdate> {
        self.delivery_status(id, now)
    }
    fn dispose(&self) {
        self.revoke();
    }
}

pub(super) fn no_capabilities() -> AgentCapabilities {
    AgentCapabilities {
        identify_current_session: false,
        direct_send: false,
        images: false,
        file_references: false,
        busy_state: false,
        receipt: false,
        cancel_pending: false,
    }
}

pub(super) fn status(kind: AgentKind) -> AgentAdapterStatus {
    let reason = match kind {
        AgentKind::Codex => "Codex app-server 支持 thread/turn，但尚未核验连接当前桌面会话的授权写入通道。不会启动第二个 app-server 或新线程代替发送。",
        AgentKind::Claude => "Claude Channels 可向已显式启用的运行中会话推送，但本机尚未注册、配对并核验该通道及提交回执。普通 MCP、resume 与 Remote Control 不提供 ArkDock 当前会话写入权限。",
    };
    AgentAdapterStatus {
        agent_kind: kind,
        capabilities: no_capabilities(),
        status: DeliveryState::Unsupported,
        reason: reason.into(),
    }
}

/// A future adapter must prove the current thread via an explicitly authorized
/// channel immediately before submit, including target process lifecycle and
/// workspace. A manual string registration cannot satisfy this interface.
#[cfg(test)]
pub(super) trait VerifiedAdapter: Send + Sync {
    fn verify(&self, binding: &AgentSessionBinding) -> Result<(), String>;
    fn submit(
        &self,
        binding: &AgentSessionBinding,
        request_id: &str,
        prompt: &str,
    ) -> Result<Receipt, String>;
}

#[cfg(test)]
pub(super) struct Receipt {
    pub request_id: String,
    pub thread_id: String,
    pub receipt_id: String,
}
