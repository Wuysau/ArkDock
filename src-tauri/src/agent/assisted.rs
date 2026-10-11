//! Explicit, single-use authorization for a lifecycle-verified composer.
//!
//! This transport never submits messages. HWND/PID/UIA identities identify a
//! window and input control, not an Agent conversation. The user still submits.
use crate::ipc::model::{AssistedFill, AssistedOffer, TargetInfo};
use std::sync::{atomic::AtomicBool, Arc};

#[cfg(windows)]
mod windows;

pub const CONFIRMATION_TTL_MS: u64 = 30_000;
pub const MAX_PROMPT_BYTES: usize = 32 * 1024;

#[derive(Default)]
pub struct AssistedController {
    cancellation: Option<Arc<AtomicBool>>,
    #[cfg(windows)]
    pending: Option<windows::Confirmation>,
    #[cfg(all(windows, test))]
    fixture: Option<windows::FixtureProfile>,
}

impl AssistedController {
    /// Only the native caller may supply handles, after WindowController's
    /// current scope/lifecycle check. Never accept a frontend-provided HWND.
    pub fn inspect(
        &mut self,
        handle: isize,
        target: &TargetInfo,
        caller_handle: isize,
        now: u64,
    ) -> Result<AssistedOffer, String> {
        self.invalidate();
        #[cfg(windows)]
        {
            let profile = windows::profile(
                handle,
                target,
                #[cfg(test)]
                self.fixture.as_ref(),
            )?;
            let confirmation = windows::inspect(
                handle,
                target,
                caller_handle,
                now,
                profile,
                self.cancellation.as_deref(),
            )?;
            let offer = confirmation.offer.clone();
            self.pending = Some(confirmation);
            Ok(offer)
        }
        #[cfg(not(windows))]
        {
            let _ = (handle, target, caller_handle, now);
            Err("当前平台不支持 Windows 辅助填写；请复制修改要求。".into())
        }
    }

    /// The confirmation is consumed on *every* attempt, including stale or
    /// malformed requests. Retrying requires a fresh inspection and consent.
    pub fn fill(
        &mut self,
        handle: isize,
        target: &TargetInfo,
        caller_handle: isize,
        confirmation_id: &str,
        prompt: &str,
        now: u64,
    ) -> Result<AssistedFill, String> {
        #[cfg(windows)]
        let pending = self.pending.take();
        validate_prompt(prompt)?;
        #[cfg(windows)]
        {
            let pending = pending.ok_or("辅助填写确认已失效；请重新检查目标输入框。")?;
            windows::fill(
                pending,
                windows::FillRequest {
                    handle,
                    target,
                    caller_handle,
                    confirmation_id,
                    prompt,
                    now,
                },
                self.cancellation.as_deref(),
            )
        }
        #[cfg(not(windows))]
        {
            let _ = (handle, target, caller_handle, confirmation_id, now);
            Err("当前平台不支持 Windows 辅助填写；请复制修改要求。".into())
        }
    }

    pub fn invalidate(&mut self) {
        #[cfg(windows)]
        {
            self.pending = None;
        }
    }

    /// The desktop actor cancels independently while a blocking provider call
    /// is in flight. A cancellation never authorizes a late mutation or retry.
    pub fn set_cancellation(&mut self, cancelled: Arc<AtomicBool>) {
        self.cancellation = Some(cancelled);
    }
}

fn validate_prompt(prompt: &str) -> Result<(), String> {
    if prompt.trim().is_empty() || prompt.len() > MAX_PROMPT_BYTES || prompt.contains('\0') {
        return Err("修改要求为空、过长或包含无效字符。".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assisted_prompt_rejects_empty_nul_and_oversize() {
        assert!(validate_prompt(" \n ").is_err());
        assert!(validate_prompt("hello\0world").is_err());
        assert!(validate_prompt(&"x".repeat(MAX_PROMPT_BYTES + 1)).is_err());
        assert!(validate_prompt("请修改按钮\nSelection ID: selection-1").is_ok());
    }

    #[cfg(not(windows))]
    #[test]
    fn unsupported_platform_never_offers_assisted_fill() {
        use crate::ipc::model::ProcessRef;
        let target = TargetInfo {
            id: "window-1".into(),
            title: "Agent".into(),
            app: "Agent".into(),
            process: ProcessRef {
                pid: 1,
                started_at: "1".into(),
            },
        };
        assert!(AssistedController::default()
            .inspect(1, &target, 2, 1)
            .is_err());
    }
}
