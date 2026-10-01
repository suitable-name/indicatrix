//! Saving the results on screen as a stored plan.
//!
//! What is saved comes from the snapshot the results were planned for (model, settings,
//! material), never from the form or the live-edited model. The write itself, including
//! the shapes of the used designs, runs on a worker thread.

use super::{
    LoadedFingerprints, announce,
    convert::CandidateSource,
    dto::{DesignShape, SavedDesignDto},
    format::{RankedLayout, SerializeInput, design_shape, serialize_checked_plan},
    naming::{clean_plan_name, current_unix_time, default_plan_name},
    show_error, spawn_task, store,
};
use crate::{
    RoughPlanModel,
    gui::rough_plan::{
        editing,
        host::Host,
        run::{ResultsSource, RunState},
        session::Session,
    },
};
use indicatrix_cut_core::rough_plan::{PlanSettings, RoughLayout, RoughModel};
use indicatrix_vault::db::sqlite::Database;
use slint::ComponentHandle;
use std::{
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
    sync::{Mutex, PoisonError},
};

/// Whose entry ids a saved plan carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LibraryId {
    /// This library's: results planned here, every id is one of its designs.
    Local,
    /// The library a loaded plan came from (none if the file named none): the ids of
    /// designs that were not found here are still that library's.
    Kept(Option<u32>),
}

/// Everything a save needs, copied off the session so the write can leave the UI thread.
pub(super) struct Snapshot {
    /// The model the layouts were planned for.
    model: RoughModel,
    /// The settings the layouts were planned with.
    settings: PlanSettings,
    /// The plan's material.
    material_name: String,
    /// The plan's weighed carat, if one was entered.
    weighed_ct: Option<f64>,
    /// Where the plan's candidate designs came from.
    candidate_source: CandidateSource,
    /// The layouts to save with their rank (1-based) in the plan they came from.
    layouts: Vec<(usize, RoughLayout)>,
    /// The design titles of every entry id in `layouts`.
    titles: BTreeMap<i64, String>,
    /// The shapes the designs had when the layouts were planned (a loaded plan: what its
    /// file came with); they win over today's measurements.
    stored_shapes: BTreeMap<i64, DesignShape>,
    /// Whose ids the plan carries.
    library: LibraryId,
}

/// The result rows a save covers: the ticked ones for "save selected" (all of them when
/// none is ticked), every row otherwise.
fn rows_to_save(row_count: usize, keep: &[bool], selected_only: bool) -> Vec<usize> {
    let ticked: Vec<usize> = (0..row_count)
        .filter(|&row| keep.get(row).copied().unwrap_or(false))
        .collect();
    if selected_only && !ticked.is_empty() {
        ticked
    } else {
        (0..row_count).collect()
    }
}

/// The shapes a save stores for the designs of `run`'s layouts, and whose entry ids they
/// are.
///
/// A loaded plan that is still the one on screen (`loaded` names its stored plan) keeps
/// the shapes its file came with and the library its file came from; every other result
/// keeps the shapes it was planned with (`run.shapes`) in this library. Neither reads
/// today's measurements, so a design that was rescaled since planning is not re-measured
/// into the file.
pub(super) fn stored_shapes_and_library(
    run: &RunState,
    loaded: Option<&LoadedFingerprints>,
) -> (BTreeMap<i64, DesignShape>, LibraryId) {
    match (&run.source, loaded) {
        (ResultsSource::Loaded { plan_id, .. }, Some(loaded)) if loaded.plan_id == *plan_id => {
            (loaded.shapes.clone(), LibraryId::Kept(loaded.library_id))
        }
        _ => (run.shapes.clone(), LibraryId::Local),
    }
}

/// Copies what a save writes off the session.
fn snapshot(session: &Session, selected_only: bool) -> Result<Snapshot, String> {
    snapshot_of(&session.run, session.saved.loaded.as_ref(), selected_only)
}

/// [`snapshot`] from the parts of the session it reads: the results on screen and the
/// stored shapes of the loaded plan, if one is shown.
pub(super) fn snapshot_of(
    run: &RunState,
    loaded: Option<&LoadedFingerprints>,
    selected_only: bool,
) -> Result<Snapshot, String> {
    if run.layouts.is_empty() {
        return Err("There are no results to save. Plan first.".to_string());
    }
    let (Some(model), Some(settings)) = (&run.plan_model, &run.plan_settings) else {
        return Err("The results on screen carry no model to save.".to_string());
    };
    let layouts = rows_to_save(run.layouts.len(), &run.keep, selected_only)
        .into_iter()
        .filter_map(|row| Some((row + 1, run.layouts.get(row)?.clone())))
        .collect();
    let (stored_shapes, library) = stored_shapes_and_library(run, loaded);
    Ok(Snapshot {
        model: model.clone(),
        settings: *settings,
        material_name: run.material_name.clone(),
        weighed_ct: run.weighed_ct,
        candidate_source: run.candidate_source,
        layouts,
        titles: run.titles.clone(),
        stored_shapes,
        library,
    })
}

/// `RoughPlanModel.begin_save`: opens the name row with the default name, and closes the
/// saved list that would cover it.
pub(super) fn begin_save(host: &Rc<Host>, selected_only: bool) {
    let model = host.window.global::<RoughPlanModel>();
    if model.get_running() {
        show_error(host, "Wait for the plan to finish before saving.");
        return;
    }
    // A field still being typed is committed first; an invalid one says why.
    if let Err(message) = editing::commit_pending_base(host) {
        show_error(host, &message);
        return;
    }
    let name = {
        let mut session = host.session.borrow_mut();
        if let Err(message) = snapshot(&session, selected_only) {
            drop(session);
            show_error(host, &message);
            return;
        }
        session.saved.selected_only = selected_only;
        let planned = session.run.plan_model.as_ref().unwrap_or(&session.model);
        default_plan_name(planned, &session.run.material_name, current_unix_time())
    };
    show_error(host, "");
    model.set_save_name(name.into());
    model.set_saved_open(false);
    model.set_save_name_open(true);
}

/// The designs a save stores: title and shape of every design the layouts use.
fn design_records(snapshot: &Snapshot, database: &Database) -> Result<Vec<SavedDesignDto>, String> {
    let ids: Vec<i64> = snapshot
        .layouts
        .iter()
        .flat_map(|(_, layout)| layout.stones.iter().map(|stone| stone.entry_id))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let extents = database
        .solid_extents_for(&ids)
        .map_err(|e| format!("Could not read the design measurements: {e}"))?;
    Ok(ids
        .into_iter()
        .map(|entry_id| {
            let shape = snapshot
                .stored_shapes
                .get(&entry_id)
                .copied()
                .or_else(|| {
                    extents
                        .get(&entry_id)
                        .and_then(|row| row.extents)
                        .map(|found| design_shape(&found))
                })
                .unwrap_or_default();
            SavedDesignDto {
                entry_id,
                title: snapshot
                    .titles
                    .get(&entry_id)
                    .cloned()
                    .unwrap_or_else(|| format!("Design {entry_id}")),
                fingerprint: shape.fingerprint,
                width_caliper: shape.width_caliper,
                extents_version: shape.extents_version,
            }
        })
        .collect())
}

/// Serialises the snapshot, proves the text reads back, and stores it (worker thread).
pub(super) fn write_plan(
    db: &Mutex<Database>,
    name: &str,
    snapshot: &Snapshot,
) -> Result<(), String> {
    let library_id = match snapshot.library {
        LibraryId::Local => store::library_stamp(db),
        LibraryId::Kept(id) => id,
    };
    let designs = {
        let database = db.lock().unwrap_or_else(PoisonError::into_inner);
        design_records(snapshot, &database)?
    };
    let ranked: Vec<RankedLayout<'_>> = snapshot
        .layouts
        .iter()
        .map(|(rank, layout)| RankedLayout {
            rank: *rank,
            layout,
        })
        .collect();
    let text = serialize_checked_plan(&SerializeInput {
        name,
        created_at: current_unix_time(),
        library_id,
        model: &snapshot.model,
        material_name: &snapshot.material_name,
        weighed_ct: snapshot.weighed_ct,
        settings: &snapshot.settings,
        candidate_source: snapshot.candidate_source,
        designs: &designs,
        layouts: &ranked,
    })?;
    store::save_plan(db, name, &text).map(|_id| ())
}

/// The confirmation line after a save.
fn saved_message(count: usize, name: &str) -> String {
    format!(
        "Saved {count} result{} as \"{name}\".",
        if count == 1 { "" } else { "s" }
    )
}

/// A save that covered `saved` of the `rows` results that were on screen when it started
/// makes them "saved" (what replacing them asks about) when it covered every one, and the
/// same results are still on screen (no plan is running, the row count is the same).
fn mark_saved(host: &Host, saved: usize, rows: usize) {
    let model = host.window.global::<RoughPlanModel>();
    let mut session = host.session.borrow_mut();
    if saved == rows && !model.get_running() && session.run.layouts.len() == rows {
        session.run.results_unsaved = false;
        model.set_results_unsaved(false);
    }
}

/// `RoughPlanModel.confirm_save`: stores the plan under the name in the name row.
pub(super) fn confirm_save(host: &Rc<Host>) {
    let model = host.window.global::<RoughPlanModel>();
    let name = clean_plan_name(&model.get_save_name());
    if name.is_empty() {
        show_error(host, "Enter a name for the saved plan.");
        return;
    }
    let selected_only = host.session.borrow().saved.selected_only;
    let taken = {
        let session = host.session.borrow();
        snapshot(&session, selected_only).map(|taken| (taken, session.run.layouts.len()))
    };
    let (taken, all_rows) = match taken {
        Ok(taken) => taken,
        Err(message) => {
            show_error(host, &message);
            return;
        }
    };
    show_error(host, "");
    model.set_save_name_open(false);
    let count = taken.layouts.len();
    let task_name = name.clone();
    spawn_task(
        host,
        move |db| write_plan(db, &task_name, &taken),
        move |host, result| match result {
            Ok(()) => {
                mark_saved(host, count, all_rows);
                announce(host, &saved_message(count, &name));
                super::list::refresh_list(host);
            }
            Err(message) => {
                show_error(host, &message);
                // Give the name back so the user can retry.
                let model = host.window.global::<RoughPlanModel>();
                model.set_save_name(name.into());
                model.set_save_name_open(true);
            }
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_selected_covers_the_ticked_rows_or_everything_when_none_is_ticked() {
        let keep = [false, true, false, true];
        assert_eq!(rows_to_save(4, &keep, true), vec![1, 3]);
        assert_eq!(rows_to_save(4, &keep, false), vec![0, 1, 2, 3]);
        assert_eq!(rows_to_save(3, &[false; 3], true), vec![0, 1, 2]);
        // A keep list shorter than the rows counts the missing ones as not ticked.
        assert_eq!(rows_to_save(3, &[false, true], true), vec![1]);
        assert_eq!(rows_to_save(0, &[], true), Vec::<usize>::new());
    }

    #[test]
    fn the_confirmation_counts_the_results() {
        assert_eq!(saved_message(1, "A"), "Saved 1 result as \"A\".");
        assert_eq!(saved_message(3, "B"), "Saved 3 results as \"B\".");
    }
}
