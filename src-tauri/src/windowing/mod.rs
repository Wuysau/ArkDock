//! Independent external docking. Native handles stay inside this backend module.
//! The caller owns the Tauri drag operation and must validate IPC/session provenance.

use crate::ipc::model::{
    DisplayMode, DisplayStatus, DockConfirmation, MotionMetrics, MotionSettings, PairLayoutState,
    PairLayoutStatus, TargetInfo,
};

#[cfg(any(windows, test))]
mod display;
#[cfg(any(windows, test))]
mod effects;
#[cfg(any(windows, test))]
mod geometry;
#[cfg(any(windows, test))]
mod lifecycle;
#[cfg(windows)]
mod outline;
#[cfg(windows)]
mod surfaces;
#[cfg(windows)]
mod windows;

#[derive(Clone, Debug, Default)]
pub struct WindowUpdate {
    pub target: Option<TargetInfo>,
    pub dock_hint: Option<String>,
    pub layout_required: bool,
    pub pair_layout: PairLayoutStatus,
    pub display: DisplayStatus,
    pub dock_confirmation: Option<DockConfirmation>,
    pub exit_requested: bool,
}

/// The faintest configured Orb remains findable as the only recovery entry.
pub const ORB_MIN_OPACITY: u8 = 20;

impl Default for DisplayStatus {
    fn default() -> Self {
        Self {
            mode: DisplayMode::Expanded,
            request_id: 0,
            orb_auto_hide: false,
            orb_opacity: 100,
            tray_available: false,
            reason: None,
        }
    }
}

impl Default for PairLayoutStatus {
    fn default() -> Self {
        Self {
            state: PairLayoutState::Unbound,
            preferred_width_dip: 440,
            reason: None,
        }
    }
}

pub struct WindowController {
    #[cfg(windows)]
    inner: windows::Controller,
}

#[cfg(all(windows, feature = "desktop"))]
#[derive(Clone)]
pub(crate) struct DiscoveryScope {
    pub handle: isize,
}

impl WindowController {
    pub fn motion_metrics(&self) -> Result<MotionMetrics, String> {
        #[cfg(windows)]
        {
            self.inner.motion_metrics()
        }
        #[cfg(not(windows))]
        {
            Err("Windows motion surfaces are unsupported on this platform".into())
        }
    }
    pub fn set_display_mode(&self, mode: DisplayMode, request_id: u64) -> Result<(), String> {
        #[cfg(windows)]
        {
            self.inner.set_display_mode(mode, request_id)
        }
        #[cfg(not(windows))]
        {
            let _ = (mode, request_id);
            Err("Windows display surfaces are unsupported on this platform".into())
        }
    }

    pub fn set_orb_auto_hide(&self, auto_hide: bool) -> Result<(), String> {
        #[cfg(windows)]
        {
            self.inner.set_orb_auto_hide(auto_hide)
        }
        #[cfg(not(windows))]
        {
            let _ = auto_hide;
            Err("Windows display surfaces are unsupported on this platform".into())
        }
    }

    /// Percent form, `20..=100`: 20 keeps a faint but locatable recovery entry.
    pub fn set_orb_opacity(&self, opacity: u8) -> Result<(), String> {
        #[cfg(windows)]
        {
            self.inner.set_orb_opacity(opacity)
        }
        #[cfg(not(windows))]
        {
            let _ = opacity;
            Err("Windows display surfaces are unsupported on this platform".into())
        }
    }

    pub fn set_motion_preferences(&self, settings: MotionSettings) -> Result<(), String> {
        #[cfg(windows)]
        {
            self.inner.set_motion_preferences(settings)
        }
        #[cfg(not(windows))]
        {
            let _ = settings;
            Err("Windows motion surfaces are unsupported on this platform".into())
        }
    }

    #[cfg(all(windows, feature = "desktop"))]
    pub(crate) fn discovery_scope(&self, target_id: &str) -> Result<DiscoveryScope, String> {
        self.inner.discovery_scope(target_id)
    }
    /// Start after the independent main window has been created. `emit` runs on the
    /// hook thread: queue the update; do not call blocking controller methods there.
    pub fn start(
        own_handle: isize,
        emit: impl Fn(WindowUpdate) + Send + Sync + 'static,
    ) -> Result<Self, String> {
        #[cfg(windows)]
        {
            windows::Controller::start(own_handle, emit).map(|inner| Self { inner })
        }
        #[cfg(not(windows))]
        {
            let _ = (own_handle, emit);
            Err("Windows external docking is unsupported on this platform".into())
        }
    }

    /// Arm detection before the Tauri main-thread `startDragging` call. Only an
    /// observed native move-size start/end pair can bind a target.
    pub fn begin_drag(&self) -> Result<(), String> {
        #[cfg(windows)]
        {
            self.inner.begin_drag()
        }
        #[cfg(not(windows))]
        {
            Err("Windows external docking is unsupported on this platform".into())
        }
    }

    /// Unbind and restore a consent layout only if both windows still match it.
    pub fn detach(&self) {
        #[cfg(windows)]
        self.inner.detach();
    }

    /// `Ok` means Windows accepted the asynchronous requests. Actual application
    /// is reported later in `dock_hint`; no acceptance is fabricated.
    pub fn arrange_side_by_side(&self, consent: bool) -> Result<(), String> {
        if !consent {
            return Err("Explicit consent is required to move the target window".into());
        }
        #[cfg(windows)]
        {
            self.inner.arrange_side_by_side()
        }
        #[cfg(not(windows))]
        {
            Err("Windows external docking is unsupported on this platform".into())
        }
    }

    pub fn current_target(&self) -> Option<TargetInfo> {
        #[cfg(windows)]
        {
            self.inner.current_target()
        }
        #[cfg(not(windows))]
        {
            None
        }
    }

    pub fn maximize_pair(&self, target_id: &str) -> Result<(), String> {
        #[cfg(windows)]
        {
            self.inner.maximize_pair(target_id)
        }
        #[cfg(not(windows))]
        {
            Err("Windows joint layout is unsupported on this platform".into())
        }
    }

    pub fn restore_pair(&self, target_id: &str) -> Result<(), String> {
        #[cfg(windows)]
        {
            self.inner.restore_pair(target_id)
        }
        #[cfg(not(windows))]
        {
            Err("Windows joint layout is unsupported on this platform".into())
        }
    }
}

#[cfg(all(test, not(windows)))]
mod tests {
    use super::*;

    #[test]
    fn unsupported_platform_never_creates_a_fake_controller() {
        assert!(WindowController::start(0, |_| {}).is_err());
    }
}
