//! Tests for [`PendingDelta`]'s coalescing and swap behavior.

use super::fixtures::tiny_scene;
use crate::{render_core, stream_emit::emitter::PendingDelta};
use glam::Vec3;

#[test]
fn coalesced_deltas_sum_identically_to_un_coalesced_ones() {
    let scene = tiny_scene();
    let pixel_count = scene.width as usize * scene.height as usize;

    let a = render_core::trace_samples(&scene, 0, 3, 1);
    let b = render_core::trace_samples(&scene, 3, 5, 1);

    let mut pending = PendingDelta::new(pixel_count);
    pending.add(0, 3, &a);
    pending.add(3, 5, &b);
    let mut spare = vec![Vec3::ZERO; pixel_count];
    let (first_sample, samples) = pending.swap_with(&mut spare).unwrap();
    assert_eq!(first_sample, 0);
    assert_eq!(samples, 8);
    let coalesced = spare;

    let direct = render_core::trace_samples(&scene, 0, 8, 1);
    for (c, d) in coalesced.iter().zip(&direct) {
        let diff = (*c - *d).abs();
        let scale = c.abs().max(d.abs()).max(Vec3::splat(1e-6));
        assert!((diff / scale).max_element() < 1e-3, "c={c:?} d={d:?}");
    }
}

#[test]
fn pending_delta_swap_returns_none_when_empty() {
    let mut pending = PendingDelta::new(16);
    let mut spare = vec![Vec3::ZERO; 16];
    assert!(pending.swap_with(&mut spare).is_none());
}

#[test]
fn pending_delta_is_empty_again_immediately_after_a_swap() {
    let scene = tiny_scene();
    let pixel_count = scene.width as usize * scene.height as usize;
    let a = render_core::trace_samples(&scene, 0, 2, 1);

    let mut pending = PendingDelta::new(pixel_count);
    pending.add(0, 2, &a);
    let mut spare_a = vec![Vec3::ZERO; pixel_count];
    assert!(pending.swap_with(&mut spare_a).is_some());
    let mut spare_b = vec![Vec3::ZERO; pixel_count];
    assert!(pending.swap_with(&mut spare_b).is_none());
}
