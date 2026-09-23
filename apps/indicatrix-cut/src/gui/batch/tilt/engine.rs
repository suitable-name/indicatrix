//! The per-design resolve/compute/dispatch engine and the local/remote lane runners
//! that pull from a shared [`WorkQueue`] -- see this group's own `mod.rs` doc comment
//! for the design behind this file's shape.

use crate::{
    BatchModel,
    bridge::{
        preview_render::{PREVIEW_LIGHT_PITCH, PREVIEW_LIGHT_YAW},
        remote::remote_render,
    },
    gui::{
        batch::{
            batch_queue::WorkQueue,
            preview::{RI_MATCH_TOLERANCE, seeded_random_unit, target_ri_for_design},
        },
        library::detail::reconstruct_planes,
    },
    settings::WorkerSettings,
};
use indicatrix::{
    color::metrics::{PROFILE_AZIMUTHS_DEG, evaluate_full_axis_profile_at_azimuth},
    geometry::plane::GpuFacetPlane,
    optics::{materials::GemMaterial, raytracer::LightingPreset},
};
use indicatrix_net::{
    SceneState,
    messages::{AxisTiltCurves as WireAxisTiltCurves, TiltCurvesRequest, TiltCurvesResponse},
};
use indicatrix_vault::model::tilt_curves::{
    AxisTiltCurves as StorageAxisTiltCurves, TILT_CURVE_AXIS_COUNT, TILT_CURVE_POINTS_PER_AXIS,
    TiltPerformanceCurves,
};
use slint::{ComponentHandle, Weak};
use std::{
    any::Any,
    sync::{
        Mutex, PoisonError,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tracing::warn;

/// `max_bounces` on the [`SceneState`] a remote tilt-curve request carries -- IGNORED
/// by the worker's `TILT_CURVES` handler entirely (see `indicatrix_net::messages::tilt`'s
/// module doc comment: only `planes`/`material`/`light_yaw`/`light_pitch` actually feed
/// the computation), but still validated, so this must be a plausible value
/// (`1..=128`, see `apps/indicatrix-worker::validate::MAX_BOUNCES`). `12` matches
/// `gui::batch::preview::engine::PREVIEW_MAX_BOUNCES` purely for familiarity -- no
/// render this batch performs ever actually bounces a ray, so the specific value is
/// otherwise arbitrary.
const REMOTE_SCENE_MAX_BOUNCES: u32 = 12;

/// A single fixed request id for every tilt-curve remote dispatch -- safe for the same
/// reason `bridge::preview_render::PREVIEW_REQUEST_ID` gives: each dispatch opens its
/// OWN fresh one-shot connection (see [`fetch_tilt_curves_remote`]), so nothing is ever
/// pipelined behind an unrelated request on the same socket.
const TILT_REQUEST_ID: u32 = 1;

/// How long the local lane sleeps before re-checking the queue when it finds nothing to
/// claim but the remote lane has not yet signalled it is done -- see `gui::batch::batch_queue`'s
/// module doc comment's "How the local lane knows the batch is truly finished" section.
/// Item durations here are ~1.36s local / a remote round trip of similar order, so a
/// 15ms poll is immaterial to total batch time; it only ever matters in the closing
/// moments while local waits out the last design(s) still in flight remotely.
const LOCAL_IDLE_POLL: Duration = Duration::from_millis(15);

/// Everything shared, read-only, across every design a batch processes -- same
/// bundling reasoning as `gui::batch::preview::engine::BatchContext`. Deliberately
/// holds no worker/queue/progress state: those are per-lane concerns bundled
/// separately in [`LaneShared`] so a plain `&BatchContext` stays `Sync` on its own
/// terms (every field here is a shared reference to already-`Sync` data), which is
/// what lets both lanes borrow it across the `std::thread::scope` in
/// `super::spawn_tilt_batch` with no `Arc` needed.
pub(super) struct BatchContext<'a> {
    pub(super) db: &'a Mutex<indicatrix_vault::db::sqlite::Database>,
    pub(super) material_candidates:
        &'a [indicatrix_vault::model::material_match::RiPresetCandidate],
}

/// One design's resolved geometry/material -- the shared prelude both
/// [`process_local_entry`] and [`process_remote_entry`] need before they can do
/// anything engine-specific. Since this batch's item granularity is already "one whole
/// design" (unlike `gui::batch::preview`'s per-VIEW items), there is exactly one
/// resolve per design per lane attempt -- no duplicate-resolve cost to reason about
/// here the way there is in that module.
struct ResolvedDesign {
    title: String,
    planes: Vec<GpuFacetPlane>,
    material: GemMaterial,
}

/// Resolves `entry_id`'s geometry and preview material, or `None` if either step comes
/// up empty (malformed/unreadable row, unreconstructable geometry, or no material could
/// be matched/assigned) -- mirrors `gui::batch::preview::engine`'s own resolve prelude
/// exactly, factored out here since BOTH lanes need it independently (a design claimed
/// by remote and a design claimed by local each resolve their own copy; nothing about a
/// design's resolved geometry is shared or cached across lanes).
fn resolve_design(ctx: &BatchContext<'_>, entry_id: i64) -> Option<ResolvedDesign> {
    let full = {
        let guard = ctx.db.lock().unwrap_or_else(PoisonError::into_inner);
        guard.get_diagram_full(entry_id)
    };
    let Ok(Some(full)) = full else {
        return None;
    };

    let facet_specs: Vec<indicatrix::geometry::cuts::FacetSpec> = full
        .angle_settings
        .iter()
        .map(|a| indicatrix::geometry::cuts::FacetSpec {
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

/// Converts one wire [`WireAxisTiltCurves`] (`Vec<f32>` fields, forced by `serde`'s
/// array-length-32 ceiling -- see that type's own doc comment) into the storage
/// [`StorageAxisTiltCurves`] (`[f32; 181]` fields) a fixed-length remote reply's axis
/// must decode into.
///
/// # Errors
///
/// Returns an error string if any curve's length isn't exactly
/// `indicatrix_vault::model::tilt_curves::TILT_CURVE_POINTS_PER_AXIS` -- would mean a
/// worker on a different build with a different `TILT_CURVE_POINTS_PER_AXIS` replied
/// (the wire type's own length isn't enforced by the type system, only by every
/// production call site sending exactly that many points; see that constant's own doc
/// comment), not an ordinary failure this batch should silently swallow the same way as
/// a connection error.
fn wire_axis_to_storage(axis: WireAxisTiltCurves) -> Result<StorageAxisTiltCurves, String> {
    let to_array = |v: Vec<f32>, name: &str| -> Result<[f32; TILT_CURVE_POINTS_PER_AXIS], String> {
        v.try_into().map_err(|v: Vec<f32>| {
            format!(
                "{name} has {} points, expected {TILT_CURVE_POINTS_PER_AXIS}",
                v.len()
            )
        })
    };
    Ok(StorageAxisTiltCurves {
        brilliance_pct: to_array(axis.brilliance_pct, "brilliance_pct")?,
        extinction_pct: to_array(axis.extinction_pct, "extinction_pct")?,
        windowing_pct: to_array(axis.windowing_pct, "windowing_pct")?,
    })
}

/// Dispatches one design's full tilt-curve sweep to `worker` as a single `TILT_CURVES`
/// request, blocking until it finishes, fails, or `cancel` is already set. See this
/// group's `mod.rs` doc comment's "Remote dispatch has no mid-request cancellation or
/// progress" section for why `cancel` is checked only once, up front, rather than
/// during the request. Returns `None` on ANY shortfall (connection failure, missing
/// `tilt_curves` capability, a `Cancelled`/`Error` reply, or a malformed reply) so the
/// caller can decide what to do next -- see this group's "Local + remote" section:
/// [`Both`] requeues for guaranteed local processing, [`RemoteOnly`] surfaces the
/// failure directly.
///
/// [`Both`]: crate::settings::LiveComputeTarget::Both
/// [`RemoteOnly`]: crate::settings::LiveComputeTarget::RemoteOnly
fn fetch_tilt_curves_remote(
    worker: &WorkerSettings,
    planes: &[GpuFacetPlane],
    material: &GemMaterial,
    cancel: &AtomicBool,
) -> Option<TiltPerformanceCurves> {
    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    let scene = SceneState {
        // Ignored by the `TILT_CURVES` handler (see this group's `mod.rs` doc
        // comment) -- `1` is a plausible, `validate_scene`-legal placeholder, not a
        // real render target.
        width: 1,
        height: 1,
        yaw: 0.0,
        pitch: 0.0,
        distance: 1.0,
        light_yaw: PREVIEW_LIGHT_YAW,
        light_pitch: PREVIEW_LIGHT_PITCH,
        exposure: 1.0,
        max_bounces: REMOTE_SCENE_MAX_BOUNCES,
        lighting_preset: LightingPreset::RingLights,
        material: material.clone(),
        planes: planes.to_vec(),
        girdle_frosted: false,
        backdrop: 0.0,
    };
    let (mut stream, welcome) = remote_render::connect_and_handshake(worker).ok()?;
    if !welcome.tilt_curves {
        return None;
    }
    let request = TiltCurvesRequest {
        request_id: TILT_REQUEST_ID,
        scene,
    };
    indicatrix_net::client::send_tilt_curves_request(&mut stream, &request).ok()?;
    match indicatrix_net::client::recv_tilt_curves_response(&mut stream) {
        Ok(TiltCurvesResponse::Curves(result)) => {
            let mut axes_vec = Vec::with_capacity(result.axes.len());
            for axis in result.axes {
                match wire_axis_to_storage(axis) {
                    Ok(axis) => axes_vec.push(axis),
                    Err(e) => {
                        warn!("Remote tilt-curve reply had a malformed axis: {e}");
                        return None;
                    }
                }
            }
            let axes: [StorageAxisTiltCurves; TILT_CURVE_AXIS_COUNT] = axes_vec.try_into().ok()?;
            Some(TiltPerformanceCurves { axes })
        }
        Ok(TiltCurvesResponse::Cancelled { .. } | TiltCurvesResponse::Error(_)) | Err(_) => None,
    }
}

/// Computes all 4 axis sweeps for one design LOCALLY, checking `cancel` before each --
/// the finer-grained cancellation the local path affords over the remote one (see this
/// group's `mod.rs` doc comment). Returns `None` the moment `cancel` is observed,
/// abandoning this one in-flight design without saving anything for it (this group's
/// own "all-or-nothing" section).
///
/// Reports no per-axis progress to the caller -- see this group's `mod.rs` doc
/// comment's "Progress with N local lanes" section for why a per-axis readout isn't
/// meaningful once more than one local lane can be mid-design at once. `cancel` is
/// still checked once per axis regardless: that granularity is about how quickly a
/// cancel is honoured, unrelated to what the UI displays.
fn compute_tilt_curves_locally(
    planes: &[GpuFacetPlane],
    material: &GemMaterial,
    cancel: &AtomicBool,
) -> Option<TiltPerformanceCurves> {
    let mut axes = Vec::with_capacity(PROFILE_AZIMUTHS_DEG.len());
    for &azimuth_deg in &PROFILE_AZIMUTHS_DEG {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        let (brilliance_pct, extinction_pct, windowing_pct) = evaluate_full_axis_profile_at_azimuth(
            planes,
            material,
            azimuth_deg,
            PREVIEW_LIGHT_YAW,
            PREVIEW_LIGHT_PITCH,
        );
        axes.push(StorageAxisTiltCurves {
            brilliance_pct,
            extinction_pct,
            windowing_pct,
        });
    }
    let axes: [StorageAxisTiltCurves; TILT_CURVE_AXIS_COUNT] = axes.try_into().ok()?;
    Some(TiltPerformanceCurves { axes })
}

/// Persists `curves` for `entry_id`. Shared by both [`process_local_entry`] and
/// [`process_remote_entry`] -- whichever lane actually produced the curves, the save
/// itself is identical.
fn save_curves(
    db: &Mutex<indicatrix_vault::db::sqlite::Database>,
    entry_id: i64,
    curves: &TiltPerformanceCurves,
) -> bool {
    // Unix seconds fits in i64 until well past the year 292 billion; the column this
    // feeds (`diagram_tilt_curves.generated_at`) is already declared INTEGER (i64) to
    // match (`cast_possible_wrap` is workspace-`allow`ed, `Cargo.toml`).
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let guard = db.lock().unwrap_or_else(PoisonError::into_inner);
    // `curve_image_png: None` -- rendered tilt-curve images are explicitly out of
    // scope for this batch; nothing in this module ever produces one.
    guard.save_tilt_curves(entry_id, curves, None, now).is_ok()
}

/// [`save_curves`], exposed for a caller outside this module -- the single-design
/// counterpart the Edit tab needs (its own "save tilt curves for this design" action,
/// once a design has been saved into the catalogue and so has a real `entry_id` to
/// save against). Kept as a thin wrapper rather than making `save_curves` itself
/// `pub` so every existing in-module call site stays untouched.
#[must_use]
pub fn save_tilt_curves_for_entry(
    db: &Mutex<indicatrix_vault::db::sqlite::Database>,
    entry_id: i64,
    curves: &TiltPerformanceCurves,
) -> bool {
    save_curves(db, entry_id, curves)
}

/// Resolves and computes `entry_id`'s tilt curves on the LOCAL engine. Returns `true`
/// iff curves were computed and saved.
///
/// Reports no title or per-axis progress -- see this group's `mod.rs` doc comment's
/// "Progress with N local lanes" section: with N lanes potentially
/// resolving/computing different designs at once, no single title is meaningful to
/// surface from here. [`run_local_lane`] reports only that a lane is busy (via
/// `local_active`), not what it is working on.
fn process_local_entry(ctx: &BatchContext<'_>, entry_id: i64, cancel: &AtomicBool) -> bool {
    let Some(resolved) = resolve_design(ctx, entry_id) else {
        return false;
    };
    let Some(curves) = compute_tilt_curves_locally(&resolved.planes, &resolved.material, cancel)
    else {
        return false;
    };
    save_curves(ctx.db, entry_id, &curves)
}

/// Resolves `entry_id` and dispatches its tilt curves to `worker` (if any), reporting
/// the design's title as soon as it's known. Returns `true` iff curves were computed
/// and saved. `worker` is `None` only when `LiveComputeTarget::RemoteOnly` is selected
/// with no worker configured at all -- treated as an immediate shortfall (no attempt
/// possible) rather than a connection failure, so this never touches the network in
/// that case.
fn process_remote_entry(
    ctx: &BatchContext<'_>,
    worker: Option<&WorkerSettings>,
    entry_id: i64,
    cancel: &AtomicBool,
    mut on_title: impl FnMut(&str),
) -> bool {
    let Some(resolved) = resolve_design(ctx, entry_id) else {
        return false;
    };
    on_title(&resolved.title);
    let Some(worker) = worker else {
        return false;
    };
    let Some(curves) =
        fetch_tilt_curves_remote(worker, &resolved.planes, &resolved.material, cancel)
    else {
        return false;
    };
    save_curves(ctx.db, entry_id, &curves)
}

fn panic_message(payload: &(dyn Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_string())
}

/// Running totals both lanes update concurrently -- plain atomics (not a `Mutex`)
/// suffice since `computed`/`failed` are independent counters with no invariant between
/// them that needs atomic coupling.
#[derive(Default)]
pub(super) struct Tally {
    pub(super) computed: AtomicU32,
    pub(super) failed: AtomicU32,
}

/// Live status all lanes update as they claim/finish designs, and [`push_progress`]
/// reads to send one coherent snapshot to the UI thread -- see this group's `mod.rs`
/// doc comment's "Progress with N local lanes" section for why `completed` replaced a
/// single running index, and why LOCAL is now a plain busy-count rather than a single
/// title/axis-index pair.
#[derive(Default, Clone)]
pub(super) struct LiveProgress {
    /// Designs fully accounted for so far, by any lane, success or failure.
    completed: u32,
    /// How many local lanes currently have a design claimed -- out of
    /// `tilt_batch_local_lane_total` (see [`push_progress`]'s `local_lane_total`
    /// parameter, fixed for the whole batch so it isn't duplicated into this
    /// per-update struct).
    local_active: u32,
    remote_title: String,
    /// Whether the remote lane is running at all this batch -- distinct from
    /// `remote_title` being empty, which also happens briefly between two remote
    /// designs while the lane is very much still active.
    remote_active: bool,
}

/// Pushes a snapshot of `progress` to the UI thread. Called by every lane after any
/// change to its own status. `local_lane_total` is fixed for the whole batch (the lane
/// count `super::spawn_tilt_batch` decided via `batch_queue::local_lane_count`), so it
/// travels as a plain parameter here rather than living inside the per-update
/// [`LiveProgress`] -- the same reason `design_total` already does.
fn push_progress(
    ui_weak: &Weak<crate::MainWindow>,
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
            .set_tilt_design_index(snapshot.completed as i32);
        ui.global::<BatchModel>()
            .set_tilt_design_total(design_total as i32);
        ui.global::<BatchModel>()
            .set_tilt_local_active(snapshot.local_active as i32);
        ui.global::<BatchModel>()
            .set_tilt_local_lane_total(local_lane_total as i32);
        ui.global::<BatchModel>()
            .set_tilt_remote_title(snapshot.remote_title.into());
        ui.global::<BatchModel>()
            .set_tilt_remote_active(snapshot.remote_active);
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

/// Marks one local lane as having just claimed a design (`+1`) or having just finished
/// one (`-1`, `delta = -1`) -- called right after `WorkQueue::claim_local` succeeds and
/// again right before the lane loops back to claim its next item. `delta` is always `1`
/// or `-1`; taking a signed step rather than separate increment/decrement functions
/// keeps both call sites' intent ("this lane just became busy" / "this lane just became
/// idle again") visible at the call site instead of behind two near-identical helpers.
fn adjust_local_active(progress: &Mutex<LiveProgress>, delta: i32) {
    let mut p = progress.lock().unwrap_or_else(PoisonError::into_inner);
    p.local_active = p.local_active.saturating_add_signed(delta);
}

/// Everything both lane-runner functions need that is safe to share by plain reference
/// across the `std::thread::scope` in `super::spawn_tilt_batch` -- deliberately
/// excludes `Weak<MainWindow>` (each lane gets its OWN owned clone instead, passed
/// separately), since this struct being captured by reference into both lane closures
/// requires it be `Sync`, and there is no need to find out whether Slint's `Weak` is.
pub(super) struct LaneShared<'a> {
    pub(super) ctx: &'a BatchContext<'a>,
    pub(super) queue: &'a WorkQueue<i64>,
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

/// Runs one LOCAL lane out of `shared.local_lane_total`: claims designs via
/// `WorkQueue::claim_local` (retried-after-remote-failure designs first, then fresh
/// ones) until both the retry pile and the shared pool are empty AND `remote_lane_done`
/// confirms no more work is coming -- see `gui::batch::batch_queue`'s module doc
/// comment for why that combination, not just "queue empty", is the correct stop
/// condition. Called once per local lane by `super::spawn_tilt_batch` (see this
/// group's `mod.rs` doc comment's "Why N local lanes" section); every call shares the
/// same `shared.queue`/`shared.progress` so N lanes running this function concurrently
/// self-balance with no coordination beyond the queue itself.
pub(super) fn run_local_lane(
    shared: &LaneShared<'_>,
    ui_weak: &Weak<crate::MainWindow>,
    remote_lane_done: &AtomicBool,
) {
    loop {
        if shared.cancel.load(Ordering::Relaxed) {
            break;
        }
        let Some(entry_id) = shared.queue.claim_local() else {
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

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            process_local_entry(shared.ctx, entry_id, shared.cancel)
        }));

        let saved = match result {
            Ok(saved) => saved,
            Err(payload) => {
                warn!(
                    "Tilt-curve computation panicked for entry {entry_id}: {}",
                    panic_message(&*payload)
                );
                false
            }
        };

        if saved {
            shared.tally.computed.fetch_add(1, Ordering::Relaxed);
        } else {
            shared.tally.failed.fetch_add(1, Ordering::Relaxed);
        }
        adjust_local_active(shared.progress, -1);
        increment_completed(shared.progress);
        push_progress(
            ui_weak,
            shared.design_total,
            shared.local_lane_total,
            shared.progress,
        );
    }
}

/// Runs the REMOTE lane: claims fresh designs via `WorkQueue::claim_shared` only (never
/// a local-retried one -- that pile is reserved for the local lane, see
/// `gui::batch::batch_queue`'s doc comment) until the shared pool is empty, then
/// signals `remote_lane_done` so the local lane knows no further requeues are coming.
///
/// `worker` is `None` only for `RemoteOnly` with no worker configured (see
/// [`process_remote_entry`]'s own doc comment); `fallback_to_local` is `true` only for
/// `LiveComputeTarget::Both` -- for `RemoteOnly` every failure is final and tallied
/// `failed` directly: a remote failure is reported as failed and never silently
/// re-rendered locally.
pub(super) fn run_remote_lane(
    shared: &LaneShared<'_>,
    ui_weak: &Weak<crate::MainWindow>,
    worker: Option<&WorkerSettings>,
    fallback_to_local: bool,
    remote_lane_done: &AtomicBool,
) {
    loop {
        if shared.cancel.load(Ordering::Relaxed) {
            break;
        }
        let Some(entry_id) = shared.queue.claim_shared() else {
            break;
        };

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            process_remote_entry(shared.ctx, worker, entry_id, shared.cancel, |title| {
                set_remote_status(shared.progress, true, title);
                push_progress(
                    ui_weak,
                    shared.design_total,
                    shared.local_lane_total,
                    shared.progress,
                );
            })
        }));

        let saved = match result {
            Ok(saved) => saved,
            Err(payload) => {
                warn!(
                    "Remote tilt-curve dispatch panicked for entry {entry_id}: {}",
                    panic_message(&*payload)
                );
                false
            }
        };

        if saved {
            shared.tally.computed.fetch_add(1, Ordering::Relaxed);
            increment_completed(shared.progress);
        } else if fallback_to_local {
            // Not accounted for yet -- the local lane will attempt this exact design
            // next, and IT is the one that finally tallies/completes it. See
            // `gui::batch::batch_queue`'s module doc comment for why this never goes
            // back to the shared pool.
            shared.queue.return_to_local(entry_id);
        } else {
            // `RemoteOnly`: no local fallback exists, so this failure is final.
            shared.tally.failed.fetch_add(1, Ordering::Relaxed);
            increment_completed(shared.progress);
        }
        push_progress(
            ui_weak,
            shared.design_total,
            shared.local_lane_total,
            shared.progress,
        );
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

/// The single-design entry point: computes tilt curves directly from already-resolved
/// `planes`/`material` -- typically `EditorState::design`'s own solved planes plus
/// `gui::editor::material_lookup::resolved_gem_material` (neither defined in this
/// module) -- entirely bypassing [`resolve_design`]'s
/// database read. `resolve_design` can only ever see whatever a design's catalogue
/// row currently stores, so routing the Edit tab's own "run tilt analysis for what's
/// on the bench right now" action through the ordinary batch would silently score a
/// STALE on-disk copy instead of the cutter's actual unsaved (or since-edited) work,
/// or fail outright for a design that was never saved at all (no `entry_id` to look
/// up).
///
/// Tries `worker` first when given, falling back to the local engine on any remote
/// shortfall -- the same fallback [`LiveComputeTarget::Both`] gives the catalogue
/// batch (see [`process_remote_entry`]'s own doc comment); `worker: None` runs local
/// only, matching [`LiveComputeTarget::LocalOnly`]. Deliberately reuses
/// [`fetch_tilt_curves_remote`]/[`compute_tilt_curves_locally`] verbatim rather than a
/// third implementation, so a single-design run and a catalogue batch run can never
/// silently disagree about how a design's tilt curves are computed. Unlike
/// [`process_local_entry`]/[`process_remote_entry`], this never calls [`save_curves`]:
/// there is no `entry_id` to save against for a design that may not exist in the
/// catalogue at all, and a design that DOES have one should not have its saved
/// catalogue curves silently overwritten by a still-being-edited version without the
/// cutter explicitly asking for that -- a caller that wants to persist should call
/// [`save_curves`] itself once it has an `entry_id` to save against.
///
/// Synchronous and blocking (a real ~1.36s local sweep, or a remote round trip) --
/// exactly like [`process_local_entry`]/[`process_remote_entry`], so a caller must run
/// this off the UI thread and marshal the result back itself, the same
/// `thread::spawn` + `upgrade_in_event_loop` shape every other worker call in this
/// crate already uses (e.g. `gui::tilt::tilt_profile::spawn_tilt_profile_sweep`).
///
/// [`LiveComputeTarget::Both`]: crate::settings::LiveComputeTarget::Both
/// [`LiveComputeTarget::LocalOnly`]: crate::settings::LiveComputeTarget::LocalOnly
#[must_use]
pub fn tilt_curves_for_planes(
    planes: &[GpuFacetPlane],
    material: &GemMaterial,
    worker: Option<&WorkerSettings>,
    cancel: &AtomicBool,
) -> Option<TiltPerformanceCurves> {
    if let Some(worker) = worker
        && let Some(curves) = fetch_tilt_curves_remote(worker, planes, material, cancel)
    {
        return Some(curves);
    }
    compute_tilt_curves_locally(planes, material, cancel)
}

/// Runs every lane for one batch attempt inside a `std::thread::scope`, blocking until
/// all of them finish. Factored out of `super::spawn_tilt_batch` purely to keep that
/// function under clippy's line-count limit -- every parameter here is already a
/// reference (or `Copy`), so each `scope.spawn(move || ...)` closure below simply moves
/// a cheap copy of it, with no further indirection needed the way `spawn_tilt_batch`
/// itself would have needed had it kept this block inline (see that function's own
/// comment on why, for the ONE local variable it still owns directly: `shared`).
pub(super) fn run_batch_lanes(
    shared: &LaneShared<'_>,
    plan: super::super::batch_queue::LanePlan,
    remote_worker: Option<&WorkerSettings>,
    local_lane_total: u32,
    ui_weak: &Weak<crate::MainWindow>,
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
            // N independently-claiming local lanes -- see this group's `mod.rs` doc
            // comment's "Why N local lanes" section. Every lane runs the identical
            // `run_local_lane`; `WorkQueue` is what makes that safe with no other
            // coordination between them.
            for _ in 0..local_lane_total {
                let local_ui_weak = ui_weak.clone();
                scope.spawn(move || {
                    run_local_lane(shared, &local_ui_weak, remote_lane_done);
                });
            }
        }
    });
}
