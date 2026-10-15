//! The Library's UI-thread state: what is shown, what is selected, the open database
//! handle and the threads behind it.
//!
//! One [`Runtime`] lives in a thread-local for the life of the UI thread. It holds an
//! `Rc` of the app state and the worker threads, so [`shutdown`] must drop it before the
//! thread-local teardown (Windows aborts when a thread is joined from there).

use std::{
    cell::RefCell,
    collections::{BTreeSet, HashSet},
    rc::Rc,
};

use indicatrix_mobile_core::{
    library::{EntryId, FilterOptions, LibraryCard, LibraryHandle},
    settings::LibrarySettings,
    tasks::{BackgroundQueue, Generation},
};
use slint::{Timer, VecModel, Weak};

use super::{filters::FilterState, tags::SheetState, thumbs::ThumbPool};
use crate::{AppWindow, LibraryCardData, TagChipData, design::Shared};

/// How many designs one search page holds.
pub(super) const PAGE: usize = 60;

/// Everything the Library page keeps on the UI thread.
#[expect(
    clippy::struct_excessive_bools,
    reason = "UI-thread runtime record; opening, tag_sheet_open, loading and loading_more are independent progress flags"
)]
pub(super) struct Runtime {
    /// The app state (for the library handle, the platform and the revision).
    pub shared: Shared,
    /// The window, for work that finishes on another thread.
    pub weak: Weak<AppWindow>,
    /// The open library; `None` until the first visit.
    pub handle: Option<LibraryHandle>,
    /// The library is being opened on the queue's thread.
    pub opening: bool,
    /// The thread of all database work except thumbnails.
    pub queue: Option<BackgroundQueue>,
    /// The thumbnail workers.
    pub pool: Option<ThumbPool>,
    /// Names the newest search; older results are dropped.
    pub generation: Generation,
    /// Waits for the typing to pause before searching.
    pub debounce: Timer,
    /// The rows the cards are drawn from.
    pub rows: Rc<VecModel<LibraryCardData>>,
    /// The "Add Tags" chips of the tag sheet.
    pub tag_add_model: Rc<VecModel<TagChipData>>,
    /// The "Remove Tags" chips of the tag sheet.
    pub tag_remove_model: Rc<VecModel<TagChipData>>,
    /// The tag sheet's choices.
    pub tag_sheet: SheetState,
    /// Whether the tag sheet is showing.
    pub tag_sheet_open: bool,
    /// The search text and the filters.
    pub filters: FilterState,
    /// The settings that shape the list.
    pub settings: LibrarySettings,
    /// What the filter sheet offers.
    pub options: FilterOptions,
    /// Why the library could not be opened.
    pub error: String,
    /// The loaded pages.
    pub cards: Vec<LibraryCard>,
    /// How many designs match.
    pub total: usize,
    /// A first page is on its way.
    pub loading: bool,
    /// A further page is on its way.
    pub loading_more: bool,
    /// The library revision the list was read at.
    pub shown_revision: u64,
    /// The first and last card index on screen.
    pub range: (usize, usize),
    /// The card indices whose row holds an image.
    pub loaded: HashSet<usize>,
    /// The entries whose thumbnail failed; they are not asked for again.
    pub failed: HashSet<EntryId>,
    /// The selected designs.
    pub selected: BTreeSet<EntryId>,
    /// The long write that is running (an import), for the busy line.
    pub busy: Option<String>,
    /// The designs the open "Delete Designs?" question is about.
    pub pending_delete: Vec<EntryId>,
    /// A bump makes the grid scroll back to the top.
    pub scroll_token: i32,
}

thread_local! {
    static RUNTIME: RefCell<Option<Runtime>> = const { RefCell::new(None) };
}

/// Installs the runtime. Call once on the UI thread.
pub(super) fn install(runtime: Runtime) {
    RUNTIME.with(|slot| *slot.borrow_mut() = Some(runtime));
}

/// Runs `job` on the runtime. `None` when there is no runtime (before `wire`, after
/// `shutdown`) or when it is already in use further up the stack; the latter is a bug
/// and is logged.
pub(super) fn with<R>(job: impl FnOnce(&mut Runtime) -> R) -> Option<R> {
    RUNTIME.with(|slot| {
        slot.try_borrow_mut().map_or_else(
            |_| {
                tracing::warn!("the library runtime was used re-entrantly; the call was skipped");
                None
            },
            |mut slot| slot.as_mut().map(job),
        )
    })
}

/// Ends the threads and drops every handle. Call on the UI thread after the event loop.
pub(super) fn shutdown() {
    let runtime = RUNTIME.with(|slot| slot.borrow_mut().take());
    // Dropped here, outside the borrow: the drops join threads.
    drop(runtime);
}
