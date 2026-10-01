//! The Plan, Cancel and keep-toggle buttons: validating the inputs, choosing the
//! candidate id source, asking before unsaved results are replaced and handing the work
//! to the background thread.

use super::{CANCELLING_STAGE, IdSource, PlanJob, spawn_worker, state::Screen};
use crate::{
    RoughPlanModel, RoughPlanResult,
    gui::rough_plan::{
        base::is_blank,
        counts::{is_remote, refresh_counts},
        editing,
        format::to_i32,
        host::Host,
        inputs::{
            BAD_CUT_FIELD_MESSAGE, FilterSnapshot, NO_SIZE_MESSAGE, check_skin, current_settings,
        },
        saved::convert::CandidateSource,
        session::REMOTE_MESSAGE,
        view,
    },
};
use indicatrix_cut_core::rough_plan::{PlanSettings, RoughModel};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::{
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use tracing::warn;

/// Shown when the library window that holds the filter is gone.
const NO_MAIN_WINDOW: &str = "The library window is closed, so its filter cannot be read.";

/// Where a run gets its candidate design ids. Always resolved afresh by the worker when
/// the run starts, never from the counts the window showed earlier, so a library that
/// changed since then is planned as it is now. The worker also subtracts the designs
/// excluded from the planner ([`IdSource::resolve`]).
///
/// # Errors
///
/// Returns the message for `error_text` when the library filter is wanted but the window
/// that holds it no longer exists.
fn id_source(host: &Host, use_filter: bool) -> Result<IdSource, String> {
    if !use_filter {
        return Ok(IdSource::Library);
    }
    let main = host.main.upgrade().ok_or(NO_MAIN_WINDOW)?;
    Ok(IdSource::Filter(Box::new(FilterSnapshot::read(&main))))
}

/// What the Plan button plans: a snapshot of the model and the form.
struct Request {
    model: RoughModel,
    settings: PlanSettings,
    material_name: String,
    weighed_ct: Option<f64>,
}

/// Fails when one kerf plus the allowance on both sides of a stone is more than the
/// smallest side of a rough with these `extents`: no piece could be sawn from it.
///
/// # Errors
///
/// Returns the message for `error_text`.
fn check_kerf_and_allowance(settings: &PlanSettings, extents: [f64; 3]) -> Result<(), String> {
    let needed = 2.0f64.mul_add(settings.allowance_mm, settings.kerf_mm);
    let smallest = extents.into_iter().fold(f64::INFINITY, f64::min);
    if needed <= smallest {
        Ok(())
    } else {
        Err(format!(
            "The kerf ({:.2} mm) and the allowance on both sides of a stone ({:.2} mm) need \
             {needed:.2} mm, more than the rough's smallest side ({smallest:.2} mm).",
            settings.kerf_mm,
            2.0 * settings.allowance_mm,
        ))
    }
}

/// Reads the model and the form. Whether the model is a valid rough is decided by the
/// worker, which measures it and says why it is not.
///
/// # Errors
///
/// Returns the message for `error_text`.
fn read_request(host: &Rc<Host>) -> Result<Request, String> {
    let (model, bad_field) = {
        let session = host.session.borrow();
        (session.model.clone(), !session.bad_fields.is_empty())
    };
    if is_blank(&model) {
        return Err(NO_SIZE_MESSAGE.to_string());
    }
    if bad_field {
        return Err(BAD_CUT_FIELD_MESSAGE.to_string());
    }
    let (settings, material_name, weighed_ct) = current_settings(host)?;
    let extents = model.base.bounding_box_extents();
    check_skin(&settings, extents)?;
    check_kerf_and_allowance(&settings, extents)?;
    Ok(Request {
        model,
        settings,
        material_name,
        weighed_ct,
    })
}

/// Empties the window's result outputs for a new run and drops what belongs to the
/// results that were on screen: the loaded-plan banner, the save-name row, the replace
/// question, the armed "face from view" pick.
fn clear_outputs(host: &Rc<Host>) {
    let model = host.window.global::<RoughPlanModel>();
    model.set_results(ModelRc::new(VecModel::<RoughPlanResult>::default()));
    model.set_keep_count(0);
    model.set_summary("".into());
    model.set_skipped_count(0);
    model.set_expanded_index(-1);
    model.set_selected_result(-1);
    model.set_selected_group(-1);
    model.set_thumbnails_pending(false);
    model.set_loaded_banner("".into());
    model.set_save_name_open(false);
    model.set_confirm_replace_open(false);
    model.set_results_unsaved(false);
    model.set_face_from_view_armed(false);
    model.set_progress_fraction(0.0);
    model.set_stage("Preparing...".into());
    model.set_running(true);
}

/// The first of `candidates` that is not blank, or an empty string.
fn first_message<'a>(candidates: impl IntoIterator<Item = &'a str>) -> String {
    candidates
        .into_iter()
        .find(|text| !text.trim().is_empty())
        .unwrap_or_default()
        .to_string()
}

/// The text for `error_text` when committing the typed form refused Plan: the message the
/// commit returned, else the model error or the weight check the window shows for the
/// field that failed.
fn blocking_message(model: &RoughPlanModel<'_>, message: &str) -> String {
    first_message([
        message,
        model.get_model_error().as_str(),
        model.get_weight_check_text().as_str(),
    ])
}

/// What Plan was pressed with: the model and form as the window showed them at that
/// moment, and where the candidate designs come from.
struct Prepared {
    request: Request,
    ids: IdSource,
    use_filter: bool,
}

/// Validates everything Plan needs. `Err` is the message for `error_text`.
fn prepare(host: &Rc<Host>) -> Result<Prepared, String> {
    let request = read_request(host)?;
    if is_remote(&host.source) {
        return Err(REMOTE_MESSAGE.to_string());
    }
    let use_filter = host.window.global::<RoughPlanModel>().get_use_filter();
    Ok(Prepared {
        request,
        ids: id_source(host, use_filter)?,
        use_filter,
    })
}

/// `RoughPlanModel.plan` (and "Re-plan"): validates, asks before unsaved results are
/// replaced, then sets the results aside, marks the window running and hands the work to
/// a background thread.
pub(in crate::gui::rough_plan) fn start_plan(host: &Rc<Host>) {
    let model = host.window.global::<RoughPlanModel>();
    if model.get_running() {
        return;
    }
    model.set_error_text("".into());
    // A base, material or weight the user typed but did not leave yet is part of the model
    // being planned. An invalid one has shown its own error.
    if let Err(message) = editing::commit_pending_base(host) {
        model.set_error_text(blocking_message(&model, &message).into());
        return;
    }
    match prepare(host) {
        Ok(prepared) => guard_unsaved_results(host, move |host| launch(host, prepared)),
        Err(message) => model.set_error_text(message.into()),
    }
}

/// Runs `proceed` at once when no results that were not saved are on screen. Otherwise
/// opens the window's replace question (`RoughPlanModel.confirm_replace_open`, with the
/// number of layouts that would be lost) and runs `proceed` only when the answer is to
/// replace them; asking again replaces the request that was waiting.
pub(in crate::gui::rough_plan) fn guard_unsaved_results(
    host: &Rc<Host>,
    proceed: impl FnOnce(&Rc<Host>) + 'static,
) {
    let unsaved = {
        let session = host.session.borrow();
        if session.run.results_unsaved {
            session.run.layouts.len()
        } else {
            0
        }
    };
    if unsaved == 0 {
        proceed(host);
        return;
    }
    host.session.borrow_mut().run.pending = Some(Box::new(proceed));
    let model = host.window.global::<RoughPlanModel>();
    model.set_confirm_replace_count(to_i32(unsaved));
    model.set_confirm_replace_open(true);
}

/// `RoughPlanModel.confirm_replace`: the answer to the replace question. `true` runs the
/// waiting request; `false` drops it and keeps the results.
pub(super) fn answer_replace(host: &Rc<Host>, replace: bool) {
    let pending = host.session.borrow_mut().run.pending.take();
    host.window
        .global::<RoughPlanModel>()
        .set_confirm_replace_open(false);
    if let (true, Some(proceed)) = (replace, pending) {
        proceed(host);
    }
}

/// Starts the run of `prepared`: the shown results are set aside (a run that ends without
/// results gives them back), the outputs are cleared and the worker thread starts.
fn launch(host: &Rc<Host>, prepared: Prepared) {
    let model = host.window.global::<RoughPlanModel>();
    if model.get_running() {
        return;
    }
    let Prepared {
        request,
        ids,
        use_filter,
    } = prepared;

    let cancel = Arc::new(AtomicBool::new(false));
    let screen = Screen::capture(&model);
    {
        let mut session = host.session.borrow_mut();
        session.cancel = Some(Arc::clone(&cancel));
        session.run.begin_run(screen);
    }
    clear_outputs(host);
    view::results_changed(host);
    // The library may have changed since the window counted it.
    if let Some(main) = host.main.upgrade() {
        refresh_counts(&main, &host.window, &host.db, &host.source, &host.session);
    }

    let job = PlanJob {
        ids,
        model: request.model,
        settings: request.settings,
        material_name: request.material_name,
        weighed_ct: request.weighed_ct,
        candidate_source: CandidateSource::from_use_filter(use_filter),
    };
    if let Err(e) = spawn_worker(host.window.as_weak(), Arc::clone(&host.db), job, cancel) {
        warn!("Rough planner: could not start the worker thread: {e}");
        model.set_running(false);
        model.set_stage("".into());
        restore_previous(host);
        model.set_error_text(format!("Could not start the planner: {e}").into());
    }
}

/// Puts back the results a run that ended without results pushed off the screen. Returns
/// whether there was anything to put back (`false`: the run had published its own, or none
/// was set aside).
pub(super) fn restore_previous(host: &Rc<Host>) -> bool {
    let Some(screen) = host.session.borrow_mut().run.restore_replaced() else {
        return false;
    };
    let model = host.window.global::<RoughPlanModel>();
    screen.show(&model);
    model.set_results_unsaved(host.session.borrow().run.results_unsaved);
    view::results_changed(host);
    true
}

/// `RoughPlanModel.cancel`: asks the running plan to stop.
pub(super) fn cancel_plan(host: &Rc<Host>) {
    if let Some(flag) = &host.session.borrow().cancel {
        flag.store(true, Ordering::Relaxed);
    }
    let model = host.window.global::<RoughPlanModel>();
    if model.get_running() {
        model.set_stage(CANCELLING_STAGE.into());
    }
}

/// Mirrors the number of results with their keep box ticked into the window.
fn push_keep_count(host: &Rc<Host>) {
    let kept = host.session.borrow().run.kept();
    host.window
        .global::<RoughPlanModel>()
        .set_keep_count(to_i32(kept));
}

/// `RoughPlanModel.toggle_keep`: flips the keep flag of result `index`, in the state and
/// in the row.
pub(super) fn toggle_keep(host: &Rc<Host>, index: i32) {
    let Ok(row_index) = usize::try_from(index) else {
        return;
    };
    let now_kept = {
        let mut session = host.session.borrow_mut();
        let Some(flag) = session.run.keep.get_mut(row_index) else {
            return;
        };
        *flag = !*flag;
        *flag
    };
    let results = host.window.global::<RoughPlanModel>().get_results();
    if let Some(mut row) = results.row_data(row_index) {
        row.keep = now_kept;
        results.set_row_data(row_index, row);
    }
    push_keep_count(host);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(kerf_mm: f64, allowance_mm: f64) -> PlanSettings {
        PlanSettings {
            kerf_mm,
            allowance_mm,
            ..PlanSettings::default()
        }
    }

    #[test]
    fn a_rough_thinner_than_one_kerf_and_both_allowances_is_refused_with_the_figures() {
        // Every figure is a sum of powers of two, so the arithmetic is exact:
        // 0.25 + 2 x 0.375 = 1.0 mm is needed.
        let ok = check_kerf_and_allowance(&settings(0.25, 0.375), [10.0, 1.0, 8.0]);
        assert_eq!(ok, Ok(()), "exactly as thick as needed is enough");

        let message = check_kerf_and_allowance(&settings(0.25, 0.375), [10.0, 0.5, 8.0])
            .expect_err("0.5 mm is too thin");
        assert!(message.contains("(0.25 mm)"), "kerf: {message}");
        assert!(message.contains("(0.75 mm)"), "both allowances: {message}");
        assert!(message.contains("1.00 mm"), "needed: {message}");
        assert!(message.contains("(0.50 mm)"), "smallest side: {message}");
    }

    #[test]
    fn a_block_is_judged_by_its_smallest_side_whichever_axis_that_is() {
        let settings = settings(1.0, 1.0);
        for extents in [[2.9, 9.0, 9.0], [9.0, 2.9, 9.0], [9.0, 9.0, 2.9]] {
            assert!(check_kerf_and_allowance(&settings, extents).is_err());
        }
        assert_eq!(check_kerf_and_allowance(&settings, [3.0; 3]), Ok(()));
    }

    #[test]
    fn the_first_non_blank_message_wins_and_nothing_gives_an_empty_one() {
        assert_eq!(
            first_message(["Bad weight", "model", "check"]),
            "Bad weight"
        );
        assert_eq!(first_message(["", "  ", "check"]), "check");
        assert_eq!(first_message(["", "\t", " "]), "");
    }
}
