//! Top-level UI wiring for the preview batch: starting one, the confirm-step dialog
//! shared by every trigger, and the Slint callback registrations -- see this group's
//! own `mod.rs` doc comment for the batch this drives.

use super::{
    engine::{
        BatchContext, DesignAccum, LaneShared, LiveProgress, Tally, build_items, run_batch_lanes,
    },
    import_choice::{remember_choice, show_saved_choice},
    scan::spawn_missing_preview_scan,
};
use crate::{
    BatchModel, LibraryModel, MainWindow, SettingsModel,
    bridge::{preview_render, render_thread::RenderContext},
    gui::batch::{
        batch_queue::{LanePlan, WorkQueue, local_lane_count, remote_lane_count},
        preview_cache::PreviewThumbnailCache,
        remote_lanes_setting::setup_remote_batch_lanes,
    },
    settings::{ImportPreviewChoice, LiveComputeTarget, SettingsPersister, WorkerSettings},
};
use indicatrix::renderer::gpu_backend::GpuBackend;
use indicatrix_vault::db::sqlite::Database;
use slint::{ComponentHandle, Model, Weak};
use std::{
    cell::RefCell,
    collections::{BTreeSet, HashMap},
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
};

/// Which kind of picture a batch makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewMode {
    /// The traced previews, on every engine the Live Compute setting allows.
    Full,
    /// Fast solid stand-ins drawn on the CPU; the traced previews can follow later.
    Solid,
}

/// Handle returned by [`spawn_preview_batch`]. Cancelling is cooperative -- see this
/// group's `mod.rs` doc comment's "Cancellation leaves completed work in place"
/// section.
pub struct PreviewBatchHandle {
    cancel: Arc<AtomicBool>,
}

impl PreviewBatchHandle {
    /// Asks the running batch to stop before it claims another item.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// The final tally reported once to `on_done` when a batch ends, for any reason.
#[derive(Debug, Clone, Copy, Default)]
pub struct PreviewBatchOutcome {
    pub generated: u32,
    pub failed: u32,
    /// Designs whose geometry came from the angle table (no usable design file).
    pub angle_table: usize,
    pub cancelled: bool,
}

/// Every setting [`spawn_preview_batch`] needs beyond the id list itself -- bundled
/// purely to keep that function's own argument count under clippy's limit (it already
/// has `ui_weak`/`db`/`render_ctx`/`entry_ids` as genuinely separate concerns).
pub struct PreviewBatchSettings {
    /// The remote endpoint's connection (`AppSettings::remote`), if one is configured.
    pub worker: Option<WorkerSettings>,
    pub live_compute_target: LiveComputeTarget,
    pub preview_size: u32,
    pub preview_spp: u32,
    /// How many pictures the batch keeps in flight on the remote at once
    /// (`AppSettings::remote_batch_lanes`); read through
    /// [`remote_lane_count`], which limits it to `1..=32`.
    pub remote_batch_lanes: u32,
    /// Which kind of picture the batch makes.
    pub mode: PreviewMode,
    /// The library card thumbnail cache; invalidated per design once its previews are
    /// saved, so the card shows the new images immediately.
    pub thumbnail_cache: PreviewThumbnailCache,
}

/// Releases a batch's claim on the render context and clears the dialog's "running" flag
/// when dropped, whatever ended the batch -- see this group's `mod.rs` doc comment.
struct BusyGuard {
    render_ctx: Arc<Mutex<RenderContext>>,
    ui_weak: Weak<MainWindow>,
}

impl Drop for BusyGuard {
    fn drop(&mut self) {
        // A COUNT, not a bool: the batch preview, the batch tilt
        // sweep and a hi-res export queue can each be in flight at once, so
        // this decrements its own claim rather than unconditionally clearing
        // every job's -- see `RenderContext::export_active_count`'s doc
        // comment.
        RenderContext::lock(&self.render_ctx).export_active_count -= 1;
        let ui_weak = self.ui_weak.clone();
        let _ = ui_weak.upgrade_in_event_loop(move |ui| {
            ui.global::<BatchModel>().set_preview_batch_running(false);
        });
    }
}

/// Writes the finished batch's summary line into the dialog and marks it done.
fn push_summary(ui_weak: &Weak<MainWindow>, outcome: PreviewBatchOutcome, solid: bool) {
    let _ = ui_weak.upgrade_in_event_loop(move |ui| {
        ui.global::<BatchModel>().set_preview_summary(
            format!(
                "Generated {}previews for {} design(s){}{}{}.",
                if solid { "solid " } else { "" },
                outcome.generated,
                if outcome.failed > 0 {
                    format!(", {} failed", outcome.failed)
                } else {
                    String::new()
                },
                super::super::angle_table_summary(outcome.angle_table),
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
}

/// Starts a preview-image batch over the requested library entries on a background thread and returns a handle that can cancel it.
pub fn spawn_preview_batch(
    ui_weak: Weak<MainWindow>,
    db: Arc<Mutex<Database>>,
    render_ctx: Arc<Mutex<RenderContext>>,
    settings: PreviewBatchSettings,
    entry_ids: Vec<i64>,
    // This batch's own id, plus the shared "most recently started batch" counter --
    // see `start_batch`'s own doc comment for why the final summary/done push below
    // compares them before writing anything, rather than trusting this is still the
    // only batch anyone cares about by the time it finishes.
    batch_id: u64,
    current_batch_id: Arc<AtomicU64>,
) -> PreviewBatchHandle {
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_thread = Arc::clone(&cancel);

    thread::spawn(move || {
        RenderContext::lock(&render_ctx).export_active_count += 1;
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
        let solid = settings.mode == PreviewMode::Solid;
        // The solid pictures are drawn on the CPU, so no adapter is acquired for them.
        let gpu = if solid {
            GpuBackend::disabled()
        } else {
            GpuBackend::acquire()
        };
        // Shared by every local lane (via `BatchContext::gpu_retired`) so a
        // caught wgpu-fatal panic in any ONE of them retires `gpu` for the rest of the
        // batch, not just the lane it happened on -- see `engine::catch_local_render`'s
        // own doc comment.
        let gpu_retired = AtomicBool::new(false);
        // The one remote endpoint -- see this group's `mod.rs` doc
        // comment's "Local + remote" section.
        // Solid pictures are cheap enough that shipping scenes to a remote would cost
        // more than drawing them, so that mode never uses it.
        let remote_worker = if solid { None } else { settings.worker };
        let thumbnail_cache = settings.thumbnail_cache;
        let angle_table_entries = Mutex::new(BTreeSet::new());
        let ctx = BatchContext {
            db: &db,
            material_candidates: &material_candidates,
            preview_size: settings.preview_size,
            preview_spp: settings.preview_spp,
            solid,
            gpu_retired: &gpu_retired,
            angle_table_entries: &angle_table_entries,
        };

        let queue = WorkQueue::new(build_items(&entry_ids));
        let design_state: Mutex<HashMap<i64, DesignAccum>> = Mutex::new(HashMap::new());
        let tally = Tally::default();
        let progress = Mutex::new(LiveProgress::default());

        // See `gui::batch::batch_queue::LanePlan`'s own doc comment for exactly what
        // each `LiveComputeTarget` variant runs.
        let target = if solid {
            LiveComputeTarget::LocalOnly
        } else {
            settings.live_compute_target
        };
        let plan = LanePlan::for_target(target, remote_worker.is_some());
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
        // One remote dispatcher per picture kept in flight on the remote -- see
        // `batch_queue::remote_lane_count`. `RemoteOnly` and `Both` both use all of
        // them; `LocalOnly` runs none.
        let remote_lane_total = if plan.run_remote {
            remote_lane_count(settings.remote_batch_lanes) as u32
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
            remote_lane_total,
            sit_out_toasted: AtomicBool::new(false),
            thumbnail_cache: &thumbnail_cache,
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
            angle_table: super::super::angle_table_count(&angle_table_entries),
            cancelled: cancel_thread.load(Ordering::Relaxed),
        };
        // Dropped, not pushed, when a NEWER batch has already started -- `start_batch`
        // refuses a second batch while `preview_batch_running` is true, but a batch
        // that finishes in the same tick a fresh one starts (Close then immediately
        // re-trigger) could otherwise still land its summary/done on top of the new
        // batch's own freshly reset dialog state. See `start_batch`'s own doc comment.
        if current_batch_id.load(Ordering::SeqCst) == batch_id {
            push_summary(&ui_weak, outcome, solid);
        }
        // `_busy_guard` drops here, clearing `export_active` and
        // `preview_batch_running` unconditionally -- see this group's `mod.rs` doc
        // comment.
    });

    PreviewBatchHandle { cancel }
}

/// The per-window state every [`start_batch`] call site shares, bundled to keep that
/// function's argument count under clippy's limit: the running batch's cancel handle,
/// the "most recently started batch" counter and the card thumbnail cache the batch
/// invalidates as designs finish.
struct BatchSlots {
    handle_slot: Rc<RefCell<Option<PreviewBatchHandle>>>,
    current_batch_id: Arc<AtomicU64>,
    thumbnail_cache: PreviewThumbnailCache,
}

/// Starts a batch for `entry_ids`, reading `preview_size`/`preview_spp`/the configured
/// remote endpoint AND the persisted `LiveComputeTarget` from `settings_store`'s current
/// snapshot -- the SAME standing "Live Compute" preference the live viewport uses
/// (`settings_dialog.slint`'s pill, `AppSettings::live_compute_target`), not a separate
/// picker of this batch's own. The one place every trigger (`setup_preview_batch_callbacks`'s
/// context-menu and confirmed-offer handlers, and its own `spawn_missing_preview_scan`
/// result handler) funnels through, so all three read settings the same way and none of
/// them can drift.
/// Refuses (toasts) rather than starting a SECOND batch while one is already
/// running: every caller (the confirm step, the single-design context-menu
/// trigger, and the missing-previews scan's own confirm offer, plus the library's
/// own post-import "generate previews for what was just imported" offer) funnels
/// through here, but nothing previously stopped two of them firing close together
/// -- `handle_slot`'s previous `Some(handle)` would simply be overwritten,
/// orphaning the FIRST batch's thread with no `PreviewBatchHandle` left to cancel
/// it, and resetting the dialog's progress fields out from under it mid-run.
/// `current_batch_id` is bumped on every ACCEPTED start and threaded into
/// [`spawn_preview_batch`], which compares it before writing its own final
/// summary/done -- see that function's own comment on the completion push.
fn start_batch(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
    slots: &BatchSlots,
    entry_ids: Vec<i64>,
    mode: PreviewMode,
) {
    let BatchSlots {
        handle_slot,
        current_batch_id,
        thumbnail_cache,
    } = slots;
    if entry_ids.is_empty() {
        return;
    }
    if ui.global::<BatchModel>().get_preview_batch_running() {
        crate::gui::show_toast(
            ui,
            "A preview batch is already running -- wait for it to finish or cancel it \
             first.",
            "info",
        );
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
    ui.global::<BatchModel>().set_preview_remote_in_flight(0);
    ui.global::<BatchModel>()
        .set_preview_summary(String::new().into());

    let batch_id = current_batch_id.fetch_add(1, Ordering::SeqCst) + 1;
    let handle = spawn_preview_batch(
        ui.as_weak(),
        Arc::clone(db),
        Arc::clone(render_ctx),
        PreviewBatchSettings {
            worker: snapshot.settings.remote_worker(),
            live_compute_target: snapshot.settings.live_compute_target,
            preview_size: snapshot.settings.preview_size,
            preview_spp: snapshot.settings.preview_spp,
            remote_batch_lanes: snapshot.settings.remote_batch_lanes,
            mode,
            thumbnail_cache: thumbnail_cache.clone(),
        },
        entry_ids,
        batch_id,
        Arc::clone(current_batch_id),
    );
    *handle_slot.borrow_mut() = Some(handle);
}

/// What the confirm step's answers need to start a batch.
struct ConfirmEnv {
    db: Arc<Mutex<Database>>,
    render_ctx: Arc<Mutex<RenderContext>>,
    settings_store: Arc<SettingsPersister>,
    slots: BatchSlots,
}

/// Starts the batch the confirm step offered, in `mode`. When an import opened the
/// question and its "remember my choice" box is ticked, the answer is stored first.
fn confirm_offer(ui: &MainWindow, env: &ConfirmEnv, mode: PreviewMode) {
    let model = ui.global::<BatchModel>();
    if model.get_preview_offer_from_import() && model.get_preview_remember_choice() {
        let choice = match mode {
            PreviewMode::Full => ImportPreviewChoice::Full,
            PreviewMode::Solid => ImportPreviewChoice::Solid,
        };
        remember_choice(ui, &env.settings_store, choice);
    }
    let ids: Vec<i64> = model
        .get_preview_offer_ids()
        .iter()
        .map(i64::from)
        .collect();
    start_batch(
        ui,
        &env.db,
        &env.render_ctx,
        &env.settings_store,
        &env.slots,
        ids,
        mode,
    );
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
    open_offer(ui, ids, false);
}

/// [`offer_batch_confirmation`], also naming whether an import opened it -- which adds
/// the quick solid option and the remember box to the question.
pub(super) fn open_offer(ui: &MainWindow, ids: &[i64], from_import: bool) {
    if ids.is_empty() {
        return;
    }
    let ids_i32: Vec<i32> = ids.iter().map(|&id| id as i32).collect();
    ui.global::<BatchModel>()
        .set_preview_offer_ids(slint::ModelRc::new(slint::VecModel::from(ids_i32)));
    ui.global::<BatchModel>()
        .set_preview_offer_count(ids.len() as i32);
    ui.global::<BatchModel>()
        .set_preview_offer_regenerate(false);
    ui.global::<BatchModel>()
        .set_preview_offer_from_import(from_import);
    ui.global::<BatchModel>().set_preview_remember_choice(false);
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
/// `thumbnail_cache` is the library card thumbnail cache
/// (`gui::batch::preview_cache::setup_preview_thumbnail_callback`'s return value); every
/// batch this wires up invalidates it per saved design so the cards refresh at once.
///
/// Called once, from `gui::build_main_window`, alongside every other `setup_*` call.
pub fn setup_preview_batch_callbacks(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
    thumbnail_cache: &PreviewThumbnailCache,
) {
    let handle: Rc<RefCell<Option<PreviewBatchHandle>>> = Rc::new(RefCell::new(None));
    // Shared by every `start_batch` call site below -- see that function's own doc
    // comment for why the completion push in `spawn_preview_batch` compares against
    // it.
    let current_batch_id: Arc<AtomicU64> = Arc::new(AtomicU64::new(0));

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
    // The remote coordinator panel's "Remote lanes for batches" spin box -- the lane
    // count both batches read when they start. Registered here, beside the other
    // batch-setting callbacks, rather than with the rest of the remote panel.
    setup_remote_batch_lanes(ui, settings_store);

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
            crate::gui::batch::regenerate_all::preview_dialog_closed(&ui);
        }
    });

    let settings_store_dismiss = Arc::clone(settings_store);
    let ui_weak_dismiss = ui.as_weak();
    ui.global::<BatchModel>().on_preview_dismiss_offer(move || {
        if let Some(ui) = ui_weak_dismiss.upgrade() {
            let model = ui.global::<BatchModel>();
            if model.get_preview_offer_from_import() && model.get_preview_remember_choice() {
                remember_choice(&ui, &settings_store_dismiss, ImportPreviewChoice::Skip);
            }
            model.set_preview_visible(false);
            model.set_preview_confirming(false);
            crate::gui::batch::regenerate_all::preview_dialog_closed(&ui);
        }
    });

    // The confirm step's two answers share everything but the mode they start.
    let env = Rc::new(ConfirmEnv {
        db: Arc::clone(db),
        render_ctx: Arc::clone(render_ctx),
        settings_store: Arc::clone(settings_store),
        slots: BatchSlots {
            handle_slot: Rc::clone(&handle),
            current_batch_id: Arc::clone(&current_batch_id),
            thumbnail_cache: thumbnail_cache.clone(),
        },
    });
    let (env_full, ui_weak_full) = (Rc::clone(&env), ui.as_weak());
    ui.global::<BatchModel>()
        .on_preview_generate_confirmed(move || {
            if let Some(ui) = ui_weak_full.upgrade() {
                confirm_offer(&ui, &env_full, PreviewMode::Full);
            }
        });
    let (env_solid, ui_weak_solid) = (env, ui.as_weak());
    ui.global::<BatchModel>()
        .on_preview_generate_solid_confirmed(move || {
            if let Some(ui) = ui_weak_solid.upgrade() {
                confirm_offer(&ui, &env_solid, PreviewMode::Solid);
            }
        });
    show_saved_choice(ui, settings_store);

    let db_entry = Arc::clone(db);
    let render_ctx_entry = Arc::clone(render_ctx);
    let settings_store_entry = Arc::clone(settings_store);
    let slots_entry = BatchSlots {
        handle_slot: Rc::clone(&handle),
        current_batch_id: Arc::clone(&current_batch_id),
        thumbnail_cache: thumbnail_cache.clone(),
    };
    let ui_weak_entry = ui.as_weak();
    ui.global::<LibraryModel>()
        .on_generate_previews_for_entry(move |id: i32| {
            if let Some(ui) = ui_weak_entry.upgrade() {
                start_batch(
                    &ui,
                    &db_entry,
                    &render_ctx_entry,
                    &settings_store_entry,
                    &slots_entry,
                    vec![i64::from(id)],
                    PreviewMode::Full,
                );
            }
        });

    let db_scan = Arc::clone(db);
    let scan_settings = settings_store.snapshot().settings;
    spawn_missing_preview_scan(
        ui.as_weak(),
        db_scan,
        scan_settings.preview_size,
        scan_settings.preview_spp,
        |ui, ids| {
            offer_batch_confirmation(ui, &ids);
        },
    );
}
