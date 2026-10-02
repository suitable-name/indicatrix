//! The model editing callbacks of the planner window: base, cuts, undo and redo.
//!
//! Each edit changes the session's model, mirrors it into the window and asks the shape
//! worker for fresh figures.
//!
//! Two kinds of edit exist. A live edit (typing in a cut field) changes the model and the
//! worker's figures at once but is no undo step; a committed edit (Enter or focus loss,
//! adding or removing a cut, changing the faces or the base) is one.

use super::{
    base::{
        BaseFields, blank_model, is_blank, parse_base, pending_base, push_base, switch_base_kind,
    },
    cut_faces::{
        CORNERS, DEFAULT_CORNER, DEFAULT_EDGE, EDGES, clamp_corner_setbacks, clamp_edge_setbacks,
        default_corner, default_edge, default_face,
    },
    cut_rows::{CutField, apply_field, cut_row, cut_rows, field_text, set_field_text},
    format::to_i32,
    host::{Host, on_host, on_idle_host},
    inputs::{PICK_MATERIAL_MESSAGE, is_input_message, parse_mm, parse_weighed},
    shape_worker::{ShapeRequest, show_blank},
};
use crate::{RoughCutRow, RoughPlanModel};
use indicatrix_cut_core::rough_plan::{RoughBase, RoughCut};
use slint::{ComponentHandle, Model};
use std::{rc::Rc, time::Duration};

/// How long a field that held text that is not a number shows a stand-in before it shows
/// the model's value. The field only takes a pushed text when that text differs from the
/// previous push, so the revert goes through two different texts, a pass of the event
/// loop apart.
const REVERT_DELAY: Duration = Duration::from_millis(30);

/// Shown when an edge or corner cut is asked for on a base that is not a block.
const BLOCK_ONLY: &str = "Edge and corner cuts are only supported on block rough.";

/// Shown when a cut is asked for before the rough has a size.
const NEEDS_SIZE: &str = "Enter the rough size before adding cuts.";

/// Registers the model editing callbacks on the window's `RoughPlanModel`. While a plan
/// runs they are ignored (the controls are disabled in the window as well).
pub(super) fn setup_edit_callbacks(host: &Rc<Host>) {
    let model = host.window.global::<RoughPlanModel>();
    model.on_set_base(|kind| on_idle_host(|host| set_base(host, kind)));
    model.on_import_obj(|| on_idle_host(super::obj_import::start));
    model.on_base_field_committed(|| on_idle_host(base_field_committed));
    model.on_fit_to_weight(|| on_idle_host(super::carat::fit_to_weight));
    model.on_add_cut(|kind| on_idle_host(|host| add_cut(host, kind)));
    model.on_remove_cut(|row| on_idle_host(|host| remove_cut(host, row)));
    model.on_cut_field_edited(|row, field, text| {
        on_idle_host(|host| cut_field_edited(host, row, field, &text));
    });
    model.on_cut_field_committed(|row| on_idle_host(|host| cut_field_committed(host, row)));
    model.on_cut_faces_changed(|row, index| {
        on_idle_host(|host| cut_faces_changed(host, row, index));
    });
    model.on_undo_shape(|| on_idle_host(undo_shape));
    model.on_redo_shape(|| on_idle_host(redo_shape));
    model.on_new_model(|| on_idle_host(new_model));
}

/// The window inputs the shape worker needs besides the model.
pub(super) struct ShapeInputs {
    pub(super) specific_gravity: f64,
    pub(super) material_name: String,
    pub(super) weighed_ct: Option<f64>,
    pub(super) selected_cut: Option<usize>,
}

/// Reads the material, the weighed carat and the selected cut off the window. The carat
/// counts as weighed only when it was typed over the model's own (see `carat`) and is a
/// positive number; anything else only switches the weight check off (the model itself
/// is still evaluated).
pub(super) fn read_inputs(host: &Rc<Host>) -> ShapeInputs {
    let model = host.window.global::<RoughPlanModel>();
    let weighed_ct = super::carat::typed_carat(host).ok().flatten();
    let (material_name, specific_gravity) = usize::try_from(model.get_material_index())
        .ok()
        .and_then(|i| {
            host.session
                .borrow()
                .choices
                .get(i)
                .map(|c| (c.name.clone(), c.specific_gravity))
        })
        .unwrap_or_default();
    ShapeInputs {
        specific_gravity,
        material_name,
        weighed_ct,
        selected_cut: usize::try_from(model.get_selected_cut()).ok(),
    }
}

/// Shows `message` as the model error and makes every earlier shape result stale (the
/// inputs no longer describe a model that can be evaluated).
pub(super) fn input_error(host: &Rc<Host>, message: &str) {
    host.session.borrow_mut().revision += 1;
    host.shape.invalidate();
    host.window
        .global::<RoughPlanModel>()
        .set_model_error(message.into());
}

/// Clears the error line when it holds a message about the inputs (see
/// [`is_input_message`]): it is out of date once an input changes. A run or library
/// message stays.
fn clear_input_error(host: &Rc<Host>) {
    let model = host.window.global::<RoughPlanModel>();
    if is_input_message(model.get_error_text().as_str()) {
        model.set_error_text("".into());
    }
}

/// Asks the shape worker for the figures of the session's model, unless it is the very
/// request the worker got last. A model without a size shows nothing instead.
pub(super) fn after_change(host: &Rc<Host>) {
    clear_input_error(host);
    let inputs = read_inputs(host);
    let mut session = host.session.borrow_mut();
    if is_blank(&session.model) {
        session.revision += 1;
        drop(session);
        host.shape.invalidate();
        show_blank(host);
        return;
    }
    let request = ShapeRequest {
        revision: session.revision + 1,
        model: session.model.clone(),
        specific_gravity: inputs.specific_gravity,
        material_name: inputs.material_name,
        weighed_ct: inputs.weighed_ct,
        selected_cut: inputs.selected_cut,
    };
    if host.shape.submit_changed(request) {
        session.revision += 1;
    }
}

/// The material or weighed carat changed (or the material list was refilled): the
/// figures need a fresh evaluation even though the model did not change.
pub(super) fn readout_inputs_changed(host: &Rc<Host>) {
    host.shape.invalidate();
    after_change(host);
}

/// Mirrors the undo and redo availability into the window.
pub(super) fn push_history_flags(host: &Rc<Host>) {
    let (can_undo, can_redo) = {
        let session = host.session.borrow();
        (session.can_undo(), session.can_redo())
    };
    let model = host.window.global::<RoughPlanModel>();
    model.set_can_undo(can_undo);
    model.set_can_redo(can_redo);
}

/// Mirrors the whole session model into the window after a programmatic change (undo,
/// redo, a new or removed cut, a face change, a view click): new cut rows, so the row
/// components are re-created, the history flags, and a fresh evaluation. `rewrite_base`
/// also rewrites the size fields. The azimuths typed for face rows are forgotten: the
/// cuts may have been renumbered or replaced.
pub(super) fn resync(host: &Rc<Host>, rewrite_base: bool) {
    rebuild(host, rewrite_base, false);
}

/// [`resync`], keeping the azimuths typed for face rows when `keep_azimuths` (for a change
/// that leaves the cut numbers alone).
fn rebuild(host: &Rc<Host>, rewrite_base: bool, keep_azimuths: bool) {
    let (snapshot, azimuths) = {
        let mut session = host.session.borrow_mut();
        session.bad_fields.clear();
        if !keep_azimuths {
            session.face_azimuths.clear();
        }
        (session.model.clone(), session.face_azimuths.clone())
    };
    let model = host.window.global::<RoughPlanModel>();
    if rewrite_base {
        push_base(&host.window, &snapshot.base);
    }
    model.set_cuts(cut_rows(&snapshot, &azimuths));
    if usize::try_from(model.get_selected_cut()).is_ok_and(|row| row >= snapshot.cuts.len()) {
        model.set_selected_cut(-1);
    }
    push_history_flags(host);
    host.shape.invalidate();
    after_change(host);
}

/// The new base for a radio pick, from the size the fields show right now.
fn set_base(host: &Rc<Host>, kind: i32) {
    let fields = BaseFields::read(&host.window);
    let current = parse_base(&fields).unwrap_or_else(|_| host.session.borrow().model.base);
    install_base(host, switch_base_kind(&current, kind));
}

/// Makes `base` the model's base as one undo step and shows it.
pub(super) fn install_base(host: &Rc<Host>, base: RoughBase) {
    {
        let mut session = host.session.borrow_mut();
        session.model.base = base;
        session.commit();
    }
    resync(host, true);
}

/// What committing the typed size fields did.
#[derive(Debug, Clone, PartialEq, Eq)]
enum BaseCommit {
    /// A field is not a number; this message is shown as the model error.
    Invalid(String),
    /// The fields show the session's base already.
    Unchanged,
    /// The typed base is the session's base now (one undo step).
    Changed,
}

/// Moves the typed size fields into the session's model as one undo step. The caller
/// re-evaluates the model when the answer is [`BaseCommit::Changed`].
fn commit_base_fields(host: &Rc<Host>) -> BaseCommit {
    let fields = BaseFields::read(&host.window);
    let current = host.session.borrow().model.base;
    match pending_base(&fields, &current) {
        Err(message) => {
            input_error(host, &message);
            BaseCommit::Invalid(message)
        }
        Ok(None) => BaseCommit::Unchanged,
        Ok(Some(base)) => {
            let recorded = {
                let mut session = host.session.borrow_mut();
                session.model.base = base;
                session.commit()
            };
            if recorded {
                push_history_flags(host);
            }
            BaseCommit::Changed
        }
    }
}

/// Commits the typed size fields and re-evaluates the model if they changed it (or if an
/// earlier input error still shows).
///
/// # Errors
///
/// Returns the message (also shown as the model error) of a size field that is not a
/// number.
fn commit_sizes(host: &Rc<Host>) -> Result<(), String> {
    let stale_error = !host
        .window
        .global::<RoughPlanModel>()
        .get_model_error()
        .is_empty();
    match commit_base_fields(host) {
        BaseCommit::Invalid(message) => Err(message),
        BaseCommit::Changed => {
            after_change(host);
            Ok(())
        }
        BaseCommit::Unchanged => {
            if stale_error {
                host.shape.invalidate();
                after_change(host);
            }
            Ok(())
        }
    }
}

/// [`commit_sizes`] for the callers that only need to know whether to go on: false when a
/// size field is not a number.
///
/// A size typed into a field is only committed by Enter or by leaving the field, and
/// clicking a button or the view does neither: every action that works on the model
/// starts with this.
pub(super) fn commit_pending_sizes(host: &Rc<Host>) -> bool {
    commit_sizes(host).is_ok()
}

/// Commits everything typed into the rough form: the size fields (see
/// [`commit_pending_sizes`]) and the material and weighed-carat fields, which are only
/// checked (they are not part of the model).
///
/// # Errors
///
/// Returns the message of the first invalid field, which is also shown in the window (as
/// the model error, the weight check or the error line); the caller then stops what it
/// was about to do and may show the message where the user is looking.
pub(super) fn commit_pending_base(host: &Rc<Host>) -> Result<(), String> {
    commit_sizes(host)?;
    let model = host.window.global::<RoughPlanModel>();
    if let Err(message) = parse_weighed(&model.get_weighed_ct()) {
        model.set_weight_check_text(message.clone().into());
        model.set_weight_check_level(3);
        return Err(message);
    }
    let material_known = usize::try_from(model.get_material_index())
        .is_ok_and(|index| index < host.session.borrow().choices.len());
    if !material_known {
        model.set_error_text(PICK_MATERIAL_MESSAGE.into());
        return Err(PICK_MATERIAL_MESSAGE.to_string());
    }
    Ok(())
}

/// A base field, the material or the weighed carat was committed.
fn base_field_committed(host: &Rc<Host>) {
    clear_input_error(host);
    if !matches!(commit_base_fields(host), BaseCommit::Invalid(_)) {
        readout_inputs_changed(host);
    }
}

/// A new cut of window kind `kind` (0 edge, 1 corner, 2 face) with default values for
/// `base`.
///
/// # Errors
///
/// Returns why the cut cannot be added.
fn new_cut(base: &RoughBase, kind: i32) -> Result<RoughCut, String> {
    let extents = base.bounding_box_extents();
    let is_block = matches!(base, RoughBase::Block { .. });
    match kind {
        0 | 1 if !is_block => Err(BLOCK_ONLY.to_string()),
        0 => Ok(default_edge(EDGES[DEFAULT_EDGE], extents)),
        1 => Ok(default_corner(CORNERS[DEFAULT_CORNER], extents)),
        _ => Ok(default_face(base, [0.0, 1.0, 0.0])),
    }
}

/// `+Edge`, `+Corner` or `+Face`.
fn add_cut(host: &Rc<Host>, kind: i32) {
    if !commit_pending_sizes(host) {
        return;
    }
    let cut = {
        let session = host.session.borrow();
        if is_blank(&session.model) {
            Err(NEEDS_SIZE.to_string())
        } else {
            new_cut(&session.model.base, kind)
        }
    };
    let cut = match cut {
        Ok(cut) => cut,
        Err(message) => {
            host.window
                .global::<RoughPlanModel>()
                .set_model_error(message.as_str().into());
            return;
        }
    };
    add_committed_cut(host, cut);
}

/// Adds `cut` to the model as one undo step, selects its row, puts the cursor in the
/// row's first field and re-evaluates.
pub(super) fn add_committed_cut(host: &Rc<Host>, cut: RoughCut) {
    let index = {
        let mut session = host.session.borrow_mut();
        session.model.cuts.push(cut);
        session.commit();
        session.model.cuts.len() - 1
    };
    let model = host.window.global::<RoughPlanModel>();
    model.set_selected_cut(to_i32(index));
    model.set_focus_cut_row(to_i32(index));
    // Appending renumbers nothing, so the azimuths typed for other rows stay valid.
    rebuild(host, false, true);
}

/// Where the selection goes when row `removed` is deleted.
#[must_use]
const fn selection_after_removal(selected: i32, removed: i32) -> i32 {
    if selected == removed {
        -1
    } else if selected > removed {
        selected - 1
    } else {
        selected
    }
}

/// The delete button of a cut row.
fn remove_cut(host: &Rc<Host>, row: i32) {
    let Ok(index) = usize::try_from(row) else {
        return;
    };
    {
        let mut session = host.session.borrow_mut();
        if index >= session.model.cuts.len() {
            return;
        }
        session.model.cuts.remove(index);
        session.commit();
    }
    let model = host.window.global::<RoughPlanModel>();
    model.set_selected_cut(selection_after_removal(model.get_selected_cut(), row));
    resync(host, false);
}

/// Changes cut row `index` of the window with `change`, writing it back (that row only,
/// so no row component is re-created and no field loses the keyboard focus) when the
/// change altered it.
fn update_row(host: &Rc<Host>, index: usize, change: impl FnOnce(&mut RoughCutRow)) {
    let rows = host.window.global::<RoughPlanModel>().get_cuts();
    if let Some(mut row) = rows.row_data(index) {
        let before = row.clone();
        change(&mut row);
        if row != before {
            rows.set_row_data(index, row);
        }
    }
}

/// Shows `message` under the cut row `index` (an empty message clears it).
fn set_row_error(host: &Rc<Host>, index: usize, message: &str) {
    update_row(host, index, |row| row.error = message.into());
}

/// Marks `field` of cut row `index` as holding text that is not a valid value, with the
/// reason shown under the row. The model keeps its last valid value until the field is
/// committed.
fn mark_bad_text(host: &Rc<Host>, index: usize, field: i32, message: &str) {
    host.session.borrow_mut().bad_fields.insert((index, field));
    set_row_error(host, index, message);
}

/// Live typing in a cut field: applies a number at once (no undo step), or marks the
/// field as invalid until the edit is committed. The number of an azimuth or elevation
/// must be in range (-180..=180 and -90..=90 degrees); the typed azimuth of a face is
/// remembered, because a vertical face cannot tell it.
fn cut_field_edited(host: &Rc<Host>, row: i32, field: i32, text: &str) {
    let (Ok(index), Some(cut_field)) = (usize::try_from(row), CutField::from_index(field)) else {
        return;
    };
    let value = match parse_mm(text, cut_field.label()) {
        Ok(value) => value,
        Err(message) => {
            mark_bad_text(host, index, field, &message);
            return;
        }
    };
    let applied = {
        let mut session = host.session.borrow_mut();
        let typed_azimuth = session.face_azimuths.get(&index).copied();
        let Some(cut) = session.model.cuts.get_mut(index) else {
            return;
        };
        apply_field(cut, cut_field, value, typed_azimuth)
    };
    match applied {
        Ok(azimuth) => {
            let others_bad = {
                let mut session = host.session.borrow_mut();
                session.bad_fields.remove(&(index, field));
                if let Some(azimuth) = azimuth {
                    session.face_azimuths.insert(index, azimuth);
                }
                session.bad_fields.iter().any(|&(bad, _)| bad == index)
            };
            if !others_bad {
                set_row_error(host, index, "");
            }
            push_history_flags(host);
            after_change(host);
        }
        Err(message) => mark_bad_text(host, index, field, &message),
    }
}

/// The window row the session's model gives cut `index`, with the azimuth typed for it.
fn model_row(host: &Rc<Host>, index: usize) -> Option<RoughCutRow> {
    let session = host.session.borrow();
    let typed_azimuth = session.face_azimuths.get(&index).copied();
    session
        .model
        .cuts
        .get(index)
        .map(|cut| cut_row(cut, typed_azimuth))
}

/// Puts the model's values into the fields of cut row `index` that `fields` names, and
/// the row's title and hint, leaving the typed text of every other field alone.
fn show_model_values(host: &Rc<Host>, index: usize, fields: &[CutField]) {
    let Some(fresh) = model_row(host, index) else {
        return;
    };
    update_row(host, index, |row| {
        row.title = fresh.title.clone();
        row.hint = fresh.hint.clone();
        for &field in fields {
            set_field_text(row, field, field_text(&fresh, field).as_str());
        }
    });
}

/// Brings fields that held invalid text back to the model's values. A field only takes a
/// text pushed to it when it differs from the text pushed before, and the row still holds
/// the model's text from before the bad typing; so the fields first get the model's text
/// with a trailing blank, and the plain text follows [`REVERT_DELAY`] later.
fn revert_bad_fields(host: &Rc<Host>, index: usize, fields: Vec<CutField>) {
    let Some(standin) = model_row(host, index) else {
        return;
    };
    update_row(host, index, |row| {
        row.error = "".into();
        for &field in &fields {
            let text = format!("{} ", field_text(&standin, field));
            set_field_text(row, field, &text);
        }
    });
    slint::Timer::single_shot(REVERT_DELAY, move || {
        on_host(|host| {
            // A field that turned invalid again in the meantime is the user's to fix.
            let still_good: Vec<CutField> = {
                let session = host.session.borrow();
                fields
                    .iter()
                    .copied()
                    .filter(|field| !session.bad_fields.contains(&(index, field.number())))
                    .collect()
            };
            show_model_values(host, index, &still_good);
        });
    });
}

/// Enter or focus loss in a cut field: the live edits become one undo step. A field that
/// still holds text that is not a valid value goes back to the model's value (that row
/// only is written, see [`revert_bad_fields`]); the angles of a face row are rewritten
/// from the model, so the row shows what the model holds and the pole hint.
fn cut_field_committed(host: &Rc<Host>, row: i32) {
    let Ok(index) = usize::try_from(row) else {
        return;
    };
    let (bad, recorded) = {
        let mut session = host.session.borrow_mut();
        let bad: Vec<CutField> = session
            .bad_fields
            .iter()
            .filter(|&&(bad_row, _)| bad_row == index)
            .filter_map(|&(_, field)| CutField::from_index(field))
            .collect();
        session.bad_fields.retain(|&(bad_row, _)| bad_row != index);
        (bad, session.commit())
    };
    if recorded {
        push_history_flags(host);
    }
    show_model_values(host, index, &[CutField::Azimuth, CutField::Elevation]);
    if !bad.is_empty() {
        revert_bad_fields(host, index, bad);
        // The row's error line was for the bad text; the evaluation shows the real one.
        host.shape.invalidate();
        after_change(host);
    }
}

/// Puts `cut` on the faces of drop-down entry `index`, keeping its setbacks clamped to
/// the new faces' extents. Returns whether the cut changed.
fn change_faces(cut: &mut RoughCut, index: usize, extents: [f64; 3]) -> bool {
    let before = cut.clone();
    match cut {
        RoughCut::Edge { faces, setbacks_mm } => {
            if let Some(&new_faces) = EDGES.get(index) {
                *faces = new_faces;
                *setbacks_mm = clamp_edge_setbacks(new_faces, *setbacks_mm, extents);
            }
        }
        RoughCut::Corner { faces, setbacks_mm } => {
            if let Some(&new_faces) = CORNERS.get(index) {
                *faces = new_faces;
                *setbacks_mm = clamp_corner_setbacks(new_faces, *setbacks_mm, extents);
            }
        }
        RoughCut::Face { .. } => {}
    }
    *cut != before
}

/// The edge or corner drop-down of a cut row.
fn cut_faces_changed(host: &Rc<Host>, row: i32, index: i32) {
    let (Ok(row), Ok(index)) = (usize::try_from(row), usize::try_from(index)) else {
        return;
    };
    let changed = {
        let mut session = host.session.borrow_mut();
        let extents = session.model.base.bounding_box_extents();
        let changed = session
            .model
            .cuts
            .get_mut(row)
            .is_some_and(|cut| change_faces(cut, index, extents));
        if changed {
            session.commit();
        }
        changed
    };
    if changed {
        rebuild(host, false, true);
    }
}

/// The Undo button. Sizes typed but not committed yet are committed first, so Undo takes
/// them back like any other edit; a field that is not a number is dropped (the fields
/// are rewritten from the model).
fn undo_shape(host: &Rc<Host>) {
    let _ = commit_base_fields(host);
    let changed = host.session.borrow_mut().undo_step();
    if changed {
        resync(host, true);
    } else {
        push_history_flags(host);
    }
}

/// The Redo button. Typed sizes count as a new edit, which ends the redo branch.
fn redo_shape(host: &Rc<Host>) {
    let _ = commit_base_fields(host);
    let changed = host.session.borrow_mut().redo_step();
    if changed {
        resync(host, true);
    } else {
        push_history_flags(host);
    }
}

/// The New button: an empty model (one undo step), without weighed carat or selection.
fn new_model(host: &Rc<Host>) {
    {
        let mut session = host.session.borrow_mut();
        session.replace_model(blank_model());
        session.carat_shown = None;
    }
    let model = host.window.global::<RoughPlanModel>();
    model.set_weighed_ct("".into());
    model.set_selected_cut(-1);
    model.set_model_error("".into());
    resync(host, true);
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix_cut_core::rough_plan::BoxFace;

    fn block() -> RoughBase {
        RoughBase::Block {
            x_mm: 10.0,
            y_mm: 8.0,
            z_mm: 6.0,
        }
    }

    #[test]
    fn each_add_button_makes_its_default_cut() {
        assert_eq!(
            new_cut(&block(), 0),
            Ok(default_edge(
                [BoxFace::Top, BoxFace::Front],
                [10.0, 8.0, 6.0]
            ))
        );
        assert_eq!(
            new_cut(&block(), 1),
            Ok(default_corner(
                [BoxFace::Top, BoxFace::Front, BoxFace::Right],
                [10.0, 8.0, 6.0]
            ))
        );
        let RoughCut::Face { normal, depth_mm } =
            new_cut(&block(), 2).expect("a face cut fits every base")
        else {
            panic!("kind 2 is a face cut");
        };
        assert_eq!(normal, [0.0, 1.0, 0.0]);
        assert!((depth_mm - 0.8).abs() < 1e-12);
    }

    #[test]
    fn edges_and_corners_are_refused_on_other_bases() {
        let pebble = RoughBase::Pebble {
            x_mm: 10.0,
            y_mm: 8.0,
            z_mm: 6.0,
        };
        assert_eq!(new_cut(&pebble, 0), Err(BLOCK_ONLY.to_string()));
        assert_eq!(new_cut(&pebble, 1), Err(BLOCK_ONLY.to_string()));
        assert!(new_cut(&pebble, 2).is_ok());
    }

    #[test]
    fn deleting_a_row_moves_the_selection_with_its_row() {
        assert_eq!(selection_after_removal(2, 2), -1);
        assert_eq!(selection_after_removal(3, 1), 2);
        assert_eq!(selection_after_removal(0, 1), 0);
        assert_eq!(selection_after_removal(-1, 0), -1);
    }

    #[test]
    fn a_face_change_keeps_the_setbacks_but_clamps_them() {
        // Top-Front (index 0) on a 10 x 8 x 6 block, setbacks 5 and 7.
        let mut cut = RoughCut::Edge {
            faces: [BoxFace::Top, BoxFace::Front],
            setbacks_mm: [5.0, 7.0],
        };
        let extents = [10.0, 8.0, 6.0];
        // Front-Left (index 8): setbacks[0] runs along Front, bounded by the X extent
        // (10); setbacks[1] runs along Left, bounded by the Z extent (6).
        assert!(change_faces(&mut cut, 8, extents));
        assert_eq!(
            cut,
            RoughCut::Edge {
                faces: [BoxFace::Front, BoxFace::Left],
                setbacks_mm: [5.0, 6.0],
            }
        );
        // The same entry again changes nothing; an index past the table changes nothing.
        assert!(!change_faces(&mut cut, 8, extents));
        assert!(!change_faces(&mut cut, 40, extents));

        let mut face = RoughCut::Face {
            normal: [0.0, 1.0, 0.0],
            depth_mm: 1.0,
        };
        assert!(!change_faces(&mut face, 0, extents));
    }

    #[test]
    fn a_corner_face_change_clamps_each_setback_to_its_own_axis() {
        let mut cut = RoughCut::Corner {
            faces: [BoxFace::Top, BoxFace::Front, BoxFace::Left],
            setbacks_mm: [7.5, 5.5, 9.5],
        };
        // Bottom-Back-Right (index 7): Bottom is Y (8), Back is Z (6), Right is X (10).
        assert!(change_faces(&mut cut, 7, [10.0, 8.0, 6.0]));
        assert_eq!(
            cut,
            RoughCut::Corner {
                faces: [BoxFace::Bottom, BoxFace::Back, BoxFace::Right],
                setbacks_mm: [7.5, 5.5, 9.5],
            }
        );
        // A narrower rough clamps them.
        assert!(change_faces(&mut cut, 6, [10.0, 4.0, 3.0]));
        let RoughCut::Corner { setbacks_mm, .. } = cut else {
            panic!("still a corner");
        };
        assert_eq!(setbacks_mm, [4.0, 3.0, 9.5]);
    }
}
