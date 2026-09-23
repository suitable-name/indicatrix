//! Pure parameter validation and frame-pose enumeration for the tilt performance video
//! export: the angle sweep (start/end/step), the resolution preset table, and the
//! zero-padded frame-numbering width. Deliberately free of any Slint/render dependency
//! so `mod.rs`'s wiring and `render.rs`'s per-frame loop build on the exact same
//! numbers the dialog itself shows via `TiltVideoExportModel.frame_count`.

/// Minimum tilt-angle step the dialog must accept, in degrees -- the owner's own
/// requirement is that any value in `0.01..=1.0` works.
pub const MIN_STEP_DEG: f64 = 0.01;
/// Maximum step accepted at all. Above `1.0` this is only a "quick preview" allowance,
/// not a hard requirement, but still clamped rather than left unbounded.
pub const MAX_STEP_DEG: f64 = 5.0;
/// Frame count above which the dialog must ask the user to confirm before starting.
pub const CONFIRM_FRAME_THRESHOLD: usize = 2000;

pub const MIN_ANGLE_DEG: f64 = -90.0;
pub const MAX_ANGLE_DEG: f64 = 90.0;

/// Clamps a requested step into the accepted range. A non-finite input (a half-typed
/// text field, e.g. `"-"` or `""` parsed as `NaN`) collapses to the `1.0` default
/// rather than propagating a `NaN` into `frame_count`'s division.
#[must_use]
pub const fn clamp_step_deg(step: f64) -> f64 {
    if step.is_finite() {
        step.clamp(MIN_STEP_DEG, MAX_STEP_DEG)
    } else {
        1.0
    }
}

/// Clamps a requested sweep endpoint into `[MIN_ANGLE_DEG, MAX_ANGLE_DEG]`.
#[must_use]
pub const fn clamp_angle_deg(angle: f64) -> f64 {
    if angle.is_finite() {
        angle.clamp(MIN_ANGLE_DEG, MAX_ANGLE_DEG)
    } else {
        0.0
    }
}

/// Number of frames the sweep `[start_deg, end_deg]` at `step_deg` produces:
/// `floor((end - start) / step) + 1`. `0` for a degenerate request (non-positive step,
/// or an end before the start).
#[must_use]
pub fn frame_count(start_deg: f64, end_deg: f64, step_deg: f64) -> usize {
    if step_deg <= 0.0 || end_deg < start_deg {
        return 0;
    }
    let span = end_deg - start_deg;
    // Epsilon guards against float division landing a hair under an exact multiple --
    // e.g. `180.0 / 0.01` can evaluate to `17999.999999999996` rather than `18000.0`,
    // which a bare `.floor()` would under-count by one frame.
    let steps = (span / step_deg + 1e-6).floor();
    if steps.is_finite() {
        (steps as usize).saturating_add(1)
    } else {
        0
    }
}

/// The exact fractional tilt angle, in degrees, for frame `index` of a `total`-frame
/// sweep (`total` must equal `frame_count(start_deg, end_deg, step_deg)`). The LAST
/// frame (`index + 1 == total`) is pinned to exactly `end_deg` rather than
/// `start_deg + step_deg * index`, so it lands on the end angle within float epsilon
/// regardless of accumulated step error -- required so the camera pose (and the
/// overlay's own angle readout) never snaps to the nearest 1° sweep-grid sample.
#[must_use]
pub const fn frame_angle_deg(
    start_deg: f64,
    end_deg: f64,
    step_deg: f64,
    index: usize,
    total: usize,
) -> f64 {
    if total == 0 {
        start_deg
    } else if index + 1 >= total {
        end_deg
    } else {
        step_deg.mul_add(index as f64, start_deg)
    }
}

/// The full list of exact fractional tilt angles a sweep visits, in frame order.
#[must_use]
pub fn frame_angles(start_deg: f64, end_deg: f64, step_deg: f64) -> Vec<f64> {
    let total = frame_count(start_deg, end_deg, step_deg);
    (0..total)
        .map(|i| frame_angle_deg(start_deg, end_deg, step_deg, i, total))
        .collect()
}

/// How many decimal digits `total_frames` needs so every frame's zero-padded filename
/// (`frame_00001.png`) is exactly as wide as the LAST one -- e.g. `18001` frames need
/// width `5`.
#[must_use]
pub fn frame_number_digits(total_frames: usize) -> usize {
    total_frames.max(1).to_string().len()
}

/// One PNG frame's filename inside the export's frame subfolder, zero-padded to
/// [`frame_number_digits`] of `total_frames` -- `index` is 1-based (`frame_00001.png`
/// for the first frame), matching the `frame_%0Nd.png` pattern `encode::mux_mp4` feeds
/// ffmpeg.
#[must_use]
pub fn frame_file_name(index_one_based: usize, total_frames: usize) -> String {
    let digits = frame_number_digits(total_frames);
    format!("frame_{index_one_based:0digits$}.png")
}

/// One of the fixed resolution presets `TiltVideoExportModel.resolution_preset` indexes
/// into: seven square presets (128..8192), then five 16:9 presets (720p..8K) -- kept in
/// lock-step with the combo box's item order in `performance_graph_dialog.slint`.
pub const RESOLUTION_PRESETS: &[(u32, u32)] = &[
    (128, 128),
    (256, 256),
    (512, 512),
    (1024, 1024),
    (2048, 2048),
    (4096, 4096),
    (8192, 8192),
    (1280, 720),
    (1920, 1080),
    (2560, 1440),
    (3840, 2160),
    (7680, 4320),
];
/// The `resolution_preset` index meaning "Custom" (`custom_width`/`custom_height`) --
/// one past the end of [`RESOLUTION_PRESETS`].
pub const CUSTOM_RESOLUTION_INDEX: i32 = RESOLUTION_PRESETS.len() as i32;

const MIN_VIDEO_DIM: i32 = 128;
const MAX_VIDEO_DIM: i32 = 8192;

/// Resolves `preset_index`/`custom_width`/`custom_height` into the actual pixel size to
/// render. A custom size is clamped to `[128, 8192]` per axis independently -- `128`
/// (not the still-image export's `16`) since the smallest preset this dialog itself
/// offers is 128x128.
#[must_use]
pub fn resolve_resolution(preset_index: i32, custom_width: i32, custom_height: i32) -> (u32, u32) {
    if preset_index >= 0 && (preset_index as usize) < RESOLUTION_PRESETS.len() {
        RESOLUTION_PRESETS[preset_index as usize]
    } else {
        (
            custom_width.clamp(MIN_VIDEO_DIM, MAX_VIDEO_DIM) as u32,
            custom_height.clamp(MIN_VIDEO_DIM, MAX_VIDEO_DIM) as u32,
        )
    }
}

/// The `fps_index` pill (0/1/2) -> the actual frame rate.
#[must_use]
pub const fn resolve_fps(fps_index: i32) -> u32 {
    match fps_index {
        0 => 24,
        2 => 60,
        _ => 30,
    }
}

/// The bounce-ladder pill index (0..=5) -> the actual max-bounce cap -- the same
/// 4/8/12/24/64/128 ladder `export_dialog.slint`/`settings_dialog.slint` already use.
#[must_use]
pub const fn resolve_max_bounces(bounce_index: i32) -> u32 {
    match bounce_index {
        0 => 4,
        1 => 8,
        3 => 24,
        4 => 64,
        5 => 128,
        _ => 12,
    }
}

/// The `done`/`total` frame-count pair (`run::report_progress`'s own inputs) as a
/// `0.0..=1.0` completion fraction -- the number both `TiltVideoExportModel.
/// progress` (the dialog's own bar) and the status strip's `ActivityChip` (via
/// `ActivityModel.invoke_progress_external`) show, so the two can never
/// disagree. `total.max(1)` guards a
/// degenerate zero-frame request against a division by zero -- `frame_count`
/// itself already refuses to start a run at `0`, but this stays total rather than
/// assuming that guard always ran first.
#[must_use]
pub fn export_progress_fraction(done: usize, total: usize) -> f32 {
    done as f32 / total.max(1) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_progress_fraction_is_zero_before_any_frame() {
        assert_eq!(export_progress_fraction(0, 181), 0.0);
    }

    #[test]
    fn export_progress_fraction_is_one_on_the_last_frame() {
        assert_eq!(export_progress_fraction(181, 181), 1.0);
    }

    #[test]
    fn export_progress_fraction_is_proportional_partway_through() {
        assert_eq!(export_progress_fraction(50, 200), 0.25);
    }

    #[test]
    fn export_progress_fraction_never_divides_by_zero_for_a_degenerate_total() {
        assert_eq!(export_progress_fraction(0, 0), 0.0);
        assert_eq!(export_progress_fraction(5, 0), 5.0);
    }

    #[test]
    fn frame_count_matches_the_181_point_default_sweep() {
        // -90..=90 in 1 deg steps is exactly this dialog's own always-available sweep
        // resolution -- 181 points.
        assert_eq!(frame_count(-90.0, 90.0, 1.0), 181);
    }

    #[test]
    fn frame_count_reaches_18001_at_the_finest_required_step() {
        assert_eq!(frame_count(-90.0, 90.0, 0.01), 18001);
    }

    #[test]
    fn frame_count_is_zero_for_degenerate_requests() {
        assert_eq!(frame_count(0.0, -10.0, 1.0), 0, "end before start");
        assert_eq!(frame_count(-10.0, 10.0, 0.0), 0, "zero step");
        assert_eq!(frame_count(-10.0, 10.0, -1.0), 0, "negative step");
    }

    #[test]
    fn frame_count_handles_a_single_point_sweep() {
        assert_eq!(frame_count(5.0, 5.0, 1.0), 1);
    }

    /// Steps 0.01, 0.25 and 1.0 must
    /// each produce a frame list whose LAST angle lands on the end angle within 1e-9,
    /// not the nearest step-grid sample short of it.
    #[test]
    fn last_frame_lands_exactly_on_the_end_angle_for_every_required_step() {
        for step in [0.01, 0.25, 1.0] {
            let angles = frame_angles(-90.0, 90.0, step);
            let last = *angles.last().expect("non-empty sweep");
            assert!(
                (last - 90.0).abs() < 1e-9,
                "step {step}: last frame angle {last} did not land on 90.0 within 1e-9"
            );
            assert!(
                (angles[0] - -90.0).abs() < 1e-9,
                "step {step}: first frame must be the start angle"
            );
            assert_eq!(angles.len(), frame_count(-90.0, 90.0, step));
        }
    }

    #[test]
    fn frame_angles_are_monotonically_non_decreasing() {
        let angles = frame_angles(-10.0, 10.0, 0.3);
        for pair in angles.windows(2) {
            assert!(pair[1] >= pair[0]);
        }
    }

    #[test]
    fn frame_number_digits_matches_the_last_frames_own_width() {
        assert_eq!(frame_number_digits(181), 3);
        assert_eq!(frame_number_digits(18001), 5);
        assert_eq!(frame_number_digits(1), 1);
        assert_eq!(frame_number_digits(0), 1);
    }

    #[test]
    fn frame_file_name_zero_pads_to_the_total_width() {
        assert_eq!(frame_file_name(1, 181), "frame_001.png");
        assert_eq!(frame_file_name(181, 181), "frame_181.png");
        assert_eq!(frame_file_name(7, 18001), "frame_00007.png");
    }

    #[test]
    fn clamp_step_deg_clamps_and_rejects_nan() {
        assert_eq!(clamp_step_deg(0.001), MIN_STEP_DEG);
        assert_eq!(clamp_step_deg(100.0), MAX_STEP_DEG);
        assert_eq!(clamp_step_deg(0.25), 0.25);
        assert_eq!(clamp_step_deg(f64::NAN), 1.0);
    }

    #[test]
    fn resolve_resolution_uses_the_preset_table_in_range() {
        assert_eq!(resolve_resolution(0, 999, 999), (128, 128));
        assert_eq!(resolve_resolution(8, 999, 999), (1920, 1080));
    }

    #[test]
    fn resolve_resolution_falls_back_to_clamped_custom_dimensions() {
        assert_eq!(
            resolve_resolution(CUSTOM_RESOLUTION_INDEX, 64, 20000),
            (128, 8192)
        );
        assert_eq!(resolve_resolution(-1, 500, 500), (500, 500));
    }

    #[test]
    fn resolve_fps_covers_all_three_pills_and_falls_back_to_30() {
        assert_eq!(resolve_fps(0), 24);
        assert_eq!(resolve_fps(1), 30);
        assert_eq!(resolve_fps(2), 60);
        assert_eq!(resolve_fps(99), 30);
    }

    #[test]
    fn resolve_max_bounces_covers_the_full_six_rung_ladder() {
        assert_eq!(resolve_max_bounces(0), 4);
        assert_eq!(resolve_max_bounces(1), 8);
        assert_eq!(resolve_max_bounces(2), 12);
        assert_eq!(resolve_max_bounces(3), 24);
        assert_eq!(resolve_max_bounces(4), 64);
        assert_eq!(resolve_max_bounces(5), 128);
    }
}
