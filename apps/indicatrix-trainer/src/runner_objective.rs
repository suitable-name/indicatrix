//! What a run trains with: the algorithm and the objective weights of an episode.
//!
//! One model serves every weighting of the optical objective, because the objective's weight
//! fractions are part of the observation (design section 3.1). That only works for weightings the
//! model has seen, so every training episode and every validation design draws its own weights
//! with [`draw_weights`]. The draws are pure functions of a seed: the same seed gives the same
//! weights in any process and for any thread count. All seven presets, including tone goals, are
//! represented.

use std::fmt;

use indicatrix_ai::model::Prng;
use indicatrix_cut_core::{ObjectivePreset, ObjectiveWeights};

/// Random stream of the weight draw. The target draw of `dataset::sample_target` uses
/// `0x7A12_0001`.
const STREAM_WEIGHTS: u64 = 0x7A12_0002;
/// Random stream of the same-material draw.
const STREAM_SAME_MATERIAL: u64 = 0x7A12_0003;

/// Probability that [`ObjectiveSampling::Mixed`] draws a named preset instead of random weights.
const PRESET_PROBABILITY: f32 = 0.7;
/// Probability that a random weighting leaves the yield out.
const NO_YIELD_PROBABILITY: f32 = 0.7;
/// Range (low, high) of the windowing, extinction and tilt-brilliance weights of a random
/// weighting.
const OPTICAL_RANGE: (f32, f32) = (0.5, 4.0);
/// Range (low, high) of the yield weight of a random weighting that has one.
const YIELD_RANGE: (f32, f32) = (0.5, 3.0);

/// Every objective preset, including both face-up tone goals.
pub const TRAINABLE_PRESETS: [ObjectivePreset; 7] = [
    ObjectivePreset::Balanced,
    ObjectivePreset::Brilliance,
    ObjectivePreset::LowWindowing,
    ObjectivePreset::LowExtinction,
    ObjectivePreset::KeepWeight,
    ObjectivePreset::LightenDark,
    ObjectivePreset::IntensifyPale,
];

/// The training algorithm of a run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Algorithm {
    /// Supervised imitation of the finite-difference teacher on the policy's own states (`DAgger`).
    /// The default.
    #[default]
    Imitation,
    /// Proximal policy optimisation on the environment's reward.
    Ppo,
}

impl Algorithm {
    /// The algorithm `text` names (`imitation` or `ppo`; case and surrounding whitespace are
    /// ignored), or `None`.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "imitation" => Some(Self::Imitation),
            "ppo" => Some(Self::Ppo),
            _ => None,
        }
    }

    /// The lower-case name [`Self::parse`] reads back.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Imitation => "imitation",
            Self::Ppo => "ppo",
        }
    }
}

impl fmt::Display for Algorithm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// How the objective weights of an episode are drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ObjectiveSampling {
    /// Always the default weights (windowing, extinction and tilt brilliance equal, no yield).
    Balanced,
    /// Uniform over all seven presets, including both tone goals.
    Presets,
    /// A preset with probability 0.7, otherwise random weights: windowing, extinction and tilt
    /// brilliance uniform in `[0.5, 4.0)`, the yield weight 0 with probability 0.7 and uniform
    /// in `[0.5, 3.0)` otherwise, no tone term on random draws. The default.
    #[default]
    Mixed,
}

impl ObjectiveSampling {
    /// The sampling `text` names (`balanced`, `presets` or `mixed`; case and surrounding
    /// whitespace are ignored), or `None`.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "balanced" => Some(Self::Balanced),
            "presets" => Some(Self::Presets),
            "mixed" => Some(Self::Mixed),
            _ => None,
        }
    }

    /// The lower-case name [`Self::parse`] reads back.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Balanced => "balanced",
            Self::Presets => "presets",
            Self::Mixed => "mixed",
        }
    }
}

impl fmt::Display for ObjectiveSampling {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// A uniformly drawn trainable preset.
fn random_preset(rng: &mut Prng) -> ObjectivePreset {
    TRAINABLE_PRESETS[rng.next_index(TRAINABLE_PRESETS.len())]
}

/// A uniform draw in `[low, high)`.
fn uniform(rng: &mut Prng, (low, high): (f32, f32)) -> f32 {
    (high - low).mul_add(rng.next_f32(), low)
}

/// A random weighting: the three optical weights uniform, the yield weight mostly zero.
fn random_weights(rng: &mut Prng) -> ObjectiveWeights {
    let windowing = uniform(rng, OPTICAL_RANGE);
    let extinction = uniform(rng, OPTICAL_RANGE);
    let tilt_brilliance = uniform(rng, OPTICAL_RANGE);
    let yield_weight = if rng.next_f32() < NO_YIELD_PROBABILITY {
        0.0
    } else {
        uniform(rng, YIELD_RANGE)
    };
    ObjectiveWeights {
        windowing,
        extinction,
        tilt_brilliance,
        yield_weight,
        ..ObjectiveWeights::default()
    }
}

/// The objective weights of the episode (or validation design) identified by `seed`, drawn as
/// `sampling` says. Deterministic: a pure function of `sampling` and `seed`. The tone weight is
/// always zero.
#[must_use]
pub fn draw_weights(sampling: ObjectiveSampling, seed: u64) -> ObjectiveWeights {
    let mut rng = Prng::from_stream(seed, STREAM_WEIGHTS);
    match sampling {
        ObjectiveSampling::Balanced => ObjectiveWeights::default(),
        ObjectiveSampling::Presets => random_preset(&mut rng).weights(),
        ObjectiveSampling::Mixed => {
            if rng.next_f32() < PRESET_PROBABILITY {
                random_preset(&mut rng).weights()
            } else {
                random_weights(&mut rng)
            }
        }
    }
}

/// Whether the episode identified by `seed` is judged in the design's own material (the
/// optimizer task inside a retarget run) instead of a drawn target. `fraction` is the share of
/// episodes for which this holds; a fraction that is not above 0 never draws it and one of 1 or
/// more always does. Deterministic in `seed`.
#[must_use]
pub fn draws_same_material(fraction: f32, seed: u64) -> bool {
    if fraction.is_nan() || fraction <= 0.0 {
        return false;
    }
    if fraction >= 1.0 {
        return true;
    }
    Prng::from_stream(seed, STREAM_SAME_MATERIAL).next_f32() < fraction
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The weights of the seven trainable presets.
    fn preset_weights() -> Vec<ObjectiveWeights> {
        TRAINABLE_PRESETS.iter().map(|p| p.weights()).collect()
    }

    #[test]
    fn the_defaults_are_imitation_and_mixed() {
        assert_eq!(Algorithm::default(), Algorithm::Imitation);
        assert_eq!(ObjectiveSampling::default(), ObjectiveSampling::Mixed);
    }

    #[test]
    fn names_round_trip_through_display_and_parse() {
        for algo in [Algorithm::Imitation, Algorithm::Ppo] {
            assert_eq!(Algorithm::parse(&algo.to_string()), Some(algo));
        }
        for sampling in [
            ObjectiveSampling::Balanced,
            ObjectiveSampling::Presets,
            ObjectiveSampling::Mixed,
        ] {
            assert_eq!(
                ObjectiveSampling::parse(&sampling.to_string()),
                Some(sampling)
            );
        }
        assert_eq!(Algorithm::Imitation.to_string(), "imitation");
        assert_eq!(Algorithm::Ppo.to_string(), "ppo");
        assert_eq!(ObjectiveSampling::Balanced.to_string(), "balanced");
        assert_eq!(ObjectiveSampling::Presets.to_string(), "presets");
        assert_eq!(ObjectiveSampling::Mixed.to_string(), "mixed");
    }

    #[test]
    fn parsing_ignores_case_and_whitespace_and_refuses_unknown_names() {
        assert_eq!(Algorithm::parse("  PPO "), Some(Algorithm::Ppo));
        assert_eq!(Algorithm::parse("Imitation"), Some(Algorithm::Imitation));
        assert_eq!(
            ObjectiveSampling::parse("\tMixed\n"),
            Some(ObjectiveSampling::Mixed)
        );
        for bad in ["", "dagger", "ppo2", "balanced"] {
            assert_eq!(Algorithm::parse(bad), None, "{bad:?}");
        }
        for bad in ["", "random", "preset", "tone"] {
            assert_eq!(ObjectiveSampling::parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn every_library_preset_is_trainable_including_both_tone_goals() {
        assert_eq!(ObjectivePreset::ALL, TRAINABLE_PRESETS);
    }

    #[test]
    fn a_draw_is_a_pure_function_of_the_seed() {
        for sampling in [
            ObjectiveSampling::Balanced,
            ObjectiveSampling::Presets,
            ObjectiveSampling::Mixed,
        ] {
            for seed in [0, 1, 42, u64::MAX] {
                assert_eq!(draw_weights(sampling, seed), draw_weights(sampling, seed));
            }
        }
        // Different seeds reach different weights.
        let distinct = (0..64)
            .map(|seed| draw_weights(ObjectiveSampling::Mixed, seed))
            .fold(Vec::<ObjectiveWeights>::new(), |mut seen, weights| {
                if !seen.contains(&weights) {
                    seen.push(weights);
                }
                seen
            });
        assert!(
            distinct.len() > 10,
            "{} distinct weightings",
            distinct.len()
        );
    }

    #[test]
    fn balanced_is_the_default_weights_for_every_seed() {
        for seed in 0..32 {
            assert_eq!(
                draw_weights(ObjectiveSampling::Balanced, seed),
                ObjectiveWeights::default()
            );
        }
    }

    #[test]
    fn presets_draw_only_trainable_presets_and_reach_all_of_them() {
        let allowed = preset_weights();
        let mut reached = vec![false; allowed.len()];
        for seed in 0..400 {
            let weights = draw_weights(ObjectiveSampling::Presets, seed);
            let index = allowed
                .iter()
                .position(|candidate| *candidate == weights)
                .expect("a preset draw is one of the seven trainable presets");
            reached[index] = true;
        }
        assert!(reached.iter().all(|&hit| hit), "{reached:?}");
        assert!(
            TRAINABLE_PRESETS
                .iter()
                .any(|p| p.weights().tone_weight > 0.0)
        );
    }

    #[test]
    fn mixed_draws_presets_about_seven_times_in_ten_and_random_weights_in_range() {
        let presets = preset_weights();
        let draws: u64 = 4000;
        let mut preset_draws = 0_u32;
        let mut yield_free = 0_u32;
        let mut random_draws = 0_u32;
        for seed in 0..draws {
            let weights = draw_weights(ObjectiveSampling::Mixed, seed);
            if presets.contains(&weights) {
                preset_draws += 1;
                continue;
            }
            random_draws += 1;
            for weight in [
                weights.windowing,
                weights.extinction,
                weights.tilt_brilliance,
            ] {
                assert!((0.5..4.0).contains(&weight), "seed {seed}: {weight}");
            }
            if weights.yield_weight == 0.0 {
                yield_free += 1;
            } else {
                assert!(
                    (0.5..3.0).contains(&weights.yield_weight),
                    "seed {seed}: yield {}",
                    weights.yield_weight
                );
            }
        }
        let preset_share = f64::from(preset_draws) / draws as f64;
        assert!((0.66..0.74).contains(&preset_share), "{preset_share}");
        let yield_free_share = f64::from(yield_free) / f64::from(random_draws);
        assert!(
            (0.64..0.76).contains(&yield_free_share),
            "{yield_free_share}"
        );
    }

    #[test]
    fn the_same_material_draw_follows_its_fraction() {
        for seed in 0..64 {
            assert!(!draws_same_material(0.0, seed));
            assert!(!draws_same_material(-0.5, seed));
            assert!(!draws_same_material(f32::NAN, seed));
            assert!(draws_same_material(1.0, seed));
            assert!(draws_same_material(7.0, seed));
            assert_eq!(
                draws_same_material(0.25, seed),
                draws_same_material(0.25, seed)
            );
        }
        let draws: u64 = 4000;
        let hits = (0..draws)
            .filter(|&seed| draws_same_material(0.25, seed))
            .count();
        let share = hits as f64 / draws as f64;
        assert!((0.21..0.29).contains(&share), "{share}");
    }
}
