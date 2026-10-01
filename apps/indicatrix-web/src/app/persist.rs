//! Tab-session persistence in `sessionStorage`: it survives a reload of the same tab
//! and is gone when the tab closes.
//!
//! Two keys:
//! - [`SETTINGS_KEY`] (`indicatrix.settings.v1`): JSON, a [`SessionPayload`]
//!   (`indicatrix_web_core::settings`, where its format is tested) -- the render
//!   settings plus the two facts about the design the native file cannot carry (its
//!   recorded `.asc` name, and whether it had unsaved changes);
//! - [`DESIGN_KEY`] (`indicatrix.design.v1`): the current design as a
//!   self-contained native `.indicatrix.toml`
//!   (`indicatrix_cut_core::native::save_native_only_toml`), the same format the
//!   desktop's autosave writes and "Save native only" downloads.
//!
//! Every change calls [`schedule_save`], which writes after [`PERSIST_DEBOUNCE`] of
//! quiet. Start-up restores both silently; an entry that does not parse is
//! removed, with a console note, and the app starts as if it were absent.
//!
//! Every state change funnels through [`schedule_save`], so it is also where the
//! renderer hears about one: it asks `crate::render::request_sync` to compare the
//! scene it would render now with the one it is rendering.

use super::{
    Ctx,
    diagnostics::{console_note, console_warn},
    settings::{RenderSettings, SessionPayload},
    state::{DesignSource, WebApp},
};
use indicatrix_cut_core::native::{SaveExtras, load_native_only, save_native_only_toml};
use slint::TimerMode;
use std::time::Duration;

/// The settings key (JSON).
pub const SETTINGS_KEY: &str = "indicatrix.settings.v1";
/// The design key (native TOML).
pub const DESIGN_KEY: &str = "indicatrix.design.v1";
/// Quiet time before a change is written.
pub const PERSIST_DEBOUNCE: Duration = Duration::from_millis(500);

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.session_storage().ok().flatten()
}

/// Reads `key`, or `None` when absent or storage is unavailable.
pub fn read(key: &str) -> Option<String> {
    storage()?.get_item(key).ok().flatten()
}

/// Writes `value` under `key`. Storage failures are console notes, never user errors.
pub fn write(key: &str, value: &str) {
    if storage().is_none_or(|s| s.set_item(key, value).is_err()) {
        console_warn(&format!("could not store {key} in sessionStorage"));
    }
}

/// Removes a corrupt `key` and says so on the console.
pub fn discard(key: &str, why: &str) {
    console_warn(&format!(
        "ignoring the stored {key} ({why}); it has been removed"
    ));
    if let Some(storage) = storage() {
        let _ = storage.remove_item(key);
    }
}

/// The stored session payload, or the default when absent or corrupt.
fn restore_payload() -> SessionPayload {
    let Some(text) = read(SETTINGS_KEY) else {
        return SessionPayload::default();
    };
    match SessionPayload::from_json(&text) {
        Ok(payload) => payload,
        Err(e) => {
            discard(SETTINGS_KEY, &e);
            SessionPayload::default()
        }
    }
}

/// The stored render settings (sanitized), or the defaults.
#[must_use]
pub fn restore_settings() -> RenderSettings {
    restore_payload().render.sanitized()
}

/// The stored auto-solve budget in milliseconds (`0` is off), or the desktop's 300 ms.
#[must_use]
pub fn restore_auto_solve_budget() -> u32 {
    restore_payload().auto_solve_budget()
}

/// Restores the stored design, if any, into `ctx.state` (no UI push; the caller
/// pushes everything once).
pub fn restore_design(ctx: &Ctx) {
    let Some(text) = read(DESIGN_KEY) else {
        return;
    };
    let payload = restore_payload();
    match load_native_only(&text) {
        Ok(loaded) => {
            let mut app = ctx.state.borrow_mut();
            let (design, _notes, _attention) = crate::io::load::design_from_native_only(
                &mut app,
                loaded,
                payload.design_name,
                DesignSource::Restored,
            );
            app.replace_design(design);
            if payload.design_unsaved
                && let Some(design) = app.design.as_mut()
            {
                // `replace_design` marks the replacement saved; a design that had
                // unsaved changes before the reload still has them. Generations only
                // grow, so `u64::MAX` reads as "never saved" until the next save.
                design.session.saved_generation = u64::MAX;
            }
            console_note("restored the design from this tab's session");
        }
        Err(e) => discard(DESIGN_KEY, &e.to_string()),
    }
}

/// Writes both keys now. Storage failures (quota, privacy mode) are console notes,
/// never user errors: persistence is a convenience.
fn save_now(app: &WebApp) {
    let Some(storage) = storage() else {
        return;
    };
    let payload = SessionPayload {
        render: app.settings.clone(),
        design_name: app.design.as_ref().and_then(|d| d.asc_filename.clone()),
        design_unsaved: app.design.as_ref().is_some_and(|d| d.session.is_dirty()),
        auto_solve_budget_ms: app.auto_solve_budget_ms,
    };
    match payload.to_json() {
        Ok(json) => {
            if storage.set_item(SETTINGS_KEY, &json).is_err() {
                console_warn("could not store the settings in sessionStorage");
            }
        }
        Err(e) => console_warn(&format!("could not serialize the settings: {e}")),
    }
    let Some(design) = &app.design else {
        let _ = storage.remove_item(DESIGN_KEY);
        return;
    };
    let history = design.session.history.description_log().to_vec();
    let toml = save_native_only_toml(
        &design.session.design,
        design.save_asc_name(),
        design.printed_proportions.as_ref(),
        &SaveExtras {
            custom_material: design.custom_snapshot(),
            history_entries: &history,
            custom_catalogue: &app.custom_materials,
        },
    );
    match toml {
        Ok(toml) => {
            if storage.set_item(DESIGN_KEY, &toml).is_err() {
                console_warn("could not store the design in sessionStorage (too large?)");
            }
        }
        Err(e) => console_warn(&format!("could not serialize the design: {e}")),
    }
}

/// Writes both keys after [`PERSIST_DEBOUNCE`] of quiet (restarting the same timer
/// on every call, so a burst of changes writes once).
pub fn schedule_save(ctx: &Ctx) {
    let state = ctx.state.clone();
    ctx.timers
        .persist
        .start(TimerMode::SingleShot, PERSIST_DEBOUNCE, move || {
            save_now(&state.borrow());
        });
    crate::render::request_sync(ctx);
}
