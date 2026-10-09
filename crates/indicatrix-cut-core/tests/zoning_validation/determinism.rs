//! Determinism (plan section 10.1): the records and the fit are bitwise identical for 1, 4 and 16
//! threads, and run to run.
//!
//! The scene exercises the parts that could reorder floating point work: a frosted skin with
//! polished windows (the microfacet random numbers), zones (the kernels), several views and
//! chunks, the multi-start fit, the held-out refits and the residual maps.

use std::sync::atomic::AtomicBool;

use indicatrix_cut_core::rough_plan::colour_fit::{
    forward::{ForwardRecords, evaluate},
    solve::{ColourFit, fit_records},
};

use crate::synth::{
    ColourCase, NoiseSpec, RoughKind, Setup, SetupSpec, photos_forward, quick_fit_config,
};

fn run(threads: usize) -> (ForwardRecords, Vec<Vec<[u32; 3]>>, ColourFit) {
    let setup = Setup::new(&SetupSpec {
        rough: RoughKind::PolishedWindows,
        colour: ColourCase::Bicolour,
        views: 3,
        grid_px: 8,
        samples: 32,
        threads,
        ..SetupSpec::default()
    });
    let records = setup.trace();
    let truth = &setup.truth;
    let alpha = |zone: usize, lambda: f64| truth.alpha(zone, lambda);
    // The predictions, as bit patterns, so that a NaN or a -0.0 cannot hide a difference.
    let predictions = evaluate(&records, &alpha)
        .iter()
        .map(|v| {
            v.rgb
                .iter()
                .map(|px| [px[0].to_bits(), px[1].to_bits(), px[2].to_bits()])
                .collect()
        })
        .collect();
    let photos = photos_forward(&records, &setup.truth, &NoiseSpec::RAW, 5);
    let fit = fit_records(
        &records,
        &photos,
        &quick_fit_config(threads),
        &AtomicBool::new(false),
        &mut |_| {},
    )
    .expect("the fit runs");
    (records, predictions, fit)
}

#[test]
fn records_and_fit_are_bitwise_identical_for_1_4_and_16_threads_and_run_to_run() {
    let (records, predictions, fit) = run(1);
    for threads in [4, 16, 1] {
        let (other_records, other_predictions, other_fit) = run(threads);
        assert!(
            records == other_records,
            "the path records differ between 1 and {threads} threads"
        );
        assert_eq!(
            predictions, other_predictions,
            "the predictions differ between 1 and {threads} threads"
        );
        assert!(
            fit == other_fit,
            "the fit differs between 1 and {threads} threads"
        );
    }
}
