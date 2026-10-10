//! A small per-pixel-alpha overlay; the native event hook never renders pixels.
use super::geometry::Rect;
use crate::ipc::model::MotionIntensity;
use std::sync::atomic::{AtomicPtr, Ordering};
use std::time::Duration;

#[cfg(windows)]
#[path = "effects/native.rs"]
mod native;
#[cfg(windows)]
pub(super) use native::Effects;

const FRAME_INTERVAL: Duration = Duration::from_millis(16);
const DISSIPATE: Duration = Duration::from_millis(160);
#[cfg(windows)]
const CANDIDATE_FRESHNESS: Duration = Duration::from_millis(300);
const MAX_PIXELS: usize = 1_048_576;

#[derive(Clone, Debug)]
pub(super) struct EffectTarget {
    pub own: Rect,
    pub target: Rect,
    pub target_id: String,
    pub distance: i32,
    pub threshold: i32,
    pub dpi: u32,
}

enum Command {
    Target(EffectTarget),
    Fade,
    Cancel,
}

/// Every producer and the consumer acquire exclusive ownership using one swap.
/// The mailbox retains at most one command, with no allocation/lock on its reader.
struct Latest {
    pending: AtomicPtr<Command>,
}

impl Latest {
    fn new() -> Self {
        Self {
            pending: AtomicPtr::new(std::ptr::null_mut()),
        }
    }

    fn publish(&self, command: Command) -> bool {
        let next = Box::into_raw(Box::new(command));
        let previous = self.pending.swap(next, Ordering::AcqRel);
        if previous.is_null() {
            false
        } else {
            // A pointer returned by swap belongs to this caller alone.
            unsafe { drop(Box::from_raw(previous)) };
            true
        }
    }

    fn take(&self) -> Option<Command> {
        let pending = self.pending.swap(std::ptr::null_mut(), Ordering::AcqRel);
        if pending.is_null() {
            None
        } else {
            Some(unsafe { *Box::from_raw(pending) })
        }
    }
}

impl Drop for Latest {
    fn drop(&mut self) {
        let _ = self.take();
    }
}

#[derive(Clone, Copy, Debug)]
struct Geometry {
    bounds: Rect,
    left_edge: f32,
    right_edge: f32,
    top: f32,
    bottom: f32,
    scale: f32,
}

fn intensity_value(intensity: &MotionIntensity) -> u8 {
    match intensity {
        MotionIntensity::Off => 0,
        MotionIntensity::Light => 1,
        MotionIntensity::Standard => 2,
        MotionIntensity::Enhanced => 3,
    }
}

fn particle_count(intensity: u8) -> usize {
    match intensity {
        1 => 16,
        2 => 24,
        3 => 28,
        _ => 0,
    }
}

/// Only the target's right and Dock's left edges participate. Screen coordinates
/// remain physical pixels, including monitors left/above the primary monitor.
fn geometry(target: &EffectTarget, work: Rect) -> Option<Geometry> {
    Rect::new(work.left, work.top, work.right, work.bottom)?;
    Rect::new(
        target.own.left,
        target.own.top,
        target.own.right,
        target.own.bottom,
    )?;
    Rect::new(
        target.target.left,
        target.target.top,
        target.target.right,
        target.target.bottom,
    )?;
    let gap = i64::from(target.own.left) - i64::from(target.target.right);
    if target.target_id.is_empty()
        || target.threshold <= 0
        || target.threshold > 512
        || target.distance < 0
        || target.distance > target.threshold
        // The actor also verifies a cursor near a maximized target's edge when
        // Windows clamps the drag before the Dock edge can leave its contents.
        || gap < -i64::from(target.own.width())
        || gap > i64::from(target.threshold)
    {
        return None;
    }
    let scale = target.dpi.clamp(48, 768) as f32 / 96.0;
    let margin = (8.0 * scale).ceil() as i32;
    let overlap_top = target.target.top.max(target.own.top).max(work.top);
    let overlap_bottom = target.target.bottom.min(target.own.bottom).min(work.bottom);
    if i64::from(overlap_bottom) - i64::from(overlap_top) < i64::from(margin) * 3 {
        return None;
    }
    let left = target
        .target
        .right
        .min(target.own.left)
        .checked_sub(margin)?
        .max(work.left);
    let right = target
        .target
        .right
        .max(target.own.left)
        .checked_add(margin)?
        .min(work.right);
    let width = right.checked_sub(left)?;
    if width <= 0 || width > 1024 {
        return None;
    }
    let max_height = (MAX_PIXELS / width as usize).min(4096) as i32;
    let height = (overlap_bottom - overlap_top).min(max_height);
    let top = overlap_top.checked_add((overlap_bottom - overlap_top - height) / 2)?;
    let bounds = Rect::new(left, top, right, top.checked_add(height)?)?;
    Some(Geometry {
        bounds,
        left_edge: (i64::from(target.target.right) - i64::from(left)) as f32,
        right_edge: (i64::from(target.own.left) - i64::from(left)) as f32,
        top: margin as f32,
        bottom: (height - margin) as f32,
        scale,
    })
}

fn distance_opacity(distance: i32, threshold: i32) -> f32 {
    if threshold <= 0 || distance < 0 || distance > threshold {
        return 0.0;
    }
    let near = 1.0 - distance as f32 / threshold as f32;
    0.18 + 0.62 * near * near
}

fn fade_opacity(elapsed: Duration) -> f32 {
    (1.0 - elapsed.as_secs_f32() / DISSIPATE.as_secs_f32()).clamp(0.0, 1.0)
}

fn seed(id: &str) -> f32 {
    let hash = id.bytes().fold(2_166_136_261_u32, |hash, byte| {
        (hash ^ u32::from(byte)).wrapping_mul(16_777_619)
    });
    (hash % 997) as f32 / 997.0
}

/// In-place premultiplied BGRA. Unpainted pixels have all four channels zero.
fn render(
    pixels: &mut [u8],
    shape: Geometry,
    target: &EffectTarget,
    elapsed: Duration,
    fade: f32,
    intensity: u8,
) {
    pixels.fill(0);
    let width = shape.bounds.width() as usize;
    let height = shape.bounds.height() as usize;
    if width * height * 4 != pixels.len() || particle_count(intensity) == 0 {
        return;
    }
    let time = elapsed.as_secs_f32();
    let base = seed(&target.target_id);
    let opacity = distance_opacity(target.distance, target.threshold) * fade.clamp(0.0, 1.0);
    let opacity = opacity * if intensity == 1 { 0.60 } else { 1.0 };
    let count = particle_count(intensity);
    for index in 0..count {
        let phase = (base + index as f32 * 0.618_034 + time * 0.55).fract();
        let flow = (base + index as f32 * 0.381_966 + time * 0.24).fract();
        let y = shape.top + (shape.bottom - shape.top) * flow;
        // A pair of edge wisps accompanies particles moving through the gap.
        let x = match index % 4 {
            0 => shape.left_edge + (time * 2.8 + index as f32).sin() * shape.scale * 1.8,
            1 => shape.right_edge + (time * 2.5 + index as f32).cos() * shape.scale * 1.8,
            _ => shape.left_edge + (shape.right_edge - shape.left_edge) * phase,
        };
        let pulse = 0.55 + 0.45 * (phase * std::f32::consts::PI).sin();
        let radius = shape.scale * (1.2 + (index % 3) as f32 * 0.25);
        let color = if index % 7 == 0 {
            [255, 230, 170]
        } else {
            [170, 226, 255]
        };
        disk(
            pixels,
            width,
            height,
            x,
            y,
            radius * 3.3,
            color,
            opacity * pulse * 0.15,
        );
        disk(pixels, width, height, x, y, radius, color, opacity * pulse);
    }
}

#[allow(clippy::too_many_arguments)]
fn disk(
    pixels: &mut [u8],
    width: usize,
    height: usize,
    x: f32,
    y: f32,
    radius: f32,
    rgb: [u8; 3],
    opacity: f32,
) {
    let left = ((x - radius).floor() as i32).max(0) as usize;
    let right = ((x + radius).ceil() as i32).clamp(0, width as i32) as usize;
    let top = ((y - radius).floor() as i32).max(0) as usize;
    let bottom = ((y + radius).ceil() as i32).clamp(0, height as i32) as usize;
    for row in top..bottom {
        for column in left..right {
            let dx = column as f32 + 0.5 - x;
            let dy = row as f32 + 0.5 - y;
            let radial = (1.0 - (dx * dx + dy * dy) / (radius * radius)).clamp(0.0, 1.0);
            let alpha = (opacity * radial * 255.0).round().clamp(0.0, 255.0) as u16;
            if alpha == 0 {
                continue;
            }
            let pixel = &mut pixels[(row * width + column) * 4..][..4];
            let inverse = 255 - alpha;
            for (channel, color) in pixel[..3].iter_mut().zip(rgb.into_iter().rev()) {
                *channel =
                    ((u16::from(color) * alpha + u16::from(*channel) * inverse + 127) / 255) as u8;
            }
            pixel[3] = (alpha + (u16::from(pixel[3]) * inverse + 127) / 255) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target() -> EffectTarget {
        EffectTarget {
            own: Rect::new(-888, 20, -480, 800).unwrap(),
            target: Rect::new(-1800, -40, -900, 900).unwrap(),
            target_id: "target-life-1".into(),
            distance: 12,
            threshold: 64,
            dpi: 144,
        }
    }

    #[test]
    fn overlay_preserves_negative_coordinates_and_clips_work_area() {
        let target = target();
        let work = Rect::new(-1920, 50, 0, 760).unwrap();
        let shape = geometry(&target, work).unwrap();
        assert_eq!(shape.bounds.tuple(), (-912, 50, -876, 760));
        assert_eq!(shape.left_edge, 12.0);
        assert_eq!(shape.right_edge, 24.0);
        assert_eq!(shape.scale, 1.5);
    }

    #[test]
    fn overlay_rejects_wrong_edge_stale_distance_overflow_and_no_overlap() {
        let work = Rect::new(-1920, -100, 0, 980).unwrap();
        let mut target = target();
        target.own = Rect::new(-1800, 20, -1400, 800).unwrap();
        assert!(geometry(&target, work).is_none());
        target = self::target();
        target.distance = 65;
        assert!(geometry(&target, work).is_none());
        target = self::target();
        target.own = Rect::new(-888, 1000, -480, 1400).unwrap();
        assert!(geometry(&target, work).is_none());
        target = self::target();
        target.target.left = i32::MIN;
        target.target.right = i32::MAX;
        assert!(geometry(&target, work).is_none());
        target = self::target();
        target.target_id.clear();
        assert!(geometry(&target, work).is_none());
    }

    #[test]
    fn verified_maximized_candidate_reverses_flow_and_clips_partially_offscreen_dock() {
        let target = EffectTarget {
            own: Rect::new(2727, 0, 3611, 1309).unwrap(),
            target: Rect::new(0, 0, 2880, 1704).unwrap(),
            target_id: "verified-maximized-edge".into(),
            distance: 1,
            threshold: 80,
            dpi: 192,
        };
        let shape = geometry(&target, Rect::new(0, 0, 2880, 1704).unwrap()).unwrap();
        assert_eq!(shape.bounds.tuple(), (2711, 0, 2880, 1309));
        assert_eq!((shape.left_edge, shape.right_edge), (169.0, 16.0));
        let mut pixels =
            vec![0; shape.bounds.width() as usize * shape.bounds.height() as usize * 4];
        render(
            &mut pixels,
            shape,
            &target,
            Duration::from_millis(100),
            1.0,
            2,
        );
        assert!(pixels.chunks_exact(4).any(|pixel| pixel[3] != 0));
        assert!(pixels
            .chunks_exact(4)
            .all(|pixel| pixel[..3].iter().all(|channel| *channel <= pixel[3])));
        let mut outside = target.clone();
        outside.own = Rect::new(1000, 0, 1884, 1309).unwrap();
        assert!(geometry(&outside, Rect::new(0, 0, 2880, 1704).unwrap()).is_none());
        let mut distant = target.clone();
        distant.distance = 81;
        assert!(geometry(&distant, Rect::new(0, 0, 2880, 1704).unwrap()).is_none());
        assert!(geometry(&target, Rect::new(0, 0, 1200, 1704).unwrap()).is_none());
    }

    #[test]
    fn allocations_and_counts_have_hard_limits_at_extreme_dpi() {
        let mut target = target();
        target.dpi = u32::MAX;
        target.target.bottom = 100_000;
        target.own.bottom = 100_000;
        let shape = geometry(&target, Rect::new(-1920, -100, 0, 100_000).unwrap()).unwrap();
        assert!(shape.bounds.width() as usize * shape.bounds.height() as usize <= MAX_PIXELS);
        assert_eq!([0, 1, 2, 3, 255].map(particle_count), [0, 16, 24, 28, 0]);
    }

    #[test]
    fn attraction_and_cancellation_have_bounded_monotonic_alpha() {
        assert!(distance_opacity(0, 64) > distance_opacity(32, 64));
        assert!(distance_opacity(32, 64) > distance_opacity(64, 64));
        assert_eq!(distance_opacity(65, 64), 0.0);
        assert_eq!(fade_opacity(Duration::ZERO), 1.0);
        assert_eq!(fade_opacity(Duration::from_millis(80)), 0.5);
        assert_eq!(fade_opacity(Duration::from_millis(160)), 0.0);
        assert_eq!(fade_opacity(Duration::from_secs(1)), 0.0);
    }

    #[test]
    fn frames_are_premultiplied_transparent_and_change_with_time() {
        let target = target();
        let shape = geometry(&target, Rect::new(-1920, -100, 0, 980).unwrap()).unwrap();
        let mut pixels =
            vec![0; shape.bounds.width() as usize * shape.bounds.height() as usize * 4];
        render(&mut pixels, shape, &target, Duration::ZERO, 1.0, 2);
        assert!(pixels.chunks_exact(4).any(|pixel| pixel[3] != 0));
        assert!(pixels.chunks_exact(4).any(|pixel| pixel == [0, 0, 0, 0]));
        assert!(pixels
            .chunks_exact(4)
            .all(|pixel| pixel[..3].iter().all(|c| *c <= pixel[3])));
        let first = pixels.clone();
        render(
            &mut pixels,
            shape,
            &target,
            Duration::from_millis(100),
            1.0,
            2,
        );
        assert_ne!(pixels, first);
        render(&mut pixels, shape, &target, Duration::ZERO, 0.0, 2);
        assert!(pixels.iter().all(|byte| *byte == 0));
        render(&mut pixels, shape, &target, Duration::ZERO, 1.0, 0);
        assert!(pixels.iter().all(|byte| *byte == 0));
    }

    #[test]
    fn mailbox_keeps_only_latest_and_cancel_supersedes_queued_particles() {
        let latest = Latest::new();
        assert!(!latest.publish(Command::Target(target())));
        for _ in 0..1000 {
            assert!(latest.publish(Command::Target(target())));
        }
        assert!(latest.publish(Command::Cancel));
        assert!(matches!(latest.take(), Some(Command::Cancel)));
        assert!(latest.take().is_none());
    }

    #[test]
    fn mailbox_transfers_unique_owned_values_across_threads() {
        use std::sync::{atomic::AtomicBool, Arc};
        let latest = Arc::new(Latest::new());
        let finished = Arc::new(AtomicBool::new(false));
        let producer = latest.clone();
        let done = finished.clone();
        let writer = std::thread::spawn(move || {
            for index in 0..2000 {
                let mut next = target();
                next.target_id = index.to_string();
                producer.publish(Command::Target(next));
            }
            done.store(true, Ordering::Release);
        });
        let mut observed = Vec::new();
        loop {
            let complete = finished.load(Ordering::Acquire);
            match latest.take() {
                Some(Command::Target(target)) => {
                    observed.push(target.target_id.parse::<usize>().unwrap())
                }
                None if complete => break,
                _ => std::thread::yield_now(),
            }
        }
        writer.join().unwrap();
        assert!(!observed.is_empty());
        assert!(observed.windows(2).all(|pair| pair[0] < pair[1]));
        assert_eq!(observed.last(), Some(&1999));
    }
}
