//! Unit tests for [`super::FacetMap`]'s construction, accessors, overlay flags and
//! meet-pair resolution.

use super::*;
use indicatrix::geometry::meet_solver::{MeetConstraint, SolvedTier};
use indicatrix_cut_core::{ConstraintTier, Design, PreformSpec, ScheduleMeta};
use std::collections::BTreeSet;

/// A synthetic "RBC-445"-style design: the tier table
/// `StandardGemCuts::standard_round_brilliant` hardcodes as raw planes,
/// reauthored as [`ConstraintTier`]s so [`Design::planes_from_solved`] derives
/// the identical arrangement through the tier -> schedule -> plane path this
/// module mirrors. Every tier pinned via `ScaleReference`, so the design solves
/// trivially.
fn standard_round_brilliant_design() -> Design {
    const GIRDLE_INDICES: [f64; 16] = [
        0.0, 6.0, 12.0, 18.0, 24.0, 30.0, 36.0, 42.0, 48.0, 54.0, 60.0, 66.0, 72.0, 78.0, 84.0,
        90.0,
    ];
    const BREAK_INDICES: [f64; 16] = [
        95.0, 1.0, 11.0, 13.0, 23.0, 25.0, 35.0, 37.0, 47.0, 49.0, 59.0, 61.0, 71.0, 73.0, 83.0,
        85.0,
    ];
    const MAIN_INDICES: [f64; 8] = [0.0, 12.0, 24.0, 36.0, 48.0, 60.0, 72.0, 84.0];
    const STAR_INDICES: [f64; 8] = [6.0, 18.0, 30.0, 42.0, 54.0, 66.0, 78.0, 90.0];

    fn tier(name: &str, angle_deg: f64, indices: &[f64], mast: f64) -> ConstraintTier {
        ConstraintTier {
            angle_deg,
            name: name.to_string(),
            indices: indices.to_vec(),
            constraint: MeetConstraint::ScaleReference(mast),
            imported_meet: None,
            original_notes: None,
            detached: Vec::new(),
        }
    }

    let tiers = vec![
        tier("Table", 0.0, &[], 0.32),
        tier("Star", 15.0, &STAR_INDICES, 0.45),
        tier("Crown Main", 34.5, &MAIN_INDICES, 0.59),
        tier("Upper Girdle", 41.0, &BREAK_INDICES, 0.67),
        tier("Girdle", 90.0, &GIRDLE_INDICES, 1.0),
        tier("Pavilion Main", -41.0, &MAIN_INDICES, 0.67),
        tier("Lower Girdle", -42.5, &BREAK_INDICES, 0.68),
        tier("Culet", -0.0, &[], 0.88),
    ];

    Design::new(
        PreformSpec::block(2.0, 1.0, 2.0),
        ScheduleMeta {
            gemcad_version: "GemCad 5.0".to_string(),
            gear_teeth: 96,
            gear_reference_angle: 0.0,
            symmetry_order: 8,
            mirror: true,
            refractive_index: 1.54,
            headers: Vec::new(),
            footnotes: Vec::new(),
        },
        tiers,
    )
}

#[test]
fn plane_count_matches_design_planes_on_the_standard_round_brilliant() {
    let design = standard_round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let planes = design.planes_from_solved(&solved);
    let map = FacetMap::from_design(&design, &solved);

    assert_eq!(map.facets.len(), planes.len());
    assert_eq!(map.preform_plane_count(), design.preform.planes().len());
}

#[test]
fn preform_planes_map_to_no_tier() {
    let design = standard_round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let map = FacetMap::from_design(&design, &solved);

    for facet_id in 0..map.preform_plane_count() {
        assert_eq!(map.tier_of(facet_id), None);
    }
}

#[test]
fn orbit_sizes_match_each_tiers_index_count() {
    let design = standard_round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let map = FacetMap::from_design(&design, &solved);

    for (tier_index, tier) in design.tiers.iter().enumerate() {
        let expected = tier.indices.len().max(1);
        assert_eq!(
            map.facets_of_tier(tier_index).len(),
            expected,
            "tier {tier_index} ({})",
            tier.name
        );
    }
}

#[test]
fn every_mapped_facets_normal_carries_its_tiers_own_elevation_angle() {
    let design = standard_round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let planes = design.planes_from_solved(&solved);
    let map = FacetMap::from_design(&design, &solved);

    for (facet_id, &(normal, _offset)) in planes.iter().enumerate().skip(map.preform_plane_count())
    {
        let tier_index = map
            .tier_of(facet_id)
            .expect("every non-preform facet must map to a tier");
        let tier = &design.tiers[tier_index];
        let theta = tier.angle_deg.abs().to_radians();
        // The tier's elevation alone fixes the normal's y-component magnitude to
        // `cos(theta)`, regardless of azimuth (x/z split `sin(theta)` between them).
        assert!(
            (normal.y.abs() - theta.cos()).abs() < 1e-5,
            "facet {facet_id} (tier {tier_index} {}): normal.y={}, expected +-{}",
            tier.name,
            normal.y,
            theta.cos()
        );
        let horizontal = normal.x.hypot(normal.z);
        assert!(
            (horizontal - theta.sin()).abs() < 1e-5,
            "facet {facet_id} (tier {tier_index} {}): horizontal={horizontal}, expected {}",
            tier.name,
            theta.sin()
        );
    }
}

#[test]
fn overlay_flags_mark_the_pending_and_selected_tiers_and_nothing_else_by_default() {
    let design = standard_round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let map = FacetMap::from_design(&design, &solved);
    let n_d = design.effective_refractive_index();

    // Neither tier is steep enough to window at diamond's critical angle
    // (~24.4 deg), so `flagged` should be empty -- this only pins the
    // pending/selected wiring.
    let flags = map.overlay_flags(&design, n_d, Some(2), &BTreeSet::from([5]));

    for &facet_id in map.facets_of_tier(5) {
        assert!(flags.pending[facet_id as usize]);
    }
    for &facet_id in map.facets_of_tier(2) {
        assert!(flags.selected[facet_id as usize]);
    }
    for &facet_id in map.facets_of_tier(3) {
        assert!(!flags.pending[facet_id as usize]);
        assert!(!flags.selected[facet_id as usize]);
    }
    assert!(
        flags.flagged.iter().all(|&f| !f),
        "diamond RBC must not window"
    );
}

/// A MULTI-tier `pending_tiers` set must mark every one of its members, not
/// just the first -- a batch nudge/offset dirties several tiers at once and
/// every one of them must read as pending in the viewport.
#[test]
fn overlay_flags_marks_every_member_of_a_multi_tier_pending_set() {
    let design = standard_round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let map = FacetMap::from_design(&design, &solved);
    let n_d = design.effective_refractive_index();

    let flags = map.overlay_flags(&design, n_d, None, &BTreeSet::from([2, 5]));

    for &facet_id in map.facets_of_tier(2) {
        assert!(flags.pending[facet_id as usize], "tier 2 must be pending");
    }
    for &facet_id in map.facets_of_tier(5) {
        assert!(flags.pending[facet_id as usize], "tier 5 must be pending");
    }
    for &facet_id in map.facets_of_tier(3) {
        assert!(
            !flags.pending[facet_id as usize],
            "tier 3 must not be pending"
        );
    }
}

#[test]
fn hover_text_omits_the_margin_clause_for_crown_and_girdle_facets() {
    let design = standard_round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let map = FacetMap::from_design(&design, &solved);
    let n_d = design.effective_refractive_index();

    // Tier 1 is "Star" (crown), tier 5 is "Pavilion Main" (pavilion) --
    // see `standard_round_brilliant_design`'s tier list.
    let crown_facet = map.facets_of_tier(1)[0] as usize;
    let pavilion_facet = map.facets_of_tier(5)[0] as usize;

    let crown_text = map.hover_text(crown_facet, n_d);
    let pavilion_text = map.hover_text(pavilion_facet, n_d);
    assert!(
        !crown_text.contains("margin"),
        "a crown facet must not claim a windowing margin: {crown_text}"
    );
    assert!(
        pavilion_text.contains("margin"),
        "a pavilion facet must still report its margin: {pavilion_text}"
    );
}

#[test]
fn index_on_gear_matches_the_tiers_own_index_and_is_zero_for_preform() {
    let design = standard_round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let map = FacetMap::from_design(&design, &solved);

    assert_eq!(map.index_on_gear(0), 0, "a preform plane carries no index");
    // Tier 2 ("Crown Main") lists index 0.0 first; its first surviving facet
    // must report exactly that tooth.
    let first_main_facet = map.facets_of_tier(2)[0] as usize;
    assert_eq!(map.index_on_gear(first_main_facet), 0);
}

#[test]
fn meeting_facet_pairs_cross_products_a_meet_nameds_two_orbits() {
    // A minimal fabricated design -- not solved for real geometry -- purely to
    // exercise `Design::facet_meets`'s tier-name resolution feeding
    // `meeting_facet_pairs`'s cross product. `solved` is fabricated too (a
    // fixed mast per tier): `facet_meets` never reads it, and `FacetMap::
    // from_design` only reads it for the (here, irrelevant) plane offset.
    fn tier(
        name: &str,
        angle_deg: f64,
        indices: &[f64],
        constraint: MeetConstraint,
    ) -> ConstraintTier {
        ConstraintTier {
            angle_deg,
            name: name.to_string(),
            indices: indices.to_vec(),
            constraint,
            imported_meet: None,
            original_notes: None,
            detached: Vec::new(),
        }
    }
    let tiers = vec![
        tier("Table", 0.0, &[], MeetConstraint::ScaleReference(0.3)),
        tier(
            "Star",
            15.0,
            &[6.0, 18.0, 30.0, 42.0],
            MeetConstraint::MeetNamed(vec!["Table".to_string()]),
        ),
    ];
    let design = Design::new(
        PreformSpec::block(2.0, 1.0, 2.0),
        ScheduleMeta {
            gemcad_version: "GemCad 5.0".to_string(),
            gear_teeth: 96,
            gear_reference_angle: 0.0,
            symmetry_order: 8,
            mirror: true,
            refractive_index: 1.54,
            headers: Vec::new(),
            footnotes: Vec::new(),
        },
        tiers,
    );
    let fabricated_solved: Vec<SolvedTier> = design
        .tiers
        .iter()
        .map(|_| SolvedTier {
            mast: 1.0,
            strategy: indicatrix::geometry::meet_solver::SolveStrategy::ScaleReference,
            detail: String::new(),
        })
        .collect();
    let map = FacetMap::from_design(&design, &fabricated_solved);

    let pairs = map.meeting_facet_pairs(&design);
    // Table (orbit size 1) x Star (orbit size 4) = 4 pairs, every one
    // involving Table's single facet id.
    assert_eq!(pairs.len(), 4, "got: {pairs:?}");
    let table_facet = map.facets_of_tier(0)[0];
    for &(a, b) in &pairs {
        assert!(
            a == table_facet || b == table_facet,
            "every pair must involve Table's facet: {a},{b}"
        );
    }
}

#[test]
fn hover_text_reports_a_preform_plane_distinctly_from_a_facet() {
    let design = standard_round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let map = FacetMap::from_design(&design, &solved);
    let n_d = design.effective_refractive_index();

    assert_eq!(map.hover_text(0, n_d), "Preform");
    let facet_text = map.hover_text(map.preform_plane_count(), n_d);
    assert!(facet_text.contains("Table"), "got: {facet_text}");
    assert!(facet_text.contains("index 0"), "got: {facet_text}");
}

// --- Does the 2D diagram apply a tier's cheater offset? ---
//
// `crown_project`/`pavilion_project`/`profile_project` project `SolidMesh`
// vertex positions directly, with no separate azimuth recomputation anywhere
// in the fill/edge passes (the module's own `index_to_azimuth`-based
// `wheel_direction` is used ONLY for the decorative index-wheel ticks, never
// for a facet's own body), so a cheater offset baked into
// `Design::planes_from_solved` (`design/export.rs::apply_cheater_offsets`)
// does reach the picture. This test proves it by actually rendering both a
// plain and a cheatered diagram from the SAME round-brilliant fixture every
// other test in this file uses, and comparing the two `DiagramFrame`s.
#[test]
fn cheater_offset_moves_only_its_own_tiers_facets_in_the_2d_diagram() {
    use super::super::diagram2d::{DiagramConfig, DiagramStyle, render_diagram};
    use indicatrix::geometry::stone_metrics::{SolidStatus, build_solid_mesh};

    let mut design = standard_round_brilliant_design();
    let cheatered_tier = design
        .tiers
        .iter()
        .position(|t| t.name == "Crown Main")
        .expect("fixture must have a Crown Main tier");
    // Tiers whose facets share no plane with Crown Main and are nowhere near
    // it in the schedule -- these must render byte-identically whichever way
    // the cheater offset is set. (Crown Main's own immediate neighbours,
    // Star and Upper Girdle, are deliberately excluded: a real rotation also
    // moves the shared EDGE with an adjacent tier, so asserting pixel
    // equality for them would be asserting something not actually true of
    // the geometry.)
    let unrelated_tier_names = ["Table", "Girdle", "Pavilion Main", "Lower Girdle", "Culet"];

    let solved = design.solve().expect("every tier is pinned");
    let map = FacetMap::from_design(&design, &solved);

    let plain_planes = design.planes_from_solved(&solved);
    design.cheater_offsets_deg.insert(cheatered_tier, 0.5);
    let cheatered_planes = design.planes_from_solved(&solved);
    // Sanity check on the precondition this test actually exercises: the
    // rotation must have landed on the planes at all (core-level behaviour
    // `design/export.rs`'s own tests already cover in depth; asserted here
    // only so a regression there fails loudly in THIS test too, rather than
    // this test silently passing because nothing differed upstream).
    assert_ne!(
        plain_planes, cheatered_planes,
        "the cheater offset must change at least one plane"
    );

    let plain_mesh = match build_solid_mesh(&plain_planes) {
        SolidStatus::Closed(m) => m,
        other => panic!("plain fixture must close: {other:?}"),
    };
    let cheatered_mesh = match build_solid_mesh(&cheatered_planes) {
        SolidStatus::Closed(m) => m,
        other => panic!("cheatered fixture must close: {other:?}"),
    };

    let config = DiagramConfig {
        width: 300,
        height: 300,
        gear_teeth: design.meta.gear_teeth_abs(),
        gear_reference_angle: design.meta.gear_reference_angle as f32,
        symmetry_order: design.meta.symmetry_order,
        mirror: design.meta.mirror,
    };
    let style = DiagramStyle::default();
    let plain_frame = render_diagram(&plain_mesh, &config, &style);
    let cheatered_frame = render_diagram(&cheatered_mesh, &config, &style);

    let unrelated_facet_ids: std::collections::BTreeSet<usize> = (0..map.facets.len())
        .filter(|&id| {
            map.tier_of(id)
                .is_some_and(|t| unrelated_tier_names.contains(&design.tiers[t].name.as_str()))
        })
        .collect();
    let cheatered_facet_ids: std::collections::BTreeSet<usize> = (0..map.facets.len())
        .filter(|&id| map.tier_of(id) == Some(cheatered_tier))
        .collect();
    assert!(!unrelated_facet_ids.is_empty());
    assert!(!cheatered_facet_ids.is_empty());

    let mut unrelated_tier_pixel_differs = false;
    let mut cheatered_tier_pixel_differs = false;
    for y in 0..config.height {
        for x in 0..config.width {
            let plain_pick = plain_frame.pick_at(x, y).map(|id| id as usize);
            let cheatered_pick = cheatered_frame.pick_at(x, y).map(|id| id as usize);
            if plain_pick == cheatered_pick {
                continue;
            }
            // A differing pixel is attributed to whichever facet was
            // painted there in EITHER render (a facet whose edge shrank
            // shows up as the OTHER frame's facet id instead, and vice
            // versa).
            for id in [plain_pick, cheatered_pick].into_iter().flatten() {
                if unrelated_facet_ids.contains(&id) {
                    unrelated_tier_pixel_differs = true;
                }
                if cheatered_facet_ids.contains(&id) {
                    cheatered_tier_pixel_differs = true;
                }
            }
        }
    }

    assert!(
        cheatered_tier_pixel_differs,
        "the cheatered tier's own facet(s) must visibly move in the diagram -- \
         if this fails, diagram2d.rs is not reading the already-rotated planes \
         `Design::planes_from_solved` produces"
    );
    assert!(
        !unrelated_tier_pixel_differs,
        "a cheater offset on Crown Main must not move any pixel belonging to a \
         tier that shares no plane with it"
    );
}

/// A design of pinned tiers on the 96-tooth wheel, with no reference angle.
fn pinned_design(tiers: Vec<(f64, Vec<f64>, f64)>) -> Design {
    let tiers = tiers
        .into_iter()
        .enumerate()
        .map(|(i, (angle_deg, indices, mast))| ConstraintTier {
            angle_deg,
            name: format!("T{i}"),
            indices,
            constraint: MeetConstraint::ScaleReference(mast),
            imported_meet: None,
            original_notes: None,
            detached: Vec::new(),
        })
        .collect();
    Design::new(
        PreformSpec::block(2.0, 1.0, 2.0),
        ScheduleMeta {
            gemcad_version: "GemCad 5.0".to_string(),
            gear_teeth: 96,
            gear_reference_angle: 0.0,
            symmetry_order: 1,
            mirror: false,
            refractive_index: 1.54,
            headers: Vec::new(),
            footnotes: Vec::new(),
        },
        tiers,
    )
}

/// Shallow tiers put adjacent indices only a fraction of a degree apart in
/// normal space; each is still its own facet, in the map exactly as in the
/// plane list it indexes.
#[test]
fn a_shallow_tier_keeps_all_96_facets() {
    let indices: Vec<f64> = (0..96).map(f64::from).collect();
    for angle in [1.0, 2.0, 3.5] {
        let design = pinned_design(vec![(angle, indices.clone(), 0.5)]);
        let solved = design.solve().expect("the tier is pinned");
        let map = FacetMap::from_design(&design, &solved);

        assert_eq!(
            map.facets_of_tier(0).len(),
            96,
            "angle {angle}: every index must keep its own entry"
        );
        assert_eq!(
            map.facet_count(),
            design.planes_from_solved(&solved).len(),
            "angle {angle}: the map must cover exactly the plane list"
        );
    }
}

/// A tier repeated verbatim is the same half-space twice: the first copy keeps
/// the entry and the second contributes none.
#[test]
fn a_duplicated_tier_yields_one_entry() {
    let design = pinned_design(vec![(41.0, vec![12.0], 0.67), (41.0, vec![12.0], 0.67)]);
    let solved = design.solve().expect("both tiers are pinned");
    let map = FacetMap::from_design(&design, &solved);

    assert_eq!(map.facets_of_tier(0).len(), 1);
    assert_eq!(map.facets_of_tier(1), &[] as &[u32]);
    assert_eq!(
        map.facet_count() - map.preform_plane_count(),
        1,
        "only the first copy may add a facet"
    );
}

/// An index listed twice inside one tier collapses to a single entry.
#[test]
fn an_index_listed_twice_in_a_tier_yields_one_entry() {
    let design = pinned_design(vec![(41.0, vec![12.0, 12.0], 0.67)]);
    let solved = design.solve().expect("the tier is pinned");
    let map = FacetMap::from_design(&design, &solved);

    assert_eq!(map.facets_of_tier(0).len(), 1);
}
