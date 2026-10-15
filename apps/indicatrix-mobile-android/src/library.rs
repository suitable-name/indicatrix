//! The Library destination's Rust side: the designs saved on this device as cards or
//! rows, with search, filters, sort, tags, import, and opening a design from the list.
//! The data layer is `indicatrix_mobile_core::library`; this module ties it to the
//! `LibraryModel` global.
//!
//! | Submodule | Job |
//! |---|---|
//! | `runtime` | the UI-thread state and the threads behind it |
//! | `list` | opening the database, searching and paging, rows, selection, properties |
//! | `thumbs` | the thumbnail workers and what they are asked for |
//! | `actions` | import, tags, ignore, delete, share |
//! | `filters` | the search text and the filters as a query |
//! | `tags` | which tags to set on the selected designs |
//! | `format` | card subtitles and the import summary |
//!
//! The library opens on the first visit to the destination, on a worker thread. All
//! database work runs on a queue thread and lands through the event loop, checked
//! against a search generation. The dialog ids 3000 to 3099 belong to this module
//! (`design::wire` routes them to [`handle_dialog_result`]).

mod actions;
mod filters;
mod format;
mod list;
mod runtime;
mod tags;
mod thumbs;

use std::{
    collections::{BTreeSet, HashSet},
    rc::Rc,
    time::Duration,
};

use indicatrix_mobile_core::{
    app_state::AppState, layout::Destination, library::FilterOptions, tasks::Generation,
};
use slint::{ComponentHandle, ModelRc, Timer, TimerMode, VecModel, Weak};

use self::{
    filters::FilterState,
    list::Reload,
    runtime::{Runtime, with},
    tags::SheetState,
};
use crate::{AppWindow, LibraryCardData, LibraryModel, TagChipData, design::Shared};

/// How long the search box waits for the typing to pause before it searches.
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(250);

/// Connects the Library's callbacks and, when the app starts on the Library, opens it.
pub fn wire(window: &AppWindow, state: &Shared) {
    let rows = Rc::new(VecModel::<LibraryCardData>::default());
    let tag_add_model = Rc::new(VecModel::<TagChipData>::default());
    let tag_remove_model = Rc::new(VecModel::<TagChipData>::default());
    let (settings, start_here) = {
        let app = state.borrow();
        (
            app.settings.library,
            app.destination == Destination::Library,
        )
    };

    let model = window.global::<LibraryModel>();
    model.set_cards(ModelRc::from(Rc::clone(&rows)));
    model.set_tag_add(ModelRc::from(Rc::clone(&tag_add_model)));
    model.set_tag_remove(ModelRc::from(Rc::clone(&tag_remove_model)));

    runtime::install(Runtime {
        shared: Rc::clone(state),
        weak: window.as_weak(),
        handle: None,
        opening: false,
        queue: None,
        pool: None,
        generation: Generation::default(),
        debounce: Timer::default(),
        rows,
        tag_add_model,
        tag_remove_model,
        tag_sheet: SheetState::default(),
        tag_sheet_open: false,
        filters: FilterState::default(),
        settings,
        options: FilterOptions::default(),
        error: String::new(),
        cards: Vec::new(),
        total: 0,
        loading: false,
        loading_more: false,
        shown_revision: 0,
        range: (0, 0),
        loaded: HashSet::new(),
        failed: HashSet::new(),
        selected: BTreeSet::new(),
        busy: None,
        pending_delete: Vec::new(),
        scroll_token: 0,
    });
    with(|rt| {
        list::sync_options(rt, window);
        list::sync(rt, window);
    });

    wire_page(window, state);
    wire_filters(window);
    wire_selection(window);

    if start_here {
        list::open_and_load(window);
    }
}

/// Pushes the Library from the app state: the list settings, and a reload when the
/// library changed behind the page (a design saved to it from the Design page).
pub fn push(window: &AppWindow, state: &AppState) {
    let revision = state.library.revision();
    let settings = state.settings.library;
    with(|rt| {
        list::settings_changed(rt, settings);
        if rt.handle.is_some() && revision != rt.shown_revision {
            list::reload(rt, revision, Reload::Refresh);
        }
        list::sync(rt, window);
    });
}

/// Ends the Library's threads and drops every handle it holds on the app state.
pub fn shutdown() {
    runtime::shutdown();
}

/// Answers the Library's dialogs (ids 3000 to 3099). Returns false for any other id.
pub fn handle_dialog_result(window: &AppWindow, _state: &Shared, id: i32, confirmed: bool) -> bool {
    actions::handle_dialog_result(window, id, confirmed)
}

/// Asks for design files and imports them into the library ("Import Files" in the
/// shell and the Library's own button end here).
pub fn import_files(window: &AppWindow, _state: &Shared) {
    actions::import_designs(window);
}

/// A callback that upgrades the window and runs `job` with it.
fn on_window(window: &AppWindow, job: fn(&AppWindow)) -> impl Fn() + 'static {
    let weak = window.as_weak();
    move || {
        if let Some(window) = weak.upgrade() {
            job(&window);
        }
    }
}

/// The page, the search box, the list and the settings.
fn wire_page(window: &AppWindow, state: &Shared) {
    let model = window.global::<LibraryModel>();
    let weak = window.as_weak();

    model.on_page_shown(on_window(window, list::open_and_load));
    model.on_import_designs(on_window(window, actions::import_designs));

    model.on_search_edited({
        let weak = weak.clone();
        move |text| {
            with(|rt| {
                rt.filters.text = text.to_string();
                let weak = weak.clone();
                rt.debounce
                    .start(TimerMode::SingleShot, SEARCH_DEBOUNCE, move || {
                        if let Some(window) = weak.upgrade() {
                            restart(&window);
                        }
                    });
            });
        }
    });

    model.on_set_list_mode({
        let (state, weak) = (Rc::clone(state), weak.clone());
        move |on| change_settings(&state, &weak, |settings| settings.list_mode = on)
    });
    model.on_set_sort({
        let (state, weak) = (Rc::clone(state), weak.clone());
        move |index| {
            if let Some(sort) = filters::sort_from_index(index) {
                change_settings(&state, &weak, |settings| settings.sort = sort);
            }
        }
    });

    model.on_visible_range(|first, last| {
        with(|rt| list::set_range(rt, first, last));
    });

    model.on_card_tapped({
        let (state, weak) = (Rc::clone(state), weak.clone());
        move |index| {
            let (Some(window), Ok(index)) = (weak.upgrade(), usize::try_from(index)) else {
                return;
            };
            if let Some(id) = list::tap(&window, index) {
                actions::open_entry(&window, &state, id);
            }
        }
    });
    model.on_card_long_pressed(move |index| {
        if let (Some(window), Ok(index)) = (weak.upgrade(), usize::try_from(index)) {
            list::long_press(&window, index);
        }
    });
}

/// Writes the list settings through the app state and applies them to the page.
fn change_settings(
    state: &Shared,
    weak: &Weak<AppWindow>,
    edit: impl FnOnce(&mut indicatrix_mobile_core::settings::LibrarySettings),
) {
    let Some(window) = weak.upgrade() else {
        return;
    };
    let Ok(mut app) = state.try_borrow_mut() else {
        return;
    };
    app.update_settings(|settings| edit(&mut settings.library));
    let settings = app.settings.library;
    drop(app);
    with(|rt| {
        list::settings_changed(rt, settings);
        list::sync(rt, &window);
    });
}

/// Searches again from the top with the current text and filters.
fn restart(window: &AppWindow) {
    with(|rt| {
        if rt.handle.is_none() {
            return;
        }
        let revision = list::current_revision(rt);
        list::reload(rt, revision, Reload::Restart);
        list::sync(rt, window);
    });
}

/// Changes the filters with `edit`, then searches again from the top.
fn change_filters(window: &AppWindow, edit: impl FnOnce(&mut Runtime)) {
    with(|rt| {
        edit(rt);
        if rt.handle.is_some() {
            let revision = list::current_revision(rt);
            list::reload(rt, revision, Reload::Restart);
        }
        list::sync(rt, window);
    });
}

/// The filter sheet and the "Clear" buttons.
fn wire_filters(window: &AppWindow) {
    let model = window.global::<LibraryModel>();
    let weak = window.as_weak();

    model.on_set_shape({
        let weak = weak.clone();
        move |index| {
            if let Some(window) = weak.upgrade() {
                change_filters(&window, |rt| rt.filters.pick_shape(&rt.options, index));
            }
        }
    });
    model.on_set_gear({
        let weak = weak.clone();
        move |index| {
            if let Some(window) = weak.upgrade() {
                change_filters(&window, |rt| rt.filters.pick_gear(&rt.options, index));
            }
        }
    });
    model.on_set_ri({
        let weak = weak.clone();
        move |index| {
            if let Some(window) = weak.upgrade() {
                change_filters(&window, |rt| rt.filters.pick_ri(index));
            }
        }
    });
    model.on_set_tag({
        let weak = weak.clone();
        move |index| {
            if let Some(window) = weak.upgrade() {
                change_filters(&window, |rt| rt.filters.pick_tag(&rt.options, index));
            }
        }
    });
    model.on_set_mine_only({
        let weak = weak.clone();
        move |on| {
            if let Some(window) = weak.upgrade() {
                change_filters(&window, |rt| rt.filters.mine_only = on);
            }
        }
    });
    model.on_set_show_ignored({
        let weak = weak.clone();
        move |on| {
            if let Some(window) = weak.upgrade() {
                change_filters(&window, |rt| rt.filters.show_ignored = on);
            }
        }
    });
    model.on_clear_filters({
        move || {
            if let Some(window) = weak.upgrade() {
                change_filters(&window, |rt| {
                    rt.filters.clear_all();
                    rt.debounce.stop();
                });
                window.global::<LibraryModel>().set_query_text("".into());
            }
        }
    });
}

/// Selection mode and its actions.
fn wire_selection(window: &AppWindow) {
    let model = window.global::<LibraryModel>();

    model.on_select_all(on_window(window, |window| {
        with(|rt| list::select_all(rt, window));
    }));
    model.on_clear_selection(on_window(window, |window| {
        with(|rt| list::clear_selection(rt, window));
    }));
    model.on_open_tag_sheet(on_window(window, actions::open_tag_sheet));
    model.on_close_tag_sheet(on_window(window, actions::close_tag_sheet));
    model.on_toggle_tag_add(actions::toggle_add);
    model.on_toggle_tag_remove(actions::toggle_remove);
    model.on_add_new_tag(on_window(window, actions::add_new_tag));
    model.on_apply_tags(on_window(window, actions::apply_tags));
    model.on_toggle_ignore_selected(on_window(window, actions::toggle_ignore_selected));
    model.on_share_selected(on_window(window, actions::share_selected));
    model.on_delete_selected(on_window(window, actions::ask_delete_selected));
}
