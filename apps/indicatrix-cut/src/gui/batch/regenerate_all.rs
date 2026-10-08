//! The Library menu's whole-catalogue actions: regenerate every design's preview
//! images, every design's tilt curves, or both.
//!
//! Each one opens the same confirm step (`preview_batch_dialog.slint`/
//! `tilt_batch_dialog.slint`) every other trigger of those batches uses, worded for
//! regeneration ([`offer_preview_regeneration`]/[`offer_tilt_regeneration`]), so this
//! adds no second copy of either batch.
//!
//! A regeneration step asks which designs: "Missing or outdated only" (the default; the
//! same vault scan the startup offer and the filter panel's "Compute missing tilt
//! curves" run, counted off the UI thread and narrowed to the offered set by
//! [`missing_among`]) or "All designs". [`chosen_ids`] picks the set the batch starts.
//!
//! "Both" runs them one after the other rather than together: the two confirm steps
//! would otherwise sit on top of each other. It offers the previews first and holds the
//! tilt ids in [`PENDING_TILT`]; [`preview_dialog_closed`] (called when the preview
//! dialog closes, after a finished batch or a declined offer) then offers the tilt
//! curves. That also holds after a CANCELLED preview batch: Close on its
//! "(cancelled)" summary still leads to the tilt offer. If a tilt batch is already
//! running by then (started from a card menu meanwhile) the offer is skipped with a
//! toast, so it cannot flip the running dialog back to its confirm view. Starting any
//! other Library regeneration drops a held tilt offer ([`clear_pending_tilt`]).

use crate::{
    BatchModel, LibraryModel, MainWindow,
    bridge::library::source::LibrarySource,
    gui::{
        batch::{preview, tilt},
        show_toast,
    },
};
use indicatrix_vault::db::sqlite::Database;
use slint::ComponentHandle;
use std::{
    cell::{Cell, RefCell},
    collections::HashSet,
    sync::{Arc, Mutex, PoisonError},
};

thread_local! {
    /// The tilt ids "Regenerate Both" offers once the preview dialog closes.
    /// UI-thread only: every reader and writer is a Slint callback.
    static PENDING_TILT: RefCell<Option<Vec<i64>>> = const { RefCell::new(None) };
}

/// Which regeneration confirm step a scope scan belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::gui) enum ScopeKind {
    Preview,
    Tilt,
}

thread_local! {
    /// One counter per [`ScopeKind`], bumped each time that kind's regenerate dialog
    /// opens, so a slow "missing" scan that lands after a newer dialog opened is dropped.
    /// UI-thread only: the bump and the check both run in UI callbacks.
    static SCOPE_GENERATION: [Cell<u64>; 2] = const { [Cell::new(0), Cell::new(0)] };
}

/// Starts a new scope scan for `kind` and returns its number for [`scope_is_current`].
pub(in crate::gui) fn next_scope_generation(kind: ScopeKind) -> u64 {
    SCOPE_GENERATION.with(|gens| {
        let cell = &gens[kind as usize];
        cell.set(cell.get() + 1);
        cell.get()
    })
}

/// Whether `generation` is still the newest scope scan of `kind`.
pub(in crate::gui) fn scope_is_current(kind: ScopeKind, generation: u64) -> bool {
    SCOPE_GENERATION.with(|gens| gens[kind as usize].get() == generation)
}

/// The designs of `ids` that `missing` (a whole-catalogue scan result) also holds, in
/// `ids` order: "missing or outdated" restricted to the set the dialog was opened for
/// (the whole library, or the filter panel's filtered set).
pub(in crate::gui) fn missing_among(ids: &[i64], missing: &[i64]) -> Vec<i64> {
    let missing: HashSet<i64> = missing.iter().copied().collect();
    ids.iter()
        .copied()
        .filter(|id| missing.contains(id))
        .collect()
}

/// The ids a regenerate confirm step starts: only the missing ones when the offer is a
/// regeneration and its "missing or outdated only" pill is chosen, otherwise every id
/// the offer holds (the startup, import and filter-panel offers hold the exact set to
/// run).
pub(in crate::gui) fn chosen_ids(
    regenerate: bool,
    missing_only: bool,
    all: Vec<i64>,
    missing: Vec<i64>,
) -> Vec<i64> {
    if regenerate && missing_only {
        missing
    } else {
        all
    }
}

/// `design` / `designs` for a count.
fn designs(count: usize) -> String {
    if count == 1 {
        "1 design".to_string()
    } else {
        format!("{count} designs")
    }
}

/// The "missing or outdated only" pill's text: `None` while the scan is still counting.
pub(in crate::gui) fn missing_choice_label(count: Option<usize>) -> String {
    match count {
        None => "Missing or outdated only (counting...)".to_string(),
        Some(0) => "All designs are up to date".to_string(),
        Some(n) => format!("Missing or outdated only ({})", designs(n)),
    }
}

/// The "all" pill's text.
pub(in crate::gui) fn all_choice_label(count: usize) -> String {
    format!("All designs ({count})")
}

/// Opens the preview confirm step as a regeneration: the designs may already have
/// previews, which the batch replaces. The dialog offers "missing or outdated only"
/// (counted off the UI thread, "Counting..." until it lands) or "all".
pub fn offer_preview_regeneration(ui: &MainWindow, ids: &[i64]) {
    preview::offer_regeneration(ui, ids);
}

/// [`offer_preview_regeneration`] for the tilt-curve confirm step.
pub fn offer_tilt_regeneration(ui: &MainWindow, ids: &[i64]) {
    tilt::offer_regeneration(ui, ids);
}

/// The preview dialog closed (a finished batch's Close, or "Not now" on its offer):
/// offers the tilt curves a "Regenerate Both" is still holding, if any.
///
/// Also runs after a cancelled preview batch (its Close). When a tilt batch is already
/// running, the held ids are dropped and a toast says so instead of offering: the
/// offer would replace the running dialog's progress view with a confirm step.
pub fn preview_dialog_closed(ui: &MainWindow) {
    if let Some(ids) = PENDING_TILT.with(|pending| pending.borrow_mut().take()) {
        if ui.global::<BatchModel>().get_tilt_batch_running() {
            show_toast(
                ui,
                "A tilt-curve batch is already running -- the tilt half of \"Regenerate Both\" was skipped.",
                "info",
            );
            return;
        }
        offer_tilt_regeneration(ui, &ids);
    }
}

/// Drops the tilt ids a "Regenerate Both" is holding, so a plain Library action started
/// afterwards never inherits its tilt tail.
fn clear_pending_tilt() {
    PENDING_TILT.with(|pending| *pending.borrow_mut() = None);
}

/// Every design in the local catalogue, or `None` after a toast saying why not: a
/// remote-browsed library (whose ids this database does not hold), an empty
/// catalogue, or a failed query.
pub(in crate::gui) fn all_ids_or_toast(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
) -> Option<Vec<i64>> {
    if source
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .is_remote()
    {
        show_toast(
            ui,
            "Switch to the local library to regenerate its designs.",
            "error",
        );
        return None;
    }
    let ids = db
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .all_entry_ids();
    match ids {
        Ok(ids) if ids.is_empty() => {
            show_toast(ui, "The library has no designs yet.", "info");
            None
        }
        Ok(ids) => Some(ids),
        Err(e) => {
            show_toast(ui, &format!("Could not list the library: {e}"), "error");
            None
        }
    }
}

/// Wires the Library menu's three `LibraryModel.regenerate_all_*` callbacks.
///
/// Called once, from `gui::build_main_window`.
pub(in crate::gui) fn setup_regenerate_all_callbacks(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
) {
    let (db_p, source_p, weak_p) = (Arc::clone(db), Arc::clone(source), ui.as_weak());
    ui.global::<LibraryModel>()
        .on_regenerate_all_previews(move || {
            clear_pending_tilt();
            if let Some(ui) = weak_p.upgrade()
                && let Some(ids) = all_ids_or_toast(&ui, &db_p, &source_p)
            {
                offer_preview_regeneration(&ui, &ids);
            }
        });

    let (db_t, source_t, weak_t) = (Arc::clone(db), Arc::clone(source), ui.as_weak());
    ui.global::<LibraryModel>()
        .on_regenerate_all_tilt_curves(move || {
            clear_pending_tilt();
            if let Some(ui) = weak_t.upgrade()
                && let Some(ids) = all_ids_or_toast(&ui, &db_t, &source_t)
            {
                offer_tilt_regeneration(&ui, &ids);
            }
        });

    let (db_b, source_b, weak_b) = (Arc::clone(db), Arc::clone(source), ui.as_weak());
    ui.global::<LibraryModel>()
        .on_regenerate_all_previews_and_tilt_curves(move || {
            if let Some(ui) = weak_b.upgrade()
                && let Some(ids) = all_ids_or_toast(&ui, &db_b, &source_b)
            {
                PENDING_TILT.with(|pending| *pending.borrow_mut() = Some(ids.clone()));
                offer_preview_regeneration(&ui, &ids);
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_among_keeps_only_the_offered_ids_in_offer_order() {
        // `all_entry_ids` already leaves ignored designs out, and so does the vault's
        // missing scan, so an ignored design (9, 7 here) can reach neither side.
        let all = [1, 2, 3, 5, 8];
        let missing = [8, 3, 7, 9];
        assert_eq!(missing_among(&all, &missing), vec![3, 8]);
        assert!(missing_among(&all, &[]).is_empty());
        assert!(missing_among(&[], &missing).is_empty());
    }

    #[test]
    fn chosen_ids_picks_the_missing_set_only_for_a_regeneration_that_asked_for_it() {
        let all = vec![1, 2, 3];
        let missing = vec![2];
        assert_eq!(
            chosen_ids(true, true, all.clone(), missing.clone()),
            vec![2]
        );
        assert_eq!(chosen_ids(true, false, all.clone(), missing.clone()), all);
        // The startup, import and filter-panel offers hold the exact set to run.
        assert_eq!(chosen_ids(false, true, all.clone(), missing), all);
    }

    #[test]
    fn choice_labels_count_and_pluralise() {
        assert_eq!(
            missing_choice_label(None),
            "Missing or outdated only (counting...)"
        );
        assert_eq!(missing_choice_label(Some(0)), "All designs are up to date");
        assert_eq!(
            missing_choice_label(Some(1)),
            "Missing or outdated only (1 design)"
        );
        assert_eq!(
            missing_choice_label(Some(12)),
            "Missing or outdated only (12 designs)"
        );
        assert_eq!(all_choice_label(3299), "All designs (3299)");
    }

    #[test]
    fn scope_generation_drops_a_scan_that_a_newer_dialog_overtook() {
        let first = next_scope_generation(ScopeKind::Preview);
        let second = next_scope_generation(ScopeKind::Preview);
        assert!(!scope_is_current(ScopeKind::Preview, first));
        assert!(scope_is_current(ScopeKind::Preview, second));
        // The tilt dialog counts on its own.
        let tilt = next_scope_generation(ScopeKind::Tilt);
        assert!(scope_is_current(ScopeKind::Tilt, tilt));
        assert!(scope_is_current(ScopeKind::Preview, second));
    }
}
