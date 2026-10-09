//! The user brush of the Masks step: clicks that exclude or restore round patches of a view's
//! working-resolution mask.
//!
//! Only the `USER` flag is touched; the automatic flags (saturated,
//! below the noise floor, outside the outline, edge band, inclusion, ghost) are not the
//! brush's to clear.

use indicatrix_cut_core::rough_plan::photometry::{PixelMask, flag};

/// What a click does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrushMode {
    /// Mark the patch as unusable.
    Exclude,
    /// Take the user mark off the patch again.
    Restore,
}

/// The largest brush radius in working pixels.
pub const MAX_BRUSH_RADIUS_PX: f64 = 40.0;

/// The working-pixel position under a click given as fractions of the picture (0 to 1), or
/// `None` outside the picture.
#[must_use]
pub fn working_position(fx: f32, fy: f32, width: usize, height: usize) -> Option<[f64; 2]> {
    if width == 0 || height == 0 || !fx.is_finite() || !fy.is_finite() {
        return None;
    }
    if !(0.0..=1.0).contains(&fx) || !(0.0..=1.0).contains(&fy) {
        return None;
    }
    Some([
        (f64::from(fx) * width as f64).min(width as f64 - 1e-6),
        (f64::from(fy) * height as f64).min(height as f64 - 1e-6),
    ])
}

/// Applies one click at working position `centre` with `radius_px` (clamped to
/// `0.5..=MAX_BRUSH_RADIUS_PX`). A pixel is in the patch when its centre is within the
/// radius.
///
/// Returns how many pixels changed.
pub fn apply(mask: &mut PixelMask, centre: [f64; 2], radius_px: f64, mode: BrushMode) -> usize {
    let radius = if radius_px.is_finite() {
        radius_px.clamp(0.5, MAX_BRUSH_RADIUS_PX)
    } else {
        0.5
    };
    let (width, height) = (mask.width(), mask.height());
    if width == 0 || height == 0 {
        return 0;
    }
    let x_lo = (centre[0] - radius).floor().max(0.0) as usize;
    let y_lo = (centre[1] - radius).floor().max(0.0) as usize;
    let x_hi = ((centre[0] + radius).ceil().max(0.0) as usize).min(width - 1);
    let y_hi = ((centre[1] + radius).ceil().max(0.0) as usize).min(height - 1);
    let mut changed = 0;
    for y in y_lo..=y_hi {
        for x in x_lo..=x_hi {
            let dx = x as f64 + 0.5 - centre[0];
            let dy = y as f64 + 0.5 - centre[1];
            if dx.hypot(dy) > radius {
                continue;
            }
            let had = mask.has(x, y, flag::USER);
            let want = matches!(mode, BrushMode::Exclude);
            if had != want {
                mask.set_user(x, y, want);
                changed += 1;
            }
        }
    }
    changed
}

/// Removes every user mark.
pub fn clear_user(mask: &mut PixelMask) -> usize {
    let mut cleared = 0;
    for y in 0..mask.height() {
        for x in 0..mask.width() {
            if mask.has(x, y, flag::USER) {
                mask.set_user(x, y, false);
                cleared += 1;
            }
        }
    }
    cleared
}

/// The user marks of `source` copied into `target` (after the automatic flags were computed
/// again, the brush work is not lost). Sizes must match; otherwise nothing is copied.
pub fn carry_user_marks(source: &PixelMask, target: &mut PixelMask) -> usize {
    if source.width() != target.width() || source.height() != target.height() {
        return 0;
    }
    let mut copied = 0;
    for y in 0..source.height() {
        for x in 0..source.width() {
            if source.has(x, y, flag::USER) {
                target.set_user(x, y, true);
                copied += 1;
            }
        }
    }
    copied
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_click_excludes_a_round_patch() {
        let mut mask = PixelMask::new(20, 20);
        let changed = apply(&mut mask, [10.0, 10.0], 3.0, BrushMode::Exclude);
        assert_eq!(mask.count(flag::USER), changed);
        // About pi r^2 pixels.
        assert!((24..=34).contains(&changed), "{changed}");
        assert!(mask.has(10, 10, flag::USER));
        assert!(!mask.has(10, 15, flag::USER));
        assert!(!mask.has(0, 0, flag::USER));
    }

    #[test]
    fn restoring_removes_only_the_user_flag() {
        let mut mask = PixelMask::new(10, 10);
        mask.set(5, 5, flag::SATURATED);
        apply(&mut mask, [5.5, 5.5], 2.0, BrushMode::Exclude);
        assert!(mask.has(5, 5, flag::USER));
        assert!(mask.has(5, 5, flag::SATURATED));
        apply(&mut mask, [5.5, 5.5], 2.0, BrushMode::Restore);
        assert!(!mask.has(5, 5, flag::USER));
        assert!(mask.has(5, 5, flag::SATURATED), "automatic flags stay");
        assert_eq!(mask.count(flag::USER), 0);
    }

    #[test]
    fn the_brush_is_clipped_at_the_edges() {
        let mut mask = PixelMask::new(8, 8);
        let changed = apply(&mut mask, [0.0, 0.0], 3.0, BrushMode::Exclude);
        assert!(changed > 0);
        assert!(changed < 28);
        assert!(mask.has(0, 0, flag::USER));
        let none = apply(&mut mask, [100.0, 100.0], 3.0, BrushMode::Exclude);
        assert_eq!(none, 0);
    }

    #[test]
    fn repeating_a_click_changes_nothing_the_second_time() {
        let mut mask = PixelMask::new(12, 12);
        let first = apply(&mut mask, [6.0, 6.0], 2.5, BrushMode::Exclude);
        let second = apply(&mut mask, [6.0, 6.0], 2.5, BrushMode::Exclude);
        assert!(first > 0);
        assert_eq!(second, 0);
    }

    #[test]
    fn the_radius_is_clamped() {
        let mut mask = PixelMask::new(200, 200);
        let big = apply(&mut mask, [100.0, 100.0], 1.0e9, BrushMode::Exclude);
        let limit = (std::f64::consts::PI * MAX_BRUSH_RADIUS_PX * MAX_BRUSH_RADIUS_PX) as usize;
        assert!(big <= limit + 200, "{big} vs {limit}");
        let mut other = PixelMask::new(20, 20);
        let tiny = apply(&mut other, [10.0, 10.0], f64::NAN, BrushMode::Exclude);
        assert!(tiny >= 1);
    }

    #[test]
    fn clicks_map_to_working_pixels() {
        assert_eq!(working_position(0.5, 0.5, 100, 50), Some([50.0, 25.0]));
        let end = working_position(1.0, 1.0, 100, 50).unwrap();
        assert!(end[0] < 100.0 && end[1] < 50.0);
        assert_eq!(working_position(-0.1, 0.5, 100, 50), None);
        assert_eq!(working_position(0.5, 1.5, 100, 50), None);
        assert_eq!(working_position(f32::NAN, 0.5, 100, 50), None);
        assert_eq!(working_position(0.5, 0.5, 0, 50), None);
    }

    #[test]
    fn clearing_and_carrying_user_marks() {
        let mut mask = PixelMask::new(10, 10);
        apply(&mut mask, [3.0, 3.0], 2.0, BrushMode::Exclude);
        let marks = mask.count(flag::USER);
        let mut fresh = PixelMask::new(10, 10);
        fresh.set(1, 1, flag::EDGE_BAND);
        assert_eq!(carry_user_marks(&mask, &mut fresh), marks);
        assert!(fresh.has(1, 1, flag::EDGE_BAND));
        assert_eq!(fresh.count(flag::USER), marks);
        let mut wrong_size = PixelMask::new(5, 5);
        assert_eq!(carry_user_marks(&mask, &mut wrong_size), 0);
        assert_eq!(clear_user(&mut mask), marks);
        assert_eq!(mask.count(flag::USER), 0);
    }
}
