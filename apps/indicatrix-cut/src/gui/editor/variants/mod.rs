//! The History tab's "Variants" view: named copies of the open design that live in the local
//! library, keyed by the design's UUID (never in the design file).
//!
//! A cutter tries something, likes it, saves it as a variant, tries something else, and can
//! come back: **Save** from the design as it is or from any history step, **Open** a variant
//! (the open design becomes the variant, as one undo step), **Rename**, change the **note**,
//! **Delete**, and **compare** any two of the current design and the variants as pictures (the
//! compare window) or as text (the cutting instructions, line by line).
//!
//! # Where things live
//!
//! - [`rows`], [`form`], [`diff`], [`branch`], [`apply`], [`capture`] and [`store`] are plain
//!   data and logic with no window in them, and their tests run without one. [`store`] works on
//!   a `&Database`, so its tests use an in-memory library.
//! - [`actions`] holds what each button does (save, rename, note, delete, open), [`comparing`]
//!   the two comparisons, and this module the shared state, the list and the glue to
//!   `ui/models/variants.slint` and `ui/components/editor_inspector/variants_section.slint`.
//!
//! # Threads
//!
//! Nothing here waits on the library or draws a picture on the UI thread. Every action that
//! needs the library (the list, a save, a rename, opening, comparing) runs on a short worker
//! thread ([`spawn_job`]) and comes back to the UI thread through the event loop; an answer
//! that arrives after the design changed or a newer request was made is dropped. While a
//! save, rename, delete, open or comparison is being worked out, `VariantsModel.busy` switches
//! the buttons off, so two never overlap.
//!
//! # Which variants are listed
//!
//! Only the open design's, found by its UUID ([`EditorState::design_uuid`]). The list is read
//! when the view is shown and after every change made here, and a timer looks for a different
//! design while the view is on screen. A variant belongs to the design and not to a file name:
//! a copy made with Save As keeps the id and so shows the same variants.
//!
//! # Branches
//!
//! [`branch::Branch`] remembers the variant the open design was last opened from or saved as.
//! A new variant is saved with that one as its parent, which is how "from: Variant 2" appears
//! on a row.

mod actions;
mod apply;
mod branch;
mod capture;
mod comparing;
mod diff;
mod form;
mod rows;
mod store;
#[cfg(test)]
mod tests;

use super::state::EditorState;
use crate::{
    EditorModel, MainWindow, VariantRowData, VariantsModel,
    bridge::render_thread::RenderContext,
    gui::solid_preview::preview_state::{SolidLastSolved, SolidPreviewState},
};
use branch::Branch;
use capture::Pixels;
use form::Form;
use indicatrix_vault::{
    db::sqlite::Database,
    model::{design_key::normalize_design_uuid, design_variant::VariantSummary},
};
use rows::{Choice, VariantRow};
use slint::{ComponentHandle, Image, ModelRc, SharedString, VecModel};
use std::{
    cell::RefCell,
    collections::HashMap,
    panic::{AssertUnwindSafe, catch_unwind},
    rc::Rc,
    sync::{Arc, Mutex, PoisonError, atomic::Ordering},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tracing::warn;

/// `EditorModel.inspector_tab` of the History tab, which hosts the Variants view.
const HISTORY_TAB: i32 = 4;

/// `VariantsModel.view` of the Variants view.
const VARIANTS_VIEW: i32 = 1;

/// How often the timer looks for a different open design while the view is showing.
const WATCH_INTERVAL: Duration = Duration::from_millis(400);

/// What a worker thread's answer says when the thread itself failed.
const THREAD_FAILED: &str = "Something went wrong inside the program. Nothing was changed.";

/// What the view remembers between callbacks. UI thread only.
#[derive(Default)]
struct Page {
    /// The design (UUID) the rest of this is about; `None` while no design is open.
    uuid: Option<String>,
    /// The design epoch at the last listing, for the branch.
    epoch: u64,
    /// The design's variants as last listed, oldest first.
    list: Vec<VariantSummary>,
    /// The pictures of `list`, by variant id.
    pictures: HashMap<i64, Pixels>,
    /// What the compare boxes' entries stand for, in the boxes' order.
    choices: Vec<Choice>,
    /// Counts listings; an answer for an older one is dropped.
    request: u64,
    /// The variant the open design was last opened from or saved as.
    branch: Branch,
    /// What the form above the list is for.
    form: Form,
    /// A save, rename, delete, open or comparison is being worked out.
    busy: bool,
    /// Counts text comparisons and closings; an answer for an older one is dropped.
    compare_token: u64,
}

/// Everything the callbacks share.
struct Deps {
    state: Rc<RefCell<EditorState>>,
    render_ctx: Arc<Mutex<RenderContext>>,
    preview_state: Arc<SolidPreviewState>,
    solid_last_solved: SolidLastSolved,
    db: Arc<Mutex<Database>>,
    page: RefCell<Page>,
}

impl Deps {
    /// Runs `read` on the editor state; `None` when it is in use (a callback is in the middle
    /// of changing it).
    fn with_state<R>(&self, read: impl FnOnce(&EditorState) -> R) -> Option<R> {
        self.state.try_borrow().ok().map(|state| read(&state))
    }
}

thread_local! {
    /// The shared state, for the continuations that come back from a worker thread (an `Rc`
    /// cannot cross to it). UI-thread-only.
    static LIVE: RefCell<Option<Rc<Deps>>> = const { RefCell::new(None) };

    /// The timer behind [`WATCH_INTERVAL`]. UI-thread-only.
    static TICKER: RefCell<Option<slint::Timer>> = const { RefCell::new(None) };
}

/// The shared state, once [`setup_variants`] has run.
fn live_deps() -> Option<Rc<Deps>> {
    LIVE.with(|cell| cell.borrow().clone())
}

/// Runs `call` with the library, holding its lock only for the call.
fn with_db<T>(db: &Mutex<Database>, call: impl FnOnce(&Database) -> T) -> T {
    call(&db.lock().unwrap_or_else(PoisonError::into_inner))
}

/// The time in Unix seconds.
fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX)
        })
}

/// Which design is open, as far as variants care.
struct Identity {
    /// A real design is open (not the empty start-up placeholder).
    has_design: bool,
    /// Its UUID in the library's form; `None` when there is no real design.
    uuid: Option<String>,
    /// Its design epoch: another one means the design was opened again.
    epoch: u64,
}

impl Identity {
    /// Whether this is still the design with `uuid` as it was opened at `epoch`: the check an
    /// action makes before it uses something chosen earlier (a step of its history, a variant
    /// that was loading). Another design, or the same file opened again, is not.
    fn is_design(&self, uuid: &str, epoch: u64) -> bool {
        self.uuid.as_deref() == Some(uuid) && self.epoch == epoch
    }
}

fn identity_of(state: &EditorState) -> Identity {
    Identity {
        has_design: state.has_design,
        uuid: normalize_design_uuid(state.design_uuid()).filter(|_| state.has_design),
        epoch: state.design_epoch.load(Ordering::Relaxed),
    }
}

/// Runs `work` on a new thread and `done` with its answer on the UI thread. A panic in `work`
/// comes back as an error, so `done` always runs.
///
/// # Errors
///
/// A plain sentence when the thread cannot be started; `done` does not run then.
fn spawn_job<T: Send + 'static>(
    ui: &MainWindow,
    name: &str,
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
    done: impl FnOnce(&MainWindow, Result<T, String>) + Send + 'static,
) -> Result<(), String> {
    let weak = ui.as_weak();
    thread::Builder::new()
        .name(name.to_owned())
        .spawn(move || {
            let answer =
                catch_unwind(AssertUnwindSafe(work)).unwrap_or_else(|_| Err(THREAD_FAILED.into()));
            let _ = weak.upgrade_in_event_loop(move |ui| done(&ui, answer));
        })
        .map(|_| ())
        .map_err(|error| {
            warn!("could not start the variants thread: {error}");
            "The program could not start the work. Try again in a moment.".to_owned()
        })
}

/// The custom materials the render thread holds, for writing the text of a design.
fn custom_materials(deps: &Deps) -> Vec<indicatrix::optics::materials::GemMaterial> {
    RenderContext::lock(&deps.render_ctx)
        .custom_materials
        .as_ref()
        .clone()
}

fn to_data(row: &VariantRow, picture: Option<&Pixels>) -> VariantRowData {
    VariantRowData {
        key: row.id.to_string().into(),
        name: row.name.as_str().into(),
        note: row.note.as_str().into(),
        age: row.age.as_str().into(),
        exact: row.exact.as_str().into(),
        parent: row.parent.as_str().into(),
        opened_from: row.opened_from,
        picture: picture.map_or_else(Image::default, |pixels| Image::from_rgba8(pixels.clone())),
        has_picture: picture.is_some(),
    }
}

fn strings_model(strings: Vec<String>) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(
        strings
            .into_iter()
            .map(SharedString::from)
            .collect::<Vec<_>>(),
    ))
}

// --- the list ----------------------------------------------------------------------------

/// Shows `page`'s list: the rows, the compare boxes' entries and where the boxes stand.
fn publish(ui: &MainWindow, deps: &Deps) {
    let model = ui.global::<VariantsModel>();
    let (first, second) = (model.get_compare_first(), model.get_compare_second());
    let live_epoch = deps.with_state(|state| state.design_epoch.load(Ordering::Relaxed));
    let (rows, labels, selection) = {
        let mut page = deps.page.borrow_mut();
        let epoch = live_epoch.unwrap_or(page.epoch);
        let branch = page
            .uuid
            .as_deref()
            .and_then(|uuid| page.branch.get(uuid, epoch));
        let rows: Vec<VariantRowData> = rows::build_rows(&page.list, branch, unix_now())
            .iter()
            .map(|row| to_data(row, page.pictures.get(&row.id)))
            .collect();
        let (choices, labels) = rows::build_choices(&page.list);
        let selection = rows::reselect(&page.choices, first, second, &choices);
        page.choices = choices;
        (rows, labels, selection)
    };
    model.set_rows(ModelRc::new(VecModel::from(rows)));
    model.set_choices(strings_model(labels));
    model.set_compare_first(selection.0);
    model.set_compare_second(selection.1);
    model.set_list_error(SharedString::new());
    model.set_loaded(true);
}

/// Empties everything shown for the previous design and starts over for `uuid` (`None` when
/// no design is open).
fn forget_design(ui: &MainWindow, deps: &Deps, uuid: Option<String>, epoch: u64) {
    {
        let mut page = deps.page.borrow_mut();
        let branch = std::mem::take(&mut page.branch);
        *page = Page {
            uuid,
            epoch,
            branch,
            request: page.request,
            compare_token: page.compare_token.wrapping_add(1),
            ..Page::default()
        };
    }
    let model = ui.global::<VariantsModel>();
    model.set_rows(ModelRc::default());
    model.set_choices(ModelRc::default());
    model.set_compare_first(0);
    model.set_compare_second(1);
    model.set_loaded(false);
    model.set_busy(false);
    model.set_list_error(SharedString::new());
    model.set_form_mode(0);
    model.set_form_error(SharedString::new());
    model.set_diff_open(false);
    model.set_diff_busy(false);
}

/// Whether the page was built for a different design than the open one, or for the same file
/// before it was opened again (its history, and so any step chosen in the form, is gone).
fn page_is_about_another_design(deps: &Deps, identity: &Identity) -> bool {
    let page = deps.page.borrow();
    page.uuid != identity.uuid || page.epoch != identity.epoch
}

/// Starts a fresh page when the open design is not the one the page is about.
///
/// A form opened before the view was ever on screen needs this: the view's first listing
/// must find the page already about this design, or it would start over and close the form.
fn adopt_open_design(ui: &MainWindow, deps: &Deps) {
    let Some(identity) = deps.with_state(identity_of) else {
        return;
    };
    if page_is_about_another_design(deps, &identity) {
        forget_design(ui, deps, identity.uuid, identity.epoch);
    }
}

/// Reads the open design's variants again. Does nothing while the editor is busy (the timer
/// asks again).
fn refresh(ui: &MainWindow, deps: &Rc<Deps>) {
    let Some(identity) = deps.with_state(identity_of) else {
        return;
    };
    ui.global::<VariantsModel>()
        .set_design_open(identity.has_design);
    if page_is_about_another_design(deps, &identity) {
        forget_design(ui, deps, identity.uuid.clone(), identity.epoch);
    }
    let Some(uuid) = identity.uuid else {
        return;
    };
    let request = {
        let mut page = deps.page.borrow_mut();
        page.epoch = identity.epoch;
        page.request = page.request.wrapping_add(1);
        page.request
    };
    let db = Arc::clone(&deps.db);
    let job_uuid = uuid.clone();
    let started = spawn_job(
        ui,
        "variants-list",
        // The pictures are decoded after the lock is let go: a design opening on the UI
        // thread must not wait for the PNG decoding of a long variants list.
        move || with_db(&db, |db| store::list_with_png(db, &job_uuid)).map(store::decode_pictures),
        move |ui, answer| {
            if let Some(deps) = live_deps() {
                finish_listing(ui, &deps, request, &uuid, answer);
            }
        },
    );
    if let Err(reason) = started {
        ui.global::<VariantsModel>().set_list_error(reason.into());
    }
}

/// [`refresh`]'s continuation.
fn finish_listing(
    ui: &MainWindow,
    deps: &Rc<Deps>,
    request: u64,
    uuid: &str,
    answer: Result<Vec<(VariantSummary, Option<Pixels>)>, String>,
) {
    {
        let mut page = deps.page.borrow_mut();
        if page.request != request || page.uuid.as_deref() != Some(uuid) {
            return;
        }
        if let Ok(entries) = &answer {
            page.pictures = entries
                .iter()
                .filter_map(|(summary, picture)| {
                    picture
                        .as_ref()
                        .map(|pixels| (summary.variant_id, pixels.clone()))
                })
                .collect();
            page.list = entries.iter().map(|(summary, _)| summary.clone()).collect();
        }
    }
    match answer {
        Ok(_) => publish(ui, deps),
        Err(reason) => {
            let model = ui.global::<VariantsModel>();
            model.set_list_error(reason.into());
            model.set_loaded(true);
        }
    }
}

// --- the timer ---------------------------------------------------------------------------

/// Whether the Variants view is on screen: the History tab is selected, not collapsed, and
/// showing its variants.
fn variants_showing(ui: &MainWindow) -> bool {
    let editor = ui.global::<EditorModel>();
    editor.get_inspector_tab() == HISTORY_TAB
        && !editor.get_inspector_collapsed()
        && ui.global::<VariantsModel>().get_view() == VARIANTS_VIEW
}

/// One look at the open design: while the view is showing, lists again if another design is
/// open than the one the list is about.
fn watch(ui: &MainWindow, deps: &Rc<Deps>) {
    if !variants_showing(ui) {
        return;
    }
    let Some(identity) = deps.with_state(identity_of) else {
        return;
    };
    if page_is_about_another_design(deps, &identity) {
        refresh(ui, deps);
    }
}

// --- wiring ------------------------------------------------------------------------------

/// A callback without arguments that runs `action`.
fn handler(
    ui: &MainWindow,
    deps: &Rc<Deps>,
    action: fn(&MainWindow, &Rc<Deps>),
) -> impl Fn() + 'static {
    let (weak, deps) = (ui.as_weak(), Rc::clone(deps));
    move || {
        if let Some(ui) = weak.upgrade() {
            action(&ui, &deps);
        }
    }
}

/// A callback with a variant's key that runs `action`.
fn key_handler(
    ui: &MainWindow,
    deps: &Rc<Deps>,
    action: fn(&MainWindow, &Rc<Deps>, &str),
) -> impl Fn(SharedString) + 'static {
    let (weak, deps) = (ui.as_weak(), Rc::clone(deps));
    move |key| {
        if let Some(ui) = weak.upgrade() {
            action(&ui, &deps, key.as_str());
        }
    }
}

/// Wires the Variants view: its callbacks and the timer that keeps its list about the open
/// design. Called once from `setup_editor_callbacks`.
pub(in crate::gui::editor) fn setup_variants(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
    db: &Arc<Mutex<Database>>,
) {
    let deps = Rc::new(Deps {
        state: Rc::clone(state),
        render_ctx: Arc::clone(render_ctx),
        preview_state: Arc::clone(preview_state),
        solid_last_solved: Arc::clone(solid_last_solved),
        db: Arc::clone(db),
        page: RefCell::new(Page::default()),
    });
    LIVE.with(|cell| *cell.borrow_mut() = Some(Rc::clone(&deps)));

    let model = ui.global::<VariantsModel>();
    model.on_refresh(handler(ui, &deps, refresh));
    model.on_submit_form(handler(ui, &deps, actions::submit_form));
    model.on_cancel_form(handler(ui, &deps, actions::cancel_form));
    model.on_compare_pictures(handler(ui, &deps, comparing::compare_pictures));
    model.on_compare_text(handler(ui, &deps, comparing::compare_text));
    model.on_close_diff(handler(ui, &deps, comparing::close_diff));
    model.on_open_variant(key_handler(ui, &deps, actions::open_variant));
    model.on_begin_rename(key_handler(ui, &deps, actions::begin_rename));
    model.on_begin_note(key_handler(ui, &deps, actions::begin_note));
    model.on_begin_delete(key_handler(ui, &deps, actions::begin_delete));
    let (weak, save_deps) = (ui.as_weak(), Rc::clone(&deps));
    model.on_begin_save(move |position| {
        if let Some(ui) = weak.upgrade() {
            actions::begin_save(&ui, &save_deps, position);
        }
    });

    let timer = slint::Timer::default();
    let weak = ui.as_weak();
    timer.start(slint::TimerMode::Repeated, WATCH_INTERVAL, move || {
        if let Some(ui) = weak.upgrade() {
            watch(&ui, &deps);
        }
    });
    TICKER.with(|cell| *cell.borrow_mut() = Some(timer));
}
