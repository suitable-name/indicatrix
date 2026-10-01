//! The pipeline's request-resolution state machine: its remembered
//! [`WorkerMemory`] (including the Diagram-mode [`DiagramMemory`] half), and
//! [`resolve_request_state`], which turns one incoming [`RedrawRequest`] into the
//! plane/camera/style tuple [`super::render_request`] draws.
//!
//! Moved verbatim from the desktop's `gui::solid_preview::preview_state::state`,
//! which re-exports it.

use super::{
    request::{PlannedFrame, RedrawRequest, panel_kind_from_index},
    types::CameraPose,
};
use crate::{
    diagram2d::{DiagramStyle, PanelKind},
    facet_map::FacetMap,
    mesh_cache::MeshCache,
    raster::SolidStyle,
};
use glam::Vec3;
use indicatrix::geometry::{meet_solver::SolvedTier, stone_metrics::SolidStatus};
use std::sync::{Arc, Mutex, PoisonError};

/// Facet outlines an embedding app owns OUTSIDE the request stream: the Slice tool's
/// provisional tier and the tiers that follow a handle drag.
///
/// They cannot ride on [`RedrawRequest::UpdateFacetOverlay`] alone: every `Planned`
/// request rebuilds the whole [`SolidStyle`] (so it would drop them), and the request
/// gate is "latest wins", so an overlay update superseded by a `Reproject` or a
/// `Planned` request is lost outright. Read at draw time through
/// [`WorkerMemory::outlines`] instead, every request kind shows the newest value.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outlines {
    /// Facet ids of the provisional slice tier (green).
    pub provisional: Vec<u32>,
    /// Facet ids of the tiers that follow a drag (orange).
    pub moved: Vec<u32>,
}

/// The handle an embedding app and the worker share: the app writes, the worker reads
/// at draw time. See [`Outlines`].
pub type SharedOutlines = Arc<Mutex<Outlines>>;

/// The pipeline's memory of the last diagram-relevant data it computed.
///
/// Carried across a [`RedrawRequest::Reproject`] request, which has no `Design`
/// to recompute gear info/labels from, exactly like [`WorkerMemory::style`] is
/// carried forward for the ordinary solid render.
///
/// Defaults to a 96-tooth gear
/// with no reference angle and no labels -- a reasonable "nothing solved into the
/// diagram yet" starting point.
pub struct DiagramMemory {
    /// The index wheel's tooth count.
    pub gear_teeth: u32,
    /// The index wheel's reference angle.
    pub gear_reference_angle: f32,
    /// The schedule's own rotational symmetry order (`ScheduleMeta::
    /// symmetry_order`), carried forward like `gear_teeth` -- see
    /// [`crate::diagram2d::DiagramConfig::symmetry_order`].
    pub symmetry_order: u32,
    /// The schedule's own mirror flag (`ScheduleMeta::mirror`) -- see
    /// [`crate::diagram2d::DiagramConfig::mirror`].
    pub mirror: bool,
    /// The enlarged panel at the last replan: `None` (the default) means the
    /// ordinary three-column [`crate::diagram2d::render_diagram`] layout;
    /// `Some(panel)` means [`crate::diagram2d::render_diagram_single_panel`].
    pub enlarged_panel: Option<PanelKind>,
    /// The diagram's facet-level style and labels.
    pub style: DiagramStyle,
    /// Facet id -> hover tooltip text.
    pub hover_text: Vec<String>,
    /// Facet id -> owning tier index.
    pub facet_tier: Vec<Option<usize>>,
}

impl Default for DiagramMemory {
    fn default() -> Self {
        Self {
            gear_teeth: 96,
            gear_reference_angle: 0.0,
            // Matches `indicatrix_cut_core::ScheduleMeta::standard_round_brilliant`'s
            // own 8-fold mirrored symmetry -- a reasonable "nothing solved into the
            // diagram yet" starting point, same spirit as `gear_teeth: 96` above.
            symmetry_order: 8,
            mirror: true,
            enlarged_panel: None,
            style: DiagramStyle::default(),
            hover_text: Vec::new(),
            facet_tier: Vec::new(),
        }
    }
}

/// Everything the pipeline remembers from one request to the next, bundled into
/// a single value so [`resolve_request_state`]/[`super::render_request`] each take
/// one `&mut` parameter.
///
/// `solved_masts` and `planes` exist for [`RedrawRequest::Reproject`]'s sake: a
/// plain camera drag/zoom/pose-button redraw has no `Design` to replan against,
/// so it must reuse the pipeline's own memory of the last real
/// [`RedrawRequest::Planned`] rather than reporting "nothing solved" or leaking a
/// previous design's leftover style.
#[derive(Default)]
pub struct WorkerMemory {
    /// The last `Planned` request's facet-level style, reused (never mutated in
    /// place) by a `Reproject` request.
    pub style: SolidStyle,
    /// The last `Planned` request's diagram label/hover/tier tables.
    pub diagram: DiagramMemory,
    /// The last non-empty mast list a `Planned` frame produced, chained forward
    /// as a `Reproject` frame's own `solved`.
    pub solved_masts: Option<Vec<SolvedTier>>,
    /// The plane arrangement last rendered (`Planned` or `Reproject` alike),
    /// `None` before the very first request -- used only to detect that a
    /// `Reproject` request's planes actually changed (a different design was just
    /// loaded/solved).
    pub planes: Option<Vec<(Vec3, f32)>>,
    /// The camera last rendered at -- what a [`RedrawRequest::UpdateFacetOverlay`]
    /// re-renders with, since it carries none of its own. `None` only before the
    /// very first request; an overlay update arriving that early is a no-op.
    pub camera: Option<CameraPose>,
    /// The size last rendered at (see [`Self::camera`]).
    pub size: Option<(u32, u32)>,
    /// The view mode last rendered in (see [`Self::camera`]).
    pub view_mode: Option<u8>,
    /// The generation the fields above were last (re)computed for -- `0` before
    /// the very first `Planned` frame. Set from a `Planned` frame's own
    /// `generation`; carried forward UNCHANGED by `Reproject`/`UpdateFacetOverlay`.
    pub generation: u64,
    /// The outlines the embedding app sets out-of-band (see [`Outlines`]), stamped onto
    /// the style of EVERY frame at draw time by [`Self::with_outlines`]. `None` (the
    /// default) leaves the style exactly as the request produced it.
    pub outlines: Option<SharedOutlines>,
}

impl WorkerMemory {
    /// `style` with the shared [`Outlines`] (if any) copied over its `provisional` and
    /// `moved` sets.
    #[must_use]
    pub fn with_outlines(&self, mut style: SolidStyle) -> SolidStyle {
        if let Some(shared) = &self.outlines {
            let outlines = shared.lock().unwrap_or_else(PoisonError::into_inner);
            style.provisional.clone_from(&outlines.provisional);
            style.moved.clone_from(&outlines.moved);
        }
        style
    }
}

/// Dims `style` for a held-over solid that is not this frame's fresh result,
/// using flatter, greyed base shading to read as visibly not current.
/// Keeps every facet-overlay field untouched.
#[must_use]
pub fn dim_style(style: SolidStyle) -> SolidStyle {
    SolidStyle {
        base_color: [120, 122, 128],
        ambient: 0.4,
        diffuse: 0.35,
        ..style
    }
}

/// One `Unbounded` escaping plane's label for the status banner.
///
/// `"Girdle (tier 5)"` when the owning tier is named, else `"tier 5"` (1-based,
/// matching the tier table's own `#` column), or the raw `"plane <n>"` fallback
/// when `Design::tier_for_plane_index` can't place it (a preform plane, or an
/// index past the arrangement).
#[must_use]
pub fn escaping_tier_label(
    design: &indicatrix_cut_core::Design,
    solved: &[SolvedTier],
    plane_index: usize,
) -> String {
    design
        .tier_for_plane_index(solved, plane_index)
        .map_or_else(
            || format!("plane {plane_index}"),
            |tier_index| {
                design.tiers.get(tier_index).map_or_else(
                    || format!("tier {}", tier_index + 1),
                    |tier| {
                        if tier.name.is_empty() {
                            format!("tier {}", tier_index + 1)
                        } else {
                            format!("{} (tier {})", tier.name, tier_index + 1)
                        }
                    },
                )
            },
        )
}

/// Rebuilds `last_diagram`'s facet-label/hover-text/tier tables and style from a
/// freshly (re)solved `design`. `solid_style` is the SAME [`SolidStyle`] the
/// planned frame just finished, so the diagram's flagged/pending/selected overlay
/// is guaranteed to agree with the ordinary Solid view's.
fn update_diagram_memory_from_design(
    last_diagram: &mut DiagramMemory,
    design: &indicatrix_cut_core::Design,
    solved: Option<&[SolvedTier]>,
    solid_style: &SolidStyle,
    n_d: f64,
) {
    let facet_map = FacetMap::from_design(design, solved.unwrap_or(&[]));
    let facet_count = facet_map.facet_count();
    let mut labels = vec![String::new(); facet_count];
    let mut hover_text = vec![String::new(); facet_count];
    let mut facet_tier = vec![None; facet_count];
    let mut facet_index_on_gear = vec![0u32; facet_count];
    for id in 0..facet_count {
        labels[id] = facet_map.facet_label(id);
        hover_text[id] = facet_map.hover_text(id, n_d);
        facet_tier[id] = facet_map.tier_of(id);
        facet_index_on_gear[id] = facet_map.index_on_gear(id);
    }
    last_diagram.style = DiagramStyle {
        flagged: solid_style.flagged.clone(),
        pending: solid_style.pending.clone(),
        selected: solid_style.selected.clone(),
        facet_labels: labels,
        // The index-wheel radial-line pass's facet->tooth lookup, and the
        // meet-point markers -- both need nothing beyond `facet_map`, already
        // built above for the label/hover/tier tables.
        facet_index_on_gear,
        meet_marker_pairs: facet_map.meeting_facet_pairs(design),
        ..DiagramStyle::default()
    };
    last_diagram.hover_text = hover_text;
    last_diagram.facet_tier = facet_tier;
}

/// Applies a [`RedrawRequest::Reproject`] request's optional `gear` override onto
/// `last_diagram`. `None` leaves `last_diagram`'s gear fields exactly as they were.
const fn apply_reproject_gear(last_diagram: &mut DiagramMemory, gear: Option<(u32, f32)>) {
    if let Some((gear_teeth, gear_reference_angle)) = gear {
        last_diagram.gear_teeth = gear_teeth;
        last_diagram.gear_reference_angle = gear_reference_angle;
    }
}

/// The state [`resolve_request_state`] resolves one [`RedrawRequest`] into.
///
/// In order: planes, camera, size, view mode, style, solved masts,
/// stale flag, the ready-to-show unsolvable/unbounded status (always `None` for a
/// [`RedrawRequest::Reproject`], which never plans), and the generation.
pub type RequestState = (
    Vec<(Vec3, f32)>,
    CameraPose,
    (u32, u32),
    u8,
    SolidStyle,
    Option<Vec<SolvedTier>>,
    bool,
    Option<String>,
    u64,
);

/// The request-kind `match` half of [`super::render_request`]. `mesh_cache` is
/// only touched by the `Planned` arm, via the same cache the render step queries
/// afterward, so it's always a cache hit there.
///
/// Returns `None` if a [`RedrawRequest::UpdateFacetOverlay`] arrives before the
/// first `Planned`/`Reproject` request (`memory.size` still `None`).
pub fn resolve_request_state(
    memory: &mut WorkerMemory,
    mesh_cache: &mut MeshCache,
    request: RedrawRequest,
) -> Option<RequestState> {
    match request {
        RedrawRequest::Reproject {
            planes,
            camera,
            size,
            view_mode,
            gear,
        } => {
            apply_reproject_gear(&mut memory.diagram, gear);
            // A `Reproject` also carries every New/Load/explicit-Solve redraw on the
            // desktop, not only a camera drag/zoom -- so when the incoming plane
            // arrangement differs from the pipeline's last-known one, a genuinely
            // different geometry just replaced the old one. Reset the leftover
            // flagged/pending/selected style and diagram labels rather than let
            // them leak onto a design that never produced them. An ordinary
            // orbit/zoom always resubmits the SAME planes, so this never fires
            // mid-drag.
            if memory.planes.as_deref() != Some(planes.as_slice()) {
                memory.style = SolidStyle::default();
                memory.diagram = DiagramMemory::default();
            }
            memory.planes = Some(planes.clone());
            memory.camera = Some(camera);
            memory.size = Some(size);
            memory.view_mode = Some(view_mode);
            Some((
                planes,
                camera,
                size,
                view_mode,
                memory.style.clone(),
                memory.solved_masts.clone(),
                false,
                None,
                memory.generation,
            ))
        }
        // No `Design`, no new camera/planes/size, just an id-keyed style tweak
        // re-rendered at whatever the pipeline last used.
        RedrawRequest::UpdateFacetOverlay(overlay) => {
            // `memory.size` is `None` only before the first `Planned`/`Reproject`
            // frame. Bail out before touching style fields; they're recomputed anyway.
            memory.size?;
            memory.style.hovered = overlay.hovered;
            memory.style.selected_facet = overlay.selected_facet;
            memory.style.multi_selected = overlay.multi_selected.clone();
            // The 3D-only outlines (slice tier, followers): the diagram draws neither.
            memory.style.provisional = overlay.provisional;
            memory.style.moved = overlay.moved;
            memory.diagram.style.hovered = overlay.hovered;
            memory.diagram.style.selected_facet = overlay.selected_facet;
            memory.diagram.style.multi_selected = overlay.multi_selected;
            Some((
                memory.planes.clone().unwrap_or_default(),
                memory.camera.unwrap_or(CameraPose {
                    yaw: 0.0,
                    pitch: 0.0,
                    distance: 5.0,
                }),
                memory.size.unwrap_or((1, 1)),
                memory.view_mode.unwrap_or(0),
                memory.style.clone(),
                memory.solved_masts.clone(),
                false,
                None,
                memory.generation,
            ))
        }
        RedrawRequest::Planned(frame) => Some(resolve_planned_state(memory, mesh_cache, *frame)),
    }
}

/// [`RedrawRequest::Planned`]'s half of [`resolve_request_state`]. Finishes what
/// [`super::build_planned_frame`] could not: whether the arrangement closes, and
/// the resulting `Unbounded` status.
fn resolve_planned_state(
    memory: &mut WorkerMemory,
    mesh_cache: &mut MeshCache,
    frame: PlannedFrame,
) -> RequestState {
    let PlannedFrame {
        design,
        planes,
        style,
        solved,
        stale,
        unsolvable_status,
        camera,
        size,
        view_mode,
        generation,
        n_d,
        enlarged_panel,
    } = frame;
    // This frame's arrangement may not close. Name the tier whose facet escapes.
    // Cheap even though it looks like a second mesh build: guaranteed cache hit.
    let closes = mesh_cache.get_or_build(&planes).is_some();
    let unbounded_status = if unsolvable_status.is_none() && !closes {
        match mesh_cache.status() {
            Some(SolidStatus::Unbounded { escaping }) => {
                let solved_ref = solved.as_deref().unwrap_or(&[]);
                let names = escaping
                    .iter()
                    .map(|&plane_index| escaping_tier_label(&design, solved_ref, plane_index))
                    .collect::<Vec<_>>()
                    .join(", ");
                Some(format!("Unbounded: {names} never close the solid."))
            }
            _ => None,
        }
    } else {
        None
    };
    // Both a missing anchor and a solid that never closes leave the viewport
    // showing geometry that is NOT what the design currently says, so both dim
    // it: the status line says why, and the dimming is what stops a cutter
    // reading a held-over solid as current.
    let holding_over = unsolvable_status.is_some() || unbounded_status.is_some();
    let preview_status = unsolvable_status.or(unbounded_status);
    let style = if holding_over {
        dim_style(style)
    } else {
        style
    };

    memory.style = style.clone();
    if solved.is_some() {
        memory.solved_masts.clone_from(&solved);
    }
    memory.planes = Some(planes.clone());
    memory.camera = Some(camera);
    memory.size = Some(size);
    memory.view_mode = Some(view_mode);
    memory.generation = generation;
    memory.diagram.gear_teeth = design.meta.gear_teeth_abs();
    memory.diagram.gear_reference_angle = design.meta.gear_reference_angle as f32;
    memory.diagram.symmetry_order = design.meta.symmetry_order;
    memory.diagram.mirror = design.meta.mirror;
    memory.diagram.enlarged_panel = panel_kind_from_index(enlarged_panel);
    // Unconditional -- not gated on `view_mode == 3`: an edit made while
    // Solid/Both is on screen must still leave the diagram's label/hover/tier
    // tables fresh, so switching to Diagram mode afterward (a `Reproject`, which
    // has no `Design` to rebuild them from) shows a live, correct diagram
    // immediately.
    update_diagram_memory_from_design(&mut memory.diagram, &design, solved.as_deref(), &style, n_d);
    (
        planes,
        camera,
        size,
        view_mode,
        style,
        solved,
        stale,
        preview_status,
        generation,
    )
}
