//! The per-item render engine and the local/remote lane runners that pull from a
//! shared [`WorkQueue`] -- see this group's own `mod.rs` doc comment's "Local +
//! remote" and "Why per-item resolve" sections for the design behind this file's
//! shape, and "Progress with N local lanes (plus remote) in flight" for the
//! [`LiveProgress`]/[`push_progress`] bookkeeping below.

use crate::{
    BatchModel, MainWindow,
    bridge::preview_render::{self, PreviewJob, PreviewView},
    gui::{batch::batch_queue::WorkQueue, library::detail::reconstruct_planes},
    settings::WorkerSettings,
};
use indicatrix::{
    geometry::{cuts::FacetSpec, plane::GpuFacetPlane},
    optics::materials::GemMaterial,
    renderer::gpu_backend::GpuBackend,
};
use indicatrix_vault::db::sqlite::Database;
use slint::{ComponentHandle, Weak};
use std::{
    any::Any,
    collections::HashMap,
    panic::{self, AssertUnwindSafe},
    sync::{
        Mutex, PoisonError,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tracing::warn;

/// Every preview render's bounce cap -- fixed, not a settings-file field the way
/// `preview_size`/`preview_spp` are (see `bridge::preview_render`'s module doc comment
/// for why those two ARE exposed). `12` matches `settings::model::app_settings::
/// DEFAULT_MAX_BOUNCES` (this app's own fresh-install live-viewport default) and the
/// bounce count the sizing table in `bridge::preview_render`'s doc comment was measured
/// at, so the measured cost figures there stay accurate.
const PREVIEW_MAX_BOUNCES: u32 = 12;

/// How close (absolute refractive-index difference) a `GemMaterial` preset must be to a
/// design's own scraped RI to count as a match for
/// `Database::ensure_preview_material`/`pick_ri_preset`. Chosen loosely rather than
/// measured: most named gem-species RI bands in `GemMaterial::all_materials()` are
/// separated by well over this (e.g. Quartz ~1.55 vs. Beryl ~1.58), while a handful of
/// distinct colour varieties of the SAME species intentionally share (near-)identical
/// RI and are meant to tie (see `pick_ri_preset`'s own doc comment on ties resolving by
/// random draw) -- `0.02` sits comfortably inside a single species' natural RI spread
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

/// A tiny non-cryptographic splitmix64-based generator producing values in `[0.0,
/// 1.0)`, seeded from `entry_id` and the current time -- this crate has no `rand`
/// dependency (nothing else in this workspace needs one; `indicatrix`'s own sampling is a
/// deterministic hash of pixel/sample indices, not a general-purpose RNG), and
/// `indicatrix_vault::model::material_match::pick_ri_preset`'s `random_unit` contract
/// (see that function's own doc comment) only ever needs ONE low-stakes draw per design
/// in that design's ENTIRE lifetime, to break a tie between visually-similar material
/// presets -- nowhere near a use that would justify adding a real RNG crate dependency
/// for. Mixing in wall-clock time (not just `entry_id`) keeps two designs processed in
/// the same batch, or the same design re-processed after a future reset, from drawing
/// identically every time.
///
/// `pub` (see [`RI_MATCH_TOLERANCE`]'s own note on why plain `pub` over `pub(crate)`
/// here): `gui::batch::tilt` reuses this directly for its own
/// `Database::ensure_preview_material` calls -- see that constant's own doc comment
/// for why both batches must resolve a design's material the same way.
pub fn seeded_random_unit(entry_id: i64) -> impl FnMut() -> f64 {
    let time_bits = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64);
    #[allow(
        clippy::cast_sign_loss,
        reason = "entry_id is a SQLite AUTOINCREMENT row id, always non-negative in \
                  practice; this cast only feeds a hash seed, where a wrapped negative \
                  id would still produce a valid (if different) seed rather than a \
                  wrong answer"
    )]
    let mut state = (entry_id as u64) ^ time_bits;
    move || {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        // Top 53 bits -> [0.0, 1.0), matching an `f64` mantissa's full precision --
        // the same "shift then scale" idiom most splitmix64-derived float generators
        // use.
        (z >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }
}

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
/// [`render_item_local`] and [`render_item_remote`]'s callers need before rendering
/// anything. See this group's `mod.rs` doc comment's "Why per-item resolve" section for
/// why there is deliberately no cache sharing this between a design's two items.
struct ResolvedDesign {
    title: String,
    planes: Vec<GpuFacetPlane>,
    material: GemMaterial,
}

/// Resolves `entry_id`'s geometry and preview material, or `None` if either step comes
/// up empty (malformed/unreadable row, unreconstructable geometry, or no material could
/// be matched/assigned).
fn resolve_design(ctx: &BatchContext<'_>, entry_id: i64) -> Option<ResolvedDesign> {
    let full = {
        let guard = ctx.db.lock().unwrap_or_else(PoisonError::into_inner);
        guard.get_diagram_full(entry_id)
    };
    let Ok(Some(full)) = full else {
        return None;
    };

    let facet_specs: Vec<FacetSpec> = full
        .angle_settings
        .iter()
        .map(|a| FacetSpec {
            facet: a.facet.clone(),
            angle: a.angle.clone(),
            index: a.index.clone(),
            notes: a.notes.clone(),
        })
        .collect();
    let planes = reconstruct_planes(
        full.shape.as_deref(),
        full.index_gear.as_deref(),
        &facet_specs,
    );
    if planes.is_empty() {
        return None;
    }

    let target_ri = target_ri_for_design(&full);
    let material_name = {
        let guard = ctx.db.lock().unwrap_or_else(PoisonError::into_inner);
        let mut rng = seeded_random_unit(entry_id);
        guard.ensure_preview_material(
            entry_id,
            target_ri,
            ctx.material_candidates,
            RI_MATCH_TOLERANCE,
            &mut rng,
        )
    };
    let Ok(Some(material_name)) = material_name else {
        return None;
    };
    let material = GemMaterial::by_name(&material_name)?;

    Some(ResolvedDesign {
        title: full.title,
        planes,
        material,
    })
}

/// Downcasts a `catch_unwind` payload to a human-readable message -- the exact same
/// convention `gui::library::local::import::catch_file_panic` and
/// `bridge::export_thread::spawn_export` already use for this.
fn panic_message(payload: &(dyn Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_string())
}

/// Runs `f` (a single view's render, local or remote) under `catch_unwind`, so a panic
/// tracing this one view can never take the rest of the batch down with it -- see this
/// group's `mod.rs` doc comment's "Panic isolation" section.
fn catch_render(view: PreviewView, f: impl FnOnce() -> Option<Vec<u8>>) -> Option<Vec<u8>> {
    panic::catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|payload| {
        warn!(
            "Preview render panicked for a {view:?} view: {}",
            panic_message(&*payload)
        );
        None
    })
}

/// Renders `resolved`'s `view` on the LOCAL engine (GPU with CPU scanline fallback --
/// `bridge::preview_render::render_view`'s own doc comment).
fn render_item_local(
    ctx: &BatchContext<'_>,
    gpu: &GpuBackend,
    resolved: &ResolvedDesign,
    view: PreviewView,
) -> Option<Vec<u8>> {
    let job = PreviewJob {
        planes: &resolved.planes,
        material: &resolved.material,
        size: ctx.preview_size,
        spp: ctx.preview_spp,
        max_bounces: PREVIEW_MAX_BOUNCES,
    };
    catch_render(view, || preview_render::render_view(&job, view, gpu))
}

/// Renders `resolved`'s `view` against `worker` (`bridge::preview_render::
/// render_view_remote`'s own doc comment). Returns `None` on ANY shortfall, per that
/// function's own contract.
fn render_item_remote(
    ctx: &BatchContext<'_>,
    worker: &WorkerSettings,
    resolved: &ResolvedDesign,
    view: PreviewView,
    cancel: &AtomicBool,
) -> Option<Vec<u8>> {
    let job = PreviewJob {
        planes: &resolved.planes,
        material: &resolved.material,
        size: ctx.preview_size,
        spp: ctx.preview_spp,
        max_bounces: PREVIEW_MAX_BOUNCES,
    };
    catch_render(view, || {
        preview_render::render_view_remote(&job, view, worker, cancel)
    })
}

/// One design's front/top views as they trickle in from (possibly) two different
/// lanes, plus how many of its (always 2) items are still outstanding. Lives in
/// `super::spawn_preview_batch`'s shared `design_state` map for exactly as long as at
/// least one of a design's two items hasn't finished yet -- [`record_item_result`]
/// removes the entry the moment `remaining` reaches `0` and hands the finished pair to
/// its caller for saving.
pub(super) struct DesignAccum {
    front: Option<Vec<u8>>,
    top: Option<Vec<u8>>,
    remaining: u8,
}

/// A finished design's front/top pair, ready to save -- `record_item_result`'s return
/// type, factored into a named alias purely to keep that signature legible (clippy's
/// `type_complexity` lint).
type FinishedViews = Option<(Option<Vec<u8>>, Option<Vec<u8>>)>;

/// Records one item's result (`bytes`, `None` on any failure) against `entry_id`'s
/// accumulator, creating it on first touch. Returns `Some((front, top))` -- ready to
/// save -- the moment this was the design's LAST outstanding item, regardless of which
/// lane produced either result; `None` while the design still has an item in flight
/// elsewhere.
fn record_item_result(
    design_state: &Mutex<HashMap<i64, DesignAccum>>,
    entry_id: i64,
    view: PreviewView,
    bytes: Option<Vec<u8>>,
) -> FinishedViews {
    let mut map = design_state.lock().unwrap_or_else(PoisonError::into_inner);
    let accum = map.entry(entry_id).or_insert_with(|| DesignAccum {
        front: None,
        top: None,
        remaining: 2,
    });
    match view {
        PreviewView::Front => accum.front = bytes,
        PreviewView::Top => accum.top = bytes,
    }
    accum.remaining = accum.remaining.saturating_sub(1);
    if accum.remaining > 0 {
        return None;
    }
    map.remove(&entry_id).map(|done| (done.front, done.top))
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
    remote_title: String,
    /// Whether the remote lane is running at all this batch -- distinct from
    /// `remote_title` being empty, which also happens briefly between two remote
    /// items while the lane is very much still active.
    remote_active: bool,
}

/// Pushes a snapshot of `progress` to the UI thread. Called by every lane after any
/// change to its own status. `local_lane_total` is fixed for the whole batch (the lane
/// count `super::spawn_preview_batch` decided via `batch_queue::local_lane_count`), so
/// it travels as a plain parameter here rather than living inside the per-update
/// [`LiveProgress`] -- the same reason `design_total` already does.
fn push_progress(
    ui_weak: &Weak<MainWindow>,
    design_total: u32,
    local_lane_total: u32,
    progress: &Mutex<LiveProgress>,
) {
    let snapshot = progress
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    let ui_weak = ui_weak.clone();
    let _ = ui_weak.upgrade_in_event_loop(move |ui| {
        ui.global::<BatchModel>()
            .set_preview_design_index(snapshot.completed as i32);
        ui.global::<BatchModel>()
            .set_preview_design_total(design_total as i32);
        ui.global::<BatchModel>()
            .set_preview_local_active(snapshot.local_active as i32);
        ui.global::<BatchModel>()
            .set_preview_local_lane_total(local_lane_total as i32);
        ui.global::<BatchModel>()
            .set_preview_remote_title(snapshot.remote_title.into());
        ui.global::<BatchModel>()
            .set_preview_remote_active(snapshot.remote_active);
    });
}

fn set_remote_status(progress: &Mutex<LiveProgress>, active: bool, title: &str) {
    let mut p = progress.lock().unwrap_or_else(PoisonError::into_inner);
    p.remote_active = active;
    p.remote_title = title.to_string();
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
}

/// Persists a finished design's front/top pair (whatever combination succeeded) and
/// folds the result into `shared.tally`/`shared.progress` -- called once a design's
/// LAST outstanding item comes back, from whichever lane that happens to be (see
/// [`record_item_result`]).
fn finish_item(
    shared: &LaneShared<'_>,
    ui_weak: &Weak<MainWindow>,
    entry_id: i64,
    view: PreviewView,
    bytes: Option<Vec<u8>>,
) {
    let Some((front, top)) = record_item_result(shared.design_state, entry_id, view, bytes) else {
        // This design still has its other view in flight elsewhere -- nothing to save
        // or tally yet.
        return;
    };

    let saved = (front.is_some() || top.is_some()) && {
        #[allow(
            clippy::cast_possible_wrap,
            reason = "unix seconds fits in i64 until well past the year 292 billion; \
                      the column this feeds (`diagram_previews.preview_generated_at`) \
                      is already declared INTEGER (i64) to match"
        )]
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        let guard = shared.ctx.db.lock().unwrap_or_else(PoisonError::into_inner);
        guard
            .save_preview_images(entry_id, front.as_deref(), top.as_deref(), now)
            .is_ok()
    };

    if saved {
        shared.tally.generated.fetch_add(1, Ordering::Relaxed);
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
        let Some(item) = shared.queue.claim_local() else {
            if remote_lane_done.load(Ordering::Acquire) {
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
        let bytes = resolved.and_then(|r| render_item_local(shared.ctx, gpu, &r, item.view));
        finish_item(shared, ui_weak, item.entry_id, item.view, bytes);

        adjust_local_active(shared.progress, -1);
        push_progress(
            ui_weak,
            shared.design_total,
            shared.local_lane_total,
            shared.progress,
        );
    }
}

/// Runs the REMOTE lane: claims fresh items via `WorkQueue::claim_shared` only (never
/// a local-retried one -- that pile is reserved for the local lane, see
/// `gui::batch::batch_queue`'s doc comment) until the shared pool is empty, then
/// signals `remote_lane_done` so the local lane knows no further requeues are coming.
///
/// `worker` is `None` only for `RemoteOnly` with no worker configured -- treated as an
/// immediate shortfall (no attempt possible) rather than a connection failure, so this
/// never touches the network in that case. `fallback_to_local` is `true` only for
/// `LiveComputeTarget::Both` -- for `RemoteOnly` every failure is final and tallied
/// `failed` directly, per this task's own "must not silently fall back to local"
/// requirement.
pub(super) fn run_remote_lane(
    shared: &LaneShared<'_>,
    ui_weak: &Weak<MainWindow>,
    worker: Option<&WorkerSettings>,
    fallback_to_local: bool,
    remote_lane_done: &AtomicBool,
) {
    loop {
        if shared.cancel.load(Ordering::Relaxed) {
            break;
        }
        let Some(item) = shared.queue.claim_shared() else {
            break;
        };

        let resolved = resolve_design(shared.ctx, item.entry_id);
        let title = resolved
            .as_ref()
            .map_or_else(|| format!("Design #{}", item.entry_id), |r| r.title.clone());
        set_remote_status(shared.progress, true, &title);
        push_progress(
            ui_weak,
            shared.design_total,
            shared.local_lane_total,
            shared.progress,
        );

        let bytes = match (resolved, worker) {
            (Some(r), Some(w)) => render_item_remote(shared.ctx, w, &r, item.view, shared.cancel),
            _ => None,
        };

        if bytes.is_some() {
            finish_item(shared, ui_weak, item.entry_id, item.view, bytes);
        } else if fallback_to_local {
            // Not accounted for yet -- the local lane will attempt this exact item
            // next, and IT is the one that finally tallies/completes its design. See
            // `gui::batch::batch_queue`'s module doc comment for why this never goes
            // back to the shared pool.
            shared.queue.return_to_local(item);
        } else {
            // `RemoteOnly`: no local fallback exists, so this failure is final.
            finish_item(shared, ui_weak, item.entry_id, item.view, None);
        }
    }
    set_remote_status(shared.progress, false, "");
    push_progress(
        ui_weak,
        shared.design_total,
        shared.local_lane_total,
        shared.progress,
    );
    // Ordered AFTER every possible `return_to_local` call above (all inside the loop
    // this follows) -- see `gui::batch::batch_queue`'s doc comment for why the local
    // lane's own stop condition depends on that ordering.
    remote_lane_done.store(true, Ordering::Release);
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
pub(super) fn run_batch_lanes(
    shared: &LaneShared<'_>,
    gpu: &GpuBackend,
    plan: super::super::batch_queue::LanePlan,
    remote_worker: Option<&WorkerSettings>,
    local_lane_total: u32,
    ui_weak: &Weak<MainWindow>,
    remote_lane_done: &AtomicBool,
) {
    std::thread::scope(|scope| {
        if plan.run_remote {
            let remote_ui_weak = ui_weak.clone();
            scope.spawn(move || {
                run_remote_lane(
                    shared,
                    &remote_ui_weak,
                    remote_worker,
                    plan.fallback_to_local,
                    remote_lane_done,
                );
            });
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
