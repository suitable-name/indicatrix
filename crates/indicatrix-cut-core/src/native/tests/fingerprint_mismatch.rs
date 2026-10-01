//! What happens when the paired `.asc` no longer matches the fingerprint the
//! native sidecar was saved against: the tier overlay is skipped (or, on an
//! explicit override, applied anyway), while non-tier-indexed and
//! geometry-free fields are still restored.

use super::fixtures::{SIMPLE_ASC, simple_design};
use crate::native::{
    FingerprintCheck, SaveExtras, TierOverlay, load_paired, to_native_file, to_toml_string,
};
use indicatrix::geometry::meet_solver::MeetConstraint;

/// A paired `.asc` that changed since the native file was last saved must be
/// reported as a fingerprint mismatch, and the per-tier overlay must NOT be
/// reapplied on top of the changed geometry. `girdle_diameter_mm`/`material`/
/// `preform` are not tier-indexed, so they are still restored even on a mismatch.
#[test]
fn a_changed_paired_asc_reports_a_mismatch_and_skips_the_tier_overlay() {
    let design = simple_design();
    let native = to_native_file(
        &design,
        "design.asc",
        SIMPLE_ASC.as_bytes(),
        None,
        &SaveExtras::default(),
    );
    let native_toml = to_toml_string(&native).expect("must serialize");

    // A real, innocuous re-touch of the SAME schedule (GemCAD adding a header
    // comment) still changes the file's bytes, hence its hash.
    let changed_asc = format!("GemCad 5.0\nH Re-touched by GemCAD\n{}", &SIMPLE_ASC[11..]);

    let loaded = load_paired(&changed_asc, &native_toml, false).expect("must still load");
    assert!(matches!(
        loaded.fingerprint,
        FingerprintCheck::Mismatch { .. }
    ));
    assert_eq!(loaded.tier_overlay, TierOverlay::SkippedFingerprintMismatch);
    // Non-tier-indexed fields are still restored.
    assert_eq!(loaded.design.girdle_diameter_mm, design.girdle_diameter_mm);
    assert_eq!(loaded.design.material, design.material);
    // The tier-1 overlay was skipped: `.asc` import alone pins this tier back to
    // its own real recorded mast (a `ScaleReference`), NOT the saved
    // `MeetExisting`.
    assert_ne!(
        loaded.design.tiers[1].constraint,
        MeetConstraint::MeetExisting
    );
}

/// A `note` is truly geometry-free (it feeds nothing but display), so it must
/// still be applied when the tier counts agree, even on a fingerprint mismatch
/// that skips the `constraint`/`detached` overlay. A `cheater_offset_deg`, by
/// contrast, DOES feed geometry (it rotates the tier's own facet plane, and is
/// baked into a tier's exported `.asc` indices -- see
/// `Design::cheater_offsets_deg`'s own doc comment) and so must be skipped right
/// alongside `constraint`/`detached` on the very same mismatch (restoring it against a `.asc` that changed underneath the
/// sidecar would silently re-shift the wrong facet's indices).
#[test]
fn a_fingerprint_mismatch_still_applies_the_note_but_not_the_cheater_offset() {
    let mut design = simple_design();
    design.tier_notes.insert(1, "check meet here".to_string());
    design.cheater_offsets_deg.insert(1, -0.75);
    let native = to_native_file(
        &design,
        "design.asc",
        SIMPLE_ASC.as_bytes(),
        None,
        &SaveExtras::default(),
    );
    let native_toml = to_toml_string(&native).expect("must serialize");

    // Same innocuous re-touch as the sibling mismatch test.
    let changed_asc = format!("GemCad 5.0\nH Re-touched by GemCAD\n{}", &SIMPLE_ASC[11..]);

    let loaded = load_paired(&changed_asc, &native_toml, false).expect("must still load");
    assert_eq!(loaded.tier_overlay, TierOverlay::SkippedFingerprintMismatch);
    // The geometry-affecting overlay was skipped...
    assert_ne!(
        loaded.design.tiers[1].constraint,
        MeetConstraint::MeetExisting
    );
    // ...the true display-only field was still applied...
    assert_eq!(loaded.design.tier_note(1), Some("check meet here"));
    // ...but the geometry-affecting cheater offset was skipped right alongside
    // `constraint`/`detached`.
    assert_eq!(loaded.design.cheater_offset_deg(1), None);
}

/// A fingerprint mismatch must not be the last word when the caller explicitly asks
/// for the saved overlay to be applied anyway (e.g. the cutter confirms they still
/// want it) -- as long as the tier counts still agree, `load_paired`'s
/// `apply_overlay_on_mismatch: true` must apply it and report
/// [`TierOverlay::AppliedDespiteMismatch`], distinct from the ordinary
/// [`TierOverlay::Applied`] a clean reload gets.
#[test]
fn apply_overlay_on_mismatch_applies_the_overlay_despite_a_changed_asc() {
    let design = simple_design();
    let native = to_native_file(
        &design,
        "design.asc",
        SIMPLE_ASC.as_bytes(),
        None,
        &SaveExtras::default(),
    );
    let native_toml = to_toml_string(&native).expect("must serialize");

    // Same innocuous re-touch `a_changed_paired_asc_reports_a_mismatch_and_skips_the_tier_overlay`
    // uses.
    let changed_asc = format!("GemCad 5.0\nH Re-touched by GemCAD\n{}", &SIMPLE_ASC[11..]);

    let loaded = load_paired(&changed_asc, &native_toml, true).expect("must still load and apply");
    assert!(matches!(
        loaded.fingerprint,
        FingerprintCheck::Mismatch { .. }
    ));
    assert_eq!(loaded.tier_overlay, TierOverlay::AppliedDespiteMismatch);
    assert_eq!(
        loaded.design.tiers[1].constraint,
        MeetConstraint::MeetExisting
    );
    assert_eq!(loaded.design.tiers[1].detached, vec![0.0, 2.0]);
}

/// A tier-count mismatch between a (fingerprint-matching, hence not itself flagged)
/// native file and its paired `.asc` must also skip the tier overlay.
#[test]
fn tier_count_mismatch_skips_the_overlay_even_with_a_matching_fingerprint() {
    let design = simple_design();
    let mut native = to_native_file(
        &design,
        "design.asc",
        SIMPLE_ASC.as_bytes(),
        None,
        &SaveExtras::default(),
    );
    native.tiers.pop();
    let native_toml = to_toml_string(&native).expect("must serialize");

    let loaded = load_paired(SIMPLE_ASC, &native_toml, false).expect("must load");
    assert_eq!(loaded.fingerprint, FingerprintCheck::Match);
    assert_eq!(
        loaded.tier_overlay,
        TierOverlay::SkippedTierCountMismatch {
            native_tiers: 2,
            asc_tiers: 3,
        }
    );
}
