//! The Inspector's "History" tab: every step of the open design's undo history as a list,
//! with a small picture of the design at each step, and a click to go back to it.
//!
//! The list, the pictures and the jump all work on the editor's own history (see
//! `indicatrix_editor::EditorSession::history_entries`, `jump_to` and `history_snapshot`);
//! this module is the glue to the tab (`ui/components/editor_inspector/history_tab.slint`,
//! state in `ui/models/history.slint`).
//!
//! # Keeping the list current
//!
//! Many places change the design (every edit callback, Undo, Redo, Optimize, Retarget, the
//! drag handles), and none of them should have to know about this tab. So a UI-thread timer
//! ([`TICK`]) looks at a cheap [`Signature`] of the editor (design epoch, edit generation,
//! undo and redo depth, picture size) and rebuilds the rows only when it changed -- and only
//! while the tab is showing. The tab also asks for an immediate refresh when it is opened,
//! and a jump refreshes right away. The rows are updated in place (row by row), so the list
//! keeps its scroll position while a drag adds steps.
//!
//! # Pictures
//!
//! A row asks `HistoryModel.thumbnail` for its picture, and only the rows in view exist (the
//! list is virtualised), so only the pictures you can see are drawn. The callback answers
//! from a cache ([`thumbs::ThumbCache`], keyed by [`thumbs::ThumbKey`]: design, step,
//! revision, words, size) and, on a miss, queues the picture for the [`worker::Worker`]
//! thread, which works out the design at that step on a copy, solves it and draws it with
//! the flat solid raster. Nothing here ever waits for the thread: the timer collects finished
//! pictures and bumps `HistoryModel.thumb_version` so the visible rows ask again.
//!
//! # Jump
//!
//! A click calls `EditorSession::jump_to_mapped` and then the same refresh Undo and Redo end
//! in (`refresh_after_renumbering_move`: the selected row follows its tier, then a full
//! re-solve, selection check, preview replan). Jumping is not itself a step, so every step
//! stays where it is and the cutter can jump back; a new change after a jump back drops the
//! undone steps, as after an ordinary undo.

mod render;
mod rows;
#[cfg(test)]
mod tests;
mod thumbs;
mod worker;

use super::{
    callbacks::{HistoryMove, refresh_after_renumbering_move},
    state::EditorState,
};
use crate::{
    EditorModel, GuideModel, HistoryModel, HistoryRowData, HistoryThumb, MainWindow,
    bridge::render_thread::RenderContext,
    gui::{
        show_toast,
        solid_preview::preview_state::{SolidLastSolved, SolidPreviewState},
    },
};
use rows::{RowSpec, build_rows};
use slint::{ComponentHandle, Image, Model, ModelRc, VecModel};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex, atomic::Ordering},
    time::Duration,
};
use thumbs::{CACHE_CAPACITY, Lookup, ThumbCache, ThumbKey, edge_px, thumb_keys};
use worker::{Finished, Outcome, RenderSnapshot, Worker};

/// `EditorModel.inspector_tab` of the History tab.
const HISTORY_TAB: i32 = 4;

/// How often the timer looks for a changed history and finished pictures.
const TICK: Duration = Duration::from_millis(150);

/// `n` as the `int` Slint wants, saturating.
fn to_i32(n: usize) -> i32 {
    i32::try_from(n).unwrap_or(i32::MAX)
}

/// What decides whether the rows are out of date.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Signature {
    /// The design epoch: bumped when New / Open / Load swap in another design.
    epoch: u64,
    /// The edit generation: bumped by every edit, undo, redo and jump.
    generation: u64,
    /// Steps that can be undone.
    undo: usize,
    /// Steps that can be redone.
    redo: usize,
    /// The picture edge in physical pixels (the window's scale factor changed).
    edge_px: u32,
}

impl Signature {
    fn of(state: &EditorState, edge_px: u32) -> Self {
        Self {
            epoch: state.design_epoch.load(Ordering::Relaxed),
            generation: state.current_generation(),
            undo: state.history.len_undo(),
            redo: state.history.len_redo(),
            edge_px,
        }
    }
}

/// The picture a row shows while there is none yet.
fn waiting_thumb() -> HistoryThumb {
    HistoryThumb {
        picture: Image::default(),
        state: 0,
    }
}

/// The Slint row for a [`RowSpec`].
fn to_row_data(spec: &RowSpec) -> HistoryRowData {
    HistoryRowData {
        position: to_i32(spec.position),
        title: spec.title.as_str().into(),
        detail: spec.detail.as_str().into(),
        current: spec.current,
        undone: spec.undone,
        start: spec.start,
    }
}

/// Everything the tab keeps between ticks. UI thread only.
struct Panel {
    /// The rows the list shows.
    model: Rc<VecModel<HistoryRowData>>,
    /// The rows as last pushed, to update only the ones that differ.
    shown: Vec<RowSpec>,
    /// What the rows were built from.
    signature: Option<Signature>,
    /// The key of every row's picture, indexed by position.
    keys: Vec<ThumbKey>,
    cache: ThumbCache<Image>,
    worker: Worker,
    /// `HistoryModel.thumb_version`: bumped whenever pictures may have changed.
    version: i32,
    /// `HistoryModel.current_position` and `step_count`, as last built.
    current_position: usize,
    step_count: usize,
}

impl Panel {
    fn new(model: Rc<VecModel<HistoryRowData>>) -> Self {
        Self {
            model,
            shown: Vec::new(),
            signature: None,
            keys: Vec::new(),
            cache: ThumbCache::new(CACHE_CAPACITY),
            worker: Worker::spawn(),
            version: 0,
            current_position: 0,
            step_count: 0,
        }
    }

    /// A row's picture: from the cache, or marked as being drawn and queued. Never waits.
    fn thumbnail(&mut self, position: i32) -> HistoryThumb {
        let Some(key) = usize::try_from(position)
            .ok()
            .and_then(|position| self.keys.get(position))
            .cloned()
        else {
            return waiting_thumb();
        };
        match self.cache.lookup(&key) {
            Lookup::Ready(picture) => HistoryThumb { picture, state: 1 },
            Lookup::Failed => HistoryThumb {
                picture: Image::default(),
                state: 2,
            },
            Lookup::Waiting => waiting_thumb(),
            Lookup::Requested => {
                self.worker.request(key);
                waiting_thumb()
            }
        }
    }

    /// Takes in the pictures the worker finished. Returns whether there were any (the
    /// visible rows then have to ask again).
    fn collect_finished(&mut self) -> bool {
        let finished = self.worker.take_finished();
        if finished.is_empty() {
            return false;
        }
        for Finished { key, outcome } in finished {
            match outcome {
                Outcome::Ready(pixels) => self.cache.store_ready(key, Image::from_rgba8(pixels)),
                Outcome::CannotDraw => self.cache.store_failed(key),
                Outcome::Stale => self.cache.forget_waiting(&key),
            }
        }
        self.bump_version();
        true
    }

    /// Rebuilds the rows, the picture keys and the worker's snapshot when `state`'s history
    /// differs from what they were built from (or the picture size changed). Returns
    /// whether it did.
    fn sync(&mut self, state: &EditorState, edge: u32) -> bool {
        let signature = Signature::of(state, edge);
        if self.signature == Some(signature) {
            return false;
        }
        let entries = state.history_entries();
        let position = state.history_position();
        self.push_rows(build_rows(&entries, position));
        self.keys = thumb_keys(signature.epoch, &entries, edge);
        self.cache.retain_epoch(signature.epoch);
        self.worker.set_snapshot(Arc::new(RenderSnapshot::new(
            signature.epoch,
            edge,
            state.history_snapshot(),
        )));
        self.current_position = position;
        self.step_count = entries.len();
        self.signature = Some(signature);
        self.bump_version();
        true
    }

    /// Makes the list show `specs`, touching only the rows that differ, so the list keeps
    /// its scroll position and its row components.
    fn push_rows(&mut self, specs: Vec<RowSpec>) {
        for (index, spec) in specs.iter().enumerate() {
            if self.shown.get(index) == Some(spec) {
                continue;
            }
            let data = to_row_data(spec);
            if index < self.model.row_count() {
                self.model.set_row_data(index, data);
            } else {
                self.model.push(data);
            }
        }
        while self.model.row_count() > specs.len() {
            self.model.remove(self.model.row_count() - 1);
        }
        self.shown = specs;
    }

    const fn bump_version(&mut self) {
        self.version = self.version.wrapping_add(1);
    }

    /// Writes the numbers the tab reads into `HistoryModel`.
    fn publish(&self, ui: &MainWindow) {
        let model = ui.global::<HistoryModel>();
        model.set_current_position(to_i32(self.current_position));
        model.set_step_count(to_i32(self.step_count));
        model.set_thumb_version(self.version);
    }
}

/// What the callbacks of the tab share.
struct Deps {
    state: Rc<RefCell<EditorState>>,
    render_ctx: Arc<Mutex<RenderContext>>,
    preview_state: Arc<SolidPreviewState>,
    solid_last_solved: SolidLastSolved,
    panel: RefCell<Panel>,
}

/// Whether the History tab is on screen (selected and its section not collapsed).
fn history_tab_showing(ui: &MainWindow) -> bool {
    let editor = ui.global::<EditorModel>();
    editor.get_inspector_tab() == HISTORY_TAB && !editor.get_inspector_collapsed()
}

/// One look at the editor: collects finished pictures and, while the tab shows, rebuilds
/// the list if the history moved. A no-op when the panel or the state is busy (a callback is
/// in the middle of using it); the next tick tries again.
fn refresh(ui: &MainWindow, deps: &Deps) {
    let Ok(mut panel) = deps.panel.try_borrow_mut() else {
        return;
    };
    let mut changed = panel.collect_finished();
    if history_tab_showing(ui)
        && let Ok(state) = deps.state.try_borrow()
    {
        changed |= panel.sync(&state, edge_px(ui.window().scale_factor()));
    }
    if changed {
        panel.publish(ui);
    }
}

/// Goes to step `position` and refreshes everything a move along the history refreshes.
fn jump(ui: &MainWindow, deps: &Deps, position: i32) {
    // The same lock the Undo and Redo buttons have.
    if !ui.global::<GuideModel>().invoke_allows_history() {
        return;
    }
    let Ok(position) = usize::try_from(position) else {
        return;
    };
    {
        let Ok(mut state) = deps.state.try_borrow_mut() else {
            return;
        };
        // The map is every undo or redo of the jump composed into one, so the selected
        // row follows its tier across the whole jump (also one that stopped early).
        let (result, renumbering) = state.jump_to_mapped(position);
        let change = match result {
            Ok(outcome) => outcome.change,
            Err(failure) => {
                show_toast(
                    ui,
                    &format!("Could not go to that step. {failure}"),
                    "error",
                );
                failure.change
            }
        };
        if let Some(change) = change {
            refresh_after_renumbering_move(
                ui,
                &deps.render_ctx,
                &deps.preview_state,
                &deps.solid_last_solved,
                &state,
                &HistoryMove::new(change, renumbering),
            );
        }
    }
    refresh(ui, deps);
}

thread_local! {
    /// The timer behind [`TICK`]; kept here so it lives as long as the window's callbacks.
    /// UI-thread-only.
    static TICKER: RefCell<Option<slint::Timer>> = const { RefCell::new(None) };
}

/// Wires the History tab: its picture lookup, the jump, the "opened" refresh and the timer
/// that keeps the list current. Called once from `setup_editor_callbacks`.
pub(in crate::gui::editor) fn setup_history_panel(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    let model = Rc::new(VecModel::<HistoryRowData>::default());
    let history = ui.global::<HistoryModel>();
    history.set_rows(ModelRc::from(Rc::clone(&model)));

    let deps = Rc::new(Deps {
        state: Rc::clone(state),
        render_ctx: Arc::clone(render_ctx),
        preview_state: Arc::clone(preview_state),
        solid_last_solved: Arc::clone(solid_last_solved),
        panel: RefCell::new(Panel::new(model)),
    });

    // Asked while a row is drawn, with a panel that is busy only if a refresh is in the
    // middle of writing the list: that row shows "Drawing" and asks again at the next bump.
    let lookup_deps = Rc::clone(&deps);
    history.on_thumbnail(move |position, _version| {
        lookup_deps
            .panel
            .try_borrow_mut()
            .map_or_else(|_| waiting_thumb(), |mut panel| panel.thumbnail(position))
    });

    let jump_deps = Rc::clone(&deps);
    let jump_weak = ui.as_weak();
    history.on_jump(move |position| {
        if let Some(ui) = jump_weak.upgrade() {
            jump(&ui, &jump_deps, position);
        }
    });

    let opened_deps = Rc::clone(&deps);
    let opened_weak = ui.as_weak();
    history.on_opened(move || {
        if let Some(ui) = opened_weak.upgrade() {
            refresh(&ui, &opened_deps);
        }
    });

    let timer = slint::Timer::default();
    let tick_weak = ui.as_weak();
    timer.start(slint::TimerMode::Repeated, TICK, move || {
        if let Some(ui) = tick_weak.upgrade() {
            refresh(&ui, &deps);
        }
    });
    TICKER.with(|cell| *cell.borrow_mut() = Some(timer));
}
