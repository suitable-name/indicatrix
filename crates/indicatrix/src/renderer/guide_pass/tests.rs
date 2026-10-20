use super::*;
use crate::geometry::cuts::StandardGemCuts;

#[test]
fn generate_guide_buffers_is_correctly_sized_and_facet_bounded() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let camera = Camera::new(0.60, 0.45, 2.4, 42.0);
    let guides = generate_guide_buffers(16, 12, &camera, &planes);

    assert_eq!(guides.depth.len(), 16 * 12);
    assert_eq!(guides.normal.len(), 16 * 12);
    assert_eq!(guides.facet_id.len(), 16 * 12);

    // Looking straight at a centred gem from a reasonable distance, the centre
    // pixel must hit *some* facet, and every recorded facet id must be a valid
    // index into `planes` (or the -1 "miss" sentinel).
    let centre = 6 * 16 + 8;
    assert!(
        guides.facet_id[centre] >= 0,
        "centre pixel should hit the gem"
    );
    for &id in &guides.facet_id {
        assert!(
            id == -1 || usize::try_from(id).is_ok_and(|index| index < planes.len()),
            "facet id {id} out of range for {} planes",
            planes.len()
        );
    }
}

#[test]
fn generate_guide_buffers_handles_zero_area_without_panicking() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let camera = Camera::new(0.0, 0.0, 2.4, 42.0);
    let guides = generate_guide_buffers(0, 0, &camera, &planes);
    assert_eq!(guides.depth.len(), 0);
    assert_eq!(guides.normal.len(), 0);
    assert_eq!(guides.facet_id.len(), 0);
}

#[test]
fn a_miss_pixel_gets_the_sentinel_depth_and_facet_id() {
    // A ray aimed far off the gem's silhouette misses every plane.
    let planes = StandardGemCuts::standard_round_brilliant();
    // Pull the camera far back and look at a corner of a tiny image so at least
    // the corner pixels miss.
    let camera = Camera::new(0.0, 1.5, 50.0, 5.0);
    let guides = generate_guide_buffers(4, 4, &camera, &planes);
    assert!(
        guides.facet_id.contains(&-1),
        "expected at least one miss pixel at this camera distance/fov"
    );
    for (i, &id) in guides.facet_id.iter().enumerate() {
        if id == -1 {
            assert_eq!(guides.depth[i].to_bits(), 1.0e6_f32.to_bits());
            assert_eq!(guides.normal[i], Vec3::ZERO);
        }
    }
}

#[test]
fn generate_guide_buffers_cancellable_matches_the_non_cancellable_version_when_never_cancelled() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let camera = Camera::new(0.60, 0.45, 2.4, 42.0);
    let cancel = AtomicBool::new(false);

    let expected = generate_guide_buffers(16, 12, &camera, &planes);
    let actual = generate_guide_buffers_cancellable(16, 12, &camera, &planes, &cancel)
        .expect("an AtomicBool that's never set true must never yield a cancelled result");

    assert_eq!(actual.depth, expected.depth);
    assert_eq!(actual.facet_id, expected.facet_id);
}

/// The single-thread path (the only one `wasm32` ever takes) must produce exactly
/// the buffers the threaded native path does, for thread counts that split the rows
/// unevenly and for one larger than the row count.
#[test]
fn single_thread_and_multi_thread_paths_produce_identical_guides() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let camera = Camera::new(0.60, 0.45, 2.4, 42.0);
    let cancel = AtomicBool::new(false);
    let job = GuideJob {
        width: 21,
        height: 13,
        camera: &camera,
        planes: &planes,
        tools: &[],
        n_d: 0.0,
        cancel: &cancel,
    };
    let single = generate_with_threads(job, 1).expect("never cancelled");
    assert!(
        single.facet_id.iter().any(|&id| id >= 0),
        "fixture must hit the gem somewhere"
    );
    let bits = |g: &GuideBuffers| -> Vec<u32> {
        g.depth
            .iter()
            .copied()
            .map(f32::to_bits)
            .chain(g.normal.iter().flat_map(|n| n.to_array().map(f32::to_bits)))
            .collect()
    };
    for threads in [2, 3, 7, 64] {
        let multi = generate_with_threads(job, threads).expect("never cancelled");
        assert_eq!(bits(&multi), bits(&single), "{threads} threads");
        assert_eq!(multi.facet_id, single.facet_id, "{threads} threads");
    }
}

/// Regenerating into a buffer that still holds another pose's (and another size's)
/// guides yields exactly the freshly allocated result.
#[test]
fn generating_into_a_reused_buffer_matches_a_fresh_allocation() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let cancel = AtomicBool::new(false);
    let old_camera = Camera::new(1.10, 0.20, 2.4, 42.0);
    let camera = Camera::new(0.60, 0.45, 2.4, 42.0);

    let mut reused = generate_guide_buffers(24, 16, &old_camera, &planes);
    assert!(generate_guide_buffers_into(
        16,
        12,
        &camera,
        &planes,
        &cancel,
        &mut reused
    ));
    let fresh = generate_guide_buffers(16, 12, &camera, &planes);

    assert_eq!(reused.facet_id, fresh.facet_id);
    assert_eq!(reused.depth.len(), fresh.depth.len());
    for (a, b) in reused.depth.iter().zip(&fresh.depth) {
        assert_eq!(a.to_bits(), b.to_bits());
    }
    assert_eq!(reused.normal, fresh.normal);
}

#[test]
fn generate_guide_buffers_cancellable_returns_none_when_pre_cancelled() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let camera = Camera::new(0.60, 0.45, 2.4, 42.0);
    let cancel = AtomicBool::new(true);

    let result = generate_guide_buffers_cancellable(64, 64, &camera, &planes, &cancel);
    assert!(
        result.is_none(),
        "a generation cancelled before it starts must not produce buffers"
    );
}

#[test]
fn guide_buffers_with_no_tools_match_the_plane_only_entry_point_bit_for_bit() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let camera = Camera::new(0.60, 0.45, 2.4, 42.0);
    let old = generate_guide_buffers(16, 12, &camera, &planes);
    let new = generate_guide_buffers_geom(16, 12, &camera, StoneGeometry::planes_only(&planes));
    assert_eq!(old.facet_id, new.facet_id);
    let bits = |b: &GuideBuffers| -> Vec<u32> { b.depth.iter().map(|d| d.to_bits()).collect() };
    assert_eq!(bits(&old), bits(&new));
}

/// No index (the old entry points) means no signature at all, and the other three
/// buffers are the indexed run's, bit for bit.
#[test]
fn without_an_index_the_signature_is_all_zeros_and_other_buffers_are_unchanged() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let camera = Camera::new(0.60, 0.45, 2.4, 42.0);
    let plain = generate_guide_buffers(24, 16, &camera, &planes);
    assert_eq!(plain.path_sig, vec![0; 24 * 16]);
    let zero = generate_guide_buffers_with_index(24, 16, &camera, &planes, 0.0);
    assert_eq!(zero.path_sig, plain.path_sig);
    let indexed = generate_guide_buffers_with_index(24, 16, &camera, &planes, 2.417);
    assert_eq!(indexed.facet_id, plain.facet_id);
    assert_eq!(indexed.normal, plain.normal);
    for (a, b) in indexed.depth.iter().zip(&plain.depth) {
        assert_eq!(a.to_bits(), b.to_bits());
    }
}

/// A miss pixel carries signature 0 and every gem pixel a non-zero one.
#[test]
fn signature_is_zero_exactly_on_misses() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let camera = Camera::new(0.60, 0.45, 2.4, 42.0);
    let guides = generate_guide_buffers_with_index(48, 32, &camera, &planes, 2.417);
    assert!(guides.facet_id.contains(&-1), "fixture needs a miss pixel");
    assert!(
        guides.facet_id.iter().any(|&id| id >= 0),
        "fixture needs a hit pixel"
    );
    for (i, &id) in guides.facet_id.iter().enumerate() {
        assert_eq!(guides.path_sig[i] == 0, id < 0, "pixel {i}, facet {id}");
    }
}

/// The signature, like the other buffers, does not depend on the thread count.
#[test]
fn signature_is_identical_for_one_two_and_eight_threads() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let camera = Camera::new(0.60, 0.45, 2.4, 42.0);
    let cancel = AtomicBool::new(false);
    let job = GuideJob {
        width: 61,
        height: 37,
        camera: &camera,
        planes: &planes,
        tools: &[],
        n_d: 2.417,
        cancel: &cancel,
    };
    let single = generate_with_threads(job, 1).expect("never cancelled");
    let distinct: std::collections::HashSet<u32> = single.path_sig.iter().copied().collect();
    assert!(
        distinct.len() > 4,
        "a brilliant seen at this pose must show several interior regions"
    );
    for threads in [2, 8] {
        let multi = generate_with_threads(job, threads).expect("never cancelled");
        assert_eq!(multi.path_sig, single.path_sig, "{threads} threads");
    }
}

/// The refraction depends on the index, so the regions move when it does.
#[test]
fn signature_regions_change_with_the_index() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let camera = Camera::new(0.60, 0.45, 2.4, 42.0);
    let diamond = generate_guide_buffers_with_index(64, 48, &camera, &planes, 2.417);
    let glass = generate_guide_buffers_with_index(64, 48, &camera, &planes, 1.52);
    assert_ne!(diamond.path_sig, glass.path_sig);
    assert_eq!(diamond.facet_id, glass.facet_id);
}

/// Reusing a buffer yields the fresh signature, including after a size change.
#[test]
fn signature_survives_buffer_reuse() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let cancel = AtomicBool::new(false);
    let old_camera = Camera::new(1.10, 0.20, 2.4, 42.0);
    let camera = Camera::new(0.60, 0.45, 2.4, 42.0);
    let mut reused = generate_guide_buffers_with_index(24, 16, &old_camera, &planes, 2.417);
    assert!(generate_guide_buffers_into_with_index(
        16,
        12,
        &camera,
        &planes,
        2.417,
        &cancel,
        &mut reused
    ));
    let fresh = generate_guide_buffers_with_index(16, 12, &camera, &planes, 2.417);
    assert_eq!(reused.path_sig, fresh.path_sig);
}

#[test]
fn guide_buffers_encode_tool_facet_ids_above_the_plane_count() {
    let planes = StandardGemCuts::standard_round_brilliant();
    // A groove across the table, seen from almost straight above.
    let tools = [ToolPrimitive::cylinder(
        Vec3::new(0.0, 0.44, 0.0),
        Vec3::X,
        0.3,
        2.0,
    )];
    let camera = Camera::new(0.0, 1.5, 4.0, 20.0);
    let guides = generate_guide_buffers_geom(
        24,
        24,
        &camera,
        StoneGeometry {
            planes: &planes,
            tools: &tools,
        },
    );
    let first_tool_id = i32::try_from(planes.len()).expect("the plane count fits i32");
    assert!(
        guides.facet_id.contains(&first_tool_id),
        "the groove floor must appear as tool facet {first_tool_id}"
    );
    assert!(guides.facet_id.iter().all(|&id| id <= first_tool_id));
}
