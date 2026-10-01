//! Recombination, stale-drop and planner tests.

use super::*;
use crate::scene::{
    CameraSpec, FinishSpec, LightingSpec, MaterialSpec, OwnedScene, SceneSpec, planes_to_data,
};
use indicatrix::{
    geometry::cuts::StandardGemCuts, optics::raytracer::LightingPreset, render_setup::Backdrop,
};

const WIDTH: u32 = 13;
const HEIGHT: u32 = 7;

fn scene(finishes: FinishSpec, material: &str) -> OwnedScene {
    let planes = StandardGemCuts::standard_round_brilliant();
    OwnedScene::build(
        &SceneSpec {
            planes: planes_to_data(&planes),
            finishes,
            material: MaterialSpec::catalogue(material),
            camera: CameraSpec {
                yaw: 0.35,
                pitch: 0.28,
                distance: 5.0,
            },
            lighting: LightingSpec::new(LightingPreset::LightTent, 1.0, 0.4, 0.35, Backdrop::Grey),
            max_bounces: 6,
            width: WIDTH,
            height: HEIGHT,
            hdr_id: None,
        },
        None,
    )
    .expect("fixture scene builds")
}

fn bits(buf: &[Vec3]) -> Vec<[u32; 3]> {
    buf.iter()
        .map(|v| [v.x.to_bits(), v.y.to_bits(), v.z.to_bits()])
        .collect()
}

/// The passes the simulated pool traces: uneven sizes, like a planner's ramp.
const PASSES: [(u32, u32); 3] = [(0, 1), (1, 3), (4, 2)];

/// Simulates `workers` Workers tracing every pass, delivering the chunks to the
/// accumulator in a scrambled order (later passes and higher partitions first).
fn accumulate_via_workers(scene: &OwnedScene, workers: u32) -> Accumulator {
    let mut acc = Accumulator::new(9, WIDTH, HEIGHT, workers);
    let mut chunks = Vec::new();
    for &(offset, spp) in &PASSES {
        for partition in 0..workers {
            let sums =
                handle_trace_chunk(scene, scene.plane_soa(), partition, workers, offset, spp);
            chunks.push((partition, offset, spp, sums));
        }
    }
    // Reverse delivery: every pass arrives before its predecessors are merged.
    for (partition, offset, spp, sums) in chunks.into_iter().rev() {
        let outcome = acc.add_chunk(9, partition, workers, offset, spp, sums);
        assert!(
            matches!(
                outcome,
                ChunkOutcome::Pending | ChunkOutcome::Committed { .. }
            ),
            "{outcome:?}"
        );
    }
    acc
}

/// The single-caller reference: the whole frame (stride 1) per pass, added in pass
/// order -- what one thread tracing everything would produce.
fn stride_one_reference(scene: &OwnedScene) -> Vec<Vec3> {
    let mut reference = vec![Vec3::ZERO; (WIDTH * HEIGHT) as usize];
    for &(offset, spp) in &PASSES {
        let sums = handle_trace_chunk(scene, scene.plane_soa(), 0, 1, offset, spp);
        for (dst, src) in reference.iter_mut().zip(sums) {
            *dst += src;
        }
    }
    reference
}

#[test]
fn simulated_workers_recombine_bit_for_bit_against_a_stride_one_trace() {
    let scene = scene(FinishSpec::AllPolished, "Diamond");
    let reference = bits(&stride_one_reference(&scene));
    for workers in [1, 3, 8] {
        let acc = accumulate_via_workers(&scene, workers);
        assert_eq!(
            acc.completed_passes(),
            PASSES.len() as u32,
            "{workers} workers"
        );
        assert_eq!(acc.sample_count(), 6);
        assert_eq!(acc.pending_passes(), 0);
        assert_eq!(bits(acc.sum()), reference, "{workers} workers");
    }
}

/// Against the desktop's own CPU estimator, `renderer::gpu::hybrid::cpu_accumulate`
/// (`gpu`-gated, so native only), called once per pass in pass order.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn simulated_workers_recombine_bit_for_bit_against_cpu_accumulate() {
    for (finishes, material) in [
        (FinishSpec::AllPolished, "Diamond"),
        (FinishSpec::FrostedGirdle, "Zircon"),
    ] {
        let scene = scene(finishes, material);
        let mut reference = vec![Vec3::ZERO; (WIDTH * HEIGHT) as usize];
        for &(offset, spp) in &PASSES {
            indicatrix::renderer::gpu::hybrid::cpu_accumulate(
                &scene.frame_scene(),
                offset,
                spp,
                &mut reference,
            );
        }
        for workers in [2, 5, 7] {
            let acc = accumulate_via_workers(&scene, workers);
            assert_eq!(
                bits(acc.sum()),
                bits(&reference),
                "{material}, {workers} workers"
            );
        }
    }
}

/// Row groups are finer interleaved partitions of one chunk: every pixel's sum is the
/// same bits as one whole trace, however many groups there are.
#[test]
fn a_chunk_traced_in_row_groups_equals_one_trace_bit_for_bit() {
    let scene = scene(FinishSpec::AllPolished, "Diamond");
    let range = ChunkRange {
        first_pixel: 1,
        stride: 3,
        sample_offset: 2,
        spp: 2,
    };
    let whole = bits(&handle_trace_chunk(&scene, scene.plane_soa(), 1, 3, 2, 2));
    assert_eq!(whole.len(), 30, "a third of the 91 pixels, from the second");
    for slices in [1, 2, 5, 31, 500] {
        let grouped =
            trace_chunk_in_slices(&scene, range, slices, &|| false).expect("never aborted");
        assert_eq!(bits(&grouped), whole, "{slices} groups");
    }
    let automatic = trace_chunk_abortable(&scene, range, &|| false).expect("never aborted");
    assert_eq!(bits(&automatic), whole);
}

#[test]
fn an_aborted_chunk_returns_nothing_and_stops_at_the_first_no() {
    let scene = scene(FinishSpec::AllPolished, "Diamond");
    let range = ChunkRange {
        first_pixel: 0,
        stride: 2,
        sample_offset: 0,
        spp: 1,
    };
    let asked = std::cell::Cell::new(0);
    let result = trace_chunk_in_slices(&scene, range, 4, &|| {
        asked.set(asked.get() + 1);
        asked.get() > 2
    });
    assert_eq!(result, None);
    assert_eq!(asked.get(), 3, "two groups traced, the third refused");
    // A chunk of one group is asked once, before any work.
    assert_eq!(trace_chunk_in_slices(&scene, range, 1, &|| true), None);
}

#[test]
fn row_group_counts_follow_the_work_and_stay_in_bounds() {
    assert_eq!(slice_count(0, 4), 1);
    assert_eq!(slice_count(100, 1), 1, "under one group's worth of work");
    assert_eq!(slice_count(1024, 4), 4);
    assert_eq!(slice_count(2_000_000, 1), MAX_SLICES);
    assert_eq!(slice_count(3, 4096), 3, "never more groups than pixels");
}

#[test]
fn an_undelivered_chunk_is_handed_out_again() {
    let mut planner = ChunkPlanner::new(2, 1_000, 256);
    let first = planner.next_chunk(0, 0).expect("first chunk");
    planner.unassign(&first);
    assert_eq!(planner.next_chunk(0, 0), Some(first), "the same pass again");
    // Only a partition's latest assignment can be taken back.
    let second = planner.next_chunk(0, 1).expect("next pass");
    assert_eq!(second.pass_index, 1);
    planner.unassign(&first);
    assert_eq!(planner.next_chunk(0, 1).map(|c| c.pass_index), Some(2));
}

#[test]
fn a_pass_is_merged_only_when_every_partition_arrived() {
    let scene = scene(FinishSpec::AllPolished, "Diamond");
    let mut acc = Accumulator::new(1, WIDTH, HEIGHT, 2);
    let chunk = |p: u32, offset: u32, spp: u32| {
        handle_trace_chunk(&scene, scene.plane_soa(), p, 2, offset, spp)
    };
    // Pass 1 complete, pass 0 half: nothing merges.
    assert_eq!(
        acc.add_chunk(1, 0, 2, 1, 2, chunk(0, 1, 2)),
        ChunkOutcome::Pending
    );
    assert_eq!(
        acc.add_chunk(1, 1, 2, 1, 2, chunk(1, 1, 2)),
        ChunkOutcome::Pending
    );
    assert_eq!(
        acc.add_chunk(1, 1, 2, 0, 1, chunk(1, 0, 1)),
        ChunkOutcome::Pending
    );
    assert_eq!(acc.sample_count(), 0);
    assert!(acc.mean().iter().all(|v| *v == Vec3::ZERO));
    // The last piece of pass 0 merges both passes at once.
    assert_eq!(
        acc.add_chunk(1, 0, 2, 0, 1, chunk(0, 0, 1)),
        ChunkOutcome::Committed { passes: 2 }
    );
    assert_eq!(acc.sample_count(), 3);
    assert_eq!(acc.completed_passes(), 2);
    let mean = acc.mean();
    for (m, s) in mean.iter().zip(acc.sum()) {
        assert_eq!(*m, *s * (1.0 / 3.0));
    }
}

#[test]
fn stale_and_malformed_chunks_are_dropped() {
    let scene = scene(FinishSpec::AllPolished, "Diamond");
    let mut acc = Accumulator::new(5, WIDTH, HEIGHT, 2);
    let sums = handle_trace_chunk(&scene, scene.plane_soa(), 0, 2, 0, 1);
    assert_eq!(
        acc.add_chunk(4, 0, 2, 0, 1, sums.clone()),
        ChunkOutcome::Stale
    );
    assert_eq!(
        acc.add_chunk(5, 0, 3, 0, 1, sums.clone()),
        ChunkOutcome::Rejected(ChunkRejection::BadPartition)
    );
    assert_eq!(
        acc.add_chunk(5, 2, 2, 0, 1, sums.clone()),
        ChunkOutcome::Rejected(ChunkRejection::BadPartition)
    );
    assert!(matches!(
        acc.add_chunk(5, 1, 2, 0, 1, sums.clone()),
        ChunkOutcome::Rejected(ChunkRejection::WrongLength { .. })
    ));
    assert_eq!(
        acc.add_chunk(5, 0, 2, 0, 0, Vec::new()),
        ChunkOutcome::Rejected(ChunkRejection::Empty)
    );
    assert_eq!(
        acc.add_chunk(5, 0, 2, 0, 1, sums.clone()),
        ChunkOutcome::Pending
    );
    assert_eq!(
        acc.add_chunk(5, 0, 2, 0, 1, sums.clone()),
        ChunkOutcome::Rejected(ChunkRejection::Duplicate)
    );
    let other = handle_trace_chunk(&scene, scene.plane_soa(), 1, 2, 0, 2);
    assert_eq!(
        acc.add_chunk(5, 1, 2, 0, 2, other),
        ChunkOutcome::Rejected(ChunkRejection::SppMismatch)
    );
    let rest = handle_trace_chunk(&scene, scene.plane_soa(), 1, 2, 0, 1);
    assert_eq!(
        acc.add_chunk(5, 1, 2, 0, 1, rest),
        ChunkOutcome::Committed { passes: 1 }
    );
    assert_eq!(
        acc.add_chunk(5, 0, 2, 0, 1, sums),
        ChunkOutcome::Rejected(ChunkRejection::AlreadyCommitted)
    );
    // Nothing stale or rejected leaked into the sum.
    assert_eq!(acc.sample_count(), 1);
}

#[test]
fn partition_lengths_cover_the_frame_exactly() {
    for (pixels, stride) in [(91, 1), (91, 3), (91, 8), (5, 8), (0, 4)] {
        let total: u32 = (0..stride).map(|p| partition_len(pixels, p, stride)).sum();
        assert_eq!(total, pixels, "{pixels} pixels / {stride}");
    }
    assert_eq!(partition_len(10, 0, 0), 0);
}

#[test]
fn the_first_pass_is_one_sample_and_later_passes_are_sized_from_the_slowest_worker() {
    // 2 partitions of 50_000 pixels.
    let mut planner = ChunkPlanner::new(2, 100_000, 256);
    let a = planner.next_chunk(0, 0).expect("first chunk");
    assert_eq!(
        (a.pass_index, a.sample_offset, a.spp, a.stride),
        (0, 0, 1, 2)
    );
    let b = planner
        .next_chunk(1, 0)
        .expect("same pass, other partition");
    assert_eq!((b.pass_index, b.sample_offset, b.spp), (0, 0, 1));
    // Worker 0: 50 ms for 50_000 pixel-samples = 0.001 ms each; worker 1 twice as slow.
    planner.record_timing(0, 1, 50.0);
    planner.record_timing(1, 1, 100.0);
    // Slowest: 0.002 ms * 50_000 px = 100 ms per spp -> 2 spp for 200 ms.
    let c = planner.next_chunk(0, 0).expect("second pass");
    assert_eq!((c.pass_index, c.sample_offset, c.spp), (1, 1, 2));
    // Lookahead: partition 0 is now two passes ahead of the merged count (0).
    assert_eq!(planner.next_chunk(0, 0), None);
    assert!(planner.next_chunk(0, 1).is_some());
}

#[test]
fn growth_is_capped_and_the_last_pass_lands_on_the_target() {
    let mut planner = ChunkPlanner::new(1, 1_000, 20);
    assert_eq!(planner.next_chunk(0, 0).map(|c| c.spp), Some(1));
    // Very fast: sizing alone would ask for MAX_CHUNK_SPP.
    planner.record_timing(0, 1, 0.001);
    assert_eq!(
        planner.next_chunk(0, 1).map(|c| c.spp),
        Some(4),
        "4x growth cap"
    );
    assert_eq!(
        planner.next_chunk(0, 2).map(|c| c.spp),
        Some(15),
        "cut to the target"
    );
    assert_eq!(planner.planned_spp(), 20);
    assert_eq!(planner.next_chunk(0, 3), None);
    assert!(!planner.is_done(2));
    assert!(planner.is_done(3));
    planner.set_target_spp(22);
    assert_eq!(
        planner.next_chunk(0, 3).map(|c| (c.sample_offset, c.spp)),
        Some((20, 2))
    );
}

#[test]
fn rates_carry_over_to_the_next_scene_but_the_first_pass_stays_one_sample() {
    let mut old = ChunkPlanner::new(2, 1_000, 256);
    old.record_timing(0, 4, 20.0);
    let mut new = ChunkPlanner::with_rates_from(&old, 2, 1_000, 256);
    assert_eq!(new.rate(0), old.rate(0));
    assert_eq!(new.next_chunk(0, 0).map(|c| c.spp), Some(1));
    // Sized from the carried rate (40 spp for 200 ms), capped at 4x the first pass.
    assert_eq!(new.next_chunk(0, 1).map(|c| c.spp), Some(4));
    let resized = ChunkPlanner::with_rates_from(&old, 3, 1_000, 256);
    assert_eq!(resized.rate(0), None);
    // A nonsensical timing is ignored.
    old.record_timing(1, 0, 5.0);
    old.record_timing(1, 1, f64::NAN);
    assert_eq!(old.rate(1), None);
}

#[test]
fn spp_targets_clamp_to_the_owner_limits() {
    assert_eq!(clamp_live_spp(1), LIVE_MIN_SPP);
    assert_eq!(clamp_live_spp(99_999), LIVE_MAX_SPP);
    assert_eq!(clamp_live_spp(DEFAULT_LIVE_SPP), 256);
    assert_eq!(clamp_export_spp(0), EXPORT_MIN_SPP);
    assert_eq!(clamp_export_spp(1 << 20), EXPORT_MAX_SPP);
}
