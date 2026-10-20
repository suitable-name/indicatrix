//! Wires deterministic-view settings through the shared context and persistence.
use crate::{
    MainWindow, SettingsModel,
    bridge::render_thread::RenderContext,
    settings::{SettingsPersister, model::RaySettings},
};
use indicatrix::renderer::engine::RayOptions;
use slint::ComponentHandle;
use std::sync::{Arc, Mutex};

pub(in crate::gui) fn setup(
    ui: &MainWindow,
    context: &Arc<Mutex<RenderContext>>,
    store: &Arc<SettingsPersister>,
) {
    let ctx = Arc::clone(context);
    let settings = Arc::clone(store);
    let weak = ui.as_weak();
    ui.global::<SettingsModel>()
        .on_ray_taps_selected(move |taps| {
            let Some(ui) = weak.upgrade() else { return };
            let fire = ui.global::<SettingsModel>().get_ray_fire();
            update(
                &ui,
                &ctx,
                &settings,
                RayOptions {
                    aa_taps: u8::try_from(taps).unwrap_or(8),
                    fire,
                },
            );
        });
    let ctx = Arc::clone(context);
    let settings = Arc::clone(store);
    let weak = ui.as_weak();
    ui.global::<SettingsModel>()
        .on_ray_fire_selected(move |fire| {
            let Some(ui) = weak.upgrade() else { return };
            let taps = ui.global::<SettingsModel>().get_ray_taps();
            update(
                &ui,
                &ctx,
                &settings,
                RayOptions {
                    aa_taps: u8::try_from(taps).unwrap_or(8),
                    fire,
                },
            );
        });
}

fn update(
    ui: &MainWindow,
    context: &Arc<Mutex<RenderContext>>,
    store: &SettingsPersister,
    options: RayOptions,
) {
    let options = options.sanitized();
    {
        let mut context = RenderContext::lock(context);
        context.engine.ray = options;
        context.dirty = true;
    }
    store.update(|settings| {
        settings.settings.ray_trace = RaySettings {
            live_taps: options.aa_taps,
            fire: options.fire,
        }
    });
    ui.global::<SettingsModel>()
        .set_ray_taps(i32::from(options.aa_taps));
    ui.global::<SettingsModel>().set_ray_fire(options.fire);
}
