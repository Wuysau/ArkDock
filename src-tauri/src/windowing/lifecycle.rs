//! Pure lifecycle and consent-layout rules, independent of the platform APIs.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Identity {
    pub handle: isize,
    pub pid: u32,
    pub started_at: u64,
    pub thread_id: u32,
    pub destruction_epoch: u64,
}

impl Identity {
    pub fn matches(self, observed: Self) -> bool {
        self == observed
    }
}

pub(super) type Bounds = (i32, i32, i32, i32);

#[derive(Clone, Debug)]
pub(super) struct Layout {
    original_target: Bounds,
    original_own: Bounds,
    pub expected_target: Bounds,
    pub expected_own: Bounds,
    applied: bool,
    changed: bool,
}

impl Layout {
    pub fn new(
        original_target: Bounds,
        original_own: Bounds,
        expected_target: Bounds,
        expected_own: Bounds,
    ) -> Self {
        Self {
            original_target,
            original_own,
            expected_target,
            expected_own,
            applied: false,
            changed: false,
        }
    }

    pub fn observe(&mut self, target: Bounds, own: Bounds) {
        let expected = target == self.expected_target && own == self.expected_own;
        if (self.applied && !expected) || !self.awaiting_expected(target, own) {
            self.changed = true;
        }
        if expected && !self.changed {
            self.applied = true;
        }
    }

    /// While separate asynchronous requests apply, each window may still be original or expected.
    pub fn awaiting_expected(&self, target: Bounds, own: Bounds) -> bool {
        !self.changed
            && (target == self.original_target || target == self.expected_target)
            && (own == self.original_own || own == self.expected_own)
    }

    pub fn applied(&self) -> bool {
        self.applied
    }

    pub fn can_restore(&self, target: Bounds, own: Bounds) -> bool {
        self.applied && !self.changed && target == self.expected_target && own == self.expected_own
    }

    pub fn restore_bounds(&self, target: Bounds, own: Bounds) -> Option<(Bounds, Bounds)> {
        self.can_restore(target, own)
            .then_some((self.original_target, self.original_own))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> Identity {
        Identity {
            handle: 40,
            pid: 7,
            started_at: 900,
            thread_id: 12,
            destruction_epoch: 3,
        }
    }

    #[test]
    fn process_restart_never_keeps_a_window_binding() {
        let original = identity();
        let restarted = Identity {
            started_at: 901,
            ..original
        };
        assert!(!original.matches(restarted));
        assert!(!original.matches(Identity { pid: 8, ..original }));
    }

    #[test]
    fn same_process_handle_reuse_is_rejected_after_destruction() {
        let original = identity();
        assert!(!original.matches(Identity {
            destruction_epoch: 4,
            ..original
        }));
        assert!(!original.matches(Identity {
            thread_id: 13,
            ..original
        }));
        assert!(original.matches(identity()));
    }

    #[test]
    fn user_layout_change_prevents_restoring_even_if_it_later_returns() {
        let original = Layout::new(
            (0, 0, 800, 600),
            (820, 0, 1200, 600),
            (0, 0, 600, 800),
            (610, 0, 1000, 800),
        );
        let mut layout = original;
        layout.observe((0, 0, 600, 800), (610, 0, 1000, 800));
        assert!(layout.can_restore((0, 0, 600, 800), (610, 0, 1000, 800)));
        assert_eq!(
            layout.restore_bounds((0, 0, 600, 800), (610, 0, 1000, 800)),
            Some(((0, 0, 800, 600), (820, 0, 1200, 600)))
        );
        layout.observe((20, 0, 620, 800), (610, 0, 1000, 800));
        layout.observe((0, 0, 600, 800), (610, 0, 1000, 800));
        assert!(!layout.can_restore((0, 0, 600, 800), (610, 0, 1000, 800)));
        assert_eq!(
            layout.restore_bounds((0, 0, 600, 800), (610, 0, 1000, 800)),
            None
        );
    }

    #[test]
    fn a_queued_layout_is_not_treated_as_applied() {
        let mut layout = Layout::new(
            (0, 0, 800, 600),
            (820, 0, 1200, 600),
            (0, 0, 600, 800),
            (610, 0, 1000, 800),
        );
        assert!(!layout.applied());
        assert!(!layout.can_restore((0, 0, 800, 600), (820, 0, 1200, 600)));
        layout.observe((0, 0, 600, 800), (610, 0, 1000, 800));
        assert!(layout.applied());
        assert!(layout.can_restore((0, 0, 600, 800), (610, 0, 1000, 800)));
    }

    #[test]
    fn pending_layout_rejects_user_changes_before_both_requests_apply() {
        let mut layout = Layout::new(
            (0, 0, 800, 600),
            (820, 0, 1200, 600),
            (0, 0, 600, 800),
            (610, 0, 1000, 800),
        );
        assert!(layout.awaiting_expected((0, 0, 600, 800), (820, 0, 1200, 600)));
        layout.observe((0, 0, 600, 800), (820, 0, 1200, 600));
        layout.observe((20, 0, 620, 800), (820, 0, 1200, 600));
        layout.observe((0, 0, 600, 800), (610, 0, 1000, 800));
        assert!(!layout.applied());
        assert!(!layout.can_restore((0, 0, 600, 800), (610, 0, 1000, 800)));
    }
}
