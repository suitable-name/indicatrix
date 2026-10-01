//! Mouse-driven direct manipulation on the Solid view: the angle, depth and index drag
//! handles of the selected facet and the Slice tool, with the desktop's behaviour.
//!
//! The math (projection, handle placement, hit-testing, snapping, wording, the
//! throttled-value decisions, the provisional slice tier) lives in
//! `indicatrix_editor::manipulate`, shared with the desktop; this module is the web
//! wiring around it.
//!
//! # The pick frame
//!
//! Everything from the math side is in the **pick frame**: pixels of the solid raster
//! (which is also its pick buffer). The raster is shown with `image-fit: contain`, so a
//! logical pointer position is mapped through the same placement the `Image` is drawn
//! with (`indicatrix_solid::preview::view::ContainFit`), unclamped, and the handles are
//! mapped back the same way to be drawn.
//!
//! # How a drag reaches the design
//!
//! `solid.slint` routes the pointer to the `ManipulateModel` callbacks:
//!
//! 1. `handle-hit-test` / `handle-hover` ([`place`]): map the pointer to the pick frame,
//!    hit-test it against the handle layout of the last frame, light the handle and word
//!    the hint.
//! 2. `drag-begin` ([`drag`]): a [`DragStart`](indicatrix_editor::manipulate::DragStart)
//!    (start angle from the design, start mast from the last planned frame, the layout)
//!    and an ended coalescing run, so the gesture is its own undo step.
//! 3. `drag-move`: `drag_value` posts an `EditIntent::DragTier` into a single-slot queue
//!    ([`queue`]) that drains once per 16 ms, so a burst of pointer moves costs ONE
//!    apply/replan per frame.
//! 4. The drain applies the value through `EditorSession::{set_tier_angle,
//!    pin_tier_mast, rotate_tier_indices}` (coalescing edits on one gesture clock), tells
//!    the app the design changed and asks the views for a replan; the frame that comes
//!    back re-places the handles' feedback (the tiers that follow the drag are outlined).
//! 5. `drag-end` flushes the newest pointer value, ends the coalescing run, saves and
//!    toasts; `drag-cancel` undoes the gesture's one step.
//!
//! # Slice mode
//!
//! [`slice`] adds the Slice tool on top of the same session: a mouse line becomes a
//! PROVISIONAL tier (never in the `EditorSession`, never in the undo history) that the
//! views render by planning the committed design plus that tier under the reserved
//! generation `PROVISIONAL_GENERATION`. Such frames never touch the views' solve cache;
//! the handles attach to the provisional facet and a drag on it edits the session's
//! design clone in place. Keep is the one `Edit::AddTier` (one undo step); Discard drops
//! the session.
//!
//! # Where it hooks into the views
//!
//! - [`frame_landed`], from `refresh` after every shown frame: re-places the handles, or
//!   mid-drag refreshes the "follows your drag" feedback, and syncs the provisional
//!   outline;
//! - [`expire`], from `refresh` before it looks at the app: a provisional slice cannot
//!   outlive an edit of the committed design or a change of the selected tier;
//! - `refresh::replan_now` plans the provisional design instead of the committed one
//!   while a session exists.

mod drag;
mod place;
mod queue;
mod slice;

use super::{VIEWS, state::ViewsState};
use crate::{AppWindow, ManipulateModel, SolidModel, app::Ctx};
use indicatrix::geometry::meet_solver::SolvedTier;
use indicatrix_cut_core::Design;
use indicatrix_editor::manipulate::{ActiveDrag, HandleKind, HandleTarget, ProvisionalSlice};
use indicatrix_solid::facet_map::FacetMap;
use slint::ComponentHandle;
use std::{collections::BTreeSet, rc::Rc, sync::Arc};

pub use place::frame_landed;
pub use slice::expire;

/// The [`FacetMap`] of one solve-cache revision, kept so a stream of camera frames does not
/// rebuild it.
struct CachedFacetMap {
    /// `ViewsState::cache_rev` it was built for.
    rev: u64,
    map: Rc<FacetMap>,
}

/// What a provisional replan plans: the session's design and what it chains from.
pub struct ProvisionalPlan {
    /// The committed design plus the provisional tier.
    pub design: Arc<Design>,
    /// The masts to chain from (`None`: a full solve).
    pub last_solved: Option<Vec<SolvedTier>>,
    /// The tiers to re-solve when chaining.
    pub dirty: BTreeSet<usize>,
}

/// The direct-manipulation tools' state (one per page, inside `ViewsState`).
#[derive(Default)]
pub struct ManipState {
    /// The last clicked Solid facet (the desktop's `SELECTED_FACET_ID`): the facet the
    /// handles sit on while it belongs to the selected tier.
    remembered_facet: Option<u32>,
    /// Where the handles are drawn now; `None` while they are hidden.
    target: Option<HandleTarget>,
    facet_map: Option<CachedFacetMap>,
    /// The handle under the pointer (no drag in progress).
    hovered: Option<HandleKind>,
    /// Whether `ManipulateModel.hint-text` currently holds OUR hover hint (so hiding the
    /// handles clears it without wiping a hint another tool wrote).
    hover_hint_active: bool,
    /// The gesture in progress, if any.
    drag: Option<ActiveDrag>,
    /// The Slice tool's line gesture: where the press started (logical).
    slice_gesture: Option<(f32, f32)>,
    /// The provisional slice tier, if there is one.
    provisional: Option<ProvisionalSlice>,
    /// The tier selected when the session began; a change of it ends the session.
    provisional_selected: Option<usize>,
    /// Whether a `Stale` provisional frame already scheduled its one follow-up replan.
    provisional_followup: bool,
    /// The Cut slider value the resting hint was last worded for.
    last_cutoff: Option<i32>,
}

impl ManipState {
    /// Whether a provisional slice exists right now (facet clicks select nothing then).
    #[must_use]
    pub const fn has_provisional(&self) -> bool {
        self.provisional.is_some()
    }

    /// Remembers the Solid facet a click landed on (`None`: the click missed).
    pub const fn remember_facet(&mut self, facet: Option<u32>) {
        self.remembered_facet = facet;
    }

    /// What the next provisional replan plans -- see `ProvisionalSlice::replan_chain`.
    /// `committed_masts` are the committed design's.
    #[must_use]
    pub fn provisional_inputs(
        &self,
        committed_masts: Option<&[SolvedTier]>,
    ) -> Option<ProvisionalPlan> {
        let slice = self.provisional.as_ref()?;
        let (last_solved, dirty) = slice.replan_chain(committed_masts);
        Some(ProvisionalPlan {
            design: Arc::clone(&slice.design),
            last_solved,
            dirty,
        })
    }

    /// Hands the solved masts of a provisional frame to the session.
    pub fn note_provisional_masts(&mut self, masts: Vec<SolvedTier>) {
        if let Some(slice) = self.provisional.as_mut() {
            slice.note_masts(masts);
        }
    }

    /// Decides whether a provisional frame needs its one follow-up replan: a `Stale`
    /// frame (the subgraph re-solve overran the budget) does, once, until an edit or a
    /// fresh frame resets it.
    pub const fn wants_followup(&mut self, stale: bool) -> bool {
        if !stale {
            self.provisional_followup = false;
            return false;
        }
        if self.provisional_followup {
            return false;
        }
        self.provisional_followup = true;
        true
    }
}

/// Wires every `ManipulateModel` callback. Call once, from `views::wire`.
pub fn wire(ui: &AppWindow, ctx: &Ctx) {
    place::wire(ui, ctx);
    drag::wire(ui, ctx);
    slice::wire(ui, ctx);
}

/// Writes `text` into the hint line under the toolbar.
fn set_hint(ui: &AppWindow, text: &str) {
    ui.global::<ManipulateModel>().set_hint_text(text.into());
}

/// The Cut slider's value (`-1`: the whole design).
fn cutoff(ui: &AppWindow) -> i32 {
    ui.global::<SolidModel>().get_tier_cutoff()
}

/// Runs `f` on the views' state.
fn with_views<R>(f: impl FnOnce(&mut ViewsState) -> R) -> R {
    VIEWS.with(|cell| f(&mut cell.borrow_mut()))
}
