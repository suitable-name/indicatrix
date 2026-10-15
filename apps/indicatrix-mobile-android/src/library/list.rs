//! The Library list: opening the database, searching and paging, the card rows,
//! thumbnails landing, selection, and the properties the page reads.
//!
//! Everything here runs on the UI thread against the [`Runtime`]; the database work it
//! starts runs on the runtime's queue and comes back through `upgrade_in_event_loop`,
//! checked against the search generation so an older answer never replaces a newer one.

use std::{
    collections::{HashMap, HashSet},
    rc::Rc,
    sync::Arc,
};

use indicatrix_mobile_core::{
    library::{EntryId, FilterOptions, LibraryCard, LibraryHandle, LibraryQuery, Thumbnail},
    settings::LibrarySettings,
    tasks::BackgroundQueue,
};
use slint::{ComponentHandle, Image, Model, ModelRc, SharedString, VecModel};

use super::{
    filters::{self, index_i32},
    format,
    runtime::{self, PAGE, Runtime},
    thumbs::{self, Job, ThumbPool},
};
use crate::{AppWindow, LibraryCardData, LibraryModel, push::toast};

/// How many cards before the end of the loaded list the next page is asked for.
const MORE_MARGIN: usize = 12;

/// How a reload treats the list on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Reload {
    /// A new search, filter or sort: back to the top, one page.
    Restart,
    /// The data changed underneath: keep as many cards as are shown now.
    Refresh,
}

// ---------------------------------------------------------------------------------
// Opening
// ---------------------------------------------------------------------------------

/// Makes `handle` the runtime's library: starts the queue (if it is not running) and the
/// thumbnail workers.
fn attach(rt: &mut Runtime, handle: LibraryHandle) {
    rt.error.clear();
    if rt.queue.is_none() {
        rt.queue = Some(BackgroundQueue::new("library"));
    }
    rt.pool = Some(ThumbPool::start(&handle, &rt.weak));
    rt.handle = Some(handle);
}

/// The open library, opened on the calling (UI) thread when it is not open yet. Used by
/// the rare callers that cannot wait for [`open_and_load`]'s background open (an import
/// started before the page was ever shown). A failure is kept in the runtime and shown
/// by the page.
pub(super) fn ensure_open(window: &AppWindow) -> Option<LibraryHandle> {
    let (shared, existing) = runtime::with(|rt| (Rc::clone(&rt.shared), rt.handle.clone()))?;
    if existing.is_some() {
        return existing;
    }
    let platform = match shared.try_borrow() {
        Ok(app) => Arc::clone(app.platform()),
        Err(_) => return None,
    };
    let opened = match shared.try_borrow_mut() {
        Ok(mut app) => app.library.handle(platform.app_dirs()),
        Err(_) => Err("The app was busy. Try again.".to_owned()),
    };
    match opened {
        Ok(handle) => {
            runtime::with(|rt| {
                if rt.handle.is_none() {
                    attach(rt, handle.clone());
                    let revision = current_revision(rt);
                    reload(rt, revision, Reload::Restart);
                }
                sync(rt, window);
            });
            Some(handle)
        }
        Err(message) => {
            tracing::warn!(%message, "the library could not be opened");
            runtime::with(|rt| {
                rt.error = message;
                sync(rt, window);
            });
            None
        }
    }
}

/// The page became visible: opens the library on the first visit (on a worker thread),
/// and catches up with any change made while the page was away.
pub(super) fn open_and_load(window: &AppWindow) {
    let (open, opening) =
        runtime::with(|rt| (rt.handle.is_some(), rt.opening)).unwrap_or((false, false));
    if open {
        runtime::with(|rt| {
            let revision = current_revision(rt);
            if revision != rt.shown_revision {
                reload(rt, revision, Reload::Refresh);
            }
            sync(rt, window);
        });
    } else if !opening {
        begin_open(window);
    }
}

/// Opens the database on the queue's thread; [`opened`] continues on the UI thread.
fn begin_open(window: &AppWindow) {
    let opener = runtime::with(|rt| {
        rt.shared
            .try_borrow()
            .ok()
            .map(|app| app.library.opener(app.platform().app_dirs()))
    })
    .flatten();
    let Some(opener) = opener else {
        return;
    };
    runtime::with(|rt| {
        rt.error.clear();
        rt.opening = true;
        let weak = rt.weak.clone();
        rt.queue
            .get_or_insert_with(|| BackgroundQueue::new("library"))
            .submit(move || {
                let result = opener.open();
                let delivered = weak.upgrade_in_event_loop(move |window| {
                    opened(&window, result);
                });
                if let Err(error) = delivered {
                    tracing::debug!(%error, "the library opened after the UI thread was gone");
                }
            });
        sync(rt, window);
    });
}

/// The database was opened (or could not be) on the queue's thread.
fn opened(window: &AppWindow, result: Result<LibraryHandle, String>) {
    let Some(shared) = runtime::with(|rt| {
        rt.opening = false;
        Rc::clone(&rt.shared)
    }) else {
        return;
    };
    match result {
        Ok(handle) => {
            // The app state keeps the handle too (the document code reaches the library
            // through it); a handle it already holds wins, so there is one connection.
            let handle = match shared.try_borrow_mut() {
                Ok(mut app) => {
                    app.library.adopt(handle.clone());
                    app.library.current().unwrap_or(handle)
                }
                Err(_) => handle,
            };
            runtime::with(|rt| {
                if rt.handle.is_none() {
                    attach(rt, handle);
                    let revision = current_revision(rt);
                    reload(rt, revision, Reload::Restart);
                }
                sync(rt, window);
            });
        }
        Err(message) => {
            tracing::warn!(%message, "the library could not be opened");
            runtime::with(|rt| {
                rt.error = message;
                sync(rt, window);
            });
        }
    }
}

/// The library's change counter, read from the app state (the last known value when the
/// state is busy).
pub(super) fn current_revision(rt: &Runtime) -> u64 {
    rt.shared
        .try_borrow()
        .map_or(rt.shown_revision, |app| app.library.revision())
}

// ---------------------------------------------------------------------------------
// Searching and paging
// ---------------------------------------------------------------------------------

/// The query for the current search text, filters and sort.
fn query_of(rt: &Runtime) -> LibraryQuery {
    rt.filters.to_query(rt.settings.sort)
}

/// Starts a search for the first page (or, for [`Reload::Refresh`], as many cards as are
/// shown now) and the filter options. The answer lands in [`apply_first`].
pub(super) fn reload(rt: &mut Runtime, revision: u64, mode: Reload) {
    let Some(handle) = rt.handle.clone() else {
        return;
    };
    let limit = match mode {
        Reload::Restart => PAGE,
        Reload::Refresh => rt.cards.len().max(PAGE),
    };
    let generation = rt.generation.advance();
    let query = query_of(rt);
    let (counter, weak) = (rt.generation.clone(), rt.weak.clone());
    let Some(queue) = rt.queue.as_ref() else {
        return;
    };
    queue.submit(move || {
        if !counter.is_current(generation) {
            return;
        }
        let options = handle.filter_options();
        let result = handle.search(&query, 0, limit);
        let delivered = weak.upgrade_in_event_loop(move |window| {
            apply_first(&window, generation, options, result);
        });
        if let Err(error) = delivered {
            tracing::debug!(%error, "a library search was dropped: the UI thread is gone");
        }
    });
    rt.loading = true;
    rt.loading_more = false;
    rt.shown_revision = revision;
    if mode == Reload::Restart {
        rt.scroll_token = rt.scroll_token.wrapping_add(1);
    }
}

/// The first page of a search arrived.
fn apply_first(
    window: &AppWindow,
    generation: u64,
    options: Result<FilterOptions, String>,
    result: Result<(Vec<LibraryCard>, usize), String>,
) {
    let notice = runtime::with(|rt| {
        if !rt.generation.is_current(generation) {
            return None;
        }
        rt.loading = false;
        let mut restart = false;
        if let Ok(options) = options {
            restart = rt.filters.prune(&options);
            rt.options = options;
            sync_options(rt, window);
        }
        let notice = match result {
            Ok((cards, total)) => {
                install(rt, cards, total);
                None
            }
            Err(message) => Some(message),
        };
        if restart {
            let revision = current_revision(rt);
            reload(rt, revision, Reload::Restart);
        }
        sync(rt, window);
        notice
    })
    .flatten();
    if let Some(message) = notice {
        toast::show(window, &format!("Search failed: {message}"));
    }
}

/// Replaces the shown cards. The images of cards that did not change are kept.
fn install(rt: &mut Runtime, cards: Vec<LibraryCard>, total: usize) {
    let previous: HashMap<EntryId, (i64, Image)> = rt
        .cards
        .iter()
        .enumerate()
        .filter(|(index, _)| rt.loaded.contains(index))
        .filter_map(|(index, card)| {
            rt.rows
                .row_data(index)
                .map(|row| (card.id, (card.updated_unix, row.thumb)))
        })
        .collect();
    let present: HashSet<EntryId> = cards.iter().map(|card| card.id).collect();
    rt.selected.retain(|id| present.contains(id));

    let mut loaded = HashSet::new();
    let rows: Vec<LibraryCardData> = cards
        .iter()
        .enumerate()
        .map(|(index, card)| {
            let thumb = previous
                .get(&card.id)
                .filter(|(updated, _)| *updated == card.updated_unix)
                .map(|(_, image)| image.clone());
            if thumb.is_some() {
                loaded.insert(index);
            }
            card_row(card, rt.selected.contains(&card.id), thumb)
        })
        .collect();
    rt.rows.set_vec(rows);
    rt.cards = cards;
    rt.total = total;
    rt.loaded = loaded;
    rt.loading_more = false;
    request_thumbnails(rt);
}

/// A further page arrived.
fn apply_more(
    window: &AppWindow,
    generation: u64,
    offset: usize,
    result: Result<(Vec<LibraryCard>, usize), String>,
) {
    let notice = runtime::with(|rt| {
        if !rt.generation.is_current(generation) {
            return None;
        }
        rt.loading_more = false;
        let mut notice = None;
        match result {
            Ok((page, total)) if rt.cards.len() == offset => {
                let empty = page.is_empty();
                for card in page {
                    rt.rows
                        .push(card_row(&card, rt.selected.contains(&card.id), None));
                    rt.cards.push(card);
                }
                // A page past the end (the query caps its rows below the total) ends
                // the paging instead of asking again.
                rt.total = if empty { rt.cards.len() } else { total };
                request_thumbnails(rt);
                load_more_if_needed(rt);
            }
            Ok(_) => {}
            Err(message) => notice = Some(message),
        }
        sync(rt, window);
        notice
    })
    .flatten();
    if let Some(message) = notice {
        toast::show(window, &format!("Search failed: {message}"));
    }
}

/// Asks for the next page when the visible range is near the end of what is loaded.
fn load_more_if_needed(rt: &mut Runtime) {
    if rt.loading
        || rt.loading_more
        || rt.cards.len() >= rt.total
        || rt.range.1 + MORE_MARGIN < rt.cards.len()
    {
        return;
    }
    let Some(handle) = rt.handle.clone() else {
        return;
    };
    let offset = rt.cards.len();
    let generation = rt.generation.current();
    let query = query_of(rt);
    let (counter, weak) = (rt.generation.clone(), rt.weak.clone());
    let Some(queue) = rt.queue.as_ref() else {
        return;
    };
    queue.submit(move || {
        if !counter.is_current(generation) {
            return;
        }
        let result = handle.search(&query, offset, PAGE);
        let delivered = weak.upgrade_in_event_loop(move |window| {
            apply_more(&window, generation, offset, result);
        });
        if let Err(error) = delivered {
            tracing::debug!(%error, "a library page was dropped: the UI thread is gone");
        }
    });
    rt.loading_more = true;
}

/// The page reports which cards are on screen.
pub(super) fn set_range(rt: &mut Runtime, first: i32, last: i32) {
    let first = usize::try_from(first).unwrap_or(0);
    let last = usize::try_from(last).unwrap_or(0).max(first);
    if rt.range == (first, last) {
        return;
    }
    rt.range = (first, last);
    load_more_if_needed(rt);
    request_thumbnails(rt);
}

// ---------------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------------

/// The list settings may have changed (sort order, list or grid).
pub(super) fn settings_changed(rt: &mut Runtime, settings: LibrarySettings) {
    if rt.settings == settings {
        return;
    }
    let previous = std::mem::replace(&mut rt.settings, settings);
    if previous.list_mode != settings.list_mode {
        // The thumbnails were made for the other mode's size.
        for index in std::mem::take(&mut rt.loaded) {
            set_thumb(rt, index, None);
        }
        request_thumbnails(rt);
    }
    if previous.sort != settings.sort {
        let revision = current_revision(rt);
        reload(rt, revision, Reload::Restart);
    }
}

// ---------------------------------------------------------------------------------
// Rows and thumbnails
// ---------------------------------------------------------------------------------

/// The row a card is drawn from.
fn card_row(card: &LibraryCard, selected: bool, thumb: Option<Image>) -> LibraryCardData {
    LibraryCardData {
        title: format::title(card).into(),
        designer: card.designer.trim().into(),
        subtitle: format::subtitle(card).into(),
        tags: ModelRc::new(VecModel::from(
            card.tags
                .iter()
                .take(3)
                .map(|tag| SharedString::from(tag.as_str()))
                .collect::<Vec<_>>(),
        )),
        has_thumb: thumb.is_some(),
        thumb: thumb.unwrap_or_default(),
        selected,
        mine: card.mine,
        ignored: card.ignored,
    }
}

/// Puts `image` (or none) into the row at `index`.
fn set_thumb(rt: &Runtime, index: usize, image: Option<Image>) {
    if let Some(mut row) = rt.rows.row_data(index) {
        row.has_thumb = image.is_some();
        row.thumb = image.unwrap_or_default();
        rt.rows.set_row_data(index, row);
    }
}

/// The long edge of the thumbnails the current mode needs.
const fn current_edge(rt: &Runtime) -> u32 {
    if rt.settings.list_mode {
        thumbs::LIST_EDGE
    } else {
        thumbs::GRID_EDGE
    }
}

/// Tells the thumbnail workers which cards need a picture now.
pub(super) fn request_thumbnails(rt: &Runtime) {
    let Some(pool) = &rt.pool else {
        return;
    };
    let jobs = thumbs::wishes(
        rt.cards.len(),
        &|index| rt.cards[index].id,
        rt.range,
        &|index, id| rt.loaded.contains(&index) || rt.failed.contains(&id),
        current_edge(rt),
    );
    pool.set_wanted(jobs);
}

/// A thumbnail was made (on a worker); puts it into its card's row.
pub(super) fn thumbnail_ready(job: &Job, result: Result<Thumbnail, String>) {
    runtime::with(|rt| {
        let index = if rt
            .cards
            .get(job.index)
            .is_some_and(|card| card.id == job.id)
        {
            Some(job.index)
        } else {
            rt.cards.iter().position(|card| card.id == job.id)
        };
        let Some(index) = index else {
            return;
        };
        match result {
            Ok(thumbnail) if job.edge == current_edge(rt) => match thumbs::to_image(&thumbnail) {
                Some(image) => {
                    set_thumb(rt, index, Some(image));
                    rt.loaded.insert(index);
                    evict_far_images(rt);
                }
                None => {
                    rt.failed.insert(job.id);
                }
            },
            // Made for the other mode's size: the next request asks again.
            Ok(_) => {}
            Err(message) => {
                tracing::debug!(id = job.id, %message, "a thumbnail could not be made");
                rt.failed.insert(job.id);
            }
        }
    });
}

/// Drops the images farthest from the visible range once too many rows hold one.
fn evict_far_images(rt: &mut Runtime) {
    if rt.loaded.len() <= thumbs::MAX_LOADED {
        return;
    }
    let centre = usize::midpoint(rt.range.0, rt.range.1);
    let mut by_distance: Vec<usize> = rt.loaded.iter().copied().collect();
    by_distance.sort_by_key(|index| index.abs_diff(centre));
    for index in by_distance.split_off(thumbs::MAX_LOADED) {
        set_thumb(rt, index, None);
        rt.loaded.remove(&index);
    }
}

// ---------------------------------------------------------------------------------
// Selection
// ---------------------------------------------------------------------------------

/// Marks the row at `index` selected or not.
fn set_row_selected(rt: &Runtime, index: usize, selected: bool) {
    if let Some(mut row) = rt.rows.row_data(index)
        && row.selected != selected
    {
        row.selected = selected;
        rt.rows.set_row_data(index, row);
    }
}

/// A tap on a card: in selection mode it toggles the card, else the card's entry is
/// returned so the caller can open it.
pub(super) fn tap(window: &AppWindow, index: usize) -> Option<EntryId> {
    runtime::with(|rt| {
        let id = rt.cards.get(index)?.id;
        if rt.selected.is_empty() {
            return Some(id);
        }
        if !rt.selected.remove(&id) {
            rt.selected.insert(id);
        }
        let on = rt.selected.contains(&id);
        set_row_selected(rt, index, on);
        sync(rt, window);
        None
    })
    .flatten()
}

/// A long press on a card starts the selection with it (or keeps it selected).
pub(super) fn long_press(window: &AppWindow, index: usize) {
    runtime::with(|rt| {
        let Some(id) = rt.cards.get(index).map(|card| card.id) else {
            return;
        };
        rt.selected.insert(id);
        set_row_selected(rt, index, true);
        sync(rt, window);
    });
}

/// Selects every loaded card.
pub(super) fn select_all(rt: &mut Runtime, window: &AppWindow) {
    rt.selected = rt.cards.iter().map(|card| card.id).collect();
    for index in 0..rt.cards.len() {
        set_row_selected(rt, index, true);
    }
    sync(rt, window);
}

/// Leaves selection mode.
pub(super) fn clear_selection(rt: &mut Runtime, window: &AppWindow) {
    rt.selected.clear();
    for index in 0..rt.rows.row_count() {
        set_row_selected(rt, index, false);
    }
    sync(rt, window);
}

/// The selected cards, in list order.
pub(super) fn selected_cards(rt: &Runtime) -> Vec<&LibraryCard> {
    rt.cards
        .iter()
        .filter(|card| rt.selected.contains(&card.id))
        .collect()
}

// ---------------------------------------------------------------------------------
// Properties
// ---------------------------------------------------------------------------------

/// Which empty state applies: 0 none, 1 the library has no designs, 2 nothing matches.
fn empty_kind(rt: &Runtime) -> i32 {
    if rt.handle.is_none() || rt.loading || !rt.cards.is_empty() {
        0
    } else if rt.filters.is_narrowing() {
        2
    } else {
        1
    }
}

/// Writes the option lists of the filter sheet. They change only when the library does,
/// and replacing them rebuilds the chips, so this is not part of [`sync`].
pub(super) fn sync_options(rt: &Runtime, window: &AppWindow) {
    let model = window.global::<LibraryModel>();
    model.set_shape_options(strings(filters::shape_labels(&rt.options)));
    model.set_gear_options(strings(filters::gear_labels(&rt.options)));
    model.set_ri_options(strings(filters::ri_labels()));
    model.set_tag_options(strings(rt.options.tags.clone()));
}

/// A string model for a property.
fn strings(items: Vec<String>) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(
        items
            .into_iter()
            .map(SharedString::from)
            .collect::<Vec<_>>(),
    ))
}

/// Writes every scalar property of the page from the runtime.
pub(super) fn sync(rt: &Runtime, window: &AppWindow) {
    let model = window.global::<LibraryModel>();
    model.set_opened(rt.handle.is_some());
    model.set_error(rt.error.as_str().into());
    model.set_count(index_i32(rt.total));
    model.set_loading(rt.loading || rt.loading_more);
    model.set_busy(rt.busy.is_some());
    model.set_busy_text(rt.busy.as_deref().unwrap_or_default().into());
    model.set_empty_kind(empty_kind(rt));
    model.set_scroll_token(rt.scroll_token);
    model.set_list_mode(rt.settings.list_mode);
    model.set_sort(filters::sort_index(rt.settings.sort));

    let choice = &rt.filters;
    let options = &rt.options;
    model.set_shape_index(choice.shape_index(options));
    model.set_gear_index(choice.gear_index(options));
    model.set_ri_index(index_i32(choice.ri_band));
    model.set_tag_index(choice.tag_index(options));
    model.set_mine_only(choice.mine_only);
    model.set_show_ignored(choice.show_ignored);
    model.set_filter_count(index_i32(choice.active_count()));
    model.set_shape_text(
        choice
            .shape
            .as_deref()
            .map_or_else(|| "Shape".to_owned(), format::capitalize)
            .into(),
    );
    model.set_gear_text(
        choice
            .gear
            .map_or_else(|| "Gear".to_owned(), |gear| format!("Gear {gear}"))
            .into(),
    );
    model.set_ri_text(
        filters::ri_labels()
            .get(choice.ri_band)
            .filter(|_| choice.ri_band > 0)
            .map_or_else(
                || "Refractive Index".to_owned(),
                |label| format!("RI {label}"),
            )
            .into(),
    );
    model.set_tag_text(choice.tag.as_deref().unwrap_or("Tag").into());

    model.set_selecting(!rt.selected.is_empty());
    model.set_selection_count(index_i32(rt.selected.len()));
    let chosen = selected_cards(rt);
    model.set_selection_ignored(!chosen.is_empty() && chosen.iter().all(|card| card.ignored));
    model.set_tag_sheet_open(rt.tag_sheet_open);
}
