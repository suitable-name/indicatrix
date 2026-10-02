//! Pure, dependency-free logic for picking a preview-render material preset.
//!
//! Which built-in `indicatrix::optics::materials::GemMaterial` preset a design should
//! be rendered in for its cached preview images -- see
//! `crate::db::sqlite::Database::ensure_preview_material`, the persistence half that
//! enforces "rolled once, reused forever".
//!
//! This crate deliberately does not depend on `indicatrix` (the storage layer stays free
//! of the raytracer's `bytemuck`/`chull`/`glam`/`wgpu` dependency tree), so
//! [`pick_ri_preset`] is generic over [`RiPresetCandidate`] -- plain `(String, f64)`
//! data -- instead of `GemMaterial` directly. `apps/indicatrix-cut` adapts
//! `GemMaterial::all_materials()` into `&[RiPresetCandidate]` before calling in.
//!
//! Refractive index is conventionally quoted at the sodium D line (589.3 nm), matching
//! this crate's `diagram_details.refractive_index` column. A caller building
//! `RiPresetCandidate`s from `GemMaterial::all_materials()` (which stores a full
//! dispersion curve, not one RI number) MUST evaluate it at that same 589.3 nm, or
//! numbers under different conventions get silently compared -- a bug this module can't
//! detect since it never sees a wavelength.

/// One candidate preset [`pick_ri_preset`] can match against.
///
/// A display name (persisted into `diagram_previews.preview_material`) and its
/// refractive index, evaluated at the sodium D line by the caller -- see this module's
/// doc for why that convention matters.
#[derive(Debug, Clone, PartialEq)]
pub struct RiPresetCandidate {
    /// Display name.
    pub name: String,
    /// Refractive index.
    pub refractive_index: f64,
}

/// Picks which of `candidates` best matches `target_ri`, within `tolerance`:
///
/// - Every candidate within `tolerance` of `target_ri` is a match. Exactly one -> that one.
/// - Two or more -> picked uniformly at random via `random_unit` (see
///   [`pick_ri_preset_by`] to break the tie by merit instead). Stateless: the
///   "rolled once per design forever" guarantee is the caller's
///   ([`crate::db::sqlite::Database::ensure_preview_material`]) persistence logic.
/// - No match -> the candidate with smallest `|refractive_index - target_ri|`. A tie in
///   this fallback uses the same random rule as multiple matches above.
/// - `candidates` empty -> `None`.
///
/// `random_unit` must return `[0.0, 1.0)` each call (out-of-range is clamped
/// defensively). It's a parameter, not a hardcoded RNG, so this stays deterministically
/// testable.
#[must_use]
pub fn pick_ri_preset<'a>(
    target_ri: f64,
    candidates: &'a [RiPresetCandidate],
    tolerance: f64,
    random_unit: &mut dyn FnMut() -> f64,
) -> Option<&'a RiPresetCandidate> {
    pick_ri_preset_by(target_ri, candidates, tolerance, &mut |shortlist| {
        uniform_index(shortlist.len(), random_unit)
    })
}

/// [`pick_ri_preset`] with the tie-break left to the caller.
///
/// `choose` receives the shortlist (see [`ri_shortlist`]) and returns the index of the
/// candidate to take (clamped into range). It is not called for an empty or single-entry
/// shortlist.
#[must_use]
pub fn pick_ri_preset_by<'a>(
    target_ri: f64,
    candidates: &'a [RiPresetCandidate],
    tolerance: f64,
    choose: &mut dyn FnMut(&[&'a RiPresetCandidate]) -> usize,
) -> Option<&'a RiPresetCandidate> {
    let shortlist = ri_shortlist(target_ri, candidates, tolerance);
    match shortlist.len() {
        0 => None,
        1 => Some(shortlist[0]),
        n => Some(shortlist[choose(&shortlist).min(n - 1)]),
    }
}

/// The candidates that fit `target_ri`: every one within `tolerance`, or, when none is,
/// those with the smallest `|refractive_index - target_ri|`. Empty only for no candidates.
#[must_use]
pub fn ri_shortlist(
    target_ri: f64,
    candidates: &[RiPresetCandidate],
    tolerance: f64,
) -> Vec<&RiPresetCandidate> {
    let within: Vec<&RiPresetCandidate> = candidates
        .iter()
        .filter(|c| (c.refractive_index - target_ri).abs() <= tolerance)
        .collect();
    if !within.is_empty() {
        return within;
    }
    // fold, not Iterator::min, since f64 is only PartialOrd.
    let closest = candidates
        .iter()
        .map(|c| (c.refractive_index - target_ri).abs())
        .fold(f64::INFINITY, f64::min);
    // Exact equality is intentional: both sides derive from the same target_ri.
    candidates
        .iter()
        .filter(|c| (c.refractive_index - target_ri).abs() == closest)
        .collect()
}

/// An index in `0..len` from one `random_unit()` draw; `len` must be at least 1.
#[must_use]
pub fn uniform_index(len: usize, random_unit: &mut dyn FnMut() -> f64) -> usize {
    let r = random_unit();
    // 0.999_999_999 not 1.0 so an (incorrect) exact 1.0 input still lands on the last
    // element, not one past it.
    let r = if r.is_finite() {
        r.clamp(0.0, 0.999_999_999)
    } else {
        0.0
    };
    ((r * len as f64) as usize).min(len - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(name: &str, ri: f64) -> RiPresetCandidate {
        RiPresetCandidate {
            name: name.to_string(),
            refractive_index: ri,
        }
    }

    /// Always returns the same value, to pin down which candidate a tie resolves to.
    fn fixed(value: f64) -> impl FnMut() -> f64 {
        move || value
    }

    #[test]
    fn empty_candidates_returns_none() {
        let mut rng = fixed(0.0);
        assert_eq!(pick_ri_preset(1.54, &[], 0.01, &mut rng), None);
    }

    #[test]
    fn a_single_candidate_within_tolerance_is_chosen_without_consulting_the_rng() {
        let candidates = [candidate("Quartz", 1.545)];
        // Panics if called -- proves the single-match path never draws randomness.
        let mut rng = || -> f64 { panic!("must not be called for a single match") };
        let picked = pick_ri_preset(1.544, &candidates, 0.01, &mut rng).unwrap();
        assert_eq!(picked.name, "Quartz");
    }

    #[test]
    fn multiple_matches_within_tolerance_pick_uniformly_via_the_rng() {
        let candidates = [
            candidate("A", 1.540),
            candidate("B", 1.541),
            candidate("C", 1.542),
        ];
        // All three within 0.01 of 1.541 -- r=0.0 picks index 0, r near 1.0 picks index 2.
        let mut rng_low = fixed(0.0);
        let picked_low = pick_ri_preset(1.541, &candidates, 0.01, &mut rng_low).unwrap();
        assert_eq!(picked_low.name, "A");

        let mut rng_high = fixed(0.999);
        let picked_high = pick_ri_preset(1.541, &candidates, 0.01, &mut rng_high).unwrap();
        assert_eq!(picked_high.name, "C");
    }

    #[test]
    fn no_match_falls_back_to_the_single_nearest_candidate() {
        let candidates = [candidate("Low", 1.40), candidate("High", 2.40)];
        let mut rng = || -> f64 { panic!("must not be called: nearest is unambiguous") };
        // 1.50 is 0.10 from Low, 0.90 from High -- Low wins outright.
        let picked = pick_ri_preset(1.50, &candidates, 0.001, &mut rng).unwrap();
        assert_eq!(picked.name, "Low");
    }

    #[test]
    fn no_match_with_a_tied_nearest_distance_picks_uniformly_via_the_rng() {
        let candidates = [candidate("Low", 1.40), candidate("High", 1.60)];
        // 1.50 is exactly 0.10 from both -- a genuine tie in the fallback.
        let mut rng_low = fixed(0.0);
        let picked_low = pick_ri_preset(1.50, &candidates, 0.001, &mut rng_low).unwrap();
        assert_eq!(picked_low.name, "Low");

        let mut rng_high = fixed(0.999);
        let picked_high = pick_ri_preset(1.50, &candidates, 0.001, &mut rng_high).unwrap();
        assert_eq!(picked_high.name, "High");
    }

    #[test]
    fn a_chooser_breaks_the_tie_by_merit_and_is_skipped_for_a_single_match() {
        let candidates = [
            candidate("A", 1.540),
            candidate("B", 1.541),
            candidate("C", 1.80),
        ];
        let picked = pick_ri_preset_by(1.541, &candidates, 0.01, &mut |s| {
            s.iter().position(|c| c.name == "B").unwrap()
        })
        .unwrap();
        assert_eq!(picked.name, "B");
        let mut panics = |_: &[&RiPresetCandidate]| -> usize { panic!("single match") };
        assert_eq!(
            pick_ri_preset_by(1.80, &candidates, 0.01, &mut panics)
                .unwrap()
                .name,
            "C"
        );
    }

    #[test]
    fn tolerance_boundary_is_inclusive() {
        let candidates = [candidate("Exact", 1.75)];
        let mut rng = || -> f64 { panic!("must not be called for a single match") };
        // diff == tolerance must still count as a match ("within tolerance" is `<=`).
        // 1.75/1.5/0.25 are exact binary fractions so the `<=` genuinely lands on the
        // boundary; e.g. 1.55/1.54/0.01 would not (diff is 0.010000000000000009) and
        // would silently test the nearest-fallback path instead. Keep these exact.
        let picked = pick_ri_preset(1.5, &candidates, 0.25, &mut rng).unwrap();
        assert_eq!(picked.name, "Exact");
    }
}
