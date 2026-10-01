//! Live geometry of the edited rough on a background thread: volume, weight check, the
//! mesh the view draws and the volume the selected cut removes.
//!
//! Every model change submits a [`ShapeRequest`] tagged with the session revision; the
//! worker keeps only the latest one, and a result whose revision is no longer current is
//! dropped on the UI thread.

use super::{
    format::group_thousands,
    host::{Host, on_host},
    inputs::parse_weighed,
    metrics::{format_model_readout, format_weight_check},
    view,
};
use crate::{RoughPlanModel, RoughPlannerWindow, gui::latest_worker::LatestWorker};
use glam::DVec3;
use indicatrix::geometry::stone_metrics::{SolidMesh, SolidStatus, build_solid_mesh};
use indicatrix_cut_core::{
    rough_plan::{RoughBase, RoughMeasure, RoughModel, ShapeError},
    yield_metrics::carat_weight,
};
use slint::{ComponentHandle, Model, Timer, TimerMode, Weak};
use std::{
    cell::RefCell,
    collections::BTreeSet,
    rc::Rc,
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};

/// How long a pebble edit waits for the next one before its (slow) evaluation starts.
const PEBBLE_DEBOUNCE: Duration = Duration::from_millis(60);

/// What the model error says when the evaluation itself failed (a bug, not the model).
const INTERNAL_ERROR_TEXT: &str =
    "The model could not be evaluated (internal error); edit the model to retry.";

/// What the worker evaluates.
#[derive(Debug, Clone)]
pub(super) struct ShapeRequest {
    /// The session revision this request answers.
    pub(super) revision: u64,
    /// The model to evaluate.
    pub(super) model: RoughModel,
    /// The material's specific gravity (0 when no material is picked).
    pub(super) specific_gravity: f64,
    /// The material's name for the readout ("" when no material is picked).
    pub(super) material_name: String,
    /// The weighed carat to check against.
    pub(super) weighed_ct: Option<f64>,
    /// The cut whose removed volume is wanted.
    pub(super) selected_cut: Option<usize>,
}

impl ShapeRequest {
    /// Whether `other` asks for the same evaluation (the revision does not count).
    fn same_inputs(&self, other: &Self) -> bool {
        self.model == other.model
            && self.specific_gravity.to_bits() == other.specific_gravity.to_bits()
            && self.material_name == other.material_name
            && self.weighed_ct.map(f64::to_bits) == other.weighed_ct.map(f64::to_bits)
            && self.selected_cut == other.selected_cut
    }
}

/// A finished evaluation of a valid model.
#[derive(Debug)]
pub(super) struct ShapeOutput {
    /// The measured solid.
    pub(super) measure: RoughMeasure,
    /// The solid's mesh in the centred frame (bounding-box centre at the origin, mm). The
    /// same allocation is handed on for every request that only changes what is shown
    /// about the model, so the view can tell an unchanged solid by identity.
    pub(super) mesh: Arc<SolidMesh>,
    /// The cut the request asked about (the row the removed volume belongs to).
    pub(super) selected_cut: Option<usize>,
    /// Volume the `selected_cut` removes, in mm^3.
    pub(super) removed_mm3: Option<f64>,
    /// "Model 257 mm³ · 3.40 ct (Quartz, SG 2.65)".
    pub(super) model_text: String,
    /// The weight check sentence ("" without a weighed carat).
    pub(super) check_text: String,
    /// 0 none, 1 matches, 2 close, 3 check the model.
    pub(super) check_level: i32,
    /// The material's specific gravity (for the removed-volume text).
    pub(super) specific_gravity: f64,
}

/// Why a request has no figures.
#[derive(Debug)]
pub(super) enum ShapeFailure {
    /// The model is invalid or leaves nothing.
    Invalid(ShapeError),
    /// The evaluation itself went wrong (the worker panicked on the request).
    Internal,
}

impl ShapeFailure {
    /// The sentence shown as the model error.
    fn message(&self) -> String {
        match self {
            Self::Invalid(error) => error.to_string(),
            Self::Internal => INTERNAL_ERROR_TEXT.to_string(),
        }
    }

    /// The cut row the failure belongs to, if it names one.
    const fn row(&self) -> Option<usize> {
        match self {
            Self::Invalid(error) => error_row(error),
            Self::Internal => None,
        }
    }
}

/// The worker's answer to one request.
#[derive(Debug)]
pub(super) struct ShapeResult {
    /// The revision of the request this answers.
    pub(super) revision: u64,
    /// The evaluation, or why there is none.
    pub(super) outcome: Result<ShapeOutput, ShapeFailure>,
}

/// The mesh that the planes `planes` bound, translated so `centre` is the origin. The
/// mesh's facet ids index `planes`.
///
/// # Errors
///
/// Returns [`ShapeError::NothingLeft`] when the planes do not close into a solid.
fn mesh_of_planes(planes: &[(DVec3, f64)], centre: DVec3) -> Result<SolidMesh, ShapeError> {
    let centred: Vec<(DVec3, f64)> = planes
        .iter()
        .map(|&(normal, offset)| (normal, offset - normal.dot(centre)))
        .collect();
    match build_solid_mesh(&centred) {
        SolidStatus::Closed(mesh) => Ok(mesh),
        SolidStatus::Unbounded { .. } | SolidStatus::Degenerate { .. } => {
            Err(ShapeError::NothingLeft)
        }
    }
}

/// The model's planes translated so the base's bounding-box centre is the origin, and
/// the mesh they bound. The mesh's facet ids index the model's halfspaces: the base's
/// planes first, then one per cut.
///
/// # Errors
///
/// Returns why the model is invalid, or [`ShapeError::NothingLeft`] when the planes do not
/// close into a solid.
pub(super) fn centred_mesh(model: &RoughModel) -> Result<SolidMesh, ShapeError> {
    let planes = model.halfspaces()?;
    mesh_of_planes(&planes, model.base.bounding_box_centre())
}

/// The volume cut `index` removes: the model's volume without it minus with it.
fn removed_volume(model: &RoughModel, index: usize, volume_mm3: f64) -> Option<f64> {
    if index >= model.cuts.len() {
        return None;
    }
    let mut without = model.clone();
    without.cuts.remove(index);
    let full = without.measure().ok()?;
    Some((full.volume_mm3 - volume_mm3).max(0.0))
}

/// What the worker keeps of its last evaluated model: everything that depends on the
/// model alone, so a request that only changes the selected cut, the material or the
/// weighed carat does not measure and mesh the same solid again.
struct Solid {
    /// The model the figures belong to.
    model: RoughModel,
    /// The measured solid.
    measure: RoughMeasure,
    /// The solid's mesh in the centred frame.
    mesh: Arc<SolidMesh>,
    /// The last removed-volume answer, with the cut it was asked for.
    removed: Option<(usize, Option<f64>)>,
}

impl Solid {
    /// Measures and meshes `model`; the halfspaces are computed once for both.
    fn build(model: &RoughModel) -> Result<Self, ShapeError> {
        let planes = model.halfspaces()?;
        let measure = model.measure_with_halfspaces(&planes)?;
        let mesh = mesh_of_planes(&planes, model.base.bounding_box_centre())?;
        Ok(Self {
            model: model.clone(),
            measure,
            mesh: Arc::new(mesh),
            removed: None,
        })
    }

    /// The volume cut `cut` removes (`None` without a cut, or for one that does not
    /// exist), measured once per cut.
    fn removed_by(&mut self, cut: Option<usize>) -> Option<f64> {
        let index = cut?;
        if let Some((_, value)) = self.removed.filter(|&(asked, _)| asked == index) {
            return value;
        }
        let value = removed_volume(&self.model, index, self.measure.volume_mm3);
        self.removed = Some((index, value));
        value
    }
}

/// The readout line for a measured volume.
fn model_text(request: &ShapeRequest, volume_mm3: f64) -> String {
    if request.material_name.is_empty() {
        format!("Model {} mm³", group_thousands(volume_mm3.round() as usize))
    } else {
        format_model_readout(volume_mm3, request.specific_gravity, &request.material_name)
    }
}

/// Evaluates `request`, reusing `previous` when it describes the same model. Returns the
/// answer and the solid to keep for the next request (none when the model is invalid).
/// Pure: no window, no session.
fn evaluate_reusing(
    request: &ShapeRequest,
    previous: Option<Solid>,
) -> (ShapeResult, Option<Solid>) {
    let built = previous
        .filter(|kept| kept.model == request.model)
        .map_or_else(|| Solid::build(&request.model), Ok);
    let mut solid = match built {
        Ok(solid) => solid,
        Err(error) => {
            let result = ShapeResult {
                revision: request.revision,
                outcome: Err(ShapeFailure::Invalid(error)),
            };
            return (result, None);
        }
    };
    let removed_mm3 = solid.removed_by(request.selected_cut);
    let (check_text, check_level) = format_weight_check(
        solid.measure.volume_mm3,
        request.specific_gravity,
        request.weighed_ct,
    );
    let output = ShapeOutput {
        model_text: model_text(request, solid.measure.volume_mm3),
        measure: solid.measure,
        mesh: Arc::clone(&solid.mesh),
        selected_cut: request.selected_cut,
        removed_mm3,
        check_text,
        check_level,
        specific_gravity: request.specific_gravity,
    };
    let result = ShapeResult {
        revision: request.revision,
        outcome: Ok(output),
    };
    (result, Some(solid))
}

/// The background thread, its request mailbox and the pebble debounce.
pub(super) struct ShapeWorker {
    worker: Rc<LatestWorker<ShapeRequest>>,
    debounce: Timer,
    pending: Rc<RefCell<Option<ShapeRequest>>>,
    last: RefCell<Option<ShapeRequest>>,
}

impl ShapeWorker {
    /// Starts the thread; its results are applied on `window`'s UI thread.
    ///
    /// Should an evaluation panic, the request it was working on is answered with an
    /// internal-error result, so the window shows a message instead of stale figures.
    #[must_use]
    pub(super) fn new(window: Weak<RoughPlannerWindow>) -> Self {
        let in_flight: Arc<Mutex<Option<u64>>> = Arc::new(Mutex::new(None));
        let hook_in_flight = Arc::clone(&in_flight);
        let hook_window = Mutex::new(window.clone());
        let mut kept: Option<Solid> = None;
        let worker = LatestWorker::spawn_with_panic_hook(
            "rough-shape",
            move |request: ShapeRequest| {
                *in_flight.lock().unwrap_or_else(PoisonError::into_inner) = Some(request.revision);
                let (result, solid) = evaluate_reusing(&request, kept.take());
                kept = solid;
                let _ = window.upgrade_in_event_loop(move |_window| {
                    on_host(|host| apply_result(host, result));
                });
                in_flight
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .take();
            },
            move || {
                let revision = hook_in_flight
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .take();
                let Some(revision) = revision else { return };
                let _ = hook_window
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .upgrade_in_event_loop(move |_window| {
                        on_host(|host| apply_internal_failure(host, revision));
                    });
            },
        );
        Self {
            worker: Rc::new(worker),
            debounce: Timer::default(),
            pending: Rc::new(RefCell::new(None)),
            last: RefCell::new(None),
        }
    }

    /// Sends `request` unless it asks for exactly what the previous one did. Returns
    /// whether it was sent (the caller then adopts the request's revision).
    ///
    /// A pebble waits [`PEBBLE_DEBOUNCE`] for a further edit first.
    pub(super) fn submit_changed(&self, request: ShapeRequest) -> bool {
        if self
            .last
            .borrow()
            .as_ref()
            .is_some_and(|last| last.same_inputs(&request))
        {
            return false;
        }
        *self.last.borrow_mut() = Some(request.clone());
        if matches!(request.model.base, RoughBase::Pebble { .. }) {
            *self.pending.borrow_mut() = Some(request);
            let pending = Rc::clone(&self.pending);
            let worker = Rc::clone(&self.worker);
            self.debounce
                .start(TimerMode::SingleShot, PEBBLE_DEBOUNCE, move || {
                    if let Some(request) = pending.borrow_mut().take() {
                        worker.submit(request);
                    }
                });
        } else {
            self.pending.borrow_mut().take();
            self.debounce.stop();
            if !self.worker.submit(request) {
                self.last.borrow_mut().take();
                return false;
            }
        }
        true
    }

    /// Forgets the last request, so the next [`ShapeWorker::submit_changed`] always
    /// sends, and drops a debounced one that has not been sent yet.
    pub(super) fn invalidate(&self) {
        self.last.borrow_mut().take();
        self.pending.borrow_mut().take();
        self.debounce.stop();
    }

    /// Forgets only the last request, so the same inputs can be submitted again. A
    /// debounced request that has not been sent yet stays queued.
    fn forget_last(&self) {
        self.last.borrow_mut().take();
    }
}

/// The row a shape error belongs to, if it names a cut.
#[must_use]
const fn error_row(error: &ShapeError) -> Option<usize> {
    match *error {
        ShapeError::CutNotForBase { index }
        | ShapeError::BadFaces { index }
        | ShapeError::BadSetback { index, .. }
        | ShapeError::BadNormal { index }
        | ShapeError::BadDepth { index, .. } => Some(index),
        ShapeError::NonPositiveSize
        | ShapeError::TooLarge
        | ShapeError::NothingLeft
        | ShapeError::BadInset { .. } => None,
    }
}

/// The "removes 84 mm³ (0.22 ct)" line of the selected cut.
#[must_use]
fn removed_text(removed_mm3: f64, specific_gravity: f64) -> String {
    format!(
        "removes {} mm³ ({:.2} ct)",
        group_thousands(removed_mm3.round() as usize),
        carat_weight(removed_mm3, specific_gravity)
    )
}

/// What the cut rows show after an evaluation.
struct RowNotes {
    /// The row of the cut the error names.
    error_row: Option<usize>,
    /// The error's text.
    error: String,
    /// The row of the selected cut and its removed-volume line.
    removed: Option<(usize, String)>,
}

/// Writes the error and removed-volume lines into the cut rows, leaving alone the rows
/// whose own fields hold text that is not a number (their message is the live one).
fn set_row_notes(model: &RoughPlanModel<'_>, notes: &RowNotes, typing_errors: &BTreeSet<usize>) {
    let rows = model.get_cuts();
    for index in 0..rows.row_count() {
        let Some(mut row) = rows.row_data(index) else {
            continue;
        };
        let error = if notes.error_row == Some(index) {
            notes.error.as_str()
        } else {
            ""
        };
        let removed = notes
            .removed
            .as_ref()
            .filter(|(at, _)| *at == index)
            .map_or("", |(_, text)| text.as_str());
        let error_changed = !typing_errors.contains(&index) && row.error.as_str() != error;
        if error_changed || row.removed_text.as_str() != removed {
            if error_changed {
                row.error = error.into();
            }
            row.removed_text = removed.into();
            rows.set_row_data(index, row);
        }
    }
}

/// The weight check line and level to show: the evaluation's, unless the weighed carat
/// field holds something that is not a positive number, which is reported instead (level
/// 3). The model is evaluated either way; only the check depends on the field.
fn weight_check_lines(weighed_text: &str, check_text: String, check_level: i32) -> (String, i32) {
    match parse_weighed(weighed_text) {
        Err(message) => (message, 3),
        Ok(_) => (check_text, check_level),
    }
}

/// Shows the weight check of the weighed carat field in the window.
pub(super) fn set_weight_check(model: &RoughPlanModel<'_>, check_text: String, check_level: i32) {
    let (text, level) = weight_check_lines(&model.get_weighed_ct(), check_text, check_level);
    model.set_weight_check_text(text.into());
    model.set_weight_check_level(level);
}

/// Applies a finished evaluation on the UI thread: stores the mesh and measurement in
/// the session, fills the readout and the row notes, and tells the view.
fn apply_result(host: &Rc<Host>, result: ShapeResult) {
    let (current, typing_errors) = {
        let session = host.session.borrow();
        let rows: BTreeSet<usize> = session.bad_fields.iter().map(|&(row, _)| row).collect();
        (session.revision == result.revision, rows)
    };
    if !current {
        return;
    }
    let model = host.window.global::<RoughPlanModel>();
    match result.outcome {
        Ok(output) => {
            // The removed volume belongs to the cut the request asked about, which is not
            // necessarily the row that is selected by the time the result arrives.
            let removed = output
                .removed_mm3
                .zip(output.selected_cut)
                .map(|(mm3, row)| (row, removed_text(mm3, output.specific_gravity)));
            super::carat::show(host, &model, &output);
            {
                let mut session = host.session.borrow_mut();
                session.model_mesh = Some(output.mesh);
                session.model_measure = Some(output.measure);
            }
            model.set_model_text(output.model_text.into());
            model.set_model_error("".into());
            let notes = RowNotes {
                error_row: None,
                error: String::new(),
                removed,
            };
            set_row_notes(&model, &notes, &typing_errors);
        }
        Err(failure) => {
            {
                let mut session = host.session.borrow_mut();
                session.model_mesh = None;
                session.model_measure = None;
            }
            model.set_model_text("".into());
            set_weight_check(&model, String::new(), 0);
            let text = failure.message();
            model.set_model_error(text.clone().into());
            let notes = RowNotes {
                error_row: failure.row(),
                error: text,
                removed: None,
            };
            set_row_notes(&model, &notes, &typing_errors);
        }
    }
    view::model_changed(host);
}

/// The worker panicked on the request of `revision`: shows the internal-error message
/// for it (when it is still the current one) and lets the same inputs be submitted again.
fn apply_internal_failure(host: &Rc<Host>, revision: u64) {
    if host.session.borrow().revision == revision {
        host.shape.forget_last();
    }
    let result = ShapeResult {
        revision,
        outcome: Err(ShapeFailure::Internal),
    };
    apply_result(host, result);
}

/// A cut row became the selected one: asks for the figures again, so the "removes ..."
/// line moves to that row. The request differs from the last only in the selected cut, so
/// it is not dropped as a repeat.
pub(super) fn selected_cut_changed(host: &Rc<Host>) {
    super::editing::after_change(host);
}

/// Clears everything the worker fills, for a model that has no size yet.
pub(super) fn show_blank(host: &Rc<Host>) {
    {
        let mut session = host.session.borrow_mut();
        session.model_mesh = None;
        session.model_measure = None;
    }
    let model = host.window.global::<RoughPlanModel>();
    model.set_model_text("".into());
    set_weight_check(&model, String::new(), 0);
    model.set_model_error("".into());
    let notes = RowNotes {
        error_row: None,
        error: String::new(),
        removed: None,
    };
    let typing_errors: BTreeSet<usize> = host
        .session
        .borrow()
        .bad_fields
        .iter()
        .map(|&(row, _)| row)
        .collect();
    set_row_notes(&model, &notes, &typing_errors);
    view::model_changed(host);
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix_cut_core::rough_plan::{BoxFace, RoughCut};

    /// Evaluates `request` from scratch, as the worker does for its first request.
    fn evaluate(request: &ShapeRequest) -> ShapeResult {
        evaluate_reusing(request, None).0
    }

    fn request(model: RoughModel) -> ShapeRequest {
        ShapeRequest {
            revision: 7,
            model,
            specific_gravity: 2.65,
            material_name: "Quartz".to_string(),
            weighed_ct: None,
            selected_cut: None,
        }
    }

    fn block() -> RoughModel {
        RoughModel::new(
            RoughBase::Block {
                x_mm: 10.0,
                y_mm: 8.0,
                z_mm: 6.0,
            },
            Vec::new(),
        )
    }

    /// The block with a corner cut of setbacks 3, 4, 6 mm, which removes a tetrahedron of
    /// 3 * 4 * 6 / 6 = 12 mm^3.
    fn cornered_block() -> RoughModel {
        let mut model = block();
        model.cuts.push(RoughCut::Corner {
            faces: [BoxFace::Top, BoxFace::Front, BoxFace::Right],
            setbacks_mm: [4.0, 6.0, 3.0],
        });
        model
    }

    #[test]
    fn a_plain_block_has_its_box_volume_and_a_centred_mesh() {
        let result = evaluate(&request(block()));
        assert_eq!(result.revision, 7);
        let output = result.outcome.expect("a plain block is valid");
        assert!((output.measure.volume_mm3 - 480.0).abs() < 1e-6);
        assert!(
            (output.specific_gravity - 2.65).abs() < 1e-12,
            "the gravity is carried for the removed-volume text"
        );
        assert!(
            output.model_text.contains("480 mm³"),
            "{}",
            output.model_text
        );
        assert!(
            output.model_text.contains("Quartz"),
            "{}",
            output.model_text
        );
        assert_eq!(output.check_level, 0);

        let mut low = DVec3::splat(f64::INFINITY);
        let mut high = DVec3::splat(f64::NEG_INFINITY);
        for &p in &output.mesh.positions {
            low = low.min(p);
            high = high.max(p);
        }
        assert!((low + high).length() < 1e-9, "the box centre is the origin");
        assert!((high - low - DVec3::new(10.0, 8.0, 6.0)).length() < 1e-9);
    }

    #[test]
    fn the_selected_cut_reports_the_volume_it_removes() {
        let mut ask = request(cornered_block());
        ask.selected_cut = Some(0);
        let output = evaluate(&ask).outcome.expect("valid corner cut");
        assert!((output.measure.volume_mm3 - 468.0).abs() < 1e-6);
        let removed = output.removed_mm3.expect("the selected cut has a volume");
        assert!((removed - 12.0).abs() < 1e-6, "removed {removed}");
        assert_eq!(output.selected_cut, Some(0), "the result names its cut");

        ask.selected_cut = Some(5);
        assert!(
            evaluate(&ask)
                .outcome
                .expect("still valid")
                .removed_mm3
                .is_none()
        );
        ask.selected_cut = None;
        let output = evaluate(&ask).outcome.expect("still valid");
        assert!(output.removed_mm3.is_none());
        assert_eq!(output.selected_cut, None);
    }

    #[test]
    fn a_request_that_only_changes_the_cut_reuses_the_mesh_and_measure() {
        let first = request(cornered_block());
        let (first_result, kept) = evaluate_reusing(&first, None);
        let first_output = first_result.outcome.expect("valid");
        assert!(first_output.removed_mm3.is_none());

        let mut second = first;
        second.revision = 8;
        second.selected_cut = Some(0);
        let (second_result, kept) = evaluate_reusing(&second, kept);
        let second_output = second_result.outcome.expect("valid");
        assert!(
            Arc::ptr_eq(&first_output.mesh, &second_output.mesh),
            "the same model keeps its mesh"
        );
        assert_eq!(first_output.measure, second_output.measure);
        let removed = second_output.removed_mm3.expect("the cut has a volume");
        assert!((removed - 12.0).abs() < 1e-6, "removed {removed}");

        // The figure is remembered with the cut it was measured for.
        let kept = kept.expect("the solid is kept");
        assert_eq!(kept.removed, Some((0, second_output.removed_mm3)));

        // A different model measures and meshes afresh.
        let mut third = second;
        third.model = block();
        third.selected_cut = None;
        let (third_result, _) = evaluate_reusing(&third, Some(kept));
        let third_output = third_result.outcome.expect("valid");
        assert!(!Arc::ptr_eq(&second_output.mesh, &third_output.mesh));
        assert!((third_output.measure.volume_mm3 - 480.0).abs() < 1e-6);
    }

    #[test]
    fn a_bad_cut_is_reported_with_its_row() {
        let mut model = block();
        model.cuts.push(RoughCut::Edge {
            faces: [BoxFace::Top, BoxFace::Front],
            setbacks_mm: [1.0, 1.0],
        });
        model.cuts.push(RoughCut::Edge {
            faces: [BoxFace::Top, BoxFace::Front],
            setbacks_mm: [-1.0, 1.0],
        });
        let (result, kept) = evaluate_reusing(&request(model), None);
        let failure = result.outcome.expect_err("negative setback");
        assert!(kept.is_none(), "an invalid model keeps nothing");
        assert!(
            matches!(
                failure,
                ShapeFailure::Invalid(ShapeError::BadSetback { index: 1, .. })
            ),
            "{failure:?}"
        );
        assert_eq!(failure.row(), Some(1));
        assert_eq!(error_row(&ShapeError::NothingLeft), None);
    }

    #[test]
    fn an_internal_failure_names_no_row_and_tells_the_user_to_edit() {
        let failure = ShapeFailure::Internal;
        assert_eq!(failure.row(), None);
        assert_eq!(
            failure.message(),
            "The model could not be evaluated (internal error); edit the model to retry."
        );
        assert_eq!(
            ShapeFailure::Invalid(ShapeError::NothingLeft).message(),
            ShapeError::NothingLeft.to_string()
        );
    }

    #[test]
    fn the_weight_check_uses_the_weighed_carat() {
        let mut ask = request(block());
        // 480 mm^3 * 2.65 / 200 = 6.36 ct.
        ask.weighed_ct = Some(6.4);
        let output = evaluate(&ask).outcome.expect("valid");
        assert_eq!(output.check_level, 1);
        ask.weighed_ct = Some(3.0);
        assert_eq!(evaluate(&ask).outcome.expect("valid").check_level, 3);
    }

    #[test]
    fn without_a_material_the_model_is_not_accused_of_the_wrong_weight() {
        let mut ask = request(block());
        ask.specific_gravity = 0.0;
        ask.material_name = String::new();
        ask.weighed_ct = Some(6.4);
        let output = evaluate(&ask).outcome.expect("valid");
        assert_eq!(output.check_level, 0);
        assert!(output.check_text.is_empty(), "{}", output.check_text);
        assert_eq!(output.model_text, "Model 480 mm³");
    }

    #[test]
    fn a_bad_weighed_field_replaces_only_the_weight_check_line() {
        // A valid or empty field leaves the evaluation's line alone.
        assert_eq!(
            weight_check_lines("", "close".to_string(), 2),
            ("close".to_string(), 2)
        );
        assert_eq!(
            weight_check_lines("8,9", "matches".to_string(), 1),
            ("matches".to_string(), 1)
        );
        // Anything else is reported at the "check the model" level, whatever the model says.
        for typed in ["abc", "0", "-1", "inf"] {
            let (text, level) = weight_check_lines(typed, "matches".to_string(), 1);
            assert_eq!(level, 3, "{typed}");
            assert!(text.contains("Weighed carat"), "{typed}: {text}");
        }
        // The model is still evaluated with such a field: the request carries no weighed
        // carat and a block's figures come out as usual.
        let output = evaluate(&request(block())).outcome.expect("valid block");
        assert_eq!(output.check_level, 0);
        assert!(output.measure.volume_mm3 > 0.0);
    }

    #[test]
    fn requests_that_differ_only_in_the_revision_are_the_same_inputs() {
        let a = request(block());
        let mut b = a.clone();
        b.revision += 1;
        assert!(a.same_inputs(&b));
        b.selected_cut = Some(0);
        assert!(!a.same_inputs(&b));
        let mut c = a.clone();
        c.weighed_ct = Some(1.0);
        assert!(!a.same_inputs(&c));
    }

    #[test]
    fn the_removed_line_names_volume_and_carats() {
        // 84 * 2.6 / 200 = 1.092 ct.
        assert_eq!(removed_text(84.0, 2.6), "removes 84 mm³ (1.09 ct)");
    }
}
