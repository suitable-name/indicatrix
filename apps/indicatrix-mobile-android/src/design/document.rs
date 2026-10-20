//! The document surface of the Design destination: opening, creating, saving, sharing and
//! the autosave. The logic lives in `indicatrix_mobile_core::document`; this module ties
//! it to the `DocumentModel` global and the platform.
//!
//! | Submodule | Job |
//! |---|---|
//! | `handle` | the UI thread's handle on the state, for platform callbacks |
//! | `guard` | the unsaved-changes questions (Save / Discard / Cancel) |
//! | `open` | the file picker and opening what was picked |
//! | `files` | save, save as, share, export `.asc` |
//! | `library` | open a library entry, save to the library, the recent items |
//!
//! Files arriving from other apps and the Shell's New Design / Open callbacks reach this
//! module through [`request_new_design`], [`request_open`] and [`open_incoming`].

mod files;
mod guard;
mod handle;
mod library;
mod open;

use std::{rc::Rc, time::Duration};

use indicatrix_editor::material::builtin_preset_names;
use indicatrix_mobile_core::{
    app_state::AppState,
    document::{self, NewDesignForm, RawNewDesignForm},
    platform::PickedFile,
};
use slint::{ComponentHandle, ModelRc, SharedString, Timer, TimerMode, VecModel, Weak};

use self::{files::Action, guard::Pending};
use super::Shared;
use crate::{AppWindow, DocumentModel, TemplateCardData};

pub use self::{
    handle::{forget as forget_state, run_on_ui},
    library::open_library_entry,
};

/// How often the autosave looks for a change. Cheap when there is none.
const AUTOSAVE_INTERVAL: Duration = Duration::from_secs(5);

thread_local! {
    /// The autosave timer; it lives as long as the UI thread.
    static AUTOSAVE: Timer = Timer::default();
}

/// Connects the `DocumentModel` callbacks, the autosave and the dialog answers, and
/// restores the design of the last autosave.
pub fn wire(window: &AppWindow, state: &Shared) {
    handle::install(state);
    document::warm_template_cards();

    let model = window.global::<DocumentModel>();
    model.set_material_options(ModelRc::new(VecModel::from(
        builtin_preset_names()
            .into_iter()
            .map(SharedString::from)
            .collect::<Vec<_>>(),
    )));

    let weak = window.as_weak();
    model.on_new_design(action(state, &weak, |window, _state| open_gallery(window)));
    model.on_open(action(state, &weak, request_open));
    model.on_save(action(state, &weak, |window, state| {
        files::start(window, state, Action::Save, false);
    }));
    model.on_save_as(action(state, &weak, |window, state| {
        files::start(window, state, Action::SaveAs, false);
    }));
    model.on_close_design(action(state, &weak, |window, state| {
        guard::confirm_discard(window, state, Pending::Close);
    }));
    model.on_save_to_library(action(state, &weak, |window, state| {
        library::save(window, state, false);
    }));
    model.on_open_recent({
        let (state, weak) = (Rc::clone(state), weak.clone());
        move |index| {
            if let (Some(window), Ok(index)) = (weak.upgrade(), usize::try_from(index)) {
                library::open_recent(&window, &state, index);
            }
        }
    });
    model.on_share(action(state, &weak, |window, state| {
        files::start(window, state, Action::Share, false);
    }));
    model.on_export_asc(action(state, &weak, |window, state| {
        files::start(window, state, Action::ExportAsc, false);
    }));
    model.on_share_asc(action(state, &weak, |window, state| {
        files::start(window, state, Action::ShareAsc, false);
    }));
    model.on_create_empty(action(state, &weak, create_from_form));
    model.on_create_template({
        let (state, weak) = (Rc::clone(state), weak);
        move |index| {
            let (Some(window), Ok(index)) = (weak.upgrade(), usize::try_from(index)) else {
                return;
            };
            guard::confirm_discard(&window, &state, Pending::CreateTemplate(index));
        }
    });

    // The shell's single `dialog-result` callback is installed by `design::wire`, which
    // passes every answer to [`handle_dialog_result`].
    start_autosave(state);
    restore_last_design(window, state);
}

/// Pushes the title, the unsaved-changes flag and whether there is a design to show.
pub fn push(window: &AppWindow, state: &AppState) {
    let model = window.global::<DocumentModel>();
    model.set_title(document::display_title(state).into());
    model.set_file_name(
        state
            .document
            .display_name
            .clone()
            .unwrap_or_default()
            .into(),
    );
    model.set_dirty(state.session.is_dirty());
    model.set_has_design(state.document.has_design(&state.session.design));
    library::push(window, state);
}

/// Shows the New Design page (the Shell's New Design callback and More's row end here).
pub fn request_new_design(window: &AppWindow) {
    open_gallery(window);
}

/// Opens the file picker after settling unsaved changes (the Shell's Open callback).
pub fn request_open(window: &AppWindow, state: &Shared) {
    guard::confirm_discard(window, state, Pending::Open);
}

/// Opens files another app sent ("Open with") as the current design, after settling
/// unsaved changes.
pub fn open_incoming(window: &AppWindow, state: &Shared, files: Vec<PickedFile>) {
    if files.is_empty() {
        return;
    }
    guard::confirm_discard(window, state, Pending::Incoming(files));
}

/// Answers a dialog of the document (the unsaved-changes questions and "Clear Recent
/// Items?"). Returns false for a dialog id that is not the document's. `Shell.dialog-result`
/// has one handler; `wire` installs one that calls this, so a later lane that needs dialogs
/// of its own either chains this function from its handler or asks the lead to merge them.
pub fn handle_dialog_result(window: &AppWindow, state: &Shared, id: i32, confirmed: bool) -> bool {
    guard::handle_dialog_result(window, state, id, confirmed)
        || library::handle_dialog_result(window, state, id, confirmed)
}

/// Closes the New Design page and folds its Empty form.
fn close_gallery(window: &AppWindow) {
    let model = window.global::<DocumentModel>();
    model.set_gallery_open(false);
    model.set_form_open(false);
}

/// Fills the cards and shows the New Design page.
fn open_gallery(window: &AppWindow) {
    let cards: Vec<TemplateCardData> = document::template_cards()
        .into_iter()
        .filter(|card| card.index > 0)
        .map(|card| TemplateCardData {
            index: i32::try_from(card.index).unwrap_or(i32::MAX),
            name: card.name.into(),
            description: card.description.into(),
            detail: card.detail.into(),
            facets: i32::try_from(card.facets).unwrap_or(i32::MAX),
        })
        .collect();
    let model = window.global::<DocumentModel>();
    model.set_cards(ModelRc::new(VecModel::from(cards)));
    model.set_gallery_open(true);
}

/// "Create" on the Empty form: read the fields, check them, settle unsaved changes.
fn create_from_form(window: &AppWindow, state: &Shared) {
    let model = window.global::<DocumentModel>();
    let form = NewDesignForm::from_raw(&RawNewDesignForm {
        shape_index: model.get_form_shape(),
        ratio: &model.get_form_ratio(),
        gear_teeth: model.get_form_gear(),
        symmetry: model.get_form_symmetry(),
        mirror: model.get_form_mirror(),
        material_index: model.get_form_material(),
        width_mm: &model.get_form_width(),
    });
    // Check the whole form now so a mistake is reported before any question is asked.
    match form.and_then(|form| form.to_spec().map(|_| form)) {
        Ok(form) => guard::confirm_discard(window, state, Pending::CreateForm(form)),
        Err(message) => handle::toast(window, &message),
    }
}

/// A callback closure that upgrades the window and runs `job` with it and the state.
fn action(
    state: &Shared,
    weak: &Weak<AppWindow>,
    job: fn(&AppWindow, &Shared),
) -> impl Fn() + 'static {
    let (state, weak) = (Rc::clone(state), weak.clone());
    move || {
        if let Some(window) = weak.upgrade() {
            job(&window, &state);
        }
    }
}

/// Starts the timer that writes the autosave when the design changed.
fn start_autosave(state: &Shared) {
    let state = Rc::clone(state);
    AUTOSAVE.with(|timer| {
        timer.start(TimerMode::Repeated, AUTOSAVE_INTERVAL, move || {
            // A state that is busy (a re-entrant tick) is tried again in five seconds.
            let Ok(mut app) = state.try_borrow_mut() else {
                return;
            };
            if let Err(error) = document::autosave(&mut app) {
                tracing::warn!(%error, "the autosave could not be written");
            }
        });
    });
}

/// Brings back the design of the last autosave, if there is one. The state is replaced
/// now (so the first push already shows it); the auto-solve and the toast follow on the
/// first turn of the event loop, when every surface is wired. The toast is skipped when
/// something replaced the restored session before that turn (the preview's `--template`,
/// a file opened at launch): it would be a lie.
fn restore_last_design(window: &AppWindow, state: &Shared) {
    let restored = document::restore(&mut state.borrow_mut());
    if restored.is_none() {
        return;
    }
    // A design of a library entry that was deleted meanwhile must not come back as that
    // entry's working copy.
    library::settle_restored(window, state);
    // `document::install` continues the generation counter and bumps it, so a moved
    // generation means the restored session is no longer the one on screen.
    let restored_generation = state.borrow().session.current_generation();
    let (weak, state) = (window.as_weak(), Rc::clone(state));
    Timer::single_shot(Duration::ZERO, move || {
        if let Some(window) = weak.upgrade() {
            let replaced = state.borrow().session.current_generation() != restored_generation;
            super::after_edit(&window, &state);
            if !replaced {
                handle::toast(&window, "Restored your last design");
            }
        }
    });
}
