// ---------------------------------------------------------------------------------
// Struct layouts -- must match `renderer::buffers` field-for-field (see that module's
// doc comment on why a hand-derived offset is never trusted without the echo test).
// ---------------------------------------------------------------------------------

struct GpuCameraParams {
    origin: vec3<f32>,
    fov_tan: f32,
    forward: vec3<f32>,
    width: f32,
    right: vec3<f32>,
    height: f32,
    up: vec3<f32>,
    num_samples: u32,
}

struct GpuTransportParams {
    num_pixels: u32,
    max_bounces: u32,
    sample_offset: u32,
    env_mode: u32,
    l0: f32,
    studio_temp_k: f32,
    studio_spot_mult: f32,
    studio_exposure: f32,
    studio_light_yaw: f32,
    studio_light_pitch: f32,
    pixel_offset: u32,
    // Reused pad field: gates `transport_main`'s three per-channel debug writes.
    write_debug_buffers: u32,
    white_balance: vec3<f32>,
    // Reused pad field: selects the tabulated CIE D65 measured spectrum over
    // `blackbody_spectrum`.
    studio_use_d65: u32,
    studio_model: u32,
    // `EnvironmentSource::Studio::backdrop`: the card the camera ray sees where it
    // misses the stone, 0.0 for none.
    backdrop: f32,
    // `EnvironmentSource::Studio::surface_glare`: scale of the camera path's first-surface
    // specular reflection, 1.0 for no change.
    surface_glare: f32,
    // `EnvironmentSource::Studio::head_shadow_deg` as two cosines (`head_shadow_cosines`).
    head_shadow_outer_cos: f32,
    head_shadow_inner_cos: f32,
    // `TentParams::flat`, in the first former pad slot (offset 84).
    tent_flat: f32,
    _pad_head_shadow1: u32,
    _pad_head_shadow2: u32,
    // `LightingRigParams::tent` (`TentParams`): the light-tent model's per-preset knobs.
    tent_walls: f32,
    tent_cards: f32,
    tent_spark: f32,
    tent_ground: f32,
}

struct DispersionParams {
    model_type: u32,
    param_a: vec4<f32>,
    param_b: vec4<f32>,
    param_c: vec4<f32>,
    c_axis_and_birefringence: vec4<f32>,
    is_anisotropic: u32,
    biaxial_delta_beta_alpha: f32,
    has_biaxial_delta: u32,
}

struct GpuGemMaterial {
    dispersion: DispersionParams,
    crystal_system: u32,
    optical_character: u32,
    is_pleochroic: u32,
    o_ray_band_count: u32,
    e_ray_band_count: u32,
    o_ray_bands: array<AbsorptionBand, 8>,
    e_ray_bands: array<AbsorptionBand, 8>,
    scattering_sigma_s: f32,
    scattering_g: f32,
    edge_rounding_radius: f32,
    // Appended fields below are always added at the end, never inserted earlier, so no
    // existing field's offset ever shifts -- see `renderer::buffers::GpuGemMaterial`'s
    // doc comment.
    has_beta_ray: u32,
    beta_ray_band_count: u32,
    beta_ray_bands: array<AbsorptionBand, 8>,
    absorption_path_scale: f32,
    has_extraordinary_dispersion: u32,
    extraordinary_model_type: u32,
    extraordinary_param_a: vec4<f32>,
    extraordinary_param_b: vec4<f32>,
}

struct FacetPlane {
    normal: vec3<f32>,
    d: f32,
}

// HdrEnvDims and GpuDistDims are defined in transport_physics.wgsl.

@group(0) @binding(0) var<uniform> camera: GpuCameraParams;
@group(0) @binding(1) var<uniform> params: GpuTransportParams;
@group(0) @binding(2) var<storage, read> material: GpuGemMaterial;
@group(0) @binding(3) var<storage, read> planes: array<FacetPlane>;
@group(0) @binding(4) var<storage, read_write> out_xyz: array<f32>;
@group(0) @binding(5) var<storage, read_write> out_radiance: array<f32>;
@group(0) @binding(6) var<storage, read_write> out_lambdas: array<f32>;
@group(0) @binding(7) var<storage, read_write> out_path_pdf: array<f32>;
// `optics::raytracer::FacetFinish`, one entry per `planes[i]`, PARALLEL to `planes` (a
// separate binding, not a widened `FacetPlane` -- see `renderer::buffers::facet_finish`'s
// doc comment for why). `facet_finish::FROSTED` (1u) routes that facet's bounce through
// `apply_frosted_bounce` instead of the polished TIR/reflect/refract dispatch below; any
// other value (including an out-of-bounds index) is `facet_finish::POLISHED` (0u).
@group(0) @binding(8) var<storage, read> facet_finishes: array<u32>;
// One `compat[k]` narrowed-family bitmask per channel, per (pixel, sample) tuple --
// same `write_debug_buffers`-gated, self-test-only contract as `out_radiance`/
// `out_lambdas`/`out_path_pdf` above.
@group(0) @binding(9) var<storage, read_write> out_compat: array<u32>;
// `optics::raytracer::environment::EnvironmentSource::HdrMap`'s GPU-side
// texel storage -- row-major `vec4<f32>` (alpha unused/zero), uploaded by
// `renderer::env_map_gpu::HdrEnvGpuData::upload`. See that module's doc comment for why
// these two bindings are always present regardless of `params.env_mode`.
@group(0) @binding(10) var<storage, read> hdr_texels: array<vec4<f32>>;
@group(0) @binding(11) var<uniform> hdr_env_dims: HdrEnvDims;
// renderer::env_map_gpu::HdrEnvGpuData's flattened Distribution2D -- see
// that module's own doc comment ("Bindings 12/13/14's layout") for the exact layout.
@group(0) @binding(12) var<storage, read> dist_func: array<f32>;
@group(0) @binding(13) var<storage, read> dist_cdf: array<f32>;
@group(0) @binding(14) var<uniform> dist_dims: GpuDistDims;

const FACET_FINISH_FROSTED: u32 = 1u;

// Register-pressure/divergence reduction -- workgroup-shared plane
// cache. `intersect_ray` (below) is the single hottest per-thread loop in this kernel:
// every thread re-walks `planes[]` from a storage buffer on EVERY bounce (up to
// `params.max_bounces` times), even though every thread in a workgroup is reading the
// exact same array (one gemstone's facet planes, shared across the whole dispatch).
// When the polyhedron has at most `PLANES_SHARED_CAPACITY` facets, `transport_main`
// cooperatively copies `planes[]` into this workgroup-shared array ONCE at kernel
// start (one plane per invocation, looping by `workgroup_size` strides, then a single
// `workgroupBarrier()`), and every `intersect_ray` call for the rest of the dispatch
// reads `planes_shared` instead of re-issuing a storage load per bounce per thread.
//
// Bit-identical by construction: `plane_at` below reads the exact same `FacetPlane`
// values in the exact same ascending index order either way -- `planes_shared[i]` is
// filled from the exact storage read `planes[i]` would otherwise perform, so which
// address space serves the read never changes a single bit of the result.
//
// A polyhedron with more than `PLANES_SHARED_CAPACITY` facets (rare -- comfortably
// above every existing cut gemstone's facet count) reads `planes[]` directly instead.
// The fallback decision (`plane_count <=
// PLANES_SHARED_CAPACITY`) is uniform across the entire workgroup -- `arrayLength(&planes)`
// is a dispatch-wide constant, never a per-thread quantity -- so branching on it is
// uniform control flow, not divergence, and is safe on both sides of the
// `workgroupBarrier()` in `transport_main` below.
const PLANES_SHARED_CAPACITY: u32 = 128u;

var<workgroup> planes_shared: array<FacetPlane, 128>;

// Reads plane `i` from whichever address space `transport_main` populated this
// dispatch for -- see `planes_shared`'s own doc comment above.
fn plane_at(i: u32, plane_count: u32) -> FacetPlane {
    if (plane_count <= PLANES_SHARED_CAPACITY) {
        return planes_shared[i];
    }
    return planes[i];
}
