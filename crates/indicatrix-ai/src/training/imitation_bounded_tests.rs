//! Tests of the bounded imitation update (`updates_per_round > 0`), a child of `imitation_tests`
//! so it shares the fixtures.

use super::*;

#[test]
fn the_bounded_mode_takes_exactly_n_updates_whatever_the_batch_size() {
    // (batch size, mini-batch size): larger than, equal to and smaller than one mini-batch.
    for (count, mini_batch) in [(40, 8), (8, 8), (3, 8), (1, 256)] {
        let fx = Fixture::from_teacher(count, MAX_TIERS, 11);
        let mut trainer = trainer(MAX_TIERS, 1);
        let metrics = trainer
            .train_imitation_epochs(&fx.batch(), &bounded(5, mini_batch))
            .unwrap();
        assert_eq!(metrics.updates, 5, "{count} samples");
        assert_eq!(trainer.update_count(), 5, "{count} samples");
        assert!(all_finite(&metrics), "{metrics:?}");
    }
}

#[test]
fn a_batch_with_no_labels_takes_no_bounded_update() {
    let fx = Fixture::blank(8, MAX_TIERS);
    let mut trainer = trainer(MAX_TIERS, 1);
    let metrics = trainer
        .train_imitation_epochs(&fx.batch(), &bounded(5, 4))
        .unwrap();
    assert!(metrics.packing_ms.is_finite() && metrics.packing_ms >= 0.0);
    assert!(metrics.update_ms.is_finite() && metrics.update_ms >= 0.0);
    assert_eq!(metric_core(metrics), ImitationMetrics::default());
    assert_eq!(trainer.update_count(), 0);
}

#[test]
fn draws_are_deterministic_unique_and_in_range() {
    let draw = |seed: u64| {
        let mut rng = Prng::new(seed);
        let mut order: Vec<usize> = (0..50).collect();
        let mut batches = Vec::new();
        for _ in 0..4 {
            draw_minibatch(&mut rng, &mut order, 12);
            batches.push(order[..12].to_vec());
        }
        batches
    };
    let first = draw(9);
    assert_eq!(first, draw(9));
    assert_ne!(first, draw(10));
    for batch in &first {
        let mut sorted = batch.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 12, "no repeats inside a mini-batch");
        assert!(sorted.iter().all(|&i| i < 50));
    }
    // A batch asked for beyond the sample count is the whole permutation.
    let mut rng = Prng::new(1);
    let mut order: Vec<usize> = (0..5).collect();
    draw_minibatch(&mut rng, &mut order, 5);
    order.sort_unstable();
    assert_eq!(order, vec![0, 1, 2, 3, 4]);
}

#[test]
fn the_bounded_mode_is_reproducible_and_seed_dependent() {
    let fx = Fixture::from_teacher(24, MAX_TIERS, 11);
    let config = bounded(6, 8);
    let run = |seed: u64| {
        let mut trainer = trainer(MAX_TIERS, seed);
        let metrics = trainer
            .train_imitation_epochs(&fx.batch(), &config)
            .unwrap();
        (metrics, export(&trainer))
    };
    let (metrics_a, weights_a) = run(7);
    let (metrics_b, weights_b) = run(7);
    assert_eq!(metric_core(metrics_a), metric_core(metrics_b));
    assert_eq!(weights_a, weights_b);
    assert_eq!(metrics_a.updates, 6);
    let (metrics_c, _) = run(8);
    assert_ne!(metric_core(metrics_a), metric_core(metrics_c));
}

#[test]
fn zero_updates_per_round_is_the_old_epoch_loop() {
    // The epoch path must equal the pre-change loop written out by hand on a twin trainer.
    let fx = Fixture::from_teacher(10, MAX_TIERS, 11);
    let config = ImitationConfig {
        mini_batch_size: 4,
        epochs_per_round: 3,
        ..epoch_config()
    };
    let mut via_call = trainer(MAX_TIERS, 5);
    let metrics = via_call
        .train_imitation_epochs(&fx.batch(), &config)
        .unwrap();

    let mut by_hand = trainer(MAX_TIERS, 5);
    let batch = fx.batch();
    let mut order: Vec<usize> = (0..batch.len()).collect();
    let mut sum = UpdateStats::default();
    let mut updates = 0usize;
    for _ in 0..config.epochs_per_round {
        by_hand.shuffle.shuffle(&mut order);
        for chunk in order.chunks(4) {
            let prepared = by_hand.pack_imitation(&batch, chunk);
            let local_rows: Vec<usize> = (0..chunk.len()).collect();
            let stats = by_hand
                .imitation_update(&prepared, &local_rows, &config)
                .unwrap()
                .unwrap();
            add_stats(&mut sum, &stats);
            updates += 1;
        }
    }
    assert_eq!(metrics.updates, updates);
    assert_eq!(metrics.loss, sum.loss / updates as f32);
    assert_eq!(export(&via_call), export(&by_hand));
}
