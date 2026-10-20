//! Persistence of the session: settings and the current design.
//!
//! Where things live (`crate::storage` holds the mechanics):
//! - [`SETTINGS_KEY`] (`indicatrix.settings.v1`, `localStorage`, JSON): a
//!   [`SessionPayload`] (`indicatrix_web_core::settings`, where its format is tested) --
//!   the render settings, shared by every tab. The guide's state (`editor::guide`) uses
//!   the same small-key API ([`read`] / [`write`] / [`discard`]). A value an earlier build
//!   left in `sessionStorage` is migrated once on first read; with `localStorage` blocked
//!   the keys stay in `sessionStorage`.
//! - [`TAB_KEY`] (`indicatrix.tab.v1`, `sessionStorage`, JSON): the two facts about the
//!   design the design file cannot carry (its recorded `.asc` name, and whether it had
//!   unsaved changes). They are per tab, so they are not in the shared payload; a payload
//!   an earlier build wrote still supplies them when the tab has no entry of its own.
//! - [`DESIGN_KEY`] (`indicatrix.design.v1`, `sessionStorage`): the current design as a
//!   `.isdd` design file (`indicatrix_cut_core::native::design_to_string`), the
//!   same format "Save design" downloads, `[meta]` table and attachments included
//!   (unstamped: the autosave does not change the modification time). Same-tab reloads
//!   restore from here first, synchronously. [`ATTACHMENTS_DROPPED_KEY`] marks
//!   an entry stored without its attachments (they did not fit in `sessionStorage`).
//! - `IndexedDB` (`storage::idb`, record `designs/current`): the same design file, always
//!   with its attachments, plus a meta record, written [`IDB_DEBOUNCE`] after the last
//!   change. At start-up, when `sessionStorage` holds no design, this one is restored
//!   (toast "Restored your last design"); a record that is not a valid design file is
//!   ignored with a console warning.
//!
//! Every change calls [`schedule_save`], which writes `sessionStorage` after
//! [`PERSIST_DEBOUNCE`] of quiet and `IndexedDB` after [`IDB_DEBOUNCE`]. Start-up
//! restores silently (apart from the `IndexedDB` toast); a `sessionStorage` entry that does
//! not parse is removed, with a console note, and the app falls back to `IndexedDB`.
//!
//! Every state change funnels through [`schedule_save`], so it is also where the
//! renderer hears about one: it asks `crate::render::request_sync` to compare the
//! scene it would render now with the one it is rendering.

use super::{
    Ctx,
    diagnostics::{console_note, console_warn},
    push::{MessageKind, push_all, show_message},
    settings::{RenderSettings, SessionPayload},
    solve::auto_solve,
    state::{DesignSource, DesignState, WebApp},
};
use crate::storage::{self, idb};
use indicatrix_cut_core::native::{
    AttachmentBlob, DesignExtras, design_from_str, design_to_string,
};
use indicatrix_formats::native::design::{FileKind, detect_kind};
use indicatrix_web_core::{settings::CURRENT_SETTINGS_VERSION, tab_state::TabState};
use slint::{Timer, TimerMode};
use std::time::Duration;

/// The settings key (JSON).
pub const SETTINGS_KEY: &str = "indicatrix.settings.v1";
/// This tab's design name and unsaved flag (`sessionStorage`, JSON, a `TabState`).
pub const TAB_KEY: &str = "indicatrix.tab.v1";
/// The design key (design file text).
pub const DESIGN_KEY: &str = "indicatrix.design.v1";
/// Set (to `1`) while the stored design is missing the attachments of the file it came
/// from because they did not fit in `sessionStorage`.
pub const ATTACHMENTS_DROPPED_KEY: &str = "indicatrix.design.attachments_dropped.v1";
/// Quiet time before a change is written to `sessionStorage` and the settings key.
pub const PERSIST_DEBOUNCE: Duration = Duration::from_millis(500);
/// Quiet time before a change is written to `IndexedDB`.
pub const IDB_DEBOUNCE: Duration = Duration::from_secs(2);

thread_local! {
    /// Debounces the IndexedDB write (restarting the same timer is the debounce).
    static IDB_TIMER: Timer = Timer::default();
}

/// Reads a small key (settings, guide state): `localStorage`, else `sessionStorage`
/// (migrating an old `sessionStorage` entry once). `None` when absent or unavailable.
#[must_use]
pub fn read(key: &str) -> Option<String> {
    storage::small_get(key)
}

/// Writes a small key. Storage failures are console notes, never user errors.
pub fn write(key: &str, value: &str) {
    if !storage::small_set(key, value) {
        console_warn(&format!("could not store {key} in the browser storage"));
    }
}

/// Removes a corrupt small `key` and says so on the console.
pub fn discard(key: &str, why: &str) {
    console_warn(&format!(
        "ignoring the stored {key} ({why}); it has been removed"
    ));
    storage::small_remove(key);
}

/// The stored session payload after its one-time migrations
/// (`SessionPayload::migrated`: a record from before settings version 1 gets GPU rendering
/// switched on, once; the next save writes the version, so a later "off" stays off), or the
/// defaults (GPU rendering on) when nothing is stored or it is corrupt.
fn restore_payload() -> SessionPayload {
    let Some(text) = read(SETTINGS_KEY) else {
        return SessionPayload::default().migrated();
    };
    match SessionPayload::from_json(&text) {
        Ok(payload) => payload.migrated(),
        Err(e) => {
            discard(SETTINGS_KEY, &e);
            SessionPayload::default().migrated()
        }
    }
}

/// This tab's design name and unsaved flag: its own `sessionStorage` entry
/// ([`TAB_KEY`]), else (a tab that has none yet, or data an earlier build wrote) the
/// fields of the settings payload.
fn restore_tab_state() -> TabState {
    if let Some(text) = storage::session::get(TAB_KEY) {
        match TabState::from_json(&text) {
            Ok(state) => return state,
            Err(e) => {
                console_warn(&format!(
                    "ignoring the stored {TAB_KEY} ({e}); it has been removed"
                ));
                storage::session::remove(TAB_KEY);
            }
        }
    }
    TabState::from_payload(&restore_payload())
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

/// Installs `design` as the current one, marking it unsaved when it had unsaved changes.
fn install_restored(ctx: &Ctx, design: DesignState, unsaved: bool) {
    let mut app = ctx.state.borrow_mut();
    app.replace_design(design);
    if unsaved && let Some(design) = app.design.as_mut() {
        // `replace_design` marks the replacement saved; a design that had unsaved
        // changes before the reload still has them. Generations only grow, so
        // `u64::MAX` reads as "never saved" until the next save.
        design.session.saved_generation = u64::MAX;
    }
}

/// Restores the stored design, if any, into `ctx.state`.
///
/// The same-tab copy in `sessionStorage` is restored synchronously (no UI push; the
/// caller pushes everything once). When there is none, or it does not parse, the
/// `IndexedDB` autosave is read in the background; it pushes the UI itself when it lands,
/// and only if no design has been created or opened in the meantime.
pub fn restore_design(ctx: &Ctx) {
    if let Some(text) = storage::session::get(DESIGN_KEY)
        && restore_from_session(ctx, &text)
    {
        return;
    }
    let ctx = ctx.clone();
    wasm_bindgen_futures::spawn_local(async move { restore_from_idb(&ctx).await });
}

/// Restores the `sessionStorage` design `text`; `false` when it did not parse (and was
/// removed).
fn restore_from_session(ctx: &Ctx, text: &str) -> bool {
    let tab = restore_tab_state();
    match restored_design(ctx, text, tab.design_name.clone()) {
        Ok(design) => {
            install_restored(ctx, design, tab.design_unsaved);
            console_note("restored the design from this tab's session");
            if storage::session::get(ATTACHMENTS_DROPPED_KEY).is_some() {
                show_message(
                    ctx,
                    MessageKind::Warning,
                    "The design's attachments (PDF, original files, images) were too large to keep \
                     across a page reload. Open the original .isdd file again before saving \
                     if you want to keep them.",
                );
            }
            true
        }
        Err(e) => {
            console_warn(&format!(
                "ignoring the stored {DESIGN_KEY} ({e}); it has been removed"
            ));
            storage::session::remove(DESIGN_KEY);
            false
        }
    }
}

/// Restores the `IndexedDB` autosave into an app that has no design yet, then refreshes
/// everything. A missing record, an unavailable `IndexedDB` and a record that is not a
/// valid design file all leave the app untouched (the last two with a console warning).
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
async fn restore_from_idb(ctx: &Ctx) {
    let stored = match idb::load_design().await {
        Ok(Some(stored)) => stored,
        Ok(None) => return,
        Err(e) => {
            console_warn(&format!("could not read the autosaved design: {e}"));
            return;
        }
    };
    let Ok(text) = String::from_utf8(stored.bytes) else {
        console_warn("ignoring the autosaved design (not UTF-8 text)");
        return;
    };
    if detect_kind(text.as_bytes()) != FileKind::Design {
        console_warn("ignoring the autosaved design (not a design file)");
        return;
    }
    let (name, unsaved) = stored.meta.map_or_else(
        || {
            let tab = restore_tab_state();
            (tab.design_name, tab.design_unsaved)
        },
        |meta| (meta.design_name, meta.unsaved),
    );
    // Nothing awaits from here to the end: the check and the install are one step, so a
    // design the user opened while the read was pending is never replaced.
    if ctx.state.borrow().design.is_some() {
        return;
    }
    match restored_design(ctx, &text, name) {
        Ok(design) => install_restored(ctx, design, unsaved),
        Err(e) => {
            console_warn(&format!("ignoring the autosaved design ({e})"));
            return;
        }
    }
    if let Some(ui) = ctx.ui.upgrade() {
        push_all(&ui, &ctx.state.borrow());
    }
    crate::editor::guide::check_progress(ctx);
    auto_solve(ctx);
    show_message(ctx, MessageKind::Info, "Restored your last design");
}

/// The design stored under [`DESIGN_KEY`]: a design file, the text this build writes
/// (an earlier build's `.indicatrix` text parses the same).
fn restored_design(
    ctx: &Ctx,
    text: &str,
    design_name: Option<String>,
) -> Result<DesignState, String> {
    let mut app = ctx.state.borrow_mut();
    let loaded = design_from_str(text).map_err(|e| e.to_string())?;
    let (design, _notes, _attention) = crate::io::load::design_from_design_file(
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
    // The design's name and unsaved flag are this tab's own (`sessionStorage`); the
    // shared settings payload leaves its legacy fields empty so another tab never reads
    // them.
    let tab = TabState {
        design_name: app.design.as_ref().and_then(|d| d.asc_filename.clone()),
        design_unsaved: app.design.as_ref().is_some_and(|d| d.session.is_dirty()),
    };
    match tab.to_json() {
        Ok(json) => {
            storage::session::set(TAB_KEY, &json);
        }
        Err(e) => console_warn(&format!("could not serialize the tab state: {e}")),
    }
    let payload = SessionPayload {
        render: app.settings.clone(),
        design_name: None,
        design_unsaved: false,
        auto_solve_budget_ms: app.auto_solve_budget_ms,
        // The migrations ran when the payload was restored; the version records that.
        settings_version: CURRENT_SETTINGS_VERSION,
    };
    match payload.to_json() {
        Ok(json) => write(SETTINGS_KEY, &json),
        Err(e) => console_warn(&format!("could not serialize the settings: {e}")),
    }
    let Some(design) = &app.design else {
        storage::session::remove(DESIGN_KEY);
        storage::session::remove(ATTACHMENTS_DROPPED_KEY);
        return;
    };
    store_design(design);
}

/// Writes the current design to `IndexedDB` in the background (the text is built now, on
/// the caller's stack, so the async write holds no borrow of the state). Does nothing
/// without a design: the previous autosave is kept, so a start-up whose `IndexedDB`
/// restore is still pending cannot erase it.
fn save_idb(app: &WebApp) {
    let Some(design) = &app.design else {
        return;
    };
    let text = match design_text(design, true) {
        Ok(text) => text,
        Err(e) => {
            console_warn(&format!("could not serialize the design: {e}"));
            return;
        }
    };
    let title = if design.metadata.title.is_empty() {
        design.display_name()
    } else {
        design.metadata.title.clone()
    };
    let meta = idb::DesignMeta {
        title,
        saved_at_ms: js_sys::Date::now(),
        generation: design.session.current_generation(),
        design_name: design.asc_filename.clone(),
        unsaved: design.session.is_dirty(),
    };
    wasm_bindgen_futures::spawn_local(async move {
        if let Err(e) = idb::save_design(text.as_bytes(), &meta).await {
            console_warn(&format!("could not autosave the design to IndexedDB: {e}"));
        }
    });
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
/// can say so; the attachments are still in the opened file, and `IndexedDB` ([`save_idb`])
/// keeps them regardless.
fn store_design(design: &DesignState) {
    let full = match design_text(design, true) {
        Ok(text) => text,
        Err(e) => {
            console_warn(&format!("could not serialize the design: {e}"));
            return;
        }
    };
    if storage::session::set(DESIGN_KEY, &full) {
        storage::session::remove(ATTACHMENTS_DROPPED_KEY);
        return;
    }
    if design.attachments.is_empty() {
        console_warn("could not store the design in sessionStorage (too large?)");
        return;
    }
    let stored =
        design_text(design, false).is_ok_and(|text| storage::session::set(DESIGN_KEY, &text));
    if stored {
        storage::session::set(ATTACHMENTS_DROPPED_KEY, "1");
        console_warn("stored the design in sessionStorage without its attachments (too large)");
    } else {
        console_warn("could not store the design in sessionStorage (too large?)");
    }
}

/// Writes the settings and the `sessionStorage` design after [`PERSIST_DEBOUNCE`] of
/// quiet, and the `IndexedDB` autosave after [`IDB_DEBOUNCE`] (restarting the same timers
/// on every call, so a burst of changes writes once).
pub fn schedule_save(ctx: &Ctx) {
    let state = ctx.state.clone();
    ctx.timers
        .persist
        .start(TimerMode::SingleShot, PERSIST_DEBOUNCE, move || {
            save_now(&state.borrow());
        });
    let state = ctx.state.clone();
    IDB_TIMER.with(|timer| {
        timer.start(TimerMode::SingleShot, IDB_DEBOUNCE, move || {
            save_idb(&state.borrow());
        });
    });
    crate::render::request_sync(ctx);
}
