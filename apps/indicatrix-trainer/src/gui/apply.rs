//! Worker snapshots and metric points written into the Slint controller global.
//!
//! The snapshot is split by what it describes: the run state (status, buttons), the setup (the
//! controls of the left panel, which always show the live run) and the display (counters, optics
//! and validation, which an imported run replaces while it is viewed).

use super::charts::ratio_of;
use crate::{
    StoneMetrics, TrainingController,
    stats::IterationMetricPoint,
    ui_state::{OVERFIT_GAP_WARN, ProgressSnapshot, RATIO_GOAL, ValidationSnapshot},
};

/// `usize` counter to a Slint `int`, saturating instead of wrapping.
pub fn clamp_i32(value: usize) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}

/// The status pill's text: the worker's own, unless it ended or is shutting down.
pub const fn status_label(
    worker_status: &'static str,
    worker_alive: bool,
    stopping: bool,
) -> &'static str {
    if !worker_alive {
        "Stopped"
    } else if stopping {
        "Stopping"
    } else {
        worker_status
    }
}

/// The constants the UI needs once.
pub fn apply_constants(c: &TrainingController<'_>) {
    c.set_gap_warn_threshold(OVERFIT_GAP_WARN);
    c.set_ratio_goal(RATIO_GOAL);
}

/// Status pill, run buttons and the unsaved-weights flag.
pub fn apply_run_state(
    c: &TrainingController<'_>,
    s: &ProgressSnapshot,
    worker_alive: bool,
    stopping: bool,
) {
    c.set_is_training(s.running && worker_alive && !stopping);
    c.set_status(status_label(s.status_text(), worker_alive, stopping).into());
    c.set_weights_dirty(s.weights_dirty);
}

/// The setup panel's mirror of the live configuration.
pub fn apply_setup(c: &TrainingController<'_>, s: &ProgressSnapshot) {
    c.set_model_mode(s.mode_index());
    c.set_model_tier_name(s.tier.code_str().into());
    c.set_is_corpus_mode(s.is_corpus_mode);
    c.set_is_universal_spectrum(s.is_universal_spectrum);
    c.set_train_designs_count(clamp_i32(s.train_count));
    c.set_val_designs_count(clamp_i32(s.val_count));
    c.set_test_designs_count(clamp_i32(s.test_count));
    c.set_tier_count(clamp_i32(s.tier_count));
    c.set_source_material(s.src_material_name.as_str().into());
    c.set_source_ri(s.src_n_d);
    c.set_target_material(s.tgt_material_name.as_str().into());
    c.set_target_ri(s.tgt_n_d);
    c.set_learning_rate(s.learning_rate);
    c.set_rollout_size(clamp_i32(s.rollout_size));
    c.set_batch_size(clamp_i32(s.batch_size));
    c.set_epochs(clamp_i32(s.ppo_epochs));
    c.set_updates_per_round(clamp_i32(s.updates_per_round));
    c.set_worker_count(clamp_i32(s.worker_count));
}

/// Counters, learning figures, validation and optics of the live run.
pub fn apply_display(c: &TrainingController<'_>, s: &ProgressSnapshot) {
    c.set_has_metrics(true);
    c.set_iteration(clamp_i32(s.iteration));
    c.set_episode(clamp_i32(s.episode));
    c.set_total_steps(clamp_i32(s.total_steps));
    c.set_steps_per_sec(s.steps_per_sec);
    c.set_mean_return(s.mean_return);
    c.set_has_best(s.best_score.is_some());
    c.set_best_score(s.best_score.unwrap_or_default());

    // No validation pass yet: the flags stay false and the cards show a dash, not a zero score.
    c.set_has_validation(s.validation.is_some());
    c.set_has_gap(s.validation.is_some_and(|v| v.gap.is_some()));
    let validation = s.validation.unwrap_or(ValidationSnapshot {
        score: 0.0,
        brilliance: 0.0,
        windowing: 0.0,
        gap: None,
    });
    c.set_val_score(validation.score);
    c.set_val_brilliance(validation.brilliance);
    c.set_val_windowing(validation.windowing);
    c.set_generalization_gap(validation.gap.unwrap_or_default());

    // The optics exist once a stone is on display.
    c.set_has_optics(!s.display_design_title.is_empty());
    c.set_display_design_title(s.display_design_title.as_str().into());
    c.set_solid_closed(s.solid_closed);
    c.set_baseline_metrics(StoneMetrics {
        brilliance: s.baseline_brilliance,
        windowing: s.baseline_windowing,
        extinction: s.baseline_extinction,
        yield_pct: s.baseline_yield_pct,
    });
    c.set_current_metrics(StoneMetrics {
        brilliance: s.brilliance,
        windowing: s.windowing,
        extinction: s.extinction,
        yield_pct: s.yield_pct,
    });
}

/// The figures that come from the latest metric point; none given means "not measured".
pub fn apply_point(c: &TrainingController<'_>, point: Option<&IterationMetricPoint>) {
    let loss = point
        .map(|p| p.imitation_loss)
        .filter(|loss| loss.is_finite() && *loss > 0.0);
    c.set_has_loss(loss.is_some());
    c.set_imitation_loss(loss.unwrap_or_default());
    let cosine = point.and_then(|p| p.train_cosine);
    c.set_has_cosine(cosine.is_some());
    c.set_train_cosine(cosine.unwrap_or_default());
    c.set_labelled_samples(point.map_or(0, |p| clamp_i32(p.labelled_samples)));
    c.set_accepted_pct(point.map_or(0.0, |p| p.accepted_frac * 100.0));
    let evals = point.and_then(IterationMetricPoint::evaluations_per_sec);
    c.set_has_evals_per_sec(evals.is_some());
    c.set_evals_per_sec(evals.unwrap_or_default());
    let ratio = point.and_then(ratio_of);
    c.set_has_ratio(ratio.is_some());
    c.set_val_oracle_ratio(ratio.unwrap_or_default());
}

/// Back to "nothing measured yet" (a worker restart, leaving an import without a live run).
pub fn clear_display(c: &TrainingController<'_>) {
    apply_display(c, &ProgressSnapshot::default());
    c.set_has_metrics(false);
    c.set_has_optics(false);
    c.set_display_design_title("".into());
    apply_point(c, None);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_saturate_instead_of_wrapping() {
        assert_eq!(clamp_i32(7), 7);
        assert_eq!(clamp_i32(usize::MAX), i32::MAX);
    }

    #[test]
    fn a_dead_worker_reads_stopped_whatever_it_last_said() {
        assert_eq!(status_label("Training", false, false), "Stopped");
        assert_eq!(status_label("Training", false, true), "Stopped");
    }

    #[test]
    fn shutting_down_reads_stopping() {
        assert_eq!(status_label("Training", true, true), "Stopping");
    }

    #[test]
    fn a_live_worker_keeps_its_own_status() {
        assert_eq!(status_label("Paused", true, false), "Paused");
        assert_eq!(status_label("Idle", true, false), "Idle");
    }
}
