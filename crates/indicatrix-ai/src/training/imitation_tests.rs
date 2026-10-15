//! Tests of the imitation trainer (`imitation.rs`).
//!
//! Fixtures use the CPU device, a small model and the fixture designs' shapes only; nothing needs
//! the design library. Labels come either from hand-built rows or from a fixed teacher network
//! (another seed than the student) so the learning tests have a target that is realisable.

use std::f32::consts::FRAC_1_SQRT_2;

use super::*;
use crate::{
    env::{GLOBAL_DIM, MEAN_BOUND, SLOPE_EPS, TIER_DIM, WEIGHT_FRACTION_INDEX},
    model::{GemstoneSetAttentionNetwork, Prng},
    training::{
        ModelShape, PpoConfig,
        burn_model::{BurnSetAttention, host_scalar},
    },
};

const MAX_TIERS: usize = 6;
const D_MODEL: usize = 16;
/// The teacher's sensitivities are scaled to a few tenths of a point per degree, like real labels.
const TARGET_SCALE: f32 = 0.5;
/// Balanced weight fractions (`[w_win, w_ext, w_tilt, w_yield] / sum`).
const BALANCED: [f32; N_COMPONENTS] = [1.0 / 3.0, 1.0 / 3.0, 1.0 / 3.0, 0.0];

fn shape(max_tiers: usize) -> ModelShape {
    ModelShape {
        d_tier: TIER_DIM,
        d_global: GLOBAL_DIM,
        d_model: D_MODEL,
        num_heads: 4,
        num_layers: 2,
        max_tiers,
    }
}

fn network(seed: u64) -> GemstoneSetAttentionNetwork {
    GemstoneSetAttentionNetwork::new(TIER_DIM, GLOBAL_DIM, D_MODEL, 4, 2, seed).unwrap()
}

/// A CPU trainer holding the weights of network seed 5; `shuffle_seed` drives the mini-batch order.
fn trainer(max_tiers: usize, shuffle_seed: u64) -> BurnTrainer {
    let mut trainer = BurnTrainer::new_cpu(shape(max_tiers), PpoConfig::default())
        .unwrap()
        .with_seed(shuffle_seed);
    trainer.load_weights(&network(5)).unwrap();
    trainer
}

fn export(trainer: &BurnTrainer) -> GemstoneSetAttentionNetwork {
    trainer.model.export_to_set_attention().unwrap()
}

/// `count` random states with the Balanced weight fractions; sample `i` has
/// `2 + i % (max_tiers - 1)` real tiers and zero padding after them.
fn random_states(count: usize, max_tiers: usize, seed: u64) -> (Vec<Vec<f32>>, Vec<usize>) {
    let mut rng = Prng::new(seed);
    let mut states = Vec::with_capacity(count);
    let mut n_tiers = Vec::with_capacity(count);
    for i in 0..count {
        let real = 2 + i % (max_tiers - 1);
        let mut row: Vec<f32> = (0..GLOBAL_DIM).map(|_| rng.next_signed_f32()).collect();
        row[WEIGHT_FRACTION_INDEX..WEIGHT_FRACTION_INDEX + N_COMPONENTS].copy_from_slice(&BALANCED);
        for slot in 0..max_tiers {
            let live = slot < real;
            row.extend((0..TIER_DIM).map(|_| if live { rng.next_signed_f32() } else { 0.0 }));
        }
        states.push(row);
        n_tiers.push(real);
    }
    (states, n_tiers)
}

/// The sensitivities (`[count * max_tiers * N_COMPONENTS]`) and gains (`[count]`) a fixed teacher
/// network predicts for `states`.
fn teacher_outputs(
    states: &[Vec<f32>],
    n_tiers: &[usize],
    max_tiers: usize,
    seed: u64,
) -> (Vec<f32>, Vec<f32>) {
    let device = Device::flex();
    let teacher = BurnSetAttention::from_set_attention(&network(seed), &device).unwrap();
    let count = states.len();
    let (mut globals, mut tiers, mut mask) = (Vec::new(), Vec::new(), Vec::new());
    for (state, &real) in states.iter().zip(n_tiers) {
        globals.extend_from_slice(&state[..GLOBAL_DIM]);
        tiers.extend_from_slice(&state[GLOBAL_DIM..]);
        mask.extend((0..max_tiers).map(|slot| flag(slot < real)));
    }
    let out = teacher.forward(
        Tensor::<3>::from_data(
            TensorData::new(tiers, [count, max_tiers, TIER_DIM]),
            &device,
        ),
        Tensor::<2>::from_data(TensorData::new(globals, [count, GLOBAL_DIM]), &device),
        Tensor::<2>::from_data(TensorData::new(mask, [count, max_tiers]), &device),
    );
    (
        host_vec(out.sensitivities).unwrap(),
        host_vec(out.gain).unwrap(),
    )
}

/// Owned data behind an [`ImitationBatch`].
struct Fixture {
    states: Vec<Vec<f32>>,
    n_tiers: Vec<usize>,
    free_tiers: Vec<Vec<bool>>,
    sensitivities: Vec<Vec<[f32; N_COMPONENTS]>>,
    score_slopes: Vec<Vec<f32>>,
    oracle_gain: Vec<f32>,
}

impl Fixture {
    fn batch(&self) -> ImitationBatch<'_> {
        ImitationBatch {
            states: &self.states,
            n_tiers: &self.n_tiers,
            free_tiers: &self.free_tiers,
            sensitivities: &self.sensitivities,
            score_slopes: &self.score_slopes,
            oracle_gain: &self.oracle_gain,
        }
    }

    /// Valid shapes, every tier free, every label missing (NaN).
    fn blank(count: usize, max_tiers: usize) -> Self {
        let (states, n_tiers) = random_states(count, max_tiers, 21);
        Self {
            free_tiers: n_tiers.iter().map(|&n| vec![true; n]).collect(),
            sensitivities: n_tiers
                .iter()
                .map(|&n| vec![[f32::NAN; N_COMPONENTS]; n])
                .collect(),
            score_slopes: n_tiers.iter().map(|&n| vec![f32::NAN; n]).collect(),
            oracle_gain: vec![f32::NAN; count],
            states,
            n_tiers,
        }
    }

    /// Labels from a fixed teacher network of `teacher_seed`: the sensitivities are its (scaled)
    /// head outputs, the score slope of a tier is their Balanced composition, and the gain is its
    /// gain head.
    fn from_teacher(count: usize, max_tiers: usize, teacher_seed: u64) -> Self {
        let mut fx = Self::blank(count, max_tiers);
        let (sens, gains) = teacher_outputs(&fx.states, &fx.n_tiers, max_tiers, teacher_seed);
        for i in 0..count {
            let fractions =
                &fx.states[i][WEIGHT_FRACTION_INDEX..WEIGHT_FRACTION_INDEX + N_COMPONENTS];
            let rows: Vec<[f32; N_COMPONENTS]> = (0..fx.n_tiers[i])
                .map(|slot| {
                    let at = (i * max_tiers + slot) * N_COMPONENTS;
                    std::array::from_fn(|c| TARGET_SCALE * sens[at + c])
                })
                .collect();
            fx.score_slopes[i] = rows
                .iter()
                .map(|row| fractions.iter().zip(row).map(|(f, s)| f * s).sum::<f32>())
                .collect();
            fx.sensitivities[i] = rows;
            fx.oracle_gain[i] = TARGET_SCALE * gains[i];
        }
        fx
    }

    /// Drop every label of one sample (all NaN).
    fn clear_labels(&mut self, sample: usize) {
        let n = self.n_tiers[sample];
        self.sensitivities[sample] = vec![[f32::NAN; N_COMPONENTS]; n];
        self.score_slopes[sample] = vec![f32::NAN; n];
        self.oracle_gain[sample] = f32::NAN;
    }
}

/// Two samples (3 and 2 real tiers) padded to 4, covering every masking rule: tier 1 of sample 0
/// is not free, one component is NaN, tier 0 of sample 1 has no label, sample 1 has no gain.
fn hand_built() -> Fixture {
    let row = |seed: usize| -> Vec<f32> {
        (0..GLOBAL_DIM + 4 * TIER_DIM)
            .map(|k| ((seed * 100 + k) as f32 * 0.37).sin())
            .collect()
    };
    Fixture {
        states: vec![row(1), row(2)],
        n_tiers: vec![3, 2],
        free_tiers: vec![vec![true, false, true], vec![true, true]],
        sensitivities: vec![
            vec![
                [1.0, 2.0, 3.0, 4.0],
                [5.0, 6.0, 7.0, 8.0],
                [0.5, f32::NAN, -1.5, 2.5],
            ],
            vec![[f32::NAN; N_COMPONENTS], [0.1, 0.2, 0.3, 0.4]],
        ],
        score_slopes: vec![vec![-2.0, 4.0, 1.0], vec![f32::NAN, 0.8]],
        oracle_gain: vec![3.5, f32::NAN],
    }
}

fn assert_close(got: &[f32], want: &[f32], tolerance: f32) {
    assert_eq!(got.len(), want.len(), "{got:?} vs {want:?}");
    for (index, (g, w)) in got.iter().zip(want).enumerate() {
        assert!(
            (g - w).abs() <= tolerance,
            "entry {index}: {g} vs {w} (tolerance {tolerance})"
        );
    }
}

fn shape_message(result: Result<(), TrainError>) -> String {
    match result {
        Err(TrainError::ShapeMismatch(message)) => message,
        other => panic!("expected a shape mismatch, got {other:?}"),
    }
}

fn all_finite(m: &ImitationMetrics) -> bool {
    [m.loss, m.sens_mse, m.cosine, m.gain_loss, m.grad_norm]
        .iter()
        .all(|v| v.is_finite())
}

fn matrix(values: Vec<f32>, rows: usize, cols: usize) -> Tensor<2> {
    Tensor::<2>::from_data(TensorData::new(values, [rows, cols]), &Device::flex())
}

fn cube(values: Vec<f32>, dims: [usize; 3]) -> Tensor<3> {
    Tensor::<3>::from_data(TensorData::new(values, dims), &Device::flex())
}

/// A one-update config over the whole batch.
fn single_step(batch_len: usize) -> ImitationConfig {
    ImitationConfig {
        mini_batch_size: batch_len,
        epochs_per_round: 1,
        ..ImitationConfig::default()
    }
}

// Configuration

#[test]
fn defaults_are_the_documented_ones() {
    let config = ImitationConfig::default();
    assert_eq!(config.learning_rate, 3e-4);
    assert_eq!(config.lr_schedule, LrSchedule::Constant);
    assert_eq!(config.mini_batch_size, 256);
    assert_eq!(config.epochs_per_round, 2);
    assert_eq!(config.max_grad_norm, 1.0);
    assert_eq!(config.cosine_coeff, 0.5);
    assert_eq!(config.gain_coeff, 0.1);
    assert!(config.validate().is_ok());
}

#[test]
fn validate_refuses_every_out_of_range_field() {
    let cases: [(&str, fn(&mut ImitationConfig)); 13] = [
        ("zero mini-batch", |c| c.mini_batch_size = 0),
        ("zero epochs", |c| c.epochs_per_round = 0),
        ("zero learning rate", |c| c.learning_rate = 0.0),
        ("negative learning rate", |c| c.learning_rate = -1e-3),
        ("NaN learning rate", |c| c.learning_rate = f32::NAN),
        ("negative cosine coefficient", |c| c.cosine_coeff = -0.1),
        ("NaN cosine coefficient", |c| c.cosine_coeff = f32::NAN),
        ("negative gain coefficient", |c| c.gain_coeff = -0.1),
        ("infinite gain coefficient", |c| {
            c.gain_coeff = f32::INFINITY
        }),
        ("NaN gradient cap", |c| c.max_grad_norm = f32::NAN),
        ("infinite gradient cap", |c| c.max_grad_norm = f32::INFINITY),
        ("zero gradient cap", |c| c.max_grad_norm = 0.0),
        ("empty linear schedule", |c| {
            c.lr_schedule = LrSchedule::Linear { total_updates: 0 };
        }),
    ];
    for (what, damage) in cases {
        let mut config = ImitationConfig::default();
        damage(&mut config);
        assert!(
            matches!(config.validate(), Err(TrainError::InvalidConfig(_))),
            "{what} was accepted"
        );
    }
}

#[test]
fn zero_coefficients_and_a_real_linear_schedule_are_valid() {
    let off = ImitationConfig {
        cosine_coeff: 0.0,
        gain_coeff: 0.0,
        lr_schedule: LrSchedule::Linear { total_updates: 5 },
        ..ImitationConfig::default()
    };
    assert!(off.validate().is_ok());
}

#[test]
fn config_round_trips_through_json_and_missing_fields_take_the_defaults() {
    let config = ImitationConfig {
        learning_rate: 1e-3,
        lr_schedule: LrSchedule::Linear { total_updates: 50 },
        cosine_coeff: 0.25,
        ..ImitationConfig::default()
    };
    let json = serde_json::to_string(&config).unwrap();
    let back: ImitationConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(back, config);

    let sparse: ImitationConfig = serde_json::from_str(r#"{"learning_rate": 0.001}"#).unwrap();
    assert!((sparse.learning_rate - 0.001).abs() < 1e-9);
    assert_eq!(sparse.mini_batch_size, 256);
    assert_eq!(sparse.gain_coeff, 0.1);
}

// Batch validation

#[test]
fn batch_len_and_emptiness() {
    let three = Fixture::blank(3, MAX_TIERS);
    assert_eq!((three.batch().len(), three.batch().is_empty()), (3, false));
    let none = Fixture::blank(0, MAX_TIERS);
    assert_eq!((none.batch().len(), none.batch().is_empty()), (0, true));
    assert!(
        three
            .batch()
            .validate(GLOBAL_DIM, TIER_DIM, MAX_TIERS)
            .is_ok()
    );
}

#[test]
fn validation_names_the_slice_the_sample_and_the_field() {
    // (text the message must contain, field it must name, damage to a valid 3-sample fixture)
    let cases: [(&str, &str, fn(&mut Fixture)); 11] = [
        ("batch slices", "n_tiers", |fx| fx.n_tiers.truncate(2)),
        ("batch slices", "free_tiers", |fx| fx.free_tiers.truncate(2)),
        ("batch slices", "sensitivities", |fx| {
            fx.sensitivities.truncate(1)
        }),
        ("batch slices", "score_slopes", |fx| {
            fx.score_slopes.truncate(2)
        }),
        ("batch slices", "oracle_gain", |fx| {
            fx.oracle_gain.truncate(2)
        }),
        ("sample 1", "states", |fx| fx.states[1].truncate(10)),
        ("sample 2", "n_tiers", |fx| fx.n_tiers[2] = 0),
        ("sample 0", "n_tiers", |fx| fx.n_tiers[0] = MAX_TIERS + 1),
        ("sample 1", "free_tiers", |fx| fx.free_tiers[1].push(true)),
        ("sample 2", "sensitivities", |fx| {
            fx.sensitivities[2].truncate(1)
        }),
        ("sample 0", "score_slopes", |fx| {
            fx.score_slopes[0].push(0.0)
        }),
    ];
    for (place, field, damage) in cases {
        let mut fx = Fixture::blank(3, MAX_TIERS);
        damage(&mut fx);
        let message = shape_message(fx.batch().validate(GLOBAL_DIM, TIER_DIM, MAX_TIERS));
        assert!(
            message.contains(place) && message.contains(field),
            "expected {place} / {field}: {message}"
        );
    }
}

#[test]
fn a_non_finite_state_is_refused_by_sample() {
    let mut fx = Fixture::blank(3, MAX_TIERS);
    fx.states[1][GLOBAL_DIM + 2] = f32::NAN;
    let result = fx.batch().validate(GLOBAL_DIM, TIER_DIM, MAX_TIERS);
    assert!(
        matches!(&result, Err(TrainError::NonFinite(m)) if m.contains("sample 1")),
        "{result:?}"
    );
}

// Packing

#[test]
fn label_rows_follow_the_masking_rules() {
    let fx = hand_built();
    let rows = label_rows(&fx.batch(), &[0, 1], 4);

    // Sample 0: tier 1 is not free (masked although finite), tier 2 lacks one component, tier 3
    // is padding. Sample 1: tier 0 has no label at all, tiers 2 and 3 are padding.
    let o = 0.0_f32;
    let want_target = [
        [
            1.0, 2.0, 3.0, 4.0, o, o, o, o, 0.5, o, -1.5, 2.5, o, o, o, o,
        ],
        [o, o, o, o, 0.1, 0.2, 0.3, 0.4, o, o, o, o, o, o, o, o],
    ];
    assert_eq!(rows.sens_target, want_target.concat());
    let want_mask = [
        [1.0, 1.0, 1.0, 1.0, o, o, o, o, 1.0, o, 1.0, 1.0, o, o, o, o],
        [o, o, o, o, 1.0, 1.0, 1.0, 1.0, o, o, o, o, o, o, o, o],
    ];
    assert_eq!(rows.sens_mask, want_mask.concat());

    // Directions: -MEAN_BOUND * slope / (max |slope| + SLOPE_EPS) over the sample's finite slopes.
    let direction = |slope: f32, max: f32| -MEAN_BOUND * slope / (max + SLOPE_EPS);
    let want_dir = [
        direction(-2.0, 4.0),
        o,
        direction(1.0, 4.0),
        o,
        o,
        direction(0.8, 0.8),
        o,
        o,
    ];
    assert_close(&rows.dir_target, &want_dir, 1e-5);
    assert_eq!(rows.dir_mask, vec![1.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    assert_eq!(rows.gain_target, vec![3.5, 0.0]);
    assert_eq!(rows.gain_mask, vec![1.0, 0.0]);
    assert!(rows.has_labels());
}

#[test]
fn label_rows_of_an_unlabelled_batch_report_no_labels() {
    let fx = Fixture::blank(4, MAX_TIERS);
    let rows = label_rows(&fx.batch(), &[0, 1, 2, 3], MAX_TIERS);
    assert!(!rows.has_labels());
    assert!(rows.sens_target.iter().all(|&v| v == 0.0));
    assert!(rows.dir_target.iter().all(|&v| v == 0.0));
    assert!(rows.gain_target.iter().all(|&v| v == 0.0));
}

#[test]
fn infinite_labels_are_treated_as_missing() {
    let mut fx = Fixture::blank(1, MAX_TIERS);
    fx.sensitivities[0][0] = [f32::INFINITY, 1.0, f32::NEG_INFINITY, f32::NAN];
    fx.score_slopes[0][0] = f32::INFINITY;
    fx.oracle_gain[0] = f32::INFINITY;
    let rows = label_rows(&fx.batch(), &[0], MAX_TIERS);
    assert_eq!(rows.sens_mask.iter().sum::<f32>(), 1.0);
    assert_eq!(rows.sens_target[1], 1.0);
    assert_eq!(rows.dir_mask.iter().sum::<f32>(), 0.0);
    assert_eq!(rows.gain_mask, vec![0.0]);
}

#[test]
fn labelled_tiers_count_real_free_tiers_with_a_finite_component() {
    // Sample 0: tier 0 (all four) and tier 2 (three of four) count, tier 1 is not free.
    // Sample 1: tier 0 has no label, tier 1 counts.
    assert_eq!(count_labelled_tiers(&hand_built().batch()), 3);
    assert_eq!(
        count_labelled_tiers(&Fixture::blank(5, MAX_TIERS).batch()),
        0
    );
}

#[test]
fn pack_imitation_uploads_states_masks_and_labels_in_index_order() {
    let fx = hand_built();
    let batch = fx.batch();
    let trainer = trainer(4, 0);
    let packed = trainer.pack_imitation(&batch, &[1, 0]);
    assert!(packed.has_labels);
    assert_eq!(packed.tiers.dims(), [2, 4, TIER_DIM]);
    assert_eq!(packed.globals.dims(), [2, GLOBAL_DIM]);

    let globals = host_vec(packed.globals).unwrap();
    assert_eq!(globals[..GLOBAL_DIM], fx.states[1][..GLOBAL_DIM]);
    assert_eq!(globals[GLOBAL_DIM..], fx.states[0][..GLOBAL_DIM]);
    let tiers = host_vec(packed.tiers).unwrap();
    assert_eq!(tiers[..4 * TIER_DIM], fx.states[1][GLOBAL_DIM..]);
    assert_eq!(tiers[4 * TIER_DIM..], fx.states[0][GLOBAL_DIM..]);
    // Sample 1 has 2 real tiers, sample 0 has 3.
    assert_eq!(
        host_vec(packed.mask).unwrap(),
        vec![1.0, 1.0, 0.0, 0.0, 1.0, 1.0, 1.0, 0.0]
    );

    let rows = label_rows(&batch, &[1, 0], 4);
    assert_eq!(packed.labels.sens_mask.dims(), [2, 4, N_COMPONENTS]);
    assert_eq!(host_vec(packed.labels.sens_mask).unwrap(), rows.sens_mask);
    assert_eq!(
        host_vec(packed.labels.sens_target).unwrap(),
        rows.sens_target
    );
    assert_eq!(host_vec(packed.labels.dir_mask).unwrap(), rows.dir_mask);
    assert_eq!(host_vec(packed.labels.dir_target).unwrap(), rows.dir_target);
    assert_eq!(host_vec(packed.labels.gain_mask).unwrap(), rows.gain_mask);
    assert_eq!(
        host_vec(packed.labels.gain_target).unwrap(),
        rows.gain_target
    );
    // Only sample 0, now the second row, has a gain.
    assert_eq!(rows.gain_mask, vec![0.0, 1.0]);
}

// Loss terms (hand values)

#[test]
fn sensitivity_mse_is_the_masked_mean_over_labelled_entries() {
    let hat = cube(vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0], [1, 2, 4]);
    let target = cube(vec![0.5, 0.0, 0.0, 0.0, 0.0, 0.0, 4.0, 0.0], [1, 2, 4]);
    // Entries 0 and 6 only: ((1 - 0.5)^2 + (7 - 4)^2) / 2 = 4.625; every other entry is ignored.
    let mask = cube(vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0], [1, 2, 4]);
    let mse = host_scalar(masked_sens_mse(hat.clone(), target.clone(), mask)).unwrap();
    assert!((mse - 4.625).abs() < 1e-6, "{mse}");

    let none = masked_sens_mse(hat, target, cube(vec![0.0; 8], [1, 2, 4]));
    assert_eq!(host_scalar(none).unwrap(), 0.0);
}

#[test]
fn cosine_averages_over_the_samples_that_have_a_direction() {
    // Sample 0 (tiers 0 and 1 masked): hat (1, 0) vs target (1, 1) -> 1 / sqrt(2); the huge
    // masked hat entry must not matter. Sample 1: nothing masked -> excluded. Sample 2: parallel.
    let hat = matrix(vec![1.0, 0.0, 99.0, 5.0, 5.0, 5.0, 2.0, -2.0, 0.0], 3, 3);
    let target = matrix(vec![1.0, 1.0, 7.0, 1.0, 2.0, 3.0, 1.0, -1.0, 0.0], 3, 3);
    let mask = matrix(vec![1.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0], 3, 3);
    let (cosine, loss) = masked_cosine(hat, target, mask);
    let want = f32::midpoint(FRAC_1_SQRT_2, 1.0);
    let (cosine, loss) = (host_scalar(cosine).unwrap(), host_scalar(loss).unwrap());
    assert!((cosine - want).abs() < 1e-5, "{cosine} vs {want}");
    assert!((loss - (1.0 - want)).abs() < 1e-5, "{loss}");
}

#[test]
fn cosine_of_opposite_directions_is_minus_one_and_an_empty_mask_is_exactly_zero() {
    let target = matrix(vec![1.0, 2.0], 1, 2);
    let opposite = matrix(vec![-1.0, -2.0], 1, 2);
    let both = matrix(vec![1.0; 2], 1, 2);
    let (cosine, loss) = masked_cosine(opposite.clone(), target.clone(), both);
    assert!((host_scalar(cosine).unwrap() + 1.0).abs() < 1e-5);
    assert!((host_scalar(loss).unwrap() - 2.0).abs() < 1e-5);

    let (cosine, loss) = masked_cosine(opposite, target, matrix(vec![0.0; 2], 1, 2));
    assert_eq!(host_scalar(cosine).unwrap(), 0.0);
    assert_eq!(host_scalar(loss).unwrap(), 0.0);
}

#[test]
fn huber_is_quadratic_below_the_knee_linear_above_and_masked() {
    let hat = matrix(vec![0.5, 3.0, 10.0, 7.0, -1.5], 5, 1);
    let target = matrix(vec![0.0, 0.5, 2.0, 0.0, 0.0], 5, 1);
    // Errors 0.5 -> 0.125, 2.5 -> 2.0, (masked), 7 -> 6.5, 1.5 -> 1.0; mean of four = 2.40625.
    let mask = matrix(vec![1.0, 1.0, 0.0, 1.0, 1.0], 5, 1);
    let loss = host_scalar(masked_huber(hat, target, mask)).unwrap();
    assert!((loss - 2.406_25).abs() < 1e-5, "{loss}");

    let one = |v: f32| matrix(vec![v], 1, 1);
    // Exactly at the knee both branches give 0.5; an empty mask gives exactly 0.
    let knee = masked_huber(one(1.0), one(0.0), one(1.0));
    assert!((host_scalar(knee).unwrap() - 0.5).abs() < 1e-6);
    let none = masked_huber(one(4.0), one(0.0), one(0.0));
    assert_eq!(host_scalar(none).unwrap(), 0.0);
}

// The trainer

#[test]
fn refused_input_never_touches_the_model() {
    let mut trainer = trainer(MAX_TIERS, 1);
    let before = export(&trainer);
    let config = ImitationConfig::default();

    // An invalid config is refused whether or not the batch is empty.
    let bad = ImitationConfig {
        mini_batch_size: 0,
        ..config.clone()
    };
    for fx in [
        Fixture::from_teacher(4, MAX_TIERS, 11),
        Fixture::blank(0, MAX_TIERS),
    ] {
        let result = trainer.train_imitation_epochs(&fx.batch(), &bad);
        assert!(matches!(result, Err(TrainError::InvalidConfig(_))));
    }
    // A malformed sample is named; a batch padded for another tier count is a shape error too.
    let mut fx = Fixture::from_teacher(4, MAX_TIERS, 11);
    fx.sensitivities[2].pop();
    let result = trainer.train_imitation_epochs(&fx.batch(), &config);
    assert!(matches!(&result, Err(TrainError::ShapeMismatch(m)) if m.contains("sample 2")));
    let other = Fixture::from_teacher(4, MAX_TIERS - 1, 11);
    let result = trainer.train_imitation_epochs(&other.batch(), &config);
    assert!(matches!(result, Err(TrainError::ShapeMismatch(_))));

    assert_eq!(trainer.update_count(), 0);
    assert_eq!(export(&trainer), before);
}

#[test]
fn an_empty_or_all_masked_batch_is_a_no_op() {
    // Every label NaN: nothing to learn from. The mini-batches are skipped (an all-zero gradient
    // would still move Adam's weights by its momentum), so there is no update and no change.
    let mut trainer = trainer(MAX_TIERS, 1);
    let before = export(&trainer);
    for count in [0, 8] {
        let fx = Fixture::blank(count, MAX_TIERS);
        let metrics = trainer
            .train_imitation_epochs(&fx.batch(), &ImitationConfig::default())
            .unwrap();
        assert_eq!(metrics, ImitationMetrics::default(), "{count} samples");
    }
    assert_eq!(trainer.update_count(), 0);
    assert_eq!(export(&trainer), before);
}

#[test]
fn a_batch_with_only_gains_trains_the_gain_head_and_reports_zero_for_the_rest() {
    let mut fx = Fixture::blank(8, MAX_TIERS);
    for (i, gain) in fx.oracle_gain.iter_mut().enumerate() {
        *gain = 2.0 + i as f32;
    }
    let mut trainer = trainer(MAX_TIERS, 1);
    let before = export(&trainer);
    let config = single_step(8);
    let metrics = trainer
        .train_imitation_epochs(&fx.batch(), &config)
        .unwrap();
    assert_eq!(metrics.updates, 1);
    assert_eq!(metrics.labelled_tiers, 0);
    assert_eq!(metrics.sens_mse, 0.0);
    assert_eq!(metrics.cosine, 0.0);
    assert!(metrics.gain_loss > 0.5, "gain loss {}", metrics.gain_loss);
    let gain_term = config.gain_coeff * metrics.gain_loss;
    assert!(
        (metrics.loss - gain_term).abs() < 1e-5,
        "loss {} vs gain term {gain_term}",
        metrics.loss
    );
    assert!(metrics.grad_norm.is_finite() && metrics.grad_norm > 0.0);
    // No NaN leaked into the weights through the empty sensitivity and direction terms.
    let after = export(&trainer);
    assert_ne!(after, before);
    assert!(after.sens_head.weights.iter().all(|w| w.is_finite()));
    assert!(after.tier_proj.weights.iter().all(|w| w.is_finite()));
}

#[test]
fn unlabelled_samples_are_ignored_and_the_labelled_ones_still_train() {
    let mut fx = Fixture::from_teacher(8, MAX_TIERS, 11);
    fx.clear_labels(2);
    fx.clear_labels(5);
    let expected: usize = (0..8)
        .filter(|&i| i != 2 && i != 5)
        .map(|i| fx.n_tiers[i])
        .sum();
    let mut trainer = trainer(MAX_TIERS, 1);
    let metrics = trainer
        .train_imitation_epochs(&fx.batch(), &single_step(8))
        .unwrap();
    assert_eq!(metrics.updates, 1);
    assert_eq!(metrics.labelled_tiers, expected);
    assert!(all_finite(&metrics), "{metrics:?}");
    assert!(metrics.sens_mse > 0.0 && metrics.grad_norm > 0.0);
}

#[test]
fn labels_of_tiers_that_are_not_free_do_not_change_the_result() {
    let mut pinned = Fixture::from_teacher(8, MAX_TIERS, 11);
    let mut garbage = Fixture::from_teacher(8, MAX_TIERS, 11);
    for free in pinned.free_tiers.iter_mut().chain(&mut garbage.free_tiers) {
        free[1] = false;
    }
    for sens in &mut garbage.sensitivities {
        sens[1] = [100.0, -50.0, 25.0, 12.0];
    }
    let config = ImitationConfig {
        mini_batch_size: 4,
        ..ImitationConfig::default()
    };
    let (mut first, mut second) = (trainer(MAX_TIERS, 3), trainer(MAX_TIERS, 3));
    let run_a = first
        .train_imitation_epochs(&pinned.batch(), &config)
        .unwrap();
    let run_b = second
        .train_imitation_epochs(&garbage.batch(), &config)
        .unwrap();
    assert_eq!(run_a, run_b);
    assert_eq!(export(&first), export(&second));
}

#[test]
fn a_diverged_loss_is_reported_and_the_step_is_skipped() {
    let mut fx = Fixture::from_teacher(4, MAX_TIERS, 11);
    // (3e30)^2 overflows f32, so the regression loss is infinite.
    fx.sensitivities[0][0] = [3.0e30, 0.0, 0.0, 0.0];
    let mut trainer = trainer(MAX_TIERS, 1);
    let before = export(&trainer);
    let result = trainer.train_imitation_epochs(&fx.batch(), &ImitationConfig::default());
    assert!(
        matches!(result, Err(TrainError::NonFinite(_))),
        "{result:?}"
    );
    assert_eq!(trainer.update_count(), 0);
    assert_eq!(export(&trainer), before);
}

#[test]
fn the_hand_built_batch_trains_and_counts_its_labelled_tiers() {
    let fx = hand_built();
    let mut trainer = trainer(4, 0);
    let metrics = trainer
        .train_imitation_epochs(&fx.batch(), &ImitationConfig::default())
        .unwrap();
    // Default: 2 epochs, one mini-batch of the 2 samples each.
    assert_eq!(metrics.updates, 2);
    assert_eq!(metrics.labelled_tiers, 3);
    assert!(all_finite(&metrics), "{metrics:?}");
    assert!(metrics.sens_mse > 0.0 && metrics.gain_loss > 0.0);
}

#[test]
fn epochs_and_mini_batches_set_the_number_of_updates() {
    let fx = Fixture::from_teacher(10, MAX_TIERS, 11);
    let total_tiers: usize = fx.n_tiers.iter().sum();

    // 3 epochs x ceil(10 / 4) = 9 optimizer steps; the labelled tiers are counted once.
    let mut small = trainer(MAX_TIERS, 1);
    let config = ImitationConfig {
        mini_batch_size: 4,
        epochs_per_round: 3,
        ..ImitationConfig::default()
    };
    let metrics = small.train_imitation_epochs(&fx.batch(), &config).unwrap();
    assert_eq!(metrics.updates, 9);
    assert_eq!(small.update_count(), 9);
    assert_eq!(metrics.labelled_tiers, total_tiers);

    // A mini-batch larger than the batch is one step per epoch.
    let mut large = trainer(MAX_TIERS, 1);
    let default = ImitationConfig::default();
    let metrics = large.train_imitation_epochs(&fx.batch(), &default).unwrap();
    assert_eq!(metrics.updates, 2);
    assert_eq!(metrics.labelled_tiers, total_tiers);
}

#[test]
fn the_learning_rate_schedule_follows_the_trainer_update_counter() {
    let fx = Fixture::from_teacher(8, MAX_TIERS, 11);
    let config = ImitationConfig {
        learning_rate: 1e-2,
        lr_schedule: LrSchedule::Linear { total_updates: 1 },
        ..single_step(8)
    };
    let mut trainer = trainer(MAX_TIERS, 2);
    let start = export(&trainer);
    trainer
        .train_imitation_epochs(&fx.batch(), &config)
        .unwrap();
    let after_first = export(&trainer);
    assert_ne!(after_first, start, "the first update runs at the full rate");

    // Update index 1 is past the schedule's end: the rate is exactly zero, the weights stay put.
    trainer
        .train_imitation_epochs(&fx.batch(), &config)
        .unwrap();
    assert_eq!(export(&trainer), after_first);
    assert_eq!(trainer.update_count(), 2);
}

#[test]
fn thirty_updates_fit_a_fixed_teacher() {
    // 32 random Balanced states, labels from a teacher network of another seed. The metrics of a
    // call are the figures the update saw before its step, so the last entry is the state after
    // 29 updates.
    let fx = Fixture::from_teacher(32, MAX_TIERS, 11);
    let config = ImitationConfig {
        learning_rate: 1e-3,
        ..single_step(32)
    };
    let mut trainer = trainer(MAX_TIERS, 3);
    let history: Vec<ImitationMetrics> = (0..30)
        .map(|_| {
            trainer
                .train_imitation_epochs(&fx.batch(), &config)
                .unwrap()
        })
        .collect();

    assert!(history.iter().all(all_finite), "{history:?}");
    assert!(history.iter().all(|m| m.updates == 1));
    assert_eq!(trainer.update_count(), 30);
    let (first, last) = (history[0], history[29]);
    assert!(first.sens_mse > 0.0 && first.grad_norm > 0.0);
    assert!(
        last.sens_mse <= 0.5 * first.sens_mse,
        "sens_mse {} -> {}",
        first.sens_mse,
        last.sens_mse
    );
    assert!(
        last.cosine > first.cosine,
        "cosine {} -> {}",
        first.cosine,
        last.cosine
    );
    assert!(
        last.loss < first.loss,
        "loss {} -> {}",
        first.loss,
        last.loss
    );
}

#[test]
fn the_same_seed_gives_the_same_metrics_and_weights() {
    let fx = Fixture::from_teacher(24, MAX_TIERS, 11);
    let config = ImitationConfig {
        learning_rate: 1e-3,
        mini_batch_size: 8,
        epochs_per_round: 2,
        ..ImitationConfig::default()
    };
    let run = |seed: u64| {
        let mut trainer = trainer(MAX_TIERS, seed);
        let metrics = trainer
            .train_imitation_epochs(&fx.batch(), &config)
            .unwrap();
        (metrics, export(&trainer))
    };
    let (metrics_a, weights_a) = run(7);
    let (metrics_b, weights_b) = run(7);
    assert_eq!(metrics_a, metrics_b);
    assert_eq!(weights_a, weights_b);
    assert_eq!(metrics_a.updates, 6);

    // Another shuffle seed visits the mini-batches in another order.
    let (metrics_c, _) = run(8);
    assert_ne!(metrics_a, metrics_c);
}
