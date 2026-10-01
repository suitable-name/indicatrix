//! Tests for the spectral-MIS weighting and the hero-wavelength comb.

use super::{
    super::{sampling::hash_u32, transport::wrapped_hero_wavelengths},
    *,
};

/// Deterministic unit-interval draw built from the same `hash_u32` PRNG
/// `trace_spectral_ray` itself uses, so this test's Monte Carlo trials are
/// reproducible without pulling in an external `rand` dependency.
fn unit_rand(seed: u32) -> f32 {
    (hash_u32(seed) as f32) / 4_294_967_295.0
}

/// The rejected `own_pdf / sum_pdf * num_channels` balance-heuristic weight (see
/// `mis_weighted_radiance`'s doc comment). Reproduced here directly so this
/// regression test can permanently guard against reintroducing the bias it causes.
fn shipped_biased_weight(radiance: f32, own_pdf: f32, sum_pdf: f32, num_channels: usize) -> f32 {
    let weight = (own_pdf / sum_pdf.max(1e-8)) * num_channels as f32;
    radiance * weight
}

/// Discriminating Monte Carlo regression test for the spectral-MIS bias bug, using
/// UNEQUAL per-channel pdfs (channel 0 hero R0=0.2, channel 1 companion R1=0.6) with
/// closed-form ground truth `L_k = 1.0` (a Fresnel interface reflects or transmits
/// with unit total probability) -- an equal-pdf test cannot discriminate, since
/// `own_pdf/sum_pdf * N` and the constant weight 1 are then algebraically identical.
/// Asserts the fixed estimator (weight=1) converges within a few percent while the
/// old `own_pdf/sum_pdf * N` weight is biased by roughly +17% on both channels.
#[test]
fn two_channel_fresnel_monte_carlo_discriminates_correct_from_biased_weighting() {
    const R0: f32 = 0.2; // hero (channel 0) reflectance
    const R1: f32 = 0.6; // companion (channel 1) reflectance, deliberately different
    const TRIALS: u32 = 400_000;
    const GROUND_TRUTH: f32 = 1.0;

    let mut plain_sum = [0.0f64; 2];
    let mut biased_sum = [0.0f64; 2];

    for trial in 0..TRIALS {
        let xi = unit_rand(trial ^ 0xA5A5_5A5A);

        // radiance[k] and path_pdf[k] for the branch actually taken this trial,
        // mirroring trace_spectral_ray's own per-channel bookkeeping.
        let (radiance, path_pdf) = if xi < R0 {
            // Reflect branch, selected with the HERO's own probability R0.
            ([1.0f32, R1 / R0], [R0, R1])
        } else {
            // Transmit branch, selected with the HERO's own probability (1 - R0).
            ([1.0f32, (1.0 - R1) / (1.0 - R0)], [1.0 - R0, 1.0 - R1])
        };

        let sum_pdf = path_pdf[0] + path_pdf[1];
        for k in 0..2 {
            plain_sum[k] += f64::from(mis_weighted_radiance(radiance[k]));
            biased_sum[k] += f64::from(shipped_biased_weight(radiance[k], path_pdf[k], sum_pdf, 2));
        }
    }

    let plain_avg: Vec<f32> = plain_sum
        .iter()
        .map(|s| (*s / f64::from(TRIALS)) as f32)
        .collect();
    let biased_avg: Vec<f32> = biased_sum
        .iter()
        .map(|s| (*s / f64::from(TRIALS)) as f32)
        .collect();

    for (k, &avg) in plain_avg.iter().enumerate() {
        let err = (avg - GROUND_TRUTH).abs() / GROUND_TRUTH;
        assert!(
            err < 0.03,
            "FIXED (weight=1) estimator for channel {} should converge to the ground truth {} within 3% over {} trials (got {}, {:.2}% error)",
            k,
            GROUND_TRUTH,
            TRIALS,
            avg,
            err * 100.0
        );
    }

    // The old formula must be clearly, substantially biased -- proving the test
    // actually discriminates between the two formulas.
    for (k, &avg) in biased_avg.iter().enumerate() {
        let err = (avg - GROUND_TRUTH).abs() / GROUND_TRUTH;
        assert!(
            err > 0.10,
            "the OLD shipped own_pdf/sum_pdf*N weight is expected to be substantially biased (>10%) on this scenario for channel {} (got {}, {:.2}% error) -- if this assertion fails, this regression test has lost its discriminating power",
            k,
            avg,
            err * 100.0
        );
    }
}

/// The wrapped hero-wavelength construction must (a) keep every generated
/// wavelength within the visible range [380, 780] regardless of the hero draw,
/// including right at the wraparound boundary, and (b) always place the hero
/// (`lambda_hero` itself) at array index 0.
#[test]
fn wrapped_hero_wavelengths_stay_in_visible_range_and_hero_is_always_index_0() {
    for seed in 0..20_000u32 {
        let hero_rand = unit_rand(seed);
        let lambdas: [f32; 8] = wrapped_hero_wavelengths(hero_rand);
        let lambda_hero = hero_rand.mul_add(780.0 - 380.0, 380.0);

        for (k, &l) in lambdas.iter().enumerate() {
            assert!(
                (380.0..=780.0).contains(&l),
                "wavelength at channel {k} must stay within [380, 780] (seed={seed}, hero_rand={hero_rand}, got {l})"
            );
        }
        assert!(
            (lambdas[0] - lambda_hero).abs() < 1e-3,
            "hero must always land at array index 0 (seed={}, hero_rand={}, lambdas[0]={}, lambda_hero={})",
            seed,
            hero_rand,
            lambdas[0],
            lambda_hero
        );
    }

    // Boundary check: a hero_rand right at the top of its range wraps the highest
    // companion channels back down past 380nm rather than running off past 780nm.
    let lambdas_top: [f32; 8] = wrapped_hero_wavelengths(0.999_999);
    for &l in &lambdas_top {
        assert!(
            (380.0..=780.0).contains(&l),
            "boundary hero draw produced an out-of-range wavelength: {l}"
        );
    }
}

/// Confirms the key statistical property the wrapped construction buys: every one
/// of the N channel slots is, across many draws, uniformly distributed over the
/// full comb-relative rotation, i.e. no channel index is structurally privileged.
#[test]
fn wrapped_hero_wavelengths_cover_every_channel_slot_uniformly() {
    let mut min_seen = [1000.0f32; 8];
    let mut max_seen = [0.0f32; 8];
    for seed in 0..20_000u32 {
        let hero_rand = unit_rand(seed ^ 0xDEAD_BEEF);
        let lambdas: [f32; 8] = wrapped_hero_wavelengths(hero_rand);
        for k in 0..8 {
            min_seen[k] = min_seen[k].min(lambdas[k]);
            max_seen[k] = max_seen[k].max(lambdas[k]);
        }
    }
    for k in 0..8 {
        // Each channel should, across enough draws, range across nearly the
        // entire [380, 780] spectrum, not just its "home" 50nm sub-band.
        assert!(
            max_seen[k] - min_seen[k] > 350.0,
            "channel {} should range across nearly the full spectrum over many hero draws (got min={}, max={}, span={})",
            k,
            min_seen[k],
            max_seen[k],
            max_seen[k] - min_seen[k]
        );
    }
}

/// `spectral_mis_weight` must reduce to exactly 1.0 whenever every channel's
/// `path_pdf` is identical -- the case a non-dispersive material forces (identical
/// n(lambda) makes every per-channel Fresnel probability, and hence every
/// `path_pdf` factor, identical across channels): `sum_pdf` collapses to exactly
/// `N * path_pdf[hero_idx]`, so the weight is `N * p / (N * p) == 1.0` for any
/// common value `p`, checked here across several hero indices and common values.
#[test]
fn spectral_mis_weight_is_exactly_unity_when_all_channels_agree() {
    for &p in &[1.0f32, 0.5, 1e-4, 1e-3, 0.999_9] {
        for hero_idx in 0..8 {
            let path_pdf = [p; 8];
            let w = spectral_mis_weight(&path_pdf, hero_idx);
            // `sum_pdf` is an iterative float sum of 8 equal values, not
            // necessarily bit-identical to `8.0 * p` -- checks "1.0 up to a
            // couple ULPs", not literal f32 equality.
            assert!(
                (w - 1.0).abs() < 1e-6,
                "weight must be 1.0 (up to float rounding) when every channel's path_pdf is identical (p={p}, hero_idx={hero_idx}, got {w})"
            );
        }
    }
}

/// `spectral_mis_weight` must depart from 1.0 once channels disagree, and must
/// approach `N` (here 8) as the non-hero channels' `path_pdf` collapses toward 0 --
/// once chromatic termination kills off every companion, the surviving hero's own
/// sample gets the full weight (the mechanism producing dispersion "fire").
#[test]
fn spectral_mis_weight_approaches_n_as_companions_are_chromatically_terminated() {
    let hero_idx = 0usize;
    let mut path_pdf = [0.3f32; 8];
    path_pdf[hero_idx] = 0.3;
    let w_all_alive = spectral_mis_weight(&path_pdf, hero_idx);
    assert!(
        (w_all_alive - 1.0).abs() < 1e-4,
        "all channels agreeing should give weight ~= 1.0 (got {w_all_alive})"
    );

    // Terminate every companion (path_pdf -> 0), leaving only the hero alive.
    for (k, p) in path_pdf.iter_mut().enumerate() {
        if k != hero_idx {
            *p = 0.0;
        }
    }
    let w_hero_only = spectral_mis_weight(&path_pdf, hero_idx);
    assert!(
        (w_hero_only - 8.0).abs() < 1e-4,
        "with every companion terminated, weight should approach N=8 (got {w_hero_only})"
    );
}

/// Discriminating Monte Carlo regression test for the spectral-MIS weight extended
/// to a genuine dispersive-refraction "chromatic termination" event. Unlike the
/// sibling test above (fixed hero at channel 0), this alternates which of the two
/// channels drives (p=1/2 each trial) -- the combined weight is provably biased
/// under a single fixed hero and only becomes unbiased once the ensemble genuinely
/// alternates, matching how a real render accumulates independent samples with
/// their own wrapped hero draw. The companion's transmission is modelled as
/// genuinely dispersive: the non-driving channel's `path_pdf` AND radiance both
/// zero at that event (chromatic termination). Ground truth is still exactly 1.0
/// (Fresnel unitarity) by Veach's theorem applied per-channel.
#[test]
fn two_channel_dispersive_termination_monte_carlo_is_unbiased_under_alternating_hero() {
    const R_A: f32 = 0.2;
    const R_B: f32 = 0.6;
    const TRIALS: u32 = 400_000;
    const GROUND_TRUTH: f32 = 1.0;

    // One Fresnel-interface trial. `hero_is_a` selects which channel drives the
    // shared branch decision. Returns (F_A, F_B): this trial's combined (weighted)
    // estimate of channel A's and channel B's own integral.
    fn trial(xi: f32, hero_is_a: bool) -> (f32, f32) {
        let (r_hero, r_other) = if hero_is_a { (R_A, R_B) } else { (R_B, R_A) };

        let (rad_hero, rad_other, pdf_hero, pdf_other) = if xi < r_hero {
            // Reflect: never dispersive -- both channels' directions coincide.
            (1.0f32, r_other / r_hero, r_hero, r_other)
        } else {
            // Transmit: genuinely dispersive -- the companion's refracted
            // direction never coincides with the driving channel, so its path_pdf
            // and Stokes/radiance both collapse to 0 (chromatic termination).
            (1.0f32, 0.0f32, 1.0 - r_hero, 0.0f32)
        };

        let sum_pdf = pdf_hero + pdf_other;
        let weight = 2.0 * pdf_hero / sum_pdf.max(1e-8);

        if hero_is_a {
            (rad_hero * weight, rad_other * weight)
        } else {
            (rad_other * weight, rad_hero * weight)
        }
    }

    let mut sum_a = 0.0f64;
    let mut sum_b = 0.0f64;
    for trial_idx in 0..TRIALS {
        // Independent draws: which channel is hero this trial, and branch xi.
        let hero_is_a = unit_rand(trial_idx ^ 0x1234_5678) < 0.5;
        let xi = unit_rand(trial_idx ^ 0xA5A5_5A5A);
        let (f_a, f_b) = trial(xi, hero_is_a);
        sum_a += f64::from(f_a);
        sum_b += f64::from(f_b);
    }

    let avg_a = (sum_a / f64::from(TRIALS)) as f32;
    let avg_b = (sum_b / f64::from(TRIALS)) as f32;

    for (label, avg) in [("A", avg_a), ("B", avg_b)] {
        let err = (avg - GROUND_TRUTH).abs() / GROUND_TRUTH;
        assert!(
            err < 0.03,
            "channel {} combined estimator should converge to ground truth {} within 3% over {} trials under alternating hero (got {}, {:.2}% error)",
            label,
            GROUND_TRUTH,
            TRIALS,
            avg,
            err * 100.0
        );
    }
}
