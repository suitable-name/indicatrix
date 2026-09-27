//! Tests for [`LiveEpoch`] (`SampleCursor`'s own tests moved with it to
//! `indicatrix-dispatch`), plus helpers other `bridge` tests reuse.

use super::*;
use glam::Vec3;
use indicatrix_net::{
    client::Accumulator,
    messages::{FrameHeader, StreamEvent},
    radiance,
};
use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
};

/// Asserts `sorted` (sorted by start) tiles `[start, end)` exactly: adjacent, non-empty,
/// no overlap, no gap, and every index seen exactly once.
pub(in crate::bridge) fn assert_partition(sorted: &[(u32, u32)], start: u32, end: u32) {
    let mut pos = start;
    for (s, c) in sorted {
        assert_eq!(*s, pos, "gap or overlap at {s} (expected {pos})");
        assert!(*c > 0, "a claimed range must never be empty");
        pos += *c;
    }
    assert_eq!(
        pos, end,
        "the ranges must cover the whole budget exactly once"
    );
    let mut seen = HashSet::new();
    for (s, c) in sorted {
        for idx in *s..(*s + *c) {
            assert!(seen.insert(idx), "sample index {idx} was claimed twice");
        }
    }
}

// ---- LiveEpoch ----------------------------------------------------------------------

/// Applies one synthetic FRAME delta of `value` per pixel covering `[first, first +
/// samples)` to `acc` (as the connection thread would).
pub(in crate::bridge) fn apply_frame(
    acc: &Mutex<Accumulator>,
    request_id: u32,
    first: u32,
    samples: u32,
    value: f32,
) {
    let mut acc = acc.lock().unwrap();
    let (w, h) = acc.dimensions();
    let buf = vec![Vec3::splat(value); (w * h) as usize];
    let bytes = radiance::encode(&buf);
    let header = FrameHeader::for_payload(request_id, first, samples, &bytes);
    acc.apply(&StreamEvent::Frame(header), Some(&bytes))
        .expect("well-formed frame");
}

#[test]
fn a_live_epoch_merges_finished_chunks_and_counts_the_in_flight_one() {
    let epoch = LiveEpoch::new(2, 1, 64);
    let (start, count) = epoch.claim_remote(8).unwrap();
    let chunk = Arc::new(Mutex::new(Accumulator::new(2, 1)));
    chunk
        .lock()
        .unwrap()
        .begin_request_for_range(1, start, count);
    epoch.begin_chunk(start, count, Arc::clone(&chunk));

    apply_frame(&chunk, 1, start, 3, 1.0);
    assert_eq!(epoch.remote_done(), 3, "in-flight progress counts");
    let (buf, n) = epoch.remote_snapshot();
    assert_eq!(n, 3);
    assert_eq!(buf, vec![Vec3::ONE; 2]);

    apply_frame(&chunk, 1, start + 3, 5, 2.0);
    let end = epoch.finish_chunk().unwrap();
    assert_eq!(
        end,
        ChunkEnd {
            start,
            count,
            done: 8
        }
    );
    assert_eq!(end.remainder(), (start + 8, 0));
    assert_eq!(epoch.remote_done(), 8);
    let mut dst = vec![Vec3::splat(10.0); 2];
    assert_eq!(epoch.add_remote_into(&mut dst), 8);
    assert_eq!(dst, vec![Vec3::splat(13.0); 2]);
}

#[test]
fn a_failed_chunk_merges_only_its_prefix_and_reports_the_remainder() {
    let epoch = LiveEpoch::new(1, 1, 64);
    let (start, count) = epoch.claim_remote(10).unwrap();
    let chunk = Arc::new(Mutex::new(Accumulator::new(1, 1)));
    chunk
        .lock()
        .unwrap()
        .begin_request_for_range(9, start, count);
    epoch.begin_chunk(start, count, Arc::clone(&chunk));
    apply_frame(&chunk, 9, start, 4, 1.0);
    let end = epoch.finish_chunk().unwrap();
    assert_eq!(end.done, 4);
    assert_eq!(end.remainder(), (start + 4, 6));
    assert_eq!(epoch.remote_done(), 4);
}

#[test]
fn an_abandoned_chunk_contributes_nothing() {
    let epoch = LiveEpoch::new(1, 1, 64);
    let chunk = Arc::new(Mutex::new(Accumulator::new(1, 1)));
    chunk.lock().unwrap().begin_request_for_range(2, 0, 8);
    epoch.begin_chunk(0, 8, Arc::clone(&chunk));
    apply_frame(&chunk, 2, 0, 5, 1.0);
    assert!(epoch.abandon_chunk());
    assert_eq!(epoch.remote_done(), 0);
    assert_eq!(epoch.remote_snapshot(), (vec![Vec3::ZERO], 0));
    assert!(epoch.finish_chunk().is_none());
}

#[test]
fn add_remote_into_refuses_a_mismatched_buffer_rather_than_miscounting() {
    let epoch = LiveEpoch::new(2, 2, 16);
    let mut wrong = vec![Vec3::ZERO; 3];
    assert_eq!(epoch.add_remote_into(&mut wrong), 0);
}
