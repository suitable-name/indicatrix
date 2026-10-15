//! Pushes the Cut page: the instruction rows, the progress card and which empty state
//! applies, and keeps the row selection that drives the diagram.

use std::cell::{Cell, RefCell};

use indicatrix_mobile_core::{
    app_state::AppState,
    cutting::{self, CutRow},
    document,
};
use slint::{ComponentHandle, ModelRc, VecModel};

use super::diagram;
use crate::{AppWindow, CutModel, CutRowData};

thread_local! {
    /// The rows last given to the page, so an unchanged list is not rebuilt.
    static PUSHED: RefCell<Vec<CutRow>> = const { RefCell::new(Vec::new()) };
    /// The row the diagram highlights (a step position).
    static SELECTED: Cell<Option<usize>> = const { Cell::new(None) };
}

/// The page state a push settles on.
mod kind {
    /// There are instructions.
    pub(super) const READY: i32 = 0;
    /// The design has no tiers to cut.
    pub(super) const NO_TIERS: i32 = 1;
    /// The design has no current solve.
    pub(super) const NOT_SOLVED: i32 = 2;
}

/// Writes the Cut page's properties from `state`.
pub(super) fn push(window: &AppWindow, state: &AppState) {
    let model = window.global::<CutModel>();
    model.set_title(document::display_title(state).into());
    model.set_solving(state.solve.is_solving());

    let rows = cutting::instruction_rows(state);
    let summary = cutting::summary(state);
    let (Ok(rows), Ok(summary)) = (rows, summary) else {
        show_empty(&model, state);
        diagram::push(window, state, None);
        return;
    };

    model.set_state_kind(kind::READY);
    model.set_state_text("".into());
    model.set_progress_text(summary.text.as_str().into());
    model.set_progress(summary.fraction);
    model.set_done_count(i32::try_from(summary.done).unwrap_or(i32::MAX));
    model.set_total_count(i32::try_from(summary.total).unwrap_or(i32::MAX));

    let selected = SELECTED.get().filter(|step| *step < rows.len());
    SELECTED.set(selected);
    model.set_selected_row(selected.map_or(-1, |step| i32::try_from(step).unwrap_or(-1)));
    let tier = selected
        .and_then(|step| rows.get(step))
        .and_then(|row| row.tier_index);

    let unchanged = PUSHED.with(|pushed| *pushed.borrow() == rows);
    if !unchanged {
        let data: Vec<CutRowData> = rows.iter().map(row_data).collect();
        model.set_rows(ModelRc::new(VecModel::from(data)));
        PUSHED.with(|pushed| *pushed.borrow_mut() = rows);
    }
    diagram::push(window, state, tier);
}

/// Highlights `step`'s tier in the diagram (a tap on a row of a wide window). Selecting
/// the selected row again clears the selection.
pub(super) fn select(window: &AppWindow, state: &AppState, step: i32) {
    let step = usize::try_from(step).ok();
    let next = if step.is_some() && step == SELECTED.get() {
        None
    } else {
        step
    };
    SELECTED.set(next);
    push(window, state);
}

/// The empty-state properties: no tiers, or not solved (with the reason when it failed).
fn show_empty(model: &CutModel<'_>, state: &AppState) {
    PUSHED.with(|pushed| pushed.borrow_mut().clear());
    model.set_rows(ModelRc::new(VecModel::from(Vec::<CutRowData>::new())));
    model.set_selected_row(-1);
    model.set_done_count(0);
    model.set_total_count(0);
    model.set_progress(0.0);
    model.set_progress_text("".into());
    if state.session.design.tiers.is_empty() {
        model.set_state_kind(kind::NO_TIERS);
        model.set_state_text("".into());
        return;
    }
    model.set_state_kind(kind::NOT_SOLVED);
    let generation = state.session.current_generation();
    let text = if state.solve.is_solving() {
        "The design is being solved. The instructions appear when it is done.".to_owned()
    } else if let Some(message) = state.solve.failure(generation) {
        format!("The design could not be solved: {message}")
    } else {
        format!("{}.", cutting::NotSolved)
    };
    model.set_state_text(text.into());
}

/// One list row for the page.
fn row_data(row: &CutRow) -> CutRowData {
    CutRowData {
        step: i32::try_from(row.step).unwrap_or(i32::MAX),
        code: row.code.as_str().into(),
        angle: row.angle_text.as_str().into(),
        index_text: row.index_text.as_str().into(),
        name: row.name.as_str().into(),
        notes: row.notes.as_str().into(),
        done: row.done,
        changed: row.changed,
        section: row.section.into(),
        section_start: row.section_start,
        tier: row
            .tier_index
            .map_or(-1, |tier| i32::try_from(tier).unwrap_or(-1)),
    }
}
