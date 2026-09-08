//! Top-level UI wiring for the preview batch: starting one, the confirm-step dialog
//! shared by every trigger, and the Slint callback registrations -- see this group's
//! own `mod.rs` doc comment for the batch this drives.

use super::{
    engine::{
        BatchContext, DesignAccum, LaneShared, LiveProgress, Tally, build_items, run_batch_lanes,
    },
    scan::spawn_missing_preview_scan,
};
use crate::{
    BatchModel, LibraryModel, MainWindow, SettingsModel,
    bridge::{preview_render, render_thread::RenderContext},
    gui::batch::batch_queue::{LanePlan, WorkQueue, local_lane_count},
    settings::{LiveComputeTarget, SettingsPersister, WorkerSettings},
};
use indicatrix::renderer::gpu_backend::GpuBackend;
use indicatrix_vault::db::sqlite::Database;
use slint::{ComponentHandle, Model, Weak};
use std::{
    cell::RefCell,
    collections::HashMap,
    rc::Rc,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

/// Handle returned by [`spawn_preview_batch`]. Cancelling is cooperative -- see this
/// group's `mod.rs` doc comment's "Cancellation leaves completed work in place"
/// section.
pub struct PreviewBatchHandle {
    cancel: Arc<AtomicBool>,
}

impl PreviewBatchHandle {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// The final tally reported once to `on_done` when a batch ends, for any reason.
#[derive(Debug, Clone, Copy, Default)]
pub struct PreviewBatchOutcome {
    pub generated: u32,
    pub failed: u32,
    pub cancelled: bool,
}

/// Every setting [`spawn_preview_batch`] needs beyond the id list itself -- bundled
/// purely to keep that function's own argument count under clippy's limit (it already
/// has `ui_weak`/`db`/`render_ctx`/`entry_ids` as genuinely separate concerns).
pub struct PreviewBatchSettings {
    pub workers: Vec<WorkerSettings>,
    pub live_compute_target: LiveComputeTarget,
    pub preview_size: u32,
    pub preview_spp: u32,
}

pub fn spawn_preview_batch(
    ui_weak: Weak<MainWindow>,
    db: Arc<Mutex<Database>>,
    render_ctx: Arc<Mutex<RenderContext>>,
    settings: PreviewBatchSettings,
    entry_ids: Vec<i64>,
) -> PreviewBatchHandle {
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_thread = Arc::clone(&cancel);

    thread::spawn(move || {
        struct BusyGuard {
            render_ctx: Arc<Mutex<RenderContext>>,
            ui_weak: Weak<MainWindow>,
        }
        impl Drop for BusyGuard {
            fn drop(&mut self) {
                self.render_ctx
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .export_active = false;
                let ui_weak = self.ui_weak.clone();
                let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                    ui.global::<BatchModel>().set_preview_batch_running(false);
                });
            }
        }

        {
            let mut ctx = render_ctx.lock().unwrap_or_else(PoisonError::into_inner);
            ctx.export_active = true;
        }
        let _busy_guard = BusyGuard {
            render_ctx: Arc::clone(&render_ctx),
            ui_weak: ui_weak.clone(),
        };
        let _ = ui_weak
            .upgrade_in_event_loop(|ui| ui.global::<BatchModel>().set_preview_batch_running(true));

        let design_total = entry_ids.len() as u32;
        let material_candidates = preview_render::ri_candidates();
        // Acquired ONCE for the whole batch, not per item -- adapter acquisition and
        // megakernel compilation are far too slow to repeat, same reasoning as
        // `export_thread::run_export`'s own single `GpuBackend::acquire` call.
        let gpu = GpuBackend::acquire();
        // First configured worker only -- see this group's `mod.rs` doc comment's
        // "Local + remote" section, and `export_thread::remote::probe_remote`'s own doc
        // comment for the same "session-wide, first entry" convention this mirrors.
        let remote_worker = settings.workers.into_iter().next();
        let ctx = BatchContext {
            db: &db,
            material_candidates: &material_candidates,
            preview_size: settings.preview_size,
            preview_spp: settings.preview_spp,
        };

        let queue = WorkQueue::new(build_items(&entry_ids));
        let design_state: Mutex<HashMap<i64, DesignAccum>> = Mutex::new(HashMap::new());
        let tally = Tally::default();
        let progress = Mutex::new(LiveProgress::default());

        // See `gui::batch::batch_queue::LanePlan`'s own doc comment for exactly what
        // each `LiveComputeTarget` variant runs.
        let plan = LanePlan::for_target(settings.live_compute_target, remote_worker.is_some());
        // Pre-set `true` when no remote lane will ever run at all, so the local lane's
        // own stop condition (`gui::batch::batch_queue`'s doc comment) is satisfied the
        // first time it finds nothing left to claim, with no spurious poll wait.
        let remote_lane_done = AtomicBool::new(!plan.run_remote);

        // See `batch_queue::local_lane_count`'s own doc comment for why one less than
        // full parallelism, not all of it. Only actually spawned below when
        // `plan.run_local` -- `RemoteOnly` computes this but never uses it beyond
        // reporting `0` as `preview_batch_local_lane_total`.
        let local_lane_total = if plan.run_local {
            local_lane_count() as u32
        } else {
            0
        };

        let shared = LaneShared {
            ctx: &ctx,
            queue: &queue,
            design_state: &design_state,
            tally: &tally,
            progress: &progress,
            cancel: &cancel_thread,
            design_total,
            local_lane_total,
        };

        // Every lane borrows `shared`/`gpu`/`remote_lane_done` by plain reference (no
        // `Arc` needed) -- `std::thread::scope` (inside `run_batch_lanes`) guarantees
        // every spawned thread is joined before that call returns, so those borrows
        // never need to outlive it. Each lane gets its OWN cloned `Weak<MainWindow>`
        // (owned, not borrowed) -- see `LaneShared`'s own doc comment for why.
        run_batch_lanes(
            &shared,
            &gpu,
            plan,
            remote_worker.as_ref(),
            local_lane_total,
            &ui_weak,
            &remote_lane_done,
        );

        let outcome = PreviewBatchOutcome {
            generated: tally.generated.load(Ordering::Relaxed),
            failed: tally.failed.load(Ordering::Relaxed),
            cancelled: cancel_thread.load(Ordering::Relaxed),
        };
        let _ = ui_weak.upgrade_in_event_loop(move |ui| {
            ui.global::<BatchModel>().set_preview_summary(
                format!(
                    "Generated previews for {} design(s){}{}.",
                    outcome.generated,
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
            ui.global::<BatchModel>().set_preview_done(true);
        });
        // `_busy_guard` drops here, clearing `export_active` and
        // `preview_batch_running` unconditionally -- see this group's `mod.rs` doc
        // comment.
    });

    PreviewBatchHandle { cancel }
}

/// Starts a batch for `entry_ids`, reading `preview_size`/`preview_spp`/the configured
/// remote workers AND the persisted `LiveComputeTarget` from `settings_store`'s current
/// snapshot -- the SAME standing "Live Compute" preference the live viewport uses
/// (`settings_dialog.slint`'s pill, `AppSettings::live_compute_target`), not a separate
/// picker of this batch's own. The one place every trigger (`setup_preview_batch_callbacks`'s
/// context-menu and confirmed-offer handlers, and its own `spawn_missing_preview_scan`
/// result handler) funnels through, so all three read settings the same way and none of
/// them can drift.
fn start_batch(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
    handle_slot: &Rc<RefCell<Option<PreviewBatchHandle>>>,
    entry_ids: Vec<i64>,
) {
    if entry_ids.is_empty() {
        return;
    }
    let snapshot = settings_store.snapshot();
    ui.global::<BatchModel>().set_preview_visible(true);
    ui.global::<BatchModel>().set_preview_confirming(false);
    ui.global::<BatchModel>().set_preview_done(false);
    ui.global::<BatchModel>().set_preview_batch_running(true);
    ui.global::<BatchModel>().set_preview_design_index(0);
    ui.global::<BatchModel>()
        .set_preview_design_total(entry_ids.len() as i32);
    ui.global::<BatchModel>().set_preview_local_active(0);
    ui.global::<BatchModel>().set_preview_local_lane_total(0);
    ui.global::<BatchModel>()
        .set_preview_remote_title(String::new().into());
    ui.global::<BatchModel>().set_preview_remote_active(false);
    ui.global::<BatchModel>()
        .set_preview_summary(String::new().into());

    let handle = spawn_preview_batch(
        ui.as_weak(),
        Arc::clone(db),
        Arc::clone(render_ctx),
        PreviewBatchSettings {
            workers: snapshot.settings.remote_workers,
            live_compute_target: snapshot.settings.live_compute_target,
            preview_size: snapshot.settings.preview_size,
            preview_spp: snapshot.settings.preview_spp,
        },
        entry_ids,
    );
    *handle_slot.borrow_mut() = Some(handle);
}

/// Opens the confirm step (`preview_batch_dialog.slint`'s "N designs -- Generate?"
/// state) for `ids` -- that component derives its own explanatory text from
/// `preview_offer_count` (singular/plural wording), so there is no separate message
/// string to pass here. `ids` travels through the Slint-side `preview_offer_ids`
/// property (an `[int]` model), NOT a Rust-side
/// `Rc<RefCell<...>>`, precisely so a caller OUTSIDE this module -- `gui::library`'s
/// post-import completion handler (trigger #1: "ask the user whether to generate
/// previews for what was just imported") -- can open this exact same confirm step
/// without this module having to expose any of its internal callback-closure state to
/// it. `setup_preview_batch_callbacks`'s own `spawn_missing_preview_scan` result
/// handler (trigger #2) is the other caller, so both triggers share one dialog and one
/// code path -- there is exactly one way this app ever asks "generate previews for
/// these N designs?", not two that could drift apart.
///
/// A no-op for an empty `ids` -- nothing to offer.
pub fn offer_batch_confirmation(ui: &MainWindow, ids: &[i64]) {
    if ids.is_empty() {
        return;
    }
    #[allow(
        clippy::cast_possible_truncation,
        reason = "entry ids are SQLite AUTOINCREMENT row ids from a several-thousand-\
                  row local catalogue, nowhere near i32::MAX; Slint's `[int]` model \
                  type has no i64 element type to use instead"
    )]
    let ids_i32: Vec<i32> = ids.iter().map(|&id| id as i32).collect();
    ui.global::<BatchModel>()
        .set_preview_offer_ids(slint::ModelRc::new(slint::VecModel::from(ids_i32)));
    ui.global::<BatchModel>()
        .set_preview_offer_count(ids.len() as i32);
    ui.global::<BatchModel>().set_preview_confirming(true);
    ui.global::<BatchModel>().set_preview_visible(true);
}

/// Wires up every preview-batch-related callback on `ui`:
///
/// - `preview_batch_cancel` -- the progress dialog's Cancel button
///   (`preview_batch_dialog.slint`).
/// - `preview_batch_generate_confirmed`/`preview_batch_dismiss_offer` -- the two
///   buttons on the confirm step [`offer_batch_confirmation`] opens (triggers #1/#2).
/// - `preview_batch_close` -- dismisses the finished-summary state.
/// - `generate_previews_for_entry` -- the context menu's single-design trigger
///   (trigger #3; wired up from `context_menu.slint` via `app.slint`, see that file's
///   own comment on why this one item is deliberately the ONLY thing on that menu).
///
/// Also kicks off the once-per-session "designs with no previews yet" scan (trigger
/// #2's other half) via `spawn_missing_preview_scan` -- "once per session" falls out
/// of this function itself only ever being called once, from `gui::build_main_window`,
/// rather than needing its own separate "already asked" flag.
///
/// Called once, from `gui::build_main_window`, alongside every other `setup_*` call.
pub fn setup_preview_batch_callbacks(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
) {
    let handle: Rc<RefCell<Option<PreviewBatchHandle>>> = Rc::new(RefCell::new(None));

    // Preview render size/spp sliders are exposed as settings: plain settings-file writes, no `RenderContext`/live-viewport
    // involvement at all, since these only ever affect the NEXT batch dispatch's
    // `PreviewJob`, never anything already on screen.
    let settings_store_size = Arc::clone(settings_store);
    ui.global::<SettingsModel>()
        .on_preview_size_changed(move |size| {
            settings_store_size.update(|s| s.settings.preview_size = size.max(1) as u32);
        });
    let settings_store_spp = Arc::clone(settings_store);
    ui.global::<SettingsModel>()
        .on_preview_spp_changed(move |spp| {
            settings_store_spp.update(|s| s.settings.preview_spp = spp.max(1) as u32);
        });

    let handle_cancel = Rc::clone(&handle);
    ui.global::<BatchModel>().on_preview_cancel(move || {
        if let Some(h) = handle_cancel.borrow().as_ref() {
            h.cancel();
        }
    });

    let ui_weak_close = ui.as_weak();
    ui.global::<BatchModel>().on_preview_close(move || {
        if let Some(ui) = ui_weak_close.upgrade() {
            ui.global::<BatchModel>().set_preview_visible(false);
            ui.global::<BatchModel>().set_preview_done(false);
        }
    });

    let ui_weak_dismiss = ui.as_weak();
    ui.global::<BatchModel>().on_preview_dismiss_offer(move || {
        if let Some(ui) = ui_weak_dismiss.upgrade() {
            ui.global::<BatchModel>().set_preview_visible(false);
            ui.global::<BatchModel>().set_preview_confirming(false);
        }
    });

    let db_confirm = Arc::clone(db);
    let render_ctx_confirm = Arc::clone(render_ctx);
    let settings_store_confirm = Arc::clone(settings_store);
    let handle_confirm = Rc::clone(&handle);
    let ui_weak_confirm = ui.as_weak();
    ui.global::<BatchModel>()
        .on_preview_generate_confirmed(move || {
            if let Some(ui) = ui_weak_confirm.upgrade() {
                let ids: Vec<i64> = ui
                    .global::<BatchModel>()
                    .get_preview_offer_ids()
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
        .on_generate_previews_for_entry(move |id: i32| {
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
    spawn_missing_preview_scan(ui.as_weak(), db_scan, |ui, ids| {
        offer_batch_confirmation(ui, &ids);
    });
}
