//! `imported_meet`/`original_notes` round-tripping through the design file, and the
//! draft path: a design that cannot currently solve (or has zero tiers) still yields a
//! placeholder catalogue `.asc` (never an error), its design file still holds every
//! tier in full, and a tier without geometry is refused rather than silently invented.

use super::fixtures::{design_text, reopen, simple_design, unsolved_design};
use crate::{
    native::{
        DesignExtras, DesignLoadError, DraftReason, catalogue_asc, design_from_file, design_to_file,
    },
    preform::PreformSpec,
};
use indicatrix::geometry::meet_solver::MeetConstraint;

/// `imported_meet`/`original_notes` must be read back from the file when present, not
/// left at whatever `.asc` import alone produced.
#[test]
fn imported_meet_and_original_notes_are_read_back_from_the_file_when_present() {
    let mut design = simple_design();
    // Tier 1 is `MeetExisting` in `simple_design`, so `.asc` import alone left it
    // with no `imported_meet`/`original_notes` (see `ConstraintTier::imported_meet`'s
    // own doc comment) -- set both explicitly so this test actually exercises the
    // "read back when present" path, not "already there from import".
    design.tiers[1].imported_meet = Some(MeetConstraint::MeetNamed(vec!["Girdle".to_string()]));
    design.tiers[1].original_notes = Some("Meet Girdle".to_string());

    let loaded = reopen(&design);
    assert_eq!(
        loaded.design.tiers[1].imported_meet,
        Some(MeetConstraint::MeetNamed(vec!["Girdle".to_string()]))
    );
    assert_eq!(
        loaded.design.tiers[1].original_notes,
        Some("Meet Girdle".to_string())
    );
}

/// A hand-edited (or otherwise corrupted) design file whose tier lacks
/// `angle_deg`/`indices` must refuse to load rather than silently invent `0.0`/an
/// empty index list, since there is no better geometry to fall back on.
#[test]
fn a_tier_missing_geometry_is_a_load_error_not_a_silent_default() {
    let design = unsolved_design();
    let mut file = design_to_file(&design, None, &DesignExtras::default()).with_draft(true);
    file.tiers[0].angle_deg = None;

    let err = design_from_file(file).expect_err("a tier missing angle_deg must not silently load");
    assert!(matches!(
        err,
        DesignLoadError::TierMissingGeometry { index: 0 }
    ));
}

/// `catalogue_asc` must not error just because `design` does not currently solve: it
/// must fall back to a draft (real angle/indices in the placeholder `.asc`), while the
/// design file keeps the full tier list, so `design_from_str` rebuilds `design.tiers`
/// entirely from the file rather than the placeholder `.asc`'s untrustworthy masts.
#[test]
fn catalogue_asc_falls_back_to_a_draft_when_the_design_does_not_solve() {
    let design = unsolved_design();

    let saved = catalogue_asc(&design, None, None).expect("a draft must not error");
    assert!(saved.draft_reason.is_some());
    assert!(!saved.asc_preserved);

    // The placeholder `.asc` is still well-formed, real angle/indices and all --
    // only its mast is not to be trusted.
    let reparsed = indicatrix_formats::asc::parse_asc(&saved.asc_text).expect("must still parse");
    assert_eq!(reparsed.tiers.len(), 1);
    assert_eq!(reparsed.tiers[0].angle_deg, 30.0);
    assert_eq!(reparsed.tiers[0].indices, vec![0.0, 24.0, 48.0, 72.0]);

    let file = design_to_file(&design, None, &DesignExtras::default()).with_draft(true);
    let loaded = design_from_file(file).expect("must load back");
    assert!(loaded.draft);
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

/// A design that solves yields an ordinary (non-draft) catalogue `.asc`.
#[test]
fn a_design_that_solves_yields_no_draft_reason() {
    let saved =
        catalogue_asc(&super::fixtures::fully_anchored_design(), None, None).expect("must write");
    assert!(saved.draft_reason.is_none());
}

/// A brand-new design with zero tiers must still produce a catalogue `.asc` that
/// actually parses back (`indicatrix_formats::asc::parse_asc` rejects any file with no
/// `a` facet records at all -- the defect this closes), and whose design file carries
/// the tier-less truth directly so the reopened design has an empty tier list rather
/// than ever trusting the placeholder facet record.
#[test]
fn a_tier_less_design_saves_as_a_reopenable_draft() {
    let design = crate::design::Design::fresh(PreformSpec::block(2.0, 1.0, 2.0), 96, 4, 1.62);
    assert_eq!(design.tiers.len(), 0);

    let saved = catalogue_asc(&design, None, None).expect("a tier-less design must write");
    assert!(matches!(saved.draft_reason, Some(DraftReason::NoTiers)));
    assert!(!saved.asc_preserved);
    assert!(indicatrix_formats::asc::parse_asc(&saved.asc_text).is_ok());

    let loaded = reopen(&design);
    assert_eq!(loaded.design.tiers.len(), 0);
    assert_ne!(design_text(&design), "");
}
