//! Viewing an imported run next to the live one.
//!
//! While an import is on screen the counters, optics, validation figures and charts show the
//! imported run; the setup panel and the run buttons keep following the live worker, and live
//! snapshots do not touch the display. "Return to Live" restores the latest live data.

use std::path::Path;

use super::{
    Gui,
    apply::{apply_display, apply_point, clamp_i32, clear_display},
};
use crate::{StoneMetrics, TrainingController, stats::TrainingRunStats};

/// One line describing the imported run.
pub fn import_label(file_name: &str, mode: &str, tier: &str, iterations: usize) -> String {
    format!("Imported Run: {file_name} — {mode}, tier {tier}, {iterations} iterations")
}

/// Shows an imported run's figures in the display properties.
fn apply_import(c: &TrainingController<'_>, run: &TrainingRunStats) {
    c.set_has_metrics(true);
    c.set_iteration(clamp_i32(run.total_iterations));
    c.set_episode(clamp_i32(run.total_episodes));
    c.set_total_steps(clamp_i32(run.total_steps));
    c.set_mean_return(run.final_return);
    // "None yet" is a missing value in the file, never a zero: the flags follow the options.
    c.set_has_best(run.best_score.is_some());
    c.set_best_score(run.best_score.unwrap_or_default());
    c.set_has_validation(run.val_score.is_some());
    c.set_val_score(run.val_score.unwrap_or_default());
    c.set_has_gap(run.generalization_gap.is_some());
    c.set_generalization_gap(run.generalization_gap.unwrap_or_default());
    c.set_has_optics(true);
    c.set_display_design_title(run.active_design_title.as_str().into());
    c.set_baseline_metrics(StoneMetrics {
        brilliance: run.baseline_brilliance,
        windowing: run.baseline_windowing,
        extinction: run.baseline_extinction,
        yield_pct: run.baseline_yield_pct,
    });
    c.set_current_metrics(StoneMetrics {
        brilliance: run.final_brilliance,
        windowing: run.final_windowing,
        extinction: run.final_extinction,
        yield_pct: run.final_yield_pct,
    });
    let last = run.metrics_history.last();
    c.set_solid_closed(last.is_none_or(|p| p.solid_closed));
    c.set_steps_per_sec(last.map_or(0.0, |p| p.steps_per_sec));
    c.set_val_brilliance(last.and_then(|p| p.val_brilliance).unwrap_or_default());
    c.set_val_windowing(last.and_then(|p| p.val_windowing).unwrap_or_default());
    apply_point(c, last);
}

impl Gui {
    /// A run file was read on the background thread.
    pub(super) fn finish_import(&self, path: &Path, result: Result<Box<TrainingRunStats>, String>) {
        self.with_c(|c| c.set_busy_text("".into()));
        match result {
            Err(reason) => {
                let name = path.display();
                self.log("ERROR", format!("Failed to import '{name}': {reason}"));
            }
            Ok(run) => self.show_import(path, &run),
        }
    }

    fn show_import(&self, path: &Path, run: &TrainingRunStats) {
        let file_name = path.file_name().map_or_else(
            || path.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        let label = import_label(&file_name, &run.mode, &run.model_tier, run.total_iterations);
        self.charts
            .borrow_mut()
            .set_imported(Some(run.metrics_history.clone()));
        self.charts_dirty.set(true);
        self.with_c(|c| {
            apply_import(c, run);
            c.set_import_label(label.as_str().into());
            c.set_viewing_import(true);
        });
        self.log(
            "IMPORT",
            format!(
                "{label}; best score {}",
                run.best_score
                    .map_or_else(|| "—".to_owned(), |b| format!("{b:.1}"))
            ),
        );
    }

    /// Leaves the import view and restores the latest live data.
    pub(super) fn return_to_live(&self) {
        self.charts.borrow_mut().set_imported(None);
        self.charts_dirty.set(true);
        let live = self.live.borrow();
        self.with_c(|c| {
            c.set_viewing_import(false);
            c.set_import_label("".into());
            match &live.snapshot {
                Some(snapshot) => {
                    apply_display(c, snapshot);
                    apply_point(c, live.point.as_ref());
                }
                None => clear_display(c),
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_label_names_file_mode_tier_and_length() {
        assert_eq!(
            import_label("run.json", "Retarget", "M", 120),
            "Imported Run: run.json — Retarget, tier M, 120 iterations"
        );
    }
}
