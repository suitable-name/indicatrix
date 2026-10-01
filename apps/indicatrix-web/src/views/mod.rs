//! The Solid and Diagram tabs: the desktop's solid viewport in its
//! Solid (view mode 0) and Diagram (view mode 3) modes, drawn on the main thread
//! by `indicatrix_solid::preview` -- the SAME planner/state machine/rasterizer the
//! desktop's two worker threads run, so the same design, pose and size give the
//! same pixels (`indicatrix_solid`'s `preview::tests` pin that against bytes
//! recorded from the desktop).
//!
//! # How a frame comes about
//!
//! [`refresh`] compares what the views last showed (design generation, solve,
//! selection, multi-selection, camera, view size, the view's own options) with
//! the app's state now and does the least work that brings the frame up to date,
//! exactly the desktop's three request kinds:
//! - **replan** (`PreviewPipeline::plan` + `render_planned`, the desktop's PLAN
//!   worker + `RedrawRequest::Planned`) after an edit, a solve, a selection change
//!   or a Preform/Cut/enlarged-panel change, budgeted by
//!   `live_update::DEFAULT_PREVIEW_BUDGET` against a `performance.now()` clock;
//! - **reproject** (`RedrawRequest::Reproject`) for a camera move, a resize or a
//!   tab switch;
//! - **overlay** (`RedrawRequest::UpdateFacetOverlay`) for hover, click and the
//!   multi-selection highlight.
//!
//! A replan that comes back `Stale` (the subgraph re-solve overran the budget)
//! shows the previous planes with the edited tiers outlined as pending, exactly as
//! the desktop does, and replans once more on the next event-loop turn from the
//! late result it chained forward. A design with no usable masts (first load, a
//! tier added or removed) is planned right away when the desktop would solve it
//! synchronously (`solve_policy::should_solve_synchronously`, or every tier
//! pinned); otherwise the views keep the last frame, ask the app for a solve
//! (`app::solve::with_solved`, answered by the solve Worker for larger designs) and
//! replan when it arrives.
//!
//! # When [`refresh`] runs
//!
//! On every input in the views (coalesced to one run per event-loop turn by
//! [`request_refresh`]), and from a 100 ms poll while a Solid/Diagram tab is shown
//! -- which is how an edit, undo, load or an asynchronous solve result reaches the
//! views without any other module having to call in.
//!
//! # Direct manipulation
//!
//! [`manip`] adds the desktop's mouse-driven tools to the Solid view: the angle, depth
//! and index drag handles on the selected facet and the Slice tool. It hooks into
//! [`refresh`] (`manip::frame_landed` after every shown frame, `manip::expire` before
//! each run, and `replan_now` plans the provisional slice's design while a session
//! exists) and into the click handler; see its own module doc comment.
//!
//! # Selection API (for the tier table)
//!
//! - `WebApp::selected_tier: Option<usize>` is the selected tier. A click on a
//!   facet (Solid) or a facet fill (Diagram) sets it; a click that misses clears
//!   it; Escape clears it; Up/Down/PageUp/PageDown step it. Like the desktop, every
//!   such plain selection also empties `EditorSession::multi_selected`.
//! - `EditorSession::multi_selected` is the Ctrl+click group; its tiers' facets
//!   are outlined in both views.
//! - Change either and the views follow within one poll; call
//!   [`request_refresh`] (or [`select_tier`], which also applies the desktop's
//!   multi-selection rule) for an immediate redraw.
//! - The last index-wheel tooth clicked in the Diagram is
//!   `SolidModel.clicked-tooth` (the desktop's `diagram_clicked_tooth`, which its
//!   tier table highlights).

mod input;
mod manip;
mod present;
mod refresh;
mod state;

use crate::{AppWindow, app::Ctx};
use slint::{Timer, TimerMode};
use std::{cell::RefCell, time::Duration};

pub use refresh::refresh;

/// How often the views check the app state for changes they did not cause.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

thread_local! {
    /// The views' pipeline and last-frame state (single-threaded, like `WebApp`).
    static VIEWS: RefCell<state::ViewsState> = RefCell::new(state::ViewsState::default());
    /// The one-shot timer [`request_refresh`] restarts, so a burst of input events
    /// in one event-loop turn draws once.
    static REFRESH_TIMER: Timer = Timer::default();
    /// The poll timer (kept alive for the page's lifetime).
    static POLL_TIMER: Timer = Timer::default();
}

/// Wires the `SolidModel` callbacks and starts the poll. Call once, after
/// `app::callbacks::wire`.
pub fn wire(ui: &AppWindow, ctx: &Ctx) {
    input::wire(ui, ctx);
    manip::wire(ui, ctx);
    let poll = ctx.clone();
    POLL_TIMER.with(|timer| {
        timer.start(TimerMode::Repeated, POLL_INTERVAL, move || refresh(&poll));
    });
}

/// Brings the Solid/Diagram view up to date on the next event-loop turn;
/// repeated calls within one turn coalesce into one [`refresh`].
pub fn request_refresh(ctx: &Ctx) {
    let ctx = ctx.clone();
    REFRESH_TIMER.with(|timer| {
        timer.start(TimerMode::SingleShot, Duration::ZERO, move || refresh(&ctx));
    });
}

/// Selects `tier` (or nothing) the way a plain click does on the desktop: sets
/// `WebApp::selected_tier`, empties `EditorSession::multi_selected`, and redraws.
pub fn select_tier(ctx: &Ctx, tier: Option<usize>) {
    {
        let mut app = ctx.state.borrow_mut();
        let tier_count = app
            .design
            .as_ref()
            .map_or(0, |d| d.session.design.tiers.len());
        app.selected_tier = tier.filter(|&t| t < tier_count);
        if let Some(design) = app.design.as_mut() {
            design.session.multi_selected.clear();
        }
    }
    request_refresh(ctx);
}
