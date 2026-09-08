use super::*;
use crate::{
    design::{ConstraintTier, Design},
    material::MaterialSelection,
    preform::PreformSpec,
};
use indicatrix::geometry::meet_solver::MeetConstraint;

/// A real, small schedule (two anchored tiers, no meet-derived structure needed)
/// reused by several tests below.
const SIMPLE_ASC: &str = "GemCad 5.0\n\
     g 4 0.0\n\
     y 1 n\n\
     I 1.62\n\
     a 90.000000 1.00000000 0 1 2 3 G Set girdle thickness\n\
     a 0.000000 0.60000000 G Set stone size\n\
     a -0.000000 0.55000000 G Set stone size\n";

/// Same geometry as [`SIMPLE_ASC`], but every tier's `G` note already reads exactly
/// what [`crate::design::Design::to_asc_schedule`]'s `constraint_notes` canonicalizes
/// a [`MeetConstraint::ScaleReference`] tier to ("Set stone size.", full stop
/// included). `SIMPLE_ASC` keeps the more natural "Set girdle thickness" wording
/// instead, so re-exporting it can never come back byte-identical even when nothing
/// changed (notes are regenerated from the constraint alone -- see the module doc
/// comment's "Preserving the original `.asc` text" section); this constant is the
/// already-canonical file the untouched-design preservation test needs instead.
const FULLY_CANONICAL_ASC: &str = "GemCad 5.0\n\
     g 4 0.0\n\
     y 1 n\n\
     I 1.62\n\
     a 90.000000 1.00000000 0 1 2 3 G Set stone size.\n\
     a 0.000000 0.60000000 G Set stone size.\n\
     a -0.000000 0.55000000 G Set stone size.\n";

fn simple_design() -> Design {
    let schedule = indicatrix_formats::asc::parse_asc(SIMPLE_ASC).expect("fixture must parse");
    let mut design = Design::from_asc_schedule(PreformSpec::block(2.0, 1.0, 2.0), &schedule);
    design.girdle_diameter_mm = Some(6.5);
    design.material = MaterialSelection {
        name: Some("Diamond".to_string()),
        specific_gravity_override: None,
        // `Design::effective_refractive_index` derives RI from a recognized material
        // name when there is no override; pinning this to the fixture's own `I 1.62`
        // keeps the "Diamond" selection (real n_D ~2.417) from changing what
        // `to_asc_schedule` exports for an otherwise-untouched design.
        refractive_index_override: Some(1.62),
    };
    design.tiers[1].constraint = MeetConstraint::MeetExisting;
    design.tiers[1].detached = vec![0.0, 2.0];
    design
}

/// Same base fixture as [`simple_design`], but WITHOUT its tier-1 `MeetExisting`
/// override: every tier stays exactly as [`Design::from_asc_schedule`] pinned it
/// (`ScaleReference`). That override is needed for tests exercising the meet-intent
/// overlay, but it also means `Design::solve` derives tier 1's mast fresh from
/// geometry rather than the recorded `0.6` -- wrong for a test asserting
/// `to_asc_schedule()` reproduces the original text byte for byte. Used by
/// [`save_paired_preserves_untouched_text`] and
/// [`save_paired_with_no_original_text_exports_fresh`].
fn fully_anchored_design() -> Design {
    let schedule =
        indicatrix_formats::asc::parse_asc(FULLY_CANONICAL_ASC).expect("fixture must parse");
    let mut design = Design::from_asc_schedule(PreformSpec::block(2.0, 1.0, 2.0), &schedule);
    design.girdle_diameter_mm = Some(6.5);
    design.material = MaterialSelection {
        name: Some("Diamond".to_string()),
        specific_gravity_override: None,
        // See `simple_design`'s comment on this field.
        refractive_index_override: Some(1.62),
    };
    design
}

/// [`load_paired`] with a matching fingerprint must reproduce every field
/// [`to_native_file`] captured: preform, girdle diameter, material, and every
/// tier's authored constraint/detached set -- not just the ones `.asc` itself
/// would have reconstructed via `Design::from_asc_schedule`'s own pinning.
#[test]
fn load_paired_reproduces_every_captured_field_on_a_clean_pair() {
    let design = simple_design();
    let native = to_native_file(&design, "design.asc", SIMPLE_ASC.as_bytes());
    let native_toml = to_toml_string(&native).expect("must serialize");

    let loaded = load_paired(SIMPLE_ASC, &native_toml).expect("must load");
    assert_eq!(loaded.fingerprint, FingerprintCheck::Match);
    assert_eq!(loaded.tier_overlay, TierOverlay::Applied);
    assert_eq!(loaded.design.preform, design.preform);
    assert_eq!(loaded.design.girdle_diameter_mm, design.girdle_diameter_mm);
    assert_eq!(loaded.design.material, design.material);
    // Tier 1 was deliberately set to `MeetExisting`, not what `.asc` import alone
    // would pin it to (`ScaleReference`) -- the gap this module exists to close.
    assert_eq!(
        loaded.design.tiers[1].constraint,
        MeetConstraint::MeetExisting
    );
    assert_eq!(loaded.design.tiers[1].detached, vec![0.0, 2.0]);
}

/// A paired `.asc` that changed since the native file was last saved must be
/// reported as a fingerprint mismatch, and the per-tier overlay must NOT be
/// reapplied on top of the changed geometry. `girdle_diameter_mm`/`material`/
/// `preform` are not tier-indexed, so they are still restored even on a mismatch.
#[test]
fn a_changed_paired_asc_reports_a_mismatch_and_skips_the_tier_overlay() {
    let design = simple_design();
    let native = to_native_file(&design, "design.asc", SIMPLE_ASC.as_bytes());
    let native_toml = to_toml_string(&native).expect("must serialize");

    // A real, innocuous re-touch of the SAME schedule (GemCAD adding a header
    // comment) still changes the file's bytes, hence its hash.
    let changed_asc = format!("GemCad 5.0\nH Re-touched by GemCAD\n{}", &SIMPLE_ASC[11..]);

    let loaded = load_paired(&changed_asc, &native_toml).expect("must still load");
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

/// [`save_paired`] must preserve the caller's original `.asc` text byte for byte
/// when the design was not touched in any way that changes its `to_asc_schedule`
/// output. Setting `girdle_diameter_mm`/`material` does NOT round-trip into `.asc`,
/// so it must not trigger a regeneration either. Uses [`FULLY_CANONICAL_ASC`], not
/// `SIMPLE_ASC` -- see that constant's own doc comment for why.
#[test]
fn save_paired_preserves_untouched_text() {
    let design = fully_anchored_design();
    let saved = save_paired(&design, "design.asc", Some(FULLY_CANONICAL_ASC)).expect("must save");
    assert!(saved.asc_preserved);
    assert_eq!(saved.asc_text, FULLY_CANONICAL_ASC);
    assert_eq!(
        saved.native.asc_sha256,
        sha256_hex(FULLY_CANONICAL_ASC.as_bytes())
    );
}

/// The moment a real tier-facing edit changes the schedule, [`save_paired`] must
/// regenerate fresh `.asc` text instead of preserving the (now stale) original.
#[test]
fn save_paired_regenerates_after_a_real_edit() {
    let mut design = simple_design();
    design.tiers[1].constraint = MeetConstraint::ScaleReference(0.5);
    let saved = save_paired(&design, "design.asc", Some(SIMPLE_ASC)).expect("must save");
    assert!(!saved.asc_preserved);
    assert_ne!(saved.asc_text, SIMPLE_ASC);
    // The freshly generated text must still parse back to the edited schedule.
    let reparsed = indicatrix_formats::asc::parse_asc(&saved.asc_text).expect("must parse");
    assert!((reparsed.tiers[1].mast - 0.5).abs() < 1e-9);
}

/// With no original text at all (a brand-new design, never previously exported),
/// `save_paired` must fall straight through to a fresh export rather than
/// erroring.
#[test]
fn save_paired_with_no_original_text_exports_fresh() {
    let design = fully_anchored_design();
    let saved = save_paired(&design, "design.asc", None).expect("must save");
    assert!(!saved.asc_preserved);
    assert!(indicatrix_formats::asc::parse_asc(&saved.asc_text).is_ok());
}

/// A tier-count mismatch between a (fingerprint-matching, hence not itself flagged)
/// native file and its paired `.asc` must also skip the tier overlay.
#[test]
fn tier_count_mismatch_skips_the_overlay_even_with_a_matching_fingerprint() {
    let design = simple_design();
    let mut native = to_native_file(&design, "design.asc", SIMPLE_ASC.as_bytes());
    native.tiers.pop();
    let native_toml = to_toml_string(&native).expect("must serialize");

    let loaded = load_paired(SIMPLE_ASC, &native_toml).expect("must load");
    assert_eq!(loaded.fingerprint, FingerprintCheck::Match);
    assert_eq!(
        loaded.tier_overlay,
        TierOverlay::SkippedTierCountMismatch {
            native_tiers: 2,
            asc_tiers: 3,
        }
    );
}

// --- `refractive_index_override` round-trips through `[material]` ---

/// `MaterialTable::refractive_index_override` must round-trip through a real
/// save/load pair exactly like every other `material` field already does.
#[test]
fn refractive_index_override_round_trips_through_save_and_load() {
    // `fully_anchored_design`, not `simple_design`: this test needs the design to
    // actually SOLVE (`save_paired` calls `to_asc_schedule`), and
    // `simple_design`'s own tier-1 `MeetExisting` override removes the Crown
    // block's only anchor.
    let mut design = fully_anchored_design();
    design.material.refractive_index_override = Some(1.90);
    let saved = save_paired(&design, "design.asc", Some(FULLY_CANONICAL_ASC)).expect("must save");

    let loaded = load_paired(&saved.asc_text, &saved.native_toml).expect("must load");
    assert_eq!(loaded.design.material.refractive_index_override, Some(1.90));
}

// --- `FreshDesignSpec` round-trips through native + paired `.asc` persistence ---

/// A design built via [`Design::fresh_from_spec`] (gear/symmetry/mirror/material all
/// set up front, no prior `.asc` at all) must round-trip through a real save/load
/// pair.
#[test]
fn fresh_design_spec_round_trips_through_native_and_paired_asc() {
    let spec = crate::design::FreshDesignSpec {
        gear_teeth: 80,
        symmetry_order: 5,
        mirror: false,
        material: MaterialSelection {
            name: Some("Quartz".to_string()),
            specific_gravity_override: Some(2.66),
            refractive_index_override: Some(1.545),
        },
        preform: PreformSpec::cylinder(80, 1.2, 1.0, 0.9),
    };
    let mut design = Design::fresh_from_spec(spec);
    // A real `.asc` file needs at least one `a` (facet tier) record --
    // `indicatrix_formats::asc::parse_asc` rejects a schedule with none, so an entirely
    // tierless fresh design cannot round-trip through a real paired `.asc` file.
    design.tiers.push(ConstraintTier {
        angle_deg: 0.0,
        name: "T".to_string(),
        indices: vec![],
        constraint: MeetConstraint::ScaleReference(0.5),
        imported_meet: None,
        detached: Vec::new(),
    });

    let saved = save_paired(&design, "fresh.asc", None).expect("a fresh design must save");
    let loaded = load_paired(&saved.asc_text, &saved.native_toml).expect("must load back");

    assert_eq!(loaded.design.meta.gear_teeth, 80);
    assert_eq!(loaded.design.meta.symmetry_order, 5);
    assert!(!loaded.design.meta.mirror);
    assert_eq!(loaded.design.material, design.material);
    assert_eq!(loaded.design.preform, design.preform);
    assert_eq!(loaded.design.tiers.len(), 1);
    assert_eq!(loaded.design.tiers[0].angle_deg, 0.0);
}
