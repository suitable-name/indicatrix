use super::*;
use crate::{
    design::{ConstraintTier, Design, TierTarget},
    material::MaterialSelection,
    preform::PreformSpec,
};
use indicatrix::geometry::{meet_solver::MeetConstraint, stone_metrics::ExternalProportions};

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
    let native = to_native_file(
        &design,
        "design.asc",
        SIMPLE_ASC.as_bytes(),
        None,
        &SaveExtras::default(),
    );
    let native_toml = to_toml_string(&native).expect("must serialize");

    let loaded = load_paired(SIMPLE_ASC, &native_toml, false).expect("must load");
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

/// A per-tier cutter-authored note (`Design::tier_notes`) must round-trip
/// through `to_native_file`/`load_paired` exactly like the constraint/detached
/// overlay does, on the same tier, by the same array position.
#[test]
fn tier_notes_round_trip_through_save_and_load() {
    let mut design = simple_design();
    design.tier_notes.insert(1, "check meet here".to_string());
    let native = to_native_file(
        &design,
        "design.asc",
        SIMPLE_ASC.as_bytes(),
        None,
        &SaveExtras::default(),
    );
    let native_toml = to_toml_string(&native).expect("must serialize");

    let loaded = load_paired(SIMPLE_ASC, &native_toml, false).expect("must load");
    assert_eq!(loaded.tier_overlay, TierOverlay::Applied);
    assert_eq!(loaded.design.tier_note(0), None);
    assert_eq!(loaded.design.tier_note(1), Some("check meet here"));
    assert_eq!(loaded.design.tier_note(2), None);
}

/// A design with no notes at all must round-trip with an empty `tier_notes` --
/// the "nothing to restore" case `tier_notes_round_trip_through_save_and_load`'s
/// positive case doesn't cover.
#[test]
fn a_design_with_no_notes_round_trips_with_no_notes() {
    let design = simple_design();
    let native = to_native_file(
        &design,
        "design.asc",
        SIMPLE_ASC.as_bytes(),
        None,
        &SaveExtras::default(),
    );
    let native_toml = to_toml_string(&native).expect("must serialize");

    let loaded = load_paired(SIMPLE_ASC, &native_toml, false).expect("must load");
    assert!(loaded.design.tier_notes.is_empty());
}

/// A per-tier cutter-authored cheater/azimuth offset (`Design::cheater_offsets_deg`)
/// must round-trip through `to_native_file`/`load_paired` exactly like `tier_notes`
/// does, on the same tier, by the same array position.
#[test]
fn cheater_offset_round_trips_through_save_and_load() {
    let mut design = simple_design();
    design.cheater_offsets_deg.insert(1, -0.75);
    let native = to_native_file(
        &design,
        "design.asc",
        SIMPLE_ASC.as_bytes(),
        None,
        &SaveExtras::default(),
    );
    let native_toml = to_toml_string(&native).expect("must serialize");

    let loaded = load_paired(SIMPLE_ASC, &native_toml, false).expect("must load");
    assert_eq!(loaded.tier_overlay, TierOverlay::Applied);
    assert_eq!(loaded.design.cheater_offset_deg(0), None);
    assert_eq!(loaded.design.cheater_offset_deg(1), Some(-0.75));
    assert_eq!(loaded.design.cheater_offset_deg(2), None);
}

/// A design with no cheater offsets at all must round-trip with an empty
/// `cheater_offsets_deg` -- the "nothing to restore" case
/// `cheater_offset_round_trips_through_save_and_load`'s positive case doesn't cover.
#[test]
fn a_design_with_no_cheater_offsets_round_trips_with_no_cheater_offsets() {
    let design = simple_design();
    let native = to_native_file(
        &design,
        "design.asc",
        SIMPLE_ASC.as_bytes(),
        None,
        &SaveExtras::default(),
    );
    let native_toml = to_toml_string(&native).expect("must serialize");

    let loaded = load_paired(SIMPLE_ASC, &native_toml, false).expect("must load");
    assert!(loaded.design.cheater_offsets_deg.is_empty());
}

/// `TierId`/`TierTarget` must round-trip through `to_native_file`/`load_paired`
/// exactly like `cheater_offsets_deg` does above.
#[test]
fn tier_id_and_target_round_trip_through_save_and_load() {
    let mut design = simple_design();
    let tier1_id = design.tier_id_at(1).expect("tier 1 must have an id");
    design
        .tier_targets
        .insert(tier1_id, TierTarget::DepthMm(3.2));

    let native = to_native_file(
        &design,
        "design.asc",
        SIMPLE_ASC.as_bytes(),
        None,
        &SaveExtras::default(),
    );
    let native_toml = to_toml_string(&native).expect("must serialize");

    let loaded = load_paired(SIMPLE_ASC, &native_toml, false).expect("must load");
    assert_eq!(loaded.tier_overlay, TierOverlay::Applied);
    // The targeted tier's own id and target both survive...
    assert_eq!(loaded.design.tier_id_at(1), Some(tier1_id));
    assert_eq!(loaded.design.tier_target(1), Some(TierTarget::DepthMm(3.2)));
    // ...every OTHER tier's id survives too, and neither gained a target it
    // never had.
    assert_eq!(loaded.design.tier_id_at(0), design.tier_id_at(0));
    assert_eq!(loaded.design.tier_id_at(2), design.tier_id_at(2));
    assert_eq!(loaded.design.tier_target(0), None);
    assert_eq!(loaded.design.tier_target(2), None);
}

/// A sidecar saved before `TierTable::tier_id` existed (every tier's `tier_id`
/// key stripped, simulating an old file) must still load -- with a genuinely
/// fresh, distinct id assigned to every tier instead of `None`/a panic. See
/// `apply_tier_ids_and_targets`'s own doc comment ("assign a fresh id only
/// when the file has none").
#[test]
fn a_sidecar_with_no_tier_ids_still_loads_and_gets_fresh_distinct_ids() {
    let design = simple_design();
    let native = to_native_file(
        &design,
        "design.asc",
        SIMPLE_ASC.as_bytes(),
        None,
        &SaveExtras::default(),
    );
    let mut text = to_toml_string(&native).expect("must serialize");
    // Simulate a pre-this-field sidecar by stripping every tier's freshly
    // written `tier_id` key.
    text = text
        .lines()
        .filter(|line| !line.trim_start().starts_with("tier_id"))
        .collect::<Vec<_>>()
        .join("\n");

    let loaded = load_paired(SIMPLE_ASC, &text, false).expect("must still load without tier_id");
    let mut ids = Vec::new();
    for index in 0..loaded.design.tiers.len() {
        let id = loaded
            .design
            .tier_id_at(index)
            .unwrap_or_else(|| panic!("tier {index} must still get a fresh id"));
        ids.push(id);
    }
    let before = ids.len();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), before, "every fresh id must be distinct");
}

/// `.asc` has exactly one RI slot and export writes the EFFECTIVE value there
/// (see `Design::effective_refractive_index`'s own doc comment). Re-importing
/// that `.asc` alone would silently overwrite the AUTHORED (`meta.refractive_index`)
/// figure. The native sidecar's own `authored_refractive_index` must survive a
/// real save/load round trip and restore the true authored value instead.
#[test]
fn authored_refractive_index_round_trips_through_save_and_load_even_when_the_asc_i_line_differs() {
    let mut design = simple_design();
    // `simple_design` pins an override equal to the fixture's own authored RI
    // (see that function's own doc comment) specifically so an UNRELATED test
    // doesn't have to think about this divergence -- removed here so this test
    // can exercise it on purpose.
    design.material.refractive_index_override = None;
    let authored = design.meta.refractive_index;
    assert_ne!(
        design.effective_refractive_index(),
        authored,
        "fixture must actually exercise the authored/effective divergence"
    );

    let saved = save_paired(&design, "design.asc", None, None, None).expect("must save");

    // Importing the paired `.asc` ALONE (no native sidecar at all) derives the
    // EFFECTIVE RI back as `meta.refractive_index` -- proving this fixture
    // really does exercise the lossy round trip `authored_refractive_index`
    // exists to fix, not one that happens to already agree.
    let schedule = indicatrix_formats::asc::parse_asc(&saved.asc_text).expect("must parse");
    let asc_only = Design::from_asc_schedule(design.preform, &schedule);
    assert_ne!(asc_only.meta.refractive_index, authored);

    let loaded = load_paired(&saved.asc_text, &saved.native_toml, false).expect("must load back");
    assert_eq!(
        loaded.design.meta.refractive_index, authored,
        "the AUTHORED RI must survive, not the effective one a plain .asc \
         re-import would derive"
    );
}

/// A note surviving a full `Edit::History` add-tier/remove-tier/move-tier
/// sequence, then a real save/load round trip -- the index-maintenance
/// contract `crate::edit::tests` already exercises against `History` alone,
/// checked here end to end through the native file format too.
///
/// Built on [`unsolved_design`] (a single `MeetExisting` tier, no
/// scale-reference anchor anywhere) rather than [`simple_design`]: this test
/// needs the draft save path deterministically, and `unsolved_design` can
/// never solve regardless of how its tiers get shuffled, while whether
/// `simple_design`'s own `MeetExisting` tier happens to solve after a reorder
/// is exactly the kind of thing that could flip out from under this test.
#[test]
fn tier_notes_survive_add_remove_and_reorder_then_a_save_load_round_trip() {
    use crate::edit::{Edit, History};

    let mut design = unsolved_design();
    design.tiers.push(tier_named("B"));
    design.tiers.push(tier_named("C"));
    let mut history = History::new();

    // Tier 1 ("B") gets a note.
    history
        .apply(
            &mut design,
            Edit::SetTierNote {
                index: 1,
                note: Some("grind slowly".to_string()),
            },
        )
        .expect("set tier note must apply");

    // Insert a new tier before it: the note must follow to index 2.
    history
        .apply(
            &mut design,
            Edit::AddTier {
                index: 0,
                tier: tier_named("Z"),
            },
        )
        .expect("add must apply");
    assert_eq!(design.tier_note(1), None);
    assert_eq!(design.tier_note(2), Some("grind slowly"));

    // Remove tier 0 (the one just inserted): the note must shift back to 1.
    history
        .apply(&mut design, Edit::RemoveTier { index: 0 })
        .expect("remove must apply");
    assert_eq!(design.tier_note(1), Some("grind slowly"));

    // Move the noted tier (1) to the end: the note must follow it.
    let last = design.tiers.len() - 1;
    history
        .apply(&mut design, Edit::MoveTier { from: 1, to: last })
        .expect("move must apply");
    assert_eq!(design.tier_note(1), None);
    assert_eq!(design.tier_note(last), Some("grind slowly"));

    // This design has no scale-reference anchor anywhere, so it never solves --
    // `save_paired` therefore always falls back to a draft, which
    // `load_paired` then rebuilds `tiers`/`tier_notes` wholesale from the
    // sidecar's own array (`TierOverlay::AppliedFromDraft`).
    let saved = save_paired(&design, "design.asc", None, None, None).expect("must save");
    assert!(saved.native.draft, "this design can never solve");
    let loaded = load_paired(&saved.asc_text, &saved.native_toml, false).expect("must load back");
    assert_eq!(loaded.tier_overlay, TierOverlay::AppliedFromDraft);
    assert_eq!(loaded.design.tier_note(last), Some("grind slowly"));
}

/// [`tier_notes_survive_add_remove_and_reorder_then_a_save_load_round_trip`]'s own
/// helper: a minimal, unnamed-index tier good enough to insert as filler.
fn tier_named(name: &str) -> ConstraintTier {
    ConstraintTier {
        angle_deg: 0.0,
        name: name.to_string(),
        indices: Vec::new(),
        constraint: MeetConstraint::MeetExisting,
        imported_meet: None,
        original_notes: None,
        detached: Vec::new(),
    }
}

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

/// A `note`/`cheater_offset_deg` is geometry-free (neither feeds the solver),
/// so it must still be applied when the tier counts agree, even on a fingerprint
/// mismatch that skips the `constraint`/`detached` overlay.
#[test]
fn a_fingerprint_mismatch_still_applies_geometry_free_per_tier_fields() {
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
    // ...but the geometry-free fields were still applied.
    assert_eq!(loaded.design.tier_note(1), Some("check meet here"));
    assert_eq!(loaded.design.cheater_offset_deg(1), Some(-0.75));
}

/// `imported_meet`/`original_notes` must be read back from the sidecar when
/// present, not left at whatever `.asc` import alone produced.
#[test]
fn imported_meet_and_original_notes_are_read_back_from_the_sidecar_when_present() {
    let mut design = simple_design();
    // Tier 1 is `MeetExisting` in `simple_design`, so `.asc` import alone left it
    // with no `imported_meet`/`original_notes` (see `ConstraintTier::imported_meet`'s
    // own doc comment) -- set both explicitly so this test actually exercises the
    // "read back when present" path, not "already there from import".
    design.tiers[1].imported_meet = Some(MeetConstraint::MeetNamed(vec!["Girdle".to_string()]));
    design.tiers[1].original_notes = Some("Meet Girdle".to_string());
    let native = to_native_file(
        &design,
        "design.asc",
        SIMPLE_ASC.as_bytes(),
        None,
        &SaveExtras::default(),
    );
    let native_toml = to_toml_string(&native).expect("must serialize");

    let loaded = load_paired(SIMPLE_ASC, &native_toml, false).expect("must load");
    assert_eq!(loaded.tier_overlay, TierOverlay::Applied);
    assert_eq!(
        loaded.design.tiers[1].imported_meet,
        Some(MeetConstraint::MeetNamed(vec!["Girdle".to_string()]))
    );
    assert_eq!(
        loaded.design.tiers[1].original_notes,
        Some("Meet Girdle".to_string())
    );
}

/// A hand-edited (or otherwise corrupted) draft sidecar missing a tier's
/// `angle_deg`/`indices` must refuse to load rather than silently invent
/// `0.0`/an empty index list, since there is no better geometry to fall back
/// on for a draft.
#[test]
fn a_draft_tier_missing_geometry_is_a_load_error_not_a_silent_default() {
    let design = unsolved_design();
    let saved = save_paired(&design, "draft.asc", None, None, None).expect("must save a draft");
    assert!(saved.native.draft);

    let mut corrupted = saved.native;
    corrupted.tiers[0].angle_deg = None;
    let corrupted_toml = to_toml_string(&corrupted).expect("must serialize");

    let err = load_paired(&saved.asc_text, &corrupted_toml, false)
        .expect_err("a draft tier missing angle_deg must not silently load");
    assert!(matches!(
        err,
        LoadPairedError::DraftTierMissingGeometry { index: 0 }
    ));
}

/// [`save_paired`] must preserve the caller's original `.asc` text byte for byte
/// when the design was not touched in any way that changes its `to_asc_schedule`
/// output. Setting `girdle_diameter_mm`/`material` does NOT round-trip into `.asc`,
/// so it must not trigger a regeneration either. Uses [`FULLY_CANONICAL_ASC`], not
/// `SIMPLE_ASC` -- see that constant's own doc comment for why.
#[test]
fn save_paired_preserves_untouched_text() {
    let design = fully_anchored_design();
    let saved = save_paired(&design, "design.asc", Some(FULLY_CANONICAL_ASC), None, None)
        .expect("must save");
    assert!(saved.asc_preserved);
    assert_eq!(saved.asc_text, FULLY_CANONICAL_ASC);
    assert_eq!(
        saved.native.asc_sha256,
        sha256_hex(FULLY_CANONICAL_ASC.as_bytes())
    );
}

/// Selecting a different BUILT-IN material must not, by itself, force a fresh
/// `.asc` regeneration. `fully_anchored_design`'s own `refractive_index_override:
/// Some(1.62)` exists only to keep `simple_design`'s sibling fixture byte-stable
/// for unrelated tests -- dropping it here and switching to
/// Quartz (`n_D` ~1.544, not the file's own `I 1.62`) is the REAL case: nothing
/// tier/preform-facing changed, only `Design::effective_refractive_index`'s derived
/// output, which the sidecar's own `[material]` table already records separately.
#[test]
fn save_paired_preserves_untouched_text_after_a_material_change_alone() {
    let mut design = fully_anchored_design();
    design.material.refractive_index_override = None;
    design.material.name = Some("Quartz".to_string());
    assert_ne!(
        design.effective_refractive_index(),
        1.62,
        "the test must actually exercise a changed effective RI"
    );

    let saved = save_paired(&design, "design.asc", Some(FULLY_CANONICAL_ASC), None, None)
        .expect("must save");
    assert!(
        saved.asc_preserved,
        "a material change alone must not force a fresh .asc export"
    );
    assert_eq!(saved.asc_text, FULLY_CANONICAL_ASC);
    // The real selection still reaches the sidecar even though the exported `.asc`
    // text kept the file's own original `I 1.62` line.
    assert_eq!(saved.native.material.name.as_deref(), Some("Quartz"));
}

/// The moment a real tier-facing edit changes the schedule, [`save_paired`] must
/// regenerate fresh `.asc` text instead of preserving the (now stale) original.
#[test]
fn save_paired_regenerates_after_a_real_edit() {
    let mut design = simple_design();
    design.tiers[1].constraint = MeetConstraint::ScaleReference(0.5);
    let saved =
        save_paired(&design, "design.asc", Some(SIMPLE_ASC), None, None).expect("must save");
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
    let saved = save_paired(&design, "design.asc", None, None, None).expect("must save");
    assert!(!saved.asc_preserved);
    assert!(indicatrix_formats::asc::parse_asc(&saved.asc_text).is_ok());
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
    let saved = save_paired(&design, "design.asc", Some(FULLY_CANONICAL_ASC), None, None)
        .expect("must save");

    let loaded = load_paired(&saved.asc_text, &saved.native_toml, false).expect("must load");
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
        original_notes: None,
        detached: Vec::new(),
    });

    let saved =
        save_paired(&design, "fresh.asc", None, None, None).expect("a fresh design must save");
    let loaded = load_paired(&saved.asc_text, &saved.native_toml, false).expect("must load back");

    assert_eq!(loaded.design.meta.gear_teeth, 80);
    assert_eq!(loaded.design.meta.symmetry_order, 5);
    assert!(!loaded.design.meta.mirror);
    assert_eq!(loaded.design.material, design.material);
    assert_eq!(loaded.design.preform, design.preform);
    assert_eq!(loaded.design.tiers.len(), 1);
    assert_eq!(loaded.design.tiers[0].angle_deg, 0.0);
}

// --- An explicit override can apply the tier overlay despite a fingerprint mismatch ---

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

// --- Unsolved designs save as drafts instead of erroring ---

/// A design with no scale-reference tier at all -- `Design::solve` cannot produce a
/// mast for it -- built directly (not via `Design::from_asc_schedule`) so its single
/// tier carries a real `imported_meet`/`original_notes`/`detached` for the draft
/// round trip below to actually exercise.
fn unsolved_design() -> Design {
    let mut design = Design::fresh(PreformSpec::block(2.0, 1.0, 2.0), 96, 4, 1.62);
    design.tiers.push(ConstraintTier {
        angle_deg: 30.0,
        name: "A".to_string(),
        indices: vec![0.0, 24.0, 48.0, 72.0],
        constraint: MeetConstraint::MeetExisting,
        imported_meet: Some(MeetConstraint::MeetNamed(vec!["B".to_string()])),
        original_notes: Some("Meet B".to_string()),
        detached: vec![24.0],
    });
    design
}

/// `save_paired` must not error just because `design` does not currently solve: it
/// must fall back to a draft (real angle/indices in the placeholder `.asc`, the full
/// tier list in the native sidecar), and `load_paired` must then rebuild
/// `design.tiers` entirely from that native tier list rather than the placeholder
/// `.asc`'s untrustworthy masts.
#[test]
fn save_paired_falls_back_to_a_draft_when_the_design_does_not_solve() {
    let design = unsolved_design();

    let saved =
        save_paired(&design, "draft.asc", None, None, None).expect("a draft save must not error");
    assert!(saved.draft_reason.is_some());
    assert!(!saved.asc_preserved);
    assert!(saved.native.draft);

    // The placeholder `.asc` is still well-formed, real angle/indices and all --
    // only its mast is not to be trusted.
    let reparsed = indicatrix_formats::asc::parse_asc(&saved.asc_text).expect("must still parse");
    assert_eq!(reparsed.tiers.len(), 1);
    assert_eq!(reparsed.tiers[0].angle_deg, 30.0);
    assert_eq!(reparsed.tiers[0].indices, vec![0.0, 24.0, 48.0, 72.0]);

    let loaded = load_paired(&saved.asc_text, &saved.native_toml, false).expect("must load back");
    assert_eq!(loaded.tier_overlay, TierOverlay::AppliedFromDraft);
    assert_eq!(loaded.design.tiers.len(), 1);
    let tier = &loaded.design.tiers[0];
    assert_eq!(tier.angle_deg, 30.0);
    assert_eq!(tier.indices, vec![0.0, 24.0, 48.0, 72.0]);
    assert_eq!(tier.constraint, MeetConstraint::MeetExisting);
    assert_eq!(
        tier.imported_meet,
        Some(MeetConstraint::MeetNamed(vec!["B".to_string()]))
    );
    assert_eq!(tier.original_notes, Some("Meet B".to_string()));
    assert_eq!(tier.detached, vec![24.0]);
}

// --- Untouched import's own `G`-field notes survive export verbatim ---

/// `attached_files` id 4210 ("pc46019.asc") -- "PC 46.019 For Fun" by Michiko
/// Huyhn. Its real notes (`"Cut to mast depth X."`, `"Set girdle width."`) survive
/// a round trip through the native file format. Copied here from
/// `indicatrix_formats::asc`'s own (private, `#[cfg(test)]`-only) fixture of the
/// same name -- that fixture cannot be reused across the crate boundary.
const ASC_FOR_FUN: &str = "GemCad 5.0\n\
g 96 0.0\n\
y 1 y\n\
I 1.54\n\
H PC 46.019  For Fun\n\
H by Michiko Huyhn\n\
a -44.864054 0.53791082 84 n P1 12 G Cut to mast depth X.\n\
a -90.000000 0.78956831 36 12 84 n G1 60 n G1 G Set stone size.\n\
a 54.575729 0.70935195 12 n C1 84 G Set girdle width.\n\
a 0.000000 0.44755829 96 n U\n\
F Also USFG Newsletter Sep 2013, Facets Jan 2014\n";

/// `attached_files` id 4430 ("pc42060.asc") -- "PC 42.060 Large Texas Star" by
/// Charles `McCoy`. Same reuse rationale as [`ASC_FOR_FUN`]; exercises the bare
/// `"TCP"` instruction and a note this crate doesn't classify at all (`"Make table
/// large enough to show all of the star"`).
const ASC_LARGE_TEXAS_STAR: &str = "GemCad 5.0\n\
g 80 0.0\n\
y 5 y\n\
I 1.61\n\
H PC 42.060  Large Texas Star\n\
H by Charles McCoy\n\
a -40.000000 0.54589773 76 n 1 68 60 52 44 36 28 20 12 4 G TCP\n\
a 40.000000 1.11585176 4 n A 12 20 28 36 44 52 60 68 76 G Establish girdle thickness\n\
a 0.000000 0.72641642 80 n T G Make table large enough to show all of the star\n\
F Leave #4 frosted\n";

/// Imports `asc_text` untouched, solves and re-exports it, and asserts every tier's
/// `G`-field text came back exactly as the file stated it, verbatim across the
/// round trip.
fn assert_untouched_import_preserves_notes_on_export(asc_text: &str) {
    let schedule = indicatrix_formats::asc::parse_asc(asc_text).expect("fixture must parse");
    let design = Design::from_asc_schedule(PreformSpec::block(2.0, 1.0, 2.0), &schedule);
    let solved = design
        .solve()
        .expect("every tier is import-pinned to a ScaleReference, so this always solves");
    let exported = design.to_asc_schedule_from_solved(&solved);

    assert_eq!(exported.tiers.len(), schedule.tiers.len());
    for (original, exported_tier) in schedule.tiers.iter().zip(&exported.tiers) {
        assert_eq!(
            exported_tier.notes, original.notes,
            "tier {:?}: an untouched import's G-field text must survive export verbatim",
            original.name
        );
    }
}

#[test]
fn untouched_import_preserves_g_field_notes_on_export_for_fun() {
    assert_untouched_import_preserves_notes_on_export(ASC_FOR_FUN);
}

#[test]
fn untouched_import_preserves_g_field_notes_on_export_large_texas_star() {
    assert_untouched_import_preserves_notes_on_export(ASC_LARGE_TEXAS_STAR);
}

// --- Tier-less designs save as reopenable drafts, not dead ends ---

/// A brand-new design with zero tiers must still produce a `PairedSave` whose
/// `.asc` half actually parses back (`indicatrix_formats::asc::parse_asc` rejects any
/// file with no `a` facet records at all -- the defect this closes), and whose native
/// sidecar carries the tier-less truth directly so `load_paired` rebuilds an empty
/// tier list rather than ever trusting the placeholder facet record.
#[test]
fn a_tier_less_design_saves_as_a_reopenable_draft() {
    let design = Design::fresh(PreformSpec::block(2.0, 1.0, 2.0), 96, 4, 1.62);
    assert_eq!(design.tiers.len(), 0);

    let saved =
        save_paired(&design, "empty.asc", None, None, None).expect("a tier-less design must save");
    assert!(matches!(saved.draft_reason, Some(DraftReason::NoTiers)));
    assert!(!saved.asc_preserved);
    assert!(saved.native.draft);
    assert!(indicatrix_formats::asc::parse_asc(&saved.asc_text).is_ok());

    let loaded = load_paired(&saved.asc_text, &saved.native_toml, false).expect("must load back");
    assert_eq!(loaded.tier_overlay, TierOverlay::AppliedFromDraft);
    assert_eq!(loaded.design.tiers.len(), 0);
}

// --- Placeholder-derived designs are marked, never mistaken for authored ---

/// `save_paired`'s `placeholder_note` parameter must mark the written `.asc` via
/// `indicatrix_formats::asc::mark_reconstructed` rather than writing a schedule that
/// looks like an ordinary, authored `GemCAD` file.
#[test]
fn placeholder_note_marks_the_written_asc_as_reconstructed() {
    let design = fully_anchored_design();
    let saved = save_paired(
        &design,
        "reconstructed.asc",
        None,
        Some("angle-table reconstruction, no attached .asc"),
        None,
    )
    .expect("must save");

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

// --- Printed proportions round-trip through the sidecar's `[source]` ---

#[test]
fn printed_proportions_round_trip_through_save_and_open() {
    let design = simple_design();
    let props = ExternalProportions {
        vol_w3: Some(0.42),
        lw: Some(1.05),
        cw: Some(0.17),
        pw: Some(0.44),
        hw: Some(0.61),
    };
    let saved = save_paired(&design, "design.asc", None, None, Some(&props))
        .expect("must save with printed proportions");
    assert!(
        saved.native.source.is_some(),
        "a non-empty ExternalProportions must produce a [source] table"
    );

    let loaded = load_paired(&saved.asc_text, &saved.native_toml, false).expect("must load back");
    // `ExternalProportions` derives no `PartialEq`, so each field is checked
    // individually.
    let restored = loaded
        .printed_proportions
        .expect("printed proportions must survive the round trip");
    assert_eq!(restored.vol_w3, props.vol_w3);
    assert_eq!(restored.lw, props.lw);
    assert_eq!(restored.cw, props.cw);
    assert_eq!(restored.pw, props.pw);
    assert_eq!(restored.hw, props.hw);
}

#[test]
fn an_all_none_external_proportions_writes_no_source_table_at_all() {
    let design = simple_design();
    let empty_props = ExternalProportions::default();
    let saved =
        save_paired(&design, "design.asc", None, None, Some(&empty_props)).expect("must save");
    assert!(
        saved.native.source.is_none(),
        "an all-None ExternalProportions should not grow a [source] table nothing can use"
    );
}

#[test]
fn a_sidecar_saved_before_source_existed_loads_with_no_printed_proportions() {
    let design = simple_design();
    // No `printed_proportions` at all -- the shape of a sidecar saved before this
    // field existed.
    let saved = save_paired(&design, "design.asc", None, None, None).expect("must save");
    let loaded = load_paired(&saved.asc_text, &saved.native_toml, false).expect("must load back");
    assert!(loaded.printed_proportions.is_none());
}

// --- `format_version` is written but never checked on load ---

#[test]
fn a_sidecar_saved_by_this_build_never_reports_a_newer_format_version() {
    let design = simple_design();
    let saved = save_paired(&design, "design.asc", None, None, None).expect("must save");
    let loaded = load_paired(&saved.asc_text, &saved.native_toml, false).expect("must load back");
    assert!(!loaded.written_by_newer_version);
}

#[test]
fn a_sidecar_with_a_higher_format_version_is_flagged_as_written_by_a_newer_build() {
    let design = simple_design();
    let saved = save_paired(&design, "design.asc", None, None, None).expect("must save");
    let mut bumped = saved.native;
    bumped.format_version = FORMAT_VERSION + 1;
    let bumped_toml = to_toml_string(&bumped).expect("must serialize");
    let loaded = load_paired(&saved.asc_text, &bumped_toml, false)
        .expect("a version bump alone must not refuse to load");
    assert!(loaded.written_by_newer_version);
}

// --- A custom material's optics survive a save/open round trip instead of
// --- silently resolving to Diamond ---

/// A `[material.custom]` snapshot attached via [`SaveExtras::custom_material`] must
/// actually reach the written sidecar's `material.custom` field.
#[test]
fn a_custom_material_snapshot_is_written_when_supplied_via_save_extras() {
    let mut design = simple_design();
    design.material.name = Some("My Bespoke Gemstone".to_string());
    let snapshot = CustomMaterialSnapshot::new(
        1.62,
        0.017,
        -0.021,
        Some(3.06),
        "Trigonal",
        "UniaxialNegative",
    );
    let extras = SaveExtras {
        custom_material: Some(&snapshot),
        history_entries: &[],
        custom_catalogue: &[],
    };
    let saved =
        save_paired_extended(&design, "design.asc", None, None, None, &extras).expect("must save");
    assert_eq!(saved.native.material.custom, Some(snapshot));
}

/// A design naming an unrecognized material with no `[material.custom]` snapshot
/// (an older sidecar, or one whose material was never actually custom) must report
/// [`MaterialResolution::Unresolved`] but nothing restorable -- there is nothing to
/// reconstruct from.
#[test]
fn an_unresolved_material_with_no_snapshot_has_nothing_restorable() {
    let mut design = simple_design();
    design.material.name = Some("My Bespoke Gemstone".to_string());
    let saved = save_paired(&design, "design.asc", None, None, None).expect("must save");

    let loaded = load_paired(&saved.asc_text, &saved.native_toml, false).expect("must load back");
    assert_eq!(loaded.material_resolution, MaterialResolution::Unresolved);
    assert_eq!(loaded.restorable_custom_material, None);
}

/// The actual fix: a custom material's snapshot, once attached on save, comes back
/// out of `load_paired` as `restorable_custom_material` -- a caller (the editor) can
/// then reconstruct the real material via `gem_material_from_custom_snapshot` and
/// register it, instead of `MaterialSelection::resolve` silently falling back to
/// Diamond.
#[test]
fn an_unresolved_material_with_a_snapshot_is_restorable_after_a_round_trip() {
    let mut design = simple_design();
    design.material.name = Some("My Bespoke Gemstone".to_string());
    let snapshot = CustomMaterialSnapshot::new(
        1.62,
        0.017,
        -0.021,
        Some(3.06),
        "Trigonal",
        "UniaxialNegative",
    );
    let extras = SaveExtras {
        custom_material: Some(&snapshot),
        history_entries: &[],
        custom_catalogue: &[],
    };
    let saved =
        save_paired_extended(&design, "design.asc", None, None, None, &extras).expect("must save");

    let loaded = load_paired(&saved.asc_text, &saved.native_toml, false).expect("must load back");
    assert_eq!(loaded.material_resolution, MaterialResolution::Unresolved);
    assert_eq!(loaded.restorable_custom_material, Some(snapshot.clone()));

    let rebuilt = gem_material_from_custom_snapshot("My Bespoke Gemstone", &snapshot);
    assert_eq!(rebuilt.name, "My Bespoke Gemstone");
    assert!((f64::from(rebuilt.birefringence_delta) - (-0.021)).abs() < 1e-6);
}

/// A material that DOES resolve against the built-in table must never be flagged as
/// restorable, even if a snapshot happens to be present (defensive -- this should
/// never occur in practice, since a caller only ever attaches a snapshot for a
/// material its own catalogue considers custom).
#[test]
fn a_known_material_is_never_flagged_as_restorable_even_with_a_snapshot_present() {
    let design = simple_design(); // material.name == Some("Diamond")
    let snapshot =
        CustomMaterialSnapshot::new(1.62, 0.017, -0.021, None, "Trigonal", "UniaxialNegative");
    let extras = SaveExtras {
        custom_material: Some(&snapshot),
        history_entries: &[],
        custom_catalogue: &[],
    };
    let saved =
        save_paired_extended(&design, "design.asc", None, None, None, &extras).expect("must save");

    let loaded = load_paired(&saved.asc_text, &saved.native_toml, false).expect("must load back");
    assert_eq!(loaded.material_resolution, MaterialResolution::Known);
    assert_eq!(loaded.restorable_custom_material, None);
}

// --- `save_paired_extended_from_solved` must agree byte for byte with
// --- `save_paired_extended` for a design that actually solves, since the latter
// --- is a thin wrapper that solves once and delegates to the former. ---

/// For a solved design, `save_paired_extended(design, ...)` and
/// `save_paired_extended_from_solved(design, &design.solve().unwrap(), ...)` must
/// produce an identical [`PairedSave`] -- same `asc_text`/`asc_preserved`/
/// `native_toml`/`draft_reason`. `PairedSave`/`NativeDesignFile` derive no
/// `PartialEq`, so each field is checked individually.
#[test]
fn save_paired_extended_and_from_solved_agree_for_a_solved_design() {
    let design = fully_anchored_design();
    let solved = design.solve().expect("fully_anchored_design always solves");

    let via_extended = save_paired_extended(
        &design,
        "design.asc",
        Some(FULLY_CANONICAL_ASC),
        None,
        None,
        &SaveExtras::default(),
    )
    .expect("save_paired_extended must save");
    let via_solved = save_paired_extended_from_solved(
        &design,
        &solved,
        "design.asc",
        Some(FULLY_CANONICAL_ASC),
        None,
        None,
        &SaveExtras::default(),
    )
    .expect("save_paired_extended_from_solved must save");

    assert_eq!(via_extended.asc_text, via_solved.asc_text);
    assert_eq!(via_extended.asc_preserved, via_solved.asc_preserved);
    assert_eq!(via_extended.native_toml, via_solved.native_toml);
    assert!(via_extended.draft_reason.is_none());
    assert!(via_solved.draft_reason.is_none());
}

// --- A design on a CUSTOM catalogue material exports its own RI, not the legacy
// --- schedule field, once a caller populates `SaveExtras::custom_catalogue` (the
// --- wrong-`I`-line fix). ---

/// A [`GemMaterial`] named `"My Garnet"` with a chosen `n_D`, mirroring
/// `crates/indicatrix-cut-core/src/design/export.rs`'s own test fixture of the same
/// name.
fn custom_garnet(n_d: f32) -> indicatrix::optics::materials::GemMaterial {
    let mut gem = indicatrix::optics::materials::GemMaterial::diamond();
    gem.name = "My Garnet".to_string();
    gem.dispersion = indicatrix::optics::dispersion::DispersionModel::Cauchy {
        a: n_d,
        b: 0.0,
        c: 0.0,
    };
    gem
}

/// `save_paired_extended` must write an `I` line matching a CUSTOM catalogue
/// material's own `n_D` (here 1.9) once `extras.custom_catalogue` names it, while
/// the same call with an empty catalogue -- given the exact same design -- falls
/// back to the legacy schedule RI. No `original_asc_text` is supplied, so both
/// saves freshly regenerate `asc_text` rather than preserving stale text (which
/// would mask the very `I`-line difference this test checks).
#[test]
fn save_paired_extended_resolves_a_custom_materials_own_refractive_index() {
    let mut design = fully_anchored_design();
    design.material = MaterialSelection {
        name: Some("My Garnet".to_string()),
        specific_gravity_override: None,
        refractive_index_override: None,
    };
    let custom = [custom_garnet(1.9)];

    let with_catalogue = save_paired_extended(
        &design,
        "design.asc",
        None,
        None,
        None,
        &SaveExtras {
            custom_material: None,
            history_entries: &[],
            custom_catalogue: &custom,
        },
    )
    .expect("save_paired_extended must save");
    let with_catalogue_schedule = indicatrix_formats::asc::parse_asc(&with_catalogue.asc_text)
        .expect("freshly exported .asc must parse");
    assert!((with_catalogue_schedule.refractive_index - 1.9).abs() < 1e-6);

    let built_ins_only = save_paired_extended(
        &design,
        "design.asc",
        None,
        None,
        None,
        &SaveExtras::default(),
    )
    .expect("save_paired_extended must save");
    let built_ins_only_schedule = indicatrix_formats::asc::parse_asc(&built_ins_only.asc_text)
        .expect("freshly exported .asc must parse");
    assert_eq!(
        built_ins_only_schedule.refractive_index,
        design.meta.refractive_index
    );
}

/// Same check via the already-solved entry point `save_paired_extended_from_solved`.
#[test]
fn save_paired_extended_from_solved_resolves_a_custom_materials_own_refractive_index() {
    let mut design = fully_anchored_design();
    design.material = MaterialSelection {
        name: Some("My Garnet".to_string()),
        specific_gravity_override: None,
        refractive_index_override: None,
    };
    let custom = [custom_garnet(1.9)];
    let solved = design.solve().expect("fully_anchored_design always solves");

    let with_catalogue = save_paired_extended_from_solved(
        &design,
        &solved,
        "design.asc",
        None,
        None,
        None,
        &SaveExtras {
            custom_material: None,
            history_entries: &[],
            custom_catalogue: &custom,
        },
    )
    .expect("save_paired_extended_from_solved must save");
    let with_catalogue_schedule = indicatrix_formats::asc::parse_asc(&with_catalogue.asc_text)
        .expect("freshly exported .asc must parse");
    assert!((with_catalogue_schedule.refractive_index - 1.9).abs() < 1e-6);

    let built_ins_only = save_paired_extended_from_solved(
        &design,
        &solved,
        "design.asc",
        None,
        None,
        None,
        &SaveExtras::default(),
    )
    .expect("save_paired_extended_from_solved must save");
    let built_ins_only_schedule = indicatrix_formats::asc::parse_asc(&built_ins_only.asc_text)
        .expect("freshly exported .asc must parse");
    assert_eq!(
        built_ins_only_schedule.refractive_index,
        design.meta.refractive_index
    );
}

/// A design on a recognized built-in ("Diamond") must export byte-identical
/// `asc_text`/`native_toml` whether or not an UNRELATED custom catalogue is
/// supplied -- built-ins take precedence over nothing here, since `custom` has no
/// "Diamond" entry of its own.
#[test]
fn save_paired_extended_matches_byte_for_byte_on_a_built_in_regardless_of_catalogue() {
    let mut design = fully_anchored_design();
    design.material = MaterialSelection {
        name: Some("Diamond".to_string()),
        specific_gravity_override: None,
        refractive_index_override: None,
    };
    let custom = [custom_garnet(1.9)];

    let without_catalogue = save_paired_extended(
        &design,
        "design.asc",
        None,
        None,
        None,
        &SaveExtras::default(),
    )
    .expect("save_paired_extended must save");
    let with_catalogue = save_paired_extended(
        &design,
        "design.asc",
        None,
        None,
        None,
        &SaveExtras {
            custom_material: None,
            history_entries: &[],
            custom_catalogue: &custom,
        },
    )
    .expect("save_paired_extended must save");

    assert_eq!(without_catalogue.asc_text, with_catalogue.asc_text);
    assert_eq!(without_catalogue.native_toml, with_catalogue.native_toml);
    let schedule =
        indicatrix_formats::asc::parse_asc(&without_catalogue.asc_text).expect("must parse");
    assert!((schedule.refractive_index - 2.417).abs() < 1e-3);
}

// --- A bounded history trail round-trips through the sidecar ---

/// [`SaveExtras::history_entries`] must reach the written sidecar's `[history]`
/// table and come back out of `load_paired` unchanged.
#[test]
fn history_entries_round_trip_through_save_and_load() {
    let design = simple_design();
    let entries = vec![
        "Set material to Diamond".to_string(),
        "Remove tier C1".to_string(),
    ];
    let extras = SaveExtras {
        custom_material: None,
        history_entries: &entries,
        custom_catalogue: &[],
    };
    let saved =
        save_paired_extended(&design, "design.asc", None, None, None, &extras).expect("must save");
    assert!(saved.native.history.is_some());

    let loaded = load_paired(&saved.asc_text, &saved.native_toml, false).expect("must load back");
    assert_eq!(loaded.history_entries, entries);
}

/// An empty history (a brand-new design's first save) must write no `[history]`
/// table at all, and load back as an empty trail.
#[test]
fn an_empty_history_writes_no_history_table_at_all() {
    let design = simple_design();
    let saved = save_paired(&design, "design.asc", None, None, None).expect("must save");
    assert!(saved.native.history.is_none());

    let loaded = load_paired(&saved.asc_text, &saved.native_toml, false).expect("must load back");
    assert_eq!(loaded.history_entries, Vec::<String>::new());
}

/// [`crate::edit::History::description_log`] itself must accumulate one entry per
/// applied edit, in order -- the source `save_paired_extended`'s caller draws
/// `history_entries` from.
#[test]
fn history_description_log_accumulates_applied_edits_in_order() {
    let mut design = simple_design();
    let mut history = crate::edit::History::new();
    history
        .apply(
            &mut design,
            crate::edit::Edit::SetGirdleDiameterMm {
                girdle_diameter_mm: Some(7.0),
            },
        )
        .expect("must apply");
    history
        .apply(
            &mut design,
            crate::edit::Edit::SetPreformYOffset { y_offset: 0.3 },
        )
        .expect("must apply");
    assert_eq!(history.description_log().len(), 2);
    assert_eq!(
        history.description_log()[0],
        "Set girdle diameter to 7.00 mm"
    );
    assert_eq!(history.description_log()[1], "Set preform offset to 0.30");
}

// --- The preform's vertical-span offset survives a native round trip ---

/// `Design::preform_y_offset` must round-trip through save/open exactly like
/// `girdle_diameter_mm` already does.
#[test]
fn preform_y_offset_round_trips_through_save_and_load() {
    let mut design = simple_design();
    design.preform_y_offset = 0.35;
    let saved = save_paired(&design, "design.asc", None, None, None).expect("must save");
    assert!((saved.native.preform.y_offset - 0.35).abs() < 1e-12);

    let loaded = load_paired(&saved.asc_text, &saved.native_toml, false).expect("must load back");
    assert!((loaded.design.preform_y_offset - 0.35).abs() < 1e-12);
}

/// A sidecar saved before `PreformTable::y_offset` existed must load as `0.0` -- the
/// always-centred span every such file's preform actually had.
#[test]
fn a_preform_table_with_no_y_offset_key_loads_as_zero() {
    let design = simple_design();
    let native = to_native_file(
        &design,
        "design.asc",
        SIMPLE_ASC.as_bytes(),
        None,
        &SaveExtras::default(),
    );
    let mut text = to_toml_string(&native).expect("must serialize");
    // Simulate a sidecar saved before this field existed, by stripping the
    // (freshly written) key.
    text = text
        .lines()
        .filter(|line| !line.starts_with("y_offset"))
        .collect::<Vec<_>>()
        .join("\n");
    let parsed = parse_toml_string(&text).expect("must still parse without y_offset");
    assert_eq!(parsed.preform.y_offset, 0.0);
}

// --- Self-contained (no paired
// --- `.asc`) native-only save/load, for autosave restore ------------------------

/// The autosave round trip: `save_native_only` then
/// `load_native_only`, with NO `.asc` text anywhere in the picture, must restore
/// every field the acceptance criterion names -- tiers (angle/indices/constraint/
/// detached), material, girdle diameter, cheater offsets and tier notes -- plus the
/// `.asc`-only header fields (`meta`) an ordinary native file never carries at all.
#[test]
fn save_native_only_then_load_native_only_round_trips_the_full_design() {
    let mut design = simple_design();
    design
        .tier_notes
        .insert(0, "polish this one last".to_string());
    design.cheater_offsets_deg.insert(2, 1.25);
    // `tier_id`/`target` must round-trip through the self-contained (no paired
    // `.asc`) path too, not just `load_paired`'s.
    let tier1_id = design.tier_id_at(1).expect("tier 1 must have an id");
    design
        .tier_targets
        .insert(tier1_id, TierTarget::TableWidthMm(4.1));

    let native_toml = save_native_only_toml(&design, "design.asc", None, &SaveExtras::default())
        .expect("must serialize");

    let loaded = load_native_only(&native_toml).expect("must load with no .asc present");

    assert_eq!(loaded.design.preform, design.preform);
    assert_eq!(loaded.design.meta, design.meta);
    assert_eq!(loaded.design.tiers.len(), design.tiers.len());
    for (loaded_tier, original_tier) in loaded.design.tiers.iter().zip(&design.tiers) {
        assert_eq!(loaded_tier.angle_deg, original_tier.angle_deg);
        assert_eq!(loaded_tier.indices, original_tier.indices);
        assert_eq!(loaded_tier.constraint, original_tier.constraint);
        assert_eq!(loaded_tier.detached, original_tier.detached);
    }
    assert_eq!(loaded.design.girdle_diameter_mm, design.girdle_diameter_mm);
    assert_eq!(loaded.design.material, design.material);
    assert_eq!(loaded.design.tier_notes, design.tier_notes);
    assert_eq!(
        loaded.design.cheater_offsets_deg,
        design.cheater_offsets_deg
    );
    for index in 0..design.tiers.len() {
        assert_eq!(loaded.design.tier_id_at(index), design.tier_id_at(index));
    }
    assert_eq!(
        loaded.design.tier_target(1),
        Some(TierTarget::TableWidthMm(4.1))
    );
}

/// `load_native_only` must refuse (not silently guess `gear_teeth`/`symmetry_order`
/// defaults) an ORDINARY paired-mode native file -- one that never went through
/// `save_native_only` -- since such a file's `tiers` carry no `angle_deg`/`indices`
/// at all, and it carries no stashed `meta` either.
#[test]
fn load_native_only_refuses_an_ordinary_paired_mode_file() {
    let design = simple_design();
    let saved = save_paired(&design, "design.asc", None, None, None).expect("must save");

    let err = load_native_only(&saved.native_toml)
        .expect_err("an ordinary overlay-only native file has no self-contained meta");
    assert!(matches!(err, LoadNativeOnlyError::NotSelfContained));
}

/// A draft save (`design` does not currently solve) already writes full per-tier
/// `angle_deg`/`indices` via [`super::save::draft_tier_tables`] -- but NOT the
/// stashed `meta` [`save_native_only`] adds, so it must still be refused by
/// `load_native_only` as not self-contained, distinguishing "has full tier
/// geometry" from "is actually openable with no `.asc` at all."
#[test]
fn load_native_only_refuses_an_ordinary_draft_save_with_no_stashed_meta() {
    let mut design = simple_design();
    // No `ScaleReference` tier anywhere -> `design.solve()` fails -> draft save.
    for tier in &mut design.tiers {
        tier.constraint = MeetConstraint::MeetExisting;
    }
    let saved = save_paired(&design, "design.asc", None, None, None).expect("must draft-save");
    assert!(saved.draft_reason.is_some());

    let err = load_native_only(&saved.native_toml)
        .expect_err("a plain draft save carries no stashed schedule meta");
    assert!(matches!(err, LoadNativeOnlyError::NotSelfContained));
}

/// `printed_proportions`/history round-trip through the self-contained path exactly
/// like the paired path (Items 172/178) -- same mechanism, same expectation.
#[test]
fn save_native_only_round_trips_printed_proportions_and_history() {
    let design = simple_design();
    let props = ExternalProportions {
        vol_w3: Some(0.5),
        lw: Some(1.0),
        cw: Some(0.55),
        pw: Some(0.43),
        hw: Some(0.6),
    };
    let extras = SaveExtras {
        history_entries: &["Set girdle diameter to 6.50mm".to_string()],
        ..SaveExtras::default()
    };
    let native_toml = save_native_only_toml(&design, "design.asc", Some(&props), &extras)
        .expect("must serialize");

    let loaded = load_native_only(&native_toml).expect("must load");
    // `ExternalProportions` derives no `PartialEq` (see
    // `printed_proportions_round_trip_through_save_and_open`'s own comment) --
    // each field is checked individually.
    let restored = loaded
        .printed_proportions
        .expect("printed proportions must survive the round trip");
    assert_eq!(restored.vol_w3, props.vol_w3);
    assert_eq!(restored.lw, props.lw);
    assert_eq!(restored.cw, props.cw);
    assert_eq!(restored.pw, props.pw);
    assert_eq!(restored.hw, props.hw);
    assert_eq!(
        loaded.history_entries,
        vec!["Set girdle diameter to 6.50mm".to_string()]
    );
}
