//! Visibility transactions and Orb geometry, independent of Win32 acknowledgments.
use super::geometry::Rect;
use crate::ipc::model::{DisplayMode, DisplayStatus, DockSide};

pub(super) struct DisplayTransaction {
    pub status: DisplayStatus,
    pub desired: DisplayMode,
}

impl Default for DisplayTransaction {
    fn default() -> Self {
        Self {
            status: DisplayStatus::default(),
            desired: DisplayMode::Expanded,
        }
    }
}

impl DisplayTransaction {
    /// A duplicate retries no native operation, including a pending transaction.
    pub fn begin(&mut self, desired: DisplayMode, request_id: u64) -> Result<bool, String> {
        if !matches!(
            desired,
            DisplayMode::Expanded | DisplayMode::Orb | DisplayMode::Hidden
        ) {
            return Err("Only stable display modes may be requested".into());
        }
        if request_id < self.status.request_id {
            return Err("STALE_DISPLAY_REQUEST: a newer display request already exists".into());
        }
        if request_id == self.status.request_id {
            return if desired == self.desired {
                Ok(false)
            } else {
                Err("DISPLAY_REQUEST_CONFLICT: request ID already names another mode".into())
            };
        }
        self.status.request_id = request_id;
        self.status.reason = None;
        self.desired = desired.clone();
        self.status.mode = match desired {
            DisplayMode::Expanded => DisplayMode::Expanding,
            DisplayMode::Orb | DisplayMode::Hidden => DisplayMode::Collapsing,
            _ => unreachable!(),
        };
        Ok(true)
    }

    /// Only actual visibility observations can commit a stable state.
    pub fn observe(&mut self, main: bool, orb: bool, tray: bool) -> bool {
        self.status.tray_available = tray;
        let confirmed = match self.desired {
            DisplayMode::Expanded => main && !orb,
            DisplayMode::Orb => !main && orb,
            DisplayMode::Hidden => !main && !orb && tray,
            _ => false,
        };
        if confirmed {
            self.status.mode = self.desired.clone();
            self.status.reason = None;
        }
        confirmed
    }
}

pub(super) fn orb_edge(
    work: Rect,
    pointer: (i32, i32),
    size: i32,
    safe: i32,
    fixed: Option<DockSide>,
) -> Option<(Rect, DockSide)> {
    if size <= 0 || safe < 0 || work.width() < size || work.height() < size + safe.checked_mul(2)? {
        return None;
    }
    let side = fixed.unwrap_or_else(|| {
        if (i64::from(pointer.0) - i64::from(work.left)).abs()
            < (i64::from(pointer.0) - i64::from(work.right)).abs()
        {
            DockSide::Left
        } else {
            DockSide::Right
        }
    });
    let left = if side == DockSide::Left {
        work.left
    } else {
        work.right.checked_sub(size)?
    };
    let top = pointer.1.saturating_sub(size / 2).clamp(
        work.top.checked_add(safe)?,
        work.bottom.checked_sub(size + safe)?,
    );
    Some((
        Rect::new(left, top, left.checked_add(size)?, top.checked_add(size)?)?,
        side,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visibility_acknowledgments_gate_every_stable_state() {
        let mut display = DisplayTransaction::default();
        assert!(display.begin(DisplayMode::Orb, 1).unwrap());
        assert!(!display.observe(true, true, true));
        assert_eq!(display.status.mode, DisplayMode::Collapsing);
        assert!(display.observe(false, true, false));
        assert_eq!(display.status.mode, DisplayMode::Orb);
        assert!(display.begin(DisplayMode::Expanded, 2).unwrap());
        assert!(!display.observe(false, true, true));
        assert!(!display.observe(true, true, true));
        assert!(display.observe(true, false, false));
        assert!(display.begin(DisplayMode::Hidden, 3).unwrap());
        assert!(!display.observe(false, false, false));
        assert!(display.observe(false, false, true));
    }

    #[test]
    fn requests_are_monotonic_idempotent_and_never_accept_transient_modes() {
        let mut display = DisplayTransaction::default();
        assert!(display.begin(DisplayMode::Orb, 9).unwrap());
        assert!(!display.begin(DisplayMode::Orb, 9).unwrap());
        assert!(display.begin(DisplayMode::Expanded, 8).is_err());
        assert!(display.begin(DisplayMode::Expanded, 9).is_err());
        assert!(display.begin(DisplayMode::Collapsing, 10).is_err());
        assert_eq!(display.status.request_id, 9);
    }

    #[test]
    fn orb_edges_revalidate_negative_workareas_and_safe_vertical_bounds() {
        let work = Rect::new(-1920, -100, 0, 980).unwrap();
        let (left, side) = orb_edge(work, (-1850, -900), 116, 16, None).unwrap();
        assert_eq!(side, DockSide::Left);
        assert_eq!(left.tuple(), (-1920, -84, -1804, 32));
        let (right, _) = orb_edge(work, (-10, 2000), 174, 24, None).unwrap();
        assert_eq!(right.tuple(), (-174, 782, 0, 956));
        assert!(orb_edge(Rect::new(0, 0, 70, 70).unwrap(), (0, 0), 58, 8, None).is_none());
        assert_eq!(
            orb_edge(work, (-10, 0), 58, 8, Some(DockSide::Left))
                .unwrap()
                .1,
            DockSide::Left
        );
    }
}
