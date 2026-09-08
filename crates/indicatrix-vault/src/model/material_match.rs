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
/// A display name (persisted into `diagram_details.preview_material`) and its
/// refractive index, evaluated at the sodium D line by the caller -- see this module's
/// doc for why that convention matters.
#[derive(Debug, Clone, PartialEq)]
pub struct RiPresetCandidate {
    pub name: String,
    pub refractive_index: f64,
}

/// Picks which of `candidates` best matches `target_ri`, within `tolerance`:
///
/// - Every candidate within `tolerance` of `target_ri` is a match. Exactly one -> that one.
/// - Two or more -> picked uniformly at random via `random_unit`. Stateless: the
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
    if candidates.is_empty() {
        return None;
    }

    let within_tolerance: Vec<&RiPresetCandidate> = candidates
        .iter()
        .filter(|c| (c.refractive_index - target_ri).abs() <= tolerance)
        .collect();
    match within_tolerance.len() {
        0 => {}
        1 => return Some(within_tolerance[0]),
        _ => return Some(pick_uniformly(&within_tolerance, random_unit)),
    }

    // No candidate within tolerance: fall back to nearest by absolute difference.
    // fold, not Iterator::min, since f64 is only PartialOrd.
    let closest_diff = candidates
        .iter()
        .map(|c| (c.refractive_index - target_ri).abs())
        .fold(f64::INFINITY, f64::min);
    // Exact equality is intentional: both sides derive from the same target_ri, so a
    // tie means bit-identical values, not rounding needing an epsilon.
    let nearest: Vec<&RiPresetCandidate> = candidates
        .iter()
        .filter(|c| (c.refractive_index - target_ri).abs() == closest_diff)
        .collect();
    // Skip random_unit when nearest is unambiguous, since the draw is persisted
    // forever and an unnecessary one would still perturb the caller's RNG stream.
    match nearest.len() {
        1 => Some(nearest[0]),
        _ => Some(pick_uniformly(&nearest, random_unit)),
    }
}

/// Picks one of `items` uniformly at random using one `random_unit()` draw. `items`
/// must be non-empty (both call sites only reach this with at least one candidate).
fn pick_uniformly<'a>(
    items: &[&'a RiPresetCandidate],
    random_unit: &mut dyn FnMut() -> f64,
) -> &'a RiPresetCandidate {
    let r = random_unit();
    // 0.999_999_999 not 1.0 so an (incorrect) exact 1.0 input still lands on the last
    // element, not one past it.
    let r = if r.is_finite() {
        r.clamp(0.0, 0.999_999_999)
    } else {
        0.0
    };
    let idx = ((r * items.len() as f64) as usize).min(items.len() - 1);
    items[idx]
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
