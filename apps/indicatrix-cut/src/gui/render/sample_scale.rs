//! Exponent <-> sample-count conversion for the settings dialog's "Target Samples"
//! slider, replacing the old four-tier `QualityPreset`.
//!
//! The slider itself (`settings_dialog.slint`) drags an EXPONENT, not the sample count
//! directly: image noise falls as `1/sqrt(N)`, so a linear 8..1024 control would spend
//! roughly 97% of its travel above 32 spp, where each step barely changes anything
//! visible. `settings_dialog.slint` derives the displayed count from the exponent
//! itself for the label (`Math.pow(2.0, exponent)`, the same idiom `export_dialog.slint`'s
//! sample-count slider already uses) and fires its `target_samples_changed` callback with
//! the EXPONENT (same int-discriminant treatment `gui::mod` gives `lighting_changed`),
//! so `gui::mod`'s handler calls `exponent_to_count` to get what's actually
//! stored/rendered. `count_to_exponent` is the reverse direction, used once at startup
//! to turn a persisted `AppSettings::target_samples` count back into the exponent the
//! slider should start at.
//!
//! No dependency on Slint or any other crate -- pure integer arithmetic, exercised
//! directly by the unit tests without spinning up a UI, matching this app's usual
//! `settings::model` convention of keeping plain-data/logic separate from the UI layer.

/// Smallest legal slider exponent (`2^3 = 8` samples).
pub const MIN_EXPONENT: u32 = 3;
/// Largest legal slider exponent (`2^10 = 1024` samples).
pub const MAX_EXPONENT: u32 = 10;

/// Converts a slider exponent to the target sample count (`2^exponent`), clamping the
/// exponent to `MIN_EXPONENT..=MAX_EXPONENT` first so an out-of-range value can never
/// panic (`1u32 << 32` would) or produce a count outside the slider's own legal range.
#[must_use]
pub const fn exponent_to_count(exponent: u32) -> u32 {
    exponent_to_count_bounded(exponent, MIN_EXPONENT, MAX_EXPONENT)
}

/// Inverse of [`exponent_to_count`]: the exponent whose `2^exponent` is the largest
/// power of two not exceeding `count` (`floor(log2(count))`), clamped to
/// `MIN_EXPONENT..=MAX_EXPONENT`. `count` need not itself be an exact power of two --
/// a hand-edited or foreign settings file's `target_samples` still resolves to *some*
/// legal slider position instead of failing to load (see `store::load_or_default`'s
/// doc comment on never letting a settings file block startup).
#[must_use]
pub fn count_to_exponent(count: u32) -> u32 {
    count_to_exponent_bounded(count, MIN_EXPONENT, MAX_EXPONENT)
}

/// Same mapping as [`exponent_to_count`], but clamped to an explicit
/// `min_exponent..=max_exponent` range instead of this module's own fixed
/// `MIN_EXPONENT..=MAX_EXPONENT`. The remote render sample budget
/// (`gui::remote::REMOTE_SAMPLES_MIN_EXPONENT`/`MAX_EXPONENT`) reuses this rather than
/// a second, parallel power-of-two implementation, since a remote render's one-shot
/// full-quality nature affords a wider range than the local interactive target's own
/// slider.
#[must_use]
pub const fn exponent_to_count_bounded(exponent: u32, min_exponent: u32, max_exponent: u32) -> u32 {
    1u32 << clamp_exponent(exponent, min_exponent, max_exponent)
}

/// Bounded counterpart of [`count_to_exponent`] -- see [`exponent_to_count_bounded`]'s
/// doc comment for why this exists alongside the fixed-range version.
#[must_use]
pub fn count_to_exponent_bounded(count: u32, min_exponent: u32, max_exponent: u32) -> u32 {
    // `.max(1)` before `ilog2` -- `count = 0` has no log2, and `ilog2` panics on it.
    let exponent = count.max(1).ilog2();
    clamp_exponent(exponent, min_exponent, max_exponent)
}

const fn clamp_exponent(exponent: u32, min_exponent: u32, max_exponent: u32) -> u32 {
    if exponent < min_exponent {
        min_exponent
    } else if exponent > max_exponent {
        max_exponent
    } else {
        exponent
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The round trip that actually matters: the settings file stores a COUNT, the
    /// slider stores an EXPONENT, and every legal exponent's count must map straight
    /// back to that same exponent (`count_to_exponent` uses `floor(log2)`, which is
    /// exact for an input that is itself a power of two).
    #[test]
    fn round_trips_every_legal_exponent() {
        for exponent in MIN_EXPONENT..=MAX_EXPONENT {
            let count = exponent_to_count(exponent);
            assert_eq!(
                count_to_exponent(count),
                exponent,
                "exponent={exponent} count={count}"
            );
        }
    }

    #[test]
    fn exponent_to_count_matches_expected_powers_of_two() {
        assert_eq!(exponent_to_count(3), 8);
        assert_eq!(exponent_to_count(6), 64);
        assert_eq!(exponent_to_count(8), 256);
        assert_eq!(exponent_to_count(10), 1024);
    }

    #[test]
    fn exponent_to_count_clamps_out_of_range_exponents() {
        assert_eq!(exponent_to_count(0), 8, "clamped up to MIN_EXPONENT");
        assert_eq!(exponent_to_count(31), 1024, "clamped down to MAX_EXPONENT");
    }

    #[test]
    fn count_to_exponent_clamps_out_of_range_counts() {
        assert_eq!(count_to_exponent(0), MIN_EXPONENT);
        assert_eq!(count_to_exponent(1), MIN_EXPONENT);
        assert_eq!(count_to_exponent(u32::MAX), MAX_EXPONENT);
    }

    /// A non-power-of-two count (e.g. a hand-edited settings file) must still resolve
    /// to a legal slider position rather than panicking or failing to load.
    #[test]
    fn count_to_exponent_floors_a_non_power_of_two_count() {
        assert_eq!(count_to_exponent(300), 8, "floor(log2(300)) == 8 -> 256");
        assert_eq!(
            count_to_exponent(511),
            8,
            "just below 512 still floors to 256"
        );
    }

    // ---- Bounded variants (reused for the remote render sample budget) ----

    /// The fixed-range functions must delegate to the bounded ones with exactly
    /// `MIN_EXPONENT..=MAX_EXPONENT`, not a second, independently-maintained
    /// implementation that could drift from it.
    #[test]
    fn bounded_with_the_modules_own_range_matches_the_fixed_range_functions() {
        for exponent in 0..16 {
            assert_eq!(
                exponent_to_count(exponent),
                exponent_to_count_bounded(exponent, MIN_EXPONENT, MAX_EXPONENT),
                "exponent={exponent}"
            );
        }
        for count in [0, 1, 8, 100, 300, 1024, u32::MAX] {
            assert_eq!(
                count_to_exponent(count),
                count_to_exponent_bounded(count, MIN_EXPONENT, MAX_EXPONENT),
                "count={count}"
            );
        }
    }

    /// A different range (e.g. the remote render sample budget's wider
    /// `7..=13`, 128..=8192 samples) round-trips independently of this module's own
    /// `MIN_EXPONENT..=MAX_EXPONENT`.
    #[test]
    fn bounded_round_trips_every_legal_exponent_in_a_different_range() {
        for exponent in 7..=13 {
            let count = exponent_to_count_bounded(exponent, 7, 13);
            assert_eq!(
                count_to_exponent_bounded(count, 7, 13),
                exponent,
                "exponent={exponent} count={count}"
            );
        }
    }

    #[test]
    fn bounded_clamps_out_of_range_exponents_and_counts() {
        assert_eq!(exponent_to_count_bounded(0, 7, 13), 128, "clamped up to 7");
        assert_eq!(
            exponent_to_count_bounded(31, 7, 13),
            8192,
            "clamped down to 13"
        );
        assert_eq!(count_to_exponent_bounded(0, 7, 13), 7);
        assert_eq!(count_to_exponent_bounded(u32::MAX, 7, 13), 13);
    }
}
