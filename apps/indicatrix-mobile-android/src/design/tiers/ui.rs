//! What the Tiers tab remembers on the UI thread between callbacks: the Slint models,
//! the rows last pushed, which sheets are open for which tier, and the keypad drafts.

use std::{cell::RefCell, collections::BTreeSet, rc::Rc};

use indicatrix_mobile_core::design_state::{KeypadState, MobileTierRow};
use slint::{Model, VecModel};

use crate::{AnglePresetData, TierChipData, TierRowData};

/// What the rows pushed last were built from; equal keys mean the rows are still right.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RowsKey {
    pub(super) generation: u64,
    pub(super) solve_revision: u64,
    pub(super) selected: Option<usize>,
    pub(super) selected_concave: Option<usize>,
    pub(super) multi: BTreeSet<usize>,
}

/// A question the user was asked and the action a "yes" repeats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Pending {
    /// Remove this tier although others still meet it by name.
    RemoveTier(usize),
    /// Remove the multi-selection although other tiers still meet some of it.
    RemoveMulti,
}

/// The Tiers tab's UI-thread state. See the module docs.
#[derive(Default)]
pub(super) struct TiersUi {
    /// The model behind `TiersModel.rows`.
    pub(super) rows_model: Rc<VecModel<TierRowData>>,
    /// The model behind `TiersModel.tier-chips`.
    pub(super) chips_model: Rc<VecModel<TierChipData>>,
    /// The model behind `TiersModel.angle-presets`.
    pub(super) presets_model: Rc<VecModel<AnglePresetData>>,
    /// The rows as last pushed, in model order (flat tiers, then concave).
    pub(super) rows: Vec<MobileTierRow>,
    /// What [`Self::rows`] were built from.
    pub(super) rows_key: Option<RowsKey>,
    /// The tier whose angle sheet is open.
    pub(super) angle: Option<usize>,
    /// The angle keypad's draft.
    pub(super) angle_keypad: KeypadState,
    /// The refusal the angle sheet shows, empty for none.
    pub(super) angle_error: String,
    /// The angles (degrees, magnitudes) of the presets shown, in order.
    pub(super) preset_values: Vec<f64>,
    /// The tier whose tier sheet is open.
    pub(super) sheet: Option<usize>,
    /// The index keypad's draft.
    pub(super) index_keypad: KeypadState,
    /// The question waiting for its answer.
    pub(super) pending: Option<Pending>,
}

thread_local! {
    static UI: RefCell<TiersUi> = RefCell::new(TiersUi::default());
}

/// Runs `f` on the tab's UI state. Do not call Slint callbacks from inside `f`.
pub(super) fn with<R>(f: impl FnOnce(&mut TiersUi) -> R) -> R {
    UI.with(|ui| f(&mut ui.borrow_mut()))
}

/// The flat tier a model row stands for; `None` for a concave row or a row that is not
/// there.
pub(super) fn flat_tier(row: i32) -> Option<usize> {
    let row = usize::try_from(row).ok()?;
    with(|ui| {
        ui.rows
            .get(row)
            .filter(|found| !found.is_concave)
            .map(|found| found.index)
    })
}

/// Brings `model` to `items`, changing as few rows as possible so a list keeps its scroll
/// position and rows that did not change are not re-laid-out.
pub(super) fn sync_model<T: Clone + PartialEq + 'static>(model: &VecModel<T>, items: Vec<T>) {
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

/// A display value narrowed to the `f32` Slint properties carry.
#[expect(
    clippy::cast_possible_truncation,
    reason = "a value shown on a screen: f32 precision is plenty"
)]
pub(super) const fn narrow(value: f64) -> f32 {
    value as f32
}

/// `value` rounded to two decimals.
pub(super) fn round_hundredths(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}
