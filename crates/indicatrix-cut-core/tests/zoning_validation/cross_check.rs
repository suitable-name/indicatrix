//! Forward model versus the main CPU tracer (plan section 10.1, "cross-check"): the same cube,
//! the same zoned absorption, the same uniform white furnace, the mean relative radiance within
//! 1 percent. This catches drift between the two tracers' twins (Fresnel, path lengths per zone,
//! the unit chain from model units to millimetres, the zone kernels).
//!
//! The absorption is wavelength independent (a very wide band) so that the different spectral
//! integration of the two (camera channels against CIE functions) does not enter; the camera and
//! the light spectrum are matched anyway. Only the unit cube is compared: the main tracer works on
//! facet planes, in air, polished, so it cannot express the irregular, frosted, holder and
//! immersion roughs (those are covered by the forward model alone).

use crate::synth::{
    Agreement, ColourCase, Truth, camera_furnace_rgb, furnace_agreement, furnace_agreement_rgb,
    furnace_camera_rgb,
};

/// Section 10.1: the mean relative radiance difference.
const LIMIT: f64 = 0.01;

/// The starting samples of the forward model per pixel and of the main tracer per working pixel.
const START_SAMPLES: (usize, u32) = (256, 400);
/// The forward samples at which the retries stop (each retry has four times the samples of the
/// last one, for both tracers).
const MAX_FORWARD_SAMPLES: usize = 4096;

/// Round 3, D5: a miss of `LIMIT` that is within two combined Monte-Carlo standard errors is
/// noise, not bias, and is settled by tracing more paths (up to `MAX_FORWARD_SAMPLES`); a miss
/// beyond two standard errors is a bias and is reported as it is.
fn needs_more_samples(a: &Agreement) -> bool {
    a.relative_difference() > LIMIT && a.z_score() <= 2.0
}

/// [`furnace_agreement`] with the retries of [`needs_more_samples`].
fn settled_mean(truth: &Truth) -> Agreement {
    let (mut forward, mut main) = START_SAMPLES;
    loop {
        let agreement = furnace_agreement(truth, forward, main);
        if !needs_more_samples(&agreement) || forward >= MAX_FORWARD_SAMPLES {
            return agreement;
        }
        forward *= 4;
        main *= 4;
    }
}

/// [`furnace_agreement_rgb`] with the retries of [`needs_more_samples`] for any channel.
fn settled_channels(truth: &Truth) -> [Agreement; 3] {
    let (mut forward, mut main) = START_SAMPLES;
    loop {
        let channels = furnace_agreement_rgb(truth, forward, main);
        if !channels.iter().any(needs_more_samples) || forward >= MAX_FORWARD_SAMPLES {
            return channels;
        }
        forward *= 4;
        main *= 4;
    }
}

fn check(label: &str, truth: &Truth) {
    let agreement = settled_mean(truth);
    eprintln!(
        "cross-check {label}: forward {:.5} (+-{:.5}), main tracer {:.5} (+-{:.5}), relative difference {:.4} ({:.1} combined standard errors)",
        agreement.forward,
        agreement.forward_se,
        agreement.main,
        agreement.main_se,
        agreement.relative_difference(),
        agreement.z_score()
    );
    assert!(
        agreement.relative_difference() <= LIMIT,
        "{label}: forward {:.5} against main tracer {:.5} (relative difference {:.4} > {LIMIT}, {:.1} combined standard errors: {})",
        agreement.forward,
        agreement.main,
        agreement.relative_difference(),
        agreement.z_score(),
        if agreement.z_score() <= 2.0 {
            "compatible with noise even at the largest sample count"
        } else {
            "a BIAS between the tracers"
        }
    );
}

#[test]
fn forward_model_and_main_tracer_agree_in_the_white_furnace() {
    // A clear stone: only the Fresnel and the total reflections.
    check("clear", &Truth::neutral(0.0, None, 0.0));
    // A uniformly absorbing stone: the path length inside the stone.
    check("uniform", &Truth::neutral(0.15, None, 0.0));
    // A neutral bicolour: the per-zone path lengths and the unit chain.
    check("bicolour", &Truth::neutral(0.05, Some(0.25), 0.3));
}

/// Round 2, C5: the main-tracer photos showed common gains of +3 to +15 percent. A clear stone
/// cannot show a colour-dependent calibration offset, a coloured one can, so the agreement is
/// checked per camera channel for the two uniform colours.
#[test]
fn coloured_stones_agree_per_camera_channel_in_the_white_furnace() {
    for case in [ColourCase::UniformPale, ColourCase::UniformSaturated] {
        let truth = Truth::new(case, 0.3);
        let channels = settled_channels(&truth);
        for (name, agreement) in ["red", "green", "blue"].iter().zip(&channels) {
            eprintln!(
                "cross-check {} {name}: forward {:.5} (+-{:.5}), main tracer {:.5} (+-{:.5}), relative difference {:.4} ({:.1} combined standard errors)",
                case.label(),
                agreement.forward,
                agreement.forward_se,
                agreement.main,
                agreement.main_se,
                agreement.relative_difference(),
                agreement.z_score()
            );
            assert!(
                agreement.relative_difference() <= LIMIT,
                "{} {name}: forward {:.5} against main tracer {:.5} (relative difference {:.4} > {LIMIT}, {:.1} combined standard errors: {})",
                case.label(),
                agreement.forward,
                agreement.main,
                agreement.relative_difference(),
                agreement.z_score(),
                if agreement.z_score() <= 2.0 {
                    "compatible with noise even at the largest sample count"
                } else {
                    "a BIAS between the tracers"
                }
            );
        }
    }
}

/// The two references of the empty furnace, the camera-response one the photos are normalised
/// with and the 1 nm CMF reconstruction the first version used, must be the same quantity.
#[test]
fn the_two_furnace_references_are_the_same_quantity() {
    let setup = crate::synth::Setup::new(&crate::synth::SetupSpec::default());
    let by_camera = camera_furnace_rgb(&setup.camera);
    let by_cmf = furnace_camera_rgb();
    for c in 0..3 {
        let relative = (by_camera[c] - by_cmf[c]).abs() / by_cmf[c].abs();
        assert!(
            relative < 5e-3,
            "channel {c}: camera {:.6} against CMF {:.6} (relative {relative:.5})",
            by_camera[c],
            by_cmf[c]
        );
    }
}
