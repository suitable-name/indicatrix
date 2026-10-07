//! The verdict's worker: the geometry checks and the optical measurement, off the UI thread.
//!
//! [`compute`] is the whole job and is pure (a design, its masts and a material in, the
//! verdict's inputs out), so it is what the tests call. [`spawn`] runs it on a thread and
//! hands the result back to [`super::finish`] on the UI thread.
//!
//! The job never solves: the masts come from the solve the caller already has. What it does
//! is geometry (the planes, the solid mesh, the manufacturability checks, a few milliseconds)
//! and, when the design names a material, one fast table-up optical measurement (about 2 ms,
//! the same one the Retarget dialog's table uses).

use super::finish;
use crate::MainWindow;
use indicatrix::{
    geometry::meet_solver::SolvedTier,
    optics::{LightingPreset, materials::GemMaterial},
};
use indicatrix_cut_core::Design;
use indicatrix_editor::{
    material_lookup::{EditorMaterialLookup, resolved_gem_material},
    retarget::metrics::measure_column,
    verdict::{SolveFacts, VerdictInputs, gather},
};
use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::Arc,
};

/// Everything a worker needs, bundled so the debounce timer can hold it until it fires.
pub(super) struct Job {
    /// A copy of the design the verdict is about.
    pub(super) design: Design,
    /// The masts of its solve, or `None` when it did not solve.
    pub(super) solved: Option<Vec<SolvedTier>>,
    /// The design's effective refractive index (what the tier table's margins use).
    pub(super) n_d: f64,
    /// The lighting the viewport uses, for the optical measurement.
    pub(super) lighting: LightingPreset,
    /// The custom materials, to find the design's material; `None` skips the measurement.
    pub(super) custom: Option<Arc<Vec<GemMaterial>>>,
    /// The request this job answers; a newer request makes the result a stale one.
    pub(super) serial: u64,
    /// The window to report to.
    pub(super) ui_weak: slint::Weak<MainWindow>,
}

/// The design's material as the tracer sees it, when the design names one.
///
/// A preset or a refractive-index override counts. `None` for a design with no material set:
/// there is nothing to measure then, and the verdict is worked out without the optical figures.
fn design_gem(design: &Design, custom: &[GemMaterial]) -> Option<GemMaterial> {
    let material = &design.material;
    (material.name.is_some() || material.refractive_index_override.is_some())
        .then(|| resolved_gem_material(material, &EditorMaterialLookup::new(custom)))
}

/// The verdict's inputs for `design`: the geometry facts of [`gather`] and, when the stone
/// closes and the design names a material, the optical figures.
///
/// `solved` is the mast list of the design's solve (`None` when it did not solve), `n_d` its
/// effective refractive index and `custom` the custom materials to look the design's material
/// up in (`None` skips the measurement).
pub(super) fn compute(
    design: &Design,
    solved: Option<&[SolvedTier]>,
    n_d: f64,
    lighting: LightingPreset,
    custom: Option<&[GemMaterial]>,
) -> VerdictInputs {
    let inputs = gather(design, solved, n_d, None);
    if inputs.solve != SolveFacts::Closed {
        return inputs;
    }
    let (Some(solved), Some(custom)) = (solved, custom) else {
        return inputs;
    };
    let Some(gem) = design_gem(design, custom) else {
        return inputs;
    };
    // `Closed` means `gather` accepted the mast list as one entry per tier.
    let planes = design.planes_from_solved(solved);
    let column = measure_column(&planes, &gem, lighting);
    inputs.with_optics(Some(column))
}

/// Runs `job` on a worker thread; the result comes back through [`finish`] on the UI thread.
pub(super) fn spawn(job: Job) {
    std::thread::spawn(move || {
        let Job {
            design,
            solved,
            n_d,
            lighting,
            custom,
            serial,
            ui_weak,
        } = job;
        let had_solution = solved.is_some();
        // A panic inside the geometry must not leave the badge on "out of date" for good; the
        // UI thread keeps what it showed.
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            compute(
                &design,
                solved.as_deref(),
                n_d,
                lighting,
                custom.as_deref().map(Vec::as_slice),
            )
        }));
        let _ = ui_weak.upgrade_in_event_loop(move |ui| {
            finish(&ui, serial, had_solution, outcome.ok());
        });
    });
}
