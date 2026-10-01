//! The tier table's data: builds the rows, the solve status and the warning list from
//! [`WebApp`] and keeps `TierTableModel` (the Slint global behind the Design dock) in step
//! with them.
//!
//! # When the table is pushed
//!
//! [`sync`] compares what the table last showed with the app's state now and pushes only
//! what changed:
//!
//! - the **rows** (`indicatrix_editor::view_model::rows`, the desktop's own builders) when
//!   the design, the solve or the set of tiers an edit made untrustworthy changed;
//! - the **selection** (`WebApp::selected_tier`, `EditorSession::multi_selected`) -- cheap,
//!   it only patches the rows' multi-select flags;
//! - the **status** (solving / solved / stale / failed, the solver's sentence, the
//!   undo/redo labels) and the **warnings**.
//!
//! It runs from a 100 ms poll -- so a load, an undo from the menu, a solve result or a
//! click on a facet in the Solid view all reach the table without those modules knowing
//! about it -- and immediately after every edit of this module ([`super::edit::finish_edit`]).
//!
//! # Which masts the rows show
//!
//! - a solve of the current design: `tier_items_from_solved`;
//! - after an edit, before the next solve: `tier_items_stale_with_last_solved`, which keeps
//!   the previous solve's mast (marked "stale") for every tier the edits did not touch and
//!   blanks the touched ones, exactly as the desktop's `refresh_editor_panel_stale` does.
//!   The touched set ([`Dirty`]) accumulates until the next solve lands;
//! - a design that failed to solve (a missing scale anchor, ...): `tier_items`, which
//!   names the tiers responsible -- only for a design small enough to solve on the page,
//!   since it re-solves; a larger one shows the stale rows and the solver's sentence.

use super::edit::Dirty;
use crate::{
    AppWindow, TierRowData, TierTableModel, WarningLine,
    app::{
        Ctx,
        solve::is_solving,
        state::{SolveState, WebApp},
    },
};
use indicatrix_editor::{
    solve_policy::{SolveCostEstimate, should_solve_synchronously_for},
    view_model::{
        TierRow,
        rows::{
            manufacturability_warnings_tagged, tier_items, tier_items_from_solved,
            tier_items_stale_with_last_solved,
        },
        solid_status::tier_matches_filter,
    },
};
use slint::{ComponentHandle, Model, ModelRc, SharedString, Timer, TimerMode, VecModel};
use std::{cell::RefCell, collections::BTreeSet, rc::Rc, time::Duration};

/// How often the table checks the app state for changes it did not cause.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// The tiers whose last solved mast the edits since the last solve made untrustworthy.
#[derive(Debug, Default)]
struct StaleTiers {
    /// Every tier (an edit whose blast radius is not tracked: undo, redo, move, remove).
    all: bool,
    tiers: BTreeSet<usize>,
}

/// Which solve the design currently has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SolveKey {
    None,
    Solved(u64),
    Failed(u64),
}

/// What the pushed rows were built from.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RowsKey {
    generation: Option<u64>,
    solve: SolveKey,
    stale_epoch: u64,
    custom_materials: usize,
}

/// What the pushed selection was read from.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SelectionKey {
    selected: Option<usize>,
    multi: BTreeSet<usize>,
    tiers: usize,
}

/// What the pushed status was read from.
#[derive(Debug, Clone, PartialEq, Eq)]
struct StatusKey {
    generation: Option<u64>,
    solve: SolveKey,
    solving: bool,
    auto_solve_budget_ms: u32,
    has_design: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Snapshot {
    rows: RowsKey,
    selection: SelectionKey,
    status: StatusKey,
}

/// The table's own state: what edits made stale, what was last pushed, and the models.
#[derive(Default)]
struct TableState {
    stale: StaleTiers,
    /// Bumped whenever [`Self::stale`] gains tiers, so the rows are rebuilt.
    stale_epoch: u64,
    /// [`Self::stale_epoch`] as of the last rebuild: a design generation that moved with
    /// the epoch unchanged was edited by someone else.
    rebuilt_epoch: u64,
    pushed: Option<Snapshot>,
    /// The rows as built (multi-select flags not applied).
    rows: Vec<TierRow>,
    /// `(tier, text)`, tier `-1` for none.
    warnings: Vec<(i32, String)>,
    row_model: Option<Rc<VecModel<TierRowData>>>,
    warning_model: Option<Rc<VecModel<WarningLine>>>,
}

thread_local! {
    static TABLE: RefCell<TableState> = RefCell::new(TableState::default());
    /// The poll timer (kept alive for the page's lifetime).
    static POLL: Timer = Timer::default();
}

/// Records that the next rows must not trust the previous solve's masts for `dirty`.
pub fn mark_stale(dirty: Dirty) {
    TABLE.with(|cell| {
        let mut table = cell.borrow_mut();
        match dirty {
            Dirty::All => table.stale.all = true,
            Dirty::Tiers(tiers) => table.stale.tiers.extend(tiers),
        }
        table.stale_epoch += 1;
    });
}

const fn solve_key(app: &WebApp) -> SolveKey {
    match &app.solve {
        SolveState::NotSolved => SolveKey::None,
        SolveState::Solved { generation, .. } => SolveKey::Solved(*generation),
        SolveState::Failed { generation, .. } => SolveKey::Failed(*generation),
    }
}

impl TableState {
    fn snapshot(&self, app: &WebApp, solving: bool) -> Snapshot {
        let (multi, tiers) = app.design.as_ref().map_or_else(
            || (BTreeSet::new(), 0),
            |d| {
                (
                    d.session.multi_selected.clone(),
                    d.session.design.tiers.len(),
                )
            },
        );
        Snapshot {
            rows: RowsKey {
                generation: app.generation(),
                solve: solve_key(app),
                stale_epoch: self.stale_epoch,
                custom_materials: app.custom_materials.len(),
            },
            selection: SelectionKey {
                selected: app.selected_tier,
                multi,
                tiers,
            },
            status: StatusKey {
                generation: app.generation(),
                solve: solve_key(app),
                solving,
                auto_solve_budget_ms: app.auto_solve_budget_ms,
                has_design: app.design.is_some(),
            },
        }
    }

    /// Rebuilds [`Self::rows`] and [`Self::warnings`] for the app's current state.
    fn rebuild(&mut self, app: &WebApp) {
        let Some(design) = &app.design else {
            self.rows.clear();
            self.warnings.clear();
            self.stale = StaleTiers::default();
            return;
        };
        let d = &design.session.design;
        let n_d = d.effective_refractive_index_with(&app.custom_materials);
        if let Some(solved) = app.current_solved() {
            self.stale = StaleTiers::default();
            self.rows = tier_items_from_solved(d, solved, n_d);
            self.warnings = match &app.solve {
                SolveState::Solved { warnings, .. } => warnings
                    .iter()
                    .map(|w| (i32::try_from(w.tier_index).unwrap_or(-1), w.text.clone()))
                    .collect(),
                _ => Vec::new(),
            };
            return;
        }
        // Not solved for this design (yet): only the two mast-free manufacturability checks
        // are real, and they are tagged "(pre-solve)".
        self.warnings = manufacturability_warnings_tagged(d, None)
            .into_iter()
            .map(|(tier, text)| (i32::try_from(tier).unwrap_or(-1), text))
            .collect();
        let failed_now = matches!(
            &app.solve,
            SolveState::Failed { generation, .. } if Some(*generation) == app.generation()
        );
        if failed_now && small_enough_to_resolve(app) {
            self.rows = tier_items(d, n_d);
            return;
        }
        let last = match &app.solve {
            SolveState::Solved { tiers, .. } => Some(tiers.as_slice()),
            _ => None,
        };
        let dirty: BTreeSet<usize> = if self.stale.all {
            (0..d.tiers.len()).collect()
        } else {
            self.stale.tiers.clone()
        };
        self.rows = tier_items_stale_with_last_solved(d, n_d, last, &dirty);
    }
}

/// Whether a design is small enough to re-solve on the page to name the tiers behind a
/// failed solve (the desktop's own "solve synchronously" rule).
fn small_enough_to_resolve(app: &WebApp) -> bool {
    let Some(design) = &app.design else {
        return false;
    };
    let last = match &app.solve {
        SolveState::Solved { took, .. } => Some(*took),
        _ => None,
    };
    should_solve_synchronously_for(SolveCostEstimate::of(&design.session.design), last)
}

/// One row as the Slint struct, with the multi-select flag applied.
fn row_data(row: &TierRow, multi_selected: bool) -> TierRowData {
    TierRowData {
        index: row.index,
        angle_deg: row.angle_deg.as_str().into(),
        angle_full: row.angle_full.as_str().into(),
        name: row.name.as_str().into(),
        indices: row.indices.as_str().into(),
        indices_full: row.indices_full.as_str().into(),
        constraint_kind: row.constraint_kind,
        constraint_text: row.constraint_text.as_str().into(),
        mast: row.mast.as_str().into(),
        mast_mm: row.mast_mm.as_str().into(),
        mast_full: row.mast_full.as_str().into(),
        strategy: row.strategy.as_str().into(),
        strategy_is_uncertain: row.strategy_is_uncertain,
        strategy_detail: row.strategy_detail.as_str().into(),
        needs_anchor: row.needs_anchor,
        imported_meet_text: row.imported_meet_text.as_str().into(),
        orbit_status: row.orbit_status.as_str().into(),
        orbit_incomplete: row.orbit_incomplete,
        is_detached: row.is_detached,
        block: row.block.as_str().into(),
        margin_text: row.margin_text.as_str().into(),
        risk_level: row.risk_level,
        meet_partners_text: row.meet_partners_text.as_str().into(),
        warning_text: row.warning_text.as_str().into(),
        multi_selected,
        proposed_angle: row.proposed_angle.as_str().into(),
    }
}

/// Brings `model` to `items` changing as few rows as possible, so the list's scroll
/// position and the rows' local state (an open inline editor) survive a refresh.
fn sync_model<T: Clone + PartialEq + 'static>(model: &VecModel<T>, items: Vec<T>) {
    let new_len = items.len();
    let existing = model.row_count();
    for (row, item) in items.into_iter().enumerate() {
        if row < existing {
            if model.row_data(row).as_ref() != Some(&item) {
                model.set_row_data(row, item);
            }
        } else {
            model.push(item);
        }
    }
    while model.row_count() > new_len {
        model.remove(model.row_count() - 1);
    }
}

/// What one [`sync`] pushes.
struct Push {
    /// `Some` when the rows or the multi-selection changed.
    rows: Option<Vec<TierRowData>>,
    selected: i32,
    multi_count: i32,
    status: Option<StatusPush>,
    warnings: Option<Vec<WarningLine>>,
}

struct StatusPush {
    kind: &'static str,
    text: String,
    undo_label: String,
    redo_label: String,
    auto_solve_budget_ms: u32,
}

/// The status line for the app's current state.
fn status_push(app: &WebApp, solving: bool) -> StatusPush {
    let (kind, text) = status_kind_and_text(app, solving);
    let (undo_label, redo_label) = app.design.as_ref().map_or_else(
        || (String::new(), String::new()),
        |design| {
            let history = &design.session.history;
            let d = &design.session.design;
            (
                history
                    .peek_undo()
                    .map(|e| e.describe(d))
                    .unwrap_or_default(),
                history
                    .peek_redo()
                    .map(|e| e.describe(d))
                    .unwrap_or_default(),
            )
        },
    );
    StatusPush {
        kind,
        text,
        undo_label,
        redo_label,
        auto_solve_budget_ms: app.auto_solve_budget_ms,
    }
}

fn status_kind_and_text(app: &WebApp, solving: bool) -> (&'static str, String) {
    let Some(design) = &app.design else {
        return ("none", String::new());
    };
    if design.session.design.tiers.is_empty() {
        return ("none", "Preform only -- add tiers.".to_string());
    }
    if solving {
        return ("solving", String::new());
    }
    let current = design.session.current_generation();
    match &app.solve {
        SolveState::Solved {
            generation,
            status,
            problem,
            took,
            ..
        } if *generation == current => (
            if *problem { "problem" } else { "solved" },
            format!("{status} (solved in {:.2} s)", took.as_secs_f32()),
        ),
        SolveState::Failed {
            generation,
            message,
        } if *generation == current => ("failed", message.clone()),
        _ => (
            "stale",
            if app.auto_solve_budget_ms > 0 {
                "Not solved since the last edit.".to_string()
            } else {
                "Not solved since the last edit. Auto-solve is off -- press Solve.".to_string()
            },
        ),
    }
}

/// Compares the app with what the table last showed and pushes what changed -- see the
/// module doc comment. Call with no `RefCell` borrow held.
pub fn sync(ctx: &Ctx) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let solving = is_solving(ctx);
    let (push, models) = {
        let app = ctx.state.borrow();
        TABLE.with(|cell| {
            let mut table = cell.borrow_mut();
            let now = table.snapshot(&app, solving);
            let before = table.pushed.clone();
            if before.as_ref() == Some(&now) {
                return (None, None);
            }
            let rows_changed = before.as_ref().is_none_or(|b| b.rows != now.rows);
            if rows_changed {
                // The design moved on without one of this module's edits telling us which
                // tiers (an undo or redo from the menu, a drag in the Solid view, a load):
                // no solved mast can be trusted.
                let moved_by_others = before
                    .as_ref()
                    .is_some_and(|b| b.rows.generation != now.rows.generation)
                    && table.stale_epoch == table.rebuilt_epoch;
                if moved_by_others {
                    table.stale.all = true;
                }
                table.rebuild(&app);
                table.rebuilt_epoch = table.stale_epoch;
            }
            let selection_changed =
                rows_changed || before.as_ref().is_none_or(|b| b.selection != now.selection);
            let status_changed = before.as_ref().is_none_or(|b| b.status != now.status);
            let push = Push {
                rows: selection_changed.then(|| {
                    table
                        .rows
                        .iter()
                        .map(|row| {
                            let index = usize::try_from(row.index).unwrap_or(usize::MAX);
                            row_data(row, now.selection.multi.contains(&index))
                        })
                        .collect()
                }),
                selected: app
                    .selected_tier
                    .and_then(|t| i32::try_from(t).ok())
                    .unwrap_or(-1),
                multi_count: i32::try_from(now.selection.multi.len()).unwrap_or(0),
                status: status_changed.then(|| status_push(&app, solving)),
                warnings: rows_changed.then(|| {
                    table
                        .warnings
                        .iter()
                        .map(|(tier, text)| WarningLine {
                            tier: *tier,
                            text: text.as_str().into(),
                        })
                        .collect()
                }),
            };
            table.pushed = Some(now);
            (
                Some(push),
                Some((table.row_model.clone(), table.warning_model.clone())),
            )
        })
    };
    let (Some(push), Some((row_model, warning_model))) = (push, models) else {
        return;
    };
    let model = ui.global::<TierTableModel>();
    if let (Some(items), Some(row_model)) = (push.rows, row_model) {
        sync_model(&row_model, items);
    }
    if let (Some(items), Some(warning_model)) = (push.warnings, warning_model) {
        warning_model.set_vec(items);
    }
    model.set_selected_index(push.selected);
    model.set_multi_count(push.multi_count);
    if let Some(status) = push.status {
        model.set_status_kind(status.kind.into());
        model.set_status_text(SharedString::from(status.text));
        model.set_undo_label(status.undo_label.into());
        model.set_redo_label(status.redo_label.into());
        model.set_auto_solve_budget_ms(i32::try_from(status.auto_solve_budget_ms).unwrap_or(0));
    }
    // Something about the design or its solve changed: the guide may have a goal reached
    // (the catch-all behind its explicit calls, e.g. after a drag in the Solid view).
    super::guide::check_progress(ctx);
}

/// Creates the models, wires the table's callbacks (selection, edits, nudges, orbit tools)
/// and starts the poll. Call once from [`super::wire`].
pub fn wire(ui: &AppWindow, ctx: &Ctx) {
    let row_model = Rc::new(VecModel::<TierRowData>::default());
    let warning_model = Rc::new(VecModel::<WarningLine>::default());
    let model = ui.global::<TierTableModel>();
    model.set_rows(ModelRc::from(row_model.clone()));
    model.set_warnings(ModelRc::from(warning_model.clone()));
    TABLE.with(|cell| {
        let mut table = cell.borrow_mut();
        table.row_model = Some(row_model);
        table.warning_model = Some(warning_model);
    });

    model.on_matches_filter(|haystack, filter| tier_matches_filter(&haystack, &filter));

    super::selection::wire(&model, ctx);
    super::edit::wire(&model, ctx);
    super::nudge::wire(&model, ctx);
    super::orbit_tools::wire(&model, ctx);

    let poll = ctx.clone();
    POLL.with(|timer| {
        timer.start(TimerMode::Repeated, POLL_INTERVAL, move || sync(&poll));
    });
}
