//! Top-level UI wiring for the tilt-curve batch: starting one, the confirm-step
//! dialog, and the Slint callback registrations -- see this group's own `mod.rs` doc
//! comment for the batch this drives.

use super::{
    engine::{BatchContext, LaneShared, LiveProgress, Tally, run_batch_lanes},
    scan::spawn_missing_tilt_curve_scan,
};
use crate::{
    BatchModel, LibraryModel, MainWindow,
    bridge::{preview_render, render_thread::RenderContext},
    gui::batch::batch_queue::{LanePlan, WorkQueue, local_lane_count},
    settings::{LiveComputeTarget, SettingsPersister, WorkerSettings},
};
use indicatrix_vault::db::sqlite::Database;
use slint::{ComponentHandle, Model, Weak};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

/// Handle returned by [`spawn_tilt_batch`]. Cancelling is cooperative -- see this
/// group's `mod.rs` doc comment's "Cancellation and panic isolation" section.
pub struct TiltBatchHandle {
    cancel: Arc<AtomicBool>,
}

impl TiltBatchHandle {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// The final tally reported once to `on_done` when a batch ends, for any reason.
#[derive(Debug, Clone, Copy, Default)]
pub struct TiltBatchOutcome {
    pub computed: u32,
    pub failed: u32,
    pub cancelled: bool,
}

pub fn spawn_tilt_batch(
    ui_weak: Weak<MainWindow>,
    db: Arc<Mutex<Database>>,
    render_ctx: Arc<Mutex<RenderContext>>,
    workers: Vec<WorkerSettings>,
    live_compute_target: LiveComputeTarget,
    entry_ids: Vec<i64>,
) -> TiltBatchHandle {
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_thread = Arc::clone(&cancel);

    thread::spawn(move || {
        struct BusyGuard {
            render_ctx: Arc<Mutex<RenderContext>>,
            ui_weak: Weak<MainWindow>,
        }
        impl Drop for BusyGuard {
            fn drop(&mut self) {
                // A COUNT, not a bool: the batch preview, the batch tilt sweep and a
                // hi-res export queue can each be in flight at once, so
                // this decrements its own claim rather than unconditionally clearing
                // every job's -- see `RenderContext::export_active_count`'s doc
                // comment.
                RenderContext::lock(&self.render_ctx).export_active_count -= 1;
                let ui_weak = self.ui_weak.clone();
                let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                    ui.global::<BatchModel>().set_tilt_batch_running(false);
                });
            }
        }

        RenderContext::lock(&render_ctx).export_active_count += 1;
        let _busy_guard = BusyGuard {
            render_ctx: Arc::clone(&render_ctx),
            ui_weak: ui_weak.clone(),
        };
        let _ = ui_weak
            .upgrade_in_event_loop(|ui| ui.global::<BatchModel>().set_tilt_batch_running(true));

        let design_total = entry_ids.len() as u32;
        let material_candidates = preview_render::ri_candidates();
        // First configured worker only -- same "session-wide, first entry" convention
        // `gui::batch::preview::wiring::spawn_preview_batch` uses for the identical
        // reason (see that function's own doc comment).
        let remote_worker = workers.into_iter().next();
        let ctx = BatchContext {
            db: &db,
            material_candidates: &material_candidates,
        };

        let queue = WorkQueue::new(entry_ids);
        let tally = Tally::default();
        let progress = Mutex::new(LiveProgress::default());

        // See `gui::batch::batch_queue::LanePlan`'s own doc comment for exactly what
        // each `LiveComputeTarget` variant runs.
        let plan = LanePlan::for_target(live_compute_target, remote_worker.is_some());
        // Pre-set `true` when no remote lane will ever run at all, so the local lane's
        // own stop condition (`gui::batch::batch_queue`'s doc comment) is satisfied
        // the first time it finds nothing left to claim, with no spurious poll wait.
        let remote_lane_done = AtomicBool::new(!plan.run_remote);

        // See `batch_queue::local_lane_count`'s own doc comment for why one less than
        // full parallelism, not all of it. Only actually spawned below when
        // `plan.run_local` -- `RemoteOnly` computes this but never uses it beyond
        // reporting `0` as `tilt_batch_local_lane_total`.
        let local_lane_total = if plan.run_local {
            local_lane_count() as u32
        } else {
            0
        };

        let shared = LaneShared {
            ctx: &ctx,
            queue: &queue,
            tally: &tally,
            progress: &progress,
            cancel: &cancel_thread,
            design_total,
            local_lane_total,
        };

        // Every lane borrows `shared`/`queue`/`tally`/`progress`/`remote_lane_done` by
        // plain reference (no `Arc` needed) -- `std::thread::scope` (inside
        // `run_batch_lanes`) guarantees every spawned thread is joined before that
        // call returns, so those borrows never need to outlive it. Each lane gets its
        // OWN cloned `Weak<MainWindow>` (owned, not borrowed) -- see `LaneShared`'s
        // own doc comment for why.
        run_batch_lanes(
            &shared,
            plan,
            remote_worker.as_ref(),
            local_lane_total,
            &ui_weak,
            &remote_lane_done,
        );

        let outcome = TiltBatchOutcome {
            computed: tally.computed.load(Ordering::Relaxed),
            failed: tally.failed.load(Ordering::Relaxed),
            cancelled: cancel_thread.load(Ordering::Relaxed),
        };
        let _ = ui_weak.upgrade_in_event_loop(move |ui| {
            ui.global::<BatchModel>().set_tilt_summary(
                format!(
                    "Computed tilt curves for {} design(s){}{}.",
                    outcome.computed,
                    if outcome.failed > 0 {
                        format!(", {} failed", outcome.failed)
                    } else {
                        String::new()
                    },
                    if outcome.cancelled {
                        " (cancelled)"
                    } else {
                        ""
                    }
                )
                .into(),
            );
            ui.global::<BatchModel>().set_tilt_done(true);
        });
        // `_busy_guard` drops here, clearing `export_active` and `tilt_batch_running`
        // unconditionally -- see this group's `mod.rs` doc comment.
    });

    TiltBatchHandle { cancel }
}

/// Starts a batch for `entry_ids`, reading the configured remote workers AND the
/// persisted `LiveComputeTarget` from `settings_store`'s current snapshot -- the SAME
/// standing "Live Compute" preference the live viewport uses (`settings_dialog.slint`'s
/// pill, `AppSettings::live_compute_target`), not a separate picker of this batch's own.
/// Mirrors `gui::batch::preview::wiring::start_batch`'s own shape, minus the
/// preview-size/spp settings this batch has no use for.
fn start_batch(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
    handle_slot: &Rc<RefCell<Option<TiltBatchHandle>>>,
    entry_ids: Vec<i64>,
) {
    if entry_ids.is_empty() {
        return;
    }
    let snapshot = settings_store.snapshot();
    ui.global::<BatchModel>().set_tilt_visible(true);
    ui.global::<BatchModel>().set_tilt_confirming(false);
    ui.global::<BatchModel>().set_tilt_done(false);
    ui.global::<BatchModel>().set_tilt_batch_running(true);
    ui.global::<BatchModel>().set_tilt_design_index(0);
    // `entry_ids.len()` is at most one catalogue's worth of rows (a few thousand in
    // the real corpus), nowhere near `i32::MAX` -- `cast_possible_truncation` is
    // workspace-`allow`ed (`Cargo.toml`).
    ui.global::<BatchModel>()
        .set_tilt_design_total(entry_ids.len() as i32);
    ui.global::<BatchModel>().set_tilt_local_active(0);
    ui.global::<BatchModel>().set_tilt_local_lane_total(0);
    ui.global::<BatchModel>()
        .set_tilt_remote_title(String::new().into());
    ui.global::<BatchModel>().set_tilt_remote_active(false);
    ui.global::<BatchModel>()
        .set_tilt_summary(String::new().into());

    let handle = spawn_tilt_batch(
        ui.as_weak(),
        Arc::clone(db),
        Arc::clone(render_ctx),
        snapshot.settings.remote_workers,
        snapshot.settings.live_compute_target,
        entry_ids,
    );
    *handle_slot.borrow_mut() = Some(handle);
}

/// Opens the confirm step (`tilt_batch_dialog.slint`'s "N designs -- Compute?" state)
/// for `ids` -- mirrors `gui::batch::preview::offer_batch_confirmation` exactly, just
/// against the `tilt_offer_*`/`tilt_batch_*` property names instead of the preview
/// batch's own. A no-op for an empty `ids`.
///
/// `pub` (re-exported by `super::mod`'s `pub use`): the owner's "regenerate tilt
/// curves for the filtered set" library action (`gui::library`) opens this exact
/// same confirm step for the currently-filtered id set, the same way
/// `gui::batch::preview::offer_batch_confirmation` is already `pub` for its own
/// "regenerate previews" counterpart.
pub fn offer_batch_confirmation(ui: &MainWindow, ids: &[i64]) {
    if ids.is_empty() {
        return;
    }
    // Entry ids are SQLite AUTOINCREMENT row ids from a several-thousand-row local
    // catalogue, nowhere near `i32::MAX`; Slint's `[int]` model type has no i64
    // element type to use instead -- same cast
    // `gui::batch::preview::offer_batch_confirmation` makes for the identical reason.
    let ids_i32: Vec<i32> = ids.iter().map(|&id| id as i32).collect();
    ui.global::<BatchModel>()
        .set_tilt_offer_ids(slint::ModelRc::new(slint::VecModel::from(ids_i32)));
    ui.global::<BatchModel>()
        .set_tilt_offer_count(ids.len() as i32);
    ui.global::<BatchModel>().set_tilt_confirming(true);
    ui.global::<BatchModel>().set_tilt_visible(true);
}

/// Wires up every tilt-curve-batch-related callback on `ui`:
///
/// - `tilt_batch_cancel`/`tilt_batch_close` -- the progress dialog's Cancel/Close.
/// - `tilt_batch_dismiss_offer`/`tilt_batch_generate_confirmed` -- the confirm step's
///   two buttons.
/// - `compute_tilt_curves_for_entry` -- the diagram-list context menu's single-design
///   trigger (`diagram_list.slint`, alongside "Generate Previews"/"Ignore"). Starts
///   immediately, no confirm step, same treatment
///   `gui::batch::preview::setup_preview_batch_callbacks`'s own
///   `generate_previews_for_entry` handler gives its single-design trigger.
/// - `compute_missing_tilt_curves` -- the filter panel's missing-curves notice button AND the
///   general "fix the whole catalogue" action (see `filter_panel.slint`'s own
///   `compute_missing_tilt_curves` callback, surfaced from the excluded-for-missing-
///   curves notice). Runs [`super::scan::spawn_missing_tilt_curve_scan`] first (this
///   batch is never pre-scanned automatically -- see that function's own doc comment),
///   then opens the confirm step for whatever it finds, or toasts "nothing to do" if
///   the catalogue is already fully covered.
///
/// Called once, from `gui::build_main_window`, alongside every other `setup_*` call --
/// unlike `gui::batch::preview::setup_preview_batch_callbacks`, this does NOT also
/// kick off a scan at startup; see this group's own `mod.rs` doc comment for why.
pub fn setup_tilt_batch_callbacks(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
) {
    let handle: Rc<RefCell<Option<TiltBatchHandle>>> = Rc::new(RefCell::new(None));

    let handle_cancel = Rc::clone(&handle);
    ui.global::<BatchModel>().on_tilt_cancel(move || {
        if let Some(h) = handle_cancel.borrow().as_ref() {
            h.cancel();
        }
    });

    let ui_weak_close = ui.as_weak();
    ui.global::<BatchModel>().on_tilt_close(move || {
        if let Some(ui) = ui_weak_close.upgrade() {
            ui.global::<BatchModel>().set_tilt_visible(false);
            ui.global::<BatchModel>().set_tilt_done(false);
        }
    });

    let ui_weak_dismiss = ui.as_weak();
    ui.global::<BatchModel>().on_tilt_dismiss_offer(move || {
        if let Some(ui) = ui_weak_dismiss.upgrade() {
            ui.global::<BatchModel>().set_tilt_visible(false);
            ui.global::<BatchModel>().set_tilt_confirming(false);
        }
    });

    let db_confirm = Arc::clone(db);
    let render_ctx_confirm = Arc::clone(render_ctx);
    let settings_store_confirm = Arc::clone(settings_store);
    let handle_confirm = Rc::clone(&handle);
    let ui_weak_confirm = ui.as_weak();
    ui.global::<BatchModel>()
        .on_tilt_generate_confirmed(move || {
            if let Some(ui) = ui_weak_confirm.upgrade() {
                let ids: Vec<i64> = ui
                    .global::<BatchModel>()
                    .get_tilt_offer_ids()
                    .iter()
                    .map(i64::from)
                    .collect();
                start_batch(
                    &ui,
                    &db_confirm,
                    &render_ctx_confirm,
                    &settings_store_confirm,
                    &handle_confirm,
                    ids,
                );
            }
        });

    let db_entry = Arc::clone(db);
    let render_ctx_entry = Arc::clone(render_ctx);
    let settings_store_entry = Arc::clone(settings_store);
    let handle_entry = Rc::clone(&handle);
    let ui_weak_entry = ui.as_weak();
    ui.global::<LibraryModel>()
        .on_compute_tilt_curves_for_entry(move |id: i32| {
            if let Some(ui) = ui_weak_entry.upgrade() {
                start_batch(
                    &ui,
                    &db_entry,
                    &render_ctx_entry,
                    &settings_store_entry,
                    &handle_entry,
                    vec![i64::from(id)],
                );
            }
        });

    let db_scan = Arc::clone(db);
    let ui_weak_scan = ui.as_weak();
    ui.global::<LibraryModel>()
        .on_compute_missing_tilt_curves(move || {
            if let Some(ui) = ui_weak_scan.upgrade() {
                crate::gui::show_toast(
                    &ui,
                    "Scanning the catalogue for missing tilt curves...",
                    "info",
                );
                spawn_missing_tilt_curve_scan(ui.as_weak(), Arc::clone(&db_scan), |ui, ids| {
                    if ids.is_empty() {
                        crate::gui::show_toast(
                            ui,
                            "Every visible design already has tilt curves.",
                            "success",
                        );
                    } else {
                        offer_batch_confirmation(ui, &ids);
                    }
                });
            }
        });
}
