//! The angle, depth and index drag handles in the Diagram view (view mode 3).
//!
//! The math is `indicatrix_editor::manipulate::diagram2d` (the panel projections, which
//! handles each panel offers, the index handle's rotation about the panel centre); this
//! module is the desktop wiring around it.
//!
//! # Coordinates
//!
//! `solid_viewport.slint` zooms and pans the diagram image on its own and Rust does not
//! know the pan, so the pointer callbacks it makes in Diagram mode carry UNZOOMED logical
//! coordinates (where the point would be at zoom 1, pan 0 -- the space the diagram's pick
//! buffer is indexed in, times the scale factor). Hit tests and drags work in that space,
//! and `show` hands the handle positions to `ManipulateModel` in it too: the overlay maps
//! them back to the zoomed screen when it draws (`ManipulateOverlay`). The one number Rust
//! needs from the zoom is `ManipulateModel.view_zoom`, because a marker is a fixed size on
//! screen: its hit radius is `1 / zoom` as big in the unzoomed space ([`hit_radius`]).
//!
//! # One panel at a time
//!
//! The handles sit on one panel of the selected facet at a time -- the panel the pointer
//! last entered that offers any ([`follow_pointer`]), else the first that does, in the
//! order crown, pavilion, profile. The anchor facet is the last clicked one when it belongs
//! to the selected tier ([`candidate_facets`]); a facet no panel offers a handle for hands
//! over to the next facet of the tier, so a girdle selected from the tier table lands on
//! one of its edge-on facets in the profile.

use super::{SESSION, Shared, Target, handles, tier_label};
use crate::{
    EditorModel, MainWindow, ManipulateModel, SolidPreviewModel,
    gui::{
        editor::callbacks::selected_facet_id,
        solid_preview::{diagram2d::PanelKind, facet_map::FacetMap, preview_state::FrameGeometry},
    },
};
use glam::Vec3;
use indicatrix_cut_core::Design;
use indicatrix_editor::manipulate::{
    HandleAvailability, IndexRotation, candidate_facets, facet_frame, place_handles,
    zoomed_hit_radius,
};
use slint::ComponentHandle as _;
use std::sync::PoisonError;

/// The Diagram view's `SolidPreviewModel.view_mode`.
const DIAGRAM_VIEW_MODE: i32 = 3;

/// What a handle target on a diagram panel adds to a [`Target`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct DiagramPlacement {
    /// The panel the handles sit on.
    pub(super) panel: PanelKind,
    /// Which handles that panel offers for the facet.
    pub(super) available: HandleAvailability,
    /// The index handle's rotation about the panel centre (crown and pavilion panels).
    pub(super) rotation: Option<IndexRotation>,
}

/// Whether the Diagram view is showing.
pub(super) fn is_diagram_view(ui: &MainWindow) -> bool {
    ui.global::<SolidPreviewModel>().get_view_mode() == DIAGRAM_VIEW_MODE
}

/// `radius` (in the frame's pixels, as for the 3D views) for the view's zoom: the markers
/// keep their size on screen, so a zoomed-in diagram grabs them within a smaller radius of
/// its own pixels. Unchanged outside the Diagram view.
pub(super) fn hit_radius(ui: &MainWindow, radius: f32) -> f32 {
    if is_diagram_view(ui) {
        zoomed_hit_radius(radius, ui.global::<ManipulateModel>().get_view_zoom())
    } else {
        radius
    }
}

/// The centroid of `facet_id` in `centroids`, if it has one.
fn centroid_of(centroids: &[Option<Vec3>], facet_id: u32) -> Option<Vec3> {
    centroids.get(facet_id as usize).copied().flatten()
}

/// The handle target on `tier` of `design` for the Diagram frame `geometry` describes,
/// whose facets `map` numbers: the first facet of [`candidate_facets`] that some panel
/// carries handles for, on the `preferred` panel when that one offers them. `None` when the
/// map and the frame hold different facet-id spaces, the frame has no diagram layout, or no
/// facet of the tier offers a handle anywhere.
pub(super) fn target_for(
    geometry: &FrameGeometry,
    map: &FacetMap,
    design: &Design,
    tier: usize,
    remembered: Option<u32>,
    preferred: Option<PanelKind>,
) -> Option<Target> {
    let layout = geometry.diagram.as_deref()?;
    let tier_data = design.tiers.get(tier)?;
    let centroids = geometry.facet_centroids.as_slice();
    if !handles::ids_aligned(map.facet_count(), centroids.len()) {
        return None;
    }
    let candidates = candidate_facets(remembered, tier, map.facets_of_tier(tier), |id| {
        map.tier_of(id as usize)
    });
    candidates.into_iter().find_map(|facet_id| {
        let centroid = centroid_of(centroids, facet_id)?;
        let frame = facet_frame(
            tier_data,
            f64::from(map.index_on_gear(facet_id as usize)),
            layout,
            centroid,
        );
        let placed = place_handles(&frame, layout, preferred)?;
        Some(Target {
            tier,
            frame,
            layout: placed.layout,
            label: tier_label(tier_data, tier),
            provisional: false,
            diagram: Some(DiagramPlacement {
                panel: placed.panel,
                available: placed.available,
                rotation: placed.rotation,
            }),
        })
    })
}

/// The handles for the current selection on the Diagram frame `geometry`, or `None` when
/// they should be hidden: nothing selected, an unsolved or misaligned design, a provisional
/// Slice tier on screen (the Diagram view shows the committed design), or no panel offering
/// a handle for any facet of the tier.
pub(super) fn place(ui: &MainWindow, ctx: &Shared, geometry: &FrameGeometry) -> Option<Target> {
    if super::slice::is_active() {
        return None;
    }
    let tier = usize::try_from(ui.global::<EditorModel>().get_selected_tier_index()).ok()?;
    let st = ctx.state.try_borrow().ok()?;
    let map = handles::facet_map_for(ctx, st.current_generation(), &st.design)?;
    let preferred = SESSION.with(|cell| cell.borrow().diagram_panel);
    target_for(
        geometry,
        &map,
        &st.design,
        tier,
        selected_facet_id(),
        preferred,
    )
}

/// The panel of the Diagram frame `geometry` that contains the unzoomed logical pointer
/// `(x, y)`.
fn panel_under(ui: &MainWindow, geometry: &FrameGeometry, x: f32, y: f32) -> Option<PanelKind> {
    let layout = geometry.diagram.as_deref()?;
    let pointer = handles::pointer_to_pick(ui, x, y);
    layout
        .panel_at(pointer.x, pointer.y)
        .map(|panel| panel.kind)
}

/// The pointer moved with no drag in progress: when it has entered another panel, the
/// handles move there (if that panel offers any for the selected facet).
///
/// Only a CHANGE of panel re-places them, so a pointer moving about inside one panel costs
/// one lookup, not a projection. Returns whether the handles were re-placed.
pub(super) fn follow_pointer(ui: &MainWindow, ctx: &Shared, x: f32, y: f32) -> bool {
    if !is_diagram_view(ui) || ui.global::<ManipulateModel>().get_dragging() {
        return false;
    }
    let geometry = ctx
        .geometry
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    let Some(entered) = geometry
        .as_ref()
        .and_then(|geometry| panel_under(ui, geometry, x, y))
    else {
        return false;
    };
    let changed = SESSION.with(|cell| {
        let mut session = cell.borrow_mut();
        let changed = session.diagram_panel != Some(entered);
        session.diagram_panel = Some(entered);
        changed
    });
    if changed {
        handles::refresh_handles(ui, ctx);
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::solid_preview::{
        diagram2d::{DiagramConfig, DiagramStyle, render_diagram},
        facet_map::FacetMap,
        mesh_cache::MeshCache,
    };
    use indicatrix::geometry::meet_solver::MeetConstraint;
    use indicatrix_cut_core::{ConstraintTier, PreformSpec, ScheduleMeta};
    use indicatrix_editor::manipulate::FacetFrame;
    use std::sync::Arc;

    /// The tier numbers of [`pinned_design`].
    const TABLE: usize = 0;
    const CROWN_MAIN: usize = 2;
    const GIRDLE: usize = 4;
    const PAVILION_MAIN: usize = 5;

    fn rbc_tier(name: &str, angle_deg: f64, indices: &[f64], mast: f64) -> ConstraintTier {
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

    /// A pinned round brilliant, every tier a scale reference (the same closed fixture the
    /// replan tests use): table, star, crown main, upper girdle, girdle, pavilion main,
    /// lower girdle, culet.
    fn pinned_design() -> Design {
        const GIRDLE_INDICES: [f64; 16] = [
            0.0, 6.0, 12.0, 18.0, 24.0, 30.0, 36.0, 42.0, 48.0, 54.0, 60.0, 66.0, 72.0, 78.0, 84.0,
            90.0,
        ];
        const BREAK_INDICES: [f64; 16] = [
            95.0, 1.0, 11.0, 13.0, 23.0, 25.0, 35.0, 37.0, 47.0, 49.0, 59.0, 61.0, 71.0, 73.0,
            83.0, 85.0,
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
                rbc_tier("Table", 0.0, &[], 0.32),
                rbc_tier("Star", 15.0, &STAR, 0.45),
                rbc_tier("Crown Main", 34.5, &MAIN, 0.59),
                rbc_tier("Upper Girdle", 41.0, &BREAK_INDICES, 0.67),
                rbc_tier("Girdle", 90.0, &GIRDLE_INDICES, 1.0),
                rbc_tier("Pavilion Main", -41.0, &MAIN, 0.67),
                rbc_tier("Lower Girdle", -42.5, &BREAK_INDICES, 0.68),
                rbc_tier("Culet", -0.0, &[], 0.88),
            ],
        )
    }

    /// The Diagram frame geometry of `design` at 700 x 360 pixels, and the facet map.
    fn diagram_geometry(design: &Design) -> (FrameGeometry, FacetMap) {
        let solved = design.solve().expect("every tier is pinned");
        let planes = design.planes_from_solved(&solved);
        let halfspaces: Vec<(Vec3, f32)> = planes
            .iter()
            .map(|&(normal, offset)| (normal.as_vec3(), offset as f32))
            .collect();
        let mut cache = MeshCache::default();
        let cached = cache
            .get_or_build(&halfspaces)
            .expect("the pinned round brilliant closes");
        let config = DiagramConfig {
            width: 700,
            height: 360,
            gear_teeth: design.meta.gear_teeth_abs(),
            gear_reference_angle: design.meta.gear_reference_angle as f32,
            symmetry_order: 8,
            mirror: true,
        };
        let frame = render_diagram(&cached.mesh, &config, &DiagramStyle::default());
        let geometry = FrameGeometry {
            corner_points: Arc::clone(&cached.corner_points),
            facet_centroids: Arc::clone(&cached.facet_centroids),
            bounding_radius: cached.bounding_radius(),
            camera: crate::gui::solid_preview::preview_state::CameraPose {
                yaw: 0.0,
                pitch: 0.0,
                distance: 4.0,
            },
            size: (700, 360),
            diagram: Some(Arc::new(frame.layout)),
            visible_tiers: None,
        };
        let map = FacetMap::from_design(design, &solved);
        (geometry, map)
    }

    #[test]
    fn a_crown_main_facet_gets_its_handles_on_the_crown_panel() {
        let design = pinned_design();
        let (geometry, map) = diagram_geometry(&design);
        let target = target_for(&geometry, &map, &design, CROWN_MAIN, None, None)
            .expect("the crown mains offer handles");
        let placement = target.diagram.expect("a diagram target");
        assert_eq!(placement.panel, PanelKind::Crown);
        assert!(placement.available.angle && placement.available.depth);
        assert!(placement.available.index && placement.rotation.is_some());
        assert_eq!(target.tier, CROWN_MAIN);
        assert!(!target.provisional);
    }

    #[test]
    fn a_pavilion_main_facet_gets_them_on_the_pavilion_panel() {
        let design = pinned_design();
        let (geometry, map) = diagram_geometry(&design);
        let target = target_for(&geometry, &map, &design, PAVILION_MAIN, None, None)
            .expect("the pavilion mains offer handles");
        let placement = target.diagram.expect("a diagram target");
        assert_eq!(placement.panel, PanelKind::Pavilion);
        assert!(placement.rotation.is_some());
    }

    #[test]
    fn the_girdle_is_edge_on_in_the_profile_and_has_no_index_wheel_there() {
        let design = pinned_design();
        let (geometry, map) = diagram_geometry(&design);
        let target = target_for(&geometry, &map, &design, GIRDLE, None, None)
            .expect("an edge-on girdle facet offers handles in the profile");
        let placement = target.diagram.expect("a diagram target");
        assert_eq!(placement.panel, PanelKind::Profile);
        assert!(placement.available.angle && placement.available.depth);
        assert!(!placement.available.index && placement.rotation.is_none());
    }

    #[test]
    fn the_table_has_an_angle_handle_but_no_depth_handle_from_above() {
        let design = pinned_design();
        let (geometry, map) = diagram_geometry(&design);
        let target = target_for(&geometry, &map, &design, TABLE, None, None)
            .expect("the table offers an angle handle");
        let placement = target.diagram.expect("a diagram target");
        assert_eq!(placement.panel, PanelKind::Crown);
        assert!(placement.available.angle);
        assert!(
            !placement.available.depth,
            "its normal points at the viewer"
        );
        assert!(
            !placement.available.index,
            "the table has no index positions"
        );
    }

    #[test]
    fn the_remembered_facet_anchors_the_handles_when_it_belongs_to_the_tier() {
        let design = pinned_design();
        let (geometry, map) = diagram_geometry(&design);
        let ids = map.facets_of_tier(CROWN_MAIN).to_vec();
        let remembered = ids[3];
        let target = target_for(&geometry, &map, &design, CROWN_MAIN, Some(remembered), None)
            .expect("handles");
        let centroid = geometry.facet_centroids[remembered as usize].expect("a centroid");
        assert_eq!(target.frame.centroid, centroid);
        // A remembered facet of ANOTHER tier is ignored.
        let other = map.facets_of_tier(PAVILION_MAIN)[0];
        let fallback =
            target_for(&geometry, &map, &design, CROWN_MAIN, Some(other), None).expect("handles");
        let first = geometry.facet_centroids[ids[0] as usize].expect("a centroid");
        assert_eq!(fallback.frame.centroid, first);
    }

    #[test]
    fn the_pointers_panel_wins_when_it_offers_handles() {
        let design = pinned_design();
        let (geometry, map) = diagram_geometry(&design);
        // The table is drawn on the crown only; asking for the pavilion falls back to it.
        let target = target_for(
            &geometry,
            &map,
            &design,
            TABLE,
            None,
            Some(PanelKind::Pavilion),
        )
        .expect("the table offers an angle handle");
        assert_eq!(target.diagram.expect("diagram").panel, PanelKind::Crown);
        // The crown main at index 0 faces along the profile's view plane (its normal has no
        // depth component), so the profile carries it too once the pointer is there.
        let at_zero = map
            .facets_of_tier(CROWN_MAIN)
            .iter()
            .copied()
            .find(|&id| map.index_on_gear(id as usize) == 0)
            .expect("the crown mains include index 0");
        let main = target_for(
            &geometry,
            &map,
            &design,
            CROWN_MAIN,
            Some(at_zero),
            Some(PanelKind::Profile),
        )
        .expect("the crown mains offer handles");
        assert_eq!(main.diagram.expect("diagram").panel, PanelKind::Profile);
    }

    /// The 3D handles follow the same azimuth rule as the Diagram view's: a design with a
    /// gear reference angle has its facets turned by it, and the handles' frame must point
    /// where the facet's own plane does. (They were built from the plain index and sat a few
    /// teeth off.) Lives here for the closed round brilliant `diagram_geometry` builds.
    #[test]
    fn the_3d_handles_sit_on_the_facet_whatever_the_gear_reference_angle() {
        for reference in [0.0, 3.0, 1.5] {
            let mut design = pinned_design();
            design.meta.gear_reference_angle = reference;
            let (geometry, map) = diagram_geometry(&design);
            let solved = design.solve().expect("every tier is pinned");
            let planes = design.planes_from_solved(&solved);
            let mut checked = 0;
            for tier in [CROWN_MAIN, PAVILION_MAIN] {
                for &id in map.facets_of_tier(tier) {
                    // A facet the turned stone no longer shows has no centroid, no handles.
                    let Some(target) =
                        handles::target_for(&geometry, &map, &design, tier, Some(id), false)
                    else {
                        continue;
                    };
                    checked += 1;
                    let expected = planes[id as usize].0.as_vec3();
                    let miss = (target.frame.normal - expected).length();
                    assert!(
                        miss < 1e-4,
                        "reference {reference}, tier {tier}, facet {id}: the frame is off by {miss}"
                    );
                    if reference > 0.0 {
                        let plain = FacetFrame::from_tier(
                            &design.tiers[tier],
                            f64::from(map.index_on_gear(id as usize)),
                            design.meta.gear_teeth_abs(),
                            target.frame.centroid,
                        );
                        let plain_miss = (plain.normal - expected).length();
                        assert!(
                            plain_miss > 0.04,
                            "reference {reference}, facet {id}: the plain frame would be off by only {plain_miss}"
                        );
                    }
                }
            }
            assert!(
                checked >= 12,
                "reference {reference}: only {checked} facets had handles"
            );
        }
    }

    #[test]
    fn a_frame_without_a_diagram_layout_has_no_handles() {
        let design = pinned_design();
        let (mut geometry, map) = diagram_geometry(&design);
        geometry.diagram = None;
        assert!(target_for(&geometry, &map, &design, CROWN_MAIN, None, None).is_none());
    }

    #[test]
    fn a_map_of_another_arrangement_hides_the_handles() {
        let design = pinned_design();
        let (geometry, _) = diagram_geometry(&design);
        // The same design with its last tiers left out has fewer facets than the frame.
        let solved = design.solve().expect("every tier is pinned");
        let first_four: Vec<bool> = (0..design.tiers.len()).map(|tier| tier < 4).collect();
        let shorter = FacetMap::from_design_cut(&design, &solved, &[], Some(&first_four));
        assert_ne!(shorter.facet_count(), geometry.facet_centroids.len());
        assert!(target_for(&geometry, &shorter, &design, CROWN_MAIN, None, None).is_none());
    }
}
