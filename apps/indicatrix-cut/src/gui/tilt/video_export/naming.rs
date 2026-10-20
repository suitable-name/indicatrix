//! Filename preview helpers shared by direct and queued video exports.
use super::{
    Arc, ComponentHandle, LibraryModel, MainWindow, Mutex, RenderContext, SceneSnapshot,
    TemplateContext, TiltVideoExportModel, VideoTemplateSettings, axis_label_for_index,
    colorspace_label, params, samples_per_pixel_from_exponent, template,
};
/// Builds the `TemplateContext`/`VideoTemplateExtras` pair for a given
/// design/designer/shape/RI/material/scene/settings -- the ONE place both the real
/// export (`build_request`) and the dialog's own live "Resolves to" preview
/// (`on_preview_video_folder_name`, via `live_template_context` below) turn "the
/// currently open design" into a `TemplateContext`, so the two can never disagree
/// about what that means. Pure/plain-argument on purpose so it stays testable
/// without a live `MainWindow` (see this module's own tests).
pub(super) fn build_video_template_context(
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
pub(super) fn live_template_context(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    axis_index: i32,
) -> (TemplateContext, template::VideoTemplateExtras) {
    let model = ui.global::<TiltVideoExportModel>();
    let detail = ui.global::<LibraryModel>().get_current_detail();
    // A refused material (see `SceneSnapshot::capture`'s own doc comment) has nothing
    // real to preview a filename for -- falls back to the same fixed sample the
    // no-live-window branch above uses rather than showing a stale or wrong-material
    // name; the actual export start (`build_request`) refuses for real.
    let Ok(scene) = SceneSnapshot::capture(render_ctx) else {
        return (sample_template_context(), sample_template_extras());
    };

    let fps = params::resolve_fps(model.get_fps_index());
    let (width, height) = params::resolve_resolution(
        model.get_resolution_preset(),
        model.get_custom_width(),
        model.get_custom_height(),
    );
    let spp = samples_per_pixel_from_exponent(model.get_sample_exponent_raw());
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
pub(super) fn sample_template_context() -> TemplateContext {
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

pub(super) fn sample_template_extras() -> template::VideoTemplateExtras {
    template::VideoTemplateExtras {
        axis_label: "0".to_string(),
        fps: 30,
        step_deg: 1.0,
        start_deg: -90.0,
        end_deg: 90.0,
        total_frames: 181,
    }
}
