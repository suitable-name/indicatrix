//! Tab-session persistence in `sessionStorage`: it survives a reload of the same tab
//! and is gone when the tab closes.
//!
//! Two keys:
//! - [`SETTINGS_KEY`] (`indicatrix.settings.v1`): JSON, a [`SessionPayload`]
//!   (`indicatrix_web_core::settings`, where its format is tested) -- the render
//!   settings plus the two facts about the design the design file cannot carry (its
//!   recorded `.asc` name, and whether it had unsaved changes);
//! - [`DESIGN_KEY`] (`indicatrix.design.v1`): the current design as a
//!   `.indicatrix` design file (`indicatrix_cut_core::native::design_to_string`), the
//!   same format "Save design" downloads, `[meta]` table and attachments included
//!   (unstamped: the autosave does not change the modification time). An entry an
//!   earlier build wrote as a self-contained `.indicatrix.toml` still restores.
//!   [`ATTACHMENTS_DROPPED_KEY`] marks an entry stored without its attachments.
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
    push::{MessageKind, show_message},
    settings::{RenderSettings, SessionPayload},
    state::{DesignSource, DesignState, WebApp},
};
use indicatrix_cut_core::native::{
    AttachmentBlob, DesignExtras, design_from_str, design_to_string, load_native_only,
};
use indicatrix_formats::native::design::{FileKind, detect_kind};
use slint::TimerMode;
use std::time::Duration;

/// The settings key (JSON).
pub const SETTINGS_KEY: &str = "indicatrix.settings.v1";
/// The design key (native TOML).
pub const DESIGN_KEY: &str = "indicatrix.design.v1";
/// Set (to `1`) while the stored design is missing the attachments of the file it came
/// from because they did not fit in `sessionStorage`.
pub const ATTACHMENTS_DROPPED_KEY: &str = "indicatrix.design.attachments_dropped.v1";
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
    match restored_design(ctx, &text, payload.design_name.clone()) {
        Ok(design) => {
            {
                let mut app = ctx.state.borrow_mut();
                app.replace_design(design);
                if payload.design_unsaved
                    && let Some(design) = app.design.as_mut()
                {
                    // `replace_design` marks the replacement saved; a design that had
                    // unsaved changes before the reload still has them. Generations only
                    // grow, so `u64::MAX` reads as "never saved" until the next save.
                    design.session.saved_generation = u64::MAX;
                }
            }
            console_note("restored the design from this tab's session");
            if read(ATTACHMENTS_DROPPED_KEY).is_some() {
                show_message(
                    ctx,
                    MessageKind::Warning,
                    "The design's attachments (PDF, original files, images) were too large to keep \
                     across a page reload. Open the original .indicatrix file again before saving \
                     if you want to keep them.",
                );
            }
        }
        Err(e) => discard(DESIGN_KEY, &e),
    }
}

/// The design stored under [`DESIGN_KEY`]: a `.indicatrix` design file (what this build
/// writes) or the older self-contained sidecar text an earlier build wrote.
fn restored_design(
    ctx: &Ctx,
    text: &str,
    design_name: Option<String>,
) -> Result<DesignState, String> {
    let mut app = ctx.state.borrow_mut();
    if detect_kind(text.as_bytes()) == FileKind::Design {
        let loaded = design_from_str(text).map_err(|e| e.to_string())?;
        let (design, _notes, _attention) = crate::io::load::design_from_design_file(
            &mut app,
            loaded,
            design_name,
            DesignSource::Restored,
        );
        return Ok(design);
    }
    let loaded = load_native_only(text).map_err(|e| e.to_string())?;
    let (design, _notes, _attention) = crate::io::load::design_from_native_only(
        &mut app,
        loaded,
        design_name,
        DesignSource::Restored,
    );
    Ok(design)
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
        let _ = storage.remove_item(ATTACHMENTS_DROPPED_KEY);
        return;
    };
    store_design(&storage, design);
}

/// The design as the text stored under [`DESIGN_KEY`], with or without its attachments.
fn design_text(design: &DesignState, with_attachments: bool) -> Result<String, String> {
    let history = design.session.history.description_log().to_vec();
    let attachments: &[AttachmentBlob] = if with_attachments {
        &design.attachments
    } else {
        &[]
    };
    design_to_string(
        &design.session.design,
        design.printed_proportions.as_ref(),
        &DesignExtras {
            custom_material: design.custom_snapshot(),
            history_entries: &history,
            metadata: Some(&design.metadata),
            attachments,
        },
    )
    .map_err(|e| e.to_string())
}

/// Writes the design under [`DESIGN_KEY`]. `sessionStorage` holds a few megabytes at most,
/// and attachments are base64 text of files that can be tens of megabytes, so when the
/// full text does not fit the design is stored again without its attachments (the
/// metadata stays). That drop is recorded under [`ATTACHMENTS_DROPPED_KEY`] so a restore
/// can say so; the attachments are still in the opened file.
fn store_design(storage: &web_sys::Storage, design: &DesignState) {
    let full = match design_text(design, true) {
        Ok(text) => text,
        Err(e) => {
            console_warn(&format!("could not serialize the design: {e}"));
            return;
        }
    };
    if storage.set_item(DESIGN_KEY, &full).is_ok() {
        let _ = storage.remove_item(ATTACHMENTS_DROPPED_KEY);
        return;
    }
    if design.attachments.is_empty() {
        console_warn("could not store the design in sessionStorage (too large?)");
        return;
    }
    let stored =
        design_text(design, false).is_ok_and(|text| storage.set_item(DESIGN_KEY, &text).is_ok());
    if stored {
        let _ = storage.set_item(ATTACHMENTS_DROPPED_KEY, "1");
        console_warn("stored the design in sessionStorage without its attachments (too large)");
    } else {
        console_warn("could not store the design in sessionStorage (too large?)");
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
