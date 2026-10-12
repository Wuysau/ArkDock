//! One command slot shared with Inspector. No native command has a request ID.
use super::{Pipe, NativePoint};
use super::control::{self, ControlMailbox, ControlRequest, RuntimeControlState, WireCommand};
use crate::ipc::model::{NativeControlAction, NativeControlIdentity, NativeControlPhase, NativeControlRequest, PreviewOrientation};
use crate::scheduler::BuildTask;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::sync::{mpsc, Mutex};
use std::time::{Duration, Instant};

struct Operation {
    request: ControlRequest,
    steps: VecDeque<WireCommand>,
    waiting: Option<(WireCommand, Instant)>,
    started: Instant,
    baseline_sequence: u64,
    baseline_hash: Option<[u8;32]>,
    baseline_route_revision: u64,
    accepted: bool,
    observing: bool,
    cancelled: bool,
}

struct HeldPointer { gesture_id: String, point: NativePoint, started: Instant }
struct Release { sent: Instant, request: Option<ControlRequest> }

pub(super) struct ControlWorker<'a> {
    state: &'a Mutex<RuntimeControlState>,
    mailbox: &'a Mutex<ControlMailbox>,
    task: &'a BuildTask,
    operation: Option<Operation>,
    held: Option<HeldPointer>,
    release: Option<Release>,
    held_key: Option<WireCommand>,
    cancel_reply: Option<ControlRequest>,
    cancel_pending: bool,
    legacy_release_at: Option<Instant>,
    sequence: u64,
    hash: Option<[u8;32]>,
    route_revision: u64,
}

impl<'a> ControlWorker<'a> {
    pub fn new(state: &'a Mutex<RuntimeControlState>, mailbox: &'a Mutex<ControlMailbox>, task: &'a BuildTask) -> Self {
        Self { state, mailbox, task, operation: None, held: None, release: None, held_key: None,
            cancel_reply: None, cancel_pending: false, legacy_release_at: None,
            sequence: 0, hash: None, route_revision: 0 }
    }

    pub fn busy(&self) -> bool { self.operation.is_some() || self.held.is_some() || self.release.is_some() || self.cancel_pending }

    pub fn route_message(&mut self, message: &Value) -> bool {
        if message.get("MessageType").and_then(Value::as_str) != Some("CurrentJsRouter") { return false; }
        let Some(route) = message.get("args").and_then(|args| args.get("CurrentRouter")).and_then(Value::as_str) else { return true; };
        if route.is_empty() || route.len() > 256 || route.chars().any(char::is_control) { return true; }
        self.route_revision = self.route_revision.saturating_add(1);
        if let Ok(mut state) = self.state.lock() { state.current_route = Some(route.to_owned()); }
        true
    }

    pub fn receive(&mut self, message: &Value) -> Result<bool, String> {
        if self.route_message(message) { return Ok(true); }
        let name = message.get("command").and_then(Value::as_str);
        if self.release.is_some() && name == Some("MouseRelease") {
            if message.get("version").and_then(Value::as_str) != Some("1.0.1")
                || message.get("result") != Some(&Value::Bool(true)) {
                return Err("NATIVE_SAFE_RELEASE_REJECTED: owned runtime must stop".into());
            }
            let release = self.release.take().expect("pending release");
            self.held = None;
            self.legacy_release_at = None;
            if !self.cancel_pending { self.resume_input("Native touch released; live input can resume"); }
            if let Some(request) = release.request {
                let mut receipt = request.receipt(NativeControlPhase::Cancelled, "Native touch was safely released");
                receipt.command_accepted = true;
                request.send(receipt);
            }
            return Ok(true);
        }
        let Some(operation) = self.operation.as_mut() else { return Ok(false); };
        let Some((waiting, _)) = operation.waiting.as_ref() else { return Ok(false); };
        if name != Some(waiting.name) { return Ok(false); }
        if message.get("version").and_then(Value::as_str) != Some("1.0.1")
            || message.get("result") != Some(&Value::Bool(true)) {
            let request = &operation.request;
            request.send(request.receipt(NativeControlPhase::Failed,
                "Installed Previewer rejected the experimental command; render completion is unconfirmed"));
            let shape = matches!(request.request.action, NativeControlAction::SetOrientation | NativeControlAction::SetResolution);
            self.operation = None;
            if self.held.is_some() || self.held_key.is_some() {
                self.cancel_pending = true;
                if let Ok(mut state) = self.state.lock() { state.input_ready = false; state.reason = "Rejected input requires safe release".into(); }
            } else if shape {
                return Err("NATIVE_CONFIGURATION_REJECTED: restart required for coherent surface".into());
            } else {
                self.resume_input("Command rejected; last valid native frame retained");
            }
            return Ok(true);
        }
        let completed = operation.waiting.take().expect("matched command").0;
        if completed.name == "MouseRelease" { self.held = None; self.legacy_release_at = None; }
        if completed.name == "KeyPress" && completed.key_release().is_none() { self.held_key = None; }
        if operation.cancelled { return Ok(true); }
        if operation.steps.is_empty() {
            operation.accepted = true;
            let mut receipt = operation.request.receipt(NativeControlPhase::CommandAccepted,
                "Native command acknowledged; application behavior still requires observation");
            receipt.command_accepted = true;
            if control::observes_frame(&operation.request.request.action) {
                operation.observing = true;
                operation.started = Instant::now();
                receipt.phase = NativeControlPhase::AwaitingFrame;
                receipt.reason = "Command acknowledged; waiting for a new native frame and applicable runtime evidence".into();
                operation.request.send(receipt);
            } else {
                operation.request.send(receipt);
                if operation.request.legacy_click { self.legacy_release_at = Some(Instant::now() + Duration::from_millis(80)); }
                self.operation = None;
            }
        }
        Ok(true)
    }

    pub fn tick(&mut self, pipe: &Pipe, inspector_busy: bool, legacy: &mpsc::Receiver<NativePoint>) -> Result<(), String> {
        let cancellation = self.mailbox.lock().map_err(|_| "NATIVE_CONTROL_MAILBOX")?.take_cancel();
        if let Some(reply) = cancellation {
            for _ in legacy.try_iter().take(16) {}
            if let Some(previous) = self.cancel_reply.take() {
                previous.send(previous.receipt(NativeControlPhase::Cancelled, "Cancellation superseded"));
            }
            self.cancel_reply = reply;
            self.cancel_pending = true;
            if let Ok(mut state) = self.state.lock() { state.input_ready = false; state.reason = "Safely cancelling native input".into(); }
            if let Some(operation) = self.operation.as_mut() { operation.cancelled = true; }
        }
        if let Some(release) = &self.release {
            if release.sent.elapsed() >= Duration::from_millis(control::CONTROL_TIMEOUT_MS) {
                if let Some(request) = &release.request {
                    request.send(request.receipt(NativeControlPhase::TimedOut, "Release acknowledgement timed out; owned runtime will stop"));
                }
                return Err("NATIVE_SAFE_RELEASE_TIMEOUT: owned runtime must stop".into());
            }
            return Ok(());
        }
        if self.held.as_ref().is_some_and(|held| held.started.elapsed() >= Duration::from_secs(15))
            || self.legacy_release_at.is_some_and(|due| Instant::now() >= due) { self.cancel_pending = true; }
        if let Some(operation) = &self.operation {
            if operation.waiting.as_ref().is_some_and(|(_, sent)| sent.elapsed() >= Duration::from_millis(control::CONTROL_TIMEOUT_MS)) {
                operation.request.send(operation.request.receipt(NativeControlPhase::TimedOut,
                    "Command acknowledgement timed out; correlation is unavailable until runtime restart"));
                // No request IDs: do not allow a late receipt to match another operation.
                return Err("NATIVE_CONTROL_CHANNEL_TIMEOUT: owned runtime must stop".into());
            }
            if operation.observing && operation.started.elapsed() >= Duration::from_millis(control::OBSERVATION_TIMEOUT_MS) {
                let mut receipt = operation.request.receipt(NativeControlPhase::TimedOut,
                    "Command accepted but required native frame/configuration evidence was not observed");
                receipt.command_accepted = true;
                operation.request.send(receipt);
                let shape = matches!(operation.request.request.action, NativeControlAction::SetOrientation | NativeControlAction::SetResolution);
                self.operation = None;
                if shape { return Err("NATIVE_SURFACE_OBSERVATION_TIMEOUT: restart required".into()); }
                self.resume_input("Native operation remains unconfirmed; last valid frame retained");
            }
            if self.operation.as_ref().is_some_and(|op| !op.observing && op.started.elapsed() >= Duration::from_secs(15)) {
                if let Some(op) = &self.operation { op.request.send(op.request.receipt(NativeControlPhase::TimedOut, "Native operation exceeded total 15 second bound")); }
                return Err("NATIVE_CONTROL_TOTAL_TIMEOUT".into());
            }
        }
        if inspector_busy || self.operation.as_ref().is_some_and(|op| op.waiting.is_some()) { return Ok(()); }
        if self.cancel_pending {
            if let Some(operation) = self.operation.take() {
                operation.request.send(operation.request.receipt(NativeControlPhase::Cancelled, "Remaining native operation cancelled"));
                if matches!(operation.request.request.action, NativeControlAction::SetOrientation | NativeControlAction::SetResolution) {
                    return Err("NATIVE_CONFIGURATION_CANCELLED: restart required after partial configuration".into());
                }
            }
            if let Some(key) = self.held_key.take() {
                pipe.control(&key)?;
                // Key release has no independent request ID. A cancelled partial key
                // transaction closes the runtime to prevent a late KeyPress receipt
                // from being attributed to later text input.
                return Err("NATIVE_KEY_TRANSACTION_CANCELLED: owned runtime stopped after release attempt".into());
            }
            self.cancel_pending = false;
            if let Some(held) = &self.held {
                pipe.control(&WireCommand::mouse("MouseRelease", held.point.x, held.point.y))?;
                self.release = Some(Release { sent: Instant::now(), request: self.cancel_reply.take() });
            } else if let Some(request) = self.cancel_reply.take() {
                request.send(request.receipt(NativeControlPhase::Cancelled, "No native touch remains pressed"));
                self.resume_input("Native input cancelled");
            } else { self.resume_input("Native input cancelled"); }
            return Ok(());
        }
        if self.operation.as_ref().is_some_and(|op| op.observing) { return Ok(()); }
        if self.operation.is_none() {
            let request = self.mailbox.lock().map_err(|_| "NATIVE_CONTROL_MAILBOX")?.pop();
            let request = request.or_else(|| if self.held.is_none() {
                legacy.try_recv().ok().map(|point| self.legacy_request(point))
            } else { None });
            if let Some(request) = request {
                if matches!(request.request.action, NativeControlAction::SetOrientation | NativeControlAction::SetResolution) {
                    // Legacy points contain no surface identity. They cannot be
                    // reinterpreted in a subsequently rotated/resized surface.
                    for _ in legacy.try_iter().take(16) {}
                }
                self.begin(request)?;
            }
        }
        let Some(operation) = self.operation.as_mut() else { return Ok(()); };
        if let Some(step) = operation.steps.pop_front() {
            // Remember presses before writing: an interrupted/failed write may
            // still have reached Previewer, so cleanup must attempt a release.
            if step.name == "MousePress" {
                self.held = Some(HeldPointer { gesture_id: operation.request.request.gesture_id.clone().unwrap_or_default(),
                    point: NativePoint { x: operation.request.request.x.unwrap_or(0), y: operation.request.request.y.unwrap_or(0) }, started: Instant::now() });
            } else if step.name == "MouseMove" {
                if let Some(held) = self.held.as_mut() { held.point = NativePoint {
                    x: operation.request.request.x.unwrap_or(held.point.x), y: operation.request.request.y.unwrap_or(held.point.y) }; }
            }
            if let Some(release) = step.key_release() { self.held_key = Some(release); }
            pipe.control(&step)?;
            operation.waiting = Some((step, Instant::now()));
        }
        Ok(())
    }

    fn begin(&mut self, request: ControlRequest) -> Result<(), String> {
        let state = self.state.lock().map_err(|_| "NATIVE_CONTROL_STATE")?.clone();
        if !control::identity_matches(&request.request.identity, self.task, &state) {
            request.send(request.receipt(NativeControlPhase::Cancelled, "Request belongs to an obsolete runtime or surface"));
            return Ok(());
        }
        let action = &request.request.action;
        if matches!(action, NativeControlAction::PointerMove | NativeControlAction::PointerUp)
            && !self.held.as_ref().is_some_and(|held| Some(&held.gesture_id) == request.request.gesture_id.as_ref()) {
            request.send(request.receipt(NativeControlPhase::Failed, "No matching native gesture is pressed")); return Ok(());
        }
        if self.held.is_some() && !matches!(action, NativeControlAction::PointerMove | NativeControlAction::PointerUp) {
            request.send(request.receipt(NativeControlPhase::Failed, "Finish or cancel the current gesture before another native operation")); return Ok(());
        }
        if *action == NativeControlAction::QueryRouter {
            let mut receipt = request.receipt(if state.current_route.is_some() { NativeControlPhase::Observed } else { NativeControlPhase::Blocked },
                "CurrentRouter is read from native CurrentJsRouter notifications; the public get path has no reliable reply");
            receipt.current_route = state.current_route;
            receipt.configuration_confirmed = receipt.current_route.is_some();
            request.send(receipt); return Ok(());
        }
        let steps = match control::commands(&request.request, &state) {
            Ok(steps) => steps,
            Err(error) => { request.send(request.receipt(NativeControlPhase::Blocked, error)); return Ok(()); }
        };
        if control::observes_frame(action) {
            if let Ok(mut state) = self.state.lock() { state.input_ready = false; state.reason = "Native configuration transition in progress".into(); }
        }
        self.operation = Some(Operation { request, steps, waiting: None, started: Instant::now(),
            baseline_sequence: self.sequence, baseline_hash: self.hash, baseline_route_revision: self.route_revision,
            accepted: false, observing: false, cancelled: false });
        Ok(())
    }

    fn legacy_request(&self, point: NativePoint) -> ControlRequest {
        let state = self.state.lock().map(|s| s.clone()).unwrap_or_default();
        let id = format!("legacy-{}", super::super::now_ms());
        let (reply, _) = mpsc::sync_channel(8);
        ControlRequest { request: NativeControlRequest { identity: NativeControlIdentity {
            workspace_id: self.task.workspace_id.clone(), session_id: self.task.session_id.clone(), build_id: self.task.build_id.clone(),
            generation: self.task.generation, width: state.width, height: state.height }, action: NativeControlAction::PointerDown,
            gesture_id: Some(id.clone()), x: Some(point.x), y: Some(point.y), text: None, key: None,
            orientation: None, color_mode: None, page: None, profile_id: None, width: None, height: None, dpi: None },
            operation_id: id, reply, requested_at: super::super::now_ms(), legacy_click: true }
    }

    pub fn surface(&self) -> (u32,u32) {
        let state = self.state.lock().map(|s| s.clone()).unwrap_or_default();
        if let Some(operation) = &self.operation {
            let request = &operation.request.request;
            if request.action == NativeControlAction::SetResolution { return (request.width.unwrap_or(state.width), request.height.unwrap_or(state.height)); }
            if request.action == NativeControlAction::SetOrientation {
                let (short,long) = (state.width.min(state.height),state.width.max(state.height));
                return if request.orientation == Some(PreviewOrientation::Portrait) { (short,long) } else { (long,short) };
            }
        }
        (state.width,state.height)
    }

    pub fn old_surface_packet(&self, width: u32, height: u32) -> bool {
        self.operation.as_ref().is_some_and(|op| matches!(op.request.request.action,
            NativeControlAction::SetOrientation | NativeControlAction::SetResolution))
            && self.state.lock().is_ok_and(|state| state.width == width && state.height == height)
    }

    /// Returns false while a shape change has not yet been acknowledged. The last
    /// displayed frame is retained; old-dimension packets never become new frames.
    pub fn frame(&mut self, width: u32, height: u32, sequence: u64, jpeg: &[u8]) -> Result<bool,String> {
        let hash: [u8;32] = Sha256::digest(jpeg).into();
        let mut completed = false;
        if let Some(operation) = self.operation.as_mut() {
            let action = &operation.request.request.action;
            if matches!(action, NativeControlAction::SetOrientation | NativeControlAction::SetResolution) && !operation.observing { return Ok(false); }
            if operation.observing && sequence > operation.baseline_sequence {
                let state = self.state.lock().map_err(|_| "NATIVE_CONTROL_STATE")?.clone();
                let router_confirmed = *action == NativeControlAction::NavigatePage
                    && self.route_revision > operation.baseline_route_revision
                    && state.current_route.as_deref() == operation.request.request.page.as_deref();
                if *action != NativeControlAction::NavigatePage || router_confirmed {
                    let mut receipt = operation.request.receipt(NativeControlPhase::Observed,
                        "A new native frame was observed; pixel change and configuration confirmation are reported separately");
                    receipt.command_accepted = operation.accepted;
                    receipt.frame_observed = true;
                    // Different JPEG bytes do not necessarily prove different
                    // decoded pixels. Preserve that distinction without decoding
                    // every image a second time in the control path.
                    receipt.pixels_changed = false;
                    receipt.configuration_confirmed = router_confirmed || *action == NativeControlAction::SetOrientation;
                    receipt.current_route = state.current_route;
                    receipt.frame_sequence = Some(sequence);
                    let payload_changed = operation.baseline_hash.is_some_and(|before| before != hash);
                    receipt.reason = format!("New native frame observed; JPEG payload changed={payload_changed}. Decoded pixel change is not independently measured.");
                    if *action == NativeControlAction::SetColorMode {
                        receipt.reason = "ColorMode acknowledged and a new frame observed; Previewer provides no public theme readback, so theme propagation still requires user observation".into();
                    } else if *action == NativeControlAction::SetResolution {
                        receipt.reason = format!("ResolutionSwitch acknowledged; native {width}x{height} frame observed. DPI propagation has no public readback and remains unconfirmed; JPEG payload changed={payload_changed}.");
                    }
                    operation.request.send(receipt);
                    let request = &operation.request.request;
                    if let Ok(mut state) = self.state.lock() {
                        state.width = width; state.height = height;
                        if matches!(action, NativeControlAction::SetOrientation | NativeControlAction::SetResolution) {
                            state.orientation = if width <= height { PreviewOrientation::Portrait } else { PreviewOrientation::Landscape };
                            if let Some(dpi) = request.dpi { state.dpi = dpi; }
                        }
                        if let Some(mode) = &request.color_mode { state.color_mode = mode.clone(); }
                    }
                    completed = true;
                }
            }
        }
        self.sequence = sequence; self.hash = Some(hash);
        if completed { self.operation = None; }
        if self.operation.is_none() && self.release.is_none() && !self.cancel_pending {
            self.resume_input("Live native frame; additional commands are experimental and await user acceptance");
        }
        Ok(true)
    }

    fn resume_input(&self, reason: &str) {
        if let Ok(mut state) = self.state.lock() { state.input_ready = self.sequence > 0; state.reason = reason.into(); }
    }

    pub fn cleanup(&mut self, pipe: &Pipe) {
        if let Some(held) = self.held.take() { let _ = pipe.control(&WireCommand::mouse("MouseRelease", held.point.x, held.point.y)); }
        if let Some(key) = self.held_key.take() { let _ = pipe.control(&key); }
        if let Some(operation) = self.operation.take() {
            operation.request.send(operation.request.receipt(NativeControlPhase::Cancelled, "Owned native runtime stopped; remaining operation cancelled"));
        }
        if let Some(request) = self.cancel_reply.take() { request.send(request.receipt(NativeControlPhase::Cancelled, "Owned native runtime stopped")); }
        if let Some(release) = self.release.take() {
            if let Some(request) = release.request { request.send(request.receipt(NativeControlPhase::Cancelled, "Owned native runtime stopped during release")); }
        }
    }
}
