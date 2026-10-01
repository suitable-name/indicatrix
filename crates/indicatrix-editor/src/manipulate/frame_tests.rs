//! The handle machinery against a REAL solid-preview frame: the anchor a target places on
//! the pick frame must land on the facet the pick buffer says is there, and the web's
//! pointer <-> pick-frame mapping (a letterboxed `image-fit: contain` view) must agree
//! with the pick lookup a hover or click uses.

use super::{
    HandleKind, ScreenPoint,
    target::{HandleTarget, target_for},
};
use indicatrix::geometry::meet_solver::MeetConstraint;
use indicatrix_cut_core::{ConstraintTier, Design, PreformSpec, ScheduleMeta};
use indicatrix_solid::{
    facet_map::FacetMap,
    live_update::{Clock, DEFAULT_PREVIEW_BUDGET},
    preview::{
        CameraPose, PreviewPipeline,
        view::{ReplanBasis, ReplanInputs, contain_fit, contain_pixel, plan_job},
    },
};
use std::sync::Arc;

/// A clock that never advances: every tier here is pinned, so nothing is timed.
struct ZeroClock;

impl Clock for ZeroClock {
    fn now_ms(&self) -> f64 {
        0.0
    }
}

fn pinned(name: &str, angle_deg: f64, indices: &[f64], mast: f64) -> ConstraintTier {
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

/// A standard round brilliant, every tier pinned (so no solver runs).
fn brilliant() -> Design {
    const GIRDLE: [f64; 16] = [
        0.0, 6.0, 12.0, 18.0, 24.0, 30.0, 36.0, 42.0, 48.0, 54.0, 60.0, 66.0, 72.0, 78.0, 84.0,
        90.0,
    ];
    const BREAK: [f64; 16] = [
        95.0, 1.0, 11.0, 13.0, 23.0, 25.0, 35.0, 37.0, 47.0, 49.0, 59.0, 61.0, 71.0, 73.0, 83.0,
        85.0,
    ];
    const MAIN: [f64; 8] = [0.0, 12.0, 24.0, 36.0, 48.0, 60.0, 72.0, 84.0];
    const STAR: [f64; 8] = [6.0, 18.0, 30.0, 42.0, 54.0, 66.0, 78.0, 90.0];
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
        vec![
            pinned("Table", 0.0, &[], 0.32),
            pinned("Star", 15.0, &STAR, 0.45),
            pinned("Crown Main", 34.5, &MAIN, 0.59),
            pinned("Upper Girdle", 41.0, &BREAK, 0.67),
            pinned("Girdle", 90.0, &GIRDLE, 1.0),
            pinned("Pavilion Main", -41.0, &MAIN, 0.67),
            pinned("Lower Girdle", -42.5, &BREAK, 0.68),
            pinned("Culet", -0.0, &[], 0.88),
        ],
    )
}

const CROWN_MAIN: usize = 2;

/// The frame the web's pipeline draws for `design` at 640 x 480, and what `target_for`
/// needs from it.
struct Shown {
    frame: indicatrix_solid::preview::RenderedFrame,
    map: FacetMap,
}

fn shown(design: &Design) -> Shown {
    let mut pipeline = PreviewPipeline::new();
    let job = plan_job(ReplanInputs {
        design: Arc::new(design.clone()),
        generation: 3,
        basis: ReplanBasis::FullSolve,
        camera: CameraPose {
            yaw: 0.6,
            pitch: 0.45,
            distance: 2.4,
        },
        size: (640, 480),
        selected_tier: Some(CROWN_MAIN),
        custom_materials: &[],
        view_mode: 0,
        show_preform: true,
        enlarged_panel: -1,
        tier_cutoff: -1,
    });
    let frame = pipeline
        .replan(job, DEFAULT_PREVIEW_BUDGET, &ZeroClock)
        .expect("a closed solid draws a frame");
    let masts = frame.solved.clone().expect("pinned tiers are solved");
    let map = FacetMap::from_design(design, &masts);
    Shown { frame, map }
}

fn target_on(shown: &Shown, design: &Design, facet: u32) -> Option<HandleTarget> {
    let geometry = shown.frame.geometry.as_ref()?;
    target_for(geometry, &shown.map, design, CROWN_MAIN, Some(facet), false)
}

#[test]
fn a_handle_anchor_lands_on_the_facet_the_pick_buffer_names() {
    let design = brilliant();
    let shown = shown(&design);
    let facets = shown.map.facets_of_tier(CROWN_MAIN).to_vec();
    assert_eq!(facets.len(), 8, "eight crown mains");
    let mut visible = 0;
    for &facet in &facets {
        let target = target_on(&shown, &design, facet).expect("a crown main has a centroid");
        assert_eq!(target.tier, CROWN_MAIN);
        assert_eq!(target.label, "Crown Main");
        assert!(!target.provisional);
        let anchor = target.layout.anchor;
        let picked = shown.frame.pick.facet_at(
            anchor.x.floor().max(0.0) as u32,
            anchor.y.floor().max(0.0) as u32,
        );
        // The far side of the stone hides some crown mains behind others; the ones the
        // camera sees must have their anchor on their own pixels.
        if picked == Some(facet) {
            visible += 1;
        }
    }
    assert!(
        visible >= 2,
        "only {visible} of 8 anchors landed on their own facet"
    );
}

#[test]
fn a_click_on_a_letterboxed_view_finds_the_facet_under_a_drawn_handle_anchor() {
    let design = brilliant();
    let shown = shown(&design);
    let geometry = shown.frame.geometry.as_ref().expect("geometry");
    assert_eq!(geometry.size, (640, 480));
    // A wide 1100 x 500 logical view: the 4:3 raster gets bars left and right.
    let (view_w, view_h) = (1100.0_f32, 500.0_f32);
    let fit = contain_fit(view_w, view_h, geometry.size.0, geometry.size.1).expect("a view");
    assert!(fit.offset_x > 100.0);
    for &facet in shown.map.facets_of_tier(CROWN_MAIN) {
        let target = target_on(&shown, &design, facet).expect("centroid");
        let anchor = target.layout.anchor;
        // A point within a hair of a pixel edge could round either way.
        let on_an_edge = |v: f32| (v - v.round()).abs() < 0.05;
        if on_an_edge(anchor.x) || on_an_edge(anchor.y) {
            continue;
        }
        let Some(expected) = shown
            .frame
            .pick
            .facet_at(anchor.x.floor() as u32, anchor.y.floor() as u32)
        else {
            continue;
        };
        // Where the overlay draws the anchor; a click exactly there resolves through
        // the same placement the Image is drawn with, to the same pick pixel.
        let (lx, ly) = fit.to_view(anchor.x, anchor.y);
        let (px, py) = fit.to_image(lx, ly);
        let pointer = ScreenPoint::new(px, py);
        assert!(
            pointer.distance(anchor) < 1e-2,
            "anchor {anchor:?} came back as {pointer:?}"
        );
        let pixel = contain_pixel(lx, ly, view_w, view_h, geometry.size.0, geometry.size.1)
            .expect("the anchor is inside the drawn image");
        assert_eq!(shown.frame.pick.facet_at(pixel.0, pixel.1), Some(expected));
    }
}

#[test]
fn every_handle_of_a_real_facet_can_be_grabbed_at_its_drawn_tip() {
    let design = brilliant();
    let shown = shown(&design);
    let fit = contain_fit(1100.0, 500.0, 640, 480).expect("a view");
    let radius = fit.length_to_image(super::HANDLE_HIT_RADIUS_PX);
    let mut checked = 0;
    for &facet in shown.map.facets_of_tier(CROWN_MAIN) {
        let target = target_on(&shown, &design, facet).expect("centroid");
        let kinds = [HandleKind::Angle, HandleKind::Depth, HandleKind::Index];
        for kind in kinds {
            let tip = target.layout.tip(kind);
            let (lx, ly) = fit.to_view(tip.x, tip.y);
            let (px, py) = fit.to_image(lx, ly);
            let hit = target.hit(ScreenPoint::new(px, py), radius);
            // A handle whose tip lies on another's (edge-on to the view) is won by the
            // nearer tip; every handle with a tip of its own is grabbed by it.
            let alone = kinds
                .into_iter()
                .filter(|&other| other != kind)
                .all(|other| target.layout.tip(other).distance(tip) > 1.0);
            if alone {
                assert_eq!(hit, Some(kind), "facet {facet} {kind:?} at ({lx}, {ly})");
                checked += 1;
            }
        }
    }
    assert!(
        checked >= 12,
        "only {checked} handles had a tip of their own"
    );
}
