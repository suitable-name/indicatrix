//! The wizard's callbacks: what each button, field and click does. Handlers read what they need
//! from the state, drop the borrow, then act; heavy steps run through [`spawn_job`].

use super::{
    brush::BrushMode,
    compare,
    fitwork::{self, FitOutput, SurfaceChoice},
    host::{Host, cancel_job, on_host, set_error, set_status, spawn_job},
    progress::FitClock,
    report::{self, ReportImage, ReportInput},
    steps::{self, Step},
    view_model::{self, push_all},
    work::{self, BacklightChoice, CameraChoice},
    zone_rows::{self, ShapeKind},
};
use crate::{
    gui::{
        pickers::{PickerFilter, PickerKind, PickerRequest, pick},
        rough_colour::store,
    },
    locate_io::rig_form::parse_number,
    settings::SettingsPersister,
};
use indicatrix::{color::led::LedKind, optics::chromophore::ChromophoreCatalogue};
use indicatrix_cut_core::rough_plan::{
    colour_fit::zones::{ZoneEdit, apply_with_locks},
    zoned_plan::RoughColour,
};
use slint::ComponentHandle;
use std::{
    path::PathBuf,
    rc::Rc,
    sync::{Arc, atomic::Ordering},
    time::Instant,
};

/// The extensions the photo pickers offer.
const PHOTO_EXTENSIONS: [&str; 17] = [
    "png", "jpg", "jpeg", "tif", "tiff", "dng", "cr2", "cr3", "nef", "arw", "orf", "rw2", "raf",
    "pef", "srw", "3fr", "iiq",
];

/// The width of the picture in working pixels at which a brush radius is meant.
const MAX_EDGE_BAND_PX: usize = 20;

fn stored_rigs() -> Vec<indicatrix_cut_core::rough_plan::locate::RigProfile> {
    SettingsPersister::installed_for_this_thread()
        .map(|persister| persister.snapshot().rig_profiles)
        .unwrap_or_default()
}

// --- Wiring ----------------------------------------------------------------------------------

/// Registers the window's callbacks.
pub fn wire(host: &Rc<Host>) {
    let w = &host.window;
    w.on_go_step(|i| on_host(|h| go_step(h, i)));
    w.on_next(|| {
        on_host(|h| {
            let (step, facts) = {
                let s = h.state.borrow();
                (s.step, s.facts())
            };
            if let Some(next) = steps::next_step(step, &facts) {
                set_step(h, next);
            }
        });
    });
    w.on_back(|| {
        on_host(|h| {
            let step = h.state.borrow().step;
            if let Some(previous) = step.previous() {
                set_step(h, previous);
            }
        });
    });
    w.on_cancel_job(|| on_host(cancel_job));
    w.on_rig_selected(|i| on_host(|h| select_rig(h, i)));
    w.on_open_locate(|| on_host(|h| (h.link.open_locate)()));
    w.on_load_file(|view, kind| on_host(|h| load_file(h, view, kind)));
    w.on_clear_view_files(|view| on_host(|h| clear_view_files(h, view)));
    w.on_use_locate_photos(|| on_host(use_locate_photos));
    w.on_prepare(|| on_host(prepare));
    w.on_camera_selected(|i| on_host(|h| camera_selected(h, i)));
    w.on_pick_camera_file(|| on_host(pick_camera_file));
    w.on_backlight_selected(|i| on_host(|h| backlight_selected(h, i)));
    w.on_pick_backlight_file(|| on_host(pick_backlight_file));
    w.on_apply_calibration(|| on_host(apply_calibration));
    w.on_select_view(|i| on_host(|h| select_view(h, i)));
    w.on_canvas_clicked(|fx, fy| on_host(|h| canvas_clicked(h, fx, fy)));
    w.on_edge_band_committed(|| on_host(edge_band_committed));
    w.on_clear_brush(|| on_host(clear_brush));
    w.on_surface_selected(|i| on_host(|h| surface_selected(h, i)));
    w.on_clear_windows(|| on_host(clear_windows));
    w.on_add_zone(|k| on_host(|h| add_zone(h, k)));
    w.on_remove_zone(|i| on_host(|h| remove_zone(h, i)));
    w.on_select_zone(|i| on_host(|h| select_zone(h, i)));
    w.on_param_committed(|row, text| on_host(|h| param_committed(h, row, &text)));
    w.on_toggle_lock(|row| on_host(|h| toggle_lock(h, row)));
    w.on_undo_zone_edit(|| on_host(undo_zone_edit));
    w.on_softness_committed(|| on_host(softness_committed));
    w.on_suggest_zones(|| on_host(suggest_zones));
    w.on_accept_suggestion(|i| on_host(|h| accept_suggestion(h, i)));
    w.on_marks_undo(|| on_host(marks_undo));
    w.on_marks_clear(|| on_host(marks_clear));
    w.on_marks_fit(|| on_host(marks_fit));
    w.on_refine(|| on_host(refine));
    w.on_start_fit(|| on_host(start_fit));
    w.on_compare_clicked(|fx, fy| on_host(|h| compare_clicked(h, fx, fy)));
    w.on_compare_zoom_changed(|| on_host(|h| view_model::push_compare(h)));
    w.on_add_zones_from_prompt(|| on_host(|h| set_step(h, Step::Fit)));
    w.on_accept_result(|| on_host(accept_result));
    w.on_export_report(|| on_host(export_report));
}

// --- Opening and steps -----------------------------------------------------------------------

/// Reads the planner's rough, the locate window's alignment and the stored rigs again.
pub fn refresh_from_planner(host: &Rc<Host>) {
    let context = (host.link.context)();
    let alignment = (host.link.alignment)();
    {
        let mut state = host.state.borrow_mut();
        let before = (
            state.context.as_ref().map(|c| c.mesh_id),
            state
                .alignment
                .as_ref()
                .map(|a| (a.mesh_id, format!("{:?}", a.transform))),
            state.rig().map(|r| r.name.clone()),
        );
        state.context = context;
        state.alignment = alignment;
        let name = state.rig().map(|r| r.name.clone());
        state.rigs = stored_rigs();
        let wanted = name.or_else(|| state.alignment.as_ref().map(|a| a.rig_name.clone()));
        state.rig_index = wanted
            .as_deref()
            .and_then(|n| crate::locate_io::rig_store::position(&state.rigs, n))
            .or_else(|| (!state.rigs.is_empty()).then_some(0));
        state.sync_views();
        let after = (
            state.context.as_ref().map(|c| c.mesh_id),
            state
                .alignment
                .as_ref()
                .map(|a| (a.mesh_id, format!("{:?}", a.transform))),
            state.rig().map(|r| r.name.clone()),
        );
        if before != after {
            state.forget_computed();
        }
        let settled = steps::settle(state.step, &state.facts());
        state.step = settled;
    }
    if view_model_host_names_needed() {
        let mut names = vec!["Unknown".to_owned()];
        names.extend(
            ChromophoreCatalogue::global()
                .hosts
                .iter()
                .map(|h| h.name.clone()),
        );
        host.window
            .set_host_names(slint::ModelRc::new(slint::VecModel::from(
                names
                    .into_iter()
                    .map(slint::SharedString::from)
                    .collect::<Vec<_>>(),
            )));
    }
    push_all(host);
}

const fn view_model_host_names_needed() -> bool {
    crate::gui::zoning_ui::host_picker_visible()
}

fn go_step(host: &Rc<Host>, index: i32) {
    let Some(step) = Step::from_index(index) else {
        return;
    };
    set_step(host, step);
}

fn set_step(host: &Rc<Host>, step: Step) {
    {
        let mut state = host.state.borrow_mut();
        if !steps::reachable(step, &state.facts()) {
            return;
        }
        state.step = step;
    }
    set_error(host, "");
    push_all(host);
}

fn select_rig(host: &Rc<Host>, index: i32) {
    {
        let mut state = host.state.borrow_mut();
        state.rig_index = usize::try_from(index)
            .ok()
            .filter(|&i| i < state.rigs.len());
        state.forget_computed();
        state.sync_views();
        let settled = steps::settle(state.step, &state.facts());
        state.step = settled;
    }
    push_all(host);
}

// --- Photos ----------------------------------------------------------------------------------

fn picker(title: &str, extensions: &[&str], label: &str) -> PickerRequest {
    PickerRequest {
        kind: PickerKind::OpenFile,
        title: Some(title.to_owned()),
        filters: vec![PickerFilter {
            label: label.to_owned(),
            extensions: extensions.iter().map(ToString::to_string).collect(),
        }],
        default_file_name: None,
        starting_dir: None,
    }
}

fn load_file(host: &Rc<Host>, view: i32, kind: i32) {
    let Ok(view) = usize::try_from(view) else {
        return;
    };
    let Some(main) = host.link.main.upgrade() else {
        return;
    };
    let title = match kind {
        0 => "Load the stone photo of this view",
        1 => "Load a white frame (empty rig, backlight on)",
        2 => "Load a dark frame (backlight off)",
        _ => "Load a second, shorter exposure of the stone",
    };
    let request = picker(title, &PHOTO_EXTENSIONS, "Photos (RAW, TIFF, PNG, JPEG)");
    pick(&main, request, move |_, path| {
        let Some(path) = path else {
            return;
        };
        on_host(|host| {
            {
                let mut state = host.state.borrow_mut();
                let Some(files) = state.files.get_mut(view) else {
                    return;
                };
                match kind {
                    0 => files.stone = Some(path),
                    1 => files.white.push(path),
                    2 => files.dark.push(path),
                    _ => files.second = Some(path),
                }
                state.active_view = view;
                invalidate_view(&mut state, view);
            }
            push_all(host);
        });
    });
}

fn invalidate_view(state: &mut super::state::State, view: usize) {
    if let Some(slot) = state.prepared.get_mut(view) {
        *slot = None;
    }
    state.calibration = None;
    state.forget_fit();
    state.forget_overlay();
    let settled = steps::settle(state.step, &state.facts());
    state.step = settled;
}

fn clear_view_files(host: &Rc<Host>, view: i32) {
    let Ok(view) = usize::try_from(view) else {
        return;
    };
    {
        let mut state = host.state.borrow_mut();
        if let Some(files) = state.files.get_mut(view) {
            *files = work::ViewFiles::default();
        }
        invalidate_view(&mut state, view);
    }
    push_all(host);
}

fn use_locate_photos(host: &Rc<Host>) {
    let photos = (host.link.alignment)()
        .map(|a| a.photos)
        .unwrap_or_default();
    if photos.iter().all(Option::is_none) {
        set_error(host, "The locate window has no photos loaded.");
        return;
    }
    {
        let mut state = host.state.borrow_mut();
        for (view, photo) in photos.into_iter().enumerate() {
            if let (Some(path), Some(files)) = (photo, state.files.get_mut(view)) {
                files.stone = Some(path);
                invalidate_view(&mut state, view);
            }
        }
    }
    set_status(host, "Took the stone photos of the locate window.");
    push_all(host);
}

fn prepare(host: &Rc<Host>) {
    set_error(host, "");
    let (data, jobs, edge) = {
        let mut state = host.state.borrow_mut();
        let Some(data) = state.scene_data() else {
            drop(state);
            set_error(host, "Align the mesh to the rig first.");
            return;
        };
        let jobs: Vec<(usize, String, work::ViewFiles)> = (0..state.view_count())
            .filter(|&v| state.files[v].is_complete())
            .map(|v| (v, state.view_name(v), state.files[v].clone()))
            .collect();
        (data, jobs, state.edge_band_px)
    };
    if jobs.len() < steps::MIN_VIEWS.min(host.state.borrow().view_count()) {
        set_error(
            host,
            "Load a stone photo and a white frame for at least two views first.",
        );
        return;
    }
    let total = jobs.len();
    spawn_job(
        host,
        "Preparing the photos...",
        move |cancel, sink| {
            let mut done = Vec::with_capacity(total);
            for (n, (view, name, files)) in jobs.into_iter().enumerate() {
                if cancel.load(Ordering::Relaxed) {
                    return Err("Cancelled.".to_owned());
                }
                sink(format!("Preparing {name}..."), n as f32 / total as f32);
                done.push((view, work::prepare_view(view, &name, &files, &data, edge)));
            }
            Ok(done)
        },
        |host, result| match result {
            Ok(views) => {
                let mut failures = Vec::new();
                {
                    let mut state = host.state.borrow_mut();
                    for (view, outcome) in views {
                        match outcome {
                            Ok(prepared) => state.prepared[view] = Some(prepared),
                            Err(message) => failures.push(message),
                        }
                    }
                    state.calibration = None;
                    state.forget_fit();
                }
                if failures.is_empty() {
                    set_status(host, "Photos prepared. Continue with the calibration.");
                } else {
                    set_error(host, &failures.join(" "));
                }
                push_all(host);
            }
            Err(message) => set_error(host, &message),
        },
    );
}

// --- Calibration -----------------------------------------------------------------------------

fn camera_selected(host: &Rc<Host>, index: i32) {
    {
        let mut state = host.state.borrow_mut();
        let previous = match &state.camera {
            CameraChoice::Curves(p) | CameraChoice::Filters(p) => p.clone(),
            CameraChoice::Fallback => PathBuf::new(),
        };
        state.camera = match index {
            1 => CameraChoice::Curves(previous),
            2 => CameraChoice::Filters(previous),
            _ => CameraChoice::Fallback,
        };
        state.calibration = None;
        state.forget_fit();
    }
    view_model::push_calibration(host);
    view_model::push_steps(host);
}

fn pick_camera_file(host: &Rc<Host>) {
    let Some(main) = host.link.main.upgrade() else {
        return;
    };
    let request = picker(
        "Choose the camera file",
        &["csv", "txt", "tsv"],
        "Tables (CSV, text)",
    );
    pick(&main, request, |_, path| {
        let Some(path) = path else {
            return;
        };
        on_host(|host| {
            {
                let mut state = host.state.borrow_mut();
                state.camera = match state.camera {
                    CameraChoice::Filters(_) => CameraChoice::Filters(path),
                    _ => CameraChoice::Curves(path),
                };
                state.calibration = None;
            }
            view_model::push_calibration(host);
            view_model::push_steps(host);
        });
    });
}

fn backlight_selected(host: &Rc<Host>, index: i32) {
    {
        let mut state = host.state.borrow_mut();
        let previous = match &state.backlight {
            BacklightChoice::Csv(p) => p.clone(),
            _ => PathBuf::new(),
        };
        state.backlight = match index {
            1 => BacklightChoice::Led(state.led),
            2 => BacklightChoice::Csv(previous),
            _ => BacklightChoice::Auto,
        };
        state.calibration = None;
        state.forget_fit();
    }
    view_model::push_calibration(host);
    view_model::push_steps(host);
}

fn pick_backlight_file(host: &Rc<Host>) {
    let Some(main) = host.link.main.upgrade() else {
        return;
    };
    let request = picker(
        "Choose the backlight spectrum",
        &["csv", "txt", "tsv"],
        "Tables (CSV, text)",
    );
    pick(&main, request, |_, path| {
        let Some(path) = path else {
            return;
        };
        on_host(|host| {
            {
                let mut state = host.state.borrow_mut();
                state.backlight = BacklightChoice::Csv(path);
                state.calibration = None;
            }
            view_model::push_calibration(host);
            view_model::push_steps(host);
        });
    });
}

fn apply_calibration(host: &Rc<Host>) {
    let led_index = usize::try_from(host.window.get_led_index()).unwrap_or(0);
    let result = {
        let mut state = host.state.borrow_mut();
        if let Some(kind) = LedKind::ALL.get(led_index) {
            state.led = *kind;
            if matches!(state.backlight, BacklightChoice::Led(_)) {
                state.backlight = BacklightChoice::Led(*kind);
            }
        }
        let Some(white) = state.mean_white_rgb() else {
            drop(state);
            set_error(
                host,
                "Prepare the photos first: the white frames set the backlight.",
            );
            return;
        };
        work::build_calibration(&state.camera, &state.backlight, white)
    };
    match result {
        Ok(calibration) => {
            {
                let mut state = host.state.borrow_mut();
                state.calibration = Some(calibration);
                state.forget_fit();
            }
            set_status(host, "Calibration applied.");
        }
        Err(message) => set_error(host, &message),
    }
    view_model::push_calibration(host);
    view_model::push_steps(host);
}

// --- The picture: views, brush, windows, marks -------------------------------------------------

fn select_view(host: &Rc<Host>, index: i32) {
    {
        let mut state = host.state.borrow_mut();
        let Ok(view) = usize::try_from(index) else {
            return;
        };
        if view >= state.view_count() {
            return;
        }
        state.active_view = view;
    }
    view_model::push_views(host);
    view_model::push_canvas(host);
    view_model::push_masks(host);
    view_model::push_compare(host);
}

/// The brush radius typed in the window, in working pixels (4 when it is not a number).
pub fn radius_px(host: &Host) -> f64 {
    parse_number(
        host.window.get_brush_radius_text().as_str(),
        "The brush radius",
    )
    .unwrap_or(4.0)
}

fn canvas_clicked(host: &Rc<Host>, fx: f32, fy: f32) {
    let step = host.state.borrow().step;
    let radius = radius_px(host);
    match step {
        Step::Masks => {
            let mode = match host.window.get_brush_mode() {
                1 => BrushMode::Exclude,
                2 => BrushMode::Restore,
                _ => return,
            };
            {
                let mut state = host.state.borrow_mut();
                let Some((working, _)) = state.click_pixels(fx, fy) else {
                    return;
                };
                let view = state.active_view;
                let Some(Some(prepared)) = state.prepared.get_mut(view) else {
                    return;
                };
                prepared.paint(working, radius, mode);
                state.forget_fit();
            }
            view_model::push_canvas(host);
            view_model::push_masks(host);
            view_model::push_views(host);
        }
        Step::Surfaces => {
            let paint = match host.window.get_paint_mode() {
                1 => true,
                2 => false,
                _ => return,
            };
            {
                let mut state = host.state.borrow_mut();
                let Some((working, _)) = state.click_pixels(fx, fy) else {
                    return;
                };
                let Some(prepared) = state.active_prepared() else {
                    return;
                };
                let (w, h) = (prepared.grid.width, prepared.grid.height);
                let mut touched = Vec::new();
                let r = radius.clamp(0.5, 40.0);
                for y in 0..h {
                    for x in 0..w {
                        if (x as f64 + 0.5 - working[0]).hypot(y as f64 + 0.5 - working[1]) <= r {
                            let t = prepared.triangles[y * w + x];
                            if t != u32::MAX {
                                touched.push(t);
                            }
                        }
                    }
                }
                // The same set the 3D brush in the Rough Planner's view paints into.
                state.paint_triangles(touched, paint);
                state.forget_fit();
            }
            view_model::push_canvas(host);
            view_model::push_surfaces(host);
        }
        Step::Fit => {
            if !host.window.get_mark_mode() {
                return;
            }
            {
                let mut state = host.state.borrow_mut();
                let Some((_, full)) = state.click_pixels(fx, fy) else {
                    return;
                };
                let view = state.active_view;
                state.marks.click(view, full);
            }
            view_model::push_canvas(host);
            view_model::push_zones(host);
        }
        _ => {}
    }
}

fn edge_band_committed(host: &Rc<Host>) {
    let Ok(value) = parse_number(host.window.get_edge_band_text().as_str(), "The edge band") else {
        set_error(host, "The edge band is not a number.");
        return;
    };
    let edge = (value.max(0.0).round() as usize).min(MAX_EDGE_BAND_PX);
    let (data, views) = {
        let mut state = host.state.borrow_mut();
        state.edge_band_px = edge;
        let Some(data) = state.scene_data() else {
            return;
        };
        let views: Vec<_> = state.prepared.iter().flatten().cloned().collect();
        (data, views)
    };
    if views.is_empty() {
        return;
    }
    spawn_job(
        host,
        "Recomputing the masks...",
        move |cancel, sink| {
            let total = views.len();
            let mut out = Vec::with_capacity(total);
            for (n, mut view) in views.into_iter().enumerate() {
                if cancel.load(Ordering::Relaxed) {
                    return Err("Cancelled.".to_owned());
                }
                sink(
                    format!("Masks of {}...", view.name),
                    n as f32 / total as f32,
                );
                view.recompute_auto(&data, edge);
                out.push((view.view, view.auto_mask));
            }
            Ok(out)
        },
        |host, result| {
            match result {
                Ok(masks) => {
                    let mut state = host.state.borrow_mut();
                    for (view, mask) in masks {
                        if let Some(Some(prepared)) = state.prepared.get_mut(view) {
                            prepared.auto_mask = mask;
                        }
                    }
                    state.forget_fit();
                }
                Err(message) => set_error(host, &message),
            }
            push_all(host);
        },
    );
}

fn clear_brush(host: &Rc<Host>) {
    {
        let mut state = host.state.borrow_mut();
        let view = state.active_view;
        if let Some(Some(prepared)) = state.prepared.get_mut(view) {
            super::brush::clear_user(&mut prepared.user_mask);
        }
        state.forget_fit();
    }
    view_model::push_canvas(host);
    view_model::push_masks(host);
    view_model::push_views(host);
}

fn surface_selected(host: &Rc<Host>, index: i32) {
    {
        let mut state = host.state.borrow_mut();
        state.surface = match index {
            1 => SurfaceChoice::Frosted(0.2),
            2 => SurfaceChoice::FrostedAuto,
            _ => SurfaceChoice::Polished,
        };
        state.forget_fit();
    }
    view_model::push_canvas(host);
    view_model::push_steps(host);
}

fn clear_windows(host: &Rc<Host>) {
    {
        let mut state = host.state.borrow_mut();
        state.windows.clear();
        state.forget_fit();
    }
    view_model::push_canvas(host);
    view_model::push_surfaces(host);
}

// --- Zones -----------------------------------------------------------------------------------

fn apply_edit(host: &Rc<Host>, edit: &ZoneEdit) -> bool {
    let result = {
        let state = host.state.borrow();
        apply_with_locks(&state.geometry_or_empty(), &state.locks, edit)
    };
    match result {
        Ok((zoned, locks)) => {
            let mut state = host.state.borrow_mut();
            state.push_zone_undo();
            state.geometry = Some(zoned);
            state.locks = locks;
            state.forget_overlay();
            state.forget_fit();
            drop(state);
            true
        }
        Err(error) => {
            set_error(host, &format!("{error:?}"));
            false
        }
    }
}

/// Takes back the newest zone edit (a typed value, an added or removed zone, or a whole drag of a
/// 3D handle in the Rough Planner's view).
fn undo_zone_edit(host: &Rc<Host>) {
    if !host.state.borrow_mut().undo_zone_edit() {
        set_status(host, "No zone edit to undo.");
        return;
    }
    view_model::push_canvas(host);
    view_model::push_zones(host);
    view_model::push_steps(host);
}

fn mesh_centre_extent(host: &Host) -> (glam::DVec3, f64) {
    host.state
        .borrow()
        .context
        .as_ref()
        .map_or((glam::DVec3::ZERO, 10.0), |c| {
            let (lo, hi) = c.mesh.bounds();
            ((lo + hi) * 0.5, (hi - lo).max_element())
        })
}

fn add_zone_shape(host: &Rc<Host>, shape: indicatrix::optics::zoning::ZoneShape) {
    let count = host
        .state
        .borrow()
        .geometry
        .as_ref()
        .map_or(0, |g| g.zones.len());
    if apply_edit(
        host,
        &ZoneEdit::Add {
            zone: zone_rows::placeholder_zone(shape),
            position: None,
        },
    ) {
        host.state.borrow_mut().selected_zone = count + 1;
    }
    view_model::push_canvas(host);
    view_model::push_zones(host);
    view_model::push_steps(host);
}

fn add_zone(host: &Rc<Host>, index: i32) {
    let Some(kind) = ShapeKind::from_index(index) else {
        return;
    };
    let (centre, extent) = mesh_centre_extent(host);
    add_zone_shape(host, zone_rows::default_shape(kind, centre, extent));
}

fn remove_zone(host: &Rc<Host>, index: i32) {
    let Ok(zone) = usize::try_from(index) else {
        return;
    };
    if zone == 0 {
        return;
    }
    if apply_edit(host, &ZoneEdit::Remove { zone }) {
        host.state.borrow_mut().selected_zone = 0;
    }
    view_model::push_canvas(host);
    view_model::push_zones(host);
    view_model::push_steps(host);
}

fn select_zone(host: &Rc<Host>, index: i32) {
    host.state.borrow_mut().selected_zone = usize::try_from(index).unwrap_or(0);
    view_model::push_zones(host);
}

fn param_committed(host: &Rc<Host>, row: i32, text: &str) {
    let Ok(row) = usize::try_from(row) else {
        return;
    };
    let edit = {
        let state = host.state.borrow();
        zone_rows::edit_from_text(&state.geometry_or_empty(), state.selected_zone, row, text)
    };
    match edit {
        Ok(edit) => {
            apply_edit(host, &edit);
        }
        Err(message) => set_error(host, &message),
    }
    view_model::push_canvas(host);
    view_model::push_zones(host);
    view_model::push_steps(host);
}

fn toggle_lock(host: &Rc<Host>, row: i32) {
    let Ok(row) = usize::try_from(row) else {
        return;
    };
    let edit = {
        let state = host.state.borrow();
        let zone = state.selected_zone;
        zone_rows::parameter_of_row(&state.geometry_or_empty(), zone, row).map(|parameter| {
            if state.locks.is_locked(zone, parameter) {
                ZoneEdit::Unlock { zone, parameter }
            } else {
                ZoneEdit::Lock { zone, parameter }
            }
        })
    };
    if let Some(edit) = edit {
        // A lock is not a change of the geometry: the fit stays.
        let result = {
            let state = host.state.borrow();
            apply_with_locks(&state.geometry_or_empty(), &state.locks, &edit)
        };
        match result {
            Ok((_, locks)) => host.state.borrow_mut().locks = locks,
            Err(error) => set_error(host, &format!("{error:?}")),
        }
    }
    view_model::push_zones(host);
}

fn softness_committed(host: &Rc<Host>) {
    match parse_number(host.window.get_softness_text().as_str(), "The softness") {
        Ok(value) if value >= 0.0 => {
            apply_edit(
                host,
                &ZoneEdit::SetSoftness {
                    millimetres: value as f32,
                },
            );
        }
        Ok(_) => set_error(host, "The softness must not be negative."),
        Err(message) => set_error(host, &message),
    }
    view_model::push_zones(host);
    view_model::push_steps(host);
}

fn current_setup(host: &Rc<Host>) -> Result<fitwork::FitSetup, String> {
    let setback = parse_number(
        host.window.get_panel_setback_text().as_str(),
        "The panel distance",
    )?;
    let size = parse_number(host.window.get_panel_size_text().as_str(), "The panel size")?;
    let planned = parse_number(host.window.get_planned_text().as_str(), "The planned width")?;
    if !(size > 0.0 && planned > 0.0 && setback >= 0.0) {
        return Err("The panel size and the planned width must be positive.".to_owned());
    }
    let host_id = if crate::gui::zoning_ui::host_picker_visible() {
        usize::try_from(host.window.get_host_index())
            .ok()
            .and_then(|i| i.checked_sub(1))
            .and_then(|i| ChromophoreCatalogue::global().hosts.get(i))
            .map(|h| h.id.clone())
    } else {
        None
    };
    let cache = std::env::temp_dir().join("indicatrix-rough-colour-cache");
    host.state
        .borrow_mut()
        .fit_setup(setback, size, planned, Some(cache), host_id)
}

fn suggest_zones(host: &Rc<Host>) {
    let setup = match current_setup(host) {
        Ok(setup) => setup,
        Err(message) => {
            set_error(host, &message);
            return;
        }
    };
    let fit = host.state.borrow().fit.as_ref().map(|f| f.fit.clone());
    spawn_job(
        host,
        "Looking for colour boundaries...",
        move |_, _| fitwork::suggest(&setup, fit.as_ref()),
        |host, result| {
            match result {
                Ok(list) if list.is_empty() => {
                    set_status(host, "No clear colour boundary was found.");
                }
                Ok(list) => {
                    set_status(
                        host,
                        "Suggestions found. They are starting points, check them against the photos.",
                    );
                    host.state.borrow_mut().suggestions = list;
                }
                Err(message) => set_error(host, &message),
            }
            view_model::push_zones(host);
        },
    );
}

fn accept_suggestion(host: &Rc<Host>, index: i32) {
    let shape = usize::try_from(index).ok().and_then(|i| {
        host.state
            .borrow()
            .suggestions
            .get(i)
            .map(|s| s.shape.clone())
    });
    if let Some(shape) = shape {
        add_zone_shape(host, shape);
    }
}

fn marks_undo(host: &Rc<Host>) {
    {
        let mut state = host.state.borrow_mut();
        let view = state.active_view;
        state.marks.undo(view);
    }
    view_model::push_canvas(host);
    view_model::push_zones(host);
}

fn marks_clear(host: &Rc<Host>) {
    {
        let mut state = host.state.borrow_mut();
        state.marks.clear();
        state.pending_first = None;
    }
    view_model::push_canvas(host);
    view_model::push_zones(host);
}

fn marks_fit(host: &Rc<Host>) {
    let Some(kind) = ShapeKind::from_index(host.window.get_mark_kind_index()) else {
        return;
    };
    let lines = host.state.borrow().marks.polylines();
    if lines.len() < 2 {
        set_error(host, "Mark the boundary in at least two views first.");
        return;
    }
    let needs_second = matches!(kind, ShapeKind::Slab | ShapeKind::Sector);
    let (first, second) = if needs_second {
        let pending = host.state.borrow_mut().pending_first.take();
        match pending {
            None => {
                {
                    let mut state = host.state.borrow_mut();
                    state.pending_first = Some(lines);
                    state.marks.clear();
                }
                set_status(
                    host,
                    "First boundary kept. Mark the second boundary and press Fit zone again.",
                );
                view_model::push_canvas(host);
                view_model::push_zones(host);
                return;
            }
            Some(first) => (first, lines),
        }
    } else {
        (lines, Vec::new())
    };
    let setup = match current_setup(host) {
        Ok(setup) => setup,
        Err(message) => {
            set_error(host, &message);
            return;
        }
    };
    spawn_job(
        host,
        "Locating the marked boundary...",
        move |_, _| fitwork::zone_from_marks(&setup, kind, &first, &second),
        |host, result| {
            match result {
                Ok(marked) => {
                    host.state.borrow_mut().marks.clear();
                    add_zone_shape(host, marked.shape);
                    set_status(
                        host,
                        &format!("Zone added from the marks ({}).", marked.quality),
                    );
                }
                Err(message) => set_error(host, &message),
            }
            push_all(host);
        },
    );
}

fn refine(host: &Rc<Host>) {
    let (geometry, locks) = {
        let state = host.state.borrow();
        (state.geometry_or_empty(), state.locks.clone())
    };
    if geometry.zones.is_empty() {
        set_error(host, "Add a zone first: there is no geometry to refine.");
        return;
    }
    let setup = match current_setup(host) {
        Ok(setup) => setup,
        Err(message) => {
            set_error(host, &message);
            return;
        }
    };
    let iterations = zone_rows::iterations_from_text(host.window.get_iterations_text().as_str());
    let options = zone_rows::refine_options(iterations);
    spawn_job(
        host,
        "Refining the zone geometry...",
        move |cancel, sink| {
            fitwork::run_refine(&setup, &geometry, &locks, &options, &cancel, &mut |p| {
                sink(
                    format!(
                        "Refining the zones: round {} of up to {iterations}",
                        p.iteration + 1
                    ),
                    (p.iteration as f32 / iterations as f32).min(1.0),
                );
            })
        },
        |host, result| {
            match result {
                Ok(refined) => {
                    {
                        let mut state = host.state.borrow_mut();
                        state.geometry = Some(refined.zoned);
                        state.forget_overlay();
                        state.forget_fit();
                    }
                    set_status(
                        host,
                        &format!(
                            "Geometry refined in {} round(s): misfit {:.0} to {:.0}. Run the fit again.",
                            refined.iterations, refined.chi2_before, refined.chi2_after
                        ),
                    );
                }
                Err(message) => set_error(host, &message),
            }
            push_all(host);
        },
    );
}

// --- Fit -------------------------------------------------------------------------------------

fn start_fit(host: &Rc<Host>) {
    set_error(host, "");
    {
        let mut state = host.state.borrow_mut();
        if matches!(state.surface, SurfaceChoice::Frosted(_)) {
            match parse_number(host.window.get_roughness_text().as_str(), "The roughness") {
                Ok(a) if (0.001..=1.0).contains(&a) => {
                    state.surface = SurfaceChoice::Frosted(a as f32);
                }
                Ok(_) => {
                    drop(state);
                    set_error(host, "The roughness must be between 0.001 and 1.");
                    return;
                }
                Err(message) => {
                    drop(state);
                    set_error(host, &message);
                    return;
                }
            }
        }
    }
    let setup = match current_setup(host) {
        Ok(setup) => setup,
        Err(message) => {
            set_error(host, &message);
            return;
        }
    };
    let geometry = host.state.borrow().geometry_or_empty();
    let search = fitwork::searches_roughness(&setup);
    spawn_job(
        host,
        "Fitting the colours...",
        move |cancel, sink| {
            let mut clock = FitClock::new(search);
            fitwork::run_fit(&setup, &geometry, &cancel, &mut |p| {
                let (line, overall) = clock.report(Instant::now(), p.stage, p.fraction);
                sink(line, overall);
            })
        },
        |host, result| match result {
            Ok(output) => fit_done(host, output),
            Err(message) => {
                set_error(host, &message);
                push_all(host);
            }
        },
    );
}

fn fit_done(host: &Rc<Host>, output: FitOutput) {
    let prompt = compare::zoned_prompt(&output.fit).is_some();
    {
        let mut state = host.state.borrow_mut();
        state.geometry = Some(output.zoned.clone());
        state.fit = Some(output);
        state.step = Step::Compare;
        state.compare_centre = [0.5, 0.5];
        state.forget_overlay();
    }
    set_status(
        host,
        if prompt {
            "Fit done. The residual looks structured: the stone may be zoned."
        } else {
            "Fit done. Compare the render with the photos."
        },
    );
    push_all(host);
}

// --- Compare ---------------------------------------------------------------------------------

fn compare_clicked(host: &Rc<Host>, fx: f32, fy: f32) {
    let zoom = f64::from(host.window.get_compare_zoom().max(1));
    {
        let mut state = host.state.borrow_mut();
        let old = state.compare_centre;
        let origin = |c: f64| (c - 0.5 / zoom).clamp(0.0, 1.0 - 1.0 / zoom);
        state.compare_centre = [
            origin(old[0]) + f64::from(fx.clamp(0.0, 1.0)) / zoom,
            origin(old[1]) + f64::from(fy.clamp(0.0, 1.0)) / zoom,
        ];
    }
    view_model::push_compare(host);
}

// --- Accept and export -----------------------------------------------------------------------

fn now_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0))
}

fn accept_result(host: &Rc<Host>) {
    let (plan_id, output, photos) = {
        let state = host.state.borrow();
        let (Some(plan_id), Some(output)) = (
            state.context.as_ref().and_then(|c| c.plan_id),
            state.fit.clone(),
        ) else {
            drop(state);
            set_error(
                host,
                "Save the plan in the Rough Planner first, then accept.",
            );
            return;
        };
        let photos: Vec<_> = state
            .prepared
            .iter()
            .flatten()
            .map(|p| {
                let mut image = p.observed.clone();
                image.mask = p.combined_mask();
                (p.view, image)
            })
            .collect();
        (plan_id, output, photos)
    };
    let db = Arc::clone(&host.link.db);
    spawn_job(
        host,
        "Storing the rough colour...",
        move |_, _| {
            let colour = RoughColour::new(output.zoned.clone(), &output.fit, now_seconds())
                .map_err(|e| format!("Could not store the colour: {e}"))?;
            let guard = db
                .lock()
                .map_err(|_| "The library is busy; try again.".to_owned())?;
            store::save_rough_colour(&guard, plan_id, &colour).map_err(|e| format!("{e:#}"))?;
            for (view, image) in &photos {
                store::save_view_photos(&guard, plan_id, u32::try_from(*view).unwrap_or(0), image)
                    .map_err(|e| format!("{e:#}"))?;
            }
            Ok(())
        },
        |host, result| match result {
            Ok(()) => {
                set_status(host, "Rough colour stored with the saved plan.");
                let _ = host.window.hide();
                // The planner's previews and "Use colour" pick the new colour up.
                crate::gui::rough_plan::colour_stored();
            }
            Err(message) => set_error(host, &message),
        },
    );
}

fn export_report(host: &Rc<Host>) {
    let Some(main) = host.link.main.upgrade() else {
        return;
    };
    if host.state.borrow().fit.is_none() {
        set_error(host, "Run the fit first.");
        return;
    }
    let request = PickerRequest {
        kind: PickerKind::PickFolder,
        title: Some("Choose a folder for the report".to_owned()),
        filters: Vec::new(),
        default_file_name: None,
        starting_dir: None,
    };
    pick(&main, request, |_, folder| {
        let Some(folder) = folder else {
            return;
        };
        on_host(|host| write_report(host, folder));
    });
}

fn write_report(host: &Rc<Host>, folder: PathBuf) {
    let prepared = {
        let state = host.state.borrow();
        let (Some(output), Some(context)) = (state.fit.as_ref(), state.context.as_ref()) else {
            return;
        };
        let names: Vec<String> = (0..state.view_count())
            .map(|v| state.view_name(v))
            .collect();
        let mut images = Vec::new();
        for (view, name) in names.iter().enumerate() {
            if let Some((photo, render, heat, _)) = state.compare_images_for(view, 1) {
                images.push(ReportImage {
                    file_name: report::image_file_name("photo", view, name),
                    image: photo,
                });
                images.push(ReportImage {
                    file_name: report::image_file_name("render", view, name),
                    image: render,
                });
                images.push(ReportImage {
                    file_name: report::image_file_name("difference", view, name),
                    image: heat,
                });
            }
        }
        let planned = host.window.get_planned_text().to_string();
        let input_text = report::markdown(&ReportInput {
            rough_name: &context.rough_name,
            rig_name: state.rig().map_or("", |r| r.name.as_str()),
            view_names: &names,
            fit: &output.fit,
            zoned: &output.zoned,
            planned_mm: parse_number(&planned, "The planned width").unwrap_or(12.0),
            mask_notes: &state.mask_notes(),
            date: &format!("Created at Unix time {}.", now_seconds()),
        });
        (input_text, images)
    };
    let (text, images) = prepared;
    spawn_job(
        host,
        "Writing the report...",
        move |_, _| report::write_report(&folder, &text, &images),
        |host, result| match result {
            Ok(files) => set_status(host, &format!("Report written: {} files.", files.len())),
            Err(message) => set_error(host, &message),
        },
    );
}
