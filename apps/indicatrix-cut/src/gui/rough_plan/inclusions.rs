//! Inclusions of a mesh rough in the planner window: "Add inclusion...", the list with its
//! Remove buttons, and the entry point a tool that locates an inclusion adds it through.
//!
//! An inclusion is a closed mesh inside the rough: stones keep clear of it, and the weight
//! and the yield count it as material (see the core's mesh `inclusion` module). The registry
//! keeps the rough with its inclusions under one base, so adding or removing one is a change
//! of the model's base, one undo step through [`install_base`], exactly like importing a mesh.
//!
//! The work (reading the file, building the meshes, checking the inclusion against the rough)
//! runs on a worker thread ([`mesh_task::spawn`]); no `RefCell` borrow is held across the file
//! dialog or a job. The logic that matters is in [`crate::mesh_io`] and the core, where the
//! tests run.
//!
//! # For a tool that locates inclusions
//!
//! [`add_inclusion_to_rough`] takes the inclusion as a [`RoughMesh`] in the rough's own frame
//! (millimetres, the rough's bounding box at the origin), whatever found it, and a margin;
//! it needs no file. A mesh built from points and triangles is `RoughMesh::new(&points,
//! &triangles)`.

use super::{
    editing::install_base,
    host::{Host, on_idle_host},
    locate::open_locate,
    mesh_task,
    obj_import::read_mesh_file,
    saved::{show_error, show_status},
};
use crate::{
    RoughPlanModel,
    gui::pickers::{PickerFilter, PickerKind, PickerRequest, pick},
    mesh_io::{
        MeshUnit, NOT_A_MESH_ROUGH, add_inclusion, inclusion_in_rough_frame, inclusion_parts,
        inclusion_row_text, parse_margin_mm,
    },
};
use indicatrix_cut_core::rough_plan::{
    RoughBase,
    shape::{
        RoughMesh,
        hull::{self, inclusion_list, remove_inclusion},
    },
};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::{path::PathBuf, rc::Rc};

/// Shown when the rough changed while an inclusion was being read, added or removed.
const ROUGH_CHANGED_MESSAGE: &str =
    "The rough changed while the inclusion was being read. Do it again.";

/// Registers the inclusion callbacks on the window's `RoughPlanModel`. While a plan runs they
/// are ignored (the controls are disabled in the window as well).
pub(super) fn setup_inclusion_callbacks(host: &Rc<Host>) {
    let model = host.window.global::<RoughPlanModel>();
    model.on_add_inclusion(|| on_idle_host(start_add));
    model.on_remove_inclusion(|row| on_idle_host(|host| remove(host, row)));
    model.on_locate_inclusion(|| on_idle_host(open_locate));
}

/// The model's base when it is an imported mesh (the only rough that takes inclusions).
fn mesh_base(host: &Host) -> Option<RoughBase> {
    let base = host.session.borrow().model.base;
    matches!(base, RoughBase::Hull { .. }).then_some(base)
}

/// Mirrors the inclusions of the model's base into the window: whether the rough can take
/// them, and one line per inclusion. Called whenever the model is mirrored.
pub(super) fn push(host: &Host) {
    let base = host.session.borrow().model.base;
    let rows: Vec<SharedString> = match base {
        RoughBase::Hull { id, .. } => inclusion_list(id)
            .iter()
            .enumerate()
            .map(|(i, info)| inclusion_row_text(i, info.extents_mm, info.volume_mm3).into())
            .collect(),
        _ => Vec::new(),
    };
    // Photos are traced through a mesh: a convex outline without one cannot be used.
    let has_mesh = matches!(base, RoughBase::Hull { id, .. } if hull::mesh(id).is_some());
    let model = host.window.global::<RoughPlanModel>();
    model.set_inclusions_possible(matches!(base, RoughBase::Hull { .. }));
    model.set_locate_possible(has_mesh);
    model.set_inclusion_rows(ModelRc::new(VecModel::from(rows)));
}

/// The "Add inclusion..." button: asks for a mesh file and adds its mesh as an inclusion of
/// the rough. The unit and the margin are the ones in the window now; the file is read in the
/// same coordinates as the rough's file.
fn start_add(host: &Rc<Host>) {
    show_error(host, "");
    if mesh_base(host).is_none() {
        show_error(host, NOT_A_MESH_ROUGH);
        return;
    }
    let model = host.window.global::<RoughPlanModel>();
    let margin_mm = match parse_margin_mm(model.get_inclusion_margin().as_str()) {
        Ok(margin_mm) => margin_mm,
        Err(message) => {
            show_error(host, &message);
            return;
        }
    };
    // Read now, before the dialog: the choices are the ones made for this inclusion.
    let unit = MeshUnit::from_index(model.get_mesh_unit_index());
    let Some(main) = host.main.upgrade() else {
        return;
    };
    let request = PickerRequest {
        kind: PickerKind::OpenFile,
        title: Some("Add an inclusion from a mesh file".to_string()),
        filters: vec![PickerFilter {
            label: "Mesh (OBJ, STL, PLY)".to_string(),
            extensions: ["obj", "stl", "ply"].map(str::to_string).to_vec(),
        }],
        default_file_name: None,
        starting_dir: None,
    };
    pick(&main, request, move |_, path| {
        let Some(path) = path else {
            return;
        };
        on_idle_host(move |host| read_inclusion(host, path, unit, margin_mm));
    });
}

/// Reads the mesh file at `path` (its numbers in `unit`) on a worker thread, moves it into
/// the rough's frame and, when it is ready, adds it with [`add_inclusion_to_rough`].
fn read_inclusion(host: &Rc<Host>, path: PathBuf, unit: MeshUnit, margin_mm: f64) {
    let Some(base) = mesh_base(host) else {
        show_error(host, NOT_A_MESH_ROUGH);
        return;
    };
    show_error(host, "");
    mesh_task::spawn(
        host,
        "Reading the inclusion...",
        move || {
            let bytes = read_mesh_file(&path)?;
            let extension = path.extension().and_then(|e| e.to_str());
            let (points, triangles) = inclusion_parts(&bytes, extension, unit)?;
            inclusion_in_rough_frame(&base, &points, &triangles)
        },
        move |host, result| match result {
            Ok(inclusion) => add_inclusion_to_rough(host, inclusion, margin_mm),
            Err(message) => show_error(host, &format!("Inclusion not added: {message}")),
        },
    );
}

/// Adds `inclusion`, a closed mesh in the rough's own frame (millimetres, the rough's bounding
/// box at the origin), to the rough with a margin of `margin_mm`, as one undo step. This is the
/// way in for anything that finds an inclusion by other means than a file; the file route
/// goes through it too.
///
/// It must be called on the UI thread. The inclusion is checked against the rough on a worker
/// thread; a refusal (it reaches the surface, lies outside the material, crosses another
/// inclusion, cannot hold its margin) is shown on the window's error line and nothing changes.
pub(super) fn add_inclusion_to_rough(host: &Rc<Host>, inclusion: RoughMesh, margin_mm: f64) {
    add_inclusion_notify(host, inclusion, margin_mm, |_, _| {});
}

/// [`add_inclusion_to_rough`], and `notify` is told how it ended: `Ok` when the inclusion was
/// added (one undo step), `Err` with the reason when it was refused or the rough changed
/// meanwhile (the reason is on the planner's error line as well). `notify` runs on the UI
/// thread; it is not called when the job was overtaken (a newer job, a reset) or when there
/// was no mesh rough to add to, which the error line reports.
///
/// This is how "Locate inclusion from photos" adds the point it found and learns whether it
/// went in, so that it can keep the located inclusion's marks with its rig.
pub(super) fn add_inclusion_notify(
    host: &Rc<Host>,
    inclusion: RoughMesh,
    margin_mm: f64,
    notify: impl FnOnce(&Rc<Host>, Result<(), String>) + Send + 'static,
) {
    let Some(base) = mesh_base(host) else {
        show_error(host, NOT_A_MESH_ROUGH);
        return;
    };
    mesh_task::spawn(
        host,
        "Adding the inclusion...",
        move || add_inclusion(&base, &inclusion, margin_mm),
        move |host, result| {
            let outcome = finish(
                host,
                base,
                result,
                "Inclusion not added:",
                "Inclusion added. Stones keep clear of it, and the weight and the yield count it \
                 as material.",
            );
            notify(host, outcome);
        },
    );
}

/// The Remove button of inclusion `row`.
fn remove(host: &Rc<Host>, row: i32) {
    let (Ok(index), Some(base)) = (usize::try_from(row), mesh_base(host)) else {
        return;
    };
    show_error(host, "");
    mesh_task::spawn(
        host,
        "Removing the inclusion...",
        move || remove_inclusion(&base, index).map_err(|e| e.to_string()),
        move |host, result| {
            let _ = finish(
                host,
                base,
                result,
                "Inclusion not removed:",
                "Inclusion removed.",
            );
        },
    );
}

/// The UI-thread end of an inclusion job: installs the new base as one undo step when the
/// model still has the base the job started from, and says what happened. The result is `Ok`
/// when the base was installed and the reason (also shown) when it was not.
fn finish(
    host: &Rc<Host>,
    before: RoughBase,
    result: Result<RoughBase, String>,
    failure: &str,
    success: &str,
) -> Result<(), String> {
    let base = match result {
        Ok(base) => base,
        Err(message) => {
            show_error(host, &format!("{failure} {message}"));
            return Err(message);
        }
    };
    if host.session.borrow().model.base != before {
        show_error(host, ROUGH_CHANGED_MESSAGE);
        return Err(ROUGH_CHANGED_MESSAGE.to_owned());
    }
    install_base(host, base);
    show_status(host, success);
    Ok(())
}
