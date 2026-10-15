//! The Parameters panel's "Reset this tab": `ParametersModel.reset_tab(tab)`.
//!
//! The panel assigns no default in Slint; every default comes from Rust
//! (`settings::model::reset_tab` and the `Default` impls behind it). One dispatch covers the
//! three tabs:
//!
//! - **Stone (0):** the per-design render effects, applied to the open design as one undoable
//!   edit. That needs the editor, so this handler only fires
//!   `ParametersModel.reset_stone_effects`, which the Stone lane wires. The material and the
//!   geometry are never touched.
//! - **Lighting & Environment (1):** `ViewportModel.reset_lighting_tab` (the lighting lane's
//!   handler in `camera_lighting`). The panel calls it directly; the arm here is the same call,
//!   so a Rust-side caller reaches it through this dispatch too.
//! - **Render & Output (2):** the settings reset ([`reset_tab`]) persisted, then the render
//!   context, the controls and the dialogs' remembered values re-pushed
//!   ([`apply_render_output_settings`]).

use crate::{
    MainWindow, ParametersModel, ViewportModel,
    bridge::render_thread::RenderContext,
    gui::{show_toast, startup_settings::apply_render_output_settings},
    settings::{
        SettingsPersister,
        model::{ParameterTab, reset_tab},
    },
};
use slint::ComponentHandle;
use std::sync::{Arc, Mutex};

/// Wires `ParametersModel.reset_tab`.
pub(in crate::gui) fn setup_reset_tab_callback(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
) {
    let render_ctx = Arc::clone(render_ctx);
    let settings_store = Arc::clone(settings_store);
    let ui_weak = ui.as_weak();
    ui.global::<ParametersModel>().on_reset_tab(move |index| {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        match ParameterTab::from_index(index) {
            ParameterTab::Stone => ui.global::<ParametersModel>().invoke_reset_stone_effects(),
            ParameterTab::Lighting => ui.global::<ViewportModel>().invoke_reset_lighting_tab(),
            ParameterTab::RenderOutput => {
                // The same function on the persisted copy and on a copy to push: the settings
                // writer is debounced, and the push must not wait for it.
                let mut settings = settings_store.snapshot().settings;
                let _ = reset_tab(&mut settings, ParameterTab::RenderOutput);
                settings_store.update(|s| {
                    let _ = reset_tab(&mut s.settings, ParameterTab::RenderOutput);
                });
                apply_render_output_settings(&ui, &render_ctx, &settings);
                show_toast(
                    &ui,
                    "Render & Output settings are back to their defaults.",
                    "info",
                );
            }
        }
    });
}
