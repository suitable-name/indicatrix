//! Tilt performance video export: the "Export tilt video" collapsible section of
//! `performance_graph_dialog.slint`. Renders one high-quality frame per swept angle
//! (posed exactly like the dialog's own curve/hover-preview, see
//! `tilt_hover_preview::camera_pose_for_axis_tilt`), optionally overlays the tilt
//! performance values (`overlay`), writes a numbered PNG sequence, then muxes an MP4
//! (ffmpeg) or GIF (fallback) -- see `encode`'s own module doc comment for the full
//! encoding chain.
//!
//! Split into: [`params`] (pure sweep/resolution math), [`metrics`] (reading the
//! dialog's already-computed curves at an arbitrary angle), [`overlay`] (layout
//! selection and the bundled bitmap font), [`template`] (the output folder-name
//! template), [`render`] (per-frame CPU tracing), [`encode`] (PNG -> MP4/GIF/README),
//! and [`run`] (the background thread tying all of those together). This file keeps
//! only the UI-thread wiring: reading `TiltVideoExportModel`'s settings, validating
//! them, and handing a fully-resolved request to `run::spawn`.
//!
//! # Scope: CPU-only, local-only rendering
//!
//! Every frame renders on the CPU via `bridge::export_thread::batch::render_batch`
//! (see `render`'s own doc comment) -- this app's single shared GPU adapter must never
//! run two programs at once, and the live viewport keeps using it while a video export
//! runs in the background, so this deliberately never touches the GPU or a remote
//! worker. A full sweep is therefore slower than the still-image export dialog's own
//! hybrid CPU+GPU+remote path; the "Compute" pill offered there has no equivalent here
//! for the same reason, rather than offering a choice that silently only worked partway.

mod encode;
mod metrics;
mod overlay;
mod params;
mod render;
mod run;
mod template;

use crate::{
    ActivityModel, ExportModel, LibraryModel, MainWindow, TiltModel, TiltVideoExportModel,
    bridge::{
        export_thread::{SceneSnapshot, filename_template::TemplateContext},
        render_thread::RenderContext,
    },
};
use indicatrix::color::{ColorSpace, metrics::PROFILE_AZIMUTHS_DEG};
use slint::{ComponentHandle, Model};
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

/// Whichever video-export run is currently in flight, if any: the
/// `crate::ActivityModel` id `handle_start_video_export` registered for it
/// paired with its own cancel flag -- see [`setup_video_export_callback`]'s own
/// doc comment on `current_run` for why both travel together.
type CurrentExportRun = Arc<Mutex<Option<(i32, Arc<AtomicBool>)>>>;

/// Wires `TiltVideoExportModel`'s callbacks. Called from
/// `tilt_profile::setup_tilt_profile_callback` (both are wired from the same
/// `gui::mod::run_gui` call site -- see this module's own `mod.rs`-adjacent doc
/// comment in `tilt_profile.rs`) rather than directly from `gui::mod`.
pub(in crate::gui) fn setup_video_export_callback(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
) {
    // `Some((activity_id, cancel))` for whichever run is currently in flight --
    // `on_cancel_video_export` signals THIS one; a fresh `on_start_video_export`
    // replaces it with a new pair once the previous run has finished (guarded by
    // `TiltVideoExportModel.is_exporting`, so two runs are never in flight at once).
    // `activity_id` is the `crate::ActivityModel` id `handle_start_video_export`
    // registered via `invoke_start_external` -- `on_external_cancel` below
    // reaches back through this SAME `Arc<AtomicBool>` when a cancel click lands
    // on the status strip's own activity chip instead of this dialog's own
    // Cancel button.
    let current_run: CurrentExportRun = Arc::new(Mutex::new(None));

    ui.global::<TiltVideoExportModel>()
        .on_frame_count(|start, end, step| {
            params::frame_count(f64::from(start), f64::from(end), f64::from(step)) as i32
        });

    ui.global::<TiltVideoExportModel>()
        .on_parse_step_deg(|raw| {
            raw.trim()
                .parse::<f64>()
                .map_or(1.0, params::clamp_step_deg) as f32
        });

    // The single source of truth for "which `resolution_preset` index means Custom" --
    // `performance_graph_dialog.slint`'s combo-box custom-size fields read this instead
    // of hard-coding the preset table's length a second time.
    ui.global::<TiltVideoExportModel>()
        .on_custom_resolution_index(|| params::CUSTOM_RESOLUTION_INDEX);

    // Builds the SAME `TemplateContext`/`VideoTemplateExtras` pair `build_request`
    // uses for the real export (`build_video_template_context`, shared below),
    // from the currently open design/material/settings and the axis the dialog
    // passes in, so the "Resolves to" preview always names the design actually
    // open rather than a fixed placeholder. `selected_axis_index` is dialog-local
    // Slint state (`performance_graph_dialog.slint`'s own doc comment on that
    // property), not something `TiltModel`/`TiltVideoExportModel` track, so it
    // travels as this callback's second argument rather than being re-derived here.
    let ui_weak_preview = ui.as_weak();
    let render_ctx_preview = render_ctx.clone();
    ui.global::<TiltVideoExportModel>()
        .on_preview_video_folder_name(move |raw_template, axis_index| {
            let Some(ui) = ui_weak_preview.upgrade() else {
                // No live window to read settings from (e.g. mid-teardown) -- falls
                // back to the fixed sample rather than panicking.
                return template::resolve_folder_name(
                    &raw_template,
                    &sample_template_context(),
                    &sample_template_extras(),
                )
                .into();
            };
            let (ctx, extras) = live_template_context(&ui, &render_ctx_preview, axis_index);
            template::resolve_folder_name(&raw_template, &ctx, &extras).into()
        });

    let ui_weak_start = ui.as_weak();
    let render_ctx_start = render_ctx.clone();
    let current_run_start = current_run.clone();
    ui.global::<TiltVideoExportModel>()
        .on_start_video_export(move |axis_index: i32| {
            let Some(ui) = ui_weak_start.upgrade() else {
                return;
            };
            handle_start_video_export(&ui, &render_ctx_start, &current_run_start, axis_index);
        });

    let current_run_cancel = current_run.clone();
    ui.global::<TiltVideoExportModel>()
        .on_cancel_video_export(move || {
            if let Some((_, cancel)) = current_run
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_ref()
            {
                cancel.store(true, Ordering::Relaxed);
            }
        });

    // The reverse half of `ui/models/activity.slint`'s own bridge doc comment --
    // a Cancel click on the status strip's `ActivityChip` (as opposed to this
    // dialog's own Cancel button, which calls `cancel_video_export` above
    // directly) reaches `ActivityRegistry`'s central `on_cancel` handler first,
    // which finds no local Rust closure for an id started via
    // `invoke_start_external` and instead invokes THIS callback. Only this
    // module registers it (see that same doc comment on why a second
    // externally-cancellable activity kind would need to filter by id/kind
    // itself).
    ui.global::<ActivityModel>().on_external_cancel(move |id| {
        let run = current_run_cancel
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((running_id, cancel)) = run.as_ref()
            && *running_id == id
        {
            cancel.store(true, Ordering::Relaxed);
        }
    });
}

/// `on_start_video_export`'s handler body: validates every setting, and either starts
/// the background render (`run::spawn`) or reports why it couldn't. Split out of
/// `setup_video_export_callback` purely to keep that function short.
///
/// [`resolve_export_directory_then`]'s own folder picker (when the export
/// folder isn't already remembered) runs off the UI thread, so this whole
/// function is itself continuation-passing: the folder is resolved FIRST, and
/// [`build_request`] (validating everything else -- angle range, the tilt
/// curves being ready, resolution/fps settings) only runs once it's in hand.
/// A cutter who has no remembered export folder therefore picks one before
/// being told the sweep hasn't finished computing yet, rather than after -- a
/// minor UX tradeoff accepted rather than adding a second, folder-independent
/// validation pass purely to check that first.
fn handle_start_video_export(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    current_run: &CurrentExportRun,
    axis_index: i32,
) {
    let model = ui.global::<TiltVideoExportModel>();
    if model.get_is_exporting() {
        // The Start button is disabled while exporting (`performance_graph_dialog.slint`),
        // so this is only reachable via a stray double-invoke -- guarded defensively
        // rather than trusted to the UI alone.
        return;
    }

    let render_ctx = Arc::clone(render_ctx);
    let current_run = Arc::clone(current_run);
    resolve_export_directory_then(ui, move |ui, export_dir| {
        let model = ui.global::<TiltVideoExportModel>();
        let Some(export_dir) = export_dir else {
            // The cutter's own explicit cancel -- no error banner.
            return;
        };

        let request = match build_request(ui, &render_ctx, axis_index, &export_dir) {
            Ok(request) => request,
            Err(message) => {
                model.set_has_error(true);
                model.set_status_message(message.into());
                return;
            }
        };

        let cancel = Arc::new(AtomicBool::new(false));
        // Real progress (`run::run`'s own frame counter -- frames done / total,
        // `report_progress`) and a real cancel handle -- registered only once
        // the request itself is known good, so a request that fails to build
        // (an unset curve) never leaves an orphaned activity behind.
        let activity_id = ui.global::<ActivityModel>().invoke_start_external(
            "tilt_video".into(),
            "Tilt video export".into(),
            true,
        );
        *current_run
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some((activity_id, cancel.clone()));

        model.set_is_exporting(true);
        model.set_has_error(false);
        model.set_status_message("Rendering...".into());
        model.set_current_frame(0);
        model.set_total_frames(request.total_frames as i32);
        model.set_progress(0.0);

        run::spawn(ui.as_weak(), request, cancel, activity_id);
    });
}

/// Gathers and validates every `TiltVideoExportModel` setting into a fully-resolved
/// [`run::VideoExportRequest`], or an `Err` message for whichever setting made the
/// request unrunnable (an empty sweep or a not-yet-computed curve). `export_dir`
/// is already resolved by the caller ([`handle_start_video_export`], via
/// [`resolve_export_directory_then`]): that picker runs off the UI thread, so it
/// can no longer be resolved synchronously from inside this otherwise-synchronous
/// builder.
fn build_request(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    axis_index: i32,
    export_dir: &std::path::Path,
) -> Result<run::VideoExportRequest, String> {
    let model = ui.global::<TiltVideoExportModel>();

    let start_deg = params::clamp_angle_deg(f64::from(model.get_start_angle_deg()));
    let end_deg = params::clamp_angle_deg(f64::from(model.get_end_angle_deg()));
    let step_deg = params::clamp_step_deg(f64::from(model.get_step_deg()));
    // (`start_angle_deg`/`end_angle_deg` are `int` in Slint -- `SpinBox`'s own
    // `-90..=90` bounds already keep them in range; `clamp_angle_deg` here is
    // defense-in-depth, not the primary guard.)
    let total_frames = params::frame_count(start_deg, end_deg, step_deg);
    if total_frames == 0 {
        return Err("The angle range/step must produce at least one frame.".to_string());
    }
    if total_frames > params::CONFIRM_FRAME_THRESHOLD {
        // The dialog itself already made the user confirm a run this large
        // (`confirmed_large_run`/`needs_confirmation` in `performance_graph_dialog.slint`)
        // -- this is just an observability breadcrumb for whoever reads the log of a
        // multi-hour run, not a second gate.
        tracing::info!("Starting a large tilt video export: {total_frames} frames");
    }

    let curves = read_axis_curves(ui, axis_index)
        .ok_or_else(|| "The tilt sweep hasn't finished computing yet.".to_string())?;

    let fps = params::resolve_fps(model.get_fps_index());
    let (width, height) = params::resolve_resolution(
        model.get_resolution_preset(),
        model.get_custom_width(),
        model.get_custom_height(),
    );
    let samples_per_pixel = model.get_sample_exponent_raw().round().exp2().round() as u32;
    let max_bounces = params::resolve_max_bounces(model.get_selected_bounce_index());
    let color_space = crate::gui::color_space_from_index(model.get_selected_color_space_index());

    // The export's OWN bounce cap, not whatever the live viewport happens to be set to
    // -- same reasoning as `gui::render_export::apply_export_bounce_cap` (that helper
    // itself is `pub(super)` to a different module, so this overrides the field
    // directly rather than importing it).
    let mut scene = SceneSnapshot::capture(render_ctx);
    scene.max_bounces = max_bounces;

    let selection = metrics::MetricSelection {
        brilliance: model.get_show_values() && model.get_metric_brilliance(),
        windowing: model.get_show_values() && model.get_metric_windowing(),
        extinction: model.get_show_values() && model.get_metric_extinction(),
        tilt_brilliance: model.get_show_values() && model.get_metric_tilt_brilliance(),
        angle: model.get_show_values() && model.get_metric_angle(),
    };

    let detail = ui.global::<LibraryModel>().get_current_detail();
    let (template_ctx, extras) = build_video_template_context(
        detail.title.as_str(),
        detail.designer.as_str(),
        detail.shape.as_str(),
        detail.ri.as_str(),
        scene.material.name.as_str(),
        &scene,
        VideoTemplateSettings {
            axis_index,
            fps,
            width,
            height,
            spp: samples_per_pixel,
            bounces: max_bounces,
            color_space,
            step_deg,
            start_deg,
            end_deg,
            total_frames,
        },
    );
    let raw_template = model.get_filename_template();
    let raw_template = if raw_template.is_empty() {
        template::DEFAULT_VIDEO_TEMPLATE
    } else {
        raw_template.as_str()
    };
    let folder_name = template::resolve_folder_name(raw_template, &template_ctx, &extras);
    let out_dir = template::unique_folder_path(export_dir, &folder_name);
    std::fs::create_dir_all(&out_dir)
        .map_err(|e| format!("Could not create {}: {e}", out_dir.display()))?;

    Ok(run::VideoExportRequest {
        scene,
        axis_index: axis_index.max(0) as usize,
        start_deg,
        end_deg,
        step_deg,
        total_frames,
        fps,
        width,
        height,
        samples_per_pixel,
        color_space,
        out_dir,
        out_name: folder_name,
        selection,
        curves,
        keep_frames: model.get_keep_frames(),
    })
}

/// Reads `axis_index`'s already-computed 181-point curves out of `TiltModel` (populated
/// by `gui::tilt::tilt_profile`'s background sweep, which the dialog always launches
/// before the video export section can be interacted with). `None` if the axis is out
/// of range or the sweep for it hasn't landed yet (fewer than 181 points).
fn read_axis_curves(ui: &MainWindow, axis_index: i32) -> Option<metrics::MetricCurves> {
    if axis_index < 0 {
        return None;
    }
    let idx = axis_index as usize;
    let tilt = ui.global::<TiltModel>();
    let to_vec = |row: slint::ModelRc<f32>| row.iter().collect::<Vec<f32>>();

    let brilliance = to_vec(tilt.get_graph_brilliance_extra_axes().row_data(idx)?);
    let windowing = to_vec(tilt.get_graph_windowing_extra_axes().row_data(idx)?);
    let extinction = to_vec(tilt.get_graph_extinction_extra_axes().row_data(idx)?);
    if brilliance.len() < 181 || windowing.len() < 181 || extinction.len() < 181 {
        return None;
    }
    Some(metrics::MetricCurves {
        brilliance,
        windowing,
        extinction,
    })
}

/// The output directory: reuses `ExportModel.directory` (the still-image export's own
/// remembered folder) when one is already set, or prompts a native folder picker and
/// seeds it back into `ExportModel.directory` for consistency -- the exact same
/// picker/flow `gui::render_export::wiring`'s own `on_start_export` uses, duplicated
/// here rather than invoked cross-module since that helper is private to its own
/// module. Not persisted to the on-disk settings store (unlike the still-image
/// export's own directory): a documented, session-only scope simplification,
/// since reaching the `SettingsStore` handle would mean touching `gui::mod`'s
/// wiring call site.
///
/// The folder picker itself runs off the UI thread via `gui::pickers::pick` --
/// `on_done`'s `None` is either "already had a remembered folder" turned into
/// `Some` synchronously (no picker shown at all) or a real cancel/dismiss of
/// the native dialog; [`handle_start_video_export`] treats both `None` cases
/// identically, since its own caller cannot tell them apart.
fn resolve_export_directory_then(
    ui: &MainWindow,
    on_done: impl FnOnce(&MainWindow, Option<PathBuf>) + 'static,
) {
    let export_model = ui.global::<ExportModel>();
    let current = export_model.get_directory();
    if !current.is_empty() {
        on_done(ui, Some(PathBuf::from(current.as_str())));
        return;
    }
    crate::gui::pickers::pick(
        ui,
        crate::gui::pickers::PickerRequest {
            kind: crate::gui::pickers::PickerKind::PickFolder,
            title: Some("Choose an export folder".to_string()),
            filters: Vec::new(),
            default_file_name: None,
            starting_dir: None,
        },
        move |ui, dir| {
            if let Some(dir) = &dir {
                ui.global::<ExportModel>()
                    .set_directory(dir.to_string_lossy().into_owned().into());
            }
            on_done(ui, dir);
        },
    );
}

/// `axis_index` -> its azimuth label (e.g. `"45"`) for `{axis}` in the folder-name
/// template.
fn axis_label_for_index(axis_index: i32) -> String {
    if axis_index < 0 {
        return "0".to_string();
    }
    PROFILE_AZIMUTHS_DEG
        .get(axis_index as usize)
        .map_or_else(|| "0".to_string(), |deg| format!("{deg:.0}"))
}

/// `ColorSpace`'s own display label -- mirrors
/// `gui::render_export::queue::colorspace_label` (that one is `pub(super)` to a
/// different module, so this is a small, deliberate duplicate rather than a
/// shared call).
const fn colorspace_label(cs: ColorSpace) -> &'static str {
    match cs {
        ColorSpace::Srgb => "sRGB",
        ColorSpace::DisplayP3 => "Display P3",
        ColorSpace::Rec2020 => "Rec.2020",
        ColorSpace::AcesCg => "ACEScg",
    }
}

/// Plain, already-resolved settings for [`build_video_template_context`] -- grouped into
/// one `Copy` struct (rather than passed as individual arguments) purely to keep that
/// function's own argument list under `clippy::too_many_arguments`'s default threshold.
#[derive(Debug, Clone, Copy)]
struct VideoTemplateSettings {
    axis_index: i32,
    fps: u32,
    width: u32,
    height: u32,
    spp: u32,
    bounces: u32,
    color_space: ColorSpace,
    step_deg: f64,
    start_deg: f64,
    end_deg: f64,
    total_frames: usize,
}

/// Builds the `TemplateContext`/`VideoTemplateExtras` pair for a given
/// design/designer/shape/RI/material/scene/settings -- the ONE place both the real
/// export (`build_request`) and the dialog's own live "Resolves to" preview
/// (`on_preview_video_folder_name`, via `live_template_context` below) turn "the
/// currently open design" into a `TemplateContext`, so the two can never disagree
/// about what that means. Pure/plain-argument on purpose so it stays testable
/// without a live `MainWindow` (see this module's own tests).
fn build_video_template_context(
    design: &str,
    designer: &str,
    shape: &str,
    ri: &str,
    material: &str,
    scene: &SceneSnapshot,
    settings: VideoTemplateSettings,
) -> (TemplateContext, template::VideoTemplateExtras) {
    let ctx = TemplateContext {
        design: design.to_string(),
        designer: designer.to_string(),
        shape: shape.to_string(),
        material: material.to_string(),
        ri: ri.to_string(),
        width: settings.width,
        height: settings.height,
        spp: settings.spp,
        bounces: settings.bounces,
        colorspace: colorspace_label(settings.color_space).to_string(),
        preset: String::new(),
        lighting: scene.lighting_preset.label().to_string(),
        yaw_deg: scene.yaw.to_degrees(),
        pitch_deg: scene.pitch.to_degrees(),
        distance: scene.distance,
        exposure: scene.exposure,
    };
    let extras = template::VideoTemplateExtras {
        axis_label: axis_label_for_index(settings.axis_index),
        fps: settings.fps,
        step_deg: settings.step_deg,
        start_deg: settings.start_deg,
        end_deg: settings.end_deg,
        total_frames: settings.total_frames,
    };
    (ctx, extras)
}

/// Wiring layer for [`build_video_template_context`]: reads the currently open
/// design (`LibraryModel`), the current scene/material (`SceneSnapshot::capture`), and
/// the video export's own current settings (`TiltVideoExportModel`) straight off the
/// live UI, for the dialog's "Resolves to" preview. `axis_index` travels as a plain
/// argument rather than being read off `TiltVideoExportModel` because the dialog's
/// `selected_axis_index` is Slint-local state that Rust never otherwise sees (see
/// `performance_graph_dialog.slint`'s own doc comment on that property).
fn live_template_context(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    axis_index: i32,
) -> (TemplateContext, template::VideoTemplateExtras) {
    let model = ui.global::<TiltVideoExportModel>();
    let detail = ui.global::<LibraryModel>().get_current_detail();
    let scene = SceneSnapshot::capture(render_ctx);

    let fps = params::resolve_fps(model.get_fps_index());
    let (width, height) = params::resolve_resolution(
        model.get_resolution_preset(),
        model.get_custom_width(),
        model.get_custom_height(),
    );
    let spp = model.get_sample_exponent_raw().round().exp2().round() as u32;
    let bounces = params::resolve_max_bounces(model.get_selected_bounce_index());
    let color_space = crate::gui::color_space_from_index(model.get_selected_color_space_index());
    let step_deg = params::clamp_step_deg(f64::from(model.get_step_deg()));
    let start_deg = params::clamp_angle_deg(f64::from(model.get_start_angle_deg()));
    let end_deg = params::clamp_angle_deg(f64::from(model.get_end_angle_deg()));
    let total_frames = params::frame_count(start_deg, end_deg, step_deg);

    build_video_template_context(
        detail.title.as_str(),
        detail.designer.as_str(),
        detail.shape.as_str(),
        detail.ri.as_str(),
        scene.material.name.as_str(),
        &scene,
        VideoTemplateSettings {
            axis_index,
            fps,
            width,
            height,
            spp,
            bounces,
            color_space,
            step_deg,
            start_deg,
            end_deg,
            total_frames,
        },
    )
}

/// A fixed example `TemplateContext`, for the dialog's live "Resolves to" preview when
/// no live window is reachable to read the current design/settings from (e.g. mid
/// teardown) -- the same fallback shape `ExportModel.preview_filename_template` uses.
fn sample_template_context() -> TemplateContext {
    TemplateContext {
        design: "Sample Design".to_string(),
        designer: "Sample Designer".to_string(),
        shape: "Round".to_string(),
        material: "Diamond".to_string(),
        ri: "2.42".to_string(),
        width: 1920,
        height: 1080,
        spp: 64,
        bounces: 12,
        colorspace: "sRGB".to_string(),
        preset: String::new(),
        lighting: "Gem Studio Ring Lights".to_string(),
        yaw_deg: 45.0,
        pitch_deg: 30.0,
        distance: 2.4,
        exposure: 1.0,
    }
}

fn sample_template_extras() -> template::VideoTemplateExtras {
    template::VideoTemplateExtras {
        axis_label: "0".to_string(),
        fps: 30,
        step_deg: 1.0,
        start_deg: -90.0,
        end_deg: 90.0,
        total_frames: 181,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn axis_label_for_index_reads_the_profile_azimuth_table() {
        assert_eq!(axis_label_for_index(0), "0");
        assert_eq!(axis_label_for_index(1), "45");
        assert_eq!(axis_label_for_index(-1), "0");
        assert_eq!(axis_label_for_index(999), "0");
    }

    #[test]
    fn colorspace_label_covers_every_variant() {
        assert_eq!(colorspace_label(ColorSpace::Srgb), "sRGB");
        assert_eq!(colorspace_label(ColorSpace::DisplayP3), "Display P3");
        assert_eq!(colorspace_label(ColorSpace::Rec2020), "Rec.2020");
        assert_eq!(colorspace_label(ColorSpace::AcesCg), "ACEScg");
    }

    /// `build_video_template_context` -- the function `live_template_context` feeds
    /// the preview from -- must carry the real design and material all the way
    /// through the rendered folder name, for a design/material pair that is
    /// nothing like the hard-coded sample, so the "Resolves to" preview never
    /// falls back to the fixed placeholder while a real design is open.
    #[test]
    fn preview_reflects_the_real_open_design_and_material_not_the_hard_coded_sample() {
        let scene = SceneSnapshot::capture(&Mutex::new(RenderContext::default()));
        let settings = VideoTemplateSettings {
            axis_index: 0,
            fps: 30,
            width: 1920,
            height: 1080,
            spp: 64,
            bounces: 12,
            color_space: ColorSpace::Srgb,
            step_deg: 1.0,
            start_deg: -90.0,
            end_deg: 90.0,
            total_frames: 181,
        };
        let (ctx, extras) = build_video_template_context(
            "40.014 Eleets",
            "Sample Designer",
            "Round",
            "1.71",
            "Spinel",
            &scene,
            settings,
        );
        assert_eq!(ctx.design, "40.014 Eleets");
        assert_eq!(ctx.material, "Spinel");

        let name = template::resolve_folder_name(template::DEFAULT_VIDEO_TEMPLATE, &ctx, &extras);
        assert!(
            name.contains("40.014 Eleets"),
            "expected the real design name in {name:?}"
        );
        assert!(
            name.contains("Spinel"),
            "expected the real material in {name:?}"
        );
        assert!(
            !name.contains("Sample Design"),
            "must not fall back to the hard-coded sample in {name:?}"
        );
    }

    /// A CPU end-to-end smoke test covering the whole pipeline this module wires up
    /// (frame pose list, per-frame render, overlay, PNG write) at 128x128 with a
    /// handful of frames. Drives `run::render_all_frames`'s pieces directly (not
    /// through the Slint callback plumbing, which needs a live `MainWindow`) so it
    /// can run in a headless `cargo test` process.
    #[test]
    fn end_to_end_cpu_render_at_128x128_with_a_handful_of_frames_and_overlay() {
        use crate::bridge::render_thread::RenderContext;
        use std::sync::{Mutex, atomic::AtomicBool};

        let scene = SceneSnapshot::capture(&Mutex::new(RenderContext::default()));
        let start_deg = -2.0;
        let end_deg = 2.0;
        let step_deg = 2.0; // 3 frames: -2, 0, 2
        let total_frames = params::frame_count(start_deg, end_deg, step_deg);
        assert_eq!(total_frames, 3);

        let dir = std::env::temp_dir().join(format!("tilt_video_e2e_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let curves = metrics::MetricCurves {
            brilliance: vec![50.0; 181],
            windowing: vec![5.0; 181],
            extinction: vec![5.0; 181],
        };
        let selection = metrics::MetricSelection {
            brilliance: true,
            angle: true,
            ..Default::default()
        };
        let cancel = AtomicBool::new(false);

        for index in 0..total_frames {
            let tilt_deg =
                params::frame_angle_deg(start_deg, end_deg, step_deg, index, total_frames);
            let (cam_yaw, cam_pitch) =
                crate::gui::tilt::tilt_hover_preview::camera_pose_for_axis_tilt(0, tilt_deg);
            let mut rgba = render::render_frame_rgba(
                &scene,
                128,
                128,
                1,
                cam_yaw,
                cam_pitch,
                ColorSpace::Srgb,
            );
            assert_eq!(rgba.len(), 128 * 128 * 4);
            let readings = metrics::readings_for_frame(selection, &curves, tilt_deg);
            overlay::draw_overlay(&mut rgba, 128, 128, &readings);

            let path = dir.join(params::frame_file_name(index + 1, total_frames));
            let image =
                image::RgbaImage::from_raw(128, 128, rgba).expect("buffer matches dimensions");
            image.save(&path).expect("frame must save");
            assert!(path.exists());
            let saved = image::open(&path).expect("saved frame must be readable");
            assert_eq!((saved.width(), saved.height()), (128, 128));
        }
        assert!(!cancel.load(std::sync::atomic::Ordering::Relaxed));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
