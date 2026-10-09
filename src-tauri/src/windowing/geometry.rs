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
