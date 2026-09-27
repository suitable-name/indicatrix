//! Adapter-free unit tests for the pure arithmetic scattered across this module tree:
//! chunk sizing, time-budgeted sizing, environment-parameter mapping, and the wavefront
//! per-tuple byte cap. None of these touch a real GPU device -- see
//! `gpu_hardware_tests` for the tests that do.

use super::*;
use crate::{
    optics::raytracer::{LightingPreset, illuminant_temperature_k},
    renderer::buffers::transport_env_mode,
};
use bind_groups::{WAVEFRONT_STOKES_BYTES_PER_TUPLE, wavefront_pixel_cap};
use dispatch::{chunk_pixels_for, environment_params};
use readback::staging_needs_growth;

/// [`staging_needs_growth`] must say "grow" exactly when the current capacity is too
/// small, and "keep" both when it matches and when strictly larger (never shrink),
/// mirroring [`GpuFrameRenderer::ensure_capacity`]'s identical check for `outputs`.
#[test]
fn staging_needs_growth_only_when_undersized() {
    // Too small: must grow.
    assert!(staging_needs_growth(0, 1));
    assert!(staging_needs_growth(99, 100));
    // Exactly enough: must NOT grow.
    assert!(!staging_needs_growth(100, 100));
    // Already larger than required: must NOT shrink either.
    assert!(!staging_needs_growth(1_000, 100));
}

#[test]
fn chunking_divides_the_budget_by_samples_per_pixel() {
    // 12 bytes per tuple (XYZ only), so a 120-byte budget is exactly 10 tuples: 10
    // pixels at 1 spp, 5 at 2 spp, 3 at 3 spp (integer division, never rounding up).
    let budget = 10 * FLOATS_PER_TUPLE * size_of::<f32>();
    assert_eq!(chunk_pixels_for(budget, 1, 10_000), 10);
    assert_eq!(chunk_pixels_for(budget, 2, 10_000), 5);
    assert_eq!(chunk_pixels_for(budget, 3, 10_000), 3);
}

#[test]
fn a_frame_smaller_than_the_budget_is_one_chunk() {
    let pixels = 800 * 600;
    assert_eq!(chunk_pixels_for(CHUNK_BUDGET_BYTES, 1, pixels), pixels);
}

/// A budget too small for even one tuple must still dispatch one pixel rather than
/// returning zero, which would loop forever on a zero-width chunk.
#[test]
fn a_budget_below_one_tuple_still_yields_a_chunk() {
    assert_eq!(chunk_pixels_for(0, 4, 10_000), 1);
    assert_eq!(chunk_pixels_for(8, 1, 10_000), 1);
}

/// With no measurement yet (`ema = None`), the first chunk(s) of a fresh renderer
/// must be capped at [`FIRST_DISPATCH_MAX_TUPLES`], never the full byte budget -- a
/// cold integrated GPU's first dispatch must not alone trip a TDR watchdog.
#[test]
fn first_dispatch_is_capped_regardless_of_byte_budget() {
    let byte_budget_pixels = chunk_pixels_for(CHUNK_BUDGET_BYTES, 4, 10_000_000);
    let picked = GpuFrameRenderer::next_chunk_pixels(None, byte_budget_pixels, 4, 10_000_000);
    assert!(picked <= FIRST_DISPATCH_MAX_TUPLES / 4);
    assert!(picked <= byte_budget_pixels);
}

/// Once a measurement exists, a fast-GPU EMA (tiny ns/tuple) must still never exceed
/// the byte-budget ceiling -- `chunk_budget_bytes` stays a hard upper bound.
#[test]
fn time_budgeted_sizing_never_exceeds_the_byte_budget() {
    let byte_budget_pixels = chunk_pixels_for(CHUNK_BUDGET_BYTES, 1, 10_000_000);
    let picked = GpuFrameRenderer::next_chunk_pixels(
        Some(1.0e-6), // absurdly fast: 1 tuple per picosecond
        byte_budget_pixels,
        1,
        10_000_000,
    );
    assert!(picked <= byte_budget_pixels);
}

/// A slow-GPU EMA (large ns/tuple) must still dispatch at least the [`MIN_CHUNK_BYTES`]
/// floor's worth of pixels (clamped to the byte budget) rather than shrinking toward
/// zero, so fixed per-dispatch overhead can never dominate.
#[test]
fn time_budgeted_sizing_never_shrinks_below_the_floor() {
    let num_pixels = 10_000_000;
    let byte_budget_pixels = chunk_pixels_for(CHUNK_BUDGET_BYTES, 1, num_pixels);
    let min_pixels = chunk_pixels_for(MIN_CHUNK_BYTES, 1, num_pixels);
    let picked = GpuFrameRenderer::next_chunk_pixels(
        Some(1.0e9), // absurdly slow: 1 second per tuple
        byte_budget_pixels,
        1,
        num_pixels,
    );
    assert_eq!(picked, min_pixels);
}

/// A forced byte budget SMALLER than [`MIN_CHUNK_BYTES`] (as
/// [`run_chunk_equivalence`] uses to force many small chunks) must still win: the
/// byte budget is the one bound that can never be exceeded, even when it is below
/// the timing floor that would otherwise apply.
#[test]
fn a_byte_budget_smaller_than_the_timing_floor_still_wins() {
    let num_pixels = 10_000;
    let tiny_budget_bytes = 700 * FLOATS_PER_TUPLE * size_of::<f32>();
    let byte_budget_pixels = chunk_pixels_for(tiny_budget_bytes, 2, num_pixels);
    for ema in [Some(1.0e-6), Some(1.0e9), None] {
        let picked = GpuFrameRenderer::next_chunk_pixels(ema, byte_budget_pixels, 2, num_pixels);
        assert!(
            picked <= byte_budget_pixels,
            "byte budget must never be exceeded (ema={ema:?}, picked={picked}, \
             byte_budget_pixels={byte_budget_pixels})"
        );
    }
}

#[test]
fn studio_environments_map_onto_the_studio_rig_mode() {
    let (mode, temp_k, spot_mult, exposure, yaw, pitch, use_d65, studio_model, backdrop) =
        environment_params(LightingPreset::Daylight.studio(1.5, 0.4, 0.35));
    assert_eq!(mode, transport_env_mode::STUDIO_RIG);
    assert_eq!(temp_k, illuminant_temperature_k(LightingPreset::Daylight));
    assert_eq!(spot_mult, LightingPreset::Daylight.params().spot_mult);
    assert_eq!((exposure, yaw, pitch), (1.5, 0.4, 0.35));
    // The Daylight preset must route through the D65 table on the GPU too -- see
    // GpuTransportParams::studio_use_d65's doc comment.
    assert!(use_d65);
    assert_eq!(studio_model, 0);
    assert_eq!(backdrop, 0.0);
}

/// An HDR map is a SUPPORTED environment (`env_mode ==
/// transport_env_mode::HDR_MAP`), not a decline -- the studio-rig fields are simply
/// unused by that branch (see [`environment_params`]'s own doc comment), not left at
/// some other environment's stale values.
#[test]
fn hdr_environments_map_onto_the_hdr_map_mode() {
    let map = crate::renderer::env_map::EnvironmentMap::uniform(4, 2, [1.0, 1.0, 1.0]);
    let (mode, temp_k, spot_mult, exposure, yaw, pitch, use_d65, studio_model, backdrop) =
        environment_params(crate::optics::raytracer::EnvironmentSource::HdrMap(&map));
    assert_eq!(mode, transport_env_mode::HDR_MAP);
    assert_eq!(
        (temp_k, spot_mult, exposure, yaw, pitch),
        (0.0, 0.0, 0.0, 0.0, 0.0)
    );
    assert!(!use_d65);
    assert_eq!(studio_model, 0);
    assert_eq!(backdrop, 0.0);
}

// `every_builtin_routes_the_way_gpu_supported_says` does not belong here:
// `GemMaterial::gpu_supported` is unconditionally `true` (see its own doc
// comment), so a test that only calls `material.gpu_supported()` directly asserts a
// predicate that cannot fail, regardless of whether `accumulate_turn_body`'s own
// enforcement of it (a few functions up, `if !scene.material.gpu_supported()`) is
// wired correctly. Making it meaningful would mean actually dispatching
// `accumulate_turn_body` for a material `gpu_supported` declines, which needs a real
// adapter (this module's other adapter-free tests, above, deliberately stay under
// plain `cargo test`) -- no such material exists today to dispatch with anyway,
// since the predicate is unconditionally `true`. If a future material type the
// megakernel cannot handle is ever added, an adapter-gated check belongs in
// `examples/gpu_equivalence_harness.rs` alongside this module's other real-hardware
// self-tests, not here.

/// [`wavefront_pixel_cap`] with a FAKE `max_storage_buffer_binding_size`
/// -- no real adapter needed (unlike [`GpuFrameRenderer::cap_byte_budget_for_wavefront`]
/// itself, which reads a live device's limits). A generous fake limit (128 MiB, the
/// WebGPU baseline) must not cap a small chunk at all; a tiny one (one tuple's worth
/// of `stokes` exactly) must cap to exactly 1 pixel at `spp == 1` and stay >= 1 even
/// at large `spp` (never 0, which would make a later chunk-sizing division panic).
#[test]
fn wavefront_pixel_cap_respects_the_binding_limit() {
    let generous = 128 * 1024 * 1024;
    assert_eq!(wavefront_pixel_cap(generous, 4, 1_000), 1_000);

    let one_tuple = WAVEFRONT_STOKES_BYTES_PER_TUPLE;
    assert_eq!(wavefront_pixel_cap(one_tuple, 1, 1_000), 1);
    // At spp=4, one tuple's worth of binding room is not even one whole pixel's 4
    // samples -- `pixel_cap`'s inner `.max(1)` floors `tuple_cap / spp` at 1 tuple,
    // and the outer `.max(1)` floors the PIXEL cap at 1 pixel either way.
    assert_eq!(wavefront_pixel_cap(one_tuple, 4, 1_000), 1);

    // A pathological zero-byte fake limit must still return a usable (degenerate)
    // cap, never 0.
    assert_eq!(wavefront_pixel_cap(0, 8, 1_000), 1);

    // The requested `byte_budget_pixels` ceiling still wins when it is SMALLER than
    // what the binding limit would allow.
    assert_eq!(wavefront_pixel_cap(generous, 1, 3), 3);
}
