//! Background design counting for the rough planner dialog.

use super::{
    format::{group_thousands, to_i32},
    inputs::{FilterSnapshot, sorted_unique},
    session::{CachedIds, REMOTE_MESSAGE, Session},
};
use crate::{
    MainWindow, RoughPlanModel, RoughPlannerWindow, bridge::library::source::LibrarySource,
};
use indicatrix_vault::db::sqlite::Database;
use slint::{ComponentHandle, Weak};
use std::{
    cell::RefCell,
    collections::BTreeSet,
    sync::{Arc, Mutex, PoisonError},
};
use tracing::warn;

/// Whether the active library is a remote one (which the planner cannot use).
pub fn is_remote(source: &Mutex<LibrarySource>) -> bool {
    source
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .is_remote()
}

/// The text the window shows for a design count: the number with thousands separators,
/// or "counting..." for the negative value that means the count is still being taken.
fn count_text(count: i32) -> String {
    usize::try_from(count).map_or_else(|_| "counting...".to_string(), group_thousands)
}

/// Shows the two design counts: the numbers and their grouped texts together, so the
/// window never pairs a number with the text of another. A negative count means "still
/// counting".
fn show_counts(model: &RoughPlanModel<'_>, filter: i32, library: i32) {
    model.set_filter_count(filter);
    model.set_filter_count_text(count_text(filter).into());
    model.set_library_count(library);
    model.set_library_count_text(count_text(library).into());
}

/// How many of `ids` a plan would use: those that are not in `excluded`, the designs
/// excluded from the planner.
pub(super) fn plannable_count(ids: &[i64], excluded: &BTreeSet<i64>) -> usize {
    ids.iter().filter(|id| !excluded.contains(id)).count()
}

/// The background half of the open-time counts: runs the filter query and lists the
/// whole library off the UI thread, caches both id lists (unless a newer open
/// superseded this one) and pushes the two counts to the planner window. The counts leave
/// out the designs excluded from the planner, so they say what Plan will use; the cached
/// lists keep them. Quiet: no toasts.
pub fn spawn_count_query(
    ui_weak: Weak<RoughPlannerWindow>,
    db: Arc<Mutex<Database>>,
    snapshot: FilterSnapshot,
    cache: Arc<Mutex<CachedIds>>,
    generation: u64,
) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name("rough-plan-count".to_string())
        .spawn(move || {
            let filter = snapshot
                .query(&db)
                .inspect_err(|e| warn!("Rough planner: could not count the filtered designs: {e}"))
                .ok();
            let (library, excluded) = {
                let guard = db.lock().unwrap_or_else(PoisonError::into_inner);
                (
                    guard
                        .all_entry_ids()
                        .inspect_err(|e| warn!("Rough planner: could not count the library: {e}"))
                        .ok()
                        .map(sorted_unique),
                    guard
                        .planner_excluded_ids()
                        .inspect_err(|e| {
                            warn!("Rough planner: could not read the excluded designs: {e}");
                        })
                        .unwrap_or_default(),
                )
            };
            let counts = (
                filter
                    .as_deref()
                    .map_or(0, |ids| plannable_count(ids, &excluded)),
                library
                    .as_deref()
                    .map_or(0, |ids| plannable_count(ids, &excluded)),
            );
            {
                let mut cached = cache.lock().unwrap_or_else(PoisonError::into_inner);
                if cached.generation != generation {
                    return;
                }
                cached.filter = filter;
                cached.library = library;
            }
            let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                // A newer open may have started since: then these counts are stale.
                let current = cache
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .generation;
                if current == generation {
                    show_counts(
                        &ui.global::<RoughPlanModel>(),
                        to_i32(counts.0),
                        to_i32(counts.1),
                    );
                }
            });
        })
        .map(|_handle| ())
}

/// Makes the window's error line agree with the library source: a remote library shows
/// [`REMOTE_MESSAGE`] (replacing whatever was there), a local one clears it again when
/// that is what the line says and leaves any other message alone.
pub fn sync_remote_message(model: &RoughPlanModel<'_>, remote: bool) {
    if remote {
        model.set_error_text(REMOTE_MESSAGE.into());
    } else if model.get_error_text().as_str() == REMOTE_MESSAGE {
        model.set_error_text("".into());
    }
}

/// Starts the two design counts for the planner window. A remote library gets the red
/// "switch" line and zero counts; a local one has its counts filled in a moment later by
/// [`spawn_count_query`], so the UI thread never waits on the database. The library
/// filter is read off the main window and remembered, so a later change of it can be
/// noticed.
///
/// The counts are a display hint: they leave out the designs excluded from the planner,
/// as the plan does. The candidate ids a plan uses are resolved again when Plan is
/// pressed, so a count that is out of date never changes what gets planned.
pub fn refresh_counts(
    main: &MainWindow,
    planner: &RoughPlannerWindow,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
    session: &RefCell<Session>,
) {
    let model = planner.global::<RoughPlanModel>();
    let cache = Arc::clone(&session.borrow().ids);
    let generation = cache.lock().unwrap_or_else(PoisonError::into_inner).reset();
    if is_remote(source) {
        show_counts(&model, 0, 0);
        sync_remote_message(&model, true);
        session.borrow_mut().counted_filter = None;
        return;
    }
    sync_remote_message(&model, false);
    // -1 = "still counting", so a reopened dialog never shows the previous counts.
    show_counts(&model, -1, -1);
    let snapshot = FilterSnapshot::read(main);
    session.borrow_mut().counted_filter = Some(snapshot.clone());
    if let Err(e) = spawn_count_query(
        planner.as_weak(),
        Arc::clone(db),
        snapshot,
        cache,
        generation,
    ) {
        warn!("Rough planner: could not start the counting thread: {e}");
        show_counts(&model, 0, 0);
    }
}

/// Asks for fresh counts when the main window's library filter differs from the one the
/// shown counts were asked for. Does nothing for a remote library or while a plan runs
/// (the run has its candidate designs already). Cheap when nothing changed: it only reads
/// the filter controls.
pub fn refresh_counts_if_filter_changed(
    main: &MainWindow,
    planner: &RoughPlannerWindow,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
    session: &RefCell<Session>,
) {
    if planner.global::<RoughPlanModel>().get_running() || is_remote(source) {
        return;
    }
    let now = FilterSnapshot::read(main);
    if session.borrow().counted_filter.as_ref() != Some(&now) {
        refresh_counts(main, planner, db, source, session);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Expectations are the digits grouped by three from the right, written by hand; a
    /// negative count is the "still counting" marker.
    #[test]
    fn a_count_reads_with_thousands_separators_and_a_marker_while_counting() {
        assert_eq!(count_text(0), "0");
        assert_eq!(count_text(1), "1");
        assert_eq!(count_text(999), "999");
        assert_eq!(count_text(1_234), "1,234");
        assert_eq!(count_text(1_234_567), "1,234,567");
        assert_eq!(count_text(-1), "counting...");
    }

    #[test]
    fn the_plannable_count_leaves_out_the_excluded_designs() {
        let ids = [2, 4, 6, 8];
        // Nothing excluded: every id counts.
        assert_eq!(plannable_count(&ids, &BTreeSet::new()), 4);
        // Some excluded: only those present in the list are taken off.
        assert_eq!(plannable_count(&ids, &BTreeSet::from([4, 8])), 2);
        // Ids that are not in the list change nothing.
        assert_eq!(plannable_count(&ids, &BTreeSet::from([1, 3, 99])), 4);
        // All excluded, and a longer excluded set than the list.
        assert_eq!(plannable_count(&ids, &BTreeSet::from([2, 4, 6, 8])), 0);
        assert_eq!(
            plannable_count(&ids, &BTreeSet::from([1, 2, 4, 6, 8, 9])),
            0
        );
        // An empty list has nothing to count.
        assert_eq!(plannable_count(&[], &BTreeSet::from([1])), 0);
    }
}
