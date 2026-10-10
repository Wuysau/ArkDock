//! Physical screen-pixel geometry. Coordinates may be negative; no DPI conversion here.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    pub fn new(left: i32, top: i32, right: i32, bottom: i32) -> Option<Self> {
        if right.checked_sub(left)? <= 0 || bottom.checked_sub(top)? <= 0 {
            return None;
        }
        Some(Self {
            left,
            top,
            right,
            bottom,
        })
    }

    pub fn width(self) -> i32 {
        self.right - self.left
    }
    pub fn height(self) -> i32 {
        self.bottom - self.top
    }
    pub fn tuple(self) -> (i32, i32, i32, i32) {
        (self.left, self.top, self.right, self.bottom)
    }
    pub fn contains(self, x: i32, y: i32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }
}

/// Adjacent visible frames of two independent top-level windows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PairRects {
    pub target: Rect,
    pub dock: Rect,
}

fn pair_rects(left: i32, top: i32, seam: i32, right: i32, bottom: i32) -> Option<PairRects> {
    Some(PairRects {
        target: Rect::new(left, top, seam, bottom)?,
        dock: Rect::new(seam, top, right, bottom)?,
    })
}

/// Validates the physical limits and chooses a Dock width that leaves the target its minimum.
fn pair_dock_width(
    work: Rect,
    preferred: i32,
    min_target: i32,
    min_dock: i32,
    max_dock: i32,
    min_height: i32,
) -> Option<i32> {
    Rect::new(work.left, work.top, work.right, work.bottom)?;
    if min_target <= 0
        || min_dock <= 0
        || max_dock < min_dock
        || min_height <= 0
        || min_height > work.height()
        || i64::from(min_target) + i64::from(min_dock) > i64::from(work.width())
    {
        return None;
    }
    Some(preferred.clamp(min_dock, max_dock.min(work.width() - min_target)))
}

/// Fits a normal joint layout near the target, reserving the preferred Dock width first.
/// Only visible frames are represented; callers convert any invisible native borders separately.
pub(super) fn pair_at_target(
    target: Rect,
    preferred_dock_width: i32,
    work: Rect,
    min_target_width: i32,
    min_dock_width: i32,
    max_dock_width: i32,
    min_height: i32,
) -> Option<PairRects> {
    Rect::new(target.left, target.top, target.right, target.bottom)?;
    let dock_width = pair_dock_width(
        work,
        preferred_dock_width,
        min_target_width,
        min_dock_width,
        max_dock_width,
        min_height,
    )?;
    let target_width = target
        .width()
        .clamp(min_target_width, work.width() - dock_width);
    let group_width = target_width + dock_width;
    let height = target.height().clamp(min_height, work.height());
    let left = target.left.clamp(work.left, work.right - group_width);
    let top = target.top.clamp(work.top, work.bottom - height);
    pair_rects(
        left,
        top,
        left.checked_add(target_width)?,
        left.checked_add(group_width)?,
        top.checked_add(height)?,
    )
}

/// Splits the entire Windows work area with the target on the left and Dock on the right.
pub(super) fn maximized_pair(
    work: Rect,
    preferred_dock_width: i32,
    min_target_width: i32,
    min_dock_width: i32,
    max_dock_width: i32,
    min_height: i32,
) -> Option<PairRects> {
    let dock_width = pair_dock_width(
        work,
        preferred_dock_width,
        min_target_width,
        min_dock_width,
        max_dock_width,
        min_height,
    )?;
    pair_rects(
        work.left,
        work.top,
        work.right.checked_sub(dock_width)?,
        work.right,
        work.bottom,
    )
}

/// Applies an observed move or resize relative to the pair before the native operation.
/// Outer horizontal edges retain the current Dock width; the shared seam redistributes width.
/// Equal edge deltas are a move. On a resize with both horizontal edges changed, the outer
/// edge takes precedence. Native frame margins must be removed before calling this helper.
#[allow(clippy::too_many_arguments)]
pub(super) fn resize_pair(
    base: PairRects,
    changed_target: bool,
    observed: Rect,
    work: Rect,
    min_target_width: i32,
    min_dock_width: i32,
    max_dock_width: i32,
    min_height: i32,
) -> Option<PairRects> {
    Rect::new(
        base.target.left,
        base.target.top,
        base.target.right,
        base.target.bottom,
    )?;
    Rect::new(
        base.dock.left,
        base.dock.top,
        base.dock.right,
        base.dock.bottom,
    )?;
    Rect::new(observed.left, observed.top, observed.right, observed.bottom)?;
    if base.target.right != base.dock.left
        || base.target.top != base.dock.top
        || base.target.bottom != base.dock.bottom
    {
        return None;
    }
    let group = Rect::new(
        base.target.left,
        base.target.top,
        base.dock.right,
        base.target.bottom,
    )?;
    let dock_width = pair_dock_width(
        work,
        base.dock.width(),
        min_target_width,
        min_dock_width,
        max_dock_width,
        min_height,
    )?;
    let original = if changed_target {
        base.target
    } else {
        base.dock
    };
    let min_group_width = min_target_width + dock_width;
    let (left, seam, right) = if observed.width() == original.width() {
        let width = group.width().clamp(min_group_width, work.width());
        let moved_left =
            i64::from(group.left) + i64::from(observed.left) - i64::from(original.left);
        let left = moved_left.clamp(i64::from(work.left), i64::from(work.right - width)) as i32;
        let right = left.checked_add(width)?;
        (left, right.checked_sub(dock_width)?, right)
    } else if changed_target && observed.left != original.left {
        let right = group.right.clamp(work.left + min_group_width, work.right);
        let left = observed.left.clamp(work.left, right - min_group_width);
        (left, right.checked_sub(dock_width)?, right)
    } else if !changed_target && observed.right != original.right {
        let left = group.left.clamp(work.left, work.right - min_group_width);
        let right = observed.right.clamp(left + min_group_width, work.right);
        (left, right.checked_sub(dock_width)?, right)
    } else {
        let min_width = min_target_width + min_dock_width;
        let width = group.width().clamp(min_width, work.width());
        let left = group.left.clamp(work.left, work.right - width);
        let right = left.checked_add(width)?;
        let observed_seam = if changed_target {
            observed.right
        } else {
            observed.left
        };
        let desired_seam = i64::from(observed_seam) + i64::from(left) - i64::from(group.left);
        let min_seam = (i64::from(left) + i64::from(min_target_width))
            .max(i64::from(right) - i64::from(max_dock_width));
        let max_seam = i64::from(right) - i64::from(min_dock_width);
        (left, desired_seam.clamp(min_seam, max_seam) as i32, right)
    };

    let (top, bottom) = if observed.height() == original.height() {
        let height = group.height().clamp(min_height, work.height());
        let top = observed.top.clamp(work.top, work.bottom - height);
        (top, top.checked_add(height)?)
    } else if observed.top != original.top && observed.bottom == original.bottom {
        let bottom = group.bottom.clamp(work.top + min_height, work.bottom);
        let top = observed.top.clamp(work.top, bottom - min_height);
        (top, bottom)
    } else {
        let top = observed.top.clamp(work.top, work.bottom - min_height);
        let bottom = observed.bottom.clamp(top + min_height, work.bottom);
        (top, bottom)
    };
    pair_rects(left, top, seam, right, bottom)
}

/// Four border windows leave the target's content and input surface uncovered.
pub(super) fn border_strips(rect: Rect, thickness: i32) -> Option<[Rect; 4]> {
    if thickness <= 0 || i64::from(thickness) * 2 >= i64::from(rect.width().min(rect.height())) {
        return None;
    }
    Some([
        Rect::new(
            rect.left,
            rect.top,
            rect.right,
            rect.top.checked_add(thickness)?,
        )?,
        Rect::new(
            rect.left,
            rect.bottom.checked_sub(thickness)?,
            rect.right,
            rect.bottom,
        )?,
        Rect::new(
            rect.left,
            rect.top,
            rect.left.checked_add(thickness)?,
            rect.bottom,
        )?,
        Rect::new(
            rect.right.checked_sub(thickness)?,
            rect.top,
            rect.right,
            rect.bottom,
        )?,
    ])
}

/// External left/right edge proximity; central overlap is never a docking candidate.
pub(super) fn edge_score(own: Rect, target: Rect, threshold: i32) -> Option<i32> {
    if threshold < 0 {
        return None;
    }
    let overlap = (own.bottom.min(target.bottom) as i64 - own.top.max(target.top) as i64).max(0);
    let required = i64::from(own.height().min(target.height())).max(1) / 4;
    if overlap < required.max(32) {
        return None;
    }
    let right = (i64::from(own.left) - i64::from(target.right)).abs();
    let left = (i64::from(own.right) - i64::from(target.left)).abs();
    let distance = right.min(left);
    (distance <= i64::from(threshold)).then_some(distance as i32)
}

/// Prefer the target's right, then left. Never move or resize the target implicitly.
pub(super) fn dock_rect(target: Rect, own_size: (i32, i32), work: Rect, gap: i32) -> Option<Rect> {
    let (width, height) = own_size;
    if width <= 0 || height <= 0 || gap < 0 || width > work.width() || height > work.height() {
        return None;
    }
    let top = target.top.clamp(work.top, work.bottom.checked_sub(height)?);
    let right_x = i64::from(target.right) + i64::from(gap);
    let left_x = i64::from(target.left) - i64::from(gap) - i64::from(width);
    let fits = |x: i64| x >= i64::from(work.left) && x + i64::from(width) <= i64::from(work.right);
    let x = if fits(right_x) {
        right_x
    } else if fits(left_x) {
        left_x
    } else {
        return None;
    };
    Rect::new(
        x as i32,
        top,
        (x + i64::from(width)) as i32,
        top.checked_add(height)?,
    )
}

/// An explicitly requested normal-window layout, bounded by both minimum widths.
pub(super) fn side_by_side(
    work: Rect,
    preferred_own: i32,
    min_target: i32,
    min_own: i32,
    gap: i32,
) -> Option<(Rect, Rect)> {
    if min_target <= 0 || min_own <= 0 || gap < 0 {
        return None;
    }
    let available = work.width().checked_sub(gap)?;
    if i64::from(available) < i64::from(min_target) + i64::from(min_own) {
        return None;
    }
    let own_width = preferred_own.clamp(min_own, available.checked_sub(min_target)?);
    let own_left = work.right.checked_sub(own_width)?;
    let target_right = own_left.checked_sub(gap)?;
    Some((
        Rect::new(work.left, work.top, target_right, work.bottom)?,
        Rect::new(own_left, work.top, work.right, work.bottom)?,
    ))
}

/// Conservative three-point z-order check at the edge nearest to ArkDock.
/// Rectangular occlusion may reject a transparent window; that is a safe false negative.
pub(super) fn edge_visible(target: Rect, own: Rect, higher_windows: &[Rect]) -> bool {
    let top = own.top.max(target.top);
    let bottom = own.bottom.min(target.bottom);
    if bottom <= top {
        return false;
    }
    let right_distance = (i64::from(own.left) - i64::from(target.right)).abs();
    let left_distance = (i64::from(own.right) - i64::from(target.left)).abs();
    let x = if right_distance <= left_distance {
        target.right - 1
    } else {
        target.left
    };
    let span = i64::from(bottom) - i64::from(top);
    [1i64, 2, 3].into_iter().any(|quarter| {
        let y = (i64::from(top) + span * quarter / 4) as i32;
        !higher_windows.iter().any(|rect| rect.contains(x, y))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn normal_pair() -> PairRects {
        PairRects {
            target: Rect::new(100, 100, 900, 800).unwrap(),
            dock: Rect::new(900, 100, 1340, 800).unwrap(),
        }
    }

    fn resize_standard(base: PairRects, changed_target: bool, observed: Rect) -> PairRects {
        resize_pair(
            base,
            changed_target,
            observed,
            Rect::new(0, 0, 1920, 1040).unwrap(),
            320,
            360,
            560,
            300,
        )
        .unwrap()
    }

    #[test]
    fn joint_normal_layout_matches_visible_edges_and_bounds_both_windows() {
        let work = Rect::new(0, 0, 1920, 1040).unwrap();
        assert_eq!(
            pair_at_target(
                Rect::new(1500, 900, 2200, 1500).unwrap(),
                440,
                work,
                320,
                360,
                560,
                300
            ),
            Some(PairRects {
                target: Rect::new(780, 440, 1480, 1040).unwrap(),
                dock: Rect::new(1480, 440, 1920, 1040).unwrap(),
            })
        );
        assert_eq!(
            pair_at_target(
                Rect::new(-100, -100, 1900, 900).unwrap(),
                440,
                Rect::new(0, 0, 1280, 680).unwrap(),
                320,
                360,
                560,
                300,
            ),
            Some(PairRects {
                target: Rect::new(0, 0, 840, 680).unwrap(),
                dock: Rect::new(840, 0, 1280, 680).unwrap(),
            })
        );
    }

    #[test]
    fn joint_layout_respects_physical_limits_on_negative_coordinate_monitor() {
        let work = Rect::new(-1920, -100, 0, 980).unwrap();
        let target = Rect::new(-1800, 0, -900, 900).unwrap();
        let pair = pair_at_target(target, 660, work, 480, 540, 840, 450).unwrap();
        assert_eq!(pair.target, target);
        assert_eq!(pair.dock.tuple(), (-900, 0, -240, 900));
        let maximum = maximized_pair(work, 660, 480, 540, 840, 450).unwrap();
        assert_eq!(maximum.target.tuple(), (-1920, -100, -660, 980));
        assert_eq!(maximum.dock.tuple(), (-660, -100, 0, 980));
    }

    #[test]
    fn joint_width_preference_is_clamped_and_reduced_only_when_work_area_requires_it() {
        let work = Rect::new(0, 0, 1920, 1040).unwrap();
        assert_eq!(
            maximized_pair(work, 1000, 320, 360, 560, 300)
                .unwrap()
                .dock
                .width(),
            560
        );
        assert_eq!(
            maximized_pair(work, 100, 320, 360, 560, 300)
                .unwrap()
                .dock
                .width(),
            360
        );
        let narrow =
            maximized_pair(Rect::new(0, 0, 700, 600).unwrap(), 440, 320, 360, 560, 300).unwrap();
        assert_eq!(narrow.target.tuple(), (0, 0, 320, 600));
        assert_eq!(narrow.dock.tuple(), (320, 0, 700, 600));
        let tiny = pair_at_target(
            Rect::new(10, 20, 110, 120).unwrap(),
            440,
            work,
            320,
            360,
            560,
            300,
        )
        .unwrap();
        assert_eq!(tiny.target.tuple(), (10, 20, 330, 320));
        assert_eq!(tiny.dock.tuple(), (330, 20, 770, 320));
    }

    #[test]
    fn joint_outer_edges_resize_group_while_retaining_current_dock_width() {
        let base = normal_pair();
        let left = resize_standard(base, true, Rect::new(-250, 100, 900, 800).unwrap());
        assert_eq!(left.target.tuple(), (0, 100, 900, 800));
        assert_eq!(left.dock, base.dock);
        let right = resize_standard(base, false, Rect::new(900, 100, 1550, 800).unwrap());
        assert_eq!(right.target.tuple(), (100, 100, 1110, 800));
        assert_eq!(right.dock.tuple(), (1110, 100, 1550, 800));
        let shrunken = resize_standard(base, false, Rect::new(900, 100, 950, 800).unwrap());
        assert_eq!(shrunken.target.tuple(), (100, 100, 510, 800));
        assert_eq!(shrunken.dock.tuple(), (510, 100, 950, 800));
        assert_eq!(shrunken.dock.width(), base.dock.width());
        let minimum = resize_standard(base, true, Rect::new(850, 100, 900, 800).unwrap());
        assert_eq!(minimum.target.tuple(), (580, 100, 900, 800));
        assert_eq!(minimum.dock, base.dock);
    }

    #[test]
    fn joint_seam_resize_redistributes_width_without_moving_outer_edges() {
        let base = normal_pair();
        let target_right = resize_standard(base, true, Rect::new(100, 100, 950, 800).unwrap());
        assert_eq!(target_right.target.tuple(), (100, 100, 950, 800));
        assert_eq!(target_right.dock.tuple(), (950, 100, 1340, 800));
        let dock_left = resize_standard(base, false, Rect::new(800, 100, 1340, 800).unwrap());
        assert_eq!(dock_left.target.tuple(), (100, 100, 800, 800));
        assert_eq!(dock_left.dock.tuple(), (800, 100, 1340, 800));
        let minimum = resize_standard(base, true, Rect::new(100, 100, 1300, 800).unwrap());
        assert_eq!(minimum.target.right, 980);
        assert_eq!(minimum.dock.width(), 360);
        let maximum = resize_standard(base, false, Rect::new(200, 100, 1340, 800).unwrap());
        assert_eq!(maximum.target.right, 780);
        assert_eq!(maximum.dock.width(), 560);
    }

    #[test]
    fn joint_seam_resize_also_preserves_target_minimum_in_narrow_group() {
        let base = PairRects {
            target: Rect::new(100, 100, 460, 800).unwrap(),
            dock: Rect::new(460, 100, 900, 800).unwrap(),
        };
        let pair = resize_standard(base, false, Rect::new(200, 100, 900, 800).unwrap());
        assert_eq!(pair.target.tuple(), (100, 100, 420, 800));
        assert_eq!(pair.dock.tuple(), (420, 100, 900, 800));
    }

    #[test]
    fn joint_vertical_and_corner_resize_matches_both_windows_and_keeps_opposite_edge() {
        let base = normal_pair();
        let top = resize_standard(base, false, Rect::new(900, -100, 1340, 800).unwrap());
        assert_eq!(top.target.tuple(), (100, 0, 900, 800));
        assert_eq!(top.dock.tuple(), (900, 0, 1340, 800));
        let corner = resize_standard(base, false, Rect::new(900, 100, 1550, 1200).unwrap());
        assert_eq!(corner.target.tuple(), (100, 100, 1110, 1040));
        assert_eq!(corner.dock.tuple(), (1110, 100, 1550, 1040));
        let top_minimum = resize_standard(base, true, Rect::new(100, 700, 900, 800).unwrap());
        assert_eq!((top_minimum.target.top, top_minimum.dock.top), (500, 500));
        assert_eq!(
            (top_minimum.target.bottom, top_minimum.dock.bottom),
            (800, 800)
        );
        let bottom_minimum = resize_standard(base, false, Rect::new(900, 100, 1340, 200).unwrap());
        assert_eq!(
            (bottom_minimum.target.top, bottom_minimum.dock.top),
            (100, 100)
        );
        assert_eq!(
            (bottom_minimum.target.bottom, bottom_minimum.dock.bottom),
            (400, 400)
        );
    }

    #[test]
    fn joint_moves_translate_both_windows_and_clamp_as_one_group() {
        let base = normal_pair();
        let moved = resize_standard(base, false, Rect::new(1000, 150, 1440, 850).unwrap());
        assert_eq!(moved.target.tuple(), (200, 150, 1000, 850));
        assert_eq!(moved.dock.tuple(), (1000, 150, 1440, 850));
        let clamped = resize_standard(base, true, Rect::new(1100, 500, 1900, 1200).unwrap());
        assert_eq!(clamped.target.tuple(), (680, 340, 1480, 1040));
        assert_eq!(clamped.dock.tuple(), (1480, 340, 1920, 1040));
    }

    #[test]
    fn joint_resize_refits_monitor_work_area_and_handles_large_negative_delta() {
        let base = normal_pair();
        let work = Rect::new(-1920, -100, 0, 980).unwrap();
        let moved = resize_pair(
            base,
            false,
            Rect::new(-1800, 0, -1360, 700).unwrap(),
            work,
            320,
            360,
            560,
            300,
        )
        .unwrap();
        assert_eq!(moved.target.tuple(), (-1920, 0, -1120, 700));
        assert_eq!(moved.dock.tuple(), (-1120, 0, -680, 700));
        let extreme = resize_standard(
            base,
            true,
            Rect::new(i32::MIN, -100, i32::MIN + 800, 600).unwrap(),
        );
        assert_eq!(extreme.target.tuple(), (0, 0, 800, 700));
        assert_eq!(extreme.dock.tuple(), (800, 0, 1240, 700));
    }

    #[test]
    fn joint_layout_rejects_invalid_limits_insufficient_space_and_malformed_pairs() {
        let work = Rect::new(0, 0, 1920, 1040).unwrap();
        assert_eq!(maximized_pair(work, 440, 0, 360, 560, 300), None);
        assert_eq!(maximized_pair(work, 440, 320, 560, 360, 300), None);
        assert_eq!(maximized_pair(work, 440, i32::MAX, 360, 560, 300), None);
        assert_eq!(maximized_pair(work, 440, 320, 360, 560, 1100), None);
        assert_eq!(
            maximized_pair(Rect::new(0, 0, 679, 600).unwrap(), 440, 320, 360, 560, 300),
            None
        );
        let invalid_work = Rect {
            left: i32::MIN,
            top: 0,
            right: i32::MAX,
            bottom: 1040,
        };
        assert_eq!(maximized_pair(invalid_work, 440, 320, 360, 560, 300), None);
        let mut base = normal_pair();
        base.dock.left += 1;
        assert_eq!(
            resize_pair(base, true, base.target, work, 320, 360, 560, 300),
            None
        );
        let mut base = normal_pair();
        base.dock.top += 1;
        assert_eq!(
            resize_pair(base, false, base.dock, work, 320, 360, 560, 300),
            None
        );
        let base = PairRects {
            target: Rect::new(i32::MIN, 0, -1, 500).unwrap(),
            dock: Rect::new(-1, 0, i32::MAX - 1, 500).unwrap(),
        };
        assert_eq!(
            resize_pair(base, true, base.target, work, 320, 360, 560, 300),
            None
        );
    }

    #[test]
    fn joint_resize_keeps_strict_pair_invariants_across_edge_combinations() {
        for work in [
            Rect::new(0, 0, 1920, 1040).unwrap(),
            Rect::new(-1920, -100, 0, 980).unwrap(),
            Rect::new(0, 0, 680, 300).unwrap(),
        ] {
            let base = pair_at_target(normal_pair().target, 440, work, 320, 360, 560, 300).unwrap();
            for changed_target in [false, true] {
                let original = if changed_target {
                    base.target
                } else {
                    base.dock
                };
                for delta_left in [-2000, -100, 0, 100, 2000] {
                    for delta_right in [-2000, -100, 0, 100, 2000] {
                        for delta_top in [-1000, 0, 1000] {
                            for delta_bottom in [-1000, 0, 1000] {
                                let Some(observed) = Rect::new(
                                    original.left + delta_left,
                                    original.top + delta_top,
                                    original.right + delta_right,
                                    original.bottom + delta_bottom,
                                ) else {
                                    continue;
                                };
                                let pair = resize_pair(
                                    base,
                                    changed_target,
                                    observed,
                                    work,
                                    320,
                                    360,
                                    560,
                                    300,
                                )
                                .unwrap();
                                assert_eq!(pair.target.right, pair.dock.left);
                                assert_eq!(pair.target.top, pair.dock.top);
                                assert_eq!(pair.target.bottom, pair.dock.bottom);
                                assert!(pair.target.left >= work.left);
                                assert!(pair.dock.right <= work.right);
                                assert!(pair.target.top >= work.top);
                                assert!(pair.target.bottom <= work.bottom);
                                assert!(pair.target.width() >= 320);
                                assert!((360..=560).contains(&pair.dock.width()));
                                assert!(pair.target.height() >= 300);
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn outline_is_four_confined_strips_with_an_empty_center() {
        let target = Rect::new(-1200, -200, -400, 600).unwrap();
        let strips = border_strips(target, 4).unwrap();
        assert_eq!(strips[0].tuple(), (-1200, -200, -400, -196));
        assert_eq!(strips[1].tuple(), (-1200, 596, -400, 600));
        assert_eq!(strips[2].width(), 4);
        assert_eq!(strips[3].width(), 4);
        assert!(strips.iter().all(|strip| !strip.contains(-800, 200)));
        assert!(border_strips(target, 0).is_none());
        assert!(border_strips(Rect::new(0, 0, 4, 4).unwrap(), 3).is_none());
    }

    // A wrong edge, sign, overlap or work-area branch must fail these fixtures.
    #[test]
    fn candidate_requires_vertical_overlap_and_near_external_edge() {
        let target = Rect::new(100, 100, 900, 800).unwrap();
        assert_eq!(
            edge_score(Rect::new(912, 200, 1212, 700).unwrap(), target, 32),
            Some(12)
        );
        assert_eq!(
            edge_score(Rect::new(-212, 200, 88, 700).unwrap(), target, 32),
            Some(12)
        );
        assert_eq!(
            edge_score(Rect::new(910, 790, 1210, 1290).unwrap(), target, 32),
            None
        );
        assert_eq!(
            edge_score(Rect::new(400, 200, 700, 700).unwrap(), target, 32),
            None
        );
        assert_eq!(
            edge_score(Rect::new(950, 200, 1250, 700).unwrap(), target, 32),
            None
        );
    }

    #[test]
    fn docks_in_physical_pixels_on_negative_coordinate_monitor() {
        let work = Rect::new(-1920, -100, 0, 980).unwrap();
        let target = Rect::new(-1800, 40, -900, 900).unwrap();
        assert_eq!(
            dock_rect(target, (380, 600), work, 12).map(Rect::tuple),
            Some((-888, 40, -508, 640))
        );
    }

    #[test]
    fn left_fallback_preserves_size_and_vertical_work_area() {
        let work = Rect::new(0, 0, 1920, 1040).unwrap();
        let target = Rect::new(900, 800, 1800, 1100).unwrap();
        assert_eq!(
            dock_rect(target, (400, 600), work, 10),
            Some(Rect::new(490, 440, 890, 1040).unwrap())
        );
    }

    #[test]
    fn insufficient_external_space_requires_explicit_layout() {
        let work = Rect::new(0, 0, 1280, 680).unwrap();
        let target = Rect::new(0, 0, 1280, 680).unwrap();
        assert_eq!(dock_rect(target, (380, 600), work, 8), None);
        assert_eq!(dock_rect(target, (380, 900), work, 8), None);
    }

    #[test]
    fn side_by_side_respects_minimums_and_work_area() {
        let work = Rect::new(-1600, 50, 0, 950).unwrap();
        assert_eq!(
            side_by_side(work, 400, 320, 280, 12),
            Some((
                Rect::new(-1600, 50, -412, 950).unwrap(),
                Rect::new(-400, 50, 0, 950).unwrap()
            ))
        );
        assert_eq!(
            side_by_side(Rect::new(0, 0, 590, 800).unwrap(), 300, 320, 280, 8),
            None
        );
    }

    #[test]
    fn occluded_edge_requires_an_uncovered_sample() {
        let target = Rect::new(0, 0, 900, 800).unwrap();
        let own = Rect::new(912, 100, 1212, 700).unwrap();
        assert!(!edge_visible(
            target,
            own,
            &[Rect::new(880, 0, 950, 800).unwrap()]
        ));
        assert!(edge_visible(
            target,
            own,
            &[Rect::new(880, 0, 950, 250).unwrap()]
        ));
    }

    #[test]
    fn invalid_and_overflowing_rectangles_are_rejected() {
        assert_eq!(Rect::new(10, 0, 10, 50), None);
        assert_eq!(Rect::new(i32::MIN, 0, i32::MAX, 50), None);
        assert_eq!(
            dock_rect(
                Rect::new(i32::MAX - 10, 0, i32::MAX, 20).unwrap(),
                (10, 10),
                Rect::new(0, 0, 100, 100).unwrap(),
                20
            ),
            None
        );
    }
}
