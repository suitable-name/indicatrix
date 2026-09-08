//! Off-UI-thread computation of the Tilt Performance dialog's full-axis (±90°, 1° step)
//! sweeps for all four `indicatrix::color::metrics::PROFILE_AZIMUTHS_DEG` axes.
//!
//! One sample point (`evaluate_gem_optical_metrics`, round brilliant/diamond, release,
//! single-threaded) measures ~1.875 ms. A full-axis profile is 181 points across all
//! four axes, so one complete sweep costs roughly `181 * 4 * 1.875 ms` ~= 1.36
//! seconds -- a real, user-visible wait, which is why:
//!
//! - This work runs on its own `std::thread::spawn` worker, never the UI thread.
//! - `AxesCacheKey`/`launched_key` short-circuit re-opening the dialog (or a `changed`
//!   re-fire with unchanged inputs) against a second ~1.4s sweep. `AxesCacheKey` keys
//!   on the fully resolved [`GemMaterial`], not just its display name -- a custom
//!   material can be edited (RI, birefringence, dispersion, ...) while a tier keeps
//!   referencing the same name, and keying on the name alone would make that register
//!   as "nothing changed," causing both the "Re-render" button to read as a no-op and
//!   a newly opened dialog to show a curve for a material it no longer matches.
//!   [`should_recompute_axes`] is that dedup decision pulled out as its own pure
//!   function.
//! - The `generation` counter lets a stale in-flight computation be abandoned: at
//!   ~1.36s per run, a superseded sweep landing after the user has moved on (changed
//!   material, orbited the camera, reopened against different geometry) would
//!   otherwise be a visible glitch.
//! - `PerformanceGraphDialog.extra_axes_loading` is a real loading state the dialog
//!   must show for over a second, not a brief flicker.
//!
//! This sweeps all four axes uniformly (not just the three non-canonical azimuths),
//! including axis 0, rather than special-casing it to reuse the live render thread's
//! own positive-half-only value (`graph_brilliance`/etc., still computed every frame
//! for `settings_dialog.slint`'s mini tilt chart) -- that value cannot supply the
//! negative half a full-axis sweep needs anyway. One code path, one cache key, one
//! generation counter, no cross-thread reuse of a value that could itself be
//! mid-update, at the cost of one redundant azimuth-0-positive-half resweep (~12% of
//! the ~724 total raytrace evaluations per run).
//!
//! Reads `RenderContext::active_planes`/`material_name`/`custom_materials`/
//! `light_yaw`/`light_pitch` directly -- the same inputs
//! `bridge::render_thread::metrics::compute_or_reuse_metrics` keys its own cache on,
//! minus camera yaw/pitch (irrelevant here since this always sweeps the full axis at
//! four fixed azimuths).

use crate::{
    MainWindow, TiltModel,
    bridge::render_thread::{RenderContext, hash_planes, resolve_material},
    gui::optics::curve_path::full_axis_curve_path,
};
use indicatrix::{
    color::metrics::{PROFILE_AZIMUTHS_DEG, evaluate_full_axis_profile_at_azimuth},
    geometry::plane::GpuFacetPlane,
    optics::materials::GemMaterial,
};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};

/// Cheap identity for "would recomputing the four full-axis sweeps produce a different
/// result" -- the same fields `bridge::render_thread::metrics::MetricsCacheKey` keys
/// on, minus camera yaw/pitch (irrelevant here, see this module's doc comment).
#[derive(Clone, PartialEq)]
struct AxesCacheKey {
    light_yaw: f32,
    light_pitch: f32,
    /// The fully resolved material (`resolve_material`'s output), not merely its
    /// display name. `GemMaterial`'s derived, field-by-field `PartialEq` is what makes
    /// editing a custom material's own RI/birefringence/dispersion under the same name
    /// register as a real cache-key change -- keying on a bare name would silently
    /// treat "same name, different optics" as "nothing changed."
    material: GemMaterial,
    planes_hash: u64,
}

/// Whether launching a fresh four-axis sweep for `requested` is actually necessary
/// given whatever was last launched (`current`) -- pulled out of the mutex-guarded
/// dedup check in [`setup_tilt_profile_callback`] purely so it is unit-testable.
/// `true` means the ~1.36s sweep must actually run; `false` means the existing
/// `graph_*_extra_axes` data already describes exactly this material/light/geometry.
#[must_use]
fn should_recompute_axes(current: Option<&AxesCacheKey>, requested: &AxesCacheKey) -> bool {
    current != Some(requested)
}

/// One metric's curve for every `PROFILE_AZIMUTHS_DEG` axis, in axis order: four rows,
/// each a full 181-point ±90° profile. Named rather than written inline purely so
/// [`sweep_all_axes`]'s three-of-these return type stays readable (and under clippy's
/// `type_complexity` bar).
type AxisCurveRows = Vec<[f32; 181]>;

/// Sweeps all four [`PROFILE_AZIMUTHS_DEG`] axes, each a full 181-point ±90° profile,
/// returning `(brilliance, extinction, windowing)` rows in axis order.
///
/// Axis 0 is swept here too, rather than reusing the live render thread's own value:
/// that one covers only the positive half and cannot supply the negative half a
/// full-axis sweep needs.
///
/// Split out of [`setup_tilt_profile_callback`] purely to keep that function under
/// clippy's function-length lint. It is also the entire ~1.36s cost of a sweep in one
/// place, which makes that cost easy to find and measure.
fn sweep_all_axes(
    planes: &[GpuFacetPlane],
    material: &GemMaterial,
    light_yaw: f32,
    light_pitch: f32,
) -> (AxisCurveRows, AxisCurveRows, AxisCurveRows) {
    let mut brilliance_rows: AxisCurveRows = Vec::with_capacity(PROFILE_AZIMUTHS_DEG.len());
    let mut extinction_rows: AxisCurveRows = Vec::with_capacity(PROFILE_AZIMUTHS_DEG.len());
    let mut windowing_rows: AxisCurveRows = Vec::with_capacity(PROFILE_AZIMUTHS_DEG.len());
    for &azimuth_deg in &PROFILE_AZIMUTHS_DEG {
        let (b, e, w) = evaluate_full_axis_profile_at_azimuth(
            planes,
            material,
            azimuth_deg,
            light_yaw,
            light_pitch,
        );
        brilliance_rows.push(b);
        extinction_rows.push(e);
        windowing_rows.push(w);
    }
    (brilliance_rows, extinction_rows, windowing_rows)
}

/// `on_request_tilt_profile_axes`'s own handler body -- checks `launched_key` for a
/// duplicate request, then spawns the background sweep and pushes its results back
/// onto the UI thread once done (dropping a stale result if superseded by a newer
/// request via `generation`). Split out of [`setup_tilt_profile_callback`] purely to
/// keep that function under clippy's function-length lint; see that function's own
/// doc comment for the full behaviour.
fn handle_request_tilt_profile_axes(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    launched_key: &Arc<Mutex<Option<AxesCacheKey>>>,
    generation: &Arc<AtomicU64>,
) {
    let (planes, material_name, custom_materials, light_yaw, light_pitch) = {
        let ctx = render_ctx.lock().unwrap();
        (
            ctx.active_planes.clone(),
            ctx.material_name.clone(),
            ctx.custom_materials.clone(),
            ctx.light_yaw,
            ctx.light_pitch,
        )
    };
    // Resolved once, up front -- both to build a key capturing the material's full
    // identity and to hand the same resolved value to the worker thread below
    // rather than re-resolving it there from a name that could resolve to
    // something else by the time the thread runs.
    let material = resolve_material(
        &GemMaterial::all_materials(),
        &custom_materials,
        &material_name,
    );

    let key = AxesCacheKey {
        light_yaw,
        light_pitch,
        material: material.clone(),
        planes_hash: hash_planes(&planes),
    };
    {
        let mut launched = launched_key.lock().unwrap();
        if !should_recompute_axes(launched.as_ref(), &key) {
            // Already computed (or currently computing) for these exact inputs.
            return;
        }
        *launched = Some(key);
    }

    let my_generation = generation.fetch_add(1, Ordering::SeqCst) + 1;
    let generation = generation.clone();
    ui.global::<TiltModel>().set_extra_axes_loading(true);
    let ui_weak_bg = ui.as_weak();

    spawn_tilt_profile_sweep(
        planes,
        material,
        light_yaw,
        light_pitch,
        generation,
        my_generation,
        ui_weak_bg,
    );
}

/// The background-thread half of [`handle_request_tilt_profile_axes`]: runs the full
/// four-curve sweep off the UI thread, then pushes the results back via
/// `upgrade_in_event_loop`, dropping a stale result superseded by a newer request
/// (see `generation`'s own doc comment on `setup_tilt_profile_callback`). Split out
/// purely to keep that caller under clippy's function-length lint.
fn spawn_tilt_profile_sweep(
    planes: Arc<Vec<GpuFacetPlane>>,
    material: GemMaterial,
    light_yaw: f32,
    light_pitch: f32,
    generation: Arc<AtomicU64>,
    my_generation: u64,
    ui_weak_bg: slint::Weak<MainWindow>,
) {
    std::thread::spawn(move || {
        let (brilliance_rows, extinction_rows, windowing_rows) =
            sweep_all_axes(&planes, &material, light_yaw, light_pitch);

        let brilliance_paths: Vec<SharedString> = brilliance_rows
            .iter()
            .map(|row| full_axis_curve_path(row).into())
            .collect();
        let extinction_paths: Vec<SharedString> = extinction_rows
            .iter()
            .map(|row| full_axis_curve_path(row).into())
            .collect();
        let windowing_paths: Vec<SharedString> = windowing_rows
            .iter()
            .map(|row| full_axis_curve_path(row).into())
            .collect();

        let to_model_rows = |rows: Vec<[f32; 181]>| -> ModelRc<ModelRc<f32>> {
            ModelRc::new(VecModel::from(
                rows.into_iter()
                    .map(|row| ModelRc::new(VecModel::from(row.to_vec())))
                    .collect::<Vec<_>>(),
            ))
        };

        let _ = ui_weak_bg.upgrade_in_event_loop(move |ui| {
            if generation.load(Ordering::SeqCst) != my_generation {
                // Superseded by a newer request -- drop this stale result rather
                // than overwriting whatever the newer computation lands.
                return;
            }
            ui.global::<TiltModel>()
                .set_graph_brilliance_extra_axes(to_model_rows(brilliance_rows));
            ui.global::<TiltModel>()
                .set_graph_extinction_extra_axes(to_model_rows(extinction_rows));
            ui.global::<TiltModel>()
                .set_graph_windowing_extra_axes(to_model_rows(windowing_rows));
            ui.global::<TiltModel>()
                .set_graph_brilliance_extra_paths(ModelRc::new(VecModel::from(brilliance_paths)));
            ui.global::<TiltModel>()
                .set_graph_extinction_extra_paths(ModelRc::new(VecModel::from(extinction_paths)));
            ui.global::<TiltModel>()
                .set_graph_windowing_extra_paths(ModelRc::new(VecModel::from(windowing_paths)));
            ui.global::<TiltModel>().set_extra_axes_loading(false);
        });
    });
}

/// Wires `MainWindow::request_tilt_profile_axes` to a background computation of all
/// four full-axis (±90°, 1° step) tilt-elevation sweeps, pushing the results into
/// `graph_*_extra_axes`/`graph_*_extra_paths` (and `extra_axes_loading` around the
/// computation) once done. Split out of `run_gui`/`build_main_window` purely to keep
/// those functions under clippy's function-length lint.
pub(in crate::gui) fn setup_tilt_profile_callback(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
) {
    // `Some(key)` once a request for exactly these inputs has been launched (whether
    // or not it has finished yet) -- deduplicates re-opening the dialog against the
    // same geometry/material/light without a second ~1.36s background sweep.
    let launched_key: Arc<Mutex<Option<AxesCacheKey>>> = Arc::new(Mutex::new(None));
    // Cloned out before `on_request_tilt_profile_axes` moves the original in below --
    // `on_rerender_curve_with_current_material` needs its own handle to force a fresh
    // sweep regardless of what the dedup check thinks.
    let launched_key_for_rerender = Arc::clone(&launched_key);
    // Bumped on every newly-launched computation; a background thread checks its own
    // snapshot against the latest value before applying results, so a stale
    // computation is silently dropped instead of overwriting fresher data.
    let generation = Arc::new(AtomicU64::new(0));

    let ui_weak = ui.as_weak();
    let render_ctx = render_ctx.clone();
    ui.global::<TiltModel>()
        .on_request_tilt_profile_axes(move || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            handle_request_tilt_profile_axes(&ui, &render_ctx, &launched_key, &generation);
        });

    // The decision-logic callback `performance_graph_dialog.slint`'s `cache_is_stale`
    // property calls into. `cached` arrives from `MainWindow::cached_curve_material`,
    // pushed by `gui::detail::load_diagram_detail`/`_remote` on every design selection.
    ui.global::<TiltModel>().on_is_cached_curve_material_stale(
        |cached: SharedString, current: SharedString| {
            let cached = if cached.is_empty() {
                None
            } else {
                Some(cached.as_str())
            };
            cached_curve_material_is_stale(cached, &current)
        },
    );

    // Re-renders the tilt curve fresh against the current material -- the "discard the
    // stale cached curve, sweep a fresh one" action the stale-cache banner promises.
    // `invoke_request_tilt_profile_axes` alone dedupes against `launched_key` exactly
    // like a dialog-open re-fire does, so clicking this while the live inputs still
    // hash to the last-launched `AxesCacheKey` would silently do nothing. This button
    // means "redo it now regardless," so it clears `launched_key` first, forcing the
    // next call to treat this as a brand-new request.
    let ui_weak_rerender = ui.as_weak();
    ui.global::<TiltModel>()
        .on_rerender_curve_with_current_material(move || {
            if let Some(ui) = ui_weak_rerender.upgrade() {
                *launched_key_for_rerender.lock().unwrap() = None;
                ui.global::<TiltModel>().invoke_request_tilt_profile_axes();
            }
        });
}

/// Whether the tilt-curve/preview data currently cached for this design was rendered
/// under a different `GemMaterial` than the one the viewport currently has loaded, and
/// the dialog should therefore offer to re-render fresh rather than silently show a
/// stale-coloured cached curve/image.
///
/// `cached_material_name` is `None` whenever there is no cached curve to compare
/// against at all (e.g. no preview generated yet, or browsing a remote library whose
/// wire protocol carries no preview-material field), in which case this always
/// returns `false`: "no cached data" is not "stale cached data". Name comparison is
/// exact (matching `resolve_material`'s `eq_ignore_ascii_case`).
///
/// The caller (`gui::detail::load_diagram_detail`) sources `cached_material_name` from
/// `Database::get_preview_images(entry_id).material`.
#[must_use]
pub fn cached_curve_material_is_stale(
    cached_material_name: Option<&str>,
    current_material_name: &str,
) -> bool {
    cached_material_name.is_some_and(|cached| !cached.eq_ignore_ascii_case(current_material_name))
}

#[cfg(test)]
mod tests {
    use super::{AxesCacheKey, cached_curve_material_is_stale, should_recompute_axes};
    use indicatrix::optics::materials::GemMaterial;

    #[test]
    fn no_cached_material_is_never_stale() {
        assert!(!cached_curve_material_is_stale(None, "Diamond"));
    }

    #[test]
    fn matching_material_is_not_stale() {
        assert!(!cached_curve_material_is_stale(Some("Diamond"), "Diamond"));
        assert!(!cached_curve_material_is_stale(Some("diamond"), "Diamond"));
    }

    #[test]
    fn differing_material_is_stale() {
        assert!(cached_curve_material_is_stale(Some("Diamond"), "Sapphire"));
    }

    fn sample_key(material: GemMaterial) -> AxesCacheKey {
        AxesCacheKey {
            light_yaw: 48.0,
            light_pitch: 54.0,
            material,
            planes_hash: 42,
        }
    }

    #[test]
    fn should_recompute_axes_is_true_the_first_time() {
        let key = sample_key(GemMaterial::all_materials()[0].clone());
        assert!(should_recompute_axes(None, &key));
    }

    #[test]
    fn should_recompute_axes_is_false_once_the_exact_key_was_already_launched() {
        let key = sample_key(GemMaterial::all_materials()[0].clone());
        assert!(!should_recompute_axes(Some(&key), &key));
    }

    #[test]
    fn should_recompute_axes_is_true_when_light_or_geometry_differ() {
        let material = GemMaterial::all_materials()[0].clone();
        let current = sample_key(material);
        let mut moved_light = current.clone();
        moved_light.light_yaw += 1.0;
        assert!(should_recompute_axes(Some(&current), &moved_light));
        let mut moved_geometry = current.clone();
        moved_geometry.planes_hash = 43;
        assert!(should_recompute_axes(Some(&current), &moved_geometry));
    }

    /// A custom material can be edited (RI, birefringence, dispersion, ...) while a
    /// tier keeps referencing the same name -- so the dedup key must change even
    /// though the name didn't. Folding the fully resolved `GemMaterial` into
    /// `AxesCacheKey` is what makes that true, via its derived `PartialEq`.
    #[test]
    fn should_recompute_axes_is_true_when_only_the_resolved_materials_optics_differ() {
        let mut edited = GemMaterial::all_materials()[0].clone();
        let current = sample_key(edited.clone());
        edited.birefringence_delta += 0.01;
        let requested = sample_key(edited);
        assert_eq!(
            current.material.name, requested.material.name,
            "the display name must be unchanged -- that is the whole point of this case"
        );
        assert!(should_recompute_axes(Some(&current), &requested));
    }
}
