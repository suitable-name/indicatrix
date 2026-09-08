use super::*;
use crate::{design::Design, preform::PreformSpec};
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
