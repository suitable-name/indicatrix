use super::*;
use crate::{
    design::{ConstraintTier, Design},
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
    let native = to_native_file(&design, "design.asc", SIMPLE_ASC.as_bytes(), None);
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

/// A paired `.asc` that changed since the native file was last saved must be
/// reported as a fingerprint mismatch, and the per-tier overlay must NOT be
/// reapplied on top of the changed geometry. `girdle_diameter_mm`/`material`/
/// `preform` are not tier-indexed, so they are still restored even on a mismatch.
#[test]
fn a_changed_paired_asc_reports_a_mismatch_and_skips_the_tier_overlay() {
    let design = simple_design();
    let native = to_native_file(&design, "design.asc", SIMPLE_ASC.as_bytes(), None);
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
    let mut native = to_native_file(&design, "design.asc", SIMPLE_ASC.as_bytes(), None);
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

// --- Item 14: an explicit override can apply the tier overlay despite a mismatch ---

/// A fingerprint mismatch must not be the last word when the caller explicitly asks
/// for the saved overlay to be applied anyway (e.g. the cutter confirms they still
/// want it) -- as long as the tier counts still agree, `load_paired`'s
/// `apply_overlay_on_mismatch: true` must apply it and report
/// [`TierOverlay::AppliedDespiteMismatch`], distinct from the ordinary
/// [`TierOverlay::Applied`] a clean reload gets.
#[test]
fn apply_overlay_on_mismatch_applies_the_overlay_despite_a_changed_asc() {
    let design = simple_design();
    let native = to_native_file(&design, "design.asc", SIMPLE_ASC.as_bytes(), None);
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

// --- Item 15: an unsolved design saves as a draft instead of erroring ---

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

// --- Item 13: an untouched import's own `G`-field notes survive export verbatim ---

/// `attached_files` id 4210 ("pc46019.asc") -- "PC 46.019 For Fun" by Michiko
/// Huyhn. Its real notes (`"Cut to mast depth X."`, `"Set girdle width."`) are
/// exactly what the old `constraint_notes` used to flatten into a single "Set stone
/// size." on every tier, since every imported tier's `constraint` is pinned to
/// `MeetConstraint::ScaleReference` regardless of what the file said. Copied here
/// from `indicatrix_formats::asc`'s own (private, `#[cfg(test)]`-only) fixture of the
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
/// `G`-field text came back exactly as the file stated it -- the defect Item 13
/// fixes.
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

// --- Item 91: a tier-less design saves as a reopenable draft, not a dead end ---

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

// --- Item 82: a placeholder-derived design's export is marked, never mistaken for
// --- an authored, verified cutting schedule ---

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

// --- Item 178: printed proportions round-trip through the sidecar's `[source]`
// --- table, so Deep Solve still has something to verify a reopened design against.

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
    // `ExternalProportions` derives no `PartialEq` (it lives in `indicatrix`, owned by
    // another lane), so each field is checked individually.
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
    // No `printed_proportions` at all -- the shape every sidecar had before Item 178.
    let saved = save_paired(&design, "design.asc", None, None, None).expect("must save");
    let loaded = load_paired(&saved.asc_text, &saved.native_toml, false).expect("must load back");
    assert!(loaded.printed_proportions.is_none());
}

// --- Item 181: format_version is written but never checked ---

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
