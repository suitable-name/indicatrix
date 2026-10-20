//! Restores options without changing the Solid startup renderer.
use crate::{
    MainWindow, SettingsModel, bridge::render_thread::RenderContext, settings::model::AppSettings,
};
use slint::ComponentHandle;
use std::sync::{Arc, Mutex};

pub(super) fn apply(ui: &MainWindow, context: &Arc<Mutex<RenderContext>>, settings: &AppSettings) {
    let options = settings.ray_trace.options();
    {
        let mut context = RenderContext::lock(context);
        if context.engine.ray != options {
            context.engine.ray = options;
            context.dirty = true;
        }
    }
    ui.global::<SettingsModel>()
        .set_ray_taps(i32::from(options.aa_taps));
    ui.global::<SettingsModel>().set_ray_fire(options.fire);
}
