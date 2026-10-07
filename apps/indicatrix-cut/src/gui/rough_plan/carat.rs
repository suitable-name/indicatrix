//! The carat field: the model's own weight until a weighed carat is typed over it, and
//! the Fit to weight action that scales the model to the typed weight.
//!
//! The field follows the model (every size, cut or material change rewrites it with the
//! model's carat for the picked material) as long as it holds the text the window wrote
//! last. Text typed over it is a weighed carat: the field keeps it, the weight check
//! compares it with the model, and Fit to weight scales every length of the model by the
//! cube root of the carat ratio (one undo step), after which the field follows the model
//! again.

use super::{
    editing::{
        commit_pending_base, push_history_flags, read_inputs, readout_inputs_changed, resync,
    },
    host::Host,
    inputs::{FIT_FAILED_PREFIX, MODEL_CHANGED_MESSAGE, parse_weighed},
    mesh_task,
    saved::show_error,
    shape_worker::{ShapeOutput, set_weight_check},
};
use crate::RoughPlanModel;
use indicatrix_cut_core::{rough_plan::RoughModel, yield_metrics::carat_weight};
use slint::ComponentHandle;
use std::rc::Rc;

/// Whether the carat field holds a typed weight rather than the carat the window wrote.
fn is_typed(host: &Rc<Host>, text: &str) -> bool {
    !text.trim().is_empty() && host.session.borrow().carat_shown.as_deref() != Some(text)
}

/// The weighed carat typed into the carat field: `None` while the field holds the
/// model's own carat (or nothing).
///
/// # Errors
///
/// Returns the message for a typed text that is not a positive number.
pub(super) fn typed_carat(host: &Rc<Host>) -> Result<Option<f64>, String> {
    let text = host.window.global::<RoughPlanModel>().get_weighed_ct();
    if is_typed(host, text.as_str()) {
        parse_weighed(&text)
    } else {
        Ok(None)
    }
}

/// The uniform scale that takes a model of `current` carat to `target` carat: the cube
/// root of their ratio. `None` when either figure is not positive, or when the ratio is
/// within a hundredth of a percent of 1 (nothing to fit).
pub(super) fn fit_factor(current: f64, target: f64) -> Option<f64> {
    if !(current > 0.0 && target > 0.0) {
        return None;
    }
    let factor = (target / current).cbrt();
    (factor.is_finite() && (factor - 1.0).abs() >= 1e-4).then_some(factor)
}

/// The carat text the window writes for a model of `volume_mm3` and `specific_gravity`.
fn model_carat_text(volume_mm3: f64, specific_gravity: f64) -> String {
    format!("{:.2}", carat_weight(volume_mm3, specific_gravity))
}

/// Shows the carat field and the weight check after an evaluation: a typed weight stays,
/// with the check against the model; otherwise the field is rewritten with the model's
/// carat and no check is shown.
pub(super) fn show(host: &Rc<Host>, model: &RoughPlanModel<'_>, output: &ShapeOutput) {
    let text = model.get_weighed_ct();
    if is_typed(host, text.as_str()) {
        set_weight_check(model, output.check_text.clone(), output.check_level);
        return;
    }
    if output.specific_gravity > 0.0 {
        let carat = model_carat_text(output.measure.volume_mm3, output.specific_gravity);
        model.set_weighed_ct(carat.clone().into());
        host.session.borrow_mut().carat_shown = Some(carat);
    }
    set_weight_check(model, String::new(), 0);
}

/// `model` with every length multiplied by `factor`, when the scaled rough is still one the
/// planner takes. Scaling a mesh rough rebuilds its mesh and registers its hull again, so
/// this is the slow part of Fit to weight.
///
/// # Errors
///
/// Returns the message for a scaled rough that is too large (or not positive).
fn scaled_model(model: &RoughModel, factor: f64) -> Result<RoughModel, String> {
    let scaled = model.scaled(factor);
    scaled.base.validate().map_err(|e| e.to_string())?;
    Ok(scaled)
}

/// The error-line text for a Fit to weight that could not scale the model, naming the
/// reason (a scaled rough over the size limit, a failed rebuild of a mesh).
fn failure_text(reason: &str) -> String {
    format!("{FIT_FAILED_PREFIX} {reason}")
}

/// Makes `scaled` the session's model, as one undo step, when the model is still
/// `original`: the weighed carat `text` was fitted for that model, and a model that was
/// edited meanwhile (a mesh is scaled in the background) is left as the user made it.
///
/// A refusal shows on the window's error line, like a failed OBJ import, and not as a model
/// error: the model on screen is unchanged and valid, so Plan must stay available (a model
/// error keeps Plan off until the next edit).
fn apply_fitted(host: &Rc<Host>, original: &RoughModel, scaled: RoughModel, text: String) {
    if host.session.borrow().model != *original {
        show_error(host, MODEL_CHANGED_MESSAGE);
        return;
    }
    let recorded = {
        let mut session = host.session.borrow_mut();
        session.model = scaled;
        session.carat_shown = Some(text);
        session.commit()
    };
    if recorded {
        push_history_flags(host);
    }
    resync(host, true);
}

/// The Fit to weight button: scales every length of the model so that its carat equals
/// the typed weight, as one undo step. A typed weight that already matches the model
/// within a hundredth of a percent only hands the field back to the model.
///
/// A mesh rough is scaled on a worker thread (see [`mesh_task`]); every other rough is
/// scaled at once.
pub(super) fn fit_to_weight(host: &Rc<Host>) {
    if commit_pending_base(host).is_err() {
        return;
    }
    let Ok(Some(target)) = typed_carat(host) else {
        return;
    };
    let Some(measure) = host.session.borrow().model_measure else {
        return;
    };
    let text = host
        .window
        .global::<RoughPlanModel>()
        .get_weighed_ct()
        .to_string();
    let current = carat_weight(measure.volume_mm3, read_inputs(host).specific_gravity);
    let Some(factor) = fit_factor(current, target) else {
        host.session.borrow_mut().carat_shown = Some(text);
        readout_inputs_changed(host);
        return;
    };
    // A new attempt clears the last failure, as the OBJ import does.
    show_error(host, "");
    let original = host.session.borrow().model.clone();
    if original.mesh().is_none() {
        match scaled_model(&original, factor) {
            Ok(scaled) => apply_fitted(host, &original, scaled, text),
            Err(message) => show_error(host, &failure_text(&message)),
        }
        return;
    }
    let job_model = original.clone();
    mesh_task::spawn(
        host,
        "Scaling the mesh...",
        move || scaled_model(&job_model, factor),
        move |host, result| match result {
            Ok(scaled) => apply_fitted(host, &original, scaled, text),
            Err(message) => show_error(host, &failure_text(&message)),
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::rough_plan::inputs::is_input_message;
    use indicatrix_cut_core::rough_plan::{RoughBase, RoughCut};

    #[test]
    fn the_fit_factor_is_the_cube_root_of_the_carat_ratio() {
        let factor = fit_factor(3.40, 3.80).expect("a change");
        assert!((factor - (3.80f64 / 3.40).cbrt()).abs() < 1e-12, "{factor}");
        assert!(factor.powi(3).mul_add(3.40, -3.80).abs() < 1e-12);
        let shrink = fit_factor(10.0, 1.25).expect("a change");
        assert!((shrink - 0.5).abs() < 1e-12, "{shrink}");
    }

    #[test]
    fn a_matching_or_impossible_weight_gives_no_factor() {
        assert_eq!(fit_factor(3.40, 3.40), None);
        assert_eq!(fit_factor(3.40, 3.40 * (1.0 + 2e-4)), None);
        assert_eq!(fit_factor(0.0, 3.40), None);
        assert_eq!(fit_factor(3.40, 0.0), None);
        assert_eq!(fit_factor(3.40, -1.0), None);
        assert_eq!(fit_factor(f64::NAN, 3.40), None);
    }

    #[test]
    fn the_written_carat_has_two_decimals() {
        assert_eq!(model_carat_text(256.6, 2.65), "3.40");
        assert_eq!(model_carat_text(0.0, 2.65), "0.00");
    }

    fn block_with_a_face_cut(x_mm: f64) -> RoughModel {
        RoughModel::new(
            RoughBase::Block {
                x_mm,
                y_mm: 8.0,
                z_mm: 6.0,
            },
            vec![RoughCut::Face {
                normal: [0.0, 1.0, 0.0],
                depth_mm: 1.0,
            }],
        )
    }

    #[test]
    fn scaling_a_model_multiplies_the_base_and_every_cut_length() {
        let scaled = scaled_model(&block_with_a_face_cut(10.0), 2.0).expect("a valid rough");
        assert_eq!(
            scaled.base,
            RoughBase::Block {
                x_mm: 20.0,
                y_mm: 16.0,
                z_mm: 12.0,
            }
        );
        assert_eq!(
            scaled.cuts,
            vec![RoughCut::Face {
                normal: [0.0, 1.0, 0.0],
                depth_mm: 2.0,
            }]
        );
    }

    #[test]
    fn a_scaled_rough_the_planner_cannot_take_is_refused_with_its_message() {
        let message = scaled_model(&block_with_a_face_cut(1500.0), 2.0)
            .expect_err("3000 mm is over the limit");
        assert_eq!(message, "Dimensions must not exceed 2000 mm.");
    }

    #[test]
    fn a_failed_fit_names_its_reason_on_the_error_line() {
        // The text goes to the window's error line through `show_error`: a model error would
        // keep Plan off although the model is unchanged and valid. The line is cleared by
        // the next edit, because both texts count as input messages.
        let message = scaled_model(&block_with_a_face_cut(1500.0), 2.0)
            .expect_err("3000 mm is over the limit");
        let text = failure_text(&message);
        assert_eq!(
            text,
            "Fit to weight failed: Dimensions must not exceed 2000 mm."
        );
        assert!(is_input_message(&text));
        assert!(is_input_message(MODEL_CHANGED_MESSAGE));
        assert!(MODEL_CHANGED_MESSAGE.ends_with("Press Fit to weight again."));
    }

    #[test]
    fn a_mesh_rough_is_scaled_with_its_mesh_by_the_job_the_worker_runs() {
        use crate::gui::rough_plan::obj_import::{C_SHAPE_OBJ, mesh_base_of};
        let model = RoughModel::new(mesh_base_of(C_SHAPE_OBJ), Vec::new());
        assert!(model.mesh().is_some(), "the C-shape is a mesh rough");
        let scaled = scaled_model(&model, 2.0).expect("a mesh scales");
        let mesh = scaled.mesh().expect("the scaled rough still has its mesh");
        // 6000 mm^3 of material, every length doubled.
        assert!((mesh.volume() - 48_000.0).abs() < 1e-6, "{}", mesh.volume());
    }
}
