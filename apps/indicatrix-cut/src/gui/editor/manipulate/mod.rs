//! Mouse-driven direct manipulation of the Solid viewport's selected facet: the angle,
//! depth and index drag handles.
//!
//! The math (projection, handle placement, hit-testing, snapping, wording) lives in
//! `indicatrix_editor::manipulate` and is shared with the web app; this module is the
//! desktop wiring around it.
//!
//! # How a drag reaches the design
//!
//! `solid_viewport.slint` routes the pointer to [`ManipulateModel`]'s callbacks:
//!
//! 1. `handle_hit_test` / `handle_hover` ([`handles`]): map the logical pointer to the
//!    raster's pick frame, `manipulate::hit_test` it against the handle layout of the
//!    last frame, light the handle and word the hint.
//! 2. `drag_begin` ([`drag`]): snapshots a [`DragStart`](indicatrix_editor::manipulate::DragStart)
//!    (start angle from the design, start mast from `solid_last_solved`, the layout) and
//!    ends any coalescing run so the gesture is its own undo step.
//! 3. `drag_move`: `manipulate::drag_value` -> `EditIntent::DragTier` posted into an
//!    [`EditIntentQueue`](super::edit_intent::EditIntentQueue), so a burst of pointer moves
//!    costs ONE apply/replan per 16 ms.
//! 4. The queue's drain applies the value through `EditorState::{set_tier_angle,
//!    pin_tier_mast, rotate_tier_indices}` (coalescing edits on one gesture clock),
//!    refreshes the panel and submits a replan; the live re-solve lands as a normal
//!    solid-preview frame.
//! 5. `drag_end` flushes the newest pointer value, ends the coalescing run and toasts;
//!    `drag_cancel` undoes the gesture's one step.
//!
//! # Slice mode
//!
//! [`slice`] adds the "Slice" tool on top of the same session: a mouse line becomes a
//! PROVISIONAL tier (never in `EditorState`, never in the undo history) that is rendered
//! by submitting a replan of the committed design plus that tier under the reserved
//! generation [`PROVISIONAL_GENERATION`]. The sink keeps such frames out of
//! `solid_last_solved`, the tier table and the path tracer's planes
//! ([`frame_updates_mast_cache`]); the three handles attach to the provisional facet and
//! a drag on it edits the session's design clone in place, with no history entry and no
//! toast. Keep is the one `Edit::AddTier` (one undo step); Discard drops the session.
//! [`on_frame_landed`] receives every frame's generation: a frame of the committed design
//! landing over a live session makes the session re-render itself, and a design that
//! moved on discards it (see `slice`'s "Keeping the provisional picture on screen").
//!
//! # Staying cheap
//!
//! [`on_frame_landed`] runs after EVERY solid-preview frame (an orbit produces dozens a
//! second). It reads a few `Mutex`es, reuses one cached
//! [`FacetMap`](crate::gui::solid_preview::facet_map::FacetMap) until the design or the
//! solved masts change, and projects seven points; nothing else. While a drag is active
//! the handles are left exactly where the drag started (the pixel-to-value mapping is
//! fixed for the gesture) and only the "follows your drag" feedback is refreshed.

mod drag;
mod handles;
mod slice;
#[cfg(test)]
mod tests;

use crate::{
    MainWindow, ManipulateModel,
    bridge::render_thread::RenderContext,
    gui::{
        editor::{
            edit_intent::{EditIntent, EditIntentQueue},
            state::EditorState,
            view::SolidLastSolved,
        },
        solid_preview::{
            facet_map::FacetMap,
            preview_state::{FrameGeometry, SolidPickState, SolidPreviewState},
        },
    },
};
use indicatrix::geometry::meet_solver::SolvedTier;
use indicatrix_cut_core::ConstraintTier;
use indicatrix_editor::manipulate::{FacetFrame, HandleKind, HandleLayout, SnapMode};
use slint::ComponentHandle as _;
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex},
};

/// The generation a solid-preview frame of the PROVISIONAL slice design is stamped with.
///
/// Above every real `EditorState` generation (a session would need 2^63 edits to get
/// there) and above every `generation_floor` the plan worker holds, but never
/// `u64::MAX` itself. A frame carrying it describes a design that is not the
/// committed one, so `gui::solid_sink` keeps it out of `solid_last_solved`, out of the
/// tier table (`apply_matching_preview_frame`) and out of the path tracer's plane
/// slot; see [`frame_updates_mast_cache`].
pub(super) const PROVISIONAL_GENERATION: u64 = u64::MAX - 1;

/// Whether a frame stamped `generation` describes the committed design, and so may
/// update `solid_last_solved`, the tier table, the mast cache and the path tracer's
/// planes. `false` only for [`PROVISIONAL_GENERATION`].
pub(super) const fn frame_updates_mast_cache(generation: u64) -> bool {
    generation != PROVISIONAL_GENERATION
}

/// Whether a provisional slice exists right now (facet clicks select nothing then).
pub(super) fn provisional_active() -> bool {
    slice::is_active()
}

/// The solved masts of a provisional-generation frame, handed over by the sink (which
/// never writes them into `solid_last_solved`). Ignored without a session.
pub(super) fn note_provisional_masts(masts: Vec<SolvedTier>) {
    slice::note_masts(masts);
}

/// A replan of the COMMITTED design was just submitted: it replaced whatever provisional
/// replan was still queued (the plan gate keeps only the newest job), so the Slice tool
/// stops waiting for that frame. See `slice::landed_action`.
pub(super) fn note_committed_replan_submitted() {
    slice::clear_awaiting();
}

/// The planes of a provisional-generation frame, handed over by the sink so redraws that
/// reproject the committed planes keep showing the provisional facet. Ignored without a
/// session or for planes that do not match it.
pub(super) fn note_provisional_planes(planes: &[(glam::Vec3, f32)]) {
    slice::note_planes(planes);
}

/// A tier-list selection change is about to replan the committed design: the
/// provisional slice (if any) cannot outlive it.
pub(super) fn drop_provisional_for_selection(ui: &MainWindow) {
    slice::drop_for_selection(ui);
}

/// The shared handles every manipulation callback needs, cloned into each closure and
/// stored once in [`CTX`] for [`on_frame_landed`] (which only receives the window).
#[derive(Clone)]
struct Shared {
    state: Rc<RefCell<EditorState>>,
    render_ctx: Arc<Mutex<RenderContext>>,
    preview_state: Arc<SolidPreviewState>,
    solid_last_solved: SolidLastSolved,
    geometry: Arc<Mutex<Option<FrameGeometry>>>,
}

/// What the handles are currently drawn on: the selected tier, the facet whose
/// centroid anchors them, and their layout in the pick frame of the frame they were
/// computed from.
#[derive(Clone)]
struct Target {
    tier: usize,
    frame: FacetFrame,
    layout: HandleLayout,
    label: String,
    /// Whether `tier` is the provisional slice tier (an index one past the committed
    /// design's last tier) rather than a tier of `EditorState`.
    provisional: bool,
}

/// The [`FacetMap`] of one (design generation, solved-masts generation) pair, kept so
/// a stream of camera frames does not rebuild it.
struct CachedFacetMap {
    design_generation: u64,
    solved_generation: u64,
    map: Rc<FacetMap>,
}

/// The UI-thread-confined state of the manipulation tools.
struct HandleSession {
    /// Where the handles are drawn now; `None` while they are hidden.
    target: Option<Target>,
    facet_map: Option<CachedFacetMap>,
    /// The handle under the pointer (no drag in progress).
    hovered: Option<HandleKind>,
    /// Whether `ManipulateModel.hint_text` currently holds OUR hover hint (so hiding
    /// the handles clears it without wiping a hint another tool wrote).
    hover_hint_active: bool,
    /// The gesture in progress, if any.
    drag: Option<drag::ActiveDrag>,
    /// The Slice tool's line gesture and provisional tier.
    slice: slice::SliceState,
}

impl HandleSession {
    const fn new() -> Self {
        Self {
            target: None,
            facet_map: None,
            hovered: None,
            hover_hint_active: false,
            drag: None,
            slice: slice::SliceState::new(),
        }
    }
}

thread_local! {
    /// The session, UI-thread-only like `facet_overlay`'s `FACET_OVERLAY`. Never
    /// borrowed across a call that can re-enter (a drag is taken out of it while it
    /// is being worked on and put back afterwards).
    static SESSION: RefCell<HandleSession> = const { RefCell::new(HandleSession::new()) };
    /// The handles [`setup_manipulate_callbacks`] captured, for [`on_frame_landed`].
    static CTX: RefCell<Option<Shared>> = const { RefCell::new(None) };
}

/// The handle `kind` int `ManipulateModel` uses (`0` angle, `1` depth, `2` index).
const fn kind_from_int(kind: i32) -> Option<HandleKind> {
    match kind {
        0 => Some(HandleKind::Angle),
        1 => Some(HandleKind::Depth),
        2 => Some(HandleKind::Index),
        _ => None,
    }
}

/// The inverse of [`kind_from_int`].
const fn kind_to_int(kind: HandleKind) -> i32 {
    match kind {
        HandleKind::Angle => 0,
        HandleKind::Depth => 1,
        HandleKind::Index => 2,
    }
}

/// How finely a drag snaps: the Snap pill turns snapping off outright, otherwise Shift
/// selects the fine step.
const fn snap_mode(shift: bool, snap_off: bool) -> SnapMode {
    if snap_off {
        SnapMode::Off
    } else if shift {
        SnapMode::Fine
    } else {
        SnapMode::Coarse
    }
}

/// A tier's name for hints and toasts: its own name, else `"tier N"` (1-based, like the
/// tier table's `#` column).
fn tier_label(tier: &ConstraintTier, index: usize) -> String {
    if tier.name.is_empty() {
        format!("tier {}", index + 1)
    } else {
        tier.name.clone()
    }
}

/// Writes `text` into the hint line under the toolbar.
fn set_hint(ui: &MainWindow, text: &str) {
    ui.global::<ManipulateModel>().set_hint_text(text.into());
}

/// Wires every `ManipulateModel` callback and stores the shared handles for
/// [`on_frame_landed`]. Called once from `setup_editor_tertiary_callbacks`.
pub(super) fn setup_manipulate_callbacks(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
    solid_pick_state: &SolidPickState,
) {
    let ctx = Shared {
        state: Rc::clone(state),
        render_ctx: Arc::clone(render_ctx),
        preview_state: Arc::clone(preview_state),
        solid_last_solved: Arc::clone(solid_last_solved),
        geometry: Arc::clone(&solid_pick_state.geometry),
    };
    CTX.with(|cell| *cell.borrow_mut() = Some(ctx.clone()));
    wire_pointer_callbacks(ui, &ctx);
    wire_drag_callbacks(ui, &ctx);
    wire_slice_callbacks(ui, &ctx);
}

/// The Slice tool: mode toggle, the line gesture, and the provisional tier's buttons.
fn wire_slice_callbacks(ui: &MainWindow, ctx: &Shared) {
    let model = ui.global::<ManipulateModel>();
    let ui_weak = ui.as_weak();
    let toggle_ctx = ctx.clone();
    model.on_slice_toggled(move || {
        if let Some(ui) = ui_weak.upgrade() {
            slice::toggled(&ui, &toggle_ctx);
        }
    });
    let ui_weak = ui.as_weak();
    model.on_slice_begin(move |x: f32, y: f32| {
        if let Some(ui) = ui_weak.upgrade() {
            slice::begin(&ui, x, y);
        }
    });
    let ui_weak = ui.as_weak();
    model.on_slice_move(move |x: f32, y: f32| {
        if let Some(ui) = ui_weak.upgrade() {
            slice::move_to(&ui, x, y);
        }
    });
    let ui_weak = ui.as_weak();
    let end_ctx = ctx.clone();
    model.on_slice_end(move |x: f32, y: f32| {
        if let Some(ui) = ui_weak.upgrade() {
            slice::end(&ui, &end_ctx, x, y);
        }
    });
    let ui_weak = ui.as_weak();
    model.on_slice_cancel(move || {
        if let Some(ui) = ui_weak.upgrade() {
            slice::cancel_gesture(&ui);
        }
    });
    wire_provisional_buttons(ui, ctx);
}

/// Flip, Keep, Discard and the Symmetric toggle of the provisional tier.
fn wire_provisional_buttons(ui: &MainWindow, ctx: &Shared) {
    let model = ui.global::<ManipulateModel>();
    let ui_weak = ui.as_weak();
    let flip_ctx = ctx.clone();
    model.on_slice_flip(move || {
        if let Some(ui) = ui_weak.upgrade() {
            slice::flip(&ui, &flip_ctx);
        }
    });
    let ui_weak = ui.as_weak();
    let keep_ctx = ctx.clone();
    model.on_slice_keep(move || {
        if let Some(ui) = ui_weak.upgrade() {
            slice::keep(&ui, &keep_ctx);
        }
    });
    let ui_weak = ui.as_weak();
    let discard_ctx = ctx.clone();
    model.on_slice_discard(move || {
        if let Some(ui) = ui_weak.upgrade() {
            slice::discard(&ui, &discard_ctx, slice::DiscardReason::User);
        }
    });
    let ui_weak = ui.as_weak();
    let symmetric_ctx = ctx.clone();
    model.on_slice_symmetric_toggled(move || {
        if let Some(ui) = ui_weak.upgrade() {
            slice::symmetric_toggled(&ui, &symmetric_ctx);
        }
    });
}

/// Hit-testing, hover and the Snap pill.
fn wire_pointer_callbacks(ui: &MainWindow, ctx: &Shared) {
    let model = ui.global::<ManipulateModel>();
    let ui_weak = ui.as_weak();
    model.on_handle_hit_test(move |x: f32, y: f32| {
        ui_weak
            .upgrade()
            .map_or(-1, |ui| handles::hit_test_int(&ui, x, y))
    });
    let ui_weak = ui.as_weak();
    let hover_ctx = ctx.clone();
    model.on_handle_hover(move |x: f32, y: f32| {
        if let Some(ui) = ui_weak.upgrade() {
            handles::hover(&ui, &hover_ctx, x, y);
        }
    });
    let ui_weak = ui.as_weak();
    let snap_ctx = ctx.clone();
    model.on_snap_toggled(move || {
        if let Some(ui) = ui_weak.upgrade() {
            handles::refresh_hover_hint(&ui, &snap_ctx);
        }
    });
}

/// The drag gesture: begin, move (through the 16 ms intent queue), end, cancel.
fn wire_drag_callbacks(ui: &MainWindow, ctx: &Shared) {
    let model = ui.global::<ManipulateModel>();
    let queue = {
        let ctx = ctx.clone();
        let ui_weak = ui.as_weak();
        EditIntentQueue::new(move |intent: EditIntent| {
            if let Some(ui) = ui_weak.upgrade() {
                drag::apply_intent(&ui, &ctx, &intent);
            }
        })
    };
    let ui_weak = ui.as_weak();
    let begin_ctx = ctx.clone();
    model.on_drag_begin(move |kind: i32, x: f32, y: f32| {
        if let Some(ui) = ui_weak.upgrade() {
            drag::begin(&ui, &begin_ctx, kind, x, y);
        }
    });
    let ui_weak = ui.as_weak();
    model.on_drag_move(move |x: f32, y: f32, shift: bool| {
        if let Some(ui) = ui_weak.upgrade() {
            drag::move_to(&ui, &queue, x, y, shift);
        }
    });
    let ui_weak = ui.as_weak();
    let end_ctx = ctx.clone();
    model.on_drag_end(move || {
        if let Some(ui) = ui_weak.upgrade() {
            drag::end(&ui, &end_ctx);
        }
    });
    let ui_weak = ui.as_weak();
    let cancel_ctx = ctx.clone();
    model.on_drag_cancel(move || {
        if let Some(ui) = ui_weak.upgrade() {
            drag::cancel(&ui, &cancel_ctx);
        }
    });
}

/// Runs after every solid-preview frame has been stored (see
/// `gui::editor::on_solid_frame_landed`): re-places the handles on the new frame, or,
/// mid-drag, refreshes the "follows your drag" feedback.
pub(super) fn on_frame_landed(ui: &MainWindow, generation: u64) {
    let Some(ctx) = CTX.with(|cell| cell.borrow().clone()) else {
        return;
    };
    // A provisional slice cannot outlive an edit of the committed design, and a frame of
    // the committed design that lands over it (an idle replan, a background solve)
    // brings the provisional picture back.
    if slice::on_landed(ui, &ctx, generation) {
        return;
    }
    slice::sync_outline(ui, &ctx);
    if ui.global::<ManipulateModel>().get_dragging() {
        drag::update_feedback(ui, &ctx);
    } else {
        handles::refresh_handles(ui, &ctx);
    }
}
