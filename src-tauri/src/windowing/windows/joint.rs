//! Joint geometry for two independent top-level windows. No ownership/parent changes.
use super::super::geometry::{maximized_pair, pair_at_target, resize_pair, PairRects};
use super::*;
use windows_sys::Win32::Graphics::Gdi::ClientToScreen;

#[derive(Clone)]
struct PendingPair {
    before: PairRects,
    expected: PairRects,
    state: PairLayoutState,
    deadline: Instant,
    issued: bool,
}
#[derive(Clone)]
struct Gesture {
    target: bool,
    base: PairRects,
    last: Rect,
    commanded: Option<PairRects>,
}
pub(super) struct JointGroup {
    original: PairRects,
    original_target: WINDOWPLACEMENT,
    original_own: WINDOWPLACEMENT,
    normal_target: WINDOWPLACEMENT,
    normal_own: WINDOWPLACEMENT,
    normal: PairRects,
    current: PairRects,
    pending: Option<PendingPair>,
    gesture: Option<Gesture>,
    user_changed: bool,
    state: PairLayoutState,
    preferred_width_dip: u32,
    compensating: bool,
    normal_unobserved: bool,
    native_maximized: bool,
    normalizing_native_max: bool,
    normalizing_own_max: bool,
    suspended: bool,
    release_pending: Option<(Rect, Rect)>,
    resume_pending: bool,
}
pub(super) enum JointCompensation {
    Waiting,
    Restore(WINDOWPLACEMENT, WINDOWPLACEMENT, PairRects),
    Finished,
}

fn client_size(hwnd: HWND) -> Option<(i32, i32)> {
    let mut rect: RECT = unsafe { zeroed() };
    (unsafe { GetClientRect(hwnd, &mut rect) } != 0).then_some((rect.right, rect.bottom))
}
fn observed_pair(target: HWND, own: HWND) -> Option<PairRects> {
    Some(PairRects {
        target: coherent_visible(target)?,
        dock: coherent_visible(own)?,
    })
}
fn coherent_visible(hwnd: HWND) -> Option<Rect> {
    let first = outer_bounds(hwnd)?;
    let frame = visible_bounds(hwnd)?;
    let size = client_size(hwnd)?;
    let mut origin = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };
    if unsafe { ClientToScreen(hwnd, &mut origin) } == 0 {
        return None;
    }
    let second = outer_bounds(hwnd)?;
    // DWM and User32 can report different frames during Windows maximize animation.
    // Wait for a coherent sample instead of deriving large fictitious frame insets.
    if first != second
        || origin.x < frame.left
        || origin.y < frame.top
        || origin.x.checked_add(size.0)? > frame.right
        || origin.y.checked_add(size.1)? > frame.bottom
    {
        return None;
    }
    Some(frame)
}
fn normal_matches(a: WINDOWPLACEMENT, b: WINDOWPLACEMENT) -> bool {
    same_normal_placement(a, b)
}
fn joined(pair: PairRects) -> bool {
    pair.target.right == pair.dock.left
        && pair.target.top == pair.dock.top
        && pair.target.bottom == pair.dock.bottom
}
fn can_save_normal_pair(
    actual: PairRects,
    confirmed: PairRects,
    target_zoomed: bool,
    own_zoomed: bool,
) -> bool {
    !target_zoomed && !own_zoomed && actual == confirmed && joined(actual)
}
fn fills_workarea(pair: PairRects, work: Rect) -> bool {
    pair.target.left == work.left
        && pair.target.top == work.top
        && pair.target.bottom == work.bottom
        && pair.target.right == pair.dock.left
        && pair.dock.right == work.right
        && pair.dock.top == work.top
        && pair.dock.bottom == work.bottom
}

fn owns_reserved_area(
    actual: PairRects,
    confirmed: PairRects,
    pending: bool,
    gesture: bool,
) -> bool {
    actual == confirmed && joined(actual) && !pending && !gesture
}

impl Native {
    pub(super) fn suspend_joint(&mut self) {
        if self.joint.as_ref().is_some_and(|group| group.suspended) {
            return;
        }
        let Some(target) = self.target.as_ref() else {
            return;
        };
        if !self.valid(target) || !self.valid(&self.own) {
            return;
        }
        let hwnd = target.identity.handle as HWND;
        let own = self.own.identity.handle as HWND;
        let actual = observed_pair(hwnd, own);
        let release = self.joint.as_ref().and_then(|group| {
            let actual = actual?;
            if !owns_reserved_area(
                actual,
                group.current,
                group.pending.is_some(),
                group.gesture.is_some(),
            ) || unsafe { IsIconic(hwnd) } != 0
                || !visible(hwnd)
            {
                return None;
            }
            Some((
                actual.target,
                Rect::new(
                    actual.target.left,
                    actual.target.top,
                    actual.dock.right,
                    actual.target.bottom,
                )?,
            ))
        });
        if let Some(group) = &mut self.joint {
            group.suspended = true;
            group.resume_pending = false;
            group.gesture = None;
            // An unacknowledged layout keeps its recorded ownership. Do not issue a
            // compensating target write while independent accepted requests may be late.
            group.user_changed = true;
        }
        self.pair_status.reason = Some("悬浮球模式暂停窗口联动；目标的新布局由用户保留".into());
        if let Some((before, desired)) = release {
            let requested = outer_bounds(hwnd)
                .zip(visible_bounds(hwnd))
                .and_then(|(outer, visible)| outer_for_visible(outer, visible, desired));
            if let Some(requested) = requested {
                if self.valid(target) && position(hwnd, requested).is_ok() {
                    if let Some(group) = &mut self.joint {
                        group.release_pending = Some((before, desired));
                    }
                }
            }
        }
    }

    pub(super) fn observe_suspended_joint(&mut self) {
        let actual = self.target.as_ref().and_then(|target| {
            observed_pair(
                target.identity.handle as HWND,
                self.own.identity.handle as HWND,
            )
        });
        let current = self
            .target
            .as_ref()
            .and_then(|target| coherent_visible(target.identity.handle as HWND));
        if let (Some(group), Some(current)) = (&mut self.joint, current) {
            if let Some((before, expected)) = group.release_pending {
                if current == expected || current != before {
                    // Exact release acknowledgment or a new user layout ends our transaction.
                    group.release_pending = None;
                }
            }
            if let (Some(pending), Some(actual)) = (&group.pending, actual) {
                let target_known = actual.target == pending.before.target
                    || actual.target == pending.expected.target;
                let dock_known =
                    actual.dock == pending.before.dock || actual.dock == pending.expected.dock;
                if actual == pending.expected || !pending.issued || !target_known || !dock_known {
                    group.pending = None;
                }
            }
        }
    }

    pub(super) fn resume_joint(&mut self) {
        if let Some(group) = &mut self.joint {
            group.resume_pending = true;
        }
        self.observe_suspended_joint();
        self.try_resume_joint();
    }

    fn try_resume_joint(&mut self) {
        if self.user_surface_hidden {
            return;
        }
        let ready = self.joint.as_ref().is_some_and(|group| {
            group.suspended
                && group.resume_pending
                && group.release_pending.is_none()
                && group.pending.is_none()
        });
        if !ready {
            return;
        }
        let Some(target) = self.target.as_ref() else {
            return;
        };
        let hwnd = target.identity.handle as HWND;
        if !self.valid(target) || !self.valid(&self.own) {
            self.invalidate("Target lifecycle changed before joint expansion");
            return;
        }
        if !eligible(hwnd, self.own_pid) || unsafe { IsIconic(hwnd) } != 0 {
            return;
        }
        // Rebuild from the live target geometry / current work area and DPI. No old
        // pre-collapse rectangle or released reservation is replayed over the user.
        if let Err(error) = self.establish_joint() {
            self.pair_status.state = PairLayoutState::Blocked;
            self.pair_status.reason = Some(error);
        }
    }
    fn joint_limits(&self) -> Option<(i32, i32, i32, i32)> {
        let hwnd = self.own.identity.handle as HWND;
        let visible = visible_bounds(hwnd)?;
        let client = client_size(hwnd)?;
        let extra_x = visible.width().checked_sub(client.0)?.max(0);
        let extra_y = visible.height().checked_sub(client.1)?.max(0);
        Some((
            scaled(hwnd, 320),
            scaled(hwnd, 360) + extra_x,
            scaled(hwnd, 560) + extra_x,
            scaled(hwnd, 640) + extra_y,
        ))
    }
    fn preferred_physical_width(&self, dip: u32) -> Option<i32> {
        let hwnd = self.own.identity.handle as HWND;
        let visible = visible_bounds(hwnd)?;
        let client = client_size(hwnd)?;
        Some(scaled(hwnd, dip as i32) + (visible.width() - client.0).max(0))
    }
    fn joint_position(&self, expected: PairRects) -> Result<(), String> {
        let target = self.target.as_ref().ok_or("TARGET_REQUIRED")?;
        if !self.valid(target) || !self.valid(&self.own) {
            return Err("STALE_TARGET: joint layout lifecycle changed".into());
        }
        // DWM visible bounds are converted separately for each window/DPI. No shared border guess.
        for (window, desired) in [(target, expected.target), (&self.own, expected.dock)] {
            let hwnd = window.identity.handle as HWND;
            let outer = outer_bounds(hwnd).ok_or("WINDOW_BOUNDS_UNAVAILABLE")?;
            let visible = visible_bounds(hwnd).ok_or("VISIBLE_BOUNDS_UNAVAILABLE")?;
            if visible != desired {
                let desired_outer =
                    outer_for_visible(outer, visible, desired).ok_or("WINDOW_FRAME_UNAVAILABLE")?;
                if !self.valid(window) {
                    return Err("STALE_TARGET: joint positioning cancelled".into());
                }
                position(hwnd, desired_outer)?;
            }
        }
        Ok(())
    }
    fn request_joint_position(
        &mut self,
        expected: PairRects,
        state: PairLayoutState,
    ) -> Result<(), String> {
        let target = self.target.as_ref().ok_or("TARGET_REQUIRED")?;
        let before = observed_pair(
            target.identity.handle as HWND,
            self.own.identity.handle as HWND,
        )
        .ok_or("WINDOW_BOUNDS_UNAVAILABLE")?;
        let group = self.joint.as_mut().ok_or("PAIR_REQUIRED")?;
        group.pending = Some(PendingPair {
            before,
            expected,
            state: state.clone(),
            deadline: Instant::now() + Duration::from_secs(2),
            issued: true,
        });
        self.pair_status.state = match state {
            PairLayoutState::Maximized => PairLayoutState::Maximizing,
            PairLayoutState::Paired => PairLayoutState::Restoring,
            _ => state,
        };
        self.pair_status.reason = Some("等待 Windows 确认两个窗口的实际边界".into());
        // Record ownership before either independent asynchronous operation can be accepted.
        self.joint_position(expected)?;
        Ok(())
    }
    pub(super) fn establish_joint(&mut self) -> Result<(), String> {
        let target = self.target.as_ref().ok_or("TARGET_REQUIRED")?;
        let target_hwnd = target.identity.handle as HWND;
        let own_hwnd = self.own.identity.handle as HWND;
        if !self.valid(target) || !self.valid(&self.own) {
            return Err("STALE_TARGET".into());
        }
        let actual = observed_pair(target_hwnd, own_hwnd).ok_or("WINDOW_BOUNDS_UNAVAILABLE")?;
        let work = work_area(target_hwnd).ok_or("WORK_AREA_UNAVAILABLE")?;
        let limits = self.joint_limits().ok_or("WINDOW_LIMITS_UNAVAILABLE")?;
        let preferred = self
            .preferred_physical_width(440)
            .ok_or("WINDOW_FRAME_UNAVAILABLE")?;
        let already_maximized = unsafe { IsZoomed(target_hwnd) } != 0;
        let expected = if already_maximized {
            maximized_pair(work, preferred, limits.0, limits.1, limits.2, limits.3)
        } else {
            pair_at_target(
                actual.target,
                preferred,
                work,
                limits.0,
                limits.1,
                limits.2,
                limits.3,
            )
        }
        .ok_or("WORK_AREA_TOO_SMALL: two windows cannot fit their minimum dimensions")?;
        let original_target = placement(target_hwnd).ok_or("TARGET_PLACEMENT_UNAVAILABLE")?;
        let original_own = placement(own_hwnd).ok_or("OWN_PLACEMENT_UNAVAILABLE")?;
        self.joint = Some(JointGroup {
            original: actual,
            original_target,
            original_own,
            normal_target: WINDOWPLACEMENT {
                showCmd: SW_SHOWNOACTIVATE as u32,
                ..original_target
            },
            normal_own: original_own,
            normal: actual,
            current: actual,
            pending: None,
            gesture: None,
            user_changed: false,
            state: PairLayoutState::Paired,
            preferred_width_dip: 440,
            compensating: false,
            normal_unobserved: already_maximized,
            native_maximized: already_maximized,
            normalizing_native_max: false,
            normalizing_own_max: false,
            suspended: false,
            release_pending: None,
            resume_pending: false,
        });
        self.pair_status = PairLayoutStatus {
            state: PairLayoutState::Restoring,
            preferred_width_dip: 440,
            reason: Some("正在对齐两个独立窗口的可见边界".into()),
        };
        self.layout_required = false;
        self.request_joint_position(
            expected,
            if already_maximized {
                PairLayoutState::Maximized
            } else {
                PairLayoutState::Paired
            },
        )
    }
    pub(super) fn joint_gesture_active(&self) -> bool {
        self.joint.as_ref().is_some_and(|g| g.gesture.is_some())
    }
    pub(super) fn begin_joint_gesture(&mut self, own: bool) {
        if self.user_surface_hidden || self.joint.as_ref().is_some_and(|group| group.suspended) {
            if let Some(group) = &mut self.joint {
                group.release_pending = None;
                group.pending = None;
                group.user_changed = true;
            }
            return;
        }
        let Some(target) = self.target.as_ref() else {
            // A resize without begin_drag never arms detection or chooses a target.
            self.dragging = false;
            self.move_started = false;
            self.armed_at = None;
            self.candidate = None;
            return;
        };
        let Some(actual) = observed_pair(
            target.identity.handle as HWND,
            self.own.identity.handle as HWND,
        ) else {
            return;
        };
        if let Some(group) = &mut self.joint {
            let base = if actual.target.right == actual.dock.left
                && actual.target.top == actual.dock.top
                && actual.target.bottom == actual.dock.bottom
            {
                actual
            } else {
                group.current
            };
            group.pending = None;
            group.user_changed = true;
            group.gesture = Some(Gesture {
                target: !own,
                base,
                last: if own { actual.dock } else { actual.target },
                commanded: None,
            });
        }
        if self
            .joint
            .as_ref()
            .is_some_and(|group| group.state == PairLayoutState::Maximized)
            && unsafe { IsZoomed(target.identity.handle as HWND) } != 0
        {
            if self.valid(target) {
                unsafe {
                    ShowWindowAsync(target.identity.handle as HWND, SW_SHOWNOACTIVATE);
                }
            }
            if let Some(group) = &mut self.joint {
                group.native_maximized = false;
            }
        }
    }
    pub(super) fn end_joint_gesture(&mut self) {
        if self.user_surface_hidden {
            return;
        }
        self.follow_joint();
        let actual = self.target.as_ref().and_then(|target| {
            observed_pair(
                target.identity.handle as HWND,
                self.own.identity.handle as HWND,
            )
        });
        let expected = self
            .joint
            .as_ref()
            .and_then(|group| group.gesture.as_ref().and_then(|gesture| gesture.commanded));
        let final_state = expected.and_then(|pair| {
            self.target
                .as_ref()
                .and_then(|target| work_area(target.identity.handle as HWND))
                .map(|work| {
                    if fills_workarea(pair, work) {
                        PairLayoutState::Maximized
                    } else {
                        PairLayoutState::Paired
                    }
                })
        });
        if let Some(group) = &mut self.joint {
            group.gesture = None;
            if let (Some(actual), Some(expected)) = (actual, expected) {
                if actual != expected {
                    group.pending = Some(PendingPair {
                        before: actual,
                        expected,
                        state: final_state.clone().unwrap_or(PairLayoutState::Paired),
                        deadline: Instant::now() + Duration::from_secs(2),
                        issued: true,
                    });
                }
            }
        }
        if let (Some(actual), Some(expected)) = (actual, expected) {
            if actual != expected {
                let _ = self.joint_position(expected);
            }
        }
    }
    pub(super) fn maximize_joint(&mut self) -> Result<(), String> {
        if self.cancelled.is_some() {
            return Err(
                "LAYOUT_CANCELLATION_PENDING: wait for accepted consent requests to settle".into(),
            );
        }
        if self.user_surface_hidden || self.joint.as_ref().is_some_and(|group| group.suspended) {
            return Err("DISPLAY_SUSPENDED: joint layout waits for expansion".into());
        }
        if self.dragging {
            return Err("FINISH_DRAG_FIRST".into());
        }
        if self.joint.is_none() {
            self.establish_joint()?;
        }
        let target = self.target.as_ref().ok_or("TARGET_REQUIRED")?;
        if !self.valid(target) {
            return Err("STALE_TARGET".into());
        }
        let hwnd = target.identity.handle as HWND;
        let work = work_area(hwnd).ok_or("WORK_AREA_UNAVAILABLE")?;
        let limits = self.joint_limits().ok_or("WINDOW_LIMITS_UNAVAILABLE")?;
        let preferred = self.joint.as_ref().map_or(440, |g| g.preferred_width_dip);
        let expected = maximized_pair(
            work,
            self.preferred_physical_width(preferred)
                .ok_or("WINDOW_FRAME_UNAVAILABLE")?,
            limits.0,
            limits.1,
            limits.2,
            limits.3,
        )
        .ok_or("WORK_AREA_TOO_SMALL")?;
        if let Some(group) = &mut self.joint {
            if group.state != PairLayoutState::Maximized {
                if let Some(actual) = observed_pair(hwnd, self.own.identity.handle as HWND) {
                    if can_save_normal_pair(
                        actual,
                        group.current,
                        unsafe { IsZoomed(hwnd) } != 0,
                        unsafe { IsZoomed(self.own.identity.handle as HWND) } != 0,
                    ) {
                        group.normal = actual;
                        group.normal_target =
                            placement(hwnd).ok_or("TARGET_PLACEMENT_UNAVAILABLE")?;
                        group.normal_own = placement(self.own.identity.handle as HWND)
                            .ok_or("OWN_PLACEMENT_UNAVAILABLE")?;
                    }
                }
            }
        }
        // Keep the target's native maximize style. Its OS restore button remains meaningful.
        let actual = observed_pair(hwnd, self.own.identity.handle as HWND)
            .ok_or("WINDOW_BOUNDS_UNAVAILABLE")?;
        let group = self.joint.as_mut().ok_or("PAIR_REQUIRED")?;
        group.pending = Some(PendingPair {
            before: actual,
            expected,
            state: PairLayoutState::Maximized,
            deadline: Instant::now() + Duration::from_secs(2),
            issued: false,
        });
        self.pair_status.state = PairLayoutState::Maximizing;
        self.pair_status.reason = Some("等待 Windows 最大化并确认共同工作区".into());
        if let Some(group) = &mut self.joint {
            group.native_maximized = unsafe { IsZoomed(hwnd) } != 0;
        }
        if self.valid(&self.own) && unsafe { IsZoomed(self.own.identity.handle as HWND) } != 0 {
            if let Some(group) = &mut self.joint {
                group.normalizing_own_max = true;
            }
            unsafe {
                ShowWindowAsync(self.own.identity.handle as HWND, SW_SHOWNOACTIVATE);
            }
        }
        // The group fills the real work area without activating an inactive target.
        // A target already maximized by Windows keeps its actual maximize style.
        if !self
            .joint
            .as_ref()
            .is_some_and(|group| group.normalizing_own_max)
        {
            self.joint_position(expected)?;
            if let Some(group) = &mut self.joint {
                if let Some(pending) = &mut group.pending {
                    pending.issued = true;
                }
            }
        }
        Ok(())
    }
    pub(super) fn restore_joint(&mut self) -> Result<(), String> {
        if self.cancelled.is_some() {
            return Err(
                "LAYOUT_CANCELLATION_PENDING: wait for accepted consent requests to settle".into(),
            );
        }
        if self.user_surface_hidden || self.joint.as_ref().is_some_and(|group| group.suspended) {
            return Err("DISPLAY_SUSPENDED: joint layout waits for expansion".into());
        }
        let target = self.target.as_ref().ok_or("TARGET_REQUIRED")?;
        if !self.valid(target) {
            return Err("STALE_TARGET".into());
        }
        let group = self.joint.as_ref().ok_or("PAIR_REQUIRED")?;
        let expected = group.normal;
        let target_placement = group.normal_target;
        let own_placement = group.normal_own;
        let before = observed_pair(
            target.identity.handle as HWND,
            self.own.identity.handle as HWND,
        )
        .ok_or("WINDOW_BOUNDS_UNAVAILABLE")?;
        let normal_unobserved = group.normal_unobserved;
        if let Some(group) = &mut self.joint {
            if normal_unobserved {
                group.user_changed = true;
            }
            group.pending = Some(PendingPair {
                before,
                expected,
                state: PairLayoutState::Paired,
                deadline: Instant::now() + Duration::from_secs(2),
                issued: false,
            });
        }
        self.pair_status.state = PairLayoutState::Restoring;
        let target = self.target.as_ref().ok_or("TARGET_REQUIRED")?;
        if !self.valid(target) {
            return Err("STALE_TARGET".into());
        }
        // A shared maximum may already use ordinary windows (custom-frame fallback).
        // Replaying asynchronous placement in that state can arrive after our paired
        // position request and expand the target over the Dock again.
        if unsafe { IsZoomed(target.identity.handle as HWND) } != 0 {
            restore_placement(target.identity.handle as HWND, target_placement)?;
        }
        if self.valid(&self.own) && unsafe { IsZoomed(self.own.identity.handle as HWND) } != 0 {
            restore_placement(self.own.identity.handle as HWND, own_placement)?;
        }
        if !normal_unobserved {
            self.joint_position(expected)?;
            if let Some(group) = &mut self.joint {
                if let Some(pending) = &mut group.pending {
                    pending.issued = true;
                }
            }
        }
        Ok(())
    }
    pub(super) fn follow_joint(&mut self) -> bool {
        if self.joint.is_none() || self.layout.is_some() || self.arrangement.is_some() {
            return false;
        }
        if self.joint.as_ref().is_some_and(|group| group.suspended) {
            self.observe_suspended_joint();
            self.try_resume_joint();
            if self.joint.as_ref().is_some_and(|group| group.suspended) {
                return true;
            }
        }
        let Some(target) = self.target.as_ref() else {
            self.joint = None;
            return false;
        };
        if !self.valid(target) || !self.valid(&self.own) {
            self.invalidate("Target lifecycle cannot be verified; binding was removed");
            return true;
        }
        let hwnd = target.identity.handle as HWND;
        let own_hwnd = self.own.identity.handle as HWND;
        let Some(target_visible) = (match visibility_status(hwnd) {
            VisibilityStatus::Visible => Some(true),
            VisibilityStatus::Hidden => Some(false),
            // DWM inspection failure is unknown, not evidence that the target was hidden.
            VisibilityStatus::Unavailable => None,
        }) else {
            self.hint =
                Some("Target visibility is unavailable; waiting for Windows observation".into());
            return true;
        };
        if !target_visible {
            if !self.expanded_while_target_hidden
                && !self.hidden_by_us
                && unsafe { IsIconic(own_hwnd) } == 0
                && unsafe { ShowWindowAsync(own_hwnd, SW_HIDE) } != 0
            {
                self.hidden_by_us = true;
                self.main_visibility_pending = Some(false);
            }
            self.hint = Some("Target is minimized or hidden; companion waits for restore".into());
            return true;
        }
        self.expanded_while_target_hidden = false;
        self.show_own();
        // This hint describes the previous visibility observation, not a layout failure.
        // Clear it as soon as the target is visible, even when no geometry changes follow.
        if matches!(
            self.hint.as_deref(),
            Some(
                "Target is minimized or hidden; companion waits for restore"
                    | "Target visibility is unavailable; waiting for Windows observation"
            )
        ) {
            self.hint = None;
        }
        let Some(actual) = observed_pair(hwnd, own_hwnd) else {
            return true;
        };
        let zoomed = unsafe { IsZoomed(hwnd) } != 0;
        let own_zoomed = unsafe { IsZoomed(own_hwnd) } != 0;
        if unsafe { IsIconic(own_hwnd) } != 0 {
            return true;
        }
        let pending = self.joint.as_ref().and_then(|g| g.pending.clone());
        if let Some(mut pending) = pending {
            if self
                .joint
                .as_ref()
                .is_some_and(|group| group.normalizing_own_max)
            {
                if own_zoomed {
                    if Instant::now() >= pending.deadline {
                        self.pair_status.state = PairLayoutState::Blocked;
                        self.pair_status.reason =
                            Some("尚未收到伴侣窗口正常状态确认，继续等待已发出的请求".into());
                    }
                    return true;
                }
                let Some(work) = work_area(hwnd) else {
                    return true;
                };
                let Some(limits) = self.joint_limits() else {
                    return true;
                };
                let Some(width) =
                    self.preferred_physical_width(self.pair_status.preferred_width_dip)
                else {
                    return true;
                };
                let Some(expected) =
                    maximized_pair(work, width, limits.0, limits.1, limits.2, limits.3)
                else {
                    return true;
                };
                pending.expected = expected;
                pending.issued = false;
                pending.deadline = Instant::now() + Duration::from_secs(2);
                if let Some(group) = &mut self.joint {
                    group.normalizing_own_max = false;
                    group.pending = Some(pending.clone());
                }
            }
            if self
                .joint
                .as_ref()
                .is_some_and(|group| group.normalizing_native_max)
            {
                if zoomed {
                    if Instant::now() >= pending.deadline {
                        self.pair_status.state = PairLayoutState::Blocked;
                        self.pair_status.reason =
                            Some("目标窗口尚未确认共同布局状态，继续等待已发出的请求".into());
                    }
                    return true;
                }
                pending.issued = false;
                pending.deadline = Instant::now() + Duration::from_secs(2);
                let normal = placement(hwnd);
                if let Some(group) = &mut self.joint {
                    group.normalizing_native_max = false;
                    if let Some(normal) = normal {
                        group.normal_target = normal;
                    }
                    if let Some(active) = &mut group.pending {
                        active.issued = false;
                        active.deadline = Instant::now() + Duration::from_secs(2);
                    }
                }
            }
            if pending.state == PairLayoutState::Maximized
                && self
                    .joint
                    .as_ref()
                    .is_some_and(|group| group.native_maximized)
                && !zoomed
            {
                if Instant::now() >= pending.deadline {
                    self.pair_status.state = PairLayoutState::Blocked;
                    self.pair_status.reason =
                        Some("尚未收到目标最大化确认，继续等待已发出的请求".into());
                }
                return true;
            }
            if pending.state == PairLayoutState::Paired && (zoomed || own_zoomed) {
                if Instant::now() >= pending.deadline {
                    self.pair_status.state = PairLayoutState::Blocked;
                    self.pair_status.reason =
                        Some("尚未收到正常共同布局确认，继续等待已发出的请求".into());
                }
                return true;
            }
            if actual == pending.expected && joined(actual) {
                if let Some(group) = &mut self.joint {
                    group.current = actual;
                    group.pending = None;
                    group.state = pending.state.clone();
                    if pending.state == PairLayoutState::Paired {
                        group.normal_unobserved = false;
                        group.normal = actual;
                        if let Some(p) = placement(hwnd) {
                            group.normal_target = p;
                        }
                        if let Some(p) = placement(own_hwnd) {
                            group.normal_own = p;
                        }
                    }
                }
                self.pair_status.state = pending.state;
                self.pair_status.reason = None;
                self.hint = None;
                self.layout_required = false;
                if self.dock_confirmation.is_none() {
                    if let Some(target) = &self.target {
                        self.dock_confirmation = Some(DockConfirmation {
                            binding_id: target.info.id.clone(),
                            side: DockSide::Right,
                        });
                    }
                }
            } else if Instant::now() >= pending.deadline {
                let work = work_area(hwnd);
                let owns_unchanged_max = pending.state == PairLayoutState::Maximized
                    && zoomed
                    && self
                        .joint
                        .as_ref()
                        .is_some_and(|group| group.native_maximized)
                    && work == Some(actual.target)
                    && actual.dock == pending.expected.dock;
                if owns_unchanged_max {
                    // Some custom-frame windows refuse a partial native-MAX rectangle.
                    // Their exact unchanged response permits a focus-safe normal-state
                    // workarea layout; arbitrary/user-changed geometry never grants this.
                    if let Some(group) = &mut self.joint {
                        group.native_maximized = false;
                        group.normalizing_native_max = true;
                        if let Some(active) = &mut group.pending {
                            active.issued = false;
                            active.deadline = Instant::now() + Duration::from_secs(2);
                        }
                    }
                    let Some(target) = self.target.as_ref() else {
                        return true;
                    };
                    if self.valid(target)
                        && unsafe { ShowWindowAsync(hwnd, SW_SHOWNOACTIVATE) } == 0
                    {
                        self.pair_status.state = PairLayoutState::Blocked;
                        self.pair_status.reason =
                            Some(last_error("Cannot normalize shared workarea"));
                    }
                    return true;
                }
                self.pair_status.state = PairLayoutState::Blocked;
                self.pair_status.reason = Some(format!(
                    "Windows 尚未确认共同布局；预期 {:?} / {:?}，实际 {:?} / {:?}",
                    pending.expected.target.tuple(),
                    pending.expected.dock.tuple(),
                    actual.target.tuple(),
                    actual.dock.tuple()
                ));
                self.layout_required = true;
                // Keep watching this exact owned transaction for a late acknowledgment.
            } else if !pending.issued {
                let expected = if pending.state == PairLayoutState::Paired
                    && self
                        .joint
                        .as_ref()
                        .is_some_and(|group| group.normal_unobserved)
                {
                    let Some(work) = work_area(hwnd) else {
                        return true;
                    };
                    let Some(limits) = self.joint_limits() else {
                        return true;
                    };
                    let Some(width) = self.preferred_physical_width(440) else {
                        return true;
                    };
                    let Some(value) = pair_at_target(
                        actual.target,
                        width,
                        work,
                        limits.0,
                        limits.1,
                        limits.2,
                        limits.3,
                    ) else {
                        return true;
                    };
                    value
                } else {
                    pending.expected
                };
                if let Err(error) = self.joint_position(expected) {
                    self.pair_status.state = PairLayoutState::Blocked;
                    self.pair_status.reason = Some(error);
                } else if let Some(group) = &mut self.joint {
                    if let Some(active) = &mut group.pending {
                        active.issued = true;
                        active.expected = expected;
                    }
                }
            }
            return true;
        }
        let (state, current, gesture) = self
            .joint
            .as_ref()
            .map(|g| (g.state.clone(), g.current, g.gesture.clone()))
            .unwrap();
        if gesture.is_none() && state == PairLayoutState::Maximized {
            if own_zoomed {
                if let Err(error) = self.maximize_joint() {
                    self.pair_status.reason = Some(error);
                }
                return true;
            }
            if let (Some(work), Some(limits)) = (work_area(hwnd), self.joint_limits()) {
                let width = self
                    .joint
                    .as_ref()
                    .map_or(440, |group| group.preferred_width_dip);
                if let Some(preferred) = self.preferred_physical_width(width) {
                    if let Some(expected) =
                        maximized_pair(work, preferred, limits.0, limits.1, limits.2, limits.3)
                    {
                        if !fills_workarea(current, work) {
                            if let Err(error) =
                                self.request_joint_position(expected, PairLayoutState::Maximized)
                            {
                                self.pair_status.reason = Some(error);
                            }
                            return true;
                        }
                    }
                }
            }
            if actual != current
                && !zoomed
                && !self
                    .joint
                    .as_ref()
                    .is_some_and(|group| group.native_maximized)
            {
                let target_changed = actual.target != current.target;
                let observed = if target_changed {
                    actual.target
                } else {
                    actual.dock
                };
                if let (Some(work), Some(limits)) = (work_area(hwnd), self.joint_limits()) {
                    if let Some(expected) = resize_pair(
                        current,
                        target_changed,
                        observed,
                        work,
                        limits.0,
                        limits.1,
                        limits.2,
                        limits.3,
                    ) {
                        if let Some(group) = &mut self.joint {
                            group.user_changed = true;
                        }
                        if let Err(error) =
                            self.request_joint_position(expected, PairLayoutState::Paired)
                        {
                            self.pair_status.reason = Some(error);
                        }
                        return true;
                    }
                }
            }
        }
        if gesture.is_none() && (zoomed || own_zoomed) && state != PairLayoutState::Maximized {
            if let Err(error) = self.maximize_joint() {
                self.pair_status.reason = Some(error);
            }
            return true;
        }
        if state == PairLayoutState::Maximized
            && !zoomed
            && gesture.is_none()
            && self
                .joint
                .as_ref()
                .is_some_and(|group| group.native_maximized)
        {
            if let Err(error) = self.restore_joint() {
                self.pair_status.reason = Some(error);
            }
            return true;
        }
        if gesture.is_none() && state == PairLayoutState::Maximized && zoomed && actual != current {
            if let Some(group) = &mut self.joint {
                group.native_maximized = true;
            }
            if let Err(error) = self.request_joint_position(current, PairLayoutState::Maximized) {
                self.pair_status.reason = Some(error);
            }
            return true;
        }
        if let Some(gesture) = gesture {
            if gesture.commanded == Some(actual) {
                let actual_state = work_area(hwnd)
                    .map(|work| {
                        if fills_workarea(actual, work) {
                            PairLayoutState::Maximized
                        } else {
                            PairLayoutState::Paired
                        }
                    })
                    .unwrap_or(PairLayoutState::Paired);
                if let Some(group) = &mut self.joint {
                    group.current = actual;
                    group.state = actual_state.clone();
                    if actual_state == PairLayoutState::Paired {
                        group.normal = actual;
                        if let Some(p) = placement(hwnd) {
                            group.normal_target = p;
                        }
                        if let Some(p) = placement(own_hwnd) {
                            group.normal_own = p;
                        }
                    }
                    if let Some((width, _)) = client_size(own_hwnd) {
                        group.preferred_width_dip = ((i64::from(width) * 96
                            + i64::from(unsafe { GetDpiForWindow(own_hwnd) }) / 2)
                            / i64::from(unsafe { GetDpiForWindow(own_hwnd) }.max(96)))
                        .clamp(360, 560) as u32;
                        self.pair_status.preferred_width_dip = group.preferred_width_dip;
                    }
                }
                self.pair_status.state = actual_state;
                self.pair_status.reason = None;
            }
            let observed = if gesture.target {
                actual.target
            } else {
                actual.dock
            };
            let commanded = gesture.commanded.map(|pair| {
                if gesture.target {
                    pair.target
                } else {
                    pair.dock
                }
            });
            if observed == gesture.last || Some(observed) == commanded {
                return true;
            }
            let Some(work) = work_area(hwnd) else {
                return true;
            };
            let Some(limits) = self.joint_limits() else {
                return true;
            };
            if let Some(expected) = resize_pair(
                gesture.base,
                gesture.target,
                observed,
                work,
                limits.0,
                limits.1,
                limits.2,
                limits.3,
            ) {
                if self.joint_position(expected).is_ok() {
                    if let Some(group) = &mut self.joint {
                        if let Some(gesture) = &mut group.gesture {
                            gesture.last = observed;
                            gesture.commanded = Some(expected);
                        }
                    }
                }
            }
            return true;
        }
        if actual != current && state != PairLayoutState::Maximized {
            let target_changed = actual.target != current.target;
            let observed = if target_changed {
                actual.target
            } else {
                actual.dock
            };
            let Some(work) = work_area(hwnd) else {
                return true;
            };
            let Some(limits) = self.joint_limits() else {
                return true;
            };
            if let Some(expected) = resize_pair(
                current,
                target_changed,
                observed,
                work,
                limits.0,
                limits.1,
                limits.2,
                limits.3,
            ) {
                if let Some(group) = &mut self.joint {
                    group.user_changed = true;
                }
                if let Err(error) = self.request_joint_position(expected, PairLayoutState::Paired) {
                    self.pair_status.reason = Some(error);
                }
            }
        }
        true
    }
    pub(super) fn joint_compensation(
        &self,
        group: &mut JointGroup,
        target: &TrackedWindow,
    ) -> JointCompensation {
        if group.user_changed {
            return JointCompensation::Finished;
        }
        let hwnd = target.identity.handle as HWND;
        let Some(actual) = observed_pair(hwnd, self.own.identity.handle as HWND) else {
            return JointCompensation::Waiting;
        };
        if group.compensating {
            return if actual == group.original
                && (group.original_target.showCmd == SW_SHOWMAXIMIZED as u32
                    || unsafe { IsZoomed(hwnd) } == 0)
            {
                JointCompensation::Finished
            } else {
                JointCompensation::Waiting
            };
        }
        let allowed = actual == group.current
            || group.pending.as_ref().is_some_and(|pending| {
                // Independent queued moves can settle separately. Only each window's exact
                // original/recorded command rectangles retain compensation ownership.
                let target_known = actual.target == group.current.target
                    || actual.target == pending.before.target
                    || actual.target == pending.expected.target
                    || pending.state == PairLayoutState::Maximized
                        && work_area(hwnd) == Some(actual.target);
                let own_known = actual.dock == group.current.dock
                    || actual.dock == pending.before.dock
                    || actual.dock == pending.expected.dock;
                target_known && own_known
            });
        if !allowed {
            return JointCompensation::Finished;
        }
        let Some(current) = placement(hwnd) else {
            return JointCompensation::Waiting;
        };
        // Only our exact recorded command states retain restoration ownership.
        let current_normal = current.rcNormalPosition;
        let own_requested = group.normal_target.rcNormalPosition;
        let same_known = (
            current_normal.left,
            current_normal.top,
            current_normal.right,
            current_normal.bottom,
        ) == (
            own_requested.left,
            own_requested.top,
            own_requested.right,
            own_requested.bottom,
        ) || normal_matches(current, group.original_target);
        let owned_normal_position = unsafe { IsZoomed(hwnd) } == 0
            && (actual.target == group.current.target
                || group
                    .pending
                    .as_ref()
                    .is_some_and(|pending| actual.target == pending.expected.target));
        if !same_known && !owned_normal_position {
            return JointCompensation::Finished;
        }
        group.compensating = true;
        JointCompensation::Restore(group.original_target, group.original_own, group.original)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn normal_pair() -> PairRects {
        PairRects {
            target: Rect::new(100, 100, 1300, 1500).unwrap(),
            dock: Rect::new(1300, 100, 2184, 1500).unwrap(),
        }
    }

    #[test]
    fn reserved_area_release_requires_exact_confirmed_pair_and_no_pending_user_gesture() {
        let pair = normal_pair();
        assert!(owns_reserved_area(pair, pair, false, false));
        assert!(!owns_reserved_area(pair, pair, true, false));
        assert!(!owns_reserved_area(pair, pair, false, true));
        let mut changed = pair;
        changed.target.left += 1;
        assert!(!owns_reserved_area(changed, pair, false, false));
        changed = pair;
        changed.dock.top += 1;
        assert!(!owns_reserved_area(changed, changed, false, false));
    }

    #[test]
    fn native_maximize_of_either_window_preserves_confirmed_normal_layout() {
        let normal = normal_pair();
        assert!(can_save_normal_pair(normal, normal, false, false));
        assert!(!can_save_normal_pair(normal, normal, false, true));
        assert!(!can_save_normal_pair(normal, normal, true, false));
        let own_maximized = PairRects {
            dock: Rect::new(0, 0, 2880, 1704).unwrap(),
            ..normal
        };
        assert!(!can_save_normal_pair(own_maximized, normal, false, true));
        // The OS flag may clear before DWM has settled. Geometry must still be confirmed.
        assert!(!can_save_normal_pair(own_maximized, normal, false, false));
    }

    #[test]
    fn overlapping_or_misaligned_rectangles_never_confirm_a_pair() {
        let normal = normal_pair();
        assert!(joined(normal));
        let overlap = PairRects {
            dock: Rect::new(0, 0, 2880, 1704).unwrap(),
            ..normal
        };
        assert!(!joined(overlap));
        assert!(!can_save_normal_pair(overlap, overlap, false, false));
        assert!(!joined(PairRects {
            dock: Rect::new(1300, 120, 2184, 1500).unwrap(),
            ..normal
        }));
    }
}
