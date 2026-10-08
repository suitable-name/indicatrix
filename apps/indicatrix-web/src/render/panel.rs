//! The render settings panel (`ui/components/render_settings_panel.slint`): its
//! values are pushed from `RenderSettings` ([`push_panel`]) and read back whenever the
//! user changes one (`RenderModel.settings-edited`), then persisted -- which also
//! re-renders (`app::persist::schedule_save` asks the renderer to sync).
//!
//! The lighting preset is not here: the panel's combo shares `AppModel.lighting-index`
//! with the header and reports through the header's own `lighting-changed`.

use super::live;
use crate::{
    AppWindow, RenderModel,
    app::{Ctx, persist::schedule_save},
    io::{IncomingFile, open_files},
};
use indicatrix::render_setup::inclusion_scale;
use indicatrix_web_core::settings::RenderSettings;
use slint::ComponentHandle;

/// Copies the panel's values from `settings`.
pub(super) fn push_panel(ui: &AppWindow, settings: &RenderSettings) {
    let model = ui.global::<RenderModel>();
    model.set_exposure(settings.exposure);
    model.set_light_yaw_deg(settings.light_yaw_deg);
    model.set_light_pitch_deg(settings.light_pitch_deg);
    model.set_light_direction_used(settings.uses_light_direction());
    model.set_head_shadow_deg(settings.head_shadow_deg);
    model.set_head_shadow_used(settings.uses_head_shadow());
    model.set_backdrop_index(settings.backdrop_index);
    model.set_bounces(settings.max_bounces as i32);
    model.set_target_spp(settings.target_spp as i32);
    model.set_denoise(settings.denoise);
    model.set_frosted_girdle(settings.frosted_girdle);
    model.set_link_material(settings.link_material);
    model.set_c_axis_override(settings.c_axis_override);
    model.set_c_axis_tilt_deg(settings.c_axis_tilt_deg);
    model.set_c_axis_azimuth_deg(settings.c_axis_azimuth_deg);
    model.set_inclusion_sigma_s(settings.inclusion_sigma_s);
    model.set_inclusion_position(inclusion_scale::sigma_s_to_position(
        settings.inclusion_sigma_s,
    ));
    model.set_edge_rounding(settings.edge_rounding_radius);
    model.set_stone_width_mm(settings.stone_width_mm);
    model.set_use_hdr(settings.use_hdr);
}

/// `settings` with the panel's values (sanitised).
fn read_panel(ui: &AppWindow, settings: &RenderSettings) -> RenderSettings {
    let model = ui.global::<RenderModel>();
    let unsigned = |value: i32| u32::try_from(value).unwrap_or(0);
    // The coefficient follows the slider only when the slider moved: an untouched panel keeps
    // the stored value exactly (a stored value above the slider's top would otherwise be pulled
    // down to it by any other edit).
    let position = model.get_inclusion_position();
    let inclusion_sigma_s =
        if (position - inclusion_scale::sigma_s_to_position(settings.inclusion_sigma_s)).abs()
            > 1e-6
        {
            inclusion_scale::position_to_sigma_s(position)
        } else {
            settings.inclusion_sigma_s
        };
    RenderSettings {
        exposure: model.get_exposure(),
        light_yaw_deg: model.get_light_yaw_deg(),
        light_pitch_deg: model.get_light_pitch_deg(),
        head_shadow_deg: model.get_head_shadow_deg(),
        backdrop_index: model.get_backdrop_index(),
        max_bounces: unsigned(model.get_bounces()),
        target_spp: unsigned(model.get_target_spp()),
        denoise: model.get_denoise(),
        frosted_girdle: model.get_frosted_girdle(),
        link_material: model.get_link_material(),
        c_axis_override: model.get_c_axis_override(),
        c_axis_tilt_deg: model.get_c_axis_tilt_deg(),
        c_axis_azimuth_deg: model.get_c_axis_azimuth_deg(),
        inclusion_sigma_s,
        edge_rounding_radius: model.get_edge_rounding(),
        stone_width_mm: model.get_stone_width_mm(),
        use_hdr: model.get_use_hdr(),
        ..settings.clone()
    }
    .sanitized()
}

/// A panel control changed.
fn on_edited(ctx: &Ctx) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let denoise_changed = {
        let mut app = ctx.state.borrow_mut();
        let next = read_panel(&ui, &app.settings);
        let denoise_changed = next.denoise != app.settings.denoise;
        app.settings = next;
        denoise_changed
    };
    push_panel(&ui, &ctx.state.borrow().settings);
    schedule_save(ctx);
    if denoise_changed {
        live::denoise_setting_changed(ctx);
    }
}

/// "Load HDR environment...": the picker, filtered to `.hdr`, into the app's own
/// file loading (`io::open_files`, which checks the browser caps and keeps the map).
fn load_hdr(ctx: &Ctx) {
    let ctx = ctx.clone();
    wasm_bindgen_futures::spawn_local(async move {
        let Some(handle) = rfd::AsyncFileDialog::new()
            .add_filter("Radiance HDR environment map (.hdr)", &["hdr"])
            .pick_file()
            .await
        else {
            return;
        };
        let file = IncomingFile {
            name: handle.file_name(),
            bytes: handle.read().await,
        };
        open_files(&ctx, vec![file]);
    });
}

pub(super) fn wire(ui: &AppWindow, ctx: &Ctx) {
    let model = ui.global::<RenderModel>();
    let c = ctx.clone();
    model.on_settings_edited(move || on_edited(&c));
    let c = ctx.clone();
    model.on_load_hdr(move || load_hdr(&c));
}
