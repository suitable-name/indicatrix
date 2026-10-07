//! The locate window: photos of a mesh rough on a fixed rig, the alignment of the mesh to the
//! rig, the marks, the solve with its checks and Accept.
//!
//! The window is created on the first open and hidden when closed, with its photos, marks
//! and results, so a closed window can be reopened where it was. Its state lives in
//! [`State`], on the UI thread; the heavy steps (reading a photo, aligning, solving) run on
//! worker threads and hand a result back through a ticket, so a result that was overtaken (the
//! rig changed, another job started) is dropped. No `RefCell` borrow is held across a file
//! dialog or a job: handlers read what they need, drop the borrow, then act.

use super::{
    super::{
        host::{Host, with_host},
        inclusions::add_inclusion_notify,
    },
    canvas::{CanvasView, rows_model, slot, slots_model, strings_model},
    jobs,
    photo::{Decoded, Photo, spawn_decode},
    rig_window, stored_rigs,
};
use crate::{
    LocateWindow, MainWindow,
    gui::pickers::{PickerFilter, PickerKind, PickerRequest, pick},
    locate_io::{
        axes::{AXIS_NAMES, DEFAULT_AXES, NUDGE_LABELS, Nudge, start_transform},
        marks::{ClickMode, MarkSet, ViewMarks},
        overlay::{locate_drawing, to_pixel},
        pipeline::{self, Found, Solution},
        record::LocatedRecord,
        report::{
            LINES_NOT_ADDED, Row, alignment_rows, alignment_summary, found_rows, found_summary,
        },
        rig_form::{format_number, parse_number},
        rig_store,
    },
    mesh_io::{NOT_A_MESH_ROUGH, parse_margin_mm},
};
use indicatrix_cut_core::rough_plan::{
    RoughBase,
    locate::{AlignResult, OutlineView, RigProfile, Rigid, ViewPose},
    shape::{RoughMesh, hull},
};
use slint::{CloseRequestResponse, ComponentHandle, Weak};
use std::{
    cell::RefCell,
    fmt::Write as _,
    path::PathBuf,
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
};
use tracing::warn;

/// The number of views before any rig is picked: the default eight-view layout.
const DEFAULT_VIEWS: usize = 8;

/// The largest radius of the shell that stands for an inclusion, in mm.
const MAX_RADIUS_MM: f64 = 25.0;

/// Shown when the rough has no mesh to trace photos through.
const NO_MESH_MESSAGE: &str =
    "The rough has no mesh to trace the photos through. Import a closed mesh of the rough.";

/// The next job ticket. Process-wide, so a result that outlives its window cannot match a
/// ticket of a new one.
static NEXT_TICKET: AtomicU64 = AtomicU64::new(1);

/// The alignment of the mesh to the rig that the marks are solved with.
#[derive(Debug, Clone, Copy)]
struct Alignment {
    /// The mesh-to-rig transform.
    transform: Rigid,
    /// The rough it was fitted to; a different rough (or a scaled copy) needs a new one.
    mesh_id: u64,
}

/// What a report shows: a headline and its lines.
#[derive(Default, Clone)]
struct Report {
    text: String,
    rows: Vec<Row>,
}

/// Everything the window remembers.
struct State {
    rigs: Vec<RigProfile>,
    rig_index: Option<usize>,
    photos: Vec<Option<Photo>>,
    marks: MarkSet,
    active: usize,
    nudges: Vec<String>,
    alignment: Option<Alignment>,
    align_report: Report,
    solution: Option<Solution>,
    solve_report: Report,
    records: Vec<LocatedRecord>,
    job: Option<u64>,
}

impl State {
    fn new() -> Self {
        Self {
            rigs: Vec::new(),
            rig_index: None,
            photos: (0..DEFAULT_VIEWS).map(|_| None).collect(),
            marks: MarkSet::new(DEFAULT_VIEWS),
            active: 0,
            nudges: vec![String::new(); NUDGE_LABELS.len()],
            alignment: None,
            align_report: Report::default(),
            solution: None,
            solve_report: Report::default(),
            records: Vec::new(),
            job: None,
        }
    }

    fn rig(&self) -> Option<&RigProfile> {
        self.rig_index.and_then(|index| self.rigs.get(index))
    }

    fn view_count(&self) -> usize {
        self.rig().map_or(DEFAULT_VIEWS, |rig| rig.views.len())
    }

    fn view_name(&self, view: usize) -> String {
        self.rig()
            .and_then(|rig| rig.views.get(view))
            .map_or_else(|| format!("View {}", view + 1), |pose| pose.name.clone())
    }

    /// Makes the photos and marks match the rig's view count.
    fn sync_views(&mut self) {
        let count = self.view_count();
        self.photos.resize_with(count, || None);
        self.marks.resize(count);
        self.active = self.active.min(count.saturating_sub(1));
    }

    /// Forgets the alignment and the solution (the rig or the rough changed).
    fn forget_results(&mut self) {
        self.alignment = None;
        self.align_report = Report::default();
        self.forget_solution();
    }

    /// Forgets the solution (a mark changed, so its overlay no longer fits).
    fn forget_solution(&mut self) {
        self.solution = None;
        self.solve_report = Report::default();
    }
}

/// The window and its state.
struct LocateHost {
    window: LocateWindow,
    main: Weak<MainWindow>,
    state: RefCell<State>,
}

thread_local! {
    /// The one locate window of the app session, once it was opened.
    static LOCATE: RefCell<Option<Rc<LocateHost>>> = const { RefCell::new(None) };
}

/// Runs `f` with the locate host, or returns `None` when the window does not exist. The host
/// is cloned out first, so `f` may call this again.
fn with_locate<R>(f: impl FnOnce(&Rc<LocateHost>) -> R) -> Option<R> {
    let host = LOCATE.with(|cell| cell.borrow().clone())?;
    Some(f(&host))
}

/// [`with_locate`] for callbacks that return nothing.
fn on_locate(f: impl FnOnce(&Rc<LocateHost>)) {
    let _ = with_locate(f);
}

/// The rig editor saved or deleted a rig: the picker follows.
pub(super) fn rigs_changed() {
    on_locate(reload_rigs);
}

/// Hides the window for good (the planner is closing).
pub(super) fn close() {
    if let Some(host) = LOCATE.with(|cell| cell.borrow_mut().take()) {
        let _ = host.window.hide();
    }
}

/// Opens the window: creates it on the first call, otherwise shows the existing one with
/// everything it held.
pub(super) fn open(planner: &Rc<Host>) {
    let Some(host) = with_locate(Rc::clone).or_else(|| create(planner)) else {
        return;
    };
    reload_rigs(&host);
    if let Err(error) = host.window.show() {
        warn!("Could not show the locate window: {error}");
        return;
    }
    host.window.window().set_minimized(false);
}

/// Creates the window and wires every callback once.
fn create(planner: &Rc<Host>) -> Option<Rc<LocateHost>> {
    let window = match LocateWindow::new() {
        Ok(window) => window,
        Err(error) => {
            warn!("Could not create the locate window: {error}");
            return None;
        }
    };
    crate::gui::preferences::bind_locate_window_theme(&window);
    window
        .window()
        .on_close_requested(|| CloseRequestResponse::HideWindow);
    window.set_axis_names(strings_model(
        AXIS_NAMES.iter().map(ToString::to_string).collect(),
    ));
    window.set_axis_z_index(index_to_i32(DEFAULT_AXES.0));
    window.set_axis_x_index(index_to_i32(DEFAULT_AXES.1));
    window.set_nudge_labels(strings_model(
        NUDGE_LABELS.iter().map(ToString::to_string).collect(),
    ));
    let host = Rc::new(LocateHost {
        window,
        main: planner.main.clone(),
        state: RefCell::new(State::new()),
    });
    push_nudges(&host);
    wire(&host);
    LOCATE.with(|cell| *cell.borrow_mut() = Some(Rc::clone(&host)));
    Some(host)
}

/// Registers the window's callbacks.
fn wire(host: &Rc<LocateHost>) {
    let window = &host.window;
    window.on_rig_selected(|index| on_locate(|host| select_rig(host, index)));
    window.on_edit_rigs(|| on_locate(edit_rigs));
    window.on_load_photo(|view| on_locate(|host| load_photo(host, view)));
    window.on_select_view(|view| on_locate(|host| select_view(host, view)));
    window.on_canvas_clicked(|fx, fy| on_locate(|host| canvas_clicked(host, fx, fy)));
    window.on_undo_click(|| on_locate(|host| edit_marks(host, Edit::Undo)));
    window.on_clear_view(|| on_locate(|host| edit_marks(host, Edit::Clear)));
    window.on_nudge_edited(|index, text| on_locate(|host| nudge_edited(host, index, &text)));
    window.on_align(|| on_locate(start_align));
    window.on_solve(|| on_locate(start_solve));
    window.on_accept_inclusion(|| on_locate(accept));
    window.on_reopen_record(|index| on_locate(|host| reopen_record(host, index)));
}

fn index_to_i32(index: usize) -> i32 {
    i32::try_from(index).unwrap_or(i32::MAX)
}

fn set_status(host: &LocateHost, text: &str) {
    host.window.set_error_text("".into());
    host.window.set_status_text(text.into());
}

fn set_error(host: &LocateHost, text: &str) {
    host.window.set_error_text(text.into());
}

// --- Showing the state ---------------------------------------------------------------------

/// Pushes everything but the nudge fields (replacing those while the user types would cost
/// them the keyboard focus).
fn push_all(host: &LocateHost) {
    push_rigs(host);
    push_slots(host);
    push_canvas(host);
    push_reports(host);
    push_records(host);
}

fn push_nudges(host: &LocateHost) {
    let texts = host.state.borrow().nudges.clone();
    host.window.set_nudge_texts(strings_model(texts));
}

fn push_rigs(host: &LocateHost) {
    let (names, index) = {
        let state = host.state.borrow();
        (
            rig_store::names(&state.rigs),
            state.rig_index.map_or(-1, index_to_i32),
        )
    };
    host.window.set_rig_names(strings_model(names));
    host.window.set_rig_index(index);
}

fn push_slots(host: &LocateHost) {
    let slots = {
        let state = host.state.borrow();
        (0..state.view_count())
            .map(|view| {
                slot(
                    &state.view_name(view),
                    state.photos.get(view).and_then(Option::as_ref),
                    state.marks.summary(view),
                    view == state.active,
                )
            })
            .collect()
    };
    host.window.set_slots(slots_model(slots));
}

fn push_canvas(host: &LocateHost) {
    let (view, title) = {
        let state = host.state.borrow();
        let photo = state.photos.get(state.active).and_then(Option::as_ref);
        let drawing = photo.map_or_else(
            || (Vec::new(), Vec::new()),
            |photo| {
                locate_drawing(
                    state.active,
                    photo.size,
                    &state.marks,
                    state.solution.as_ref(),
                )
            },
        );
        (
            CanvasView::new(photo, drawing),
            state.view_name(state.active),
        )
    };
    let window = &host.window;
    window.set_photo(view.image);
    window.set_image_w(view.width);
    window.set_image_h(view.height);
    window.set_has_photo(view.has_photo);
    window.set_markers(view.markers);
    window.set_paths(view.paths);
    window.set_view_title(title.into());
}

fn push_reports(host: &LocateHost) {
    let (align, solve, accept) = {
        let state = host.state.borrow();
        let accept = match state.solution.as_ref().map(|solution| &solution.found) {
            None => (false, "Solve first.".to_owned()),
            Some(Found::Line(_)) => (false, LINES_NOT_ADDED.to_owned()),
            Some(Found::Point(_)) => (
                true,
                "Add the point to the rough as a shell of this radius, with this margin. One undo step in the Rough Planner.".to_owned(),
            ),
        };
        (
            state.align_report.clone(),
            state.solve_report.clone(),
            accept,
        )
    };
    let window = &host.window;
    window.set_align_text(align.text.into());
    window.set_align_rows(rows_model(align.rows));
    window.set_solve_text(solve.text.into());
    window.set_solve_rows(rows_model(solve.rows));
    window.set_can_accept(accept.0);
    window.set_accept_hint(accept.1.into());
}

fn push_records(host: &LocateHost) {
    let rows: Vec<String> = host
        .state
        .borrow()
        .records
        .iter()
        .enumerate()
        .map(|(index, record)| record.row_text(index + 1))
        .collect();
    host.window.set_records(strings_model(rows));
}

// --- Rigs ------------------------------------------------------------------------------------

/// Reads the stored rigs again and keeps the picked one (by name) when it still exists. A rig
/// that was edited invalidates the alignment and the solution, which were made with the old
/// poses.
fn reload_rigs(host: &Rc<LocateHost>) {
    {
        let mut state = host.state.borrow_mut();
        let before = state.rig().cloned();
        let name = before.as_ref().map(|rig| rig.name.clone());
        state.rigs = stored_rigs();
        state.rig_index = name
            .as_deref()
            .and_then(|name| rig_store::position(&state.rigs, name))
            .or_else(|| (!state.rigs.is_empty()).then_some(0));
        if state.rig() != before.as_ref() {
            state.forget_results();
        }
        state.sync_views();
    }
    let none = host.state.borrow().rigs.is_empty();
    if none {
        set_status(host, "No camera rig yet. Press Edit rigs... to create one.");
    }
    push_all(host);
}

fn select_rig(host: &Rc<LocateHost>, index: i32) {
    {
        let mut state = host.state.borrow_mut();
        state.rig_index = usize::try_from(index)
            .ok()
            .filter(|&index| index < state.rigs.len());
        state.forget_results();
        state.sync_views();
    }
    push_all(host);
}

fn edit_rigs(host: &Rc<LocateHost>) {
    rig_window::open(&host.main);
}

// --- Photos and marks ----------------------------------------------------------------------

fn load_photo(host: &Rc<LocateHost>, view: i32) {
    let Ok(view) = usize::try_from(view) else {
        return;
    };
    let Some(main) = host.main.upgrade() else {
        return;
    };
    set_error(host, "");
    let request = PickerRequest {
        kind: PickerKind::OpenFile,
        title: Some("Load the photo of this view".to_string()),
        filters: vec![PickerFilter {
            label: "Photos (PNG, JPEG)".to_string(),
            extensions: ["png", "jpg", "jpeg"].map(str::to_string).to_vec(),
        }],
        default_file_name: None,
        starting_dir: None,
    };
    // No borrow is held here: the dialog runs on its own thread and answers later.
    pick(&main, request, move |_, path| {
        let Some(path) = path else {
            return;
        };
        on_locate(move |host| decode_photo(host, view, path));
    });
}

fn decode_photo(host: &Rc<LocateHost>, view: usize, path: PathBuf) {
    set_status(host, "Reading the photo...");
    spawn_decode(host.window.as_weak(), path, move |path, result| {
        on_locate(move |host| photo_ready(host, view, path, result));
    });
}

fn photo_ready(host: &Rc<LocateHost>, view: usize, path: PathBuf, result: Result<Decoded, String>) {
    let decoded = match result {
        Ok(decoded) => decoded,
        Err(message) => {
            set_error(host, &message);
            return;
        }
    };
    let photo = Photo::new(path, decoded);
    let mut note = format!(
        "Loaded {} ({} x {} px).",
        photo.file_name(),
        photo.size[0],
        photo.size[1]
    );
    {
        let mut state = host.state.borrow_mut();
        if view >= state.photos.len() {
            return;
        }
        let replaced = state.photos[view].is_some();
        if let Some(pose) = state.rig().and_then(|rig| rig.views.get(view))
            && pose.image_size != photo.size
        {
            let _ = write!(
                note,
                " The rig expects {} x {} px: its focal length and principal point are in that size, so fix the rig or use photos of that size.",
                pose.image_size[0], pose.image_size[1]
            );
        }
        state.photos[view] = Some(photo);
        if replaced {
            // The marks were placed on the old photo.
            state.marks.views[view] = ViewMarks::default();
            state.forget_solution();
        }
        state.active = view;
    }
    set_status(host, &note);
    push_all(host);
}

fn select_view(host: &Rc<LocateHost>, view: i32) {
    {
        let mut state = host.state.borrow_mut();
        let Ok(view) = usize::try_from(view) else {
            return;
        };
        if view >= state.view_count() {
            return;
        }
        state.active = view;
    }
    push_slots(host);
    push_canvas(host);
}

fn canvas_clicked(host: &Rc<LocateHost>, fx: f32, fy: f32) {
    let mode = ClickMode::from_index(host.window.get_mode_index());
    {
        let mut state = host.state.borrow_mut();
        let active = state.active;
        let Some(size) = state
            .photos
            .get(active)
            .and_then(Option::as_ref)
            .map(|photo| photo.size)
        else {
            return;
        };
        state.marks.click(active, mode, to_pixel([fx, fy], size));
        if matches!(mode, ClickMode::Mark(_)) {
            state.forget_solution();
        }
    }
    push_all(host);
}

/// What the Undo and Clear buttons do to the chosen tool's marks in the shown view.
#[derive(Clone, Copy)]
enum Edit {
    Undo,
    Clear,
}

fn edit_marks(host: &Rc<LocateHost>, edit: Edit) {
    let mode = ClickMode::from_index(host.window.get_mode_index());
    {
        let mut state = host.state.borrow_mut();
        let active = state.active;
        match edit {
            Edit::Undo => state.marks.undo(active, mode),
            Edit::Clear => state.marks.clear(active, mode),
        }
        if matches!(mode, ClickMode::Mark(_)) {
            state.forget_solution();
        }
    }
    push_all(host);
}

fn nudge_edited(host: &Rc<LocateHost>, index: i32, text: &str) {
    let mut state = host.state.borrow_mut();
    if let Some(field) = usize::try_from(index)
        .ok()
        .and_then(|index| state.nudges.get_mut(index))
    {
        text.clone_into(field);
    }
}

// --- Jobs ----------------------------------------------------------------------------------

/// Starts `work` on a worker thread and runs `done` with its result on the UI thread, unless
/// another job started or the window was reset meanwhile. The window shows `busy_text` until
/// then and disables its buttons. A panic in `work` reaches `done` as an error.
fn spawn_job<T: Send + 'static>(
    host: &Rc<LocateHost>,
    busy_text: &str,
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
    done: impl FnOnce(&Rc<LocateHost>, Result<T, String>) + Send + 'static,
) {
    let ticket = NEXT_TICKET.fetch_add(1, Ordering::Relaxed);
    host.state.borrow_mut().job = Some(ticket);
    host.window.set_busy(true);
    host.window.set_busy_text(busy_text.into());
    let spawned = jobs::spawn(host.window.as_weak(), work, move |value| {
        on_locate(|host| deliver(host, ticket, value, done));
    });
    if let Err(error) = spawned {
        warn!("Locate: could not start the worker thread: {error}");
        end_job(host);
        set_error(host, &format!("Could not start a background task: {error}"));
    }
}

fn end_job(host: &LocateHost) {
    host.state.borrow_mut().job = None;
    host.window.set_busy(false);
    host.window.set_busy_text("".into());
}

fn deliver<T>(
    host: &Rc<LocateHost>,
    ticket: u64,
    value: Result<T, String>,
    done: impl FnOnce(&Rc<LocateHost>, Result<T, String>),
) {
    if host.state.borrow().job != Some(ticket) {
        return;
    }
    end_job(host);
    done(host, value);
}

/// The id of the rough the planner holds, if it is a mesh rough.
fn planner_mesh_id() -> Result<u64, String> {
    with_host(|planner| match planner.session.borrow().model.base {
        RoughBase::Hull { id, .. } => Ok(id),
        _ => Err(NOT_A_MESH_ROUGH.to_owned()),
    })
    .unwrap_or_else(|| Err("The Rough Planner window is closed.".to_owned()))
}

/// The rig and its views, or a message when none is picked.
fn picked_rig(host: &LocateHost) -> Result<RigProfile, String> {
    host.state
        .borrow()
        .rig()
        .cloned()
        .ok_or_else(|| "Pick or create a camera rig first (Edit rigs...).".to_owned())
}

// --- Align -----------------------------------------------------------------------------------

fn start_align(host: &Rc<LocateHost>) {
    set_status(host, "");
    let started = prepare_align(host);
    let (rig, outlines, id, axes, nudge) = match started {
        Ok(started) => started,
        Err(message) => {
            set_error(host, &message);
            return;
        }
    };
    let views = rig.views.clone();
    spawn_job(
        host,
        "Aligning the mesh to the rig...",
        move || {
            let mesh = hull::mesh(id).ok_or_else(|| NO_MESH_MESSAGE.to_owned())?;
            let (low, high) = mesh.bounds();
            let start = start_transform(axes.0, axes.1, (low + high) * 0.5, &nudge)?;
            pipeline::align(&mesh, &rig, &outlines, start)
        },
        move |host, result| align_done(host, id, &views, result),
    );
}

/// The rig, the outlines, the rough's id, the two axis picks and the nudge.
type AlignInputs = (RigProfile, Vec<OutlineView>, u64, (usize, usize), Nudge);

/// Everything the alignment job needs, read on the UI thread.
fn prepare_align(host: &LocateHost) -> Result<AlignInputs, String> {
    let rig = picked_rig(host)?;
    let (outlines, nudges) = {
        let state = host.state.borrow();
        (state.marks.outlines(), state.nudges.clone())
    };
    if outlines.len() < 2 {
        return Err(
            "Outline the stone in at least two photos first: choose Outline above the photo and click around the stone."
                .to_owned(),
        );
    }
    let nudge = Nudge::from_texts(&nudges)?;
    let id = planner_mesh_id()?;
    let axis = |index: i32| usize::try_from(index).unwrap_or(usize::MAX);
    let axes = (
        axis(host.window.get_axis_z_index()),
        axis(host.window.get_axis_x_index()),
    );
    Ok((rig, outlines, id, axes, nudge))
}

fn align_done(
    host: &Rc<LocateHost>,
    id: u64,
    views: &[ViewPose],
    result: Result<AlignResult, String>,
) {
    match result {
        Ok(result) => {
            {
                let mut state = host.state.borrow_mut();
                state.align_report = Report {
                    text: alignment_summary(&result),
                    rows: alignment_rows(&result, views),
                };
                state.alignment = result.accepted.then_some(Alignment {
                    transform: result.transform,
                    mesh_id: id,
                });
                state.forget_solution();
            }
            if result.accepted {
                set_status(
                    host,
                    "The mesh is aligned to the rig. Now mark the inclusion.",
                );
            } else {
                set_error(
                    host,
                    "The alignment was not accepted: see the misfit per view in step 2.",
                );
            }
            push_all(host);
        }
        Err(message) => set_error(host, &format!("Alignment failed: {message}")),
    }
}

// --- Solve -----------------------------------------------------------------------------------

fn start_solve(host: &Rc<LocateHost>) {
    set_status(host, "");
    let started = prepare_solve(host);
    let (rig, alignment, marks) = match started {
        Ok(started) => started,
        Err(message) => {
            set_error(host, &message);
            return;
        }
    };
    let views = rig.views.clone();
    let id = alignment.mesh_id;
    spawn_job(
        host,
        "Tracing the marks into the stone...",
        move || {
            let mesh = hull::mesh(id).ok_or_else(|| NO_MESH_MESSAGE.to_owned())?;
            pipeline::solve(&mesh, &rig, alignment.transform, &marks)
        },
        move |host, result| solve_done(host, &views, result),
    );
}

/// Everything the solve job needs, read on the UI thread. Refuses when the mesh is not aligned
/// to this rough.
fn prepare_solve(host: &LocateHost) -> Result<(RigProfile, Alignment, MarkSet), String> {
    let rig = picked_rig(host)?;
    let (alignment, marks) = {
        let state = host.state.borrow();
        (state.alignment, state.marks.clone())
    };
    let alignment =
        alignment.ok_or_else(|| "Align the mesh to the rig first (step 2).".to_owned())?;
    if planner_mesh_id()? != alignment.mesh_id {
        return Err(
            "The rough changed since the alignment (a new mesh, or Fit to weight). Align the mesh to the rig again."
                .to_owned(),
        );
    }
    Ok((rig, alignment, marks))
}

fn solve_done(host: &Rc<LocateHost>, views: &[ViewPose], result: Result<Solution, String>) {
    match result {
        Ok(solution) => {
            let margin = solution.found.margin_mm();
            {
                let mut state = host.state.borrow_mut();
                state.solve_report = Report {
                    text: found_summary(&solution.found),
                    rows: found_rows(&solution.found, views),
                };
                state.solution = Some(solution);
            }
            host.window.set_margin_text(format!("{margin:.2}").into());
            set_status(
                host,
                "Solved. Check the residuals, and the green markers against your marks in each photo.",
            );
            push_all(host);
        }
        Err(message) => {
            host.state.borrow_mut().forget_solution();
            set_error(host, &message);
            push_all(host);
        }
    }
}

// --- Accept ----------------------------------------------------------------------------------

fn accept(host: &Rc<LocateHost>) {
    set_status(host, "");
    let prepared = prepare_accept(host);
    let (mesh, margin, record) = match prepared {
        Ok(prepared) => prepared,
        Err(message) => {
            set_error(host, &message);
            return;
        }
    };
    let launched = with_host(|planner| {
        add_inclusion_notify(planner, mesh, margin, move |_planner, result| {
            on_locate(move |host| accepted(host, record, result));
        });
    });
    if launched.is_none() {
        set_error(host, "The Rough Planner window is closed.");
        return;
    }
    set_status(host, "Adding the inclusion to the rough...");
}

/// The shell of the inclusion, its margin and the record of it.
type AcceptInputs = (RoughMesh, f64, LocatedRecord);

/// The shell, the margin and the record of the solved point, from the window's fields.
fn prepare_accept(host: &LocateHost) -> Result<AcceptInputs, String> {
    let (located, marks, alignment, rig_name) = {
        let state = host.state.borrow();
        let Some(solution) = &state.solution else {
            return Err("Solve the marks first.".to_owned());
        };
        let Found::Point(located) = &solution.found else {
            return Err(LINES_NOT_ADDED.to_owned());
        };
        let Some(alignment) = state.alignment else {
            return Err("Align the mesh to the rig first.".to_owned());
        };
        let name = state.rig().map(|rig| rig.name.clone()).unwrap_or_default();
        (located.clone(), state.marks.clone(), alignment, name)
    };
    let radius = parse_number(host.window.get_radius_text().as_str(), "The radius")?;
    if !(radius > 0.0 && radius <= MAX_RADIUS_MM) {
        return Err(format!(
            "The radius must be more than 0 and at most {MAX_RADIUS_MM} mm."
        ));
    }
    let margin = parse_margin_mm(host.window.get_margin_text().as_str())?;
    let mesh = located
        .to_shell(radius)
        .mesh()
        .map_err(|error| error.to_string())?;
    let record = LocatedRecord {
        rig_name,
        marks,
        alignment: alignment.transform,
        position_mm: located.point,
        radius_mm: radius,
        margin_mm: margin,
        rms_mm: located.rms_mm,
    };
    Ok((mesh, margin, record))
}

/// The planner finished adding (or refusing) the inclusion.
fn accepted(host: &Rc<LocateHost>, record: LocatedRecord, result: Result<(), String>) {
    match result {
        Ok(()) => {
            host.state.borrow_mut().records.push(record);
            set_status(
                host,
                "Inclusion added to the rough. Undo in the Rough Planner takes it out again.",
            );
            push_records(host);
        }
        Err(message) => set_error(host, &format!("Not added: {message}")),
    }
}

/// Brings a located inclusion's marks and rig back, to solve them again.
fn reopen_record(host: &Rc<LocateHost>, index: i32) {
    let record = usize::try_from(index)
        .ok()
        .and_then(|index| host.state.borrow().records.get(index).cloned());
    let Some(record) = record else {
        return;
    };
    let id = match planner_mesh_id() {
        Ok(id) => id,
        Err(message) => {
            set_error(host, &message);
            return;
        }
    };
    {
        let mut state = host.state.borrow_mut();
        let Some(position) = rig_store::position(&state.rigs, &record.rig_name) else {
            drop(state);
            set_error(
                host,
                &format!("The rig {} no longer exists.", record.rig_name),
            );
            return;
        };
        state.rig_index = Some(position);
        state.sync_views();
        let count = state.view_count();
        state.marks = record.marks.clone();
        state.marks.resize(count);
        state.alignment = Some(Alignment {
            transform: record.alignment,
            mesh_id: id,
        });
        state.align_report = Report {
            text: "The alignment of the earlier solve is used again.".to_owned(),
            rows: Vec::new(),
        };
        state.forget_solution();
    }
    host.window
        .set_radius_text(format_number(record.radius_mm).into());
    host.window
        .set_margin_text(format_number(record.margin_mm).into());
    set_status(
        host,
        "Marks restored. Press Solve to solve them again. If the rough was scaled or replaced since, align the mesh again first.",
    );
    push_all(host);
}
