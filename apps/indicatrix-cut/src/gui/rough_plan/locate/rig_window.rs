//! The rig window: edit a camera rig by hand, and calibrate it from photos of a beam-splitter
//! cube.
//!
//! Like the locate window it is created on the first open and hidden when closed, keeps its
//! state on the UI thread, runs the calibration on a worker thread behind a ticket, and holds
//! no `RefCell` borrow across a file dialog or a job. Rigs are stored in the settings file
//! through the settings persister (the only writer of that file); this window and the locate
//! window both read them from there.

use super::{
    super::host::with_host,
    canvas::{CanvasView, rows_model, slot, slots_model, strings_model},
    jobs,
    photo::{Decoded, Photo, spawn_decode},
    stored_rigs, update_rigs, window,
};
use crate::{
    MainWindow, RigViewRow, RigWindow,
    gui::pickers::{PickerFilter, PickerKind, PickerRequest, pick},
    locate_io::{
        calib_marks::{CalibMarks, CalibMode},
        overlay::{calibration_drawing, to_pixel},
        report::{Row, SCALE_CAVEAT, calibration_rows},
        rig_form::{
            FIELD_LABELS, RigForm, cube_from_texts, default_stone_n, focal_sigma_from_percent,
            layout_views,
        },
        rig_store,
    },
};
use indicatrix_cut_core::rough_plan::locate::{
    Calibrated, CalibrationOptions, CubeSpec, RigProfile, ViewPose, calibrate_rig,
};
use slint::{CloseRequestResponse, ComponentHandle, ModelRc, VecModel, Weak};
use std::{
    cell::RefCell,
    path::PathBuf,
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
};
use tracing::warn;

/// The next job ticket, process-wide (see the locate window).
static NEXT_TICKET: AtomicU64 = AtomicU64::new(1);

/// What the calibration tab shows of a result.
#[derive(Default, Clone)]
struct CalibReport {
    text: String,
    rows: Vec<Row>,
}

/// Everything the window remembers.
struct State {
    rigs: Vec<RigProfile>,
    rig_index: Option<usize>,
    /// The form being edited: the texts of the rig.
    form: RigForm,
    /// The stored rig the form was opened from, for keeping its calibration.
    original: Option<RigProfile>,
    photos: Vec<Option<Photo>>,
    marks: CalibMarks,
    active: usize,
    calibrated: Option<Calibrated>,
    report: CalibReport,
    job: Option<u64>,
}

impl State {
    fn new(form: RigForm) -> Self {
        let views = form.views.len();
        Self {
            rigs: Vec::new(),
            rig_index: None,
            form,
            original: None,
            photos: (0..views).map(|_| None).collect(),
            marks: CalibMarks::new(views),
            active: 0,
            calibrated: None,
            report: CalibReport::default(),
            job: None,
        }
    }

    /// Makes the photos and marks match the form's view count.
    fn sync_views(&mut self) {
        let count = self.form.views.len();
        self.photos.resize_with(count, || None);
        self.marks.resize(count);
        self.active = self.active.min(count.saturating_sub(1));
    }

    fn view_name(&self, view: usize) -> String {
        self.form
            .views
            .get(view)
            .and_then(|form| form.fields.first())
            .filter(|name| !name.trim().is_empty())
            .map_or_else(
                || format!("View {}", view + 1),
                |name| name.trim().to_owned(),
            )
    }

    /// Forgets a calibration result (the marks, the cube or the rig changed).
    fn forget_calibration(&mut self) {
        self.calibrated = None;
        self.report = CalibReport::default();
    }

    /// Opens `form` (a stored rig, or a new one) and forgets what belonged to the old one.
    fn open_form(&mut self, form: RigForm, original: Option<RigProfile>) {
        self.form = form;
        self.original = original;
        self.sync_views();
        self.forget_calibration();
    }
}

/// The window and its state.
struct RigHost {
    window: RigWindow,
    main: Weak<MainWindow>,
    state: RefCell<State>,
}

thread_local! {
    /// The one rig window of the app session, once it was opened.
    static RIGS: RefCell<Option<Rc<RigHost>>> = const { RefCell::new(None) };
}

/// Runs `f` with the rig host, or returns `None` when the window does not exist.
fn with_rigs<R>(f: impl FnOnce(&Rc<RigHost>) -> R) -> Option<R> {
    let host = RIGS.with(|cell| cell.borrow().clone())?;
    Some(f(&host))
}

/// [`with_rigs`] for callbacks that return nothing.
fn on_rigs(f: impl FnOnce(&Rc<RigHost>)) {
    let _ = with_rigs(f);
}

/// Hides the window for good (the planner is closing).
pub(super) fn close() {
    if let Some(host) = RIGS.with(|cell| cell.borrow_mut().take()) {
        let _ = host.window.hide();
    }
}

/// The index to show for `index`, `-1` for none.
fn index_to_i32(index: usize) -> i32 {
    i32::try_from(index).unwrap_or(i32::MAX)
}

/// The refractive index a new rig starts with: the planner's material.
fn new_rig_stone_n() -> f64 {
    with_host(|planner| super::planner_stone_n(planner)).unwrap_or_else(|| default_stone_n(""))
}

/// A form for a new rig named so that no stored rig has the name.
fn blank_form(rigs: &[RigProfile]) -> RigForm {
    RigForm::blank(&rig_store::fresh_name(rigs), new_rig_stone_n())
}

/// Opens the window (the locate window's "Edit rigs..."): creates it on the first call,
/// otherwise shows the existing one with what the user was editing.
pub(super) fn open(main: &Weak<MainWindow>) {
    let Some(host) = with_rigs(Rc::clone).or_else(|| create(main)) else {
        return;
    };
    reload_rigs(&host);
    if let Err(error) = host.window.show() {
        warn!("Could not show the rig window: {error}");
        return;
    }
    host.window.window().set_minimized(false);
}

/// Creates the window and wires every callback once.
fn create(main: &Weak<MainWindow>) -> Option<Rc<RigHost>> {
    let window = match RigWindow::new() {
        Ok(window) => window,
        Err(error) => {
            warn!("Could not create the rig window: {error}");
            return None;
        }
    };
    crate::gui::preferences::bind_rig_window_theme(&window);
    window
        .window()
        .on_close_requested(|| CloseRequestResponse::HideWindow);
    window.set_field_labels(strings_model(
        FIELD_LABELS.iter().map(ToString::to_string).collect(),
    ));
    let rigs = stored_rigs();
    let mut state = State::new(blank_form(&rigs));
    state.rigs = rigs;
    let host = Rc::new(RigHost {
        window,
        main: main.clone(),
        state: RefCell::new(state),
    });
    wire(&host);
    RIGS.with(|cell| *cell.borrow_mut() = Some(Rc::clone(&host)));
    Some(host)
}

/// Registers the window's callbacks.
fn wire(host: &Rc<RigHost>) {
    let window = &host.window;
    window.on_rig_selected(|index| on_rigs(|host| select_rig(host, index)));
    window.on_new_rig(|| on_rigs(new_rig));
    window.on_delete_rig(|| on_rigs(delete_rig));
    window.on_save_rig(|| on_rigs(save_rig));
    window.on_field_edited(|row, field, text| {
        on_rigs(|host| field_edited(host, row, field, &text));
    });
    window.on_projection_toggled(|row| on_rigs(|host| projection_toggled(host, row)));
    window.on_fill_layout(|| on_rigs(fill_layout));
    window.on_load_photo(|view| on_rigs(|host| load_photo(host, view)));
    window.on_select_view(|view| on_rigs(|host| select_view(host, view)));
    window.on_canvas_clicked(|fx, fy| on_rigs(|host| canvas_clicked(host, fx, fy)));
    window.on_undo_click(|| on_rigs(|host| edit_marks(host, Edit::Undo)));
    window.on_clear_view(|| on_rigs(|host| edit_marks(host, Edit::Clear)));
    window.on_calibrate(|| on_rigs(start_calibration));
    window.on_save_calibrated(|| on_rigs(save_calibrated));
}

fn set_message(host: &RigHost, text: &str) {
    host.window.set_error_text("".into());
    host.window.set_message(text.into());
}

fn set_error(host: &RigHost, text: &str) {
    host.window.set_error_text(text.into());
}

// --- Showing the state ---------------------------------------------------------------------

/// Pushes the rig list, the form and the calibration tab.
fn push_all(host: &RigHost) {
    push_list(host);
    push_header(host);
    push_rows(host);
    push_calibration(host);
}

fn push_list(host: &RigHost) {
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

/// The name and the two refractive indices, and what is known of the rig's calibration.
fn push_header(host: &RigHost) {
    let (name, stone, surround, info) = {
        let state = host.state.borrow();
        (
            state.form.name.clone(),
            state.form.stone_n.clone(),
            state.form.surround_n.clone(),
            calibration_info(state.original.as_ref()),
        )
    };
    let window = &host.window;
    window.set_name_text(name.into());
    window.set_stone_n_text(stone.into());
    window.set_surround_n_text(surround.into());
    window.set_calibration_info(info.into());
}

/// The line under the form about the stored calibration.
fn calibration_info(original: Option<&RigProfile>) -> String {
    original
        .and_then(|rig| rig.calibration.as_ref())
        .map_or_else(
            || "Not calibrated.".to_owned(),
            |result| {
                let accuracy = result.diagonal_rms_mm.map_or_else(
                    || "no diagonal check".to_owned(),
                    |rms| format!("measured accuracy {rms:.2} mm RMS"),
                );
                format!(
                    "Calibrated: scale {:+.2} % against the datasheet, {accuracy}.",
                    (result.scale_ratio - 1.0) * 100.0
                )
            },
        )
}

/// The view table. Replacing the model makes the fields new, so this is only called when the
/// rows changed under the user (a rig was opened, the layout was filled, a lens was switched),
/// never while they type.
fn push_rows(host: &RigHost) {
    let rows: Vec<RigViewRow> = host
        .state
        .borrow()
        .form
        .views
        .iter()
        .map(|view| RigViewRow {
            fields: strings_model(view.fields.clone()),
            orthographic: view.orthographic,
        })
        .collect();
    host.window
        .set_view_rows(ModelRc::new(VecModel::from(rows)));
}

fn push_calibration(host: &RigHost) {
    let (slots, view, title, report, can_save) = {
        let state = host.state.borrow();
        let slots = (0..state.form.views.len())
            .map(|view| {
                slot(
                    &state.view_name(view),
                    state.photos.get(view).and_then(Option::as_ref),
                    state.marks.summary(view),
                    view == state.active,
                )
            })
            .collect();
        let photo = state.photos.get(state.active).and_then(Option::as_ref);
        let drawing = photo.map_or_else(
            || (Vec::new(), Vec::new()),
            |photo| calibration_drawing(state.active, photo.size, &state.marks),
        );
        (
            slots,
            CanvasView::new(photo, drawing),
            state.view_name(state.active),
            state.report.clone(),
            state.calibrated.is_some(),
        )
    };
    let window = &host.window;
    window.set_slots(slots_model(slots));
    window.set_photo(view.image);
    window.set_image_w(view.width);
    window.set_image_h(view.height);
    window.set_has_photo(view.has_photo);
    window.set_markers(view.markers);
    window.set_paths(view.paths);
    window.set_view_title(title.into());
    window.set_calib_text(report.text.into());
    window.set_calib_rows(rows_model(report.rows));
    let caveat = if can_save { SCALE_CAVEAT } else { "" };
    window.set_caveat(caveat.into());
    window.set_can_save_calibrated(can_save);
}

// --- Rigs ------------------------------------------------------------------------------------

/// Reads the stored rigs again. The form being edited stays; the list and the picked name are
/// brought up to date, and when no stored rig is picked yet the first one is opened.
fn reload_rigs(host: &Rc<RigHost>) {
    {
        let mut state = host.state.borrow_mut();
        let name = state.original.as_ref().map(|rig| rig.name.clone());
        state.rigs = stored_rigs();
        state.rig_index = name
            .as_deref()
            .and_then(|name| rig_store::position(&state.rigs, name));
        if state.rig_index.is_none() && state.original.is_none() && !state.rigs.is_empty() {
            let first = state.rigs[0].clone();
            state.rig_index = Some(0);
            state.open_form(RigForm::from_profile(&first), Some(first));
        }
    }
    push_all(host);
}

fn select_rig(host: &Rc<RigHost>, index: i32) {
    {
        let mut state = host.state.borrow_mut();
        let Ok(position) = usize::try_from(index) else {
            return;
        };
        let Some(rig) = state.rigs.get(position).cloned() else {
            return;
        };
        state.rig_index = Some(position);
        state.open_form(RigForm::from_profile(&rig), Some(rig));
    }
    set_message(host, "");
    push_all(host);
}

fn new_rig(host: &Rc<RigHost>) {
    {
        let mut state = host.state.borrow_mut();
        let form = blank_form(&state.rigs);
        state.rig_index = None;
        state.open_form(form, None);
    }
    set_message(
        host,
        "A new rig with the default eight views. Edit them to match your rig, then press Save rig.",
    );
    push_all(host);
}

fn delete_rig(host: &Rc<RigHost>) {
    let name = {
        let state = host.state.borrow();
        state
            .rig_index
            .and_then(|index| state.rigs.get(index))
            .map(|rig| rig.name.clone())
    };
    let Some(name) = name else {
        set_error(host, "Pick a stored rig to delete.");
        return;
    };
    if !update_rigs(|rigs| {
        rig_store::remove(rigs, &name);
    }) {
        set_error(
            host,
            "The settings are not available, so the rig cannot be deleted.",
        );
        return;
    }
    {
        let mut state = host.state.borrow_mut();
        state.rigs = stored_rigs();
        let first = state.rigs.first().cloned();
        let form = first
            .as_ref()
            .map_or_else(|| blank_form(&state.rigs), RigForm::from_profile);
        state.rig_index = first.as_ref().map(|_| 0);
        state.open_form(form, first);
    }
    set_message(host, &format!("Deleted the rig {name}."));
    push_all(host);
    window::rigs_changed();
}

/// Copies what the window's own fields hold (the name and the two indices are bound to it, so
/// the form does not see them change) into the form.
fn read_header(host: &RigHost) {
    let name = host.window.get_name_text().to_string();
    let stone = host.window.get_stone_n_text().to_string();
    let surround = host.window.get_surround_n_text().to_string();
    let mut state = host.state.borrow_mut();
    state.form.name = name;
    state.form.stone_n = stone;
    state.form.surround_n = surround;
}

fn save_rig(host: &Rc<RigHost>) {
    read_header(host);
    let profile = {
        let state = host.state.borrow();
        state.form.to_profile(state.original.as_ref())
    };
    let profile = match profile {
        Ok(profile) => profile,
        Err(message) => {
            set_error(host, &message);
            return;
        }
    };
    store_profile(host, profile, "Saved");
}

/// Stores `profile` in the settings, makes it the picked rig and tells the locate window.
fn store_profile(host: &Rc<RigHost>, profile: RigProfile, verb: &str) {
    let name = profile.name.clone();
    let calibration_lost = profile.calibration.is_none()
        && host
            .state
            .borrow()
            .original
            .as_ref()
            .is_some_and(|original| original.name == name && original.calibration.is_some());
    let stored = profile.clone();
    if !update_rigs(move |rigs| {
        rig_store::upsert(rigs, profile);
    }) {
        set_error(
            host,
            "The settings are not available, so the rig cannot be saved.",
        );
        return;
    }
    {
        let mut state = host.state.borrow_mut();
        state.rigs = stored_rigs();
        state.rig_index = rig_store::position(&state.rigs, &name);
        state.original = Some(stored);
    }
    let note = if calibration_lost {
        " Its calibration is cleared because the poses changed."
    } else {
        ""
    };
    set_message(host, &format!("{verb} the rig {name}.{note}"));
    push_list(host);
    push_header(host);
    window::rigs_changed();
}

fn field_edited(host: &Rc<RigHost>, row: i32, field: i32, text: &str) {
    let mut state = host.state.borrow_mut();
    let slot = usize::try_from(row)
        .ok()
        .and_then(|row| state.form.views.get_mut(row))
        .zip(usize::try_from(field).ok())
        .and_then(|(view, field)| view.fields.get_mut(field));
    if let Some(slot) = slot {
        text.clone_into(slot);
    }
}

fn projection_toggled(host: &Rc<RigHost>, row: i32) {
    {
        let mut state = host.state.borrow_mut();
        let Some(view) = usize::try_from(row)
            .ok()
            .and_then(|row| state.form.views.get_mut(row))
        else {
            return;
        };
        view.orthographic = !view.orthographic;
    }
    push_rows(host);
}

fn fill_layout(host: &Rc<RigHost>) {
    let window = &host.window;
    let views = layout_views(
        window.get_gen_distance().as_str(),
        window.get_gen_elevation().as_str(),
        window.get_gen_scale().as_str(),
        window.get_gen_width().as_str(),
        window.get_gen_height().as_str(),
        window.get_gen_ortho(),
    );
    match views {
        Ok(views) => {
            {
                let mut state = host.state.borrow_mut();
                state.form.views = views;
                state.sync_views();
                state.forget_calibration();
            }
            set_message(
                host,
                "The eight views are filled. Check each camera's position, and press Save rig.",
            );
            push_rows(host);
            push_calibration(host);
        }
        Err(message) => set_error(host, &message),
    }
}

// --- Photos and marks ----------------------------------------------------------------------

fn load_photo(host: &Rc<RigHost>, view: i32) {
    let Ok(view) = usize::try_from(view) else {
        return;
    };
    let Some(main) = host.main.upgrade() else {
        return;
    };
    set_error(host, "");
    let request = PickerRequest {
        kind: PickerKind::OpenFile,
        title: Some("Load the cube photo of this view".to_string()),
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
        on_rigs(move |host| decode_photo(host, view, path));
    });
}

fn decode_photo(host: &Rc<RigHost>, view: usize, path: PathBuf) {
    set_message(host, "Reading the photo...");
    spawn_decode(host.window.as_weak(), path, move |path, result| {
        on_rigs(move |host| photo_ready(host, view, path, result));
    });
}

fn photo_ready(host: &Rc<RigHost>, view: usize, path: PathBuf, result: Result<Decoded, String>) {
    let decoded = match result {
        Ok(decoded) => decoded,
        Err(message) => {
            set_error(host, &message);
            return;
        }
    };
    let photo = Photo::new(path, decoded);
    let note = format!(
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
        if state.photos[view].is_some() {
            // The marks were placed on the old photo.
            state.marks.clear(view);
            state.forget_calibration();
        }
        state.photos[view] = Some(photo);
        state.active = view;
    }
    set_message(host, &note);
    push_calibration(host);
}

fn select_view(host: &Rc<RigHost>, view: i32) {
    {
        let mut state = host.state.borrow_mut();
        let Ok(view) = usize::try_from(view) else {
            return;
        };
        if view >= state.form.views.len() {
            return;
        }
        state.active = view;
    }
    push_calibration(host);
}

fn canvas_clicked(host: &Rc<RigHost>, fx: f32, fy: f32) {
    let mode = CalibMode::from_index(host.window.get_mode_index());
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
        state.forget_calibration();
    }
    push_calibration(host);
}

/// What the Undo and Clear buttons do in the shown view.
#[derive(Clone, Copy)]
enum Edit {
    Undo,
    Clear,
}

fn edit_marks(host: &Rc<RigHost>, edit: Edit) {
    let mode = CalibMode::from_index(host.window.get_mode_index());
    {
        let mut state = host.state.borrow_mut();
        let active = state.active;
        match edit {
            Edit::Undo => state.marks.undo(active, mode),
            Edit::Clear => state.marks.clear(active),
        }
        state.forget_calibration();
    }
    push_calibration(host);
}

// --- Calibration ---------------------------------------------------------------------------

fn end_job(host: &RigHost) {
    host.state.borrow_mut().job = None;
    host.window.set_busy(false);
    host.window.set_busy_text("".into());
}

/// The inputs of a calibration, read on the UI thread.
type CalibrationInputs = (RigProfile, CubeSpec, CalibrationOptions, CalibMarks);

fn prepare_calibration(host: &RigHost) -> Result<CalibrationInputs, String> {
    read_header(host);
    let window = &host.window;
    let cube = cube_from_texts(
        window.get_cube_edge().as_str(),
        window.get_cube_tolerance().as_str(),
        window.get_cube_n().as_str(),
        window.get_diag_axis_index(),
        window.get_diag_mirrored(),
    )?;
    let options = CalibrationOptions {
        focal_sigma: focal_sigma_from_percent(window.get_focal_sigma().as_str())?,
        ..CalibrationOptions::default()
    };
    let state = host.state.borrow();
    let rig = state.form.to_profile(state.original.as_ref())?;
    let marks = state.marks.clone();
    if marks.observations().is_empty() {
        return Err(
            "Mark the cube's outer edges in the photos first (the Edge tool, two clicks per edge)."
                .to_owned(),
        );
    }
    Ok((rig, cube, options, marks))
}

fn start_calibration(host: &Rc<RigHost>) {
    set_message(host, "");
    let prepared = prepare_calibration(host);
    let (rig, cube, options, marks) = match prepared {
        Ok(prepared) => prepared,
        Err(message) => {
            set_error(host, &message);
            return;
        }
    };
    let views = rig.views.clone();
    let ticket = NEXT_TICKET.fetch_add(1, Ordering::Relaxed);
    host.state.borrow_mut().job = Some(ticket);
    host.window.set_busy(true);
    host.window.set_busy_text("Calibrating the rig...".into());
    let spawned = jobs::spawn(
        host.window.as_weak(),
        move || {
            calibrate_rig(
                &rig,
                &cube,
                &marks.observations(),
                &marks.diagonal_lines(),
                &options,
            )
            .map_err(|error| error.to_string())
        },
        move |result| on_rigs(|host| calibration_done(host, ticket, &views, result)),
    );
    if let Err(error) = spawned {
        warn!("Rig: could not start the calibration thread: {error}");
        end_job(host);
        set_error(host, &format!("Could not start a background task: {error}"));
    }
}

fn calibration_done(
    host: &Rc<RigHost>,
    ticket: u64,
    views: &[ViewPose],
    result: Result<Calibrated, String>,
) {
    if host.state.borrow().job != Some(ticket) {
        return;
    }
    end_job(host);
    match result {
        Ok(calibrated) => {
            let text = if calibrated.result.scale_within_tolerance {
                "Calibrated. Save the calibrated rig to keep the refined poses."
            } else {
                "Calibrated, but the fitted cube size is outside the datasheet's tolerance: see the scale line."
            };
            {
                let mut state = host.state.borrow_mut();
                state.report = CalibReport {
                    text: text.to_owned(),
                    rows: calibration_rows(&calibrated, views),
                };
                state.calibrated = Some(calibrated);
            }
            push_calibration(host);
        }
        Err(message) => {
            host.state.borrow_mut().forget_calibration();
            set_error(host, &format!("Calibration failed: {message}"));
            push_calibration(host);
        }
    }
}

fn save_calibrated(host: &Rc<RigHost>) {
    let calibrated = host.state.borrow().calibrated.clone();
    let Some(calibrated) = calibrated else {
        set_error(host, "Calibrate first.");
        return;
    };
    {
        // The refined poses become the form, so the table shows what was stored.
        let mut state = host.state.borrow_mut();
        state.form = RigForm::from_profile(&calibrated.rig);
        state.sync_views();
    }
    store_profile(host, calibrated.rig, "Saved the calibrated rig");
    push_rows(host);
}
