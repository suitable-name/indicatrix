//! Converts a fixed-length tilt-angle curve into an SVG path-data string for
//! `Path::commands` in `performance_graph_dialog.slint`. Slint has no polyline
//! primitive, so the dialog draws each optical channel (brilliance, windowing,
//! extinction) as a `Path` scaled into a fixed 0..100 x 0..100 viewbox.
//!
//! Two curve shapes exist side by side:
//! - [`tilt_curve_path`] / [`SAMPLE_COUNT`] (19 samples, one per 5° step, `0..=90°`):
//!   the always-available canonical (azimuth-0) sweep the render thread computes every
//!   frame, also consumed by `settings_dialog.slint`'s mini tilt chart.
//! - [`full_axis_curve_path`] / [`FULL_AXIS_SAMPLE_COUNT`] (181 samples, one per 1°
//!   step, `-90..=+90°`): the full-axis sweep `gui::tilt_profile` computes off the UI
//!   thread for all four [`indicatrix::color::metrics::PROFILE_AZIMUTHS_DEG`] axes.
//!
//! Both go through [`curve_path_generic`], which only cares about array length and
//! value, not what angle each index represents -- the x-axis domain is purely a
//! labelling concern for whoever draws the tick marks.

use std::fmt::Write as _;

/// Number of tilt-angle samples (0°, 5°, ..., 90°) -- matches `graph_brilliance` and
/// friends in `app.slint` / `gem_viewport.slint` / `performance_graph_dialog.slint`.
const SAMPLE_COUNT: usize = 19;

/// Number of full-axis tilt-angle samples (-90°, -89°, ..., 0°, ..., 90°) -- matches
/// `indicatrix::color::metrics::TILT_ANGLES_DEG` and the `graph_*_extra_axes`/
/// `graph_*_extra_paths` rows `gui::tilt_profile` populates.
pub const FULL_AXIS_SAMPLE_COUNT: usize = 181;

/// Builds an `M x y L x y L x y ...` SVG path string from `values`, laid out in a
/// 0..100 x 0..100 viewbox. `x` sweeps evenly across the `N` samples so the first and
/// last points sit exactly on the box's left/right edges; `y` is `100 - value` so a
/// value of 100 lands at the top of the box (Slint's `Path` grows downward, the
/// opposite of `values`' percentage sense).
///
/// `values` are clamped to 0..100 before conversion: they're percentages and should
/// already be in range, but a stray out-of-range sample must not draw outside the
/// chart's clipped `Rectangle`.
///
/// Shared by [`tilt_curve_path`] and [`full_axis_curve_path`] via const generics, so
/// the two curve shapes can never silently drift in how they lay out `x`.
fn curve_path_generic<const N: usize>(values: &[f32; N]) -> String {
    let mut commands = String::new();
    for (i, &value) in values.iter().enumerate() {
        if i > 0 {
            commands.push(' ');
        }
        let x = i as f32 * 100.0 / (N - 1) as f32;
        let y = 100.0 - value.clamp(0.0, 100.0);
        let cmd = if i == 0 { 'M' } else { 'L' };
        let _ = write!(commands, "{cmd} {x:.2} {y:.2}");
    }
    commands
}

/// 19-sample (5°-step, `0..=90°`) curve path -- see this module's doc comment for how
/// this differs from [`full_axis_curve_path`].
pub fn tilt_curve_path(values: &[f32; SAMPLE_COUNT]) -> String {
    curve_path_generic(values)
}

/// 181-sample (1°-step, `-90..=+90°`) full-axis curve path -- see
/// `indicatrix::color::metrics::TILT_ANGLES_DEG` for what each index represents.
pub fn full_axis_curve_path(values: &[f32; FULL_AXIS_SAMPLE_COUNT]) -> String {
    curve_path_generic(values)
}

#[cfg(test)]
mod tests {
    use super::{FULL_AXIS_SAMPLE_COUNT, SAMPLE_COUNT, full_axis_curve_path, tilt_curve_path};

    #[test]
    fn known_input_produces_expected_path() {
        let values = [0.0f32; SAMPLE_COUNT];
        let path = tilt_curve_path(&values);
        assert_eq!(
            path,
            "M 0.00 100.00 L 5.56 100.00 L 11.11 100.00 L 16.67 100.00 L 22.22 100.00 \
             L 27.78 100.00 L 33.33 100.00 L 38.89 100.00 L 44.44 100.00 L 50.00 100.00 \
             L 55.56 100.00 L 61.11 100.00 L 66.67 100.00 L 72.22 100.00 L 77.78 100.00 \
             L 83.33 100.00 L 88.89 100.00 L 94.44 100.00 L 100.00 100.00"
        );
    }

    #[test]
    fn first_and_last_points_span_full_width() {
        let values = [42.0f32; SAMPLE_COUNT];
        let path = tilt_curve_path(&values);
        let tokens: Vec<&str> = path.split_whitespace().collect();
        // Each point is 3 tokens (command, x, y); 19 points -> 57 tokens.
        assert_eq!(tokens.len(), SAMPLE_COUNT * 3);
        assert_eq!(tokens[1], "0.00", "first sample must sit at x=0");
        assert_eq!(
            tokens[tokens.len() - 2],
            "100.00",
            "last sample must sit at x=100"
        );
    }

    #[test]
    fn value_100_maps_to_top_and_0_maps_to_bottom() {
        let mut values = [50.0f32; SAMPLE_COUNT];
        values[0] = 100.0;
        values[1] = 0.0;
        let path = tilt_curve_path(&values);
        let tokens: Vec<&str> = path.split_whitespace().collect();
        // First point is tokens[0..3] = ["M", x, y]; second is tokens[3..6] = ["L", x, y].
        assert_eq!(tokens[2], "0.00", "value 100 must map to y=0 (top)");
        assert_eq!(tokens[5], "100.00", "value 0 must map to y=100 (bottom)");
    }

    #[test]
    fn out_of_range_values_are_clamped() {
        let mut values = [50.0f32; SAMPLE_COUNT];
        values[0] = 150.0; // above the 0..100 range
        values[1] = -20.0; // below the 0..100 range
        let path = tilt_curve_path(&values);
        let tokens: Vec<&str> = path.split_whitespace().collect();
        assert_eq!(tokens[2], "0.00", "150 must clamp to 100 -> y=0");
        assert_eq!(tokens[5], "100.00", "-20 must clamp to 0 -> y=100");
    }

    // ---- full_axis_curve_path (181 samples, -90..=+90 deg) ----

    #[test]
    fn full_axis_first_and_last_points_span_full_width() {
        let values = [42.0f32; FULL_AXIS_SAMPLE_COUNT];
        let path = full_axis_curve_path(&values);
        let tokens: Vec<&str> = path.split_whitespace().collect();
        // Each point is 3 tokens (command, x, y); 181 points -> 543 tokens.
        assert_eq!(tokens.len(), FULL_AXIS_SAMPLE_COUNT * 3);
        assert_eq!(tokens[1], "0.00", "first sample (-90 deg) must sit at x=0");
        assert_eq!(
            tokens[tokens.len() - 2],
            "100.00",
            "last sample (+90 deg) must sit at x=100"
        );
    }

    #[test]
    fn full_axis_midpoint_sits_at_the_horizontal_center() {
        // Index 90 is the shared zero-degree (table-up) point; with 181 evenly spaced
        // samples it must land exactly at the plot's horizontal midpoint.
        let values = [50.0f32; FULL_AXIS_SAMPLE_COUNT];
        let path = full_axis_curve_path(&values);
        let tokens: Vec<&str> = path.split_whitespace().collect();
        // Point 90 (0-indexed) occupies tokens[90*3 .. 90*3+3].
        assert_eq!(tokens[90 * 3 + 1], "50.00");
    }

    #[test]
    fn full_axis_value_100_maps_to_top_and_0_maps_to_bottom() {
        let mut values = [50.0f32; FULL_AXIS_SAMPLE_COUNT];
        values[0] = 100.0;
        values[FULL_AXIS_SAMPLE_COUNT - 1] = 0.0;
        let path = full_axis_curve_path(&values);
        let tokens: Vec<&str> = path.split_whitespace().collect();
        assert_eq!(tokens[2], "0.00", "value 100 must map to y=0 (top)");
        assert_eq!(
            tokens[tokens.len() - 1],
            "100.00",
            "value 0 must map to y=100 (bottom)"
        );
    }

    #[test]
    fn full_axis_out_of_range_values_are_clamped() {
        let mut values = [50.0f32; FULL_AXIS_SAMPLE_COUNT];
        values[0] = 150.0; // above the 0..100 range
        values[1] = -20.0; // below the 0..100 range
        let path = full_axis_curve_path(&values);
        let tokens: Vec<&str> = path.split_whitespace().collect();
        assert_eq!(tokens[2], "0.00", "150 must clamp to 100 -> y=0");
        assert_eq!(tokens[5], "100.00", "-20 must clamp to 0 -> y=100");
    }
}
