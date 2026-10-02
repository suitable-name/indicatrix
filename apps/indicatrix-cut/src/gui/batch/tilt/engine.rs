//! The per-design resolve/compute/dispatch engine and the local/remote lane runners
//! that pull from a shared [`WorkQueue`] -- see this group's own `mod.rs` doc comment
//! for the design behind this file's shape.

use crate::{
    BatchModel,
    bridge::{
        preview_render::{PREVIEW_LIGHT_PITCH, PREVIEW_LIGHT_YAW},
        remote::remote_render,
    },
    gui::batch::{
        batch_queue::WorkQueue,
        material_choice::ensure_balanced_material,
        preview::{RI_MATCH_TOLERANCE, target_ri_for_design},
        remote_dispatch::{DispatcherGroup, RemoteStatus},
    },
    settings::WorkerSettings,
};
use indicatrix::{
    color::metrics::{
        PROFILE_AZIMUTHS_DEG, SweepProgress, evaluate_all_axes_profiles_stepped,
        evaluate_full_axis_profile_at_azimuth,
    },
    geometry::plane::GpuFacetPlane,
    optics::{
        materials::GemMaterial,
        raytracer::{DEFAULT_MAX_BOUNCES, EnvironmentSource, LightingPreset},
    },
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
    collections::BTreeSet,
    sync::{
        Mutex, PoisonError,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    thread,
    time::Duration,
};
use tracing::warn;

mod save;

use super::remote_lane::run_remote_lane;
use save::save_curves;
pub use save::save_tilt_curves_for_entry;

/// `max_bounces` on the [`SceneState`] a remote tilt-curve request carries -- IGNORED
/// by the worker's `TILT_CURVES` handler entirely (see `indicatrix_net::messages::tilt`'s
/// module doc comment: only `planes`/`material`/`light_yaw`/`light_pitch` actually feed
/// the computation), but still validated, so this must be a plausible value
/// (`1..=128`, see `apps/indicatrix-worker::validate::MAX_BOUNCES`). It is the
/// raytracer's [`DEFAULT_MAX_BOUNCES`] purely for familiarity -- no render this batch
/// performs ever actually bounces a ray, so the specific value is otherwise arbitrary.
const REMOTE_SCENE_MAX_BOUNCES: u32 = DEFAULT_MAX_BOUNCES;

/// The lighting preset every stored tilt-curve set is scored under, locally and (in the
/// [`SceneState`] a remote worker receives) remotely, so both lanes describe the same
/// lighting. The editor's default rig, at the preview light pose.
const BATCH_TILT_LIGHTING_PRESET: LightingPreset = LightingPreset::RingLights;

/// The environment the batch's tilt curves are scored under: [`BATCH_TILT_LIGHTING_PRESET`]
/// at the preview light pose.
const fn batch_environment() -> EnvironmentSource<'static> {
    BATCH_TILT_LIGHTING_PRESET.studio(1.0, PREVIEW_LIGHT_YAW, PREVIEW_LIGHT_PITCH)
}

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
    /// The designs whose geometry came from the angle table rather than a design
    /// file -- see `gui::batch::record_planes_for_batch`. Read once at the end for
    /// the batch's summary line.
    pub(super) angle_table_entries: &'a Mutex<BTreeSet<i64>>,
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
    /// The persisted preview material's name `material` was looked up by -- part of the
    /// fingerprint the curves are stored with.
    material_name: String,
    /// The catalogue row's `updated_at`, read together with the record `planes` come
    /// from: the compare-and-swap token `Database::save_tilt_curves` checks, so curves
    /// swept from a superseded record are refused instead of stored.
    updated_at: Option<i64>,
}

/// `full`'s facet planes for a tilt-curve sweep -- the design file first, the angle
/// table only as the fallback (`gui::batch::record_planes_for_batch`, shared with the
/// preview batch and the library detail view). Both the local and the remote lane get
/// their planes here, through [`resolve_design`]; a remote `TILT_CURVES` request
/// ships these planes in its scene, so the worker never resolves a record itself.
pub(super) fn record_planes(
    ctx: &BatchContext<'_>,
    full: &indicatrix_vault::model::entry::FullDiagramRecord,
) -> Option<Vec<GpuFacetPlane>> {
    super::super::record_planes_for_batch(full, ctx.angle_table_entries)
}

/// Resolves `entry_id`'s geometry and preview material, or `None` if either step comes
/// up empty (malformed/unreadable row, unreconstructable geometry, or no material could
/// be matched/assigned) -- mirrors `gui::batch::preview::engine`'s own resolve prelude
/// exactly, factored out here since BOTH lanes need it independently (a design claimed
/// by remote and a design claimed by local each resolve their own copy; nothing about a
/// design's resolved geometry is shared or cached across lanes).
fn resolve_design(ctx: &BatchContext<'_>, entry_id: i64) -> Option<ResolvedDesign> {
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
    let material = GemMaterial::by_name(&material_name)?;

    Some(ResolvedDesign {
        title: full.title,
        planes,
        material,
        material_name,
        updated_at,
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
        lighting_preset: BATCH_TILT_LIGHTING_PRESET,
        material: material.clone(),
        planes: planes.to_vec(),
        girdle_frosted: false,
        backdrop: 0.0,
        environment: indicatrix_net::scene::SceneEnvironment::Studio,
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
            batch_environment(),
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

/// [`compute_tilt_curves_locally`] with `on_step` called before every one of the
/// sweep's raytrace evaluations, for the single-design run's progress bar. Uses the
/// shared stepped sweep (`indicatrix::color::metrics::evaluate_all_axes_profiles_stepped`),
/// whose curves are bit-identical to the plain per-axis calls, and honours `cancel`
/// between evaluations instead of between axes.
fn compute_tilt_curves_locally_stepped(
    planes: &[GpuFacetPlane],
    material: &GemMaterial,
    cancel: &AtomicBool,
    on_step: &mut dyn FnMut(SweepProgress),
) -> Option<TiltPerformanceCurves> {
    let profiles = evaluate_all_axes_profiles_stepped(
        planes,
        material,
        batch_environment(),
        &mut |progress| {
            on_step(progress);
            !cancel.load(Ordering::Relaxed)
        },
    )?;
    let axes: Vec<StorageAxisTiltCurves> = profiles
        .into_iter()
        .map(|profile| StorageAxisTiltCurves {
            brilliance_pct: profile.brilliance,
            extinction_pct: profile.extinction,
            windowing_pct: profile.windowing,
        })
        .collect();
    let axes: [StorageAxisTiltCurves; TILT_CURVE_AXIS_COUNT] = axes.try_into().ok()?;
    Some(TiltPerformanceCurves { axes })
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
    save_curves(
        ctx.db,
        entry_id,
        &curves,
        Some(&resolved.material_name),
        resolved.updated_at,
    )
}

/// Resolves `entry_id` and dispatches its tilt curves to `worker` (if any), reporting
/// the design's title as soon as it's known. Returns `true` iff curves were computed
/// and saved. `worker` is `None` only when `LiveComputeTarget::RemoteOnly` is selected
/// with no worker configured at all -- treated as an immediate shortfall (no attempt
/// possible) rather than a connection failure, so this never touches the network in
/// that case.
pub(super) fn process_remote_entry(
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
    save_curves(
        ctx.db,
        entry_id,
        &curves,
        Some(&resolved.material_name),
        resolved.updated_at,
    )
}

pub(super) fn panic_message(payload: &(dyn Any + Send)) -> String {
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
    /// The remote dispatchers' side: how many run, how many designs are on the remote,
    /// and the most recently started design's title. Changed only through
    /// [`update_remote`].
    remote: RemoteStatus,
}

/// Pushes a snapshot of `progress` to the UI thread. Called by every lane after any
/// change to its own status. `local_lane_total` is fixed for the whole batch (the lane
/// count `super::spawn_tilt_batch` decided via `batch_queue::local_lane_count`), so it
/// travels as a plain parameter here rather than living inside the per-update
/// [`LiveProgress`] -- the same reason `design_total` already does.
pub(super) fn push_progress(
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
            .set_tilt_remote_title(snapshot.remote.title().into());
        ui.global::<BatchModel>()
            .set_tilt_remote_active(snapshot.remote.is_active());
        ui.global::<BatchModel>()
            .set_tilt_remote_in_flight(snapshot.remote.in_flight() as i32);
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

pub(super) fn increment_completed(progress: &Mutex<LiveProgress>) {
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
    /// How many remote dispatchers this batch spawns -- see
    /// `batch_queue::remote_lane_count`. `0` when no remote lane runs (`LocalOnly`).
    pub(super) remote_lane_total: u32,
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
        // Read BEFORE the claim: a remote dispatcher raises the flag only after its last
        // requeue, so a claim that follows an observed `true` sees every requeue and an
        // empty answer is final. Read after the claim, a requeue landing between the two
        // would be stranded.
        let remote_finished = remote_lane_done.load(Ordering::Acquire);
        let Some(entry_id) = shared.queue.claim_local() else {
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
///
/// `on_step` reports the LOCAL sweep's progress, before each raytrace evaluation (see
/// [`compute_tilt_curves_locally_stepped`]); a remote sweep is one blocking request
/// and reports nothing.
///
/// [`LiveComputeTarget::Both`]: crate::settings::LiveComputeTarget::Both
/// [`LiveComputeTarget::LocalOnly`]: crate::settings::LiveComputeTarget::LocalOnly
#[must_use]
pub fn tilt_curves_for_planes(
    planes: &[GpuFacetPlane],
    material: &GemMaterial,
    worker: Option<&WorkerSettings>,
    cancel: &AtomicBool,
    on_step: &mut dyn FnMut(SweepProgress),
) -> Option<TiltPerformanceCurves> {
    if let Some(worker) = worker
        && let Some(curves) = fetch_tilt_curves_remote(worker, planes, material, cancel)
    {
        return Some(curves);
    }
    compute_tilt_curves_locally_stepped(planes, material, cancel, on_step)
}

/// Runs every lane for one batch attempt inside a `std::thread::scope`, blocking until
/// all of them finish. Factored out of `super::spawn_tilt_batch` purely to keep that
/// function under clippy's line-count limit -- every parameter here is already a
/// reference (or `Copy`), so each `scope.spawn(move || ...)` closure below simply moves
/// a cheap copy of it, with no further indirection needed the way `spawn_tilt_batch`
/// itself would have needed had it kept this block inline (see that function's own
/// comment on why, for the ONE local variable it still owns directly: `shared`).
///
/// `shared.remote_lane_total` remote dispatchers run at once, all claiming from the same
/// queue; `remote_lane_done` is raised when the last of them ends.
pub(super) fn run_batch_lanes(
    shared: &LaneShared<'_>,
    plan: super::super::batch_queue::LanePlan,
    remote_worker: Option<&WorkerSettings>,
    local_lane_total: u32,
    ui_weak: &Weak<crate::MainWindow>,
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
