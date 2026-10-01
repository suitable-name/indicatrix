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
        commit_pending_base, input_error, push_history_flags, read_inputs, readout_inputs_changed,
        resync,
    },
    host::Host,
    inputs::parse_weighed,
    shape_worker::{ShapeOutput, set_weight_check},
};
use crate::RoughPlanModel;
use indicatrix_cut_core::yield_metrics::carat_weight;
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

/// The Fit to weight button: scales every length of the model so that its carat equals
/// the typed weight, as one undo step. A typed weight that already matches the model
/// within a hundredth of a percent only hands the field back to the model.
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
    let scaled = host.session.borrow().model.scaled(factor);
    if let Err(error) = scaled.base.validate() {
        input_error(host, &error.to_string());
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
