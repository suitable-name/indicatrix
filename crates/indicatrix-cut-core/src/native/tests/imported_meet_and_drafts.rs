//! `imported_meet`/`original_notes` round-tripping through the sidecar, and
//! the draft save path: a design that cannot currently solve (or has zero
//! tiers) still saves as a reopenable draft, never errors, and never
//! silently invents missing geometry on load.

use super::fixtures::{SIMPLE_ASC, simple_design, unsolved_design};
use crate::{
    native::{
        DraftReason, LoadPairedError, SaveExtras, TierOverlay, load_paired, save_paired,
        to_native_file, to_toml_string,
    },
    preform::PreformSpec,
};
use indicatrix::geometry::meet_solver::MeetConstraint;

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

/// A brand-new design with zero tiers must still produce a `PairedSave` whose
/// `.asc` half actually parses back (`indicatrix_formats::asc::parse_asc` rejects any
/// file with no `a` facet records at all -- the defect this closes), and whose native
/// sidecar carries the tier-less truth directly so `load_paired` rebuilds an empty
/// tier list rather than ever trusting the placeholder facet record.
#[test]
fn a_tier_less_design_saves_as_a_reopenable_draft() {
    let design = crate::design::Design::fresh(PreformSpec::block(2.0, 1.0, 2.0), 96, 4, 1.62);
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
