//! The editor group's smaller `setup_*` wirings: the solve-cancel button, the startup
//! restore offer and the tier-cutoff slider's replan queue. Each registers its Slint
//! callbacks on the `EditorModel`/`SolidPreviewModel` globals and is called once from
//! [`super::setup_editor_callbacks`].

use super::{auto_solve, edit_intent, native_io, stall_guard, state::EditorState, view};
use crate::{
    MainWindow, bridge::render_thread::RenderContext,
    gui::solid_preview::preview_state::SolidPreviewState,
};
use slint::{ComponentHandle as _, Model as _};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex},
};

/// Wires every "Edit" sub-tab callback (declared on `EditorModel`, `ui/models/editor.slint`'s
/// global -- see `ui/README.md` for how a `.slint` component reaches it directly, with
/// no forwarding through `MainWindow`) to a shared [`EditorState`], and populates the
/// tab's own display once up front (see [`view::refresh_editor_panel`]) so it isn't
/// blank when first opened.
///
/// Split into one `setup_*` function per callback (in [`super::callbacks`]/[`native_io`]),
/// the same shape every other `gui::*` module in this crate uses.
///
/// `auto_solve::cancel_in_flight_solve` invalidates the in-flight result, drops any
/// queued dispatch, AND flips the worker's own cancel flag; this function is what
/// gives the cutter their editor back on the same click, rather than leaving the
/// Solve button disabled until the abandoned worker lands. The design itself is
/// untouched, so the honest state afterwards is "stale" -- it still needs a solve,
/// just not that one.
///
/// The worker stops for real, typically within a sweep or pipeline run
/// (single-digit milliseconds). `dispatch_background_solve` threads
/// `SolveControl::with_cancel` through `Design::solve_with` via
/// `auto_solve::solve_cancellably`, the same cancellation `deep_solve`/
/// `optimize_solve` already use for their long-running searches. This means the
/// design genuinely stops solving, not merely discards a result still running in
/// the background.
pub(super) fn setup_solve_cancel_callback(ui: &MainWindow) {
    let ui_weak = ui.as_weak();
    ui.global::<crate::EditorModel>().on_solve_cancel(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        auto_solve::cancel_in_flight_solve();
        let model = ui.global::<crate::EditorModel>();
        model.set_solve_running(false);
        model.set_solve_state("stale".into());
        model.set_status_text("Solve abandoned -- click Solve when you are ready.".into());
        model.set_status_is_problem(true);
    });
}

/// Offers to reopen at startup instead of always
/// opening on a blank design.
///
/// Deliberately an OFFER, matching the item's own wording. Silently reopening
/// yesterday's design would be a surprise on a tool people also use to start new
/// work -- and worse, it would hide a crash-recovery file inside an ordinary-looking
/// session, so a cutter could overwrite unsaved work without ever being told it
/// existed.
///
/// A leftover autosave outranks the recent-files list: that file is on disk only
/// because a previous run did not shut down cleanly, and it holds work that was
/// never saved at all. An ordinary recent file can always be reopened later from
/// File > Open Recent; the autosave is deleted by the next successful save.
pub(super) fn setup_startup_restore(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &view::SolidLastSolved,
) {
    let model = ui.global::<crate::EditorModel>();
    let autosave = native_io::find_leftover_autosave();
    let offer = autosave.clone().or_else(|| {
        ui.get_recent_native_files()
            .iter()
            .next()
            .map(|path| std::path::PathBuf::from(path.as_str()))
    });
    let Some(offer) = offer else {
        return;
    };
    model.set_startup_restore_is_autosave(autosave.is_some());
    model.set_startup_restore_path(offer.display().to_string().into());

    let state_accept = Rc::clone(state);
    let render_ctx_accept = Arc::clone(render_ctx);
    let preview_accept = Arc::clone(preview_state);
    let solved_accept = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<crate::EditorModel>()
        .on_startup_restore_accept(move || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            // Cleared FIRST: the open below can itself toast or open the
            // fingerprint-mismatch dialog, and this prompt must be gone by then
            // rather than stacked underneath it.
            let path = ui.global::<crate::EditorModel>().get_startup_restore_path();
            ui.global::<crate::EditorModel>()
                .set_startup_restore_path(String::new().into());
            native_io::open_recent_native_path(
                &ui,
                &state_accept,
                &render_ctx_accept,
                &preview_accept,
                &solved_accept,
                std::path::PathBuf::from(path.as_str()),
            );
        });

    let ui_weak_dismiss = ui.as_weak();
    ui.global::<crate::EditorModel>()
        .on_startup_restore_dismiss(move || {
            let Some(ui) = ui_weak_dismiss.upgrade() else {
                return;
            };
            // The file is left exactly where it is -- declining the offer is not a
            // decision to throw work away. A leftover autosave is removed only by
            // the next successful save (see `finish_save_native_success`).
            ui.global::<crate::EditorModel>()
                .set_startup_restore_path(String::new().into());
        });

    // "Delete" (owner decision 4.5): only ever shown for a leftover autosave
    // (`EditorModel.startup_restore_is_autosave`, wired in
    // `global_dialogs.slint`'s `tertiary_label`), never an ordinary recent
    // file -- removes exactly the one file that was offered, never any OTHER
    // leftover `find_leftover_autosave` did not pick (see
    // `native_io::delete_leftover_autosave`'s own doc comment).
    let ui_weak_delete = ui.as_weak();
    ui.global::<crate::EditorModel>()
        .on_startup_restore_delete(move || {
            let Some(ui) = ui_weak_delete.upgrade() else {
                return;
            };
            let path = ui.global::<crate::EditorModel>().get_startup_restore_path();
            ui.global::<crate::EditorModel>()
                .set_startup_restore_path(String::new().into());
            native_io::delete_leftover_autosave(std::path::Path::new(path.as_str()));
        });
}

/// Redraws the preview when the tier-cutoff slider moves, and when the Diagram's
/// enlarged panel changes (`on_diagram_enlarged_panel_changed`, same queue).
///
/// `submit_preview_replan` already reads `SolidPreviewModel.tier_cutoff` and hands it
/// to `SolidPreviewState::set_tier_cutoff`, but nothing asked for a replan when the
/// slider itself moved -- so the whole path from slider to `Design::planes_through_tier`
/// was correct and simply never ran until an unrelated edit triggered one.
///
/// An empty `dirty` set with `force_full_solve: false`: changing how much of the
/// schedule is DRAWN does not change the design, so the solve is reusable and only
/// the plane arrangement needs rebuilding.
///
/// `solid_viewport.slint`'s slider uses its own `changed(value)` INTERACTION
/// callback (fires only on an actual drag/keyboard nudge/click, not on a tier
/// push moving the bound expression). This is the only handler left on this path,
/// using `try_borrow`/skip rather than plain `borrow()`: a writer already
/// holding the guard will submit its own replan on its way out, so this tick's
/// work would only be redundant.
///
/// A `changed(value)` tick fires on every pixel of drag, far more often than once
/// per 16ms frame. Each one posts an [`edit_intent::EditIntent::CutOff`] into a
/// queue this function builds once (see [`edit_intent::EditIntentQueue`]'s own doc
/// comment for the coalescing/timer mechanics). This avoids calling
/// [`view::submit_preview_replan`] directly (which clones the whole `Design`
/// twice), so a whole drag burst pays for at most one replan per 16ms tick rather
/// than one per pixel.
pub(super) fn setup_tier_cutoff_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &view::SolidLastSolved,
) {
    let intent_queue = {
        let state = Rc::clone(state);
        let render_ctx = Arc::clone(render_ctx);
        let preview_state = Arc::clone(preview_state);
        let solid_last_solved = Arc::clone(solid_last_solved);
        let ui_weak = ui.as_weak();
        edit_intent::EditIntentQueue::new(move |_intent| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let Ok(st) = state.try_borrow() else {
                return;
            };
            view::submit_preview_replan(
                &ui,
                &render_ctx,
                &preview_state,
                &solid_last_solved,
                &st,
                std::collections::BTreeSet::new(),
                false,
            );
        })
    };
    // The diagram's enlarged-panel choice (`SolidPreviewModel.diagram_enlarged_panel`)
    // is read by the plan worker just like the tier cutoff, and changing it alone
    // redraws nothing, so it shares this queue: the drain ignores the intent's
    // payload and `submit_preview_replan` reads every property fresh. Raised for a
    // pill click, a double-click and Escape alike (`changed diagram_enlarged_panel`
    // in `models/solid_preview.slint`).
    let enlarged_queue = Rc::clone(&intent_queue);
    let ui_weak_enlarged = ui.as_weak();
    ui.global::<crate::SolidPreviewModel>()
        .on_diagram_enlarged_panel_changed(move || {
            stall_guard::stall_guard("on_diagram_enlarged_panel_changed", || {
                let Some(ui) = ui_weak_enlarged.upgrade() else {
                    return;
                };
                let count = ui.global::<crate::SolidPreviewModel>().get_tier_cutoff();
                enlarged_queue.post(edit_intent::EditIntent::CutOff { count });
            });
        });
    let ui_weak = ui.as_weak();
    ui.global::<crate::SolidPreviewModel>()
        .on_tier_cutoff_changed(move || {
            stall_guard::stall_guard("on_tier_cutoff_changed", || {
                let Some(ui) = ui_weak.upgrade() else {
                    return;
                };
                let count = ui.global::<crate::SolidPreviewModel>().get_tier_cutoff();
                intent_queue.post(edit_intent::EditIntent::CutOff { count });
            });
        });
}
