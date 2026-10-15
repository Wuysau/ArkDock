//! Typed, bounded requests for the owned native Previewer command pipe.
//!
//! Wire fields are based on OpenHarmony ide_previewer commit
//! 8b86305d7853d96aee92aeccab2c2daafa0d2f5b (Apache-2.0).
//! These additional commands remain experimental until the installed SDK is
//! exercised by the user. A dispatch receipt is not a render completion proof.
use crate::ipc::model::{
    NativeControlAction, NativeControlIdentity, NativeControlReceipt, NativeControlRequest,
    PreviewColorMode, PreviewOrientation,
};
#[cfg(windows)]
use crate::ipc::model::{NativeControlPhase, PreviewKey};
use crate::scheduler::BuildTask;
#[cfg(windows)]
use serde_json::{json, Value};
#[cfg(windows)]
use std::collections::VecDeque;
use std::sync::mpsc;

pub const CONTROL_TIMEOUT_MS: u64 = 2_000;
pub const OBSERVATION_TIMEOUT_MS: u64 = 5_000;
pub const CONTROL_QUEUE_LIMIT: usize = 16;
pub const CONTROL_WIRE_LIMIT: usize = 4_096;
pub type ControlReceiver = mpsc::Receiver<Result<NativeControlReceipt, String>>;

#[derive(Clone, Debug)]
pub struct RuntimeControlState {
    pub orientation: PreviewOrientation,
    pub color_mode: PreviewColorMode,
    pub width: u32,
    pub height: u32,
    pub dpi: u32,
    pub current_route: Option<String>,
    pub input_ready: bool,
    pub reason: String,
}

impl Default for RuntimeControlState {
    fn default() -> Self {
        Self {
            orientation: PreviewOrientation::Portrait,
            color_mode: PreviewColorMode::Light,
            width: 360,
            height: 640,
            dpi: 160,
            current_route: None,
            input_ready: false,
            reason: "等待原生运行时返回首帧。".into(),
        }
    }
}

impl RuntimeControlState {
    pub fn stopped() -> Self {
        Self {
            reason: "尚未启动原生预览会话。".into(),
            ..Self::default()
        }
    }
}

#[cfg(windows)]
pub(crate) struct ControlRequest {
    pub request: NativeControlRequest,
    pub operation_id: String,
    pub reply: mpsc::SyncSender<Result<NativeControlReceipt, String>>,
    pub requested_at: u64,
    pub legacy_click: bool,
}

#[cfg(windows)]
impl ControlRequest {
    pub fn receipt(
        &self,
        phase: NativeControlPhase,
        reason: impl Into<String>,
    ) -> NativeControlReceipt {
        NativeControlReceipt {
            operation_id: self.operation_id.clone(),
            identity: self.request.identity.clone(),
            action: self.request.action.clone(),
            phase,
            command_accepted: false,
            frame_observed: false,
            pixels_changed: false,
            configuration_confirmed: false,
            current_route: None,
            frame_sequence: None,
            requested_at: self.requested_at,
            observed_at: super::now_ms(),
            reason: reason.into(),
        }
    }

    pub fn send(&self, receipt: NativeControlReceipt) {
        let _ = self.reply.try_send(Ok(receipt));
    }
}

/// Move replacement applies only to adjacent moves for the same gesture. Down,
/// Up, text and configuration operations are never dropped to make queue space.
#[derive(Default)]
#[cfg(windows)]
pub(crate) struct ControlMailbox {
    queue: VecDeque<ControlRequest>,
    cancel: Option<ControlRequest>,
    cancel_requested: bool,
    cancel_all: bool,
}

#[cfg(windows)]
impl ControlMailbox {
    pub fn enqueue(&mut self, request: ControlRequest) -> Result<(), String> {
        if matches!(
            request.request.action,
            NativeControlAction::PointerCancel | NativeControlAction::CancelAll
        ) {
            self.cancel_requested = true;
            self.cancel_all |= request.request.action == NativeControlAction::CancelAll;
            if let Some(old) = self.cancel.replace(request) {
                old.send(old.receipt(
                    NativeControlPhase::Cancelled,
                    "取消请求已被更新的请求取代。",
                ));
            }
            self.cancel_queued(self.cancel_all);
            return Ok(());
        }
        if self.cancel_requested && (self.cancel_all || is_input_action(&request.request.action)) {
            return Err("NATIVE_CONTROL_CANCELLING".into());
        }
        if request.request.action == NativeControlAction::PointerMove {
            let replace = self.queue.back().is_some_and(|old| {
                old.request.action == NativeControlAction::PointerMove
                    && old.request.gesture_id == request.request.gesture_id
                    && old.request.identity == request.request.identity
            });
            if replace {
                if let Some(old) = self.queue.pop_back() {
                    old.send(
                        old.receipt(NativeControlPhase::Cancelled, "移动已合并到更新的位置。"),
                    );
                }
            }
        }
        if self.queue.len() >= CONTROL_QUEUE_LIMIT {
            return Err("NATIVE_CONTROL_QUEUE_BUSY".into());
        }
        self.queue.push_back(request);
        Ok(())
    }

    pub fn pop(&mut self) -> Option<ControlRequest> {
        self.queue.pop_front()
    }

    pub fn cancel_all(&mut self) {
        self.cancel_requested = true;
        self.cancel_all = true;
        self.cancel_queued(true);
    }

    pub fn take_cancel(&mut self) -> Option<(bool, Option<ControlRequest>)> {
        if !self.cancel_requested {
            return None;
        }
        self.cancel_requested = false;
        let all = std::mem::take(&mut self.cancel_all);
        Some((all, self.cancel.take()))
    }

    fn cancel_queued(&mut self, all: bool) {
        let mut retained = VecDeque::new();
        for request in self.queue.drain(..) {
            if all || is_input_action(&request.request.action) {
                request.send(
                    request.receipt(NativeControlPhase::Cancelled, "原生操作在发送前已取消。"),
                );
            } else {
                retained.push_back(request);
            }
        }
        self.queue = retained;
    }
}

/// No frontend string is used as a command name or type.
#[derive(Clone, Debug)]
#[cfg(windows)]
pub(crate) struct WireCommand {
    pub name: &'static str,
    kind: &'static str,
    args: Value,
}

#[cfg(windows)]
impl WireCommand {
    pub fn mouse(name: &'static str, x: u32, y: u32) -> Self {
        Self {
            name,
            kind: "action",
            args: json!({ "x": x, "y": y }),
        }
    }

    pub fn bytes(&self) -> Result<Vec<u8>, String> {
        let mut bytes = serde_json::to_vec(&json!({
            "version": "1.0.1", "type": self.kind, "command": self.name, "args": self.args,
        }))
        .map_err(|_| "NATIVE_CONTROL_ENCODING")?;
        if bytes.len() >= CONTROL_WIRE_LIMIT {
            return Err("NATIVE_CONTROL_MESSAGE_LIMIT".into());
        }
        bytes.push(0);
        Ok(bytes)
    }

    pub fn key_release(&self) -> Option<Self> {
        if self.name != "KeyPress"
            || self.args.get("isInputMethod") != Some(&Value::Bool(false))
            || self.args.get("keyAction").and_then(Value::as_i64) != Some(0)
        {
            return None;
        }
        let mut release = self.clone();
        release.args["keyAction"] = json!(1);
        Some(release)
    }
}

pub fn validate_request(request: &NativeControlRequest) -> Result<(), String> {
    let identity = &request.identity;
    if [
        &identity.workspace_id,
        &identity.session_id,
        &identity.build_id,
    ]
    .iter()
    .any(|id| id.is_empty() || id.len() > 256 || id.chars().any(char::is_control))
        || identity.generation == 0
        || !(50..=3000).contains(&identity.width)
        || !(50..=3000).contains(&identity.height)
        || u64::from(identity.width) * u64::from(identity.height)
            > super::frames::MAX_SURFACE_PIXELS
    {
        return Err("NATIVE_CONTROL_IDENTITY_INVALID".into());
    }
    let action = &request.action;
    let pointer = matches!(
        action,
        NativeControlAction::PointerDown
            | NativeControlAction::PointerMove
            | NativeControlAction::PointerUp
    );
    let cancel = matches!(
        action,
        NativeControlAction::PointerCancel | NativeControlAction::CancelAll
    );
    if (pointer || cancel)
        && request.gesture_id.as_ref().is_some_and(|id| {
            id.is_empty()
                || id.len() > 128
                || !id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
        })
    {
        return Err("NATIVE_GESTURE_ID_INVALID".into());
    }
    if pointer
        && (request.gesture_id.is_none()
            || request.x.is_none()
            || request.y.is_none()
            || request.x.unwrap_or(u32::MAX) >= identity.width
            || request.y.unwrap_or(u32::MAX) >= identity.height)
    {
        return Err("INPUT_OUTSIDE_SURFACE".into());
    }
    if !pointer && (request.x.is_some() || request.y.is_some()) {
        return Err("NATIVE_CONTROL_UNUSED_COORDINATES".into());
    }
    if !pointer && !cancel && request.gesture_id.is_some() {
        return Err("NATIVE_CONTROL_UNUSED_GESTURE".into());
    }
    if request.text.is_some() != (*action == NativeControlAction::TextCommit)
        || request.key.is_some() != (*action == NativeControlAction::KeyPress)
        || request.orientation.is_some() != (*action == NativeControlAction::SetOrientation)
        || request.color_mode.is_some() != (*action == NativeControlAction::SetColorMode)
        || request.page.is_some() != (*action == NativeControlAction::NavigatePage)
        || request.profile_id.is_some() != (*action == NativeControlAction::SwitchProfile)
    {
        return Err("NATIVE_CONTROL_ARGUMENT_MISMATCH".into());
    }
    let resize = *action == NativeControlAction::SetResolution;
    if request.width.is_some() != resize
        || request.height.is_some() != resize
        || request.dpi.is_some() != resize
    {
        return Err("NATIVE_CONTROL_RESOLUTION_ARGUMENT_MISMATCH".into());
    }
    if resize
        && (!(50..=3000).contains(&request.width.unwrap_or(0))
            || !(50..=3000).contains(&request.height.unwrap_or(0))
            || u64::from(request.width.unwrap_or(0)) * u64::from(request.height.unwrap_or(0))
                > super::frames::MAX_SURFACE_PIXELS
            || !(120..=640).contains(&request.dpi.unwrap_or(0)))
    {
        return Err("NATIVE_CONTROL_RESOLUTION_LIMIT".into());
    }
    if let Some(text) = &request.text {
        if text.is_empty()
            || text.len() > 1024
            || text.chars().count() > 128
            || text.chars().any(|c| c == '\0' || c == '\r' || c == '\n')
        {
            return Err(
                "NATIVE_TEXT_LIMIT: 一次最多提交 128 个字符；换行请使用 Enter 按键。".into(),
            );
        }
    }
    if let Some(page) = &request.page {
        if page.is_empty()
            || page.len() > 256
            || page.starts_with('/')
            || page.contains("..")
            || !page
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'_' | b'-'))
        {
            return Err("NATIVE_PAGE_INVALID".into());
        }
    }
    Ok(())
}

pub fn identity_matches(
    identity: &NativeControlIdentity,
    task: &BuildTask,
    state: &RuntimeControlState,
) -> bool {
    identity.workspace_id == task.workspace_id
        && identity.session_id == task.session_id
        && identity.build_id == task.build_id
        && identity.generation == task.generation
        && identity.width == state.width
        && identity.height == state.height
}

#[cfg(windows)]
pub(crate) fn commands(
    request: &NativeControlRequest,
    state: &RuntimeControlState,
) -> Result<VecDeque<WireCommand>, String> {
    validate_request(request)?;
    let mut steps = VecDeque::new();
    let set = |name, args| WireCommand {
        name,
        kind: "set",
        args,
    };
    match request.action {
        NativeControlAction::PointerDown
        | NativeControlAction::PointerMove
        | NativeControlAction::PointerUp => {
            let name = match request.action {
                NativeControlAction::PointerDown => "MousePress",
                NativeControlAction::PointerMove => "MouseMove",
                _ => "MouseRelease",
            };
            steps.push_back(WireCommand::mouse(
                name,
                request.x.unwrap_or(0),
                request.y.unwrap_or(0),
            ));
        }
        NativeControlAction::TextCommit => {
            for scalar in request.text.as_deref().unwrap_or("").chars() {
                steps.push_back(WireCommand {
                    name: "KeyPress",
                    kind: "action",
                    args: json!({
                        "isInputMethod": true, "codePoint": u32::from(scalar),
                    }),
                });
            }
        }
        NativeControlAction::KeyPress => {
            let code = match request.key.as_ref().ok_or("NATIVE_KEY_REQUIRED")? {
                PreviewKey::ArrowUp => 2012,
                PreviewKey::ArrowDown => 2013,
                PreviewKey::ArrowLeft => 2014,
                PreviewKey::ArrowRight => 2015,
                PreviewKey::Tab => 2049,
                PreviewKey::Enter => 2054,
                PreviewKey::Backspace => 2055,
                PreviewKey::Escape => 2070,
                PreviewKey::Delete => 2071,
                PreviewKey::Home => 2081,
                PreviewKey::End => 2082,
            };
            for action in [0, 1] {
                steps.push_back(WireCommand {
                    name: "KeyPress",
                    kind: "action",
                    args: json!({
                        "isInputMethod": false, "keyCode": code, "keyAction": action,
                        "pressedCodes": [code], "keyString": "",
                    }),
                });
            }
        }
        NativeControlAction::SetColorMode => {
            let mode = if request.color_mode == Some(PreviewColorMode::Dark) {
                "dark"
            } else {
                "light"
            };
            steps.push_back(set("ColorMode", json!({ "ColorMode": mode })));
        }
        NativeControlAction::SetOrientation => {
            let portrait = request.orientation == Some(PreviewOrientation::Portrait);
            let (short, long) = (state.width.min(state.height), state.width.max(state.height));
            let (width, height) = if portrait {
                (short, long)
            } else {
                (long, short)
            };
            steps.push_back(set(
                "Orientation",
                json!({ "Orientation": if portrait { "portrait" } else { "landscape" } }),
            ));
            steps.push_back(set("ResolutionSwitch", json!({ "originWidth": width, "originHeight": height,
                "width": width, "height": height, "screenDensity": state.dpi, "reason": "rotation" })));
        }
        NativeControlAction::SetResolution => {
            steps.push_back(set(
                "ResolutionSwitch",
                json!({ "originWidth": request.width,
                "originHeight": request.height, "width": request.width, "height": request.height,
                "screenDensity": request.dpi, "reason": "resize" }),
            ));
        }
        NativeControlAction::NavigatePage => {
            steps.push_back(set(
                "ReloadRuntimePage",
                json!({ "ReloadRuntimePage": request.page }),
            ));
        }
        NativeControlAction::QueryRouter
        | NativeControlAction::PointerCancel
        | NativeControlAction::CancelAll => {}
        NativeControlAction::SwitchProfile => {
            return Err("DEVICE_PROFILE_REQUIRES_CONTROLLED_RESTART".into())
        }
    }
    Ok(steps)
}

#[cfg(windows)]
pub(crate) fn observes_frame(action: &NativeControlAction) -> bool {
    matches!(
        action,
        NativeControlAction::SetOrientation
            | NativeControlAction::SetResolution
            | NativeControlAction::SetColorMode
            | NativeControlAction::NavigatePage
    )
}

/// Focus and modal changes cancel input without discarding a configuration
/// observation that was already explicitly requested by the user.
pub fn is_input_action(action: &NativeControlAction) -> bool {
    matches!(
        action,
        NativeControlAction::PointerDown
            | NativeControlAction::PointerMove
            | NativeControlAction::PointerUp
            | NativeControlAction::TextCommit
            | NativeControlAction::KeyPress
    )
}
