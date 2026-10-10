//! Visibility changes run in the same actor as joint layout and lifecycle tracking.
use super::*;

impl Native {
    pub(super) fn cancel_effects(&mut self) {
        if let Some(effects) = &self.effects {
            effects.cancel();
        }
        if let Some(outline) = &mut self.outline {
            outline.hide();
        }
    }

    pub(super) fn set_motion(&mut self, settings: MotionSettings) {
        if let Err(error) = self.surfaces.set_motion(settings.clone()) {
            self.display.status.reason = Some(error);
        }
        self.fallback_enabled = settings.intensity != crate::ipc::model::MotionIntensity::Off;
        if let Some(effects) = &self.effects {
            effects.set_preferences(settings.reduced, settings.intensity);
        }
        if !self.fallback_enabled {
            if let Some(outline) = &mut self.outline {
                outline.hide();
            }
        }
    }

    pub(super) fn request_display(
        &mut self,
        mode: DisplayMode,
        request_id: u64,
    ) -> Result<(), String> {
        if mode == DisplayMode::Hidden && !self.surfaces.tray_available {
            self.surfaces.retry_tray();
            if !self.surfaces.tray_available {
                self.display.status.tray_available = false;
                self.display.status.reason =
                    Some("系统托盘尚未确认注册；保留当前可恢复窗口".into());
                return Err(
                    "TRAY_UNAVAILABLE: ArkDock cannot hide without a registered recovery icon"
                        .into(),
                );
            }
        }
        if !self.display.begin(mode.clone(), request_id)? {
            return Ok(());
        }
        self.display_started = Some(Instant::now());
        self.display_hide_issued = false;
        self.cancel_effects();
        self.dragging = false;
        self.move_started = false;
        self.armed_at = None;
        self.candidate = None;
        if mode == DisplayMode::Expanded {
            self.expanded_while_target_hidden = self.target.as_ref().is_some_and(|target| {
                let hwnd = target.identity.handle as HWND;
                (unsafe { IsIconic(hwnd) }) != 0 || !visible(hwnd)
            });
            // A collapsed target may have been destroyed or its handle reused.
            if self
                .target
                .as_ref()
                .is_some_and(|target| !self.valid(target))
            {
                self.invalidate("Target lifecycle changed while collapsed; expansion is detached");
            }
            if !self.valid(&self.own) {
                return Err("MAIN_WINDOW_UNAVAILABLE".into());
            }
        } else {
            self.expanded_while_target_hidden = false;
            // Orb is available throughout any main-window hide transaction.
            if let Err(error) = self.surfaces.show_orb() {
                self.display.status.reason = Some(error.clone());
                self.display_started = None;
                self.display.status.mode =
                    if unsafe { IsWindowVisible(self.own.identity.handle as HWND) } != 0 {
                        DisplayMode::Expanded
                    } else if self.surfaces.orb_visible() {
                        DisplayMode::Orb
                    } else if self.surfaces.tray_available {
                        DisplayMode::Hidden
                    } else {
                        DisplayMode::Collapsing
                    };
                if self.display.status.mode == DisplayMode::Collapsing {
                    if let Some(recovery_id) = self.display.status.request_id.checked_add(1) {
                        let _ = self.request_display(DisplayMode::Expanded, recovery_id);
                    }
                }
                return Err(error);
            }
            self.user_surface_hidden = true;
            self.suspend_joint();
        }
        self.advance_display();
        Ok(())
    }

    pub(super) fn advance_display(&mut self) {
        self.display.status.tray_available = self.surfaces.tray_available;
        let Some(started) = self.display_started else {
            return;
        };
        let main = unsafe { IsWindowVisible(self.own.identity.handle as HWND) } != 0;
        if let Some(expected) = self.main_visibility_pending {
            if main == expected {
                self.main_visibility_pending = None;
            } else {
                // Serialize opposite visibility operations. The old visible state is
                // not an acknowledgment of a new request while a hide is still queued.
                if started.elapsed() > Duration::from_secs(2) {
                    self.display.status.reason =
                        Some("Windows 尚未确认前一个显示请求；恢复入口继续保留".into());
                }
                return;
            }
        }
        if self.display.desired == DisplayMode::Expanded && !main {
            if unsafe { ShowWindowAsync(self.own.identity.handle as HWND, SW_SHOWNOACTIVATE) } == 0
            {
                self.display.status.reason =
                    Some(last_error("Cannot request main window expansion"));
            } else {
                self.main_visibility_pending = Some(true);
            }
            return;
        }
        match self.display.desired {
            DisplayMode::Expanded if main => {
                if self.user_surface_hidden {
                    self.user_surface_hidden = false;
                    self.hidden_by_us = false;
                    self.resume_joint();
                }
                if let Err(error) = self.surfaces.hide_orb() {
                    self.display.status.reason = Some(error);
                    return;
                }
            }
            DisplayMode::Orb | DisplayMode::Hidden => {
                if self.display.desired == DisplayMode::Hidden && !self.surfaces.tray_available {
                    // Explorer restarted between registration and the hide acknowledgment.
                    self.display.status.reason = Some("托盘暂不可用；悬浮球保持可见".into());
                    return;
                }
                if main && self.surfaces.orb_visible() && !self.display_hide_issued {
                    if unsafe { ShowWindowAsync(self.own.identity.handle as HWND, SW_HIDE) } == 0 {
                        self.display.status.reason =
                            Some(last_error("Cannot request main window hide"));
                        return;
                    }
                    self.display_hide_issued = true;
                    self.main_visibility_pending = Some(false);
                    return;
                }
                if !main && self.display.desired == DisplayMode::Hidden {
                    if let Err(error) = self.surfaces.hide_orb() {
                        self.display.status.reason = Some(error);
                        return;
                    }
                }
            }
            _ => {}
        }
        let main = unsafe { IsWindowVisible(self.own.identity.handle as HWND) } != 0;
        if self.display.observe(
            main,
            self.surfaces.orb_visible(),
            self.surfaces.tray_available,
        ) {
            self.display_started = None;
        } else if started.elapsed() > Duration::from_secs(2) {
            // An accepted async request may arrive late. Keep observing, keep the new
            // recovery surface, and never turn an elapsed timer into visibility success.
            self.display.status.reason =
                Some("仍在等待 Windows 确认可见性；当前恢复入口继续保留".into());
        }
    }

    pub(super) fn surface_action(&mut self, action: SurfaceAction) {
        match action {
            SurfaceAction::Press(_)
            | SurfaceAction::Move(_)
            | SurfaceAction::Release(_)
            | SurfaceAction::CancelDrag => match self.surfaces.handle_pointer(action) {
                Ok(true) => self.surface_action(SurfaceAction::Expand),
                Err(error) => self.display.status.reason = Some(error),
                _ => {}
            },
            SurfaceAction::OrbMenu | SurfaceAction::TrayMenu => {
                if let Some(action) = self
                    .surfaces
                    .menu(matches!(action, SurfaceAction::TrayMenu))
                {
                    self.surface_action(action);
                }
            }
            SurfaceAction::Expand | SurfaceAction::ShowOrb | SurfaceAction::Hide => {
                let mode = match action {
                    SurfaceAction::Expand => DisplayMode::Expanded,
                    SurfaceAction::ShowOrb => DisplayMode::Orb,
                    _ => DisplayMode::Hidden,
                };
                match self.display.status.request_id.checked_add(1) {
                    Some(id) => {
                        if let Err(error) = self.request_display(mode, id) {
                            self.display.status.reason = Some(error);
                        }
                    }
                    None => {
                        self.display.status.reason =
                            Some("Display request sequence exhausted".into())
                    }
                }
            }
            SurfaceAction::AutoHide => {
                let enabled = !self.display.status.orb_auto_hide;
                match self.surfaces.set_auto_hide(enabled) {
                    Ok(()) => self.display.status.orb_auto_hide = enabled,
                    Err(error) => self.display.status.reason = Some(error),
                }
            }
            SurfaceAction::FixLeft | SurfaceAction::FixRight => {
                let side = if matches!(action, SurfaceAction::FixLeft) {
                    DockSide::Left
                } else {
                    DockSide::Right
                };
                if let Err(error) = self.surfaces.fix_edge(side) {
                    self.display.status.reason = Some(error);
                }
            }
            SurfaceAction::ExplorerRestart => {
                self.surfaces.explorer_restarted();
                self.display.status.tray_available = false;
                if self.display.status.mode == DisplayMode::Hidden
                    || self.display.desired == DisplayMode::Hidden
                {
                    // Restore an actual surface immediately before attempting Explorer re-registration.
                    let id = self.display.status.request_id.saturating_add(1);
                    if let Err(error) = self.request_display(DisplayMode::Orb, id) {
                        self.display.status.reason = Some(error);
                        // Main recovery request is independent of Orb rendering failure.
                        if let Some(recovery_id) = self.display.status.request_id.checked_add(1) {
                            if let Err(error) =
                                self.request_display(DisplayMode::Expanded, recovery_id)
                            {
                                self.display.status.reason = Some(error);
                            }
                        }
                    }
                }
            }
            SurfaceAction::Exit => {
                self.cancel_effects();
                self.exit_requested = true;
            }
        }
    }
}
