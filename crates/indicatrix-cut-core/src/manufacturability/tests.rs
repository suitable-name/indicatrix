use super::*;
use crate::{
    design::{Design, TierId},
    preform::PreformSpec,
};
use indicatrix::geometry::{
    meet_solver::{MeetConstraint, SolvedTier},
    stone_metrics::SolidStatus,
};

fn tier(
    name: &str,
    angle_deg: f64,
    constraint: MeetConstraint,
    indices: &[f64],
) -> crate::design::ConstraintTier {
    crate::design::ConstraintTier {
        angle_deg,
        name: name.to_string(),
        indices: indices.to_vec(),
        constraint,
        imported_meet: None,
        original_notes: None,
        detached: Vec::new(),
    }
}

fn solved(design: &Design) -> Vec<SolvedTier> {
    design.solve().expect("test design must solve")
}

/// A closed, unremarkable design (block preform, one flat table facet
/// well inside it) must report no warnings at all -- the baseline every
/// other test's "something IS wrong" assertion is checked against.
#[test]
fn an_unremarkable_closed_design_has_no_warnings() {
    let mut design = Design::fresh(PreformSpec::block(1.0, 1.0, 2.0), 96, 4, 1.62);
    design
        .tiers
        .push(tier("T", 0.0, MeetConstraint::ScaleReference(0.3), &[]));
    let solved = solved(&design);
    let warnings = check_manufacturability(&design, &solved, DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2);
    assert!(warnings.is_empty(), "{warnings:?}");
}

/// A facet cut so deep it is entirely swallowed by a later, shallower
/// facet on the same side must be reported as vanishing -- its plane
/// index is simply absent from `SolidMesh::rings`.
#[test]
fn a_facet_engulfed_by_a_later_shallower_cut_is_reported_as_vanishing() {
    let mut design = Design::fresh(PreformSpec::block(1.0, 1.0, 2.0), 96, 4, 1.62);
    // A deep crown facet at mast 0.9 -- with the block preform's own
    // half-width of 1.0 and a 90-degree crown side wall nowhere in this
    // schedule, this is still inside the preform, but...
    design.tiers.push(tier(
        "Deep",
        30.0,
        MeetConstraint::ScaleReference(0.95),
        &[0.0, 24.0, 48.0, 72.0],
    ));
    // ...a later, much shallower crown facet at the SAME angle and
    // indices as an entirely separate tier's closing table replaces it:
    // a flat table at a shallow mast cuts every one of "Deep"'s facets
    // away before they can reach the surface.
    design
        .tiers
        .push(tier("T", 0.0, MeetConstraint::ScaleReference(0.2), &[]));
    let status = design.status().expect("must solve");
    assert!(
        matches!(status, SolidStatus::Closed(_)),
        "fixture must actually close: {status:?}"
    );

    let solved = solved(&design);
    let warnings = check_manufacturability(&design, &solved, DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2);
    let vanishing: Vec<_> = warnings
        .iter()
        .filter(|w| matches!(w, ManufacturabilityWarning::VanishingFacet { .. }))
        .collect();
    assert_eq!(
        vanishing.len(),
        1,
        "expected exactly the 'Deep' tier reported vanishing: {warnings:?}"
    );
    assert!(matches!(
        vanishing[0],
        ManufacturabilityWarning::VanishingFacet {
            tier_index: 0,
            vanished: 4,
            total: 4,
            ..
        }
    ));
}

/// A facet whose surviving polygon is a tiny sliver (cut extremely close
/// to a corner) must be reported as undersized, at a threshold big enough
/// to catch it but small enough to leave the fixture's other, ordinary
/// facets alone.
///
/// Constructed from exact corner geometry rather than a guessed
/// angle/mast: a unit half-extent cube (`PreformSpec::block(1,1,2)`) has
/// a top corner at `(1,1,1)`. A crown facet whose normal is exactly
/// `(1,1,1)/sqrt(3)` -- `angle_deg = atan(sqrt(2))` (the crown angle
/// whose `cos(theta) = 1/sqrt(3)`), placed at azimuth 45 degrees (index
/// `12` on a 96-tooth gear, `12/96 * 360 = 45`) -- slices that corner off
/// with an equilateral triangle whose vertices sit on the block's own
/// three faces meeting there. With half-space offset `m`, that triangle's
/// side is `sqrt(2) * (3 - m*sqrt(3))` and its area is
/// `(sqrt(3)/2) * (3 - m*sqrt(3))^2` -- an exact figure this test checks
/// its expectation against, not just "some small number". The other
/// three indices (`36`/`60`/`84`, azimuths 135/225/315) place the same
/// cut at the cube's other three top corners by symmetry.
#[test]
fn a_sliver_facet_is_reported_as_undersized() {
    let corner_sum_offset = 3.0 - 0.02; // just short of the corner's own x+y+z = 3
    let mast = corner_sum_offset / 3.0_f64.sqrt();
    let expected_area = (3.0_f64.sqrt() / 2.0) * mast.mul_add(-3.0_f64.sqrt(), 3.0).powi(2);

    let mut design = Design::fresh(PreformSpec::block(1.0, 1.0, 2.0), 96, 4, 1.62);
    design.tiers.push(tier(
        "Corner",
        (1.0 / 3.0_f64.sqrt()).acos().to_degrees(), // acos(1/sqrt(3)) == atan(sqrt(2))
        MeetConstraint::ScaleReference(mast),
        &[12.0, 36.0, 60.0, 84.0],
    ));
    let status = design.status().expect("must solve");
    let SolidStatus::Closed(_) = status else {
        panic!("fixture must close: {status:?}");
    };

    let solved = solved(&design);
    // A generous threshold (10% of W^2 = 10% of 2^2 = 0.4) that the
    // ~0.0003-area sliver trips by three orders of magnitude while
    // trivially clearing for any ordinary facet on this unit-scale cube.
    let warnings = check_manufacturability(&design, &solved, 0.10);
    let undersized: Vec<_> = warnings
        .iter()
        .filter_map(|w| match w {
            ManufacturabilityWarning::UndersizedFacet {
                tier_index, area, ..
            } => Some((*tier_index, *area)),
            _ => None,
        })
        .collect();
    assert_eq!(
        undersized.len(),
        4,
        "all four symmetric corner cuts must be flagged: {warnings:?}"
    );
    for (tier_index, area) in undersized {
        assert_eq!(tier_index, 0);
        assert!(
            (area - expected_area).abs() < 1e-6,
            "measured area {area} != exact corner-triangle area {expected_area}"
        );
    }
}

/// A tier with a fractional index must be reported with the nearest
/// achievable (integer) gear-tooth position and the resulting azimuth
/// error -- and never silently rounded (the design itself keeps the
/// fractional value; this is purely a reported warning).
#[test]
fn a_fractional_index_reports_the_achievable_position_and_error() {
    let mut design = Design::fresh(PreformSpec::block(1.0, 1.0, 2.0), 96, 4, 1.62);
    design.tiers.push(tier(
        "P1",
        -41.0,
        MeetConstraint::ScaleReference(0.5),
        &[0.0, 24.5, 48.0, 72.0],
    ));
    let warnings = check_gear_quantization(&design);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    match &warnings[0] {
        ManufacturabilityWarning::FractionalIndex {
            tier_index,
            requested,
            achievable,
            azimuth_error_deg,
            ..
        } => {
            assert_eq!(*tier_index, 0);
            assert_eq!(*requested, 24.5);
            // `f64::round` rounds half away from zero, so 24.5 -> 25.0 -- checked
            // against `f64::round` itself rather than a hardcoded literal so this
            // assertion can't silently drift from that documented behavior.
            assert_eq!(*achievable, 24.5_f64.round());
            assert!(azimuth_error_deg.abs() > 0.0);
            // Design itself must be untouched -- the check never rounds anything.
            assert_eq!(design.tiers[0].indices[1], 24.5);
        }
        other => panic!("expected FractionalIndex, got {other:?}"),
    }
}

/// Every index landing exactly on an integer tooth must produce no
/// gear-quantization warnings at all.
#[test]
fn integer_indices_produce_no_gear_quantization_warnings() {
    let mut design = Design::fresh(PreformSpec::block(1.0, 1.0, 2.0), 96, 4, 1.62);
    design.tiers.push(tier(
        "P1",
        -41.0,
        MeetConstraint::ScaleReference(0.5),
        &[0.0, 24.0, 48.0, 72.0],
    ));
    assert_eq!(check_gear_quantization(&design), Vec::new());
}

/// A tier that names a LATER tier as its meet target must be flagged --
/// the solver resolves it fine (it doesn't care about file order), but
/// physically that facet doesn't exist yet when this one would be cut.
#[test]
fn a_meet_reference_to_a_later_tier_is_flagged_out_of_order() {
    let mut design = Design::fresh(PreformSpec::block(1.0, 1.0, 2.0), 96, 4, 1.62);
    design.tiers.push(tier(
        "A",
        30.0,
        MeetConstraint::MeetNamed(vec!["B".to_string()]),
        &[0.0, 24.0, 48.0, 72.0],
    ));
    design.tiers.push(tier(
        "B",
        45.0,
        MeetConstraint::ScaleReference(0.6),
        &[0.0, 24.0, 48.0, 72.0],
    ));
    let warnings = check_cut_order(&design);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(matches!(
        &warnings[0],
        ManufacturabilityWarning::OutOfOrderMeet {
            tier_index: 0,
            target_tier_index: 1,
            ..
        }
    ));
}

/// The mirror-image case: a tier naming an EARLIER tier must never be
/// flagged -- that is the ordinary, cuttable case this check exists to
/// distinguish from the one above.
#[test]
fn a_meet_reference_to_an_earlier_tier_is_not_flagged() {
    let mut design = Design::fresh(PreformSpec::block(1.0, 1.0, 2.0), 96, 4, 1.62);
    design.tiers.push(tier(
        "A",
        45.0,
        MeetConstraint::ScaleReference(0.6),
        &[0.0, 24.0, 48.0, 72.0],
    ));
    design.tiers.push(tier(
        "B",
        30.0,
        MeetConstraint::MeetNamed(vec!["A".to_string()]),
        &[0.0, 24.0, 48.0, 72.0],
    ));
    assert_eq!(check_cut_order(&design), Vec::new());
}

/// A design freshly imported via `Design::from_asc_schedule` pins every
/// tier to `ScaleReference` (see that method's own doc comment) --
/// meaning `check_cut_order` must find nothing to flag immediately after
/// import, even when the original file's `G`-field text named a later
/// facet. This is expected, not a check bug: the corpus brief notes this
/// check "fires rarely at first" for exactly this reason, until the user
/// adopts a real `MeetNamed` constraint via `imported_meet`.
#[test]
fn a_freshly_imported_design_has_nothing_to_flag_for_cut_order() {
    let schedule = indicatrix_formats::asc::parse_asc(
        "GemCad 5.0\ng 4 0.0\ny 1 n\nI 1.62\n\
         a 90.000000 1.00000000 0 1 2 3 G Set girdle thickness\n\
         a 0.000000 0.60000000 G Meet future\n\
         a -0.000000 0.55000000 G Set stone size\n\
         a 30.000000 0.50000000 G Set stone size n future\n",
    )
    .expect("must parse");
    let design = Design::from_asc_schedule(PreformSpec::block(2.0, 1.0, 2.0), &schedule);
    assert_eq!(check_cut_order(&design), Vec::new());
}

// --- check_meet_name_asc_safety ---

/// A `MeetNamed` target containing a space must be flagged -- it would split
/// into two unrelated tokens on a plain `.asc` re-parse.
#[test]
fn a_meet_name_with_whitespace_is_flagged_unsafe() {
    let mut design = Design::fresh(PreformSpec::block(1.0, 1.0, 2.0), 96, 4, 1.62);
    design.tiers.push(tier(
        "A",
        30.0,
        MeetConstraint::MeetNamed(vec!["Crown Main".to_string()]),
        &[0.0, 24.0, 48.0, 72.0],
    ));
    let warnings = check_meet_name_asc_safety(&design);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(matches!(
        &warnings[0],
        ManufacturabilityWarning::MeetNameNotAscSafe { tier_index: 0, unsafe_names, .. }
            if unsafe_names == &["Crown Main".to_string()]
    ));
}

/// A plain alphanumeric `MeetNamed` target must never be flagged.
#[test]
fn a_plain_meet_name_is_not_flagged() {
    let mut design = Design::fresh(PreformSpec::block(1.0, 1.0, 2.0), 96, 4, 1.62);
    design.tiers.push(tier(
        "A",
        30.0,
        MeetConstraint::MeetNamed(vec!["Girdle".to_string()]),
        &[0.0, 24.0, 48.0, 72.0],
    ));
    assert_eq!(check_meet_name_asc_safety(&design), Vec::new());
}

/// `check_manufacturability_available` with no solve state must include this
/// check alongside the other two mast-free ones.
#[test]
fn check_manufacturability_available_includes_unsafe_meet_names_with_no_solve() {
    let mut design = Design::fresh(PreformSpec::block(1.0, 1.0, 2.0), 96, 4, 1.62);
    design.tiers.push(tier(
        "A",
        30.0,
        MeetConstraint::MeetNamed(vec!["Main'".to_string()]),
        &[0.0, 24.0, 48.0, 72.0],
    ));
    let warnings =
        check_manufacturability_available(&design, None, DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2);
    assert!(
        warnings
            .iter()
            .any(|w| matches!(w, ManufacturabilityWarning::MeetNameNotAscSafe { .. })),
        "{warnings:?}"
    );
}

// --- check_manufacturability_available ---

/// A design with no `ScaleReference` anchor at all (so `Design::solve` itself
/// returns `MissingAnchor`) must still report its mast-free warnings, not drop
/// them entirely just because there is no anchor to solve against. `solved: None`
/// must not fall back to an empty `Vec`, unlike an editor that early-returns on
/// `design.solve().is_err()`.
#[test]
fn reports_mast_free_warnings_even_with_no_anchor_to_solve() {
    let mut design = Design::fresh(PreformSpec::block(1.0, 1.0, 2.0), 96, 4, 1.62);
    design.tiers.push(tier(
        "P1",
        -41.0,
        MeetConstraint::MeetExisting,
        &[0.0, 24.5],
    ));
    assert!(
        design.solve().is_err(),
        "test design must have no anchor for this to test the right thing"
    );
    let warnings =
        check_manufacturability_available(&design, None, DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(matches!(
        warnings[0],
        ManufacturabilityWarning::FractionalIndex { .. }
    ));
}

/// `solved: Some(..)` must delegate to exactly [`check_manufacturability`] --
/// same warnings, mesh checks included.
#[test]
fn delegates_to_check_manufacturability_when_solved_masts_are_available() {
    let mut design = Design::fresh(PreformSpec::block(1.0, 1.0, 2.0), 96, 4, 1.62);
    design
        .tiers
        .push(tier("T", 0.0, MeetConstraint::ScaleReference(0.3), &[]));
    let solved = solved(&design);
    let available = check_manufacturability_available(
        &design,
        Some(&solved),
        DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2,
    );
    let direct = check_manufacturability(&design, &solved, DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2);
    assert_eq!(available, direct);
}

// --- ManufacturabilityWarning::tier_index / facet_plane_index ---

/// Every variant's `tier_index()` must return exactly the field it was built
/// with -- the accessor a caller uses instead of matching on the variant itself.
#[test]
fn tier_index_reads_back_every_variants_own_field() {
    let vanishing = ManufacturabilityWarning::VanishingFacet {
        tier_index: 3,
        tier_id: TierId(3),
        tier_name: "V".to_string(),
        vanished: 1,
        total: 2,
    };
    let undersized = ManufacturabilityWarning::UndersizedFacet {
        tier_index: 4,
        tier_id: TierId(4),
        tier_name: "U".to_string(),
        facet_plane_index: 7,
        area: 0.001,
        threshold: 0.01,
        width_axis: 1.0,
    };
    let fractional = ManufacturabilityWarning::FractionalIndex {
        tier_index: 5,
        tier_id: TierId(5),
        tier_name: "F".to_string(),
        requested: 24.5,
        achievable: 25.0,
        azimuth_error_deg: 1.0,
    };
    let out_of_order = ManufacturabilityWarning::OutOfOrderMeet {
        tier_index: 6,
        tier_id: TierId(6),
        tier_name: "O".to_string(),
        target_tier_index: 9,
        target_tier_name: "T".to_string(),
    };
    let unsafe_meet_name = ManufacturabilityWarning::MeetNameNotAscSafe {
        tier_index: 7,
        tier_id: TierId(7),
        tier_name: "M".to_string(),
        unsafe_names: vec!["Crown Main".to_string()],
    };
    assert_eq!(vanishing.tier_index(), 3);
    assert_eq!(undersized.tier_index(), 4);
    assert_eq!(fractional.tier_index(), 5);
    assert_eq!(out_of_order.tier_index(), 6);
    assert_eq!(unsafe_meet_name.tier_index(), 7);
    assert_eq!(unsafe_meet_name.facet_plane_index(), None);
}

/// Only `UndersizedFacet` names a facet plane to highlight -- every other variant
/// is about a tier as a whole.
#[test]
fn facet_plane_index_is_only_some_for_undersized_facet() {
    let undersized = ManufacturabilityWarning::UndersizedFacet {
        tier_index: 0,
        tier_id: TierId(0),
        tier_name: "U".to_string(),
        facet_plane_index: 7,
        area: 0.001,
        threshold: 0.01,
        width_axis: 1.0,
    };
    assert_eq!(undersized.facet_plane_index(), Some(7));

    let out_of_order = ManufacturabilityWarning::OutOfOrderMeet {
        tier_index: 0,
        tier_id: TierId(0),
        tier_name: "O".to_string(),
        target_tier_index: 1,
        target_tier_name: "T".to_string(),
    };
    assert_eq!(out_of_order.facet_plane_index(), None);
}

// --- Display: 1-based tier numbers ---

/// `tier_index()` (used to attribute a warning to a row) must stay 0-based, but
/// the `Display` text a cutter actually reads must match the tier table's own
/// 1-based row numbers, matching every other on-screen reference to the same
/// tier.
#[test]
fn display_numbers_tiers_1_based_while_tier_index_stays_0_based() {
    let vanishing = ManufacturabilityWarning::VanishingFacet {
        tier_index: 0,
        tier_id: TierId(0),
        tier_name: "Table".to_string(),
        vanished: 1,
        total: 2,
    };
    assert_eq!(vanishing.tier_index(), 0);
    assert!(
        vanishing.to_string().starts_with("tier 1 (Table)"),
        "got {vanishing}"
    );

    let out_of_order = ManufacturabilityWarning::OutOfOrderMeet {
        tier_index: 2,
        tier_id: TierId(2),
        tier_name: "Crown Main".to_string(),
        target_tier_index: 5,
        target_tier_name: "Pavilion Main".to_string(),
    };
    let text = out_of_order.to_string();
    assert!(text.starts_with("tier 3 (Crown Main)"), "got {text}");
    assert!(text.contains("meets tier 6 (Pavilion Main)"), "got {text}");
}

/// `UndersizedFacet`'s `Display` reports a percentage of stone width
/// (`sqrt(area) / width_axis`), not the raw design-scale area/threshold numbers
/// that meant nothing to a cutter -- a facet at exactly the module's own
/// `DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2` (`1e-4`, i.e. `(1%)^2`) reads back as
/// (approximately) a 1% threshold.
#[test]
fn undersized_facet_display_reports_a_percentage_of_stone_width() {
    let width_axis = 2.0;
    let threshold = DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2 * width_axis * width_axis;
    let half_threshold_area = threshold / 4.0; // sqrt(area) is half of sqrt(threshold)
    let undersized = ManufacturabilityWarning::UndersizedFacet {
        tier_index: 1,
        tier_id: TierId(1),
        tier_name: "Girdle Facet".to_string(),
        facet_plane_index: 3,
        area: half_threshold_area,
        threshold,
        width_axis,
    };
    let text = undersized.to_string();
    assert!(text.starts_with("tier 2 (Girdle Facet)"), "got {text}");
    assert!(text.contains("0.50%"), "got {text}");
    assert!(text.contains("1.00% minimum"), "got {text}");
}
