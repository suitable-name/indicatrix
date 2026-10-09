// ---------------------------------------------------------------------------------
// Zoned absorption, unit 1 of 3: the zone table. `zoning` feature only.
//
// This file is NOT part of the default shader text. `build.rs` appends the three
// `shaders/zoning/*.wgsl` units to the transport shaders (and patches the two anchor
// sites named in `build.rs`) only when the crate is built with `--features zoning`.
//
// The structs below are the WGSL twins of `renderer::buffers::zoning`'s
// `GpuZoneHeader`/`GpuZoneShape`/`GpuZoneAbsorption`/`GpuZoneTable`. The table is a
// trailing field (`zones`) of `GpuGemMaterial`, appended at byte offset 576, so every
// earlier material field keeps its offset. Sizes: header 80, shape 112 (x4), absorption
// 400 (x4), table 2128. `renderer::gpu::layout_check::run_zone_table` proves the layout
// byte for byte against the Rust structs on a device.
//
// `AbsorptionBand` is declared in `transport_physics/03_dispersion_absorption_frosted.wgsl`.
// ---------------------------------------------------------------------------------

// Shape discriminants -- the `K_*` constants of `optics::zoning::kernels` (a unit test
// compares them).
const ZONE_KIND_NONE: u32 = 0u;
const ZONE_KIND_HALF: u32 = 1u;
const ZONE_KIND_SLAB: u32 = 2u;
const ZONE_KIND_CYL: u32 = 3u;
const ZONE_KIND_PRISM: u32 = 4u;
const ZONE_KIND_SECTOR: u32 = 5u;

// `GpuZoneShape::flags` bits.
const ZONE_FLAG_WIDE: u32 = 1u;
const ZONE_FLAG_FULL: u32 = 2u;

// Number of shaped zones the table holds (`optics::zoning::MAX_ZONES`).
const ZONE_MAX: u32 = 4u;

struct GpuZoneHeader {
    // Shaped zones in use; 0 means the material is unzoned for the shader.
    zone_count: u32,
    // Boundary softness in mm; 0 is sharp.
    softness: f32,
    // Gauss-Legendre pieces per boundary band (`renderer::buffers::GPU_SOFT_SUBDIV`).
    soft_subdiv: u32,
    _pad: u32,
    // Zone frame translation (.w unused).
    origin: vec4<f32>,
    // Rows of the transposed frame rotation (.w unused): local_i = row_i . (p - origin).
    row0: vec4<f32>,
    row1: vec4<f32>,
    row2: vec4<f32>,
}

struct GpuZoneShape {
    kind: u32,
    n_sides: u32,
    flags: u32,
    _pad: u32,
    // Axis point.
    p: vec4<f32>,
    // Plane normal or axis direction.
    d: vec4<f32>,
    // Axis reference directions.
    u: vec4<f32>,
    v: vec4<f32>,
    // Half space: x0 = offset. Slab: x0 = offset_min, x1 = offset_max. Tube/prism:
    // x0 = r_in, x1 = r_out, prism x2 = phase.
    x: vec4<f32>,
    // Sector: (e0.x, e0.y, e1.x, e1.y), the unit directions of angle_from / angle_to in
    // the (u, v) plane.
    e: vec4<f32>,
}

struct GpuZoneAbsorption {
    o_ray_band_count: u32,
    e_ray_band_count: u32,
    has_beta_ray: u32,
    beta_ray_band_count: u32,
    o_ray_bands: array<AbsorptionBand, 8>,
    e_ray_bands: array<AbsorptionBand, 8>,
    beta_ray_bands: array<AbsorptionBand, 8>,
}

struct GpuZoneTable {
    header: GpuZoneHeader,
    shapes: array<GpuZoneShape, 4>,
    absorption: array<GpuZoneAbsorption, 4>,
}
