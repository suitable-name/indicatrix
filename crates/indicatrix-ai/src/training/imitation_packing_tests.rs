use super::*;

// Packing

#[test]
fn label_rows_follow_the_masking_rules() {
    let fx = hand_built();
    let rows = label_rows(&fx.batch(), &[0, 1], 4);

    // Sample 0: tier 1 is not free (masked although finite), tier 2 lacks one component, tier 3
    // is padding. Sample 1: tier 0 has no label at all, tiers 2 and 3 are padding. A missing
    // label (NaN, including the three unmeasured components of every row) is a zero target
    // under a zero mask.
    let o = 0.0_f32;
    let zeros = [o; N_COMPONENTS];
    let labelled = |values: [f32; 4]| -> [f32; N_COMPONENTS] {
        let mut row = zeros;
        row[..4].copy_from_slice(&values);
        row
    };
    let want_target = [
        labelled([1.0, 2.0, 3.0, 4.0]).as_slice(),
        zeros.as_slice(),
        labelled([0.5, 0.0, -1.5, 2.5]).as_slice(),
        zeros.as_slice(),
        zeros.as_slice(),
        labelled([0.1, 0.2, 0.3, 0.4]).as_slice(),
        zeros.as_slice(),
        zeros.as_slice(),
    ]
    .concat();
    assert_eq!(rows.sens_target, want_target);
    let mut want_mask = vec![0.0; 2 * 4 * N_COMPONENTS];
    want_mask[..4].fill(1.0);
    want_mask[2 * N_COMPONENTS] = 1.0;
    want_mask[2 * N_COMPONENTS + 2..2 * N_COMPONENTS + 4].fill(1.0);
    want_mask[5 * N_COMPONENTS..5 * N_COMPONENTS + 4].fill(1.0);
    assert_eq!(rows.sens_mask, want_mask);

    // Normalize over finite actionable slopes, with the shared score-points/degree floor.
    let direction =
        |slope: f32, max: f32| (slope / max.max(crate::env::SLOPE_NORM_FLOOR)) * -MEAN_BOUND;
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
    assert!(rows.sample_has_labels.iter().any(|&has| has));
}

#[test]
fn label_rows_of_an_unlabelled_batch_report_no_labels() {
    let fx = Fixture::blank(4, MAX_TIERS);
    let rows = label_rows(&fx.batch(), &[0, 1, 2, 3], MAX_TIERS);
    assert!(!rows.sample_has_labels.iter().any(|&has| has));
    assert!(rows.sens_target.iter().all(|&v| v == 0.0));
    assert!(rows.dir_target.iter().all(|&v| v == 0.0));
    assert!(rows.gain_target.iter().all(|&v| v == 0.0));
}

#[test]
fn infinite_labels_are_treated_as_missing() {
    let mut fx = Fixture::blank(1, MAX_TIERS);
    fx.sensitivities[0][0] = component_row([f32::INFINITY, 1.0, f32::NEG_INFINITY, f32::NAN]);
    fx.score_slopes[0][0] = f32::INFINITY;
    fx.oracle_gain[0] = f32::INFINITY;
    let rows = label_rows(&fx.batch(), &[0], MAX_TIERS);
    assert_eq!(rows.sens_mask.iter().sum::<f32>(), 1.0);
    assert_eq!(rows.sens_target[1], 1.0);
    assert_eq!(rows.dir_mask.iter().sum::<f32>(), 0.0);
    assert_eq!(rows.gain_mask, vec![0.0]);
}

#[test]
fn mobility_masks_component_support_and_teacher_direction_labels() {
    let mut fx = Fixture::blank(1, MAX_TIERS);
    fx.free_tiers[0].fill(true);
    fx.sensitivities[0][0] = component_row([1.0, 2.0, 3.0, 4.0]);
    fx.score_slopes[0][0] = 2.0;
    fx.states[0][GLOBAL_DIM + crate::env::MOBILITY_INDEX] = 0.0;
    let rows = label_rows(&fx.batch(), &[0], MAX_TIERS);
    assert_eq!(rows.sens_mask.iter().sum::<f32>(), 0.0);
    assert_eq!(rows.dir_mask.iter().sum::<f32>(), 0.0);
    assert_eq!(
        count_component_support(&fx.batch(), &[0]),
        [0; N_COMPONENTS]
    );
}

#[test]
fn labelled_tiers_count_real_free_tiers_with_a_finite_component() {
    // Sample 0: tier 0 (all four) and tier 2 (three of four) count, tier 1 is not free.
    // Sample 1: tier 0 has no label, tier 1 counts.
    assert_eq!(count_labelled_tiers(&hand_built().batch(), &[0, 1]), 3);
    assert_eq!(
        count_labelled_tiers(&Fixture::blank(5, MAX_TIERS).batch(), &[0, 1, 2, 3, 4]),
        0
    );
}

#[test]
fn pack_imitation_uploads_states_masks_and_labels_in_index_order() {
    let fx = hand_built();
    let batch = fx.batch();
    let trainer = trainer(4, 0);
    let packed = trainer.pack_imitation(&batch, &[1, 0]);
    assert!(packed.row_has_labels.iter().any(|&has| has));
    // The window is padded to the longest design of the selection (3 real tiers), not to the
    // trainer's 4 slots: the states' fourth slot is dropped.
    let width = 3;
    assert_eq!(packed.tiers.dims(), [2, width, TIER_DIM]);
    assert_eq!(packed.globals.dims(), [2, GLOBAL_DIM]);

    let globals = host_vec(packed.globals).unwrap();
    assert_eq!(globals[..GLOBAL_DIM], fx.states[1][..GLOBAL_DIM]);
    assert_eq!(globals[GLOBAL_DIM..], fx.states[0][..GLOBAL_DIM]);
    let tiers = host_vec(packed.tiers).unwrap();
    let kept = GLOBAL_DIM..GLOBAL_DIM + width * TIER_DIM;
    assert_eq!(tiers[..width * TIER_DIM], fx.states[1][kept.clone()]);
    assert_eq!(tiers[width * TIER_DIM..], fx.states[0][kept]);
    // Sample 1 has 2 real tiers, sample 0 has 3.
    assert_eq!(
        host_vec(packed.mask).unwrap(),
        vec![1.0, 1.0, 0.0, 1.0, 1.0, 1.0]
    );

    let rows = label_rows(&batch, &[1, 0], width);
    assert_eq!(packed.labels.sens_mask.dims(), [2, width, N_COMPONENTS]);
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

#[path = "imitation_loss_tests.rs"]
mod loss_tests;
