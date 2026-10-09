//! Slider position <-> inclusion coefficient conversion for an "Inclusion Haze" slider.
//!
//! The stored and persisted value is the physical coefficient itself
//! (`GemMaterial::scattering_sigma_s`, per model unit, independent of the stone's size), so
//! saved settings keep their haze. The SLIDER drags a position `t` in `0 ..= 1` and the
//! coefficient is `SLIDER_MAX_SIGMA_S * t^2`: haze is perceived roughly in optical depth, most
//! of the interesting range sits near the clean end (a coefficient of `0.05` is barely
//! visible, `0.1` a light silk, `1.0` already strongly clouded), and a linear control spent
//! nearly all of its travel above that. With the square, a quarter of the travel is a light
//! silk (about `0.12`) and the top is milky.
//!
//! No dependency on any UI toolkit: the desktop's settings dialog and the browser app's panel
//! both call these, so the two sliders mean the same thing.

/// The coefficient at the top of the slider: milky. A stone about 2.5 model units of face-up path
/// (`MODEL_UNIT_FACE_UP_PATH`) deep then scatters nearly everything it returns.
pub const SLIDER_MAX_SIGMA_S: f32 = 2.0;

/// The largest coefficient a stored value may have (the persisted range since the control
/// existed).
///
/// A stored value above [`SLIDER_MAX_SIGMA_S`] is kept as it is until the slider is
/// moved; the slider itself sits at its top for it.
pub const STORED_MAX_SIGMA_S: f32 = 3.0;

/// The coefficient (`0.0 ..= SLIDER_MAX_SIGMA_S`) at slider position `position`; a position
/// outside `0 ..= 1` (or not a number) is clamped to the nearest end, `NaN` to `0.0`.
#[must_use]
pub fn position_to_sigma_s(position: f32) -> f32 {
    if position.is_nan() {
        return 0.0;
    }
    let t = position.clamp(0.0, 1.0);
    SLIDER_MAX_SIGMA_S * t * t
}

/// Inverse of [`position_to_sigma_s`]: the slider position for a stored coefficient, clamped to
/// `0 ..= 1` (a value at or above [`SLIDER_MAX_SIGMA_S`] sits at the top, a negative or `NaN`
/// one at `0.0`).
#[must_use]
pub fn sigma_s_to_position(sigma_s: f32) -> f32 {
    if sigma_s.is_nan() || sigma_s <= 0.0 {
        return 0.0;
    }
    (sigma_s / SLIDER_MAX_SIGMA_S).min(1.0).sqrt()
}

/// A stored coefficient limited to the persisted range `0 ..= STORED_MAX_SIGMA_S` (`NaN` to
/// `0.0`), the clamp every load and every slider handler applies.
#[must_use]
pub const fn clamp_stored_sigma_s(sigma_s: f32) -> f32 {
    if sigma_s.is_nan() {
        0.0
    } else {
        sigma_s.clamp(0.0, STORED_MAX_SIGMA_S)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ends_map_to_off_and_milky() {
        assert_eq!(position_to_sigma_s(0.0), 0.0);
        assert_eq!(position_to_sigma_s(1.0), SLIDER_MAX_SIGMA_S);
        assert_eq!(sigma_s_to_position(0.0), 0.0);
        assert_eq!(sigma_s_to_position(SLIDER_MAX_SIGMA_S), 1.0);
    }

    #[test]
    fn a_quarter_of_the_travel_is_a_light_silk() {
        let silk = position_to_sigma_s(0.25);
        assert!((0.08..=0.16).contains(&silk), "{silk}");
    }

    #[test]
    fn the_mapping_is_monotone_and_finer_at_the_clean_end() {
        let mut previous = -1.0_f32;
        for step in 0..=100 {
            let sigma = position_to_sigma_s(step as f32 / 100.0);
            assert!(sigma >= previous, "step {step}: {sigma} < {previous}");
            previous = sigma;
        }
        // The first tenth of the travel changes the haze by less than 1 % of the top; the last
        // tenth by 19 %.
        let first = position_to_sigma_s(0.1) - position_to_sigma_s(0.0);
        let last = position_to_sigma_s(1.0) - position_to_sigma_s(0.9);
        assert!(first < SLIDER_MAX_SIGMA_S * 0.011, "{first}");
        assert!(last > SLIDER_MAX_SIGMA_S * 0.18, "{last}");
    }

    /// A stored coefficient comes back as itself: persisted value -> slider position -> value.
    #[test]
    fn a_stored_value_round_trips_through_the_slider() {
        for sigma in [0.0_f32, 0.02, 0.05, 0.15, 0.25, 0.3, 0.6, 1.0, 1.5, 2.0] {
            let back = position_to_sigma_s(sigma_s_to_position(sigma));
            assert!(
                (back - sigma).abs() <= 1e-6 * sigma.max(1.0),
                "{sigma} -> {back}"
            );
        }
        for position in [0.0_f32, 0.1, 0.25, 0.5, 0.75, 1.0] {
            let back = sigma_s_to_position(position_to_sigma_s(position));
            assert!((back - position).abs() < 1e-6, "{position} -> {back}");
        }
    }

    /// The per-species recommendations keep their coefficient; only their slider position moves.
    #[test]
    fn the_recommended_scattering_values_keep_their_meaning() {
        for (name, sigma) in [("Emerald", 0.6_f32), ("Ruby", 0.3), ("Diamond", 0.02)] {
            let position = sigma_s_to_position(sigma);
            assert!((0.0..=1.0).contains(&position), "{name}: {position}");
            assert!(
                (position_to_sigma_s(position) - sigma).abs() < 1e-6,
                "{name}"
            );
        }
    }

    #[test]
    fn out_of_range_and_non_finite_inputs_are_clamped() {
        assert_eq!(position_to_sigma_s(-1.0), 0.0);
        assert_eq!(position_to_sigma_s(7.0), SLIDER_MAX_SIGMA_S);
        assert_eq!(position_to_sigma_s(f32::NAN), 0.0);
        assert_eq!(sigma_s_to_position(-0.5), 0.0);
        assert_eq!(sigma_s_to_position(f32::NAN), 0.0);
        assert_eq!(
            sigma_s_to_position(3.0),
            1.0,
            "a stored 3.0 sits at the top"
        );
        assert_eq!(clamp_stored_sigma_s(5.0), STORED_MAX_SIGMA_S);
        assert_eq!(clamp_stored_sigma_s(-1.0), 0.0);
        assert_eq!(clamp_stored_sigma_s(f32::NAN), 0.0);
        assert_eq!(
            clamp_stored_sigma_s(2.5),
            2.5,
            "an old value keeps its haze"
        );
    }
}
