//! Deterministic PRNG hashing, per-bounce RNG stream salts, and the low-discrepancy
//! (stratified) sampling helpers used for pixel jitter and hero-wavelength selection.
//!
//! Also [`add_finite_sample`], the non-finite-sample rule every render backend shares.

use glam::Vec3;

/// Decorrelated hash salts for per-bounce random draws: each draw is
/// `hash_u32(rng_seed ^ hash_u32(bounce ^ SALT))`, giving independent streams instead of
/// reusing one value for multiple decisions.
// `pub(crate)`: `renderer::gpu::rng_check`'s GPU/CPU equivalence self-test hashes
// against these exact values, so they must stay real constants, not a hand-copied duplicate.
pub(crate) const FRESNEL_BRANCH_STREAM: u32 = 0x9E37_79B1;
pub(crate) const RUSSIAN_ROULETTE_STREAM: u32 = 0x517C_C1B7;
/// Ordinary/extraordinary eigenmode split at an air->crystal entry into an anisotropic
/// material.
pub(crate) const BIREFRINGENT_SPLIT_STREAM: u32 = 0x2545_F491;
/// o<->e (uniaxial) / mode-A<->mode-B (biaxial) re-coupling stream for an internal
/// reflection inside an anisotropic crystal -- see `apply_internal_mode_coupling`'s doc
/// comment for the physics. Distinct from `BIREFRINGENT_SPLIT_STREAM` (entry-only).
pub(crate) const MODE_COUPLING_STREAM: u32 = 0xCC9E_2D51;
/// Girdle finish: the 2D cosine-weighted-hemisphere direction draw at a
/// `FacetFinish::Frosted` bounce -- two streams for `(u, v)`.
pub(crate) const FROSTED_DIR_U_STREAM: u32 = 0x27D4_EB2F;
pub(crate) const FROSTED_DIR_V_STREAM: u32 = 0x1656_67B1;
/// Free-path distance draw for [`maybe_scatter_or_extinguish`]'s homogeneous-medium
/// exponential sampler.
pub(crate) const DISTANCE_SAMPLE_STREAM: u32 = 0xA24B_AED4;
/// The 2D Henyey-Greenstein direction draw at a scattering event -- two streams for
/// `(u, v)`.
pub(crate) const PHASE_DIR_U_STREAM: u32 = 0x9FB2_1C65;
pub(crate) const PHASE_DIR_V_STREAM: u32 = 0x1CE4_E5B9;
/// The 2D environment-direction draw for next-event estimation at a
/// Henyey-Greenstein scattering event -- two streams for `(u0, u1)`, fed to
/// `EnvironmentMap::sample`. A stream pair no other draw uses: a trace with NEE
/// disabled (`NeeContext::enabled == false`, every procedural-rig scene, and any trace
/// that otherwise opts out) never hashes against these at all, so it stays bit-identical
/// to one traced with NEE support absent altogether.
pub(crate) const NEE_ENV_DIR_U_STREAM: u32 = 0x4B72_5C19;
pub(crate) const NEE_ENV_DIR_V_STREAM: u32 = 0x2F1E_8A4D;
/// The 2D environment-direction draw for next-event estimation at a
/// [`FacetFinish::Frosted`](super::camera::FacetFinish::Frosted) bounce whose sampled
/// hemisphere lands on the EXTERIOR side -- an entry-facet reflect back outward, or an
/// exit-facet transmit out to the environment; see `scattering::apply_frosted_bounce`'s
/// doc comment for the sign-convention derivation of which branch that is. A stream pair
/// DISTINCT from [`NEE_ENV_DIR_U_STREAM`]/[`NEE_ENV_DIR_V_STREAM`] even though the two
/// draws never fire on the same bounce (a bounce is either an interior
/// Henyey-Greenstein scatter event or a frosted-facet dispatch, never both) -- kept
/// separate so neither draw can ever be mistaken for reusing the other's stream even if
/// that invariant changes later. Still a pair no other draw uses, so a trace with NEE
/// disabled stays bit-identical to one traced with NEE support absent altogether.
pub(crate) const FROSTED_NEE_ENV_DIR_U_STREAM: u32 = 0x6A09_E667;
pub(crate) const FROSTED_NEE_ENV_DIR_V_STREAM: u32 = 0xBB67_AE85;
/// Fluorescence vertex draws (`transport::inner`): the free-path distance to the vertex,
/// the emitter choice, the excitation wavelength and the 2D isotropic emission direction.
/// Streams no other draw uses, and only ever hashed when a scene has fluorescence, so a
/// trace with an empty `Fluorescence` consumes no extra randomness.
pub(crate) const FLUORESCENCE_DISTANCE_STREAM: u32 = 0x3C6E_F372;
pub(crate) const FLUORESCENCE_EMITTER_STREAM: u32 = 0xA54F_F53A;
pub(crate) const FLUORESCENCE_EXCITATION_STREAM: u32 = 0x510E_527F;
pub(crate) const FLUORESCENCE_DIR_U_STREAM: u32 = 0x9B05_688C;
pub(crate) const FLUORESCENCE_DIR_V_STREAM: u32 = 0x1F83_D9AB;

/// Fast integer hash for high-quality spatial/temporal PRNG
#[must_use]
pub const fn hash_u32(mut x: u32) -> u32 {
    x = x.wrapping_mul(0x85eb_ca6b);
    x ^= x >> 13;
    x = x.wrapping_mul(0xc2b2_ae35);
    x ^= x >> 16;
    x
}

// Stratified pixel jitter and hero-wavelength sampling. Production callers pass the
// hero draw into [`trace_spectral_ray`] as an explicit `hero_rand` (hashing it
// internally would un-stratify it). Mirrored bit-for-bit in
// `shaders/spectral_transport.wgsl` and `shaders/rng_equivalence.wgsl`.
//
// Three different prime bases (2, 3, 5), not one base rotated three ways: one base-2
// sequence for all three, decorrelated only by rotation, pairs (jx, jy) into a
// structured 2D point set (samples cluster along lines) -- rejected, 4-7x worse
// variance on the highest-variance pixels.

/// Stream salts for three independent Cranley-Patterson rotations (one per
/// stratified quantity: pixel jitter X, pixel jitter Y, hero wavelength).
///
/// Each pixel's rotation for a given quantity is
/// `low_discrepancy_base2(hash_u32(pixel_index ^ SALT))` -- a per-pixel phase shift
/// applied to that quantity's own [`radical_inverse_base`] sequence.
// `pub`: production callers that compute these rotations themselves live outside this crate.
pub const PIXEL_JITTER_X_ROTATION_STREAM: u32 = 0xA511_E9B3;
/// Salt for the per-pixel vertical jitter rotation.
pub const PIXEL_JITTER_Y_ROTATION_STREAM: u32 = 0x63D8_1B23;
/// Salt for the per-pixel hero-wavelength rotation.
pub const HERO_WAVELENGTH_ROTATION_STREAM: u32 = 0x1B87_3593;

/// Base-2 van der Corput radical-inverse sequence: `n` with its bits reversed,
/// reinterpreted as a fraction of `2^32` in `[0, 1)`.
///
/// Used for pixel-jitter-X (the fast bit-reversal path; see [`radical_inverse_base`]
/// for bases 3 and 5).
///
/// Not `hash_u32(n) as f32 / 4_294_967_295.0`: a hash gives independent-looking
/// randomness but poor stratification (gaps and clumps -> speckle noise). The van der
/// Corput sequence's first `N` terms are provably more evenly spread, and term `n`
/// depends only on `n` itself -- load-bearing for distributed rendering, where a worker
/// sees only an arbitrary slice of the sample-index space.
#[must_use]
#[inline]
pub fn low_discrepancy_base2(n: u32) -> f32 {
    // `2^32`, not `2^32 - 1`: canonical van der Corput normalization (`bits / b^digits`).
    (n.reverse_bits() as f32) / 4_294_967_296.0
}

/// Radical-inverse (van der Corput) sequence in an arbitrary prime `base`: writes `n`
/// in base `base`, reflects its digits around the "radix point", giving a fraction in
/// `[0, 1)`.
///
/// `base = 2` is handled faster by [`low_discrepancy_base2`]; this general
/// loop is for `base = 3` (pixel-jitter-Y) and `base = 5` (hero wavelength).
///
/// Feeds `f32::mul_add` (true FMA), matching the WGSL `fma()` port in
/// `shaders/{spectral_transport,rng_equivalence}.wgsl` -- budgeted at Tier 2 tolerance
/// since not every GPU/driver fuses `fma()` into true hardware FMA.
#[must_use]
#[inline]
pub fn radical_inverse_base(n: u32, base: u32) -> f32 {
    let mut remaining = n;
    let mut val = 0.0f32;
    let mut inv_base = 1.0f32 / base as f32;
    while remaining > 0 {
        let digit = remaining % base;
        val = (digit as f32).mul_add(inv_base, val);
        inv_base /= base as f32;
        remaining /= base;
    }
    val
}

/// Cranley-Patterson rotation: shifts a `[0, 1)` low-discrepancy sample `x` by `offset`
/// (also `[0, 1)`), wrapping around the unit interval (`x + offset` reduced mod 1).
///
/// A toroidal shift of a low-discrepancy sequence is still low-discrepancy, so this
/// decorrelates several uses of the same sequence -- one rotation per pixel, so
/// neighbouring pixels don't draw identical values on their first sample.
#[must_use]
#[inline]
pub fn cranley_patterson_rotate(x: f32, offset: f32) -> f32 {
    let sum = x + offset;
    sum - sum.floor()
}

// Shared per-sample seed/jitter/hero formula. [`pixel_rotations`] and [`sample_draws`]
// are the one place this formula is written down; every production call site computes
// through these instead of re-deriving the arithmetic. See `sample_draws_tests` below.

/// A pixel's three Cranley-Patterson rotation offsets.
///
/// One per stratified quantity (pixel-jitter-X, pixel-jitter-Y, hero wavelength) --
/// pure functions of the pixel index alone, hoisted out of the per-sample loop since
/// they don't vary per sample. Computed once per pixel via [`pixel_rotations`] and
/// reused for every sample of that pixel via [`sample_draws`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PixelRotations {
    /// Rotation applied to the pixel-jitter-X sequence.
    pub jitter_x: f32,
    /// Rotation applied to the pixel-jitter-Y sequence.
    pub jitter_y: f32,
    /// Rotation applied to the hero-wavelength sequence.
    pub hero: f32,
}

/// Computes `pixel`'s three [`PixelRotations`] -- see that type's doc comment.
#[must_use]
#[inline]
pub fn pixel_rotations(pixel: u32) -> PixelRotations {
    PixelRotations {
        jitter_x: low_discrepancy_base2(hash_u32(pixel ^ PIXEL_JITTER_X_ROTATION_STREAM)),
        jitter_y: low_discrepancy_base2(hash_u32(pixel ^ PIXEL_JITTER_Y_ROTATION_STREAM)),
        hero: low_discrepancy_base2(hash_u32(pixel ^ HERO_WAVELENGTH_ROTATION_STREAM)),
    }
}

/// One `(pixel, sample_num)` draw: the RNG seed plus the stratified pixel-jitter-X/Y and
/// hero-wavelength values, ready to feed a `Camera::generate_ray` + `trace_spectral_ray*`
/// call.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SampleDraws {
    /// RNG seed for the per-bounce draws.
    pub seed: u32,
    /// Stratified pixel-jitter-X value.
    pub jitter_x: f32,
    /// Stratified pixel-jitter-Y value.
    pub jitter_y: f32,
    /// Stratified hero-wavelength value.
    pub hero_rand: f32,
}

/// Computes the RNG seed and stratified draws for one `(pixel, sample_num)` pair.
///
/// `sample_num` must be the ABSOLUTE sample index, never a batch-relative offset --
/// load-bearing for distributed rendering (see this module's "partition correctness"
/// note). `rot` is `pixel`'s already-computed [`PixelRotations`].
#[must_use]
#[inline]
pub fn sample_draws(pixel: u32, sample_num: u32, rot: &PixelRotations) -> SampleDraws {
    let seed = hash_u32(pixel.wrapping_mul(0x9e37_79b9) ^ sample_num.wrapping_mul(0x85eb_ca6b));
    let jitter_x = cranley_patterson_rotate(low_discrepancy_base2(sample_num), rot.jitter_x) - 0.5;
    let jitter_y =
        cranley_patterson_rotate(radical_inverse_base(sample_num, 3), rot.jitter_y) - 0.5;
    let hero_rand = cranley_patterson_rotate(radical_inverse_base(sample_num, 5), rot.hero);
    SampleDraws {
        seed,
        jitter_x,
        jitter_y,
        hero_rand,
    }
}

/// Adds one traced sample's XYZ radiance into a pixel's running sum under the ONE
/// non-finite rule every render backend applies: a sample with any NaN or ±Inf
/// component is **dropped but still counted**.
///
/// "Dropped" means it adds nothing to `sum`, so one bad sample can never poison a
/// pixel's accumulator (NaN would otherwise propagate through every later `+=`, and
/// across every merge with another backend's buffer). "Still counted" means the caller
/// keeps including it in the sample count it later divides by: accumulators carry one
/// count per image, not per pixel, so there is no way to not count it for one pixel only.
/// Because every backend uses the same rule, per-pixel sums from different backends
/// (CPU scanline, export batch, worker, GPU reduction) merge by plain addition with the
/// total sample count as divisor, whichever backend traced a bad sample.
///
/// The GPU twin is `valid_xyz_bits` in `renderer/shaders/reduce_xyz.wgsl`, which applies
/// the identical test (an IEEE exponent that is not all ones, per component) while
/// summing each pixel's samples. Keep the two in lock-step.
#[inline]
pub fn add_finite_sample(sum: &mut Vec3, sample: Vec3) {
    if sample.is_finite() {
        *sum += sample;
    }
}

#[cfg(test)]
mod non_finite_rule_tests {
    use super::*;

    /// A pixel's samples, one of them NaN and one +Inf, with a -Inf component in a third:
    /// only the finite samples reach the sum, yet the divisor is still every sample
    /// traced, so the displayed mean is finite and exactly the finite sum over the full
    /// count.
    #[test]
    fn non_finite_samples_are_dropped_but_still_counted() {
        let samples = [
            Vec3::new(1.0, 2.0, 3.0),
            Vec3::new(f32::NAN, 1.0, 1.0),
            Vec3::new(4.0, 5.0, 6.0),
            Vec3::new(1.0, f32::INFINITY, 1.0),
            Vec3::new(1.0, 1.0, f32::NEG_INFINITY),
        ];
        let mut sum = Vec3::ZERO;
        for &s in &samples {
            add_finite_sample(&mut sum, s);
        }
        let count = u8::try_from(samples.len()).expect("five samples");
        assert_eq!(sum, Vec3::new(5.0, 7.0, 9.0));
        let mean = sum / f32::from(count);
        assert!(mean.is_finite(), "mean must stay finite, got {mean}");
        assert_eq!(mean, Vec3::new(5.0, 7.0, 9.0) / 5.0);
    }

    /// Two backends trace disjoint halves of one pixel's sample range; the first hits a
    /// NaN sample, the second an Inf one. Their sums merge by plain addition and the
    /// divisor stays the full sample count: the merged pixel is finite and equals the
    /// finite samples' sum over every sample traced.
    #[test]
    fn a_merged_pixel_stays_finite_when_one_backend_produced_nan_and_another_inf() {
        let finite = Vec3::new(0.25, 0.5, 0.75);
        let backend_a = [finite, Vec3::splat(f32::NAN), finite, finite];
        let backend_b = [finite, finite, Vec3::new(f32::INFINITY, 0.0, 0.0), finite];

        let mut sum_a = Vec3::ZERO;
        for &s in &backend_a {
            add_finite_sample(&mut sum_a, s);
        }
        let mut sum_b = Vec3::ZERO;
        for &s in &backend_b {
            add_finite_sample(&mut sum_b, s);
        }

        let merged = sum_a + sum_b;
        let divisor = u8::try_from(backend_a.len() + backend_b.len()).expect("eight samples");
        assert!(
            merged.is_finite(),
            "merged sum must stay finite, got {merged}"
        );
        assert_eq!(merged, finite * 6.0);
        assert_eq!(divisor, 8, "dropped samples still count toward the divisor");
        assert_eq!(merged / f32::from(divisor), finite * 6.0 / 8.0);
    }

    /// A pixel whose every sample is non-finite ends at exactly zero radiance (not NaN),
    /// so it shows black rather than poisoning a later merge.
    #[test]
    fn an_all_non_finite_pixel_sums_to_zero() {
        let mut sum = Vec3::ZERO;
        add_finite_sample(&mut sum, Vec3::splat(f32::NAN));
        add_finite_sample(&mut sum, Vec3::splat(f32::INFINITY));
        add_finite_sample(&mut sum, Vec3::splat(f32::NEG_INFINITY));
        assert_eq!(sum, Vec3::ZERO);
    }
}

#[cfg(test)]
mod sample_draws_tests {
    use super::*;

    /// Every draw lies in its documented interval: the two jitters in `[-0.5, 0.5)` and
    /// the hero value in `[0, 1)`. A Cranley-Patterson rotation of a value in `[0, 1)` by
    /// an offset in `[0, 1)` is `sum - floor(sum)`, which is below 1 for any `sum < 2`.
    #[test]
    fn draws_lie_in_their_unit_intervals() {
        for pixel in 0..200u32 {
            let rot = pixel_rotations(pixel);
            for value in [rot.jitter_x, rot.jitter_y, rot.hero] {
                assert!(
                    (0.0..1.0).contains(&value),
                    "pixel={pixel} rotation={value}"
                );
            }
            for sample_num in 0..64u32 {
                let d = sample_draws(pixel, sample_num, &rot);
                assert!(
                    (-0.5..0.5).contains(&d.jitter_x),
                    "pixel={pixel} sample={sample_num} jitter_x={}",
                    d.jitter_x
                );
                assert!(
                    (-0.5..0.5).contains(&d.jitter_y),
                    "pixel={pixel} sample={sample_num} jitter_y={}",
                    d.jitter_y
                );
                assert!(
                    (0.0..1.0).contains(&d.hero_rand),
                    "pixel={pixel} sample={sample_num} hero_rand={}",
                    d.hero_rand
                );
            }
        }
    }

    /// Different `(pixel, sample)` pairs must not share a seed. For a fixed pixel the
    /// map `sample -> hash(pixel * A ^ sample * B)` is injective (`B` is odd so the
    /// multiply is a bijection on `u32`, xor with a constant is a bijection, and
    /// `hash_u32` is a composition of bijections), and likewise for a fixed sample over
    /// pixels, so every seed below is distinct by construction.
    #[test]
    fn distinct_pixel_or_sample_gives_a_distinct_seed() {
        let mut by_sample = std::collections::HashSet::new();
        let rot = pixel_rotations(7);
        for sample_num in 0..512u32 {
            assert!(
                by_sample.insert(sample_draws(7, sample_num, &rot).seed),
                "sample {sample_num} repeated a seed"
            );
        }
        let mut by_pixel = std::collections::HashSet::new();
        for pixel in 0..512u32 {
            let rot = pixel_rotations(pixel);
            assert!(
                by_pixel.insert(sample_draws(pixel, 3, &rot).seed),
                "pixel {pixel} repeated a seed"
            );
        }
    }

    /// The same `(pixel, sample)` produces bit-identical draws on every call.
    #[test]
    fn draws_are_bit_stable_across_calls() {
        for pixel in [0u32, 1, 977, 123_456] {
            for sample_num in [0u32, 1, 2, 31, 4096] {
                let first = sample_draws(pixel, sample_num, &pixel_rotations(pixel));
                let second = sample_draws(pixel, sample_num, &pixel_rotations(pixel));
                assert_eq!(first.seed, second.seed);
                assert_eq!(first.jitter_x.to_bits(), second.jitter_x.to_bits());
                assert_eq!(first.jitter_y.to_bits(), second.jitter_y.to_bits());
                assert_eq!(first.hero_rand.to_bits(), second.hero_rand.to_bits());
            }
        }
    }

    /// Hand-evaluated draws with explicit rotations, so nothing here calls the hash:
    ///
    /// - Pixel 0, sample 0: `hash_u32(0 * A ^ 0 * B) = hash_u32(0) = 0` (every step of
    ///   the hash maps 0 to 0), and every radical inverse of 0 is 0.
    /// - Sample 1: base 2 gives `bit_reverse(1) / 2^32 = 2^31 / 2^32 = 0.5`; base 3 gives
    ///   `1/3`; base 5 gives `1/5`.
    /// - Sample 2: base 2 gives `2^30 / 2^32 = 0.25`; base 3 gives `2/3`; base 5 gives
    ///   `2/5`.
    ///
    /// With rotations `(0.75, 0.0, 0.5)` and sample 1 the jitter-X value is
    /// `(0.5 + 0.75) - floor(1.25) - 0.5 = -0.25` (exact in `f32`); jitter-Y is
    /// `1/3 - 0.5`; the hero value is `(0.2 + 0.5) = 0.7`.
    #[test]
    fn hand_computed_draws_for_small_sample_indices() {
        let zero = PixelRotations {
            jitter_x: 0.0,
            jitter_y: 0.0,
            hero: 0.0,
        };
        let d0 = sample_draws(0, 0, &zero);
        assert_eq!(d0.seed, 0);
        assert_eq!(d0.jitter_x, -0.5);
        assert_eq!(d0.jitter_y, -0.5);
        assert_eq!(d0.hero_rand, 0.0);

        let d2 = sample_draws(0, 2, &zero);
        assert_eq!(d2.jitter_x, -0.25);
        assert!(
            (d2.jitter_y - (2.0 / 3.0 - 0.5)).abs() < 1e-6,
            "{}",
            d2.jitter_y
        );
        assert!((d2.hero_rand - 0.4).abs() < 1e-6, "{}", d2.hero_rand);

        let rotated = PixelRotations {
            jitter_x: 0.75,
            jitter_y: 0.0,
            hero: 0.5,
        };
        let d1 = sample_draws(0, 1, &rotated);
        assert_eq!(d1.jitter_x, -0.25);
        assert!(
            (d1.jitter_y - (1.0 / 3.0 - 0.5)).abs() < 1e-6,
            "{}",
            d1.jitter_y
        );
        assert!((d1.hero_rand - 0.7).abs() < 1e-6, "{}", d1.hero_rand);
    }
}

#[cfg(test)]
mod rng_decorrelation_tests {
    use super::*;

    /// Guards against a quantized `(rng_seed + bounce*7919) % 1000` style progression
    /// (only 1000 distinct values, each a deterministic function of the previous one):
    /// the hash-based stream must produce a much larger spread of distinct values.
    #[test]
    fn hashed_branch_draw_is_not_quantized_to_1000_values() {
        // Mirrors the production draw at the first bounce.
        let bounce = 0u32;
        let mut values = std::collections::HashSet::new();
        for seed in 0..5000u32 {
            let v = hash_u32(seed ^ hash_u32(bounce ^ FRESNEL_BRANCH_STREAM));
            values.insert(v);
        }
        assert!(
            values.len() > 4900,
            "hashed draw should yield close to 5000 distinct values across 5000 seeds (got {})",
            values.len()
        );
    }

    /// The Fresnel branch decision and Russian-roulette draw use distinct decorrelated
    /// streams rather than the same random value; confirm the two salted streams
    /// diverge for the same (seed, bounce) pair across many samples.
    #[test]
    fn fresnel_and_russian_roulette_streams_are_decorrelated() {
        let mut agreements = 0u32;
        let trials = 2000u32;
        for seed in 0..trials {
            let fresnel = hash_u32(seed ^ hash_u32(3u32 ^ FRESNEL_BRANCH_STREAM));
            let rr = hash_u32(seed ^ hash_u32(3u32 ^ RUSSIAN_ROULETTE_STREAM));
            if fresnel == rr {
                agreements += 1;
            }
        }
        assert!(
            agreements == 0,
            "the Fresnel-branch and Russian-roulette streams must not collide across {trials} trials (got {agreements} collisions)"
        );
    }

    /// After bounce 4, survival must be a proper weighted Russian-roulette test
    /// (survive w.p. q, then divide by q), not a hard cutoff that discards energy.
    /// Exercises the exact q/draw construction `trace_spectral_ray` uses and checks
    /// the compensated estimator is unbiased: E[`survive_indicator` / q] == 1.
    #[test]
    fn russian_roulette_survival_is_unbiased_in_expectation() {
        let q = 0.3f32; // an arbitrary throughput level within the [0.05, 1.0] clamp range
        let trials = 200_000u32;
        let mut total = 0.0f32;
        for bounce in 0..trials {
            let rr_rand = (hash_u32(bounce ^ hash_u32(bounce ^ RUSSIAN_ROULETTE_STREAM)) as f32)
                / 4_294_967_295.0;
            if rr_rand <= q {
                total += 1.0 / q;
            }
        }
        let mean = total / trials as f32;
        assert!(
            (mean - 1.0).abs() < 0.02,
            "weighted Russian-roulette survival should be unbiased (E[indicator/q] ~= 1.0), got {mean}"
        );
    }
}
