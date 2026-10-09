//! The per-item render engine and the local/remote lane runners that pull from a
//! shared [`WorkQueue`] -- see this group's own `mod.rs` doc comment's "Local +
//! remote" and "Why per-item resolve" sections for the design behind this file's
//! shape, and "Progress with N local lanes (plus remote) in flight" for the
//! [`LiveProgress`]/[`push_progress`] bookkeeping below.

use super::remote_lane::run_remote_lane;
use crate::{
    BatchModel, MainWindow,
    bridge::preview_render::{self, CacheKind, PreviewJob, PreviewView},
    gui::{
        batch::{
            batch_queue::WorkQueue,
            material_choice::ensure_balanced_material,
            remote_dispatch::{DispatcherGroup, RemoteStatus},
        },
        progress_eta::{EtaEstimator, batch_eta_label},
    },
    settings::WorkerSettings,
};
use indicatrix::{
    geometry::plane::GpuFacetPlane,
    optics::{materials::GemMaterial, raytracer::DEFAULT_MAX_BOUNCES},
    renderer::gpu_backend::GpuBackend,
};
use indicatrix_vault::db::sqlite::Database;
use slint::{ComponentHandle, Weak};
use std::{
    any::Any,
    collections::{BTreeSet, HashMap},
    panic::{self, AssertUnwindSafe},
    sync::{
        Mutex, PoisonError,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tracing::warn;

mod accumulator;

pub(super) use accumulator::{DesignAccum, RecordRevision};
use accumulator::{FinishedViews, record_item_result};

/// Every preview render's bounce cap -- fixed, not a settings-file field the way
/// `preview_size`/`preview_spp` are (see `bridge::preview_render`'s module doc comment
/// for why those two ARE exposed). It is the raytracer's [`DEFAULT_MAX_BOUNCES`] (this
/// app's own fresh-install live-viewport default) and the bounce count the sizing table
/// in `bridge::preview_render`'s doc comment was measured at, so the measured cost
/// figures there stay accurate.
pub(super) const PREVIEW_MAX_BOUNCES: u32 = DEFAULT_MAX_BOUNCES;

/// How close (absolute refractive-index difference) a `GemMaterial` preset must be to a
/// design's own scraped RI to count as a match for
/// `Database::ensure_preview_material`/`pick_ri_preset`. Chosen loosely rather than
/// measured: most named gem-species RI bands in `GemMaterial::all_materials()` are
/// separated by well over this (e.g. Quartz ~1.55 vs. Beryl ~1.58), while a handful of
/// distinct color varieties of the SAME species intentionally share (near-)identical
/// RI and are meant to tie (resolved by `gui::batch::material_choice`'s balanced
/// measure, not by chance) -- `0.02` sits comfortably inside a single species' natural RI spread
/// without being wide enough to blur two visually and physically distinct species
/// together.
///
/// `pub` (effectively crate-visible only, since this module tree is private -- clippy's
/// `redundant_pub_crate` prefers plain `pub` over `pub(crate)` here): `gui::batch::tilt`
/// resolves each design's material through `Database::ensure_preview_material`
/// too, and reuses this exact tolerance rather than an independently-tuned copy -- both
/// batches are picking a material for the SAME design via the SAME persisted-once-
/// reused-forever column (`Database::ensure_preview_material`'s own doc comment), so a
/// design's assigned material must not depend on which batch happened to run first.
pub const RI_MATCH_TOLERANCE: f64 = 0.02;

/// The refractive index a design with no parseable `refractive_index` on file is
/// matched against, so it still gets a stable, persisted preview material instead of
/// being skipped outright -- Diamond's own well-known sodium-D RI. A design's own
/// scraped RI (when present) always takes priority; this only ever applies to the
/// (rare, in the real ~3,187-design catalogue) rows where that field is missing or
/// unparseable.
///
/// `pub` (see [`RI_MATCH_TOLERANCE`]'s own note on why plain `pub` over `pub(crate)`
/// here): shared with `gui::batch::tilt` via `target_ri_for_design` below, same
/// reasoning as that constant's own doc comment.
pub const FALLBACK_TARGET_RI: f64 = 2.417;

/// A design's target refractive index for `Database::ensure_preview_material` matching
/// -- its own scraped `refractive_index` field when that parses as a finite, physically
/// plausible (`> 1.0`) number, [`FALLBACK_TARGET_RI`] otherwise. Factored out of
/// [`resolve_design`] (this module's own caller) so `gui::batch::tilt` computes a
/// design's target RI the identical way rather than re-deriving the parse/filter/
/// fallback chain a second time -- see [`RI_MATCH_TOLERANCE`]'s own doc comment for why
/// the two batches must agree on this.
#[must_use]
pub fn target_ri_for_design(full: &indicatrix_vault::model::entry::FullDiagramRecord) -> f64 {
    full.refractive_index
        .as_deref()
        .and_then(|s| s.trim().parse::<f64>().ok())
        .filter(|ri| ri.is_finite() && *ri > 1.0)
        .unwrap_or(FALLBACK_TARGET_RI)
}

/// How long the local lane sleeps before re-checking the queue when it finds nothing to
/// claim but the remote lane has not yet signalled it is done -- see `gui::batch::batch_queue`'s
/// module doc comment's "How the local lane knows the batch is truly finished" section.
/// Item durations here are ~1-2s, so a 15ms poll is immaterial to total batch time; it
/// only ever matters in the closing moments while local waits out the last item(s)
/// still in flight remotely.
const LOCAL_IDLE_POLL: Duration = Duration::from_millis(15);

/// Everything shared, read-only, across every design a batch processes AND safe to
/// share by plain reference into both the local and remote lanes -- deliberately holds
/// no `GpuBackend`: only the local lane ever touches a GPU adapter, and keeping it OUT
/// of this struct (passed as its own separate parameter to the local lane instead)
/// means this struct never needs to answer whether `GpuBackend` is `Sync` to be
/// borrowed across `std::thread::scope`. `db`/`material_candidates` are each acquired
/// exactly ONCE per batch (see `super::spawn_preview_batch`'s own doc comment for why),
/// and `preview_size`/`preview_spp` are the settings snapshot the WHOLE batch was
/// started with.
pub(super) struct BatchContext<'a> {
    pub(super) db: &'a Mutex<Database>,
    pub(super) material_candidates:
        &'a [indicatrix_vault::model::material_match::RiPresetCandidate],
    pub(super) preview_size: u32,
    pub(super) preview_spp: u32,
    /// Draw the fast solid stand-in pictures instead of the traced ones: no material is
    /// chosen, no GPU or remote lane is used, and the pictures are stored under a
    /// fingerprint that still counts the design as missing its traced preview.
    pub(super) solid: bool,
    /// Set once a caught local-render panic names a wgpu/mapped-buffer
    /// failure (see [`catch_local_render`]) and never cleared for the rest of this
    /// batch -- every local lane shares ONE `GpuBackend` (see this struct's own doc
    /// comment on why it deliberately holds no `GpuBackend` itself), so a panic that
    /// may have left the renderer's staging buffers mapped must stop EVERY lane from
    /// dispatching into it again, not just the lane it happened on. A plain
    /// `AtomicBool` (not a `GpuBackend::retire`/`mark_lost` call): `GpuBackend` itself
    /// exposes no such method, and its own internal `lost` flag is set only on a
    /// cleanly-reported `DeviceLost`, which a raw panic never reaches.
    pub(super) gpu_retired: &'a AtomicBool,
    /// The designs whose geometry came from the angle table rather than a design
    /// file -- see `gui::batch::record_planes_for_batch`. Read once at the end for
    /// the batch's summary line.
    pub(super) angle_table_entries: &'a Mutex<BTreeSet<i64>>,
}

/// One claimable unit of work: ONE view of ONE design -- see this group's `mod.rs` doc
/// comment's "Local + remote" section for why this is the item granularity (finer than
/// `gui::batch::tilt`'s own, which is a whole design).
#[derive(Debug, Clone, Copy)]
pub(super) struct PreviewItem {
    pub(super) entry_id: i64,
    pub(super) view: PreviewView,
}

/// One design's resolved geometry/material -- the shared prelude both
/// [`render_item_local`] and `remote_lane::render_item_remote`'s callers need before rendering
/// anything. See this group's `mod.rs` doc comment's "Why per-item resolve" section for
/// why there is deliberately no cache sharing this between a design's two items.
pub(super) struct ResolvedDesign {
    pub(super) title: String,
    pub(super) planes: Vec<GpuFacetPlane>,
    pub(super) material: GemMaterial,
    /// The record version `planes` and `material` were resolved from; hand it to
    /// [`finish_item_at_revision`] with the rendered bytes so a result from a superseded
    /// record is never saved.
    pub(super) revision: RecordRevision,
}

/// `full`'s facet planes for a preview render -- the design file first, the angle
/// table only as the fallback (`gui::batch::record_planes_for_batch`, shared with the
/// tilt batch and the library detail view). Both the local and the remote lane get
/// their planes here, through [`resolve_design`]; a remote render ships these planes
/// in its scene, so the worker never resolves a record itself.
pub(super) fn record_planes(
    ctx: &BatchContext<'_>,
    full: &indicatrix_vault::model::entry::FullDiagramRecord,
) -> Option<Vec<GpuFacetPlane>> {
    super::super::record_planes_for_batch(full, ctx.angle_table_entries)
}

/// Resolves `entry_id`'s geometry and preview material, or `None` if either step comes
/// up empty (malformed/unreadable row, unreconstructable geometry, or no material could
/// be matched/assigned).
pub(super) fn resolve_design(ctx: &BatchContext<'_>, entry_id: i64) -> Option<ResolvedDesign> {
    // The record and its revision stamp are read under one lock hold, so the stamp
    // describes exactly this record.
    let (full, updated_at) = {
        let guard = ctx.db.lock().unwrap_or_else(PoisonError::into_inner);
        (
            guard.get_diagram_full(entry_id),
            guard.entry_updated_at(entry_id),
        )
    };
    let Ok(Some(full)) = full else {
        return None;
    };
    let Ok(updated_at) = updated_at else {
        return None;
    };
    let planes = record_planes(ctx, &full)?;

    if ctx.solid {
        return Some(ResolvedDesign {
            title: full.title,
            planes,
            // Never shown: the solid picture is flat-shaded. The traced preview picks
            // the design's real material when it is rendered.
            material: GemMaterial::by_name("Diamond")?,
            revision: RecordRevision::Stamp(updated_at),
        });
    }
    let target_ri = target_ri_for_design(&full);
    let material_name = {
        let guard = ctx.db.lock().unwrap_or_else(PoisonError::into_inner);
        ensure_balanced_material(
            &guard,
            entry_id,
            target_ri,
            ctx.material_candidates,
            RI_MATCH_TOLERANCE,
            &planes,
        )
    };
    let Ok(Some(material_name)) = material_name else {
        return None;
    };
    // Through the render's size rule (a library design has no stone size): the same absorption
    // calibration the live view applies to a built-in's per-model-unit colour.
    let material = indicatrix::render_setup::material_for_stone(
        GemMaterial::by_name(&material_name)?,
        0.0,
        &planes,
    );

    Some(ResolvedDesign {
        title: full.title,
        planes,
        material,
        revision: RecordRevision::Stamp(updated_at),
    })
}

/// Downcasts a `catch_unwind` payload to a human-readable message -- the exact same
/// convention `gui::library::local::import::catch_file_panic` and
/// `bridge::export_thread::spawn_export` already use for this.
pub(super) fn panic_message(payload: &(dyn Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_string())
}

/// Runs `f` (one LOCAL view's render) under `catch_unwind`, so a panic tracing this one
/// view can never take the rest of the batch down with it -- see this group's `mod.rs`
/// doc comment's "Panic isolation" section -- and ALSO retires the shared `GpuBackend`
/// for the rest of this batch when the panic message names a wgpu/mapped-buffer failure
/// -- the exact class of panic traced to a failed GPU chunk readback
/// leaving a persistent staging buffer mapped, after which every LATER caller of the
/// same renderer hits `Queue::submit ... is still mapped` too. Every local lane shares
/// ONE `GpuBackend` for the whole batch (see [`BatchContext`]'s own doc comment), so
/// without this, a second lane taking its own turn right after the first one panicked
/// would dispatch straight into the same mapped state and panic again -- repeating for
/// every remaining item, which is exactly the "N identical panic-log entries" shape
/// a real crash log takes without this guard.
fn catch_local_render(
    view: PreviewView,
    gpu_retired: &AtomicBool,
    f: impl FnOnce() -> Option<Vec<u8>>,
) -> Option<Vec<u8>> {
    panic::catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|payload| {
        let message = panic_message(&*payload);
        if message.contains("wgpu") || message.contains("still mapped") {
            tracing::error!(
                "Preview render panicked for a {view:?} view with a GPU-fatal message; \
                 retiring the shared GPU backend for the rest of this batch: {message}"
            );
            gpu_retired.store(true, Ordering::Relaxed);
        } else {
            warn!("Preview render panicked for a {view:?} view: {message}");
        }
        None
    })
}

/// Renders `resolved`'s `view` on the LOCAL engine (GPU with CPU scanline fallback --
/// `bridge::preview_render::render_view`'s own doc comment). Declines outright, without
/// dispatching anything, once [`BatchContext::gpu_retired`] is set -- see
/// [`catch_local_render`]'s own doc comment for why calling back into the renderer at
/// that point risks repeating the same panic.
fn render_item_local(
    ctx: &BatchContext<'_>,
    gpu: &GpuBackend,
    resolved: &ResolvedDesign,
    view: PreviewView,
) -> Option<Vec<u8>> {
    if ctx.solid {
        return catch_local_render(view, ctx.gpu_retired, || {
            preview_render::render_view_solid(&resolved.planes, ctx.preview_size, view)
        });
    }
    if ctx.gpu_retired.load(Ordering::Relaxed) {
        return None;
    }
    let job = PreviewJob {
        planes: &resolved.planes,
        material: &resolved.material,
        size: ctx.preview_size,
        spp: ctx.preview_spp,
        max_bounces: PREVIEW_MAX_BOUNCES,
    };
    catch_local_render(view, ctx.gpu_retired, || {
        preview_render::render_view(&job, view, gpu)
    })
}

/// Running totals both lanes update concurrently -- plain atomics (not a `Mutex`)
/// suffice since `generated`/`failed` are independent counters with no invariant
/// between them that needs atomic coupling.
#[derive(Default)]
pub(super) struct Tally {
    pub(super) generated: AtomicU32,
    pub(super) failed: AtomicU32,
}

/// Live status all lanes update as they claim/finish items, and [`push_progress`] reads
/// to send one coherent snapshot to the UI thread -- see this group's `mod.rs` doc
/// comment's "Progress with N local lanes (plus remote) in flight" section for why
/// LOCAL is now a plain busy-count rather than a single title/image-index pair.
#[derive(Default, Clone)]
pub(super) struct LiveProgress {
    /// Designs whose BOTH views are fully accounted for so far, by any lane.
    completed: u32,
    /// How many local lanes currently have an item claimed -- out of
    /// `preview_batch_local_lane_total` (see [`push_progress`]'s `local_lane_total`
    /// parameter, fixed for the whole batch so it isn't duplicated into this
    /// per-update struct).
    local_active: u32,
    /// The remote dispatchers' side: how many run, how many items are on the remote,
    /// and the most recently started item's title. Changed only through
    /// [`update_remote`].
    remote: RemoteStatus,
    /// Fed `completed / design_total` on every [`push_progress`], so the remaining
    /// time reflects the mixed local + remote throughput of the whole batch.
    eta: EtaEstimator,
}

/// Pushes a snapshot of `progress` to the UI thread. Called by every lane after any
/// change to its own status. `local_lane_total` is fixed for the whole batch (the lane
/// count `super::spawn_preview_batch` decided via `batch_queue::local_lane_count`), so
/// it travels as a plain parameter here rather than living inside the per-update
/// [`LiveProgress`] -- the same reason `design_total` already does.
pub(super) fn push_progress(
    ui_weak: &Weak<MainWindow>,
    design_total: u32,
    local_lane_total: u32,
    progress: &Mutex<LiveProgress>,
) {
    let (snapshot, eta_text) = {
        let mut p = progress.lock().unwrap_or_else(PoisonError::into_inner);
        let now = std::time::Instant::now();
        if design_total > 0 {
            let fraction = f64::from(p.completed) / f64::from(design_total);
            p.eta.observe(now, fraction);
        }
        let eta_text = batch_eta_label(p.completed, design_total, p.eta.eta(now));
        (p.clone(), eta_text)
    };
    let ui_weak = ui_weak.clone();
    let _ = ui_weak.upgrade_in_event_loop(move |ui| {
        ui.global::<BatchModel>().set_preview_eta(eta_text.into());
        ui.global::<BatchModel>()
            .set_preview_design_index(snapshot.completed as i32);
        ui.global::<BatchModel>()
            .set_preview_design_total(design_total as i32);
        ui.global::<BatchModel>()
            .set_preview_local_active(snapshot.local_active as i32);
        ui.global::<BatchModel>()
            .set_preview_local_lane_total(local_lane_total as i32);
        ui.global::<BatchModel>()
            .set_preview_remote_title(snapshot.remote.title().into());
        ui.global::<BatchModel>()
            .set_preview_remote_active(snapshot.remote.is_active());
        ui.global::<BatchModel>()
            .set_preview_remote_in_flight(snapshot.remote.in_flight() as i32);
    });
}

/// Applies `change` to the shared remote status -- every remote dispatcher goes through
/// here, so no two of them can interleave a half-finished update.
pub(super) fn update_remote(
    progress: &Mutex<LiveProgress>,
    change: impl FnOnce(&mut RemoteStatus),
) {
    change(
        &mut progress
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remote,
    );
}

fn increment_completed(progress: &Mutex<LiveProgress>) {
    progress
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .completed += 1;
}

/// Marks one local lane as having just claimed an item (`+1`) or having just finished
/// one (`-1`) -- see `gui::batch::tilt::adjust_local_active`'s identical doc comment for
/// why a signed step rather than two near-identical increment/decrement functions.
fn adjust_local_active(progress: &Mutex<LiveProgress>, delta: i32) {
    let mut p = progress.lock().unwrap_or_else(PoisonError::into_inner);
    p.local_active = p.local_active.saturating_add_signed(delta);
}

/// Everything both lane-runner functions need that is safe to share by plain reference
/// across the `std::thread::scope` in `super::spawn_preview_batch` -- deliberately
/// excludes `GpuBackend` (see [`BatchContext`]'s own doc comment) and
/// `Weak<MainWindow>` (each lane gets its own owned clone instead, passed separately,
/// so this struct never needs to answer whether Slint's `Weak` is `Sync`). Also bundles
/// the arguments [`finish_item`] needs, keeping that function (called from every lane)
/// under clippy's argument-count limit.
pub(super) struct LaneShared<'a> {
    pub(super) ctx: &'a BatchContext<'a>,
    pub(super) queue: &'a WorkQueue<PreviewItem>,
    pub(super) design_state: &'a Mutex<HashMap<i64, DesignAccum>>,
    pub(super) tally: &'a Tally,
    pub(super) progress: &'a Mutex<LiveProgress>,
    pub(super) cancel: &'a AtomicBool,
    pub(super) design_total: u32,
    /// How many local lanes this batch spawned -- see `batch_queue::local_lane_count`'s
    /// own doc comment. Fixed for the whole batch; threaded through here (rather than
    /// read fresh by each lane) purely so every `push_progress` call site has it without
    /// needing its own separate parameter.
    pub(super) local_lane_total: u32,
    /// How many remote dispatchers this batch spawns -- see
    /// `batch_queue::remote_lane_count`. `0` when no remote lane runs (`LocalOnly`).
    pub(super) remote_lane_total: u32,
    /// Pictures one batched remote request carries (`AppSettings::remote_preview_batch_size`,
    /// protocol v24); `remote_lane::remote_batch` claims that many at a time.
    pub(super) remote_batch_size: usize,
    /// Set by the first remote dispatcher to start sitting a failing remote out, so the
    /// "remote lane paused" toast shows once per batch however many dispatchers (each
    /// with its own failure count) reach their own sit-out.
    pub(super) sit_out_toasted: AtomicBool,
    /// The library card thumbnail cache, invalidated per design in [`finish_item`] once
    /// its previews are saved.
    pub(super) thumbnail_cache: &'a crate::gui::batch::preview_cache::PreviewThumbnailCache,
}

/// Stores a finished design's pair, unless the design changed while it was rendered.
/// Returns whether the pair was stored.
///
/// The images go in only if the catalogue row still carries the `updated_at` the items
/// resolved their record at (`Database::save_preview_images`'s compare-and-swap), so a
/// re-import, metadata edit or re-sync that lands during the seconds of rendering is
/// never followed by a write of the old geometry's pictures. A design whose two items
/// disagree about the record version is refused the same way. An item pair that never
/// reported a revision (`RecordRevision::Unknown`) is saved against the row's current
/// stamp, i.e. without that protection.
fn save_finished_design(shared: &LaneShared<'_>, entry_id: i64, done: &FinishedViews) -> bool {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX));
    let guard = shared.ctx.db.lock().unwrap_or_else(PoisonError::into_inner);
    let expected_updated_at = match done.revision {
        RecordRevision::Stamp(stamp) => stamp,
        RecordRevision::Unknown => match guard.entry_updated_at(entry_id) {
            Ok(stamp) => stamp,
            Err(e) => {
                warn!("Could not save the previews for entry {entry_id}: {e}");
                return false;
            }
        },
        RecordRevision::Conflicting => {
            warn!(
                "Discarded the previews rendered for entry {entry_id}: the design changed \
                 between its two views"
            );
            return false;
        }
    };
    let material = guard.get_preview_material(entry_id).ok().flatten();
    let kind = if shared.ctx.solid {
        CacheKind::SolidDraft {
            size: shared.ctx.preview_size,
        }
    } else {
        CacheKind::Preview {
            size: shared.ctx.preview_size,
            spp: shared.ctx.preview_spp,
            max_bounces: PREVIEW_MAX_BOUNCES,
        }
    };
    let fingerprint = preview_render::cache_fingerprint(kind, material.as_deref());
    let stored = guard.save_preview_images(
        entry_id,
        done.front.as_deref(),
        done.top.as_deref(),
        now,
        &fingerprint,
        expected_updated_at,
    );
    drop(guard);
    match stored {
        Ok(true) => true,
        Ok(false) => {
            warn!(
                "Discarded the previews rendered for entry {entry_id}: the design changed \
                 while they were rendering"
            );
            false
        }
        Err(e) => {
            warn!("Failed to save the previews for entry {entry_id}: {e}");
            false
        }
    }
}

/// [`finish_item_at_revision`] for an item with no record revision to report. Right for
/// a failure hand-back (`bytes` is `None`, so there is nothing to protect); a caller
/// holding rendered bytes should pass its design's [`ResolvedDesign::revision`] to
/// [`finish_item_at_revision`] instead, since an item without one leaves its half of the
/// pair unchecked against edits made during the render.
pub(super) fn finish_item(
    shared: &LaneShared<'_>,
    ui_weak: &Weak<MainWindow>,
    entry_id: i64,
    view: PreviewView,
    bytes: Option<Vec<u8>>,
) {
    finish_item_at_revision(
        shared,
        ui_weak,
        entry_id,
        view,
        bytes,
        RecordRevision::Unknown,
    );
}

/// Persists a finished design's front/top pair (whatever combination succeeded) and
/// folds the result into `shared.tally`/`shared.progress` -- called once a design's
/// LAST outstanding item comes back, from whichever lane that happens to be (see
/// [`record_item_result`]). `revision` is the record version this item's `bytes` were
/// rendered from ([`ResolvedDesign::revision`]); see [`save_finished_design`] for how
/// the design's items' revisions gate the save.
pub(super) fn finish_item_at_revision(
    shared: &LaneShared<'_>,
    ui_weak: &Weak<MainWindow>,
    entry_id: i64,
    view: PreviewView,
    bytes: Option<Vec<u8>>,
    revision: RecordRevision,
) {
    let Some(done) = record_item_result(shared.design_state, entry_id, view, bytes, revision)
    else {
        // This design still has its other view in flight elsewhere -- nothing to save
        // or tally yet.
        return;
    };

    let saved = (done.front.is_some() || done.top.is_some())
        && save_finished_design(shared, entry_id, &done);

    if saved {
        shared.tally.generated.fetch_add(1, Ordering::Relaxed);
        // The library card re-queries this design's thumbnails on its next frame,
        // replacing the placeholder or the stale image.
        shared.thumbnail_cache.invalidate(ui_weak, entry_id);
    } else {
        shared.tally.failed.fetch_add(1, Ordering::Relaxed);
    }
    increment_completed(shared.progress);
    push_progress(
        ui_weak,
        shared.design_total,
        shared.local_lane_total,
        shared.progress,
    );
}

/// Runs one LOCAL lane out of `shared.local_lane_total`: claims items via
/// `WorkQueue::claim_local` (retried-after-remote-failure items first, then fresh ones)
/// until both the retry pile and the shared pool are empty AND `remote_lane_done`
/// confirms no more work is coming -- see `gui::batch::batch_queue`'s module doc
/// comment for why that combination, not just "queue empty", is the correct stop
/// condition. Called once per local lane by `super::spawn_preview_batch` (see this
/// group's `mod.rs` doc comment's "Local + remote" section); `gpu` is one `GpuBackend`
/// shared by EVERY local lane (acquired once for the whole batch -- see
/// `super::spawn_preview_batch`'s own comment on why one adapter, not one per lane),
/// safe to call concurrently because its dispatches serialize on the GPU's own queue
/// (`GpuBackend`'s own module doc comment's "Concurrency" section).
pub(super) fn run_local_lane(
    shared: &LaneShared<'_>,
    gpu: &GpuBackend,
    ui_weak: &Weak<MainWindow>,
    remote_lane_done: &AtomicBool,
) {
    loop {
        if shared.cancel.load(Ordering::Relaxed) {
            break;
        }
        // Read BEFORE the claim: a remote dispatcher raises the flag only after its last
        // requeue, so a claim that follows an observed `true` sees every requeue and an
        // empty answer is final. Read after the claim, a requeue landing between the two
        // would be stranded.
        let remote_finished = remote_lane_done.load(Ordering::Acquire);
        let Some(item) = shared.queue.claim_local() else {
            if remote_finished {
                break;
            }
            thread::sleep(LOCAL_IDLE_POLL);
            continue;
        };

        adjust_local_active(shared.progress, 1);
        push_progress(
            ui_weak,
            shared.design_total,
            shared.local_lane_total,
            shared.progress,
        );

        let resolved = resolve_design(shared.ctx, item.entry_id);
        let revision = resolved
            .as_ref()
            .map_or(RecordRevision::Unknown, |r| r.revision);
        let bytes = resolved.and_then(|r| render_item_local(shared.ctx, gpu, &r, item.view));
        finish_item_at_revision(shared, ui_weak, item.entry_id, item.view, bytes, revision);

        adjust_local_active(shared.progress, -1);
        push_progress(
            ui_weak,
            shared.design_total,
            shared.local_lane_total,
            shared.progress,
        );
    }
}

/// One [`PreviewItem`] per view (front, then top) of every id in `entry_ids` -- the
/// initial contents of `super::spawn_preview_batch`'s shared queue.
pub(super) fn build_items(entry_ids: &[i64]) -> Vec<PreviewItem> {
    entry_ids
        .iter()
        .flat_map(|&entry_id| {
            [
                PreviewItem {
                    entry_id,
                    view: PreviewView::Front,
                },
                PreviewItem {
                    entry_id,
                    view: PreviewView::Top,
                },
            ]
        })
        .collect()
}

/// Runs every lane for one batch attempt inside a `std::thread::scope`, blocking until
/// all of them finish. Factored out of `super::spawn_preview_batch` purely to keep
/// that function under clippy's line-count limit -- every parameter here is already a
/// reference (or `Copy`), so each `scope.spawn(move || ...)` closure below simply moves
/// a cheap copy of it, with no further indirection needed the way `spawn_preview_batch`
/// itself would have needed had it kept this block inline (see `gui::batch::tilt`'s
/// identical helper for the full reasoning).
///
/// `shared.remote_lane_total` remote dispatchers run at once, all claiming from the same
/// queue; `remote_lane_done` is raised when the last of them ends.
pub(super) fn run_batch_lanes(
    shared: &LaneShared<'_>,
    gpu: &GpuBackend,
    plan: super::super::batch_queue::LanePlan,
    remote_worker: Option<&WorkerSettings>,
    local_lane_total: u32,
    ui_weak: &Weak<MainWindow>,
    remote_lane_done: &AtomicBool,
) {
    // Built before the scope: the scoped dispatchers borrow it for the scope's whole
    // lifetime.
    let dispatchers = DispatcherGroup::new(shared.remote_lane_total as usize, remote_lane_done);
    let dispatchers = &dispatchers;
    std::thread::scope(|scope| {
        if plan.run_remote {
            for _ in 0..shared.remote_lane_total {
                let remote_ui_weak = ui_weak.clone();
                scope.spawn(move || {
                    run_remote_lane(
                        shared,
                        &remote_ui_weak,
                        remote_worker,
                        plan.fallback_to_local,
                        dispatchers,
                    );
                });
            }
        }
        if plan.run_local {
            // N independently-claiming local lanes, all sharing the one `gpu` acquired
            // by the caller -- see `run_local_lane`'s own doc comment for why sharing
            // one `GpuBackend` across lanes is safe, and this group's `mod.rs` doc
            // comment's "Local + remote" section for why running more than one local
            // lane needed no new coordination beyond `WorkQueue` itself.
            for _ in 0..local_lane_total {
                let local_ui_weak = ui_weak.clone();
                scope.spawn(move || {
                    run_local_lane(shared, gpu, &local_ui_weak, remote_lane_done);
                });
            }
        }
    });
}
