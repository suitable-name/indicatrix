//! Validation-banner text tests: closed/degenerate status, the tierless-design
//! proportions dash-out, the `GpuFacetPlane` sign-flip round trip, and the
//! `TargetResolveError` remedy sentence reaching the status strip.

use crate::{
    EditorSession,
    view_model::{solid_status::*, yield_report::*},
};
use indicatrix_cut_core::{ConstraintTier, Edit, TierTarget};

#[test]
fn status_text_reports_preform_only_for_a_fresh_tierless_design() {
    // A tierless design (a bare, uncut preform) solves trivially -- an
    // empty mast list "closes" with zero volume -- and must read as "nothing
    // cut yet," not `Closed solid` with the bare preform's own L/W, H/W as if
    // they belonged to a finished stone.
    let (text, is_problem) = status_text_and_is_problem(&EditorSession::fresh().design);
    assert_eq!(text, "Preform only -- add tiers.");
    assert!(!is_problem);
}

// `proportions_texts` guards against tierless designs by returning "-" for all
// fields. `girdle_and_ratio_texts` (`state/mod.rs`'s own `inline_tests` module)
// has a matching guard; this test verifies `proportions_texts`'s counterpart.
#[test]
fn proportions_texts_dashes_out_a_design_with_no_tiers() {
    let design = EditorSession::fresh().design;
    assert_eq!(
        proportions_texts(&design),
        (
            "-".to_string(),
            "-".to_string(),
            "-".to_string(),
            "-".to_string(),
            "-".to_string(),
        ),
        "a tierless design must never show the bare preform block's own \
         numbers as if they were the stone's -- including total depth, which \
         (unlike the other four fields) has no `Option`-driven '-' fallback \
         of its own and relied entirely on this guard"
    );
}

#[test]
fn status_text_reports_degenerate_once_pinched_flat() {
    // Two opposing zero-mast facets pinch the preform's vertical extent to nothing.
    let mut state = EditorSession::fresh();
    state
        .apply(Edit::AddTier {
            index: 0,
            tier: ConstraintTier {
                angle_deg: 0.0,
                name: "T".to_string(),
                indices: vec![],
                constraint: indicatrix::geometry::meet_solver::MeetConstraint::ScaleReference(0.0),
                imported_meet: None,
                original_notes: None,
                detached: Vec::new(),
            },
        })
        .unwrap();
    state
        .apply(Edit::AddTier {
            index: 1,
            tier: ConstraintTier {
                angle_deg: -0.0,
                name: "C".to_string(),
                indices: vec![],
                constraint: indicatrix::geometry::meet_solver::MeetConstraint::ScaleReference(0.0),
                imported_meet: None,
                original_notes: None,
                detached: Vec::new(),
            },
        })
        .unwrap();
    let (text, is_problem) = status_text_and_is_problem(&state.design);
    assert!(text.starts_with("Degenerate"), "{text}");
    assert!(is_problem);
}

#[test]
fn design_to_gpu_planes_inverts_the_halfspace_sign_convention() {
    // Converting a fresh design's own preform planes to `GpuFacetPlane` and back via
    // `to_halfspace_f64` must reproduce the same `m` for every plane, proving the
    // `d = -m` flip in `design_to_gpu_planes` is inverted correctly.
    let design = indicatrix_cut_core::Design::fresh(
        indicatrix_cut_core::PreformSpec::cylinder(12, 1.3, 1.0, 0.9),
        12,
        4,
        1.6,
    );
    // A fresh design has no tiers, so `planes()` cannot fail with `MissingAnchor`.
    let original = design
        .planes()
        .expect("a fresh design has no tiers to anchor");
    let converted = design_to_gpu_planes(&design);
    assert_eq!(original.len(), converted.len());
    for (&(n, m), gpu) in original.iter().zip(&converted) {
        let (round_trip_n, round_trip_m) = gpu.to_halfspace_f64();
        assert!(
            (round_trip_n - n).length() < 1e-5,
            "normal did not round-trip: {round_trip_n:?} vs {n:?}"
        );
        assert!(
            (round_trip_m - m).abs() < 1e-5,
            "offset did not round-trip: {round_trip_m} vs {m}"
        );
    }
}

// --- status strip: TargetResolveError reaching the same "problem" sentence ---

#[test]
fn status_text_reports_the_girdle_diameter_remedy_for_an_unresolvable_depth_target() {
    let mut state = EditorSession::fresh();
    state
        .apply(Edit::AddTier {
            index: 0,
            tier: ConstraintTier {
                angle_deg: 90.0,
                name: "G".to_string(),
                indices: vec![],
                constraint: indicatrix::geometry::meet_solver::MeetConstraint::ScaleReference(1.0),
                imported_meet: None,
                original_notes: None,
                detached: Vec::new(),
            },
        })
        .unwrap();
    state
        .apply(Edit::SetTierTarget {
            index: 0,
            target: Some(TierTarget::DepthMm(3.2)),
        })
        .unwrap();
    assert_eq!(
        state.design.girdle_diameter_mm, None,
        "this design must never have had a girdle diameter set, or the target \
         would resolve instead of hitting the error this test checks"
    );

    let (text, is_problem) = status_text_and_is_problem(&state.design);
    assert!(is_problem);
    assert!(
        text.contains("girdle diameter"),
        "expected TargetResolveError::MissingGirdleDiameter's own remedy \
         sentence in the status strip text, got: {text}"
    );
}

#[test]
fn design_to_gpu_geometry_has_empty_tools_for_a_planar_design() {
    let design = indicatrix_cut_core::Design::concave_fixture();
    let mut planar = design.clone();
    planar.concave_tiers.clear();
    let (planes, tools, placements) = design_to_gpu_geometry(&planar);
    assert_eq!(planes.len(), design_to_gpu_planes(&planar).len());
    assert!(tools.is_empty() && placements.is_empty());

    // With the concave tiers the planes are unchanged and one tool per placement joins.
    let (concave_planes, tools, placements) = design_to_gpu_geometry(&design);
    assert_eq!(concave_planes.len(), planes.len());
    assert_eq!(tools.len(), design.concave_placement_count());
    assert_eq!(placements.len(), tools.len());
}

/// The banner quotes the CARVED volume of a design with concave tiers (the cut sheet's and the
/// CLI's), through both entry points; a design without them keeps the flat volume.
#[test]
fn the_banner_volume_of_a_concave_design_is_the_carved_volume() {
    let design = indicatrix_cut_core::Design::concave_fixture();
    let solved = design.solve().expect("the fixture pins every mast");
    let flat =
        indicatrix::geometry::stone_metrics::measure_solid(&design.planes_from_solved(&solved))
            .expect("the flat stone measures");
    let carved = design
        .measure_from_solved_geom(&solved)
        .expect("the tiers resolve")
        .expect("a carved stone remains");
    assert!(carved.volume < flat.volume, "the tools remove material");

    let shows = |text: &str, volume: f64| text.contains(&format!("volume {volume:.4} "));
    let (solving, _) = status_text_and_is_problem(&design);
    let (from_solved, _) = status_text_and_is_problem_from_solved(&design, &solved);
    assert!(shows(&solving, carved.volume), "{solving}");
    assert!(shows(&from_solved, carved.volume), "{from_solved}");
    assert!(!shows(&solving, flat.volume), "{solving}");

    // Without its concave tiers the same design reads the flat volume, as before.
    let mut shell = design;
    shell.concave_tiers.clear();
    shell.concave_tier_ids.clear();
    let (text, _) = status_text_and_is_problem(&shell);
    let (text_from_solved, _) = status_text_and_is_problem_from_solved(&shell, &solved);
    assert!(shows(&text, flat.volume), "{text}");
    assert!(shows(&text_from_solved, flat.volume), "{text_from_solved}");
}
