//! Deterministic rows: scheduling changes which thread owns a row, never its sum.
use super::gpu_backend::{BackendFrame, FrameOutputs};
use glam::Vec3;
use indicatrix::{
    optics::raytracer::add_finite_sample,
    renderer::whitted::{WhittedPrep, whitted_tap_xyz},
};
use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};

struct Row<'a> {
    accum: &'a mut [Vec3],
    depth: &'a mut [f32],
    normal: &'a mut [Vec3],
    facet: &'a mut [i32],
}

pub(super) fn render_frame(
    frame: &BackendFrame<'_>,
    prep: &WhittedPrep<'_>,
    planes: &indicatrix::simd::PlanesSoA32,
    outputs: &mut FrameOutputs<'_>,
) {
    let width = frame.width as usize;
    if width == 0 {
        return;
    }
    let FrameOutputs {
        accum,
        depth,
        normal,
        facet_id,
    } = outputs;
    let rows = Mutex::new(
        accum
            .chunks_mut(width)
            .zip(depth.chunks_mut(width))
            .zip(normal.chunks_mut(width))
            .zip(facet_id.chunks_mut(width))
            .map(|(((accum, depth), normal), facet)| {
                Some(Row {
                    accum,
                    depth,
                    normal,
                    facet,
                })
            })
            .collect::<Vec<_>>(),
    );
    let next = AtomicUsize::new(0);
    let scene = super::engine::frame_scene(frame);
    let threads = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
    std::thread::scope(|scope| {
        for _ in 0..threads {
            let (rows, next, planes, scene) = (&rows, &next, planes, &scene);
            scope.spawn(move || {
                loop {
                    let y = next.fetch_add(1, Ordering::Relaxed);
                    if y >= frame.height as usize {
                        break;
                    }
                    let row = rows
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)[y]
                        .take()
                        .expect("an atomic row index is claimed exactly once");
                    for x in 0..width {
                        let pixel = u32::try_from(y * width + x).expect("a frame pixel fits u32");
                        let mut sum = Vec3::ZERO;
                        let mut hit = None;
                        for tap in frame.sample_offset..frame.sample_offset + frame.spp {
                            add_finite_sample(
                                &mut sum,
                                whitted_tap_xyz(
                                    scene,
                                    frame.tools,
                                    planes,
                                    prep,
                                    pixel,
                                    tap,
                                    Some(&mut hit),
                                ),
                            );
                        }
                        row.accum[x] += sum;
                        row.depth[x] = hit.map_or(1.0e6, |value| value.t);
                        row.normal[x] = hit.map_or(Vec3::ZERO, |value| value.normal);
                        row.facet[x] = hit.map_or(-1, |value| {
                            i32::try_from(value.facet_idx).expect("facet index fits i32")
                        });
                    }
                }
            });
        }
    });
}
