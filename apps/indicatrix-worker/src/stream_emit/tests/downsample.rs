//! Tests for [`downsample_preview`]'s reduced-resolution snapshot.

use crate::stream_emit::downsample::downsample_preview;
use glam::Vec3;

#[test]
fn downsample_preview_produces_the_requested_dimensions() {
    let buf = vec![Vec3::ONE; 8 * 8];
    let out = downsample_preview(&buf, 8, 8, 2, 2);
    assert_eq!(out.len(), 4);
}

#[test]
fn downsample_preview_of_a_uniform_buffer_preserves_the_value() {
    let buf = vec![Vec3::new(2.0, 4.0, 6.0); 8 * 8];
    let out = downsample_preview(&buf, 8, 8, 2, 2);
    for v in out {
        assert!((v - Vec3::new(2.0, 4.0, 6.0)).length() < 1e-5);
    }
}
