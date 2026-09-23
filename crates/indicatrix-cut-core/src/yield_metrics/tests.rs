use super::{fit::worst_halfspace_violation, *};
use crate::{
    design::{ConstraintTier, Design},
    material::MaterialSelection,
    preform::PreformSpec,
};
use glam::DVec3;
use indicatrix::geometry::meet_solver::MeetConstraint;

/// A design that closes with a real, known volume, whose girdle is anchored to a
/// known real-world diameter -- built once and reused by several tests below so
/// each one's "hand calculation" is checked against the same, simple numbers.
///
/// A block preform `2x2x2` model units (`half_width=1.0`, `length_over_width=1.0`,
/// `depth=2.0`, so `x`/`z` in `[-1,1]` and `y` in `[-1,1]`), with a single flat
/// table tier at mast 0.5 (crown side, angle 0, no indices). Per
/// `indicatrix::geometry::cuts::StandardGemCuts::from_asc_schedule`'s own documented
/// convention, an angle-0, index-less, crown tier produces the plane `+Y <= mast`
/// -- i.e. `y <= 0.5` -- which is STRICTLY TIGHTER than the preform's own `y <=
/// 1.0`, so the table facet is the one that actually caps the top (cutting off
/// the slice `y in (0.5, 1.0]`) while the preform's own floor (`y >= -1.0`) is
/// untouched. The finished solid is therefore `2 (x) * 2 (z) * 1.5 (y, from -1.0
/// to 0.5)`, volume 6.0, `width_axis` 2.0 (the table cuts height only, never
/// `x`/`z`) -- both figures independently confirmed by
/// `gate_1_yield_and_carat_weight_match_a_hand_calculation`'s own printed
/// arithmetic below, not just asserted blind.
fn box_design(girdle_diameter_mm: Option<f64>, material: MaterialSelection) -> Design {
    let mut design = Design::fresh(PreformSpec::block(1.0, 1.0, 2.0), 96, 4, 1.62);
    design.tiers.push(ConstraintTier {
        angle_deg: 0.0,
        name: "T".to_string(),
        indices: vec![],
        constraint: MeetConstraint::ScaleReference(0.5),
        imported_meet: None,
        original_notes: None,
        detached: Vec::new(),
    });
    design.girdle_diameter_mm = girdle_diameter_mm;
    design.material = material;
    design
}

/// Gate 1: yield and finished carat weight for a known design and rough must
/// match a hand calculation, arithmetic shown.
#[test]
fn gate_1_yield_and_carat_weight_match_a_hand_calculation() {
    // Girdle diameter 2.0 model-units wide anchored to 8.0mm real width ->
    // mm_per_unit = 8.0 / 2.0 = 4.0.
    let design = box_design(
        Some(8.0),
        MaterialSelection {
            name: Some("Diamond".to_string()),
            specific_gravity_override: None,
            refractive_index_override: None,
        },
    );
    let solved = design.solve().expect("single anchored tier must solve");
    let report = design.yield_report(&solved);

    // Hand calculation (see `box_design`'s own doc comment for the geometry this
    // is derived from):
    // finished volume = 2 (x) * 2 (z) * 1.5 (y, from -1.0 to +0.5) = 6.0
    // model-units^3; preform volume = 2*2*2 = 8.0 model-units^3.
    // volumetric_yield = 6.0 / 8.0 = 0.75 exactly.
    assert!((report.volumetric_yield.unwrap() - 0.75).abs() < 1e-9);

    // mm_per_unit = girdle_diameter_mm / width_axis = 8.0 / 2.0 = 4.0 (the table
    // facet only cuts height, so width_axis is still exactly the preform's own
    // 2.0).
    assert!((report.mm_per_unit.unwrap() - 4.0).abs() < 1e-9);
    // finished_volume_mm3 = 6.0 model-units^3 * 4.0^3 = 6.0 * 64.0 = 384.0 mm^3.
    assert!((report.finished_volume_mm3.unwrap() - 384.0).abs() < 1e-9);

    // Diamond SG = 3.52 (crate::material). carat = 384.0 * 3.52 / 200 = 6.7584.
    assert!((report.specific_gravity_used.unwrap() - 3.52).abs() < 1e-9);
    let expected_carat: f64 = 384.0 * 3.52 / 200.0;
    assert!((expected_carat - 6.7584).abs() < 1e-9, "{expected_carat}");
    assert!((report.carat_weight.unwrap() - expected_carat).abs() < 1e-9);
}

/// Gate 2: volumetric yield must be unaffected by the SG value -- checked here by
/// varying material (and therefore SG) across three very different values while
/// holding geometry fixed, and confirming `volumetric_yield` never moves.
#[test]
fn gate_2_volumetric_yield_is_independent_of_specific_gravity() {
    let none = box_design(Some(8.0), MaterialSelection::none());
    let diamond = box_design(
        Some(8.0),
        MaterialSelection {
            name: Some("Diamond".to_string()),
            specific_gravity_override: None,
            refractive_index_override: None,
        },
    );
    let heavy_override = box_design(
        Some(8.0),
        MaterialSelection {
            name: None,
            specific_gravity_override: Some(19.3), // arbitrary, e.g. gold-like
            refractive_index_override: None,
        },
    );

    let yields: Vec<f64> = [&none, &diamond, &heavy_override]
        .iter()
        .map(|d| {
            let solved = d.solve().expect("must solve");
            d.yield_report(&solved).volumetric_yield.expect("closed")
        })
        .collect();

    assert!((yields[0] - 0.75).abs() < 1e-9);
    assert_eq!(yields[0].to_bits(), yields[1].to_bits());
    assert_eq!(yields[0].to_bits(), yields[2].to_bits());

    // Meanwhile the carat weight DOES change with SG, confirming this isn't
    // trivially true because carat weight never varies either.
    let solved_diamond = diamond.solve().unwrap();
    let solved_heavy = heavy_override.solve().unwrap();
    let carat_diamond = diamond.yield_report(&solved_diamond).carat_weight.unwrap();
    let carat_heavy = heavy_override
        .yield_report(&solved_heavy)
        .carat_weight
        .unwrap();
    assert!(carat_diamond < carat_heavy);
}

/// Gate 3: changing the girdle diameter must scale volume by the CUBE of the
/// ratio -- checked directly against a hand-computed expectation, not just "some
/// other number".
#[test]
fn gate_3_changing_girdle_diameter_scales_volume_by_the_cube_of_the_ratio() {
    let base = box_design(Some(8.0), MaterialSelection::none());
    let doubled = box_design(Some(16.0), MaterialSelection::none()); // 2x the diameter

    let solved_base = base.solve().unwrap();
    let solved_doubled = doubled.solve().unwrap();
    let v_base = base.yield_report(&solved_base).finished_volume_mm3.unwrap();
    let v_doubled = doubled
        .yield_report(&solved_doubled)
        .finished_volume_mm3
        .unwrap();

    // Doubling the linear scale must multiply volume by 2^3 = 8.
    let expected_doubled = v_base * 8.0;
    assert!(
        v_base.mul_add(-8.0, v_doubled).abs() < 1e-6,
        "base {v_base}, doubled {v_doubled}, expected {expected_doubled}"
    );

    // Volumetric yield itself (mast-unit ratio) must NOT move with the mm scale --
    // ties gates 2 and 3 together: neither material nor mm anchor touches it.
    assert_eq!(
        base.yield_report(&solved_base)
            .volumetric_yield
            .unwrap()
            .to_bits(),
        doubled
            .yield_report(&solved_doubled)
            .volumetric_yield
            .unwrap()
            .to_bits()
    );
}

/// Gate 4: a design larger than its preform must be flagged. Uses the
/// "Octahedron" real fixture (`crate::design`'s own test corpus, PC 11.020) --
/// two tiers (one crown cone, one pavilion cone, 4-fold symmetric) that close
/// into a real bipyramid ENTIRELY ON THEIR OWN, with no preform involvement at
/// all -- exactly the self-closing-facets case [`exceeds_preform`]'s own doc
/// comment explains is the (uncommon but real) precondition for this check to
/// fire.
#[test]
fn gate_4_a_design_larger_than_its_preform_is_flagged() {
    let schedule = indicatrix_formats::asc::parse_asc(
        "GemCad 4.41\ng 96 48.0\ny 4 n\nI 1.54\n\
         H PC 11.020  Octahedron\n\
         a 54.74 0.68041 0 72 48 24\n\
         a -54.74 0.68041 0 24 48 72\n",
    )
    .expect("fixture must parse");

    // A preform deliberately far too small to contain the octahedron the
    // schedule's own facets imply (half-width 0.05, tiny) -- the facets-alone
    // solid measures far bigger than this in every axis.
    let tiny_preform = PreformSpec::block(0.05, 1.0, 0.05);
    let too_small = Design::from_asc_schedule(tiny_preform, &schedule);
    let solved = too_small.solve().expect("every tier is pinned, must solve");
    let fit = exceeds_preform(&too_small, &solved)
        .expect("the octahedron's own facets close to a solid far bigger than this tiny preform");
    assert!(fit.exceeds_any(), "{fit:?}");
    assert!(fit.exceeds_width() || fit.exceeds_length() || fit.exceeds_height());

    // And the mirror case: a preform generously bigger than the octahedron's own
    // facets need must report no fit problem at all. The octahedron's own
    // facets-alone extents (mast 0.68041 at the tetrahedral half-angle
    // 54.7356deg, i.e. acos(1/sqrt(3))) are width/length =
    // 2 * 0.68041 / sin(54.7356deg) ~= 1.6666 and total height =
    // 2 * 0.68041 / cos(54.7356deg) ~= 2.357 -- this preform (half-width 2.0,
    // i.e. width/length axis 4.0; depth 4.0) comfortably exceeds both.
    let roomy_preform = PreformSpec::block(2.0, 1.0, 4.0);
    let fits = Design::from_asc_schedule(roomy_preform, &schedule);
    let solved_fits = fits.solve().expect("must solve");
    assert_eq!(exceeds_preform(&fits, &solved_fits), None);

    // The full report surfaces the same finding for the too-small case.
    let report = too_small.yield_report(&solved);
    assert!(report.preform_fit.is_some());
    let report_fits = fits.yield_report(&solved_fits);
    assert!(report_fits.preform_fit.is_none());
}

/// `exceeds_preform` must measure against the preform
/// SHIFTED by `Design::preform_y_offset`, not always the centred arrangement
/// -- a pure vertical shift never changes the preform's own width/length/
/// total-height (top and bottom move together), so this specifically drives
/// `PreformFit::exceeds_halfspace`, the one figure that genuinely depends on
/// WHERE the preform sits, not just how big it is.
#[test]
fn exceeds_preform_honours_the_preforms_y_offset() {
    let schedule = indicatrix_formats::asc::parse_asc(
        "GemCad 4.41\ng 96 48.0\ny 4 n\nI 1.54\n\
         H PC 11.020  Octahedron\n\
         a 54.74 0.68041 0 72 48 24\n\
         a -54.74 0.68041 0 24 48 72\n",
    )
    .expect("fixture must parse");

    // Same "roomy" preform gate_4 confirms fits with no offset: half-width 2.0,
    // depth 4.0, so before any offset the floor sits at y = -2.0, comfortably
    // below the octahedron's own lower vertex (y ~= -1.1785).
    let roomy_preform = PreformSpec::block(2.0, 1.0, 4.0);
    let mut design = Design::from_asc_schedule(roomy_preform, &schedule);
    let solved = design.solve().expect("every tier is pinned, must solve");
    assert_eq!(
        exceeds_preform(&design, &solved),
        None,
        "must fit with no offset, matching gate_4's own roomy case"
    );

    // Shift the preform up by 1.0: the floor rises from y = -2.0 to y = -1.0,
    // now ABOVE the octahedron's own lower vertex, while total height (and so
    // every extents comparison) is unchanged -- top and bottom moved together.
    design.preform_y_offset = 1.0;
    let fit = exceeds_preform(&design, &solved)
        .expect("the shifted floor must now clip the octahedron's own lower vertex");
    assert!(fit.exceeds_halfspace(), "{fit:?}");
    assert!(
        !fit.exceeds_width() && !fit.exceeds_length() && !fit.exceeds_height(),
        "a pure vertical shift must not change any extents comparison: {fit:?}"
    );
}

#[test]
fn mm_per_unit_rejects_nonsense_inputs() {
    assert_eq!(mm_per_unit(0.0, 2.0), None);
    assert_eq!(mm_per_unit(-1.0, 2.0), None);
    assert_eq!(mm_per_unit(f64::NAN, 2.0), None);
    assert_eq!(mm_per_unit(f64::INFINITY, 2.0), None);
    assert_eq!(mm_per_unit(8.0, 0.0), None);
    assert_eq!(mm_per_unit(8.0, 2.0), Some(4.0));
}

#[test]
fn volumetric_yield_rejects_a_nonpositive_preform_volume() {
    assert_eq!(volumetric_yield(4.0, 0.0), None);
    assert_eq!(volumetric_yield(4.0, -1.0), None);
    assert_eq!(volumetric_yield(4.0, 8.0), Some(0.5));
}

/// A design with no `girdle_diameter_mm` set must report every mm-dependent
/// figure as `None` while still reporting the exact (mast-unit) volumetric yield
/// -- the two numbers' independence cuts both ways.
#[test]
fn no_girdle_diameter_set_reports_no_mm_figures_but_still_reports_yield() {
    let design = box_design(None, MaterialSelection::none());
    let solved = design.solve().unwrap();
    let report = design.yield_report(&solved);
    assert_eq!(report.mm_per_unit, None);
    assert_eq!(report.finished_volume_mm3, None);
    assert_eq!(report.carat_weight, None);
    assert!((report.volumetric_yield.unwrap() - 0.75).abs() < 1e-9);
}

/// A stone entirely inside the preform's own half-spaces must
/// report no violation.
#[test]
fn a_stone_entirely_inside_the_preform_reports_no_halfspace_violation() {
    let preform = PreformSpec::block(1.0, 1.0, 2.0);
    let planes = preform.planes();
    let vertices = [DVec3::new(0.5, 0.5, 0.5), DVec3::new(-0.5, -0.5, -0.5)];
    assert_eq!(worst_halfspace_violation(&vertices, &planes), None);
}

/// For a [`crate::preform::PreformShape::Block`] preform, the
/// exact half-space check must agree with the coarser extents comparison --
/// a vertex sticking `0.05` past a wall on both `+x` and `-x` is exactly the
/// symmetric `0.1`-too-wide case [`PreformFit::exceeds_width`] would also
/// catch.
#[test]
fn box_preform_halfspace_check_agrees_with_the_extents_comparison() {
    let preform = PreformSpec::block(1.0, 1.0, 2.0);
    let planes = preform.planes();
    let vertices = [DVec3::new(1.05, 0.0, 0.0), DVec3::new(-1.05, 0.0, 0.0)];
    let (plane, violation) = worst_halfspace_violation(&vertices, &planes)
        .expect("a vertex 0.05 past the wall must be reported");
    assert!(plane < planes.len());
    assert!((violation - 0.05).abs() < 1e-9, "{violation}");
}

/// The case an extents-only check misses. A box whose AABB
/// exactly matches an octagonal (`Cylinder { sides: 8 }`) preform's own AABB
/// -- both span `[-1, 1]` on `x` and `z`, since the octagon's `phi = 0/90/
/// 180/270` walls sit at exactly `half_width`/`half_length` -- still has
/// corners that poke past the octagon's diagonal (`phi = 45/135/...`) walls,
/// which only a per-vertex half-space test (not the extents alone) catches.
#[test]
fn fits_the_box_extents_but_pokes_out_of_an_octagonal_preform() {
    let preform = PreformSpec::cylinder(8, 1.0, 1.0, 10.0);
    let planes = preform.planes();
    let corners = [
        DVec3::new(1.0, 0.0, 1.0),
        DVec3::new(1.0, 0.0, -1.0),
        DVec3::new(-1.0, 0.0, 1.0),
        DVec3::new(-1.0, 0.0, -1.0),
    ];
    let (plane, violation) = worst_halfspace_violation(&corners, &planes)
        .expect("a box corner must poke past the octagon's diagonal wall");
    assert!(plane < planes.len());
    // The diagonal wall's normal is (1/sqrt(2), 0, 1/sqrt(2)) at offset
    // half_width (1.0); a corner at (+-1, 0, +-1) violates it by exactly
    // sqrt(2) - 1.
    assert!(
        (violation - (2.0_f64.sqrt() - 1.0)).abs() < 1e-9,
        "{violation}"
    );
}
