//! The RENDER worker's own request-resolution state machine: its remembered
//! [`WorkerMemory`] (including the Diagram-mode [`DiagramMemory`] half), and
//! [`resolve_request_state`]/`resolve_planned_state`, which turn one incoming
//! `super::request::RedrawRequest` into the plane/camera/style tuple
//! `super::render::render_request` actually draws.

use super::{
    DiagramStyle, MeshCache, PanelKind, SolidStyle,
    request::{PlannedFrame, RedrawRequest, panel_kind_from_index},
    types::CameraPose,
};
use glam::Vec3;
use indicatrix::geometry::{meet_solver::SolvedTier, stone_metrics::SolidStatus};

/// The worker thread's memory of the last diagram-relevant data it computed --
/// carried across a `RedrawRequest::Reproject` request as gear info/labels
/// have no `Design` to be recomputed from on that path, exactly like `last_style`
/// is already carried forward for the ordinary solid render. Defaults to a
/// 96-tooth gear with no reference angle and no labels -- a reasonable "nothing
/// solved into the diagram yet" starting point.
pub struct DiagramMemory {
    pub gear_teeth: u32,
    pub gear_reference_angle: f32,
    /// The schedule's own rotational symmetry order (`ScheduleMeta::
    /// symmetry_order`), carried forward exactly like `gear_teeth` above --
    /// see `diagram2d::DiagramConfig::symmetry_order`'s own doc comment for what
    /// it draws.
    pub symmetry_order: u32,
    /// The schedule's own mirror flag (`ScheduleMeta::mirror`), carried
    /// forward exactly like `gear_teeth` above -- see `diagram2d::
    /// DiagramConfig::mirror`'s own doc comment for what it draws.
    pub mirror: bool,
    /// `SolidPreviewModel.diagram_enlarged_panel`'s value at the last `Replan`
    /// (the "enlarge this panel" mode), carried forward exactly like
    /// `gear_teeth` above -- `None` (the default) means the ordinary
    /// three-column `diagram2d::render_diagram` layout;
    /// `Some(panel)` means `super::render::build_diagram_outputs` instead calls
    /// `diagram2d::render_diagram_single_panel` for just that one panel.
    pub enlarged_panel: Option<PanelKind>,
    pub style: DiagramStyle,
    pub hover_text: Vec<String>,
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

/// Everything the worker thread remembers from one request to the next, bundled
/// into a single value so [`resolve_request_state`]/`super::render::render_request`
/// each take one `&mut` parameter instead of growing past clippy's argument-count
/// lint every time a `Reproject` request needs one more piece of carried-forward
/// state.
///
/// `solved_masts` and `planes` exist for `RedrawRequest::Reproject`'s sake: a
/// plain camera drag/zoom/pose-button redraw has no `Design` to replan
/// against, so it must reuse the WORKER's own memory of the last real
/// `RedrawRequest::Planned` rather than reporting "nothing solved" or leaking a
/// previous design's leftover style.
#[derive(Default)]
pub struct WorkerMemory {
    /// The last `RedrawRequest::Planned`'s facet-level style, reused (never
    /// mutated in place) by a `Reproject` request.
    pub style: SolidStyle,
    /// The last `RedrawRequest::Planned`'s diagram label/hover/tier tables.
    pub diagram: DiagramMemory,
    /// The last non-empty mast list a `Planned` frame produced, chained forward
    /// as a `Reproject` frame's own `solved` -- see `super::render::render_request`'s
    /// doc comment.
    pub solved_masts: Option<Vec<SolvedTier>>,
    /// The plane arrangement the worker last rendered (`Planned` or `Reproject`
    /// alike), `None` before the very first request -- used only to detect that a
    /// `Reproject` request's planes actually changed (a different design was just
    /// loaded/solved), never compared for anything else.
    pub planes: Option<Vec<(Vec3, f32)>>,
    /// The camera/size/`view_mode` the worker last rendered at (`Planned` or
    /// `Reproject` alike) -- what a `RedrawRequest::UpdateFacetOverlay` request
    /// re-renders with, since it carries none of its own (see that variant's doc
    /// comment). `None` only before the very first request; an overlay update
    /// arriving that early has nothing to redraw and is a no-op.
    pub camera: Option<CameraPose>,
    pub size: Option<(u32, u32)>,
    pub view_mode: Option<u8>,
    /// The generation the fields above were last (re)computed for -- `0`
    /// before the very first `Planned` frame (matching `super::sink::
    /// PreviewFrame::generation`'s own "nothing solved yet" default). Set from a
    /// `Planned` frame's own `generation`; carried forward UNCHANGED by
    /// `Reproject`/`UpdateFacetOverlay`, exactly like `solved_masts`/`planes`
    /// above -- neither carries a new generation of its own, so the frame they
    /// produce still names whichever design the LAST real replan solved.
    pub generation: u64,
}

/// Dims `style` for a held-over solid that is not this frame's fresh result,
/// using flatter, greyed base shading to read as visibly not current.
/// Keeps every facet-overlay field untouched.
pub fn dim_style(style: SolidStyle) -> SolidStyle {
    SolidStyle {
        base_color: [120, 122, 128],
        ambient: 0.4,
        diffuse: 0.35,
        ..style
    }
}

/// One `Unbounded` escaping plane's label for [`resolve_planned_state`]'s banner --
/// `"Girdle (tier 5)"` when the owning tier is named, else `"tier 5"` (1-based,
/// matching the tier table's own `#` column), or the raw `"plane <n>"` fallback
/// when `Design::tier_for_plane_index` can't place it (a preform plane, or an
/// index past the arrangement -- see that method's own doc comment for both).
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
/// freshly (re)solved `design` -- split out of [`resolve_planned_state`] purely to
/// keep that function short. `solid_style` is the SAME [`SolidStyle`]
/// [`resolve_planned_state`] just finished, so the diagram's flagged/pending/selected
/// overlay is guaranteed to agree with the ordinary Solid view's.
fn update_diagram_memory_from_design(
    last_diagram: &mut DiagramMemory,
    design: &indicatrix_cut_core::Design,
    solved: Option<&[SolvedTier]>,
    solid_style: &SolidStyle,
    n_d: f64,
) {
    let facet_map = super::FacetMap::from_design(design, solved.unwrap_or(&[]));
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

/// Applies a `RedrawRequest::Reproject` request's optional `gear` override onto
/// `last_diagram` -- split out of [`resolve_request_state`] purely to keep that function
/// under clippy's function-length lint. `None` (the `request_redraw` compatibility
/// wrapper's only option -- see its own doc comment) leaves `last_diagram`'s gear
/// fields exactly as they were.
const fn apply_reproject_gear(last_diagram: &mut DiagramMemory, gear: Option<(u32, f32)>) {
    if let Some((gear_teeth, gear_reference_angle)) = gear {
        last_diagram.gear_teeth = gear_teeth;
        last_diagram.gear_reference_angle = gear_reference_angle;
    }
}

/// The plan/style/camera/solve state [`resolve_request_state`] resolves one
/// `RedrawRequest` into, for `super::render::render_request` to actually draw. The
/// last field is the ready-to-show unsolvable-status message, always `None` for a
/// `RedrawRequest::Reproject` request (it never plans, so it can never be
/// unsolvable).
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

/// The request-kind `match` half of `super::render::render_request` -- split out
/// purely to keep `mesh_cache` is only touched by the `Planned` arm via
/// [`resolve_planned_state`]. The same cache `super::render::render_request`
/// queries afterward, so it's always a cache hit.
///
/// Returns `None` if an `RedrawRequest::UpdateFacetOverlay` arrives before the
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
            // A `Reproject` also carries every New/Load/explicit-Solve redraw (see
            // `gui::editor::view::refresh_viewport`), not only a camera drag/zoom --
            // so when the incoming plane arrangement differs from the worker's own
            // last-known one, a genuinely different geometry just replaced the old
            // one. Reset the leftover flagged/pending/selected style and diagram
            // labels rather than let them leak onto a design that never produced
            // them: loading design B after editing A must not show B with some of
            // A's facets still tinted. An ordinary orbit/zoom always resubmits the
            // SAME planes, so this never fires mid-drag.
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
        // See `super::types::FacetOverlay`'s and this variant's own doc comments --
        // no `Design`, no new camera/planes/size, just an id-keyed style tweak
        // re-rendered at whatever the worker last used.
        RedrawRequest::UpdateFacetOverlay(overlay) => {
            // `memory.size` is `None` only before the first `Planned`/`Reproject`
            // frame. Bail out before touching style fields; they're recomputed anyway.
            memory.size?;
            memory.style.hovered = overlay.hovered;
            memory.style.selected_facet = overlay.selected_facet;
            memory.style.multi_selected = overlay.multi_selected.clone();
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

/// `RedrawRequest::Planned`'s half of [`resolve_request_state`], run on the
/// RENDER worker. Finishes what `super::plan_worker::build_planned_frame` could
/// not: whether the arrangement closes, and the resulting `Unbounded` status.
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
    // immediately instead of empty/no-op hover and click until the next
    // Diagram-active edit.
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
