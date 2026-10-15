//! When [`crate::native::catalogue_asc`] preserves the caller's original `.asc`
//! text byte for byte versus when it must regenerate fresh text, the
//! reconstructed-file marker, and a from-scratch [`crate::design::FreshDesignSpec`]
//! design's own round trip.

use super::fixtures::{FULLY_CANONICAL_ASC, fully_anchored_design, reopen, simple_design};
use crate::{
    design::{ConstraintTier, Design},
    material::MaterialSelection,
    native::catalogue_asc,
    preform::PreformSpec,
};
use indicatrix::geometry::meet_solver::MeetConstraint;

/// [`crate::native::catalogue_asc`] must preserve the caller's original `.asc` text
/// byte for byte when the design was not touched in any way that changes its
/// `to_asc_schedule` output. Setting `girdle_diameter_mm`/`material` does NOT
/// round-trip into `.asc`, so it must not trigger a regeneration either. Uses
/// [`FULLY_CANONICAL_ASC`], not `SIMPLE_ASC` -- see that constant's own doc comment
/// for why.
#[test]
fn catalogue_asc_preserves_untouched_text() {
    let design = fully_anchored_design();
    let saved = catalogue_asc(&design, Some(FULLY_CANONICAL_ASC), None).expect("must write");
    assert!(saved.asc_preserved);
    assert!(saved.draft_reason.is_none());
    assert_eq!(saved.asc_text, FULLY_CANONICAL_ASC);
}

/// Selecting a different BUILT-IN material must not, by itself, force a fresh
/// `.asc` regeneration. `fully_anchored_design`'s own `refractive_index_override:
/// Some(1.62)` exists only to keep `simple_design`'s sibling fixture byte-stable
/// for unrelated tests -- dropping it here and switching to
/// Quartz (`n_D` ~1.544, not the file's own `I 1.62`) is the REAL case: nothing
/// tier/preform-facing changed, only `Design::effective_refractive_index`'s derived
/// output, which the design file's own `[material]` table already records separately.
#[test]
fn catalogue_asc_preserves_untouched_text_after_a_material_change_alone() {
    let mut design = fully_anchored_design();
    design.material.refractive_index_override = None;
    design.material.name = Some("Quartz".to_string());
    assert_ne!(
        design.effective_refractive_index(),
        1.62,
        "the test must actually exercise a changed effective RI"
    );

    let saved = catalogue_asc(&design, Some(FULLY_CANONICAL_ASC), None).expect("must write");
    assert!(
        saved.asc_preserved,
        "a material change alone must not force a fresh .asc export"
    );
    assert_eq!(saved.asc_text, FULLY_CANONICAL_ASC);
    // The real selection still reaches the design file even though the exported
    // `.asc` text kept the file's own original `I 1.62` line.
    assert_eq!(
        reopen(&design).design.material.name.as_deref(),
        Some("Quartz")
    );
}

/// The moment a real tier-facing edit changes the schedule, [`crate::native::catalogue_asc`]
/// must regenerate fresh `.asc` text instead of preserving the (now stale) original.
#[test]
fn catalogue_asc_regenerates_after_a_real_edit() {
    let mut design = simple_design();
    design.tiers[1].constraint = MeetConstraint::ScaleReference(0.5);
    let saved =
        catalogue_asc(&design, Some(super::fixtures::SIMPLE_ASC), None).expect("must write");
    assert!(!saved.asc_preserved);
    assert_ne!(saved.asc_text, super::fixtures::SIMPLE_ASC);
    // The freshly generated text must still parse back to the edited schedule.
    let reparsed = indicatrix_formats::asc::parse_asc(&saved.asc_text).expect("must parse");
    assert!((reparsed.tiers[1].mast - 0.5).abs() < 1e-9);
}

/// With no original text at all (a brand-new design, never previously exported),
/// `catalogue_asc` must fall straight through to a fresh export rather than
/// erroring.
#[test]
fn catalogue_asc_with_no_original_text_exports_fresh() {
    let design = fully_anchored_design();
    let saved = catalogue_asc(&design, None, None).expect("must write");
    assert!(!saved.asc_preserved);
    assert!(indicatrix_formats::asc::parse_asc(&saved.asc_text).is_ok());
}

/// A design built via [`crate::design::Design::fresh_from_spec`]
/// (gear/symmetry/mirror/material all set up front, no prior `.asc` at all) must
/// round-trip through a real save/load.
#[test]
fn fresh_design_spec_round_trips_through_the_design_file() {
    let spec = crate::design::FreshDesignSpec {
        gear_teeth: 80,
        symmetry_order: 5,
        mirror: false,
        material: MaterialSelection {
            name: Some("Quartz".to_string()),
            specific_gravity_override: Some(2.66),
            refractive_index_override: Some(1.545),
            body_color_override: None,
            body_color_bands_override: None,
            absorption_path_scale_override: None,
        },
        preform: PreformSpec::cylinder(80, 1.2, 1.0, 0.9),
    };
    let mut design = Design::fresh_from_spec(spec);
    // One tier, so the catalogue `.asc` below is a real file too: a real `.asc` needs at
    // least one `a` (facet tier) record, `indicatrix_formats::asc::parse_asc` rejects a
    // schedule with none.
    design.tiers.push(ConstraintTier {
        angle_deg: 0.0,
        name: "T".to_string(),
        indices: vec![],
        constraint: MeetConstraint::ScaleReference(0.5),
        imported_meet: None,
        original_notes: None,
        detached: Vec::new(),
    });

    let saved = catalogue_asc(&design, None, None).expect("a fresh design must write");
    assert!(indicatrix_formats::asc::parse_asc(&saved.asc_text).is_ok());
    let loaded = reopen(&design);

    assert_eq!(loaded.design.meta.gear_teeth, 80);
    assert_eq!(loaded.design.meta.symmetry_order, 5);
    assert!(!loaded.design.meta.mirror);
    assert_eq!(loaded.design.material, design.material);
    assert_eq!(loaded.design.preform, design.preform);
    assert_eq!(loaded.design.tiers.len(), 1);
    assert_eq!(loaded.design.tiers[0].angle_deg, 0.0);
    // The authored `ScaleReference(0.5)` itself must survive too -- not just the
    // angle.
    assert_eq!(
        loaded.design.tiers[0].constraint,
        MeetConstraint::ScaleReference(0.5)
    );
}

/// [`crate::native::catalogue_asc`]'s `placeholder_note` parameter must mark the
/// written `.asc` via `indicatrix_formats::asc::mark_reconstructed` rather than
/// writing a schedule that looks like an ordinary, authored `GemCAD` file.
#[test]
fn placeholder_note_marks_the_written_asc_as_reconstructed() {
    let design = fully_anchored_design();
    let saved = catalogue_asc(
        &design,
        None,
        Some("angle-table reconstruction, no attached .asc"),
    )
    .expect("must write");

    assert!(!saved.asc_preserved);
    let reparsed = indicatrix_formats::asc::parse_asc(&saved.asc_text).expect("must still parse");
    assert!(
        reparsed
            .headers
            .first()
            .is_some_and(|h| h.starts_with("RECONSTRUCTED")),
        "the written .asc's first header must carry the reconstructed marker"
    );
}
