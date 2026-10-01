//! The inspector (Tier / Preform / Optimize / Schedule tabs), the panels beside it and
//! their refresh: modelled on the desktop's `gui/editor/view/{inspector,panel}.rs` and
//! `callbacks/tier_actions/{tier_form,facet_editing,design_forms}.rs`.
//!
//! # State and refresh
//!
//! The design lives in [`WebApp`](crate::app::state::WebApp); the panels are pushed from
//! it into the Slint globals `InspectorModel`, `DesignSettingsModel` and `OptimizeModel`
//! by [`refresh`]. A 100 ms poll compares a small fingerprint (design generation, dirty
//! marker, selection, solve, the visible tab) with the last push and refreshes when
//! it changed -- how an edit made anywhere (the tier table, the Solid view, undo,
//! open, an asynchronous solve result) reaches these panels without those modules
//! calling in -- and every edit made here ends in [`finish`], which refreshes at once.
//!
//! # The tier form
//!
//! The form's five fields are scratch state seeded from the selected tier and committed
//! only by Save / Add Tier (`form`): the desktop's rule that a selection change never
//! silently discards a typed draft, and that undo / redo re-seed a clean form.
//!
//! # Modules
//!
//! - `form`: seeding, the dirty-draft guard, the live margin bar and Save / Add Tier;
//! - `facets`: the chips, add / rotate / mirror, cheater offset and tier note;
//! - `preform`: the Preform tab's Apply Preform and Apply Yield Inputs;
//! - `readouts`: the Solved section, the proportions and yield figures, the schedule rows;
//! - `poll`: the fingerprint and the refresh orchestration.

mod facets;
mod form;
mod poll;
mod preform;
mod readouts;

pub use form::open_anchor;
pub use poll::refresh;

use crate::{
    AppWindow,
    app::{
        Ctx,
        state::{DesignState, WebApp},
    },
    editor::edit::{Dirty, finish_edit},
};
use indicatrix::{geometry::meet_solver::SolvedTier, optics::materials::GemMaterial};
use indicatrix_cut_core::Design;
use indicatrix_editor::{
    scratch::ScratchDelta,
    view_model::{
        TierRow,
        rows::{tier_items_from_solved, tier_items_stale},
    },
};
use std::{cell::OnceCell, collections::BTreeSet};

/// Everything one refresh pushes from: the app, the design, what changed since the last
/// push, and the values every panel needs.
pub struct PushCtx<'a> {
    /// The window whose globals are written.
    pub ui: &'a AppWindow,
    /// The whole app state (read-only for the push).
    pub app: &'a WebApp,
    /// The loaded design and its session.
    pub design_state: &'a DesignState,
    /// `design_state.session.design`.
    pub design: &'a Design,
    /// Which form groups changed since the last push (see `indicatrix_editor::scratch`).
    pub delta: &'a ScratchDelta,
    /// The last solve, when it matches the design's generation.
    pub solved: Option<&'a [SolvedTier]>,
    /// The design's effective refractive index (custom-material aware).
    pub n_d: f64,
    /// The session's custom materials.
    pub custom: &'a [GemMaterial],
    /// The design was replaced wholesale (New, Open, restore) since the last push.
    pub replaced: bool,
    /// The selection of the last push (`None` when nothing was selected, or on a
    /// replacement).
    pub previous_selection: Option<usize>,
    /// The design generation of the last push.
    pub previous_generation: Option<u64>,
    /// The tier count of the last push.
    pub previous_tiers: usize,
    /// The inspector tab showing (0 Tier .. 3 Schedule).
    pub tab: i32,
    /// Whether the inspector is collapsed.
    pub collapsed: bool,
    /// The tier rows, built on first use (see [`Self::rows`]).
    rows_cell: OnceCell<Vec<TierRow>>,
}

impl PushCtx<'_> {
    /// The tier rows of the design as the tier table shows them: from the last solve when
    /// it is current, otherwise without one. Built once per push, only when a panel needs
    /// them.
    pub fn rows(&self) -> &[TierRow] {
        self.rows_cell.get_or_init(|| {
            self.solved.map_or_else(
                || tier_items_stale(self.design, self.n_d),
                |solved| tier_items_from_solved(self.design, solved, self.n_d),
            )
        })
    }

    /// The selected tier, if it exists.
    #[must_use]
    pub fn selected(&self) -> Option<usize> {
        self.app
            .selected_tier
            .filter(|&i| i < self.design.tiers.len())
    }
}

/// Ends every edit made from these panels: the shared edit path (design summary,
/// auto-solve, persistence, the views, the tier table), then the panels themselves.
/// Call with no `RefCell` borrow held.
pub fn finish(ctx: &Ctx, dirty: Dirty) {
    finish_edit(ctx, dirty);
    refresh(ctx);
}

/// [`finish`] for an edit that moves no tier's mast (a preform, material or note edit).
pub fn finish_no_tier(ctx: &Ctx) {
    finish(ctx, Dirty::Tiers(BTreeSet::new()));
}

/// Wires the inspector's callbacks and starts the refresh poll.
pub fn wire(ui: &AppWindow, ctx: &Ctx) {
    form::wire(ui, ctx);
    facets::wire(ui, ctx);
    preform::wire(ui, ctx);
    poll::start(ctx);
}
