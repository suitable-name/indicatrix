//! Pure resolution logic for the optional local preview-then-settle rendering.
//!
//! `bridge::handoff::HandoffMachine` already gives the REMOTE path this feel: while the
//! camera moves, the viewport renders locally at low quality; once it settles, a
//! full-quality render is requested from a remote worker. This module gives the same
//! feel with no worker at all -- while moving, [`effective_dimensions`] returns a
//! fraction of the configured resolution (per [`LocalPreviewScale`]); once settled, it
//! returns the configured resolution unchanged. `spawn_render_thread`'s loop feeds this
//! into `update_accumulation_state`, which already reallocates buffers and resets
//! `accum_samples` whenever dimensions change -- so the preview<->full transition costs
//! nothing extra to implement.
//!
//! Pure integer arithmetic with no Slint dependency, exercised directly by unit tests.

use crate::settings::model::LocalPreviewScale;

/// Resolves the `width x height` the render loop should trace THIS frame, from the
/// configured `width x height`, the [`LocalPreviewScale`] choice, and whether the
/// camera is currently `moving`.
///
/// Returns `(width, height)` unchanged whenever `scale` is [`LocalPreviewScale::Off`]
/// or `moving` is `false`. Only `moving && scale != Off` reduces the dimensions,
/// floored at `1x1` so a tiny configured resolution never resolves to a zero-area
/// frame -- same floor `PreviewScale::resolve` applies for the remote `PREVIEW` case.
#[must_use]
pub fn effective_dimensions(
    width: u32,
    height: u32,
    scale: LocalPreviewScale,
    moving: bool,
) -> (u32, u32) {
    if !moving {
        return (width, height);
    }
    let divisor = scale.divisor();
    if divisor <= 1 {
        return (width, height);
    }
    ((width / divisor).max(1), (height / divisor).max(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_never_reduces_regardless_of_movement() {
        assert_eq!(
            effective_dimensions(1920, 1080, LocalPreviewScale::Off, true),
            (1920, 1080)
        );
        assert_eq!(
            effective_dimensions(1920, 1080, LocalPreviewScale::Off, false),
            (1920, 1080)
        );
    }

    #[test]
    fn settled_always_returns_the_full_configured_resolution() {
        for scale in [
            LocalPreviewScale::Off,
            LocalPreviewScale::Half,
            LocalPreviewScale::Quarter,
        ] {
            assert_eq!(
                effective_dimensions(1280, 720, scale, false),
                (1280, 720),
                "scale={scale:?}"
            );
        }
    }

    #[test]
    fn half_scale_halves_only_while_moving() {
        assert_eq!(
            effective_dimensions(1280, 720, LocalPreviewScale::Half, true),
            (640, 360)
        );
        assert_eq!(
            effective_dimensions(1280, 720, LocalPreviewScale::Half, false),
            (1280, 720)
        );
    }

    #[test]
    fn quarter_scale_quarters_only_while_moving() {
        assert_eq!(
            effective_dimensions(1280, 720, LocalPreviewScale::Quarter, true),
            (320, 180)
        );
        assert_eq!(
            effective_dimensions(1280, 720, LocalPreviewScale::Quarter, false),
            (1280, 720)
        );
    }

    /// A tiny configured resolution must never resolve to a zero-area preview frame.
    #[test]
    fn tiny_configured_dimensions_floor_at_one_by_one_while_moving() {
        assert_eq!(
            effective_dimensions(2, 2, LocalPreviewScale::Quarter, true),
            (1, 1)
        );
        assert_eq!(
            effective_dimensions(1, 1, LocalPreviewScale::Half, true),
            (1, 1)
        );
    }

    /// Non-power-of-two configured dimensions must floor-divide, not panic or round up.
    #[test]
    fn odd_configured_dimensions_floor_divide() {
        assert_eq!(
            effective_dimensions(801, 601, LocalPreviewScale::Half, true),
            (400, 300)
        );
    }
}
