// Shared per-bounce physics -- scene bindings, the megakernel's
// helper functions (intersection, NEE, environment sampling, the biaxial/uniaxial
// dispatch machinery), and `transport_bounce_step`, the ONE-BOUNCE body factored out of
// the megakernel's own bounce loop so `spectral_transport.wgsl`'s `transport_main` (the
// megakernel) and `wavefront_transport.wgsl`'s `wavefront_bounce` (the wavefront
// pipeline's per-bounce kernel) call the SAME function -- literally the same compiled
// arithmetic, not two texts that could drift, mirroring `transport_physics.wgsl`'s own
// rationale for why it exists (see that file's header comment for the fault-injection
// story motivating this pattern).
//
// # Why this file exists
//
// `spectral_transport.wgsl`'s `transport_main` and `wavefront_transport.wgsl`'s
// `wavefront_bounce` both need the exact same scene bindings and the exact same
// `intersect_ray`/NEE/environment-sampling machinery `transport_bounce_step` calls, so
// all of it lives here, in a file `build.rs` concatenates ahead of BOTH
// `spectral_transport.wgsl` and `wavefront_transport.wgsl` (see that file's own doc
// comment for the concatenation lists) -- exactly the same "shared prelude" pattern
// `transport_physics.wgsl` establishes for the smaller, purely-functional pieces.
// `spectral_transport.wgsl` itself holds only `transport_main`: its per-thread prologue
// (camera ray, hero wavelength, hoisted per-channel arrays), a loop that calls
// `transport_bounce_step` once per bounce, and the final XYZ integration/output-write
// tail.
//
// # `transport_bounce_step`'s calling convention
//
// One call = one iteration of the megakernel's old `for (var bounce...)` loop, unchanged
// in every respect except how control leaves it: the old loop's THREE outer-loop exits
// (`break` on a miss, `break` on Russian-roulette failure, `continue` after a surviving
// scatter event) become a `u32` return value the caller checks --
// `BOUNCE_STATUS_TERMINATE` where the old code broke out of the loop entirely,
// `BOUNCE_STATUS_CONTINUE` everywhere else, including the implicit fall-through at the
// end of the old loop body. Every per-channel `continue` INSIDE the loop body's own
// inner `for k` loops (chromatic termination sites) is untouched -- those always
// targeted the inner channel loop, never the outer bounce loop, so they stay ordinary
// WGSL `continue` statements inside this function's own nested loops.
//
// Every free variable this function's body references from the megakernel's per-ray
// prologue is an explicit parameter: read-only quantities (hoisted per-ray constants:
// wavelengths, dispersion/absorption arrays, the biaxial axis frame, the studio rig
// directions, the RNG seed) are passed by value, exactly as the megakernel's own local
// `let`s/`var`s hold them; per-bounce MUTABLE state (the ray's origin/direction/wave
// normal, `inside_gem`/`is_extraordinary`, the Stokes/radiance/path_pdf/split_radiance/
// compat arrays, `path_escaped`, `pending_light_mis`) is passed as `ptr<function, T>`
// out parameters, the same pattern this file's own `apply_frosted_bounce`/
// `maybe_scatter_or_extinguish`/`narrow_compat` already use. Every arithmetic
// expression, comparison, and RNG draw in the function body below this point is
// byte-for-byte what `transport_main` executes, addressed through a pointer rather than
// a plain local variable. This is what makes the body bit-identical by construction: no
// value or evaluation order changes, only where each already-computed value lives.
//
// Determinism (mirrors `spectral_transport.wgsl`'s own header comment, and the
// invariant `wavefront_transport.wgsl` restates for its own kernels): every input this
// function reads is either a `let`-bound value fixed for the whole ray (passed in by
// value) or this ONE ray's own mutable state (passed in by pointer to the caller's own
// storage, whether that storage is the megakernel's function-local `var`s or
// `wavefront_bounce`'s per-invocation copy of one ray's slot in the wavefront's
// storage-buffer ray state). No cross-thread communication, no atomics -- calling this
// function twice with the same inputs produces the same outputs, regardless of which
// kernel calls it or how many OTHER rays are in flight around it.

// Full isotropic + uniaxial-birefringent + biaxial-birefringent spectral estimator, a
// direct translation of `optics::raytracer::trace_spectral_ray`. Driven by
// `renderer::gpu::estimator_check` (statistical image comparison, energy-conservation
// furnace anchor, spectral-space debug comparison) and `renderer::gpu::transport_check`
// (per-function ULP budgets, exercised via `shaders/transport_functions.wgsl`).
//
// "Mode A"/"mode B" generalize uniaxial ordinary/extraordinary to biaxial crystals with
// no single optic axis: mode A is the faster (lower-index) root of
// `biaxial_wave_indices`, mode B the slower; `is_extraordinary` selects between them,
// reused unchanged from the uniaxial path. An air->crystal entry resolves both biaxial
// modes' wave-normal directions via `biaxial_resolve_entry_mode`'s two-iteration fixed
// point, evaluated once from the hero channel and shared by every companion channel.
// Both biaxial modes walk off (unlike uniaxial's ordinary ray, which never does).
// Whether this port is actually TRUSTED for a real render is governed entirely by
// `optics::materials::GemMaterial::gpu_supported` -- callers must consult that
// predicate, not infer support from this shader existing.
//
// For an isotropic material, `is_anisotropic`/`is_biaxial` are always false by
// construction, collapsing this kernel back to bit-for-bit isotropic-only behaviour.
// The pleochroic Beer-Lambert absorption path is still ported faithfully
// (`electric_field_direction`, eigen-polarizations, the quadratic form) rather than
// shortcut to a bare `spectral_absorption(lambda)`, so even the isotropic limit
// reproduces the CPU's actual rounding, not merely its mathematically-equal result.
//
// Uniaxial interfaces route through the exact closed-form `entry_solve_pair`/
// `internal_solve` machinery (`transport_physics.wgsl`, mirroring
// `optics::raytracer::uniaxial_fresnel`), except at the exact degenerate
// wave-normal-parallel-to-optic-axis limit (`uniaxial_nondegenerate` false), where the
// simpler scalar-Fresnel-per-mode path is exact rather than an approximation.
// Chromatic termination (a companion channel's own refracted/walk-off direction
// failing to match the shared hero-driven direction within `DIRECTION_MATCH_COS_TOL`)
// always compares against the stored `final_refr_dir`/`final_dir_k`, never a
// recomputation (which could fail its own match by a few ULP). Verified against the
// CPU functions by `renderer::gpu::transport_check::p2_uniaxial_fresnel` and a Tier 3
// image comparison on Zircon/Tourmaline/Quartz/Rutile.
//
// The pleochroic absorption path additionally consults the material's third
// (`beta_ray`) band set, when present, via `pleochroic_channel_alpha_biaxial`'s
// three-independent-coefficient quadratic form whenever `is_biaxial`.
//
// Every stochastic decision (Fresnel reflect/transmit, Russian roulette) uses its own
// locally computed probability for both the branch comparison and the compensating
// division, so float divergence between threads never affects correctness. Precision
// is `f32` throughout; `f32::mul_add` mirrors WGSL `fma`, and any power path uses a
// multiplication chain rather than `pow()`.
//
// `transport_main` always writes all four output buffers (final XYZ plus the
// pre-integration per-channel radiance/lambda/path_pdf debug arrays): WGSL's
// per-entry-point bind-group inference is based on static reachability, not runtime
// branching, so a `write_debug` parameter would still force both entry points to bind
// everything anyway. `estimator_check`'s large statistical dispatches simply allocate
// (never read back) the debug buffers.
//
// Per-thread state (Stokes vectors, path_pdf, lambdas, ray origin/dir, loop scalars)
// lives entirely in this function's local variables -- no cross-thread communication,
// no atomics, one thread per (pixel, sample) tuple writing only its own output slot.
// This is what makes two dispatches against identical input byte-identical
// (`estimator_check::run_determinism`).
//
// Exit-event spectral splitting: at a crystal->air exit, a still-alive companion
// channel `k` whose own Snell direction diverges from the hero's gets its own
// transmission and one bounded environment probe (`try_split_exit_channel`) instead of
// being zeroed outright, accumulating into a per-channel `split_radiance` that commits
// into `radiance` once, only if the shared path escapes. A per-channel `compat`
// bitmask tracks which channels remain in the same MIS family; an interior mismatch
// narrows it via `narrow_compat` (exits never narrow a family). Both Russian-roulette
// sites rescale `split_radiance` by the same `1/q` as `stokes` on survival. The final
// XYZ integration normalizes each channel over its own family via
// `optics::raytracer::color::integrate_channels_to_xyz_families`'s per-channel weight
// (`weight_k = N * path_pdf[hero] / sum_{j in compat[k]} path_pdf[j]`) rather than one
// shared weight. Verified by `renderer::gpu::transport_check::p6_exit_splitting` and a
// Tier 3 image comparison isolating refractive (non-TIR-only) paths for a strongly
// dispersive uniaxial material.
//
// Kernel specialisation: `MATERIAL_CLASS` is a pipeline-overridable constant (WGSL
// `override`, resolved at pipeline-creation time via
// `wgpu::PipelineCompilationOptions::constants`) that lets `GpuFrameRenderer` compile
// one specialised pipeline per material class plus the generic pipeline every self-test
// uses unmodified (default 0u == generic). It feeds only the
// `is_anisotropic`/`is_biaxial` derivations: forcing either false at compile time makes
// every per-ray array gated on it unreachable, so dead-code elimination can drop the
// writes, the arrays, and the register space they would otherwise hold live across the
// whole bounce loop. MATERIAL_CLASS_GENERIC (and MATERIAL_CLASS_BIAXIAL for
// `is_biaxial`) passes the runtime buffer value through unchanged.
override MATERIAL_CLASS: u32 = 0u;
const MATERIAL_CLASS_GENERIC: u32 = 0u;
const MATERIAL_CLASS_ISOTROPIC: u32 = 1u;
const MATERIAL_CLASS_UNIAXIAL: u32 = 2u;
const MATERIAL_CLASS_BIAXIAL: u32 = 3u;

const NUM_CHANNELS: u32 = 8u;
const SPECTRUM_MIN: f32 = 380.0;
const SPECTRUM_SPAN: f32 = 400.0;
const NORM_FACTOR: f32 = (400.0 / 8.0) / 106.856;
const DIRECTION_MATCH_COS_TOL: f32 = 1.0 - 1e-6;
const RR_FLOOR: f32 = 0.05;
// Reflect-vs-transmit SELECTION probability's own clamp bounds -- distinct from
// `R_UNPOL_MIN`/`R_UNPOL_MAX` (`transport_physics.wgsl`), which only ever scale
// `path_pdf`, never divide `stokes` directly. This megakernel's r_unpol entry decision
// is the one site that both drives a stochastic branch AND divides `stokes` by it
// (`1/r_unpol`, `1/(1-r_unpol)`), so it alone gets the tighter [0.02, 0.98] range --
// see `apply_partial_fresnel_bounce`'s doc comment on the CPU side.
const R_UNPOL_SELECT_MIN: f32 = 0.02;
const R_UNPOL_SELECT_MAX: f32 = 0.98;
// RAY_EPS, R_UNPOL_MIN, R_UNPOL_MAX, FRESNEL_BRANCH_STREAM, RUSSIAN_ROULETTE_STREAM,
// BIREFRINGENT_SPLIT_STREAM, MODE_COUPLING_STREAM, FROSTED_DIR_U_STREAM,
// FROSTED_DIR_V_STREAM, and hash_u32 live in `transport_physics.wgsl` so
// `apply_frosted_bounce` (shared with `transport_functions.wgsl`) has them in scope
// without a duplicate copy.

// optics::raytracer::{BRADFORD_XYZ_TO_LMS, BRADFORD_LMS_TO_XYZ, apply_von_kries_white_balance}.
// `params.white_balance` is `compute_illuminant_white_balance`'s precomputed
// Bradford-LMS-space scale -- applying it correctly means transforming to this same
// Bradford LMS basis, scaling, and transforming back, not multiplying into XYZ
// directly. Local to this file since `transport_functions.wgsl`'s Tier 2 kernels never
// apply a white balance -- only `compute_illuminant_white_balance` itself is tested,
// via `shaders/environment.wgsl`'s own separately defined copy of these constants.
const BRADFORD_XYZ_TO_LMS = mat3x3<f32>(
    vec3<f32>(0.8951, -0.7502, 0.0389),
    vec3<f32>(0.2664, 1.7135, -0.0685),
    vec3<f32>(-0.1614, 0.0367, 1.0296),
);
const BRADFORD_LMS_TO_XYZ = mat3x3<f32>(
    vec3<f32>(0.986993, 0.432305, -0.008529),
    vec3<f32>(-0.147054, 0.518360, 0.040043),
    vec3<f32>(0.159963, 0.049291, 0.968487),
);

fn apply_von_kries_white_balance(xyz: vec3<f32>, lms_scale: vec3<f32>) -> vec3<f32> {
    let lms = BRADFORD_XYZ_TO_LMS * xyz;
    return BRADFORD_LMS_TO_XYZ * (lms * lms_scale);
}

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
    _pad_backdrop_0: u32,
    _pad_backdrop_1: u32,
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

// hash_u32 -- optics::raytracer::hash_u32, bit-exact. Defined in
// `transport_physics.wgsl` -- look there, not here.
//
// optics::raytracer::{low_discrepancy_base2, radical_inverse_base,
// cranley_patterson_rotate, PIXEL_JITTER_X_ROTATION_STREAM,
// PIXEL_JITTER_Y_ROTATION_STREAM, HERO_WAVELENGTH_ROTATION_STREAM}, bit-exact (see
// shaders/rng_equivalence.wgsl for the dedicated GPU/CPU RNG self-test).
//
// jx/jy/hero_rand use three different prime bases (2, 3, 5), not the same base rotated
// three ways -- measured: same-base pairing made variance worse for the
// highest-variance pixels.

const PIXEL_JITTER_X_ROTATION_STREAM: u32 = 0xA511E9B3u;
const PIXEL_JITTER_Y_ROTATION_STREAM: u32 = 0x63D81B23u;
const HERO_WAVELENGTH_ROTATION_STREAM: u32 = 0x1B873593u;

fn low_discrepancy_base2(n: u32) -> f32 {
    return f32(reverseBits(n)) / 4294967296.0;
}

// optics::raytracer::radical_inverse_base -- general prime-base radical inverse (base 2
// uses the faster bit-reversal path above instead). Uses plain `+`/`*`/`/` (no `fma()`),
// matching the CPU side's non-fused `+=`/`/=` exactly.
fn radical_inverse_base(n_in: u32, base: u32) -> f32 {
    var n = n_in;
    var val: f32 = 0.0;
    var inv_base: f32 = 1.0 / f32(base);
    loop {
        if (n == 0u) {
            break;
        }
        let digit = n % base;
        val = fma(f32(digit), inv_base, val);
        inv_base = inv_base / f32(base);
        n = n / base;
    }
    return val;
}

fn cranley_patterson_rotate(x: f32, offset: f32) -> f32 {
    let sum = x + offset;
    return sum - floor(sum);
}

// cie_1931_cmf -- color::cie1931::cie_1931_cmf: CIE 1931 2-degree observer, tabulated at
// 5nm (380-780nm, CIE_15_2004_CMF_TABLE) and linearly interpolated -- ported identically
// to shaders/environment.wgsl / shaders/furnace.wgsl. See that Rust function's own doc
// comment for why a WGSL port must stay bit-identical: plain f32 arithmetic in a fixed
// order (floor, fraction, `lo + (hi - lo) * t`), no mul_add/fma, no f64 intermediate
// anywhere. This uses the real tabulated observer rather than a Wyman/Sloan/Shirley
// Gaussian-lobe fit, which carries 1-3% XYZ error against it (worst in the x_bar trough
// around 495-510nm) -- see `color::cie1931`'s module doc comment.

const CIE_15_2004_CMF_START_NM: f32 = 380.0;
const CIE_15_2004_CMF_STEP_NM: f32 = 5.0;
const CIE_15_2004_CMF_LAST_INDEX: u32 = 80u;
// CIE_15_2004_CMF_START_NM + 80.0 * CIE_15_2004_CMF_STEP_NM, precomputed since WGSL
// `const` initializers can't call CIE_15_2004_CMF_TABLE.length() the way Rust's
// `CIE_1931_TABLE.len()` can.
const CIE_15_2004_CMF_END_NM: f32 = 780.0;

// CIE 15:2004 Table T.4 / CIE 1931 2-degree observer, 5nm, 380-780nm (81 entries) --
// SAME values as `color::cie1931::CIE_1931_TABLE`, transcribed by hand from that array
// (not generated), so a future edit to one must be mirrored into the other by hand too.
const CIE_15_2004_CMF_TABLE: array<vec3<f32>, 81> = array<vec3<f32>, 81>(
    vec3<f32>(0.0014, 0.0000, 0.0065), // 380nm
    vec3<f32>(0.0022, 0.0001, 0.0105), // 385nm
    vec3<f32>(0.0042, 0.0001, 0.0201), // 390nm
    vec3<f32>(0.0076, 0.0002, 0.0362), // 395nm
    vec3<f32>(0.0143, 0.0004, 0.0679), // 400nm
    vec3<f32>(0.0232, 0.0006, 0.1102), // 405nm
    vec3<f32>(0.0435, 0.0012, 0.2074), // 410nm
    vec3<f32>(0.0776, 0.0022, 0.3713), // 415nm
    vec3<f32>(0.1344, 0.0040, 0.6456), // 420nm
    vec3<f32>(0.2148, 0.0073, 1.0391), // 425nm
    vec3<f32>(0.2839, 0.0116, 1.3856), // 430nm
    vec3<f32>(0.3285, 0.0168, 1.6230), // 435nm
    vec3<f32>(0.3483, 0.0230, 1.7471), // 440nm
    vec3<f32>(0.3481, 0.0298, 1.7826), // 445nm
    vec3<f32>(0.3362, 0.0380, 1.7721), // 450nm
    vec3<f32>(0.3187, 0.0480, 1.7441), // 455nm
    vec3<f32>(0.2908, 0.0600, 1.6692), // 460nm
    vec3<f32>(0.2511, 0.0739, 1.5281), // 465nm
    vec3<f32>(0.1954, 0.0910, 1.2876), // 470nm
    vec3<f32>(0.1421, 0.1126, 1.0419), // 475nm
    vec3<f32>(0.0956, 0.1390, 0.8130), // 480nm
    vec3<f32>(0.0580, 0.1693, 0.6162), // 485nm
    vec3<f32>(0.0320, 0.2080, 0.4652), // 490nm
    vec3<f32>(0.0147, 0.2586, 0.3533), // 495nm
    vec3<f32>(0.0049, 0.3230, 0.2720), // 500nm
    vec3<f32>(0.0024, 0.4073, 0.2123), // 505nm
    vec3<f32>(0.0093, 0.5030, 0.1582), // 510nm
    vec3<f32>(0.0291, 0.6082, 0.1117), // 515nm
    vec3<f32>(0.0633, 0.7100, 0.0782), // 520nm
    vec3<f32>(0.1096, 0.7932, 0.0573), // 525nm
    vec3<f32>(0.1655, 0.8620, 0.0422), // 530nm
    vec3<f32>(0.2257, 0.9149, 0.0298), // 535nm
    vec3<f32>(0.2904, 0.9540, 0.0203), // 540nm
    vec3<f32>(0.3597, 0.9803, 0.0134), // 545nm
    vec3<f32>(0.4334, 0.9950, 0.0087), // 550nm
    vec3<f32>(0.5121, 1.0000, 0.0057), // 555nm
    vec3<f32>(0.5945, 0.9950, 0.0039), // 560nm
    vec3<f32>(0.6784, 0.9786, 0.0027), // 565nm
    vec3<f32>(0.7621, 0.9520, 0.0021), // 570nm
    vec3<f32>(0.8425, 0.9154, 0.0018), // 575nm
    vec3<f32>(0.9163, 0.8700, 0.0017), // 580nm
    vec3<f32>(0.9786, 0.8163, 0.0014), // 585nm
    vec3<f32>(1.0263, 0.7570, 0.0011), // 590nm
    vec3<f32>(1.0567, 0.6949, 0.0010), // 595nm
    vec3<f32>(1.0622, 0.6310, 0.0008), // 600nm
    vec3<f32>(1.0456, 0.5668, 0.0006), // 605nm
    vec3<f32>(1.0026, 0.5030, 0.0003), // 610nm
    vec3<f32>(0.9384, 0.4412, 0.0002), // 615nm
    vec3<f32>(0.8544, 0.3810, 0.0002), // 620nm
    vec3<f32>(0.7514, 0.3210, 0.0001), // 625nm
    vec3<f32>(0.6424, 0.2650, 0.0000), // 630nm
    vec3<f32>(0.5419, 0.2170, 0.0000), // 635nm
    vec3<f32>(0.4479, 0.1750, 0.0000), // 640nm
    vec3<f32>(0.3608, 0.1382, 0.0000), // 645nm
    vec3<f32>(0.2835, 0.1070, 0.0000), // 650nm
    vec3<f32>(0.2187, 0.0816, 0.0000), // 655nm
    vec3<f32>(0.1649, 0.0610, 0.0000), // 660nm
    vec3<f32>(0.1212, 0.0446, 0.0000), // 665nm
    vec3<f32>(0.0874, 0.0320, 0.0000), // 670nm
    vec3<f32>(0.0636, 0.0232, 0.0000), // 675nm
    vec3<f32>(0.0468, 0.0170, 0.0000), // 680nm
    vec3<f32>(0.0329, 0.0119, 0.0000), // 685nm
    vec3<f32>(0.0227, 0.0082, 0.0000), // 690nm
    vec3<f32>(0.0158, 0.0057, 0.0000), // 695nm
    vec3<f32>(0.0114, 0.0041, 0.0000), // 700nm
    vec3<f32>(0.0081, 0.0029, 0.0000), // 705nm
    vec3<f32>(0.0058, 0.0021, 0.0000), // 710nm
    vec3<f32>(0.0041, 0.0015, 0.0000), // 715nm
    vec3<f32>(0.0029, 0.0010, 0.0000), // 720nm
    vec3<f32>(0.0020, 0.0007, 0.0000), // 725nm
    vec3<f32>(0.0014, 0.0005, 0.0000), // 730nm
    vec3<f32>(0.0010, 0.0004, 0.0000), // 735nm
    vec3<f32>(0.0007, 0.0002, 0.0000), // 740nm
    vec3<f32>(0.0005, 0.0002, 0.0000), // 745nm
    vec3<f32>(0.0003, 0.0001, 0.0000), // 750nm
    vec3<f32>(0.0002, 0.0001, 0.0000), // 755nm
    vec3<f32>(0.0002, 0.0001, 0.0000), // 760nm
    vec3<f32>(0.0001, 0.0000, 0.0000), // 765nm
    vec3<f32>(0.0001, 0.0000, 0.0000), // 770nm
    vec3<f32>(0.0001, 0.0000, 0.0000), // 775nm
    vec3<f32>(0.0000, 0.0000, 0.0000), // 780nm
);

fn cie_1931_cmf(l: f32) -> vec3<f32> {
    if (!(CIE_15_2004_CMF_START_NM <= l && l <= CIE_15_2004_CMF_END_NM)) {
        return vec3<f32>(0.0, 0.0, 0.0);
    }
    let position = (l - CIE_15_2004_CMF_START_NM) / CIE_15_2004_CMF_STEP_NM;
    let index0 = min(u32(floor(position)), CIE_15_2004_CMF_LAST_INDEX - 1u);
    let index1 = index0 + 1u;
    let t = position - f32(index0);
    let lo = CIE_15_2004_CMF_TABLE[index0];
    let hi = CIE_15_2004_CMF_TABLE[index1];
    return lo + (hi - lo) * t;
}

// optics::renderer::env_map_spectrum::rgb_to_spectral_radiance -- used for the
// direction-independent "uniform furnace" environment (env_mode == 0u): a grey
// `EnvironmentMap::uniform(w, h, [l0, l0, l0])` that is still wavelength-dependent, so
// this is deliberately not flattened to a bare `l0` return.

// asymmetric_gaussian and rgb_to_spectral_radiance are defined in transport_physics.wgsl.

// optics::raytracer::sample_studio_environment (+ optics::studio_rig::StudioRig) --
// ported identically to shaders/environment.wgsl.

fn powi_u(base: f32, exp: u32) -> f32 {
    var result: f32 = 1.0;
    var b: f32 = base;
    var e: u32 = exp;
    loop {
        if (e == 0u) {
            break;
        }
        if ((e & 1u) == 1u) {
            result = result * b;
        }
        b = b * b;
        e = e >> 1u;
    }
    return result;
}

fn blackbody_spectrum(lambda_nm: f32, temp_k: f32) -> f32 {
    let t_k = max(temp_k, 1000.0);
    let h_c_k: f32 = 14388000.0;
    let exp_val = exp(min(h_c_k / (lambda_nm * t_k), 80.0));
    let exp_560 = exp(min(h_c_k / (560.0 * t_k), 80.0));
    let denom = max(exp_val - 1.0, 1e-6);
    let denom_560 = max(exp_560 - 1.0, 1e-6);
    let ratio = denom_560 / denom;
    return clamp(powi_u(560.0 / lambda_nm, 5u) * ratio, 0.01, 20.0);
}

// optics::raytracer::environment::{CIE_D65_SPD_380_780_10NM, d65_relative_spectral_power}
// (CIE 15:2004) -- the "D65 Daylight" preset's real measured table, used instead of
// `blackbody_spectrum` above. A `const` array, not a uniform/storage buffer: 41 `f32`s
// is small enough to inline directly, needing no extra bind-group slot or upload.
const CIE_D65_SPD_380_780_10NM: array<f32, 41> = array<f32, 41>(
    49.9755, 54.6482, 82.7549, 91.4860, 93.4318, 86.6823, 104.865, 117.008, 117.812, 114.861,
    115.923, 108.811, 109.354, 107.802, 104.790, 107.689, 104.405, 104.046, 100.000, 96.3342,
    95.7880, 88.6856, 90.0062, 89.5991, 87.6987, 83.2886, 83.6992, 80.0268, 80.2146, 82.2778,
    78.2842, 69.7213, 71.6091, 74.3496, 61.6045, 69.8856, 75.0870, 63.5928, 46.4182, 66.8054,
    63.3828,
);

// Ported op-for-op from `optics::raytracer::environment::d65_relative_spectral_power`:
// same 560nm-normalization (dividing by the table's `100.000` entry), same
// clamp-to-table-edge behaviour outside 380-780nm.
fn d65_relative_spectral_power(lambda_nm: f32) -> f32 {
    let start_nm: f32 = 380.0;
    let step_nm: f32 = 10.0;
    let last_index: u32 = 40u; // CIE_D65_SPD_380_780_10NM.len() - 1

    let clamped = clamp(lambda_nm, start_nm, fma(step_nm, f32(last_index), start_nm));
    let position = (clamped - start_nm) / step_nm;
    let index0 = min(u32(floor(position)), last_index - 1u);
    let index1 = index0 + 1u;
    let frac = position - f32(index0);

    let v0 = CIE_D65_SPD_380_780_10NM[index0];
    let v1 = CIE_D65_SPD_380_780_10NM[index1];
    return fma(frac, v1 - v0, v0) / 100.0;
}

const RING_LIGHT_COUNT: u32 = 16u;

fn studio_rig_key_dir(light_yaw: f32, light_pitch: f32) -> vec3<f32> {
    let cos_lp = cos(light_pitch);
    let sin_lp = sin(light_pitch);
    let cos_ly = cos(light_yaw);
    let sin_ly = sin(light_yaw);
    return normalize(vec3<f32>(cos_lp * sin_ly, sin_lp, cos_lp * cos_ly));
}

fn studio_rig_fill_dir(light_yaw: f32, light_pitch: f32) -> vec3<f32> {
    let fill_yaw = fma(PI, 0.78, light_yaw);
    let fill_pitch = clamp(light_pitch * 0.65, 0.15, 1.2);
    return normalize(vec3<f32>(cos(fill_pitch) * sin(fill_yaw), sin(fill_pitch), cos(fill_pitch) * cos(fill_yaw)));
}

fn studio_rig_ring_dir(i: u32, light_yaw: f32, sin_lp: f32) -> vec3<f32> {
    let angle = fma(f32(i), PI * 2.0 / f32(RING_LIGHT_COUNT), light_yaw);
    return normalize(vec3<f32>(cos(angle) * 0.75, sin_lp * 0.8, sin(angle) * 0.75));
}

// optics::raytracer::environment::sample_light_tent -- needs `studio_rig_ring_dir` for
// the three black cards on ring slots 4/8/12, so it lives here rather than in the shared
// prelude with the other lit models.
fn sample_light_tent(
    d: vec3<f32>,
    spec_power: f32,
    spot_mult: f32,
    exposure: f32,
    key_dir: vec3<f32>,
    fill_dir: vec3<f32>,
    sin_lp: f32,
    light_yaw: f32,
    observer: vec3<f32>,
) -> f32 {
    let horizon = horizon_blend(d);
    var walls = fma(0.08, max(d.y, 0.0), 0.14);
    var card: f32 = 0.0;
    for (var slot: u32 = 4u; slot < RING_LIGHT_COUNT; slot = slot + 4u) {
        let card_dir = studio_rig_ring_dir(slot, light_yaw, sin_lp);
        card = max(card, smoothstep_f32(CARD_OUTER_COS, CARD_INNER_COS, dot(d, card_dir)));
    }
    walls = walls * fma(card, -0.9, 1.0);
    let key = smoothstep_f32(TENT_KEY_OUTER_COS, TENT_KEY_INNER_COS, dot(d, key_dir)) * (1.4 * spot_mult);
    let spark = smoothstep_f32(SPARK_OUTER_COS, SPARK_INNER_COS, dot(d, fill_dir)) * (5.0 * spot_mult);
    let above = ((walls + key) + spark) * (horizon * observer_visibility(d, observer));
    let ground = 0.02 * (1.0 - horizon);
    return (above + ground) * (spec_power * exposure);
}

fn sample_studio_rig(
    d: vec3<f32>,
    spec_power: f32,
    spot_mult: f32,
    exposure: f32,
    key_dir: vec3<f32>,
    fill_dir: vec3<f32>,
    sin_lp: f32,
    light_yaw: f32,
) -> f32 {
    let bg_val = max(fma(0.012, fma(d.y, 0.5, 0.5), 0.015), 0.005) * exposure;
    var radiance = bg_val * spec_power;

    let key_dot = max(dot(d, key_dir), 0.0);
    if (key_dot > 0.0) {
        let softbox = powi_u(key_dot, 28u) * 12.0 * spot_mult * exposure;
        radiance = fma(softbox, spec_power, radiance);
    }

    let fill_dot = max(dot(d, fill_dir), 0.0);
    if (fill_dot > 0.0) {
        let fill = powi_u(fill_dot, 18u) * 4.5 * exposure;
        radiance = fma(fill, spec_power, radiance);
    }

    for (var i: u32 = 0u; i < RING_LIGHT_COUNT; i = i + 1u) {
        let ring_dir = studio_rig_ring_dir(i, light_yaw, sin_lp);
        let ring_dot = max(dot(d, ring_dir), 0.0);
        if (ring_dot > 0.96) {
            let spark = (ring_dot - 0.96) / 0.04;
            let intensity = powi_u(spark, 6u) * 22.0 * spot_mult * exposure;
            radiance = fma(intensity, spec_power, radiance);
        }
    }

    return radiance;
}

fn studio_dispatch(
    model: u32,
    d: vec3<f32>,
    spec_power: f32,
    spot_mult: f32,
    exposure: f32,
    key_dir: vec3<f32>,
    fill_dir: vec3<f32>,
    sin_lp: f32,
    light_yaw: f32,
    observer: vec3<f32>,
) -> f32 {
    switch (model) {
        case 1u: {
            return sample_iso_hemisphere(d, spec_power, exposure, observer);
        }
        case 2u: {
            return sample_light_tent(d, spec_power, spot_mult, exposure, key_dir, fill_dir, sin_lp, light_yaw, observer);
        }
        case 3u: {
            return sample_daylight_dome(d, spec_power, exposure, key_dir, observer);
        }
        default: {
            return sample_studio_rig(d, spec_power, spot_mult, exposure, key_dir, fill_dir, sin_lp, light_yaw);
        }
    }
}

// `LightingPreset::spectral_power`: the tabulated CIE D65 curve for the D65 presets,
// else a Planckian fit at `studio_temp_k`.
fn studio_spectral_power(lambda_nm: f32) -> f32 {
    if (params.studio_use_d65 != 0u) {
        return d65_relative_spectral_power(lambda_nm);
    }
    return blackbody_spectrum(lambda_nm, params.studio_temp_k);
}

// `key_dir`/`fill_dir`/`sin_lp` (the `StudioRig`-equivalent quantities) are constant
// across an entire ray, so the caller (`transport_main`'s miss branch) computes them
// once before its `NUM_CHANNELS` loop and passes them in, rather than this function
// recomputing them on every per-channel call -- mirrors
// `optics::raytracer::accumulate_miss_radiance` building them once per ray.
fn sample_studio_environment_with_rig(
    dir_in: vec3<f32>,
    lambda_nm: f32,
    key_dir: vec3<f32>,
    fill_dir: vec3<f32>,
    sin_lp: f32,
    observer: vec3<f32>,
) -> f32 {
    let d = normalize(dir_in);
    let spec_power = studio_spectral_power(lambda_nm);

    return studio_dispatch(
        params.studio_model,
        d,
        spec_power,
        params.studio_spot_mult,
        params.studio_exposure,
        key_dir,
        fill_dir,
        sin_lp,
        params.studio_light_yaw,
        observer,
    );
}

// renderer::env_map::EnvironmentMap::{direction_to_uv, sample_bilinear,
// radiance_at} -- ported op-for-op, including the exact `mul_add`/`fma` chains, so this
// stays within `environment_check`'s ULP budget for `hdr_env_radiance_at` (see that
// module's standalone `env_map_radiance_main` self-test kernel, a duplicate of this same
// logic exercised independently of the megakernel, matching this file's existing
// convention of not sharing code across WGSL modules -- see e.g. `blackbody_spectrum`).

// hdr_wrap_x, hdr_clamp_y, and hdr_direction_to_uv are defined in transport_physics.wgsl.

fn hdr_texel(x: u32, y: u32, width: u32) -> vec3<f32> {
    return hdr_texels[y * width + x].xyz;
}

// EnvironmentMap::sample_bilinear -- same `fx`/`fy` half-texel offset, same wrap/clamp
// neighbour selection, same per-component `mul_add` interpolation order.
fn hdr_env_sample_bilinear(u_in: f32, v_in: f32) -> vec3<f32> {
    let width = hdr_env_dims.width;
    let height = hdr_env_dims.height;
    let width_i = i32(width);
    let height_i = i32(height);

    let u_wrapped = fract(u_in);
    let v_clamped = clamp(v_in, 0.0, 1.0);
    let fx = fma(u_wrapped, f32(width), -0.5);
    let fy = fma(v_clamped, f32(height), -0.5);

    let x0 = floor(fx);
    let y0 = floor(fy);
    let tx = fx - x0;
    let ty = fy - y0;

    let x0i = hdr_wrap_x(i32(x0), width_i);
    let x1i = hdr_wrap_x(i32(x0) + 1, width_i);
    let y0i = hdr_clamp_y(i32(y0), height_i);
    let y1i = hdr_clamp_y(i32(y0) + 1, height_i);

    let p00 = hdr_texel(x0i, y0i, width);
    let p10 = hdr_texel(x1i, y0i, width);
    let p01 = hdr_texel(x0i, y1i, width);
    let p11 = hdr_texel(x1i, y1i, width);

    let top = fma(p10, vec3<f32>(tx), p00 * (1.0 - tx));
    let bottom = fma(p11, vec3<f32>(tx), p01 * (1.0 - tx));
    return fma(bottom, vec3<f32>(ty), top * (1.0 - ty));
}

// EnvironmentMap::radiance_at -- bilinear RGB lookup, then the same `rgb_to_spectral_radiance`
// spectral lift the uniform-furnace branch above already uses.
fn hdr_env_radiance_at(dir: vec3<f32>, lambda_nm: f32) -> f32 {
    let uv = hdr_direction_to_uv(dir);
    let rgb = hdr_env_sample_bilinear(uv.x, uv.y);
    return rgb_to_spectral_radiance(rgb.x, rgb.y, rgb.z, lambda_nm);
}

fn sample_environment_with_rig(
    dir: vec3<f32>,
    lambda_nm: f32,
    key_dir: vec3<f32>,
    fill_dir: vec3<f32>,
    sin_lp: f32,
    observer: vec3<f32>,
) -> f32 {
    if (params.env_mode == 0u) {
        return rgb_to_spectral_radiance(params.l0, params.l0, params.l0, lambda_nm);
    } else if (params.env_mode == 2u) {
        return hdr_env_radiance_at(dir, lambda_nm);
    }
    return sample_studio_environment_with_rig(dir, lambda_nm, key_dir, fill_dir, sin_lp, observer);
}

// `dispersion_evaluate`, `spectral_absorption`, the four `mueller_*` Mueller-matrix
// constructors, `tir_phase_delta`, `normalize_or_zero`, `signed_frame_rotation_psi`,
// `degree_of_polarization`, `polarization_azimuth`, `arbitrary_perpendicular`,
// `electric_field_direction`, `stable_orthonormal_basis_t`,
// `ordinary_eigen_polarization`, `extraordinary_eigen_polarization`, `quadratic_form`,
// and `pleochroic_channel_alpha` all live in `shaders/transport_physics.wgsl`, the
// shared source `build.rs` concatenates ahead of this file. Look there, not here.

// optics::raytracer::intersect_polyhedron

struct HitInfo {
    hit: bool,
    t: f32,
    normal: vec3<f32>,
    // optics::raytracer::HitRecord::facet_idx -- lets the frosted-finish lookup below
    // index `facet_finishes`. Mirrors `near_facet`/`far_facet`'s `.unwrap_or(0)`
    // fallback: `0u` whenever a hit is never reported either.
    facet_idx: u32,
}

fn intersect_ray(origin: vec3<f32>, dir: vec3<f32>) -> HitInfo {
    var t_near: f32 = -1e30;
    var t_far: f32 = 1e30;
    var near_normal = vec3<f32>(0.0, 0.0, 0.0);
    var far_normal = vec3<f32>(0.0, 0.0, 0.0);
    var near_idx: u32 = 0u;
    var far_idx: u32 = 0u;
    var result: HitInfo;
    let num_planes = arrayLength(&planes);
    for (var i: u32 = 0u; i < num_planes; i = i + 1u) {
        let p = plane_at(i, num_planes);
        let n = p.normal;
        let denom = dot(n, dir);
        let side = p.d + dot(n, origin);
        let numer = -side;
        if (abs(denom) > 1e-7) {
            let t = numer / denom;
            if (denom < 0.0) {
                if (t > t_near) {
                    t_near = t;
                    near_normal = n;
                    near_idx = i;
                }
            } else if (t < t_far) {
                t_far = t;
                far_normal = n;
                far_idx = i;
            }
        } else if (side > 0.0) {
            // Ray (near-)parallel to this plane, origin already outside its half-space
            // -- the polyhedron intersection is empty for this ray.
            result.hit = false;
            result.t = 0.0;
            result.normal = vec3<f32>(0.0, 0.0, 0.0);
            result.facet_idx = 0u;
            return result;
        }
    }
    if (t_near > t_far) {
        result.hit = false;
        result.t = 0.0;
        result.normal = vec3<f32>(0.0, 0.0, 0.0);
        result.facet_idx = 0u;
    } else if (t_near > 1e-4) {
        result.hit = true;
        result.t = t_near;
        result.normal = near_normal;
        result.facet_idx = near_idx;
    } else if (t_far > 1e-4) {
        result.hit = true;
        result.t = t_far;
        result.normal = far_normal;
        result.facet_idx = far_idx;
    } else {
        result.hit = false;
        result.t = 0.0;
        result.normal = vec3<f32>(0.0, 0.0, 0.0);
        result.facet_idx = 0u;
    }
    return result;
}

// Exit-event spectral splitting: optics::raytracer::refraction::try_split_exit_channel
// -- the one bounded fan-out primitive every exit-event mismatch site below calls.
// Reads the `planes` binding (via `intersect_ray`) and `params`/`material` (via
// `sample_environment_with_rig`), so -- like `intersect_ray`/`shading_normal_near_edge`
// above -- this stays megakernel-local rather than living in the shared
// `transport_physics.wgsl` prelude. `split_radiance` is threaded as a pointer to the
// caller's own local array, like `stokes`/`path_pdf`.

fn try_split_exit_channel(
    split_radiance: ptr<function, array<f32, 8>>,
    hit_point: vec3<f32>,
    k: u32,
    lambda_k: f32,
    dir_k: vec3<f32>,
    transmitted_intensity: f32,
    key_dir: vec3<f32>,
    fill_dir: vec3<f32>,
    sin_lp: f32,
    observer: vec3<f32>,
) {
    let probe = intersect_ray(hit_point + dir_k * 1e-4, dir_k);
    if (probe.hit) {
        // Bounded re-entry: decline to trace further (a pure energy-loss truncation).
        return;
    }
    let env_spectral = sample_environment_with_rig(dir_k, lambda_k, key_dir, fill_dir, sin_lp, observer);
    (*split_radiance)[k] = fma(max(transmitted_intensity, 0.0), env_spectral, (*split_radiance)[k]);
}

// optics::raytracer::shading_normal_near_edge -- reads the `planes` storage binding
// directly (like `intersect_ray` above), so stays megakernel-local rather than living
// in the shared `transport_physics.wgsl` prelude. `shaders/shading_normal.wgsl` has its
// own standalone copy with its own `planes` binding for
// `renderer::gpu::transport_check::run_shading_normal_near_edge`'s Tier 2 self-test;
// both are unmodified line-for-line translations of the same CPU function.

fn shading_normal_near_edge(hit_point: vec3<f32>, hit_facet_idx: u32, hit_normal: vec3<f32>, rounding_radius: f32) -> vec3<f32> {
    if (rounding_radius <= 0.0) {
        return hit_normal;
    }
    var nearest_dist: f32 = 1e30;
    var nearest_normal = hit_normal;
    let num_planes = arrayLength(&planes);
    for (var i: u32 = 0u; i < num_planes; i = i + 1u) {
        if (i == hit_facet_idx) {
            continue;
        }
        let p = planes[i];
        let dist = -(p.d + dot(p.normal, hit_point));
        if (dist < nearest_dist) {
            nearest_dist = dist;
            nearest_normal = p.normal;
        }
    }
    if (nearest_dist >= rounding_radius) {
        return hit_normal;
    }
    let t = clamp(1.0 - nearest_dist / rounding_radius, 0.0, 1.0);
    let smooth_t = t * t * fma(-2.0, t, 3.0);
    let bisector = normalize_or_zero(hit_normal + nearest_normal);
    return normalize_or_zero(hit_normal * (1.0 - smooth_t) + bisector * smooth_t);
}

// ---------------------------------------------------------------------------------
// GPU Next-Event Estimation & Multiple Importance Sampling
// ---------------------------------------------------------------------------------

fn dist1d_find_bucket(cdf_start: u32, n: u32, u: f32) -> u32 {
    var first: u32 = 0u;
    var len: u32 = n + 1u;
    while (len > 0u) {
        let half = len / 2u;
        let middle = first + half;
        if (dist_cdf[cdf_start + middle] <= u) {
            first = middle + 1u;
            len = len - (half + 1u);
        } else {
            len = half;
        }
    }
    var offset: u32 = 0u;
    if (first > 0u) {
        offset = first - 1u;
    }
    return min(offset, n - 1u);
}

fn dist1d_bucket_pdf(func_start: u32, offset: u32, func_int: f32) -> f32 {
    if (func_int > 0.0) {
        return max(dist_func[func_start + offset], 0.0) / func_int;
    }
    return 1.0;
}

fn dist1d_sample_continuous(cdf_start: u32, func_start: u32, n: u32, func_int: f32, u_in: f32) -> Dist1dSample {
    let u = clamp(u_in, 0.0, 0.99999994);
    let offset = dist1d_find_bucket(cdf_start, n, u);
    let cdf0 = dist_cdf[cdf_start + offset];
    let cdf1 = dist_cdf[cdf_start + offset + 1u];
    let span = cdf1 - cdf0;
    var du: f32 = 0.0;
    if (span > 0.0) {
        du = (u - cdf0) / span;
    }
    let sample = clamp((f32(offset) + du) / f32(n), 0.0, 0.99999994);
    let pdf = dist1d_bucket_pdf(func_start, offset, func_int);
    var res: Dist1dSample;
    res.sample = sample;
    res.pdf = pdf;
    res.offset = offset;
    return res;
}

fn dist1d_pdf(func_start: u32, n: u32, func_int: f32, x: f32) -> f32 {
    let offset = min(u32(clamp(x, 0.0, 0.99999994) * f32(n)), n - 1u);
    return dist1d_bucket_pdf(func_start, offset, func_int);
}

fn dist2d_sample(u0: f32, u1: f32) -> Dist2dSample {
    let width = dist_dims.width;
    let height = dist_dims.height;
    let marginal_cdf_start = height * (width + 1u);
    let marginal_func_start = width * height;
    let marginal_func_int = dist_dims.marginal_func_int;

    let s_v = dist1d_sample_continuous(marginal_cdf_start, marginal_func_start, height, marginal_func_int, u1);
    let row = s_v.offset;
    let v = s_v.sample;
    let pdf_v = s_v.pdf;

    let cond_cdf_start = row * (width + 1u);
    let cond_func_start = row * width;
    let cond_func_int = dist_func[marginal_func_start + row];

    let s_u = dist1d_sample_continuous(cond_cdf_start, cond_func_start, width, cond_func_int, u0);
    let u = s_u.sample;
    let pdf_u = s_u.pdf;

    let dir = hdr_uv_to_direction(u, v);
    let rgb = hdr_env_sample_bilinear(u, v);
    let pdf = pdf_uv_to_solid_angle(pdf_u * pdf_v, v);

    var res: Dist2dSample;
    res.dir = dir;
    res.rgb = rgb;
    res.pdf = pdf;
    return res;
}

fn dist2d_pdf_uv(u: f32, v: f32) -> f32 {
    let width = dist_dims.width;
    let height = dist_dims.height;
    let marginal_func_start = width * height;
    let marginal_func_int = dist_dims.marginal_func_int;

    let row = min(u32(clamp(v, 0.0, 0.99999994) * f32(height)), height - 1u);
    let pdf_v = dist1d_pdf(marginal_func_start, height, marginal_func_int, v);

    let cond_func_start = row * width;
    let cond_func_int = dist_func[marginal_func_start + row];
    let pdf_u = dist1d_pdf(cond_func_start, width, cond_func_int, u);

    return pdf_u * pdf_v;
}

// renderer::env_map::EnvironmentMap::pdf -- `sin(theta)` off the unit direction
// (`length(vec2(x, z))`, the CPU's `x.hypot(z)`), never `sin(acos(y))`; see that
// function's own comment.
fn dist2d_pdf(dir: vec3<f32>) -> f32 {
    let uv = hdr_direction_to_uv(dir);
    let pdf_uv = dist2d_pdf_uv(uv.x, uv.y);
    let d = normalize(dir);
    let sin_theta = length(vec2<f32>(d.x, d.z));
    return pdf_uv_to_solid_angle_from_sin(pdf_uv, sin_theta);
}

// optics::raytracer::scattering::nee_contribution_hg_scatter. `alphas` mirrors that
// function's own explicit parameter; `sigma_s`/`absorption_path_scale`
// and `facet_finishes` are read directly off the global
// `material`/`facet_finishes` bindings instead, exactly as the megakernel's own scatter
// call site above (`transport_bounce_step`) already does for the SAME quantities.
fn nee_contribution_hg_scatter(
    lambdas: ptr<function, array<f32, 8>>,
    n_inside_hero: f32,
    scatter_point: vec3<f32>,
    scatter_dir_in: vec3<f32>,
    g: f32,
    rng_seed: u32,
    bounce: u32,
    stokes: ptr<function, array<vec4<f32>, 8>>,
    radiance: ptr<function, array<f32, 8>>,
    alphas: array<f32, 8>,
) {
    let u0 = f32(hash_u32(rng_seed ^ hash_u32(bounce ^ NEE_ENV_DIR_U_STREAM))) / 4294967295.0;
    let u1 = f32(hash_u32(rng_seed ^ hash_u32(bounce ^ NEE_ENV_DIR_V_STREAM))) / 4294967295.0;
    let sample = dist2d_sample(u0, u1);
    if (sample.pdf <= 0.0) {
        return;
    }

    let probe_origin = scatter_point + sample.dir * 1e-4;
    let hit = intersect_ray(probe_origin, sample.dir);
    if (!hit.hit) {
        return;
    }
    // A frosted exit facet has no well-defined specular Fresnel/refraction
    // for this shadow ray to use -- `nee_contribution_frosted_exterior` already handles
    // NEE for a frosted exit's own diffusely-sampled surface point.
    if (hit.facet_idx < arrayLength(&facet_finishes) && facet_finishes[hit.facet_idx] == FACET_FINISH_FROSTED) {
        return;
    }

    let cos_i = clamp(dot(sample.dir, hit.normal), 0.0, 1.0);
    let sin2_t = min(n_inside_hero * n_inside_hero * fma(-cos_i, cos_i, 1.0), 1.0);
    if (sin2_t >= 1.0) {
        return;
    }
    let cos_t = sqrt(max(1.0 - sin2_t, 0.0));
    let r_s = fma(n_inside_hero, cos_i, -cos_t) / fma(n_inside_hero, cos_i, cos_t);
    let r_p = fma(n_inside_hero, -cos_t, cos_i) / fma(n_inside_hero, cos_t, cos_i);
    let r_unpol = clamp(0.5 * fma(r_p, r_p, r_s * r_s), 0.0, 1.0);
    let t_unpol = 1.0 - r_unpol;

    let phase_cos = dot(sample.dir, scatter_dir_in);
    let phase_val = henyey_greenstein_phase(phase_cos, g);
    let mis_weight = balance_heuristic(sample.pdf, phase_val);
    if (mis_weight <= 0.0) {
        return;
    }

    // The exterior direction this light sample actually leaves along --
    // Snell's law at the exit facet, mirroring `refraction.wgsl`'s own
    // `eta*k_hat + (eta*cos_i - cos_t)*normal` vector form with `sample.dir` playing
    // the incident-direction role and (the inward-flipped) `-hit.normal` the surface
    // normal. `sample.pdf` itself stays in the INTERIOR (pre-refraction) measure
    // `dist2d_sample` sampled in -- only the radiance LOOKUP moves to the refracted
    // direction.
    let refracted_dir = normalize(n_inside_hero * sample.dir - fma(n_inside_hero, cos_i, -cos_t) * hit.normal);
    let refracted_uv = hdr_direction_to_uv(refracted_dir);
    let env_rgb = hdr_env_sample_bilinear(refracted_uv.x, refracted_uv.y);

    // The medium transmittance a phase-sampled continuation reaching this
    // same boundary would have paid -- the same per-channel `exp_poly(-(alphas[k]+sigma_s)*
    // hit.t*path_scale)` `maybe_scatter_or_extinguish`'s survive branch applies
    // (`exp_poly`, not the `exp()` builtin -- see that function's own doc comment,
    // `transport_physics.wgsl`).
    let hit_t_scaled = hit.t * material.absorption_path_scale;

    let nee_common = t_unpol * phase_val * mis_weight / sample.pdf;
    for (var k: u32 = 0u; k < 8u; k = k + 1u) {
        let transmittance_k = exp_poly(-(alphas[k] + material.scattering_sigma_s) * hit_t_scaled);
        let env_k = rgb_to_spectral_radiance(env_rgb.x, env_rgb.y, env_rgb.z, (*lambdas)[k]);
        (*radiance)[k] = fma((*stokes)[k].x * transmittance_k * nee_common * env_k, 1.0, (*radiance)[k]);
    }
}

fn nee_contribution_frosted_exterior(
    lambdas: ptr<function, array<f32, 8>>,
    ext_normal: vec3<f32>,
    rng_seed: u32,
    bounce: u32,
    stokes: ptr<function, array<vec4<f32>, 8>>,
    radiance: ptr<function, array<f32, 8>>,
) {
    let u0 = f32(hash_u32(rng_seed ^ hash_u32(bounce ^ FROSTED_NEE_ENV_DIR_U_STREAM))) / 4294967295.0;
    let u1 = f32(hash_u32(rng_seed ^ hash_u32(bounce ^ FROSTED_NEE_ENV_DIR_V_STREAM))) / 4294967295.0;
    let sample = dist2d_sample(u0, u1);
    if (sample.pdf <= 0.0) {
        return;
    }

    let cos_light = dot(sample.dir, ext_normal);
    if (cos_light <= 0.0) {
        return;
    }

    let brdf_pdf = cos_light / PI;
    let mis_weight = balance_heuristic(sample.pdf, brdf_pdf);
    if (mis_weight <= 0.0) {
        return;
    }

    let nee_common = brdf_pdf * mis_weight / sample.pdf;
    for (var k: u32 = 0u; k < 8u; k = k + 1u) {
        let env_k = rgb_to_spectral_radiance(sample.rgb.x, sample.rgb.y, sample.rgb.z, (*lambdas)[k]);
        (*radiance)[k] = fma((*stokes)[k].x * nee_common * env_k, 1.0, (*radiance)[k]);
    }
}

// ---------------------------------------------------------------------------------
// optics::raytracer::Camera::generate_ray -- bit-exact per camera_check.
// ---------------------------------------------------------------------------------

struct RayGen {
    origin: vec3<f32>,
    dir: vec3<f32>,
}

fn generate_camera_ray(pixel: u32, jx: f32, jy: f32) -> RayGen {
    let x = f32(pixel % u32(camera.width));
    let y = f32(pixel / u32(camera.width));
    let aspect = camera.width / camera.height;
    let u = ((x + jx) / camera.width - 0.5) * 2.0 * aspect * camera.fov_tan;
    let v = (0.5 - (y + jy) / camera.height) * 2.0 * camera.fov_tan;
    let dir = normalize(camera.forward + camera.right * u + camera.up * v);
    var result: RayGen;
    result.origin = camera.origin;
    result.dir = dir;
    return result;
}

// optics::raytracer::apply_internal_mode_coupling -- stochastic o<->e re-coupling at an
// INTERNAL reflection inside an anisotropic crystal. This is a RELABELING, not a SPLIT:
// unlike the entry split (BIREFRINGENT_SPLIT_STREAM, where one incident ray genuinely
// becomes two physically distinct rays each carrying its own ~0.5 energy share already
// matched by its own 0.5 selection probability, needing no `1/p` scaling), this draw
// only re-rolls which eigenmode label governs the index used for the path's next
// bounce -- no new ray, no energy split, nothing for `stokes`/`path_pdf` to do. This
// draw's own selection probability is polarization-weighted
// (`entry_eigenmode_selection`) rather than a blanket 50/50, which is safe precisely
// because `stokes`/`path_pdf` are never touched here.

// optics::raytracer::refraction::entry_eigenmode_selection -- polarization-weighted
// probability that the uniaxial ORDINARY eigenmode is the physically correct label for
// the hero-driven path, plus that eigenmode's own doubled polarization azimuth
// (`cos(2*psi_o)`, `sin(2*psi_o)`) in the SAME Stokes reference frame
// `current_plane_normal` establishes (Malus's law energy split; a unit-vector
// double-angle identity in place of an atan2/half-angle round-trip). WGSL has no
// `Option`, so `valid == 0u` stands in for the Rust `None` case (negligible `i`,
// degenerate plane of incidence, or negligible linear polarization) -- callers must
// check it before trusting `p_o`/`cos_2psi_o`/`sin_2psi_o`.
struct EntryEigenmodeSelection {
    valid: u32,
    p_o: f32,
    cos_2psi_o: f32,
    sin_2psi_o: f32,
}

fn entry_eigenmode_selection(
    c_axis: vec3<f32>,
    current_plane_normal: vec3<f32>,
    k_hat: vec3<f32>,
    hero_i: f32,
    hero_q: f32,
    hero_u: f32,
) -> EntryEigenmodeSelection {
    var result: EntryEigenmodeSelection;
    result.valid = 0u;
    result.p_o = 0.5;
    result.cos_2psi_o = 0.0;
    result.sin_2psi_o = 0.0;
    if (hero_i <= 1e-7 || dot(current_plane_normal, current_plane_normal) <= 1e-6) {
        return result;
    }
    if (fma(hero_q, hero_q, hero_u * hero_u) <= 1e-12) {
        return result;
    }
    let o_hat = ordinary_eigen_polarization(k_hat, c_axis);
    // `current_plane_normal` is already unit and exactly perpendicular to `k_hat` --
    // no re-orthogonalization needed.
    let s_hat = current_plane_normal;
    let p_hat = cross(k_hat, s_hat);
    let s_comp = dot(o_hat, s_hat);
    let p_comp = dot(o_hat, p_hat);
    let cos_2psi_o = fma(s_comp, s_comp, -(p_comp * p_comp));
    let sin_2psi_o = 2.0 * s_comp * p_comp;
    result.valid = 1u;
    result.p_o = clamp(0.5 + 0.5 * fma(hero_q, cos_2psi_o, hero_u * sin_2psi_o) / hero_i, 0.0, 1.0);
    result.cos_2psi_o = cos_2psi_o;
    result.sin_2psi_o = sin_2psi_o;
    return result;
}

// Mirrors `apply_internal_mode_coupling`'s mode-selection draw. `has_exact_p_o`/
// `exact_p_o` stand in for its `exact_p_o: Option<f32>` (WGSL has no `Option`): when
// set, reuses the closed-form `internal_solve` Poynting-weighted `R_o/(R_o+R_e)` split
// already computed for reflected-energy accounting, instead of the
// polarization-projection heuristic (`entry_eigenmode_selection`). `is_biaxial` keeps
// the blanket 50/50 unconditionally, matching the CPU's biaxial exclusion.
fn internal_mode_coupling_draw(
    c_axis: vec3<f32>,
    is_biaxial: bool,
    current_plane_normal: vec3<f32>,
    new_k: vec3<f32>,
    hero_i: f32,
    hero_q: f32,
    hero_u: f32,
    has_exact_p_o: bool,
    exact_p_o: f32,
    seed0: u32,
    bounce: u32,
) -> bool {
    var p_o = 0.5;
    if (has_exact_p_o) {
        p_o = exact_p_o;
    } else if (!is_biaxial) {
        let sel = entry_eigenmode_selection(c_axis, current_plane_normal, new_k, hero_i, hero_q, hero_u);
        if (sel.valid != 0u) {
            p_o = sel.p_o;
        }
    }
    let split_rand = f32(hash_u32(seed0 ^ hash_u32(bounce ^ MODE_COUPLING_STREAM))) / 4294967295.0;
    return split_rand >= p_o;
}

// One-bounce status codes -- see this file's header comment.
const BOUNCE_STATUS_CONTINUE: u32 = 0u;
const BOUNCE_STATUS_TERMINATE: u32 = 1u;

// See this file's header comment for the calling convention. Parameter order: `bounce`
// (the loop counter) and the per-ray RNG seed first, then every read-only per-ray
// constant in the same order the megakernel's own prologue computed them (`observer`,
// the unit direction back towards the eye for the lit lighting models' head shadow,
// last), then every mutable per-bounce state pointer in the same order the
// megakernel's own prologue declared them.
fn transport_bounce_step(
    bounce: u32,
    seed0: u32,
    lambdas: ptr<function, array<f32, 8>>,
    c_axis: vec3<f32>,
    birefringence_delta: f32,
    is_anisotropic: bool,
    is_biaxial: bool,
    biax_ax0: vec3<f32>,
    biax_ax1: vec3<f32>,
    biax_ax2: vec3<f32>,
    n_o_hero_seed: f32,
    n_beta_hero: f32,
    n_alpha_hero: f32,
    n_gamma_hero: f32,
    n_e_hero_seed: f32,
    n_o_hoisted: array<f32, 8>,
    alpha_o_hoisted: array<f32, 8>,
    alpha_e_hoisted: array<f32, 8>,
    alpha_beta_hoisted: array<f32, 8>,
    studio_key_dir: vec3<f32>,
    studio_fill_dir: vec3<f32>,
    studio_sin_lp: f32,
    observer: vec3<f32>,
    stokes: ptr<function, array<vec4<f32>, 8>>,
    radiance: ptr<function, array<f32, 8>>,
    path_pdf: ptr<function, array<f32, 8>>,
    current_origin: ptr<function, vec3<f32>>,
    current_dir: ptr<function, vec3<f32>>,
    current_k: ptr<function, vec3<f32>>,
    inside_gem: ptr<function, bool>,
    is_extraordinary: ptr<function, bool>,
    prev_plane_normal: ptr<function, vec3<f32>>,
    have_prev_plane_normal: ptr<function, bool>,
    split_radiance: ptr<function, array<f32, 8>>,
    compat: ptr<function, array<u32, 8>>,
    path_escaped: ptr<function, bool>,
    pending_light_mis: ptr<function, f32>,
    // The interior direction `pending_light_mis`'s phase pdf was evaluated
    // at -- paired with it exactly like the CPU's `Option<(f32, Vec3)>` carry, and
    // consumed the same way (`dist2d_pdf` below, instead of `(*current_dir)`, which by
    // the time a transmit-out carry reaches here is the refracted EXTERIOR direction).
    pending_light_mis_dir: ptr<function, vec3<f32>>,
) -> u32 {
        let phase_pdf_this_check = (*pending_light_mis);
        let phase_dir_this_check = (*pending_light_mis_dir);
        (*pending_light_mis) = 0.0;

        let hit = intersect_ray((*current_origin), (*current_dir));
        if (!hit.hit) {
            // The camera ray sees the backdrop card, if the scene has one -- see
            // `optics::raytracer::environment::fill_backdrop`.
            if (bounce == 0u && params.env_mode == 1u && params.backdrop > 0.0) {
                for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
                    (*radiance)[k] = params.backdrop * studio_spectral_power((*lambdas)[k]);
                }
                (*path_escaped) = true;
                return BOUNCE_STATUS_TERMINATE;
            }
            var mis_weight: f32 = 1.0;
            if (phase_pdf_this_check > 0.0 && params.env_mode == 2u) {
                let light_pdf = dist2d_pdf(phase_dir_this_check);
                mis_weight = balance_heuristic(phase_pdf_this_check, light_pdf);
            }
            for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
                let env = sample_environment_with_rig((*current_dir), (*lambdas)[k], studio_key_dir, studio_fill_dir, studio_sin_lp, observer);
                // `max((*stokes)[k].x, 0.0)` clamps `I` to >= 0 before the environment
                // lookup, matching `accumulate_miss_radiance`'s `StokesVector::intensity`
                // on the CPU side -- negative `I` is unphysical on either side.
                (*radiance)[k] = fma(max((*stokes)[k].x, 0.0) * mis_weight, env, (*radiance)[k]);
            }
            // The only site that sets this: staged exit-split contributions commit
            // only when the shared/hero path itself reaches this environment lookup.
            (*path_escaped) = true;
            return BOUNCE_STATUS_TERMINATE;
        }

        // Mirrors optics::raytracer::trace_spectral_ray_inner's restructured bounce
        // loop: attempt a Henyey-Greenstein scattering event somewhere along this
        // segment before the plane-of-incidence rotation / facet processing below.
        // Gated on `material.scattering_sigma_s > 0.0` -- every scene with it `<= 0.0`
        // skips this block entirely, matching the CPU's default-off bit-identity
        // guarantee.
        if ((*inside_gem) && material.scattering_sigma_s > 0.0) {
            var s_axis = vec3<f32>(0.0, 0.0, 0.0);
            if ((*have_prev_plane_normal)) {
                s_axis = (*prev_plane_normal);
            }
            // P1 (assigned-mode absorption): mirrors the absorption block's
            // `is_anisotropic`/`is_biaxial` branching below -- `try_scatter_step` feeds
            // the same `channel_absorption_alphas_assigned` the absorption block uses.
            // The propagation direction fed to every alpha function here is the WAVE
            // NORMAL `k`, not the Poynting direction `S` -- (*current_k), not (*current_dir).
            var alphas: array<f32, 8>;
            if (is_anisotropic) {
                for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
                    // alpha_o/alpha_e/alpha_beta hoisted above (per-ray, not per-bounce).
                    if (is_biaxial && material.has_beta_ray != 0u) {
                        alphas[k] = assigned_mode_alpha_biaxial(
                            alpha_o_hoisted[k], alpha_beta_hoisted[k], alpha_e_hoisted[k],
                            n_alpha_hero, n_beta_hero, n_gamma_hero, biax_ax0, biax_ax1, biax_ax2,
                            c_axis, (*current_k), (*is_extraordinary),
                        );
                    } else {
                        alphas[k] = assigned_mode_alpha_uniaxial(
                            alpha_o_hoisted[k], alpha_e_hoisted[k], c_axis, (*current_k), (*is_extraordinary),
                            n_o_hero_seed, n_e_hero_seed,
                        );
                    }
                }
            } else {
                // Isotropic-by-symmetry material: keeps the OLD DOP-blended call,
                // unchanged, using the real Stokes vector still available here (unlike
                // the CPU port, which has no Stokes parameter left on its own
                // assigned-mode driver at all -- see
                // optics::raytracer::absorption::channel_absorption_alphas_assigned's
                // own doc comment for why). For an isotropic tensor (alpha_o == alpha_e,
                // every cubic built-in) quadratic_form is direction-independent, so this
                // is numerically a no-op relative to the CPU's new direct-midpoint
                // formula -- kept as-is here purely to avoid touching an
                // already-verified code path for no observable benefit.
                let eigen_a = ordinary_eigen_polarization((*current_k), c_axis);
                let eigen_b = extraordinary_eigen_polarization((*current_k), c_axis);
                for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
                    alphas[k] = pleochroic_channel_alpha(
                        alpha_o_hoisted[k], alpha_e_hoisted[k], c_axis, s_axis, (*current_k), eigen_a, eigen_b, (*stokes)[k],
                    );
                }
            }
            let sc = maybe_scatter_or_extinguish(
                alphas, material.scattering_sigma_s, material.scattering_g, (*current_dir), hit.t,
                material.absorption_path_scale, seed0, bounce, stokes, path_pdf,
            );
            if (sc.scattered != 0u) {
                let scatter_point = (*current_origin) + sc.t_free * (*current_dir);
                let old_dir = (*current_dir);
                if (params.env_mode == 2u) {
                    nee_contribution_hg_scatter(
                        lambdas, n_o_hero_seed, scatter_point, old_dir,
                        material.scattering_g, seed0, bounce, stokes, radiance, alphas,
                    );
                    (*pending_light_mis) = henyey_greenstein_phase(dot(sc.new_dir, old_dir), material.scattering_g);
                    (*pending_light_mis_dir) = sc.new_dir;
                }
                (*current_origin) = scatter_point;
                (*current_dir) = sc.new_dir;
                // A scattering event depolarizes, so `k` collapses to `S` going forward.
                (*current_k) = sc.new_dir;
                // Scattered Stokes vectors are already depolarized, so the previous
                // plane of incidence is not physically meaningful -- reset it
                // like the pre-first-bounce state.
                (*have_prev_plane_normal) = false;

                if (bounce > 4u) {
                    var max_intensity: f32 = 0.0;
                    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
                        max_intensity = max(max_intensity, max((*stokes)[k].x, 0.0));
                    }
                    let q = clamp(max_intensity, RR_FLOOR, 1.0);
                    let rr_rand = f32(hash_u32(seed0 ^ hash_u32(bounce ^ RUSSIAN_ROULETTE_STREAM))) / 4294967295.0;
                    if (rr_rand > q) {
                        return BOUNCE_STATUS_TERMINATE;
                    }
                    // `split_radiance` rides along on the same `1/q` survival rescale
                    // as `stokes`.
                    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
                        (*stokes)[k] = (*stokes)[k] * (1.0 / q);
                        (*split_radiance)[k] = (*split_radiance)[k] / q;
                    }
                }
                return BOUNCE_STATUS_CONTINUE;
            }
            // No scatter event fired: the path survived to the facet boundary, and
            // `maybe_scatter_or_extinguish` already applied this segment's full
            // extinction weight (absorption AND the scattering removal probability)
            // to `stokes`/`path_pdf`. Fall through to the facet-dispatch code below,
            // but see the (skipped) absorption block further down for why it is not
            // applied a second time.
        }

        let hit_point = (*current_origin) + hit.t * (*current_dir);
        // See shading_normal_near_edge's own doc comment.
        var normal = shading_normal_near_edge(hit_point, hit.facet_idx, hit.normal, material.edge_rounding_radius);
        // `wave_dir_at_bounce` is the wave normal `k` (== (*current_k)), used for
        // EVERY index lookup, cos_i/sin_i, Snell/Fresnel evaluation, TIR decision, and
        // the Stokes plane-of-incidence frame below -- see
        // optics::raytracer::refraction's "wave normal vs Poynting direction" design
        // note. The Poynting/energy direction `S` (== (*current_dir)) is used directly
        // (geometric origin-advance/intersection only) where still needed.
        // `wave_dir_at_bounce == (*current_dir)` trivially outside the crystal and for the
        // uniaxial ordinary eigenmode, so every such case is bit-identical to the plain
        // all-`(*current_dir)` code.
        let wave_dir_at_bounce = (*current_k);

        // Plane-of-incidence frame rotation (signed psi via atan2).
        let cpn_raw = cross(wave_dir_at_bounce, normal);
        let cpn_len2 = dot(cpn_raw, cpn_raw);
        var current_plane_normal: vec3<f32>;
        if (cpn_len2 > 0.0) {
            current_plane_normal = cpn_raw / sqrt(cpn_len2);
        } else {
            current_plane_normal = vec3<f32>(0.0, 0.0, 0.0);
        }
        if ((*have_prev_plane_normal) && cpn_len2 > 1e-6 && dot((*prev_plane_normal), (*prev_plane_normal)) > 1e-6) {
            let psi = signed_frame_rotation_psi((*prev_plane_normal), current_plane_normal, wave_dir_at_bounce);
            let rot = mueller_frame_rotation(psi);
            for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
                (*stokes)[k] = rot * (*stokes)[k];
            }
        }
        (*prev_plane_normal) = current_plane_normal;
        (*have_prev_plane_normal) = true;

        if ((*inside_gem)) {
            normal = -normal;
            // A scattering-active material's extinction for this segment was
            // already applied above (in the `maybe_scatter_or_extinguish` no-scatter
            // branch) -- applying the plain absorption loop here too would charge
            // this segment's absorption TWICE. `material.scattering_sigma_s <= 0.0`
            // is exactly the pre-Task-1 case (including every existing scene), where
            // the block above never ran and this is the ONLY absorption
            // application, matching the CPU's identical guard.
            if (material.scattering_sigma_s <= 0.0) {
                // P1 (assigned-mode absorption): optics::raytracer::absorption::
                // channel_absorption_alphas_assigned. `(*is_extraordinary)` names which
                // eigenmode this path was assigned to at its most recent air->crystal
                // entry -- computed fresh from the CURRENT wave normal
                // (`wave_dir_at_bounce`) every bounce, not read off `(*stokes)[k]`'s
                // (possibly azimuth-drifted) Stokes state. The isotropic branch keeps
                // the OLD DOP-blended call unchanged -- see the scatter block's own
                // comment above for why.
                var alphas_interior: array<f32, 8>;
                if (is_anisotropic) {
                    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
                        if (is_biaxial && material.has_beta_ray != 0u) {
                            alphas_interior[k] = assigned_mode_alpha_biaxial(
                                alpha_o_hoisted[k], alpha_beta_hoisted[k], alpha_e_hoisted[k],
                                n_alpha_hero, n_beta_hero, n_gamma_hero, biax_ax0, biax_ax1, biax_ax2,
                                c_axis, wave_dir_at_bounce, (*is_extraordinary),
                            );
                        } else {
                            alphas_interior[k] = assigned_mode_alpha_uniaxial(
                                alpha_o_hoisted[k], alpha_e_hoisted[k], c_axis, wave_dir_at_bounce, (*is_extraordinary),
                                n_o_hero_seed, n_e_hero_seed,
                            );
                        }
                    }
                } else {
                    let eigen_a = ordinary_eigen_polarization(wave_dir_at_bounce, c_axis);
                    let eigen_b = extraordinary_eigen_polarization(wave_dir_at_bounce, c_axis);
                    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
                        alphas_interior[k] = pleochroic_channel_alpha(
                            alpha_o_hoisted[k], alpha_e_hoisted[k], c_axis, current_plane_normal, wave_dir_at_bounce, eigen_a, eigen_b, (*stokes)[k],
                        );
                    }
                }
                for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
                    // (absorption path scale): mirrors
                    // optics::raytracer::absorption::apply_absorption's own
                    // `path_len * ctx.material.absorption_path_scale` multiply exactly.
                    let scaled_hit_t = hit.t * material.absorption_path_scale;
                    // `exp_poly`, not the `exp()` builtin --
                    // `apply_absorption` calls `crate::simd::exp_f32x8`, not `f32::exp`.
                    let trans_factor = exp_poly(-alphas_interior[k] * scaled_hit_t);
                    (*stokes)[k] = (*stokes)[k] * trans_factor;
                }
            }
        }

        // angle of incidence measured against the WAVE NORMAL `k`
        // (`wave_dir_at_bounce`), not the Poynting/energy direction `S` -- see this
        // block's own design note above.
        let cos_i = clamp(dot(-wave_dir_at_bounce, normal), 0.0, 1.0);
        let sin_i = sqrt(max(fma(-cos_i, cos_i, 1.0), 0.0));

        // theta_c fixed-point iteration (optics::raytracer::theta_c_for_bounce)
        // plus the per-channel ordinary/effective-extraordinary index pair
        // (optics::raytracer::per_channel_uniaxial_indices, called once per channel via
        // per_channel_uniaxial_index -- see transport_physics.wgsl). For a cubic
        // material (is_anisotropic == false) this reduces to n_eff_ch[k] == n_o_ch[k]
        // for every k, reducing to the isotropic-only computation. For a biaxial
        // material this is a DEAD (unused) computation -- see transport_physics.wgsl's
        // `theta_c_for_bounce` doc comment. Fed `wave_dir_at_bounce` (`k`), not
        // `(*current_dir)` (`S`).
        let theta_c = theta_c_for_bounce(normal, wave_dir_at_bounce, cos_i, (*inside_gem), is_anisotropic, c_axis, n_o_hero_seed, n_e_hero_seed);

        // `n_o_ch[k]` is exactly `n_o_hoisted[k]` (both `dispersion_evaluate` on
        // the same `(*lambdas)[k]`, hoisted above) -- only the theta_c-dependent
        // `n_eff_k` half of `per_channel_uniaxial_index` still needs to run every
        // bounce, so that's all this loop does now, reproducing that function's own
        // `n_e_k`/`effective_extraordinary_index` body (see transport_physics.wgsl) on
        // the hoisted `n_o_hoisted[k]` rather than calling the full function (which
        // would redundantly re-evaluate dispersion) -- bit-identical either way.
        //
        // n_e_k mirrors `GemMaterial::extraordinary_index_at` exactly: a genuine
        // wavelength-dependent evaluation of the material's own extraordinary-ray
        // curve when `material.has_extraordinary_dispersion != 0`
        // (Quartz/Amethyst/Citrine), else the constant-offset
        // `n_o_hoisted[k] + birefringence_delta` approximation.
        var n_eff_ch: array<f32, 8>;
        // RAW (not theta_c-projected) per-channel extraordinary index, hoisted here so
        // the uniaxial entry/internal dispatch arms below can read it directly instead
        // of recomputing the dispersion evaluation.
        var n_e_raw_ch: array<f32, 8>;
        for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
            var n_e_k: f32;
            if (material.has_extraordinary_dispersion != 0u) {
                n_e_k = extraordinary_dispersion_evaluate(material.extraordinary_model_type, material.extraordinary_param_a, material.extraordinary_param_b, (*lambdas)[k]);
            } else {
                n_e_k = n_o_hoisted[k] + birefringence_delta;
            }
            n_e_raw_ch[k] = n_e_k;
            var n_eff_k = n_o_hoisted[k];
            if (is_anisotropic) {
                n_eff_k = effective_extraordinary_index(n_o_hoisted[k], n_e_k, theta_c);
            }
            n_eff_ch[k] = n_eff_k;
        }
        let n_o_hero = n_o_hoisted[0];
        // Deliberately the constant-offset form, NOT the accurate `n_e_hero_seed`
        // computed once per ray above -- this `n_e_hero` feeds only the
        // walk-off/direction (Poynting) approximation (`extraordinary_poynting_dir`
        // below), where the existing constant-offset behaviour is deliberately kept
        // as-is -- mirrors `BounceRefractionGeometry::n_e_hero`'s identical comment on
        // the CPU side (`optics::raytracer::refraction`, P5's fix site).
        let n_e_hero = n_o_hero + birefringence_delta;

        // Shared local frame + optic-axis direction cosines `uniaxial_fresnel`'s
        // closed-form solver needs (Lekner 1991), built once per bounce -- mirrors
        // `BounceRefractionGeometry::uniaxial_frame`'s "Some only when genuinely
        // uniaxial" contract; harmless to build unconditionally since every use below
        // is gated on `is_anisotropic && !is_biaxial`. `entry_incidence_frame`
        // (`n1 == 1.0` fixed for air->crystal entry) is likewise built once here even
        // though only the entry arm below consumes it.
        let uframe = uniaxial_frame_build(wave_dir_at_bounce, normal, c_axis, cos_i, sin_i);
        let uinc_frame = entry_incidence_frame(1.0, uframe);
        // Mirrors `BounceRefractionGeometry::uniaxial_frame`'s `Some` condition
        // exactly -- `apply_tir_bounce`'s CPU uniaxial branch gates on exactly this,
        // with no further degenerate-axis check, so this flag mirrors it bit-for-bit
        // including that narrow gap.
        let uniaxial_active = is_anisotropic && !is_biaxial;
        // Degenerate wave-normal-parallel-to-optic-axis limit -- mirrors
        // `apply_partial_fresnel_bounce`'s identical cross-product-length guard, where
        // falling through to the existing scalar-at-`n_o` machinery is exact at this
        // limit, not an approximation. Used only by the entry/general-internal dispatch
        // arms below; `apply_tir_bounce`'s CPU uniaxial branch has no equivalent guard
        // (a pre-existing narrow gap this port deliberately mirrors on both sides).
        let uniaxial_nondegenerate = uniaxial_active
            && dot(cross(wave_dir_at_bounce, c_axis), cross(wave_dir_at_bounce, c_axis)) > 1e-6;

        // optics::raytracer::{hero_biaxial_wave_dirs, per_channel_biaxial_indices}.
        // "mode A" is the faster (lower-index) root, "mode B" the slower -- see this
        // file's header comment. Zero-initialized and never consulted downstream unless
        // `is_biaxial` guards the read, exactly mirroring the CPU arrays' own
        // "computed unconditionally, only ever POPULATED when is_biaxial" contract.
        var wave_dir_a_hero = vec3<f32>(0.0, 0.0, 0.0);
        var wave_dir_b_hero = vec3<f32>(0.0, 0.0, 0.0);
        var n_biax_a_ch: array<f32, 8>;
        var n_biax_b_ch: array<f32, 8>;
        var n_alpha_ch: array<f32, 8>;
        var n_gamma_ch: array<f32, 8>;
        for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
            n_biax_a_ch[k] = 0.0;
            n_biax_b_ch[k] = 0.0;
            n_alpha_ch[k] = 0.0;
            n_gamma_ch[k] = 0.0;
        }
        if (is_biaxial) {
            if ((*inside_gem)) {
                // both biaxial modes walk off (see optics::raytracer::refraction's
                // hero_biaxial_wave_dirs doc comment) -- k, not S.
                wave_dir_a_hero = wave_dir_at_bounce;
                wave_dir_b_hero = wave_dir_at_bounce;
            } else {
                let res_a = biaxial_resolve_entry_mode(n_alpha_hero, n_beta_hero, n_gamma_hero, biax_ax0, biax_ax1, biax_ax2, wave_dir_at_bounce, normal, cos_i, n_o_hero_seed, false);
                let res_b = biaxial_resolve_entry_mode(n_alpha_hero, n_beta_hero, n_gamma_hero, biax_ax0, biax_ax1, biax_ax2, wave_dir_at_bounce, normal, cos_i, n_o_hero_seed, true);
                wave_dir_a_hero = res_a.wave_dir;
                wave_dir_b_hero = res_b.wave_dir;
            }
            for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
                // optics::materials::GemMaterial::biaxial_indicatrix's per-channel
                // n_beta/n_alpha/n_gamma -- n_beta_k is bit-identical to n_o_hoisted[k]
                // (both `dispersion.evaluate((*lambdas)[k])`, same inputs), so reused
                // rather than recomputed.
                let n_beta_k = n_o_hoisted[k];
                let n_alpha_k = n_beta_k - material.dispersion.biaxial_delta_beta_alpha;
                let n_gamma_k = n_alpha_k + birefringence_delta;
                n_alpha_ch[k] = n_alpha_k;
                n_gamma_ch[k] = n_gamma_k;
                let ni_a = biaxial_wave_indices(n_alpha_k, n_beta_k, n_gamma_k, biax_ax0, biax_ax1, biax_ax2, wave_dir_a_hero);
                let ni_b = biaxial_wave_indices(n_alpha_k, n_beta_k, n_gamma_k, biax_ax0, biax_ax1, biax_ax2, wave_dir_b_hero);
                n_biax_a_ch[k] = ni_a.y;
                n_biax_b_ch[k] = ni_b.x;
            }
        }
        let n_biax_a_hero = n_biax_a_ch[0];
        let n_biax_b_hero = n_biax_b_ch[0];

        // While inside an anisotropic crystal, the medium
        // index this ray is currently in is mode A or mode B depending on which
        // eigenmode `(*is_extraordinary)` selected at the most recent entry; outside the
        // crystal (or for an isotropic material) it is always the mode-B array, which
        // for a cubic material equals n_o_hoisted exactly (see per_channel_uniaxial_index).
        // `is_biaxial` selects which pair of arrays ("mode A"/"mode B") is consulted --
        // the biaxial ones computed just above, or the uniaxial n_o_hoisted/n_eff_ch pair.
        let use_mode_a_medium = is_anisotropic && (*inside_gem) && !(*is_extraordinary);
        var n_medium_ch: array<f32, 8>;
        for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
            var mode_a_k: f32;
            var mode_b_k: f32;
            if (is_biaxial) {
                mode_a_k = n_biax_a_ch[k];
                mode_b_k = n_biax_b_ch[k];
            } else {
                mode_a_k = n_o_hoisted[k];
                mode_b_k = n_eff_ch[k];
            }
            n_medium_ch[k] = select(mode_b_k, mode_a_k, use_mode_a_medium);
        }
        let n_medium_hero = n_medium_ch[0];
        let n1 = select(1.0, n_medium_hero, (*inside_gem));
        let n2 = select(n_medium_hero, 1.0, (*inside_gem));
        let eta = n1 / n2;
        let sin2_t = eta * eta * fma(-cos_i, cos_i, 1.0);

        // Which specular/diffuse treatment
        // this facet gets. Bounds-checked (mirrors optics::raytracer::trace_spectral_ray_inner's
        // `facet_finishes.get(hit_rec.facet_idx).copied().unwrap_or_default()`): an
        // index past the end of a shorter-than-`planes` buffer defaults to
        // `facet_finish::POLISHED`, exactly as the CPU's `Default` does.
        // `select`'s two value arguments are BOTH evaluated in WGSL (no short-circuit),
        // so an `if` guard is used instead of `select` here -- indexing
        // `facet_finishes[hit.facet_idx]` unconditionally would attempt an
        // out-of-bounds storage-buffer read whenever `facet_idx >= arrayLength(...)`
        // (harmless per WGSL's robustness guarantees, but its RESULT is
        // implementation-defined, so this must never be allowed to influence `finish`).
        var finish: u32 = 0u;
        if (hit.facet_idx < arrayLength(&facet_finishes)) {
            finish = facet_finishes[hit.facet_idx];
        }
        // Captured before any of the three bounce-dispatch arms below run -- see
        // optics::raytracer::dispatch_bounce's doc comment for why `was_internal_reflection`
        // is always computed against the PRE-bounce `(*inside_gem)`. The TIR/reflect/refract
        // arms below never need this explicitly (they either never touch `(*inside_gem)` at
        // all, or only flip it once at their very end, after which nothing else in that
        // arm reads it this bounce) -- only the frosted arm, which can update `(*inside_gem)`
        // itself, needs the pre-bounce value spelled out separately.
        let pre_bounce_inside_gem = (*inside_gem);

        if (finish == FACET_FINISH_FROSTED) {
            // optics::raytracer::apply_frosted_bounce (via dispatch_bounce): the
            // REPLACEMENT for the TIR/partial-reflect/refract dispatch below, not an
            // addition to it -- see transport_physics.wgsl's own doc comment for the
            // achromatic-by-design physics this must preserve exactly.
            let fb = apply_frosted_bounce(
                is_anisotropic, sin2_t, n1, n2, cos_i, normal, (*inside_gem), (*is_extraordinary),
                seed0, bounce, stokes, path_pdf,
            );
            if (fb.new_inside_gem == 0u) {
                let ext_normal = select(normal, -normal, pre_bounce_inside_gem);
                if (params.env_mode == 2u) {
                    nee_contribution_frosted_exterior(
                        lambdas, ext_normal, seed0, bounce, stokes, radiance,
                    );
                    (*pending_light_mis) = dot(fb.new_dir, ext_normal) / PI;
                    // `fb.new_dir` is already the true exterior propagation direction
                    // here (a frosted transmit needs no further refraction before a
                    // possible direct escape), mirroring `dispatch_bounce`'s identical
                    // frosted-arm carry on the CPU side.
                    (*pending_light_mis_dir) = fb.new_dir;
                }
            }
            (*current_origin) = hit_point + fb.new_dir * RAY_EPS;
            (*current_dir) = fb.new_dir;
            // a frosted (diffuse) bounce already depolarizes -- k collapses to S,
            // mirroring optics::raytracer::transport::dispatch_bounce's identical
            // treatment of FacetFinish::Frosted.
            (*current_k) = fb.new_dir;
            (*inside_gem) = fb.new_inside_gem != 0u;
            if (fb.has_extraordinary_update != 0u) {
                (*is_extraordinary) = fb.extraordinary_update != 0u;
            }
            // Same was_internal_reflection formula optics::raytracer::dispatch_bounce
            // uses for every bounce kind -- true for the frosted TIR-forced and reflect
            // arms ((*inside_gem) unchanged, no extraordinary update reported), false for
            // the transmit arm ((*inside_gem) always flips there).
            let was_internal_reflection = pre_bounce_inside_gem
                && (fb.has_extraordinary_update == 0u)
                && ((*inside_gem) == pre_bounce_inside_gem);
            if (is_anisotropic && was_internal_reflection) {
                // No stokes/path_pdf scaling here -- see internal_mode_coupling_draw's
                // doc comment: this is a RELABELING of which eigenmode governs the
                // NEXT bounce, not a SPLIT into two rays, so the matching unbiased
                // scale factor is 1.0 (no-op) -- the same 1.0 conclusion the entry
                // split itself reaches, for a different reason (see this file's header
                // comment / `apply_refract_bounce`'s doc comment on the CPU side).
                (*is_extraordinary) = internal_mode_coupling_draw(
                    c_axis, is_biaxial, current_plane_normal, fb.new_dir,
                    (*stokes)[0].x, (*stokes)[0].y, (*stokes)[0].z, false, 0.0, seed0, bounce,
                );
            }
        } else if (sin2_t > 1.0) {
            // Hero forced TIR (probability 1, no pdf division needed).
            // full uniaxial Fresnel (Lekner 1991): the closed-form o<->e-coupled
            // TIR reflectance -- mirrors
            // `optics::raytracer::refraction::apply_tir_bounce`'s own uniaxial branch
            // bit-for-bit, INCLUDING that function's own degenerate
            // wave-normal-parallel-to-optic-axis guard: `uniaxial_nondegenerate`
            // (not the plain `uniaxial_active`, which every OTHER dispatch arm in this
            // function deliberately avoids for exactly this reason -- see that flag's
            // own doc comment) falls through to the scalar branch below at that limit,
            // exactly as the CPU's now-guarded `apply_tir_bounce` does. `tir_exact_p_o`
            // is the hero channel's own `R_o/(R_o+R_e)`, fed to
            // `internal_mode_coupling_draw` below exactly as `apply_tir_bounce`'s own
            // `exact_p_o` return value is on the CPU side.
            var tir_exact_p_o: f32 = 0.5;
            if (uniaxial_nondegenerate) {
                for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
                    let n_inc_k = select(1.0, n_medium_ch[k], (*inside_gem));
                    let sol = internal_solve(n_inc_k, n_o_hoisted[k], n_e_raw_ch[k], c_axis, uframe, !(*is_extraordinary));
                    let flux_inc = max(sol.flux_inc, 1e-12);
                    let ro_pow = cplx_norm_sqr(sol.r_o) * sol.flux_ro;
                    let re_pow = cplx_norm_sqr(sol.r_e) * sol.flux_re;
                    let r_total_k = min((ro_pow + re_pow) / flux_inc, 1.0);
                    (*stokes)[k] = (*stokes)[k] * r_total_k;
                    let n2k_dbg = select(n_medium_ch[k], 1.0, (*inside_gem));
                    let etak_dbg = n_inc_k / n2k_dbg;
                    let sin2_t_k = etak_dbg * etak_dbg * fma(-cos_i, cos_i, 1.0);
                    if (sin2_t_k <= 1.0) {
                        (*path_pdf)[k] = (*path_pdf)[k] * clamp(r_total_k, 1e-4, 1.0 - 1e-4);
                    }
                    if (k == 0u) {
                        tir_exact_p_o = ro_pow / max(ro_pow + re_pow, 1e-12);
                    }
                }
            } else {
                for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
                    let n1k = select(1.0, n_medium_ch[k], (*inside_gem));
                    let n2k = select(n_medium_ch[k], 1.0, (*inside_gem));
                    let etak = n1k / n2k;
                    let sin2_t_k = etak * etak * fma(-cos_i, cos_i, 1.0);
                    if (sin2_t_k > 1.0) {
                        let delta_k = tir_phase_delta(n1k, cos_i, sin_i);
                        (*stokes)[k] = mueller_tir_retardation(delta_k) * (*stokes)[k];
                    } else {
                        let cos_t_k = sqrt(max(1.0 - sin2_t_k, 0.0));
                        let r_s_k = fma(n2k, -cos_t_k, n1k * cos_i) / fma(n2k, cos_t_k, n1k * cos_i);
                        let r_p_k = fma(n1k, -cos_t_k, n2k * cos_i) / fma(n1k, cos_t_k, n2k * cos_i);
                        (*stokes)[k] = mueller_fresnel_reflection(r_s_k, r_p_k) * (*stokes)[k];
                        let r_unpol_k = clamp(0.5 * fma(r_p_k, r_p_k, r_s_k * r_s_k), R_UNPOL_MIN, R_UNPOL_MAX);
                        (*path_pdf)[k] = (*path_pdf)[k] * r_unpol_k;
                    }
                }
            }
            // reflects the WAVE NORMAL `k` (not `S`), then re-derives `S'` for the
            // reflected `k'` via `poynting_dir_for_mode` -- see this file's own design
            // note above `wave_dir_at_bounce`.
            let k_prime = wave_dir_at_bounce - 2.0 * dot(wave_dir_at_bounce, normal) * normal;
            let s_prime = poynting_dir_for_mode(
                is_anisotropic, is_biaxial, (*inside_gem), (*is_extraordinary), k_prime, c_axis,
                n_o_hero, n_e_hero, n_alpha_hero, n_beta_hero, n_gamma_hero, biax_ax0, biax_ax1, biax_ax2,
            );
            (*current_origin) = hit_point + s_prime * RAY_EPS;
            (*current_dir) = s_prime;
            (*current_k) = k_prime;

            // TIR is always an internal reflection (`n1 > n2` for the hero
            // channel implies `(*inside_gem)`, exactly as on the CPU side).
            if (is_anisotropic) {
                // No stokes/path_pdf scaling -- relabeling, not a split; see
                // internal_mode_coupling_draw's doc comment.
                (*is_extraordinary) = internal_mode_coupling_draw(
                    c_axis, is_biaxial, current_plane_normal, k_prime,
                    (*stokes)[0].x, (*stokes)[0].y, (*stokes)[0].z, uniaxial_active, tir_exact_p_o, seed0, bounce,
                );
            }
        } else if (uniaxial_nondegenerate) {
            // full uniaxial Fresnel (Lekner 1991): entry AND general (non-forced)
            // internal/exit dispatch, in full, self-contained -- mirrors
            // optics::raytracer::refraction::apply_partial_fresnel_bounce's own early
            // returns to apply_uniaxial_entry_bounce/apply_uniaxial_internal_bounce.
            // `n_e_hero`/`n_e_ch` used elsewhere in this kernel for WALK-OFF/DIRECTION
            // purposes are the constant-offset `n_o + birefringence_delta`
            // approximation (matching `BounceRefractionGeometry::n_e_hero` on the CPU
            // side); the closed-form FRESNEL solve below instead needs the material's
            // own genuine extraordinary-ray dispersion curve where one exists (`n_e_
            // raw_ch`/`n_e_hero_solve`, matching `GemMaterial::extraordinary_index_at`
            // -- see `apply_uniaxial_entry_bounce`'s and `apply_uniaxial_internal_
            // bounce`'s own local `n_e_hero` shadowing on the CPU side for why these
            // are deliberately two different values, not a duplicate).
            let n_e_hero_solve = n_e_raw_ch[0];

            if (!(*inside_gem)) {
                // ---- ENTRY: mirrors apply_uniaxial_entry_bounce ----
                let pair_hero = entry_solve_pair_with_incidence(uinc_frame, 1.0, n_o_hero, n_e_hero_solve, c_axis, uframe);
                let sol_s_hero = pair_hero.s_sol;
                let sol_p_hero = pair_hero.p_sol;
                let reflect_mueller_hero = jones_to_mueller(sol_s_hero.r_s, sol_p_hero.r_s, sol_s_hero.r_p, sol_p_hero.r_p);
                let entry_r_branch = clamp(reflect_mueller_hero[0][0], R_UNPOL_SELECT_MIN, R_UNPOL_SELECT_MAX);
                let entry_inc_flux = 0.5 * uframe.cos_i;
                let p_o_raw = mode_power(sol_s_hero.t_o, sol_p_hero.t_o, sol_s_hero.flux_o, entry_inc_flux, (*stokes)[0]);
                let p_e_raw = mode_power(sol_s_hero.t_e, sol_p_hero.t_e, sol_s_hero.flux_e, entry_inc_flux, (*stokes)[0]);
                let p_o_hero = clamp(p_o_raw / max(p_o_raw + p_e_raw, 1e-12), R_UNPOL_SELECT_MIN, R_UNPOL_SELECT_MAX);
                let entry_mode_split_rand = f32(hash_u32(seed0 ^ hash_u32(bounce ^ BIREFRINGENT_SPLIT_STREAM))) / 4294967295.0;
                let use_extraordinary_entry = entry_mode_split_rand < (1.0 - p_o_hero);
                var p_mode_hero_frac = p_o_hero;
                if (use_extraordinary_entry) {
                    p_mode_hero_frac = 1.0 - p_o_hero;
                }

                let n2_hero_dir_e = select(n_o_hero, n2, use_extraordinary_entry);
                let eta_dir_e = n1 / n2_hero_dir_e;
                let sin2_t_dir_e = min(eta_dir_e * eta_dir_e * fma(-cos_i, cos_i, 1.0), 1.0);
                let cos_t_dir_e = sqrt(max(1.0 - sin2_t_dir_e, 0.0));
                let refr_wave_dir_e = normalize(eta_dir_e * wave_dir_at_bounce + fma(eta_dir_e, cos_i, -cos_t_dir_e) * normal);
                var final_refr_dir_e = refr_wave_dir_e;
                if (use_extraordinary_entry) {
                    final_refr_dir_e = extraordinary_poynting_dir(refr_wave_dir_e, c_axis, n_o_hero, n_e_hero);
                }
                let p_axis_transmitted = cross(refr_wave_dir_e, uframe.s_axis);

                let entry_rng_bounce = f32(hash_u32(seed0 ^ hash_u32(bounce ^ FRESNEL_BRANCH_STREAM))) / 4294967295.0;

                if (entry_rng_bounce < entry_r_branch) {
                    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
                        let pair_k = entry_solve_pair_with_incidence(uinc_frame, 1.0, n_o_hoisted[k], n_e_raw_ch[k], c_axis, uframe);
                        let mueller_k = jones_to_mueller(pair_k.s_sol.r_s, pair_k.p_sol.r_s, pair_k.s_sol.r_p, pair_k.p_sol.r_p);
                        let r_unpol_k = clamp(mueller_k[0][0], 1e-4, 1.0 - 1e-4);
                        (*stokes)[k] = (mueller_k * (*stokes)[k]) * (1.0 / entry_r_branch);
                        (*path_pdf)[k] = (*path_pdf)[k] * r_unpol_k;
                    }
                    let new_k_e = wave_dir_at_bounce - 2.0 * dot(wave_dir_at_bounce, normal) * normal;
                    (*current_origin) = hit_point + new_k_e * RAY_EPS;
                    (*current_dir) = new_k_e;
                    (*current_k) = new_k_e;
                } else {
                    // this is an INTERIOR dispersive event (air->crystal entry) --
                    // mismatch keeps `path_pdf` accumulating (via this event's own
                    // `t_unpol_k`) and narrows `compat` below, no split -- see this
                    // file's own header comment and
                    // `apply_uniaxial_entry_transmit_channels`'s CPU-side doc comment.
                    var entry_dirs: array<vec3<f32>, 8>;
                    var entry_dirs_valid: array<bool, 8>;
                    var entry_hero_match: array<bool, 8>;
                    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
                        let n_o_k = n_o_hoisted[k];
                        var n2k_dir = n_o_k;
                        if (use_extraordinary_entry) {
                            n2k_dir = n_eff_ch[k];
                        }
                        let eta_dir_k = n1 / n2k_dir;
                        let sin2_t_k = eta_dir_k * eta_dir_k * fma(-cos_i, cos_i, 1.0);
                        if (sin2_t_k > 1.0) {
                            (*stokes)[k] = (*stokes)[k] * 0.0;
                            (*path_pdf)[k] = 0.0;
                            continue;
                        }
                        let cos_t_k = sqrt(max(1.0 - sin2_t_k, 0.0));
                        let refr_wave_dir_k = normalize(eta_dir_k * wave_dir_at_bounce + fma(eta_dir_k, cos_i, -cos_t_k) * normal);
                        var final_dir_k = refr_wave_dir_k;
                        if (use_extraordinary_entry) {
                            let n_e_eff_k = n_o_k + birefringence_delta;
                            final_dir_k = extraordinary_poynting_dir(refr_wave_dir_k, c_axis, n_o_k, n_e_eff_k);
                        }
                        entry_dirs[k] = final_dir_k;
                        entry_dirs_valid[k] = true;
                        let direction_matches_entry = dot(final_dir_k, final_refr_dir_e) >= DIRECTION_MATCH_COS_TOL;
                        entry_hero_match[k] = direction_matches_entry;
                        if (!direction_matches_entry) {
                            (*stokes)[k] = (*stokes)[k] * 0.0;
                            // technique k stays a live member of every compatible
                            // channel's MIS family -- keep its own entry-transmit
                            // density accumulating (the SAME t_unpol_k the matching
                            // branch below folds in); only its radiance ends here.
                            let pair_k_mm = entry_solve_pair_with_incidence(uinc_frame, 1.0, n_o_k, n_e_raw_ch[k], c_axis, uframe);
                            let mueller_k_mm = jones_to_mueller(pair_k_mm.s_sol.r_s, pair_k_mm.p_sol.r_s, pair_k_mm.s_sol.r_p, pair_k_mm.p_sol.r_p);
                            let t_unpol_k_mm = clamp(1.0 - mueller_k_mm[0][0], 1e-4, 1.0 - 1e-4);
                            (*path_pdf)[k] = (*path_pdf)[k] * t_unpol_k_mm;
                            continue;
                        }
                        let pair_k = entry_solve_pair_with_incidence(uinc_frame, 1.0, n_o_k, n_e_raw_ch[k], c_axis, uframe);
                        var t_s_row = pair_k.s_sol.t_o;
                        var t_p_row = pair_k.p_sol.t_o;
                        var flux_k = pair_k.s_sol.flux_o;
                        var mode_dir = pair_k.s_sol.o_hat;
                        if (use_extraordinary_entry) {
                            t_s_row = pair_k.s_sol.t_e;
                            t_p_row = pair_k.p_sol.t_e;
                            flux_k = pair_k.s_sol.flux_e;
                            mode_dir = pair_k.s_sol.e_hat;
                        }
                        let p_mode_k = mode_power(t_s_row, t_p_row, flux_k, entry_inc_flux, (*stokes)[k]);
                        let deposit_i = p_mode_k / p_mode_hero_frac;
                        let azimuth = azimuth2_in_frame(mode_dir, uframe.s_axis, p_axis_transmitted);
                        (*stokes)[k] = vec4<f32>(deposit_i, deposit_i * azimuth.x, deposit_i * azimuth.y, 0.0) * (1.0 / (1.0 - entry_r_branch));

                        let mueller_k = jones_to_mueller(pair_k.s_sol.r_s, pair_k.p_sol.r_s, pair_k.s_sol.r_p, pair_k.p_sol.r_p);
                        let t_unpol_k = clamp(1.0 - mueller_k[0][0], 1e-4, 1.0 - 1e-4);
                        (*path_pdf)[k] = (*path_pdf)[k] * t_unpol_k;
                    }
                    narrow_compat(compat, entry_dirs, entry_dirs_valid, entry_hero_match);
                    (*current_origin) = hit_point + final_refr_dir_e * RAY_EPS;
                    (*current_dir) = final_refr_dir_e;
                    (*current_k) = refr_wave_dir_e;
                    (*inside_gem) = true;
                    (*is_extraordinary) = use_extraordinary_entry;
                }
            } else {
                // ---- INTERNAL: mirrors apply_uniaxial_internal_bounce ----
                let sol_hero_i = internal_solve(n1, n_o_hero, n_e_hero_solve, c_axis, uframe, !(*is_extraordinary));
                let flux_inc_hero_i = max(sol_hero_i.flux_inc, 1e-12);
                let ro_pow_hero = cplx_norm_sqr(sol_hero_i.r_o) * sol_hero_i.flux_ro;
                let re_pow_hero = cplx_norm_sqr(sol_hero_i.r_e) * sol_hero_i.flux_re;
                let internal_r_branch = clamp((ro_pow_hero + re_pow_hero) / flux_inc_hero_i, R_UNPOL_SELECT_MIN, R_UNPOL_SELECT_MAX);
                let internal_p_o_exact = ro_pow_hero / max(ro_pow_hero + re_pow_hero, 1e-12);
                let internal_rng_bounce = f32(hash_u32(seed0 ^ hash_u32(bounce ^ FRESNEL_BRANCH_STREAM))) / 4294967295.0;

                if (internal_rng_bounce < internal_r_branch) {
                    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
                        var sol = sol_hero_i;
                        if (k != 0u) {
                            sol = internal_solve(n_medium_ch[k], n_o_hoisted[k], n_e_raw_ch[k], c_axis, uframe, !(*is_extraordinary));
                        }
                        let flux_inc_k = max(sol.flux_inc, 1e-12);
                        let r_total_k = min((cplx_norm_sqr(sol.r_e) * sol.flux_re + cplx_norm_sqr(sol.r_o) * sol.flux_ro) / flux_inc_k, 1.0);
                        (*stokes)[k] = (*stokes)[k] * (r_total_k / internal_r_branch);
                        (*path_pdf)[k] = (*path_pdf)[k] * clamp(r_total_k, 1e-4, 1.0 - 1e-4);
                    }
                    let new_k_i = wave_dir_at_bounce - 2.0 * dot(wave_dir_at_bounce, normal) * normal;
                    (*current_origin) = hit_point + new_k_i * RAY_EPS;
                    (*current_dir) = new_k_i;
                    (*current_k) = new_k_i;
                    (*is_extraordinary) = internal_mode_coupling_draw(
                        c_axis, is_biaxial, current_plane_normal, new_k_i,
                        (*stokes)[0].x, (*stokes)[0].y, (*stokes)[0].z, true, internal_p_o_exact, seed0, bounce,
                    );
                } else {
                    let eta_dir_i = n1 / n2;
                    let sin2_t_dir_i = min(eta_dir_i * eta_dir_i * fma(-cos_i, cos_i, 1.0), 1.0);
                    let cos_t_dir_i = sqrt(max(1.0 - sin2_t_dir_i, 0.0));
                    let hero_refr_dir_i = normalize(eta_dir_i * wave_dir_at_bounce + fma(eta_dir_i, cos_i, -cos_t_dir_i) * normal);

                    // this IS the uniaxial exact EXIT event (crystal -> air) --
                    // mismatch resolves its own split (point 1) and keeps `path_pdf`
                    // accumulating via this event's own exit factor (point 2), but
                    // never narrows `compat` -- see
                    // `apply_uniaxial_internal_transmit_channels`'s CPU-side doc
                    // comment and this file's own header comment.
                    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
                        // captured before any mutation below, so the split branch
                        // (which needs the ORIGINAL incident value after `(*stokes)[k]`
                        // has already been zeroed by the unchanged chromatic-
                        // termination bookkeeping) can still compute k's own
                        // transmission.
                        let original_stokes_i = (*stokes)[k].x;
                        let n_inc_k = n_medium_ch[k];
                        let eta_dir_k = n_inc_k;
                        let sin2_t_k = eta_dir_k * eta_dir_k * fma(-cos_i, cos_i, 1.0);
                        if (sin2_t_k > 1.0) {
                            (*stokes)[k] = (*stokes)[k] * 0.0;
                            (*path_pdf)[k] = 0.0;
                            continue;
                        }
                        let cos_t_k = sqrt(max(1.0 - sin2_t_k, 0.0));
                        let refr_wave_dir_k = normalize(eta_dir_k * wave_dir_at_bounce + fma(eta_dir_k, cos_i, -cos_t_k) * normal);
                        let direction_matches_i = dot(refr_wave_dir_k, hero_refr_dir_i) >= DIRECTION_MATCH_COS_TOL;

                        // Performance: reuse the caller's already-solved hero channel
                        // instead of re-solving the identical boundary system -- needed
                        // by both branches below (matching directly; split via
                        // `compute_uniaxial_exit_transmission`).
                        var sol = sol_hero_i;
                        if (k != 0u) {
                            sol = internal_solve(n_inc_k, n_o_hoisted[k], n_e_raw_ch[k], c_axis, uframe, !(*is_extraordinary));
                        }

                        if (!direction_matches_i) {
                            // Chromatic termination.
                            let prefix_path_pdf_k = (*path_pdf)[k];
                            (*stokes)[k] = (*stokes)[k] * 0.0;
                            (*path_pdf)[k] = 0.0;

                            let et_mm = compute_uniaxial_exit_transmission(sol, internal_r_branch, original_stokes_i);
                            let t_unpol_k_mm = clamp(et_mm.i_unit, 1e-4, 1.0 - 1e-4);
                            (*path_pdf)[k] = prefix_path_pdf_k * t_unpol_k_mm;
                            if (original_stokes_i > 0.0) {
                                try_split_exit_channel(
                                    split_radiance, hit_point, k, (*lambdas)[k], refr_wave_dir_k,
                                    et_mm.transmitted.x, studio_key_dir, studio_fill_dir, studio_sin_lp, observer,
                                );
                            }
                            continue;
                        }

                        // direction_matches: the matching-branch formula.
                        let et = compute_uniaxial_exit_transmission(sol, internal_r_branch, original_stokes_i);
                        (*stokes)[k] = et.transmitted;
                        let t_unpol_k = clamp(et.i_unit, 1e-4, 1.0 - 1e-4);
                        (*path_pdf)[k] = (*path_pdf)[k] * t_unpol_k;
                    }
                    (*current_origin) = hit_point + hero_refr_dir_i * RAY_EPS;
                    (*current_dir) = hero_refr_dir_i;
                    (*current_k) = hero_refr_dir_i;
                    (*inside_gem) = false;
                    // This uniaxial closed-form branch is reached only when
                    // `(*inside_gem)` was already true (the "---- INTERNAL ----" arm),
                    // so a live incoming carry is always a genuine transmit-out here --
                    // pass it through unchanged, mirroring `dispatch_bounce`'s identical
                    // CPU-side carry-through for the generic (isotropic/biaxial) exit.
                    (*pending_light_mis) = phase_pdf_this_check;
                    (*pending_light_mis_dir) = phase_dir_this_check;
                }
            }
        } else {
            // at an air->crystal entry into an anisotropic material, which
            // eigenmode (ordinary/mode-A vs extraordinary/mode-B) this path's single
            // geometric transmission event represents is decided HERE -- before the
            // reflect-vs-transmit draw below -- rather than inside the REFRACT arm's
            // own transmit branch. See
            // optics::raytracer::refraction::apply_partial_fresnel_bounce's doc
            // comment (CPU side) for the full two-part rationale: (P1) the
            // polarization-weighted selection (`entry_eigenmode_selection`) must be
            // shared by both branches below -- a beam already aligned with the
            // ordinary axis should be MORE likely to reflect at the ordinary index
            // too, not just more likely to transmit as ordinary conditional on
            // transmitting -- and (P2) the REFLECT branch's own Fresnel coefficients
            // must be evaluated at the SAME mode's index the transmit branch uses, so
            // `R + T == 1` for whichever mode this draw actually selects. A biaxial
            // material has no uniaxial "ordinary" eigenmode to weight against, so both
            // the selection and the index correction below are gated on `!is_biaxial`.
            let entering_anisotropic = !(*inside_gem) && is_anisotropic;
            var entry_valid = 0u;
            var entry_cos_2psi_o = 0.0;
            var entry_sin_2psi_o = 0.0;
            var p_o = 0.5;
            if (entering_anisotropic && !is_biaxial) {
                let sel = entry_eigenmode_selection(
                    c_axis, current_plane_normal, wave_dir_at_bounce,
                    (*stokes)[0].x, (*stokes)[0].y, (*stokes)[0].z,
                );
                entry_valid = sel.valid;
                entry_cos_2psi_o = sel.cos_2psi_o;
                entry_sin_2psi_o = sel.sin_2psi_o;
                if (sel.valid != 0u) {
                    p_o = sel.p_o;
                }
            }
            let mode_split_rand = f32(hash_u32(seed0 ^ hash_u32(bounce ^ BIREFRINGENT_SPLIT_STREAM))) / 4294967295.0;
            var use_extraordinary = (*is_extraordinary);
            if (entering_anisotropic) {
                // Must match `apply_partial_fresnel_bounce`'s `mode_split_rand < (1.0 -
                // p_o)` EXACTLY (same operator, same sense), not just the same marginal
                // probability: `rand < 1-p_o` and `rand >= p_o` integrate to the same
                // P(extraordinary) but pick different halves of the identical [0,1)
                // draw, so they disagree on which mode a path gets. Previously read
                // `mode_split_rand >= p_o`, silently breaking CPU/GPU parity at every
                // anisotropic entry -- do not "simplify" this back.
                use_extraordinary = mode_split_rand < (1.0 - p_o);
            }
            // The chosen mode's own doubled polarization azimuth, for the per-channel
            // eigenmode projection in the REFRACT arm below -- meaningless unless
            // `entry_valid != 0u`. Extraordinary is perpendicular to ordinary
            // (psi_e == psi_o + 90deg), so its doubled azimuth is the negation of the
            // ordinary one.
            var entry_cos_2psi_x = entry_cos_2psi_o;
            var entry_sin_2psi_x = entry_sin_2psi_o;
            if (use_extraordinary) {
                entry_cos_2psi_x = -entry_cos_2psi_o;
                entry_sin_2psi_x = -entry_sin_2psi_o;
            }

            // use the SELECTED mode's own index for n2 at THIS interface -- `n2`
            // (mode B/extraordinary) unchanged when extraordinary is selected, not
            // entering an anisotropic material at all, or biaxial (`n_o_hero` is a
            // uniaxial-only quantity, not mode A), `n_o_hero` when the ordinary mode
            // is selected instead. Bit-identical to the plain `sqrt(1.0 - sin2_t)` in
            // every case this reduces to (`n2_decision == n2`): both `n1 / n2_decision`
            // and `sin2_t` itself are pure functions of `n1`/`n2`/`cos_i`, so recomputing
            // the ratio here rather than reusing the hero-level `sin2_t` produces the
            // identical bits when the index is unchanged.
            let n2_decision = select(n2, n_o_hero, entering_anisotropic && !is_biaxial && !use_extraordinary);
            let eta_decision = n1 / n2_decision;
            let cos_t = sqrt(max(1.0 - eta_decision * eta_decision * fma(-cos_i, cos_i, 1.0), 0.0));
            let r_s = fma(n2_decision, -cos_t, n1 * cos_i) / fma(n2_decision, cos_t, n1 * cos_i);
            let r_p = fma(n1, -cos_t, n2_decision * cos_i) / fma(n1, cos_t, n2_decision * cos_i);
            let r_unpol_raw = 0.5 * fma(r_p, r_p, r_s * r_s);
            // [0.02, 0.98] here (distinct from the per-channel R_UNPOL_MIN/MAX
            // used only to scale path_pdf, never to divide stokes) caps the
            // `1/r_unpol`/`1/(1-r_unpol)` divisions below at 50x instead of 10,000x at
            // grazing incidence -- still unbiased, just far less firefly-prone.
            let r_unpol = clamp(r_unpol_raw, R_UNPOL_SELECT_MIN, R_UNPOL_SELECT_MAX);
            let rng_bounce = f32(hash_u32(seed0 ^ hash_u32(bounce ^ FRESNEL_BRANCH_STREAM))) / 4294967295.0;

            if (rng_bounce < r_unpol) {
                // REFLECT: each channel applies its OWN Fresnel/TIR matrix, divided by
                // the SAME hero selection probability r_unpol (the PDF-division
                // coupling that makes GPU/CPU float divergence harmless -- see this
                // file's header comment).
                for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
                    let n1k = select(1.0, n_medium_ch[k], (*inside_gem));
                    let n2k = select(n_medium_ch[k], 1.0, (*inside_gem));
                    let etak = n1k / n2k;
                    let sin2_t_k = etak * etak * fma(-cos_i, cos_i, 1.0);
                    var refl: mat4x4<f32>;
                    if (sin2_t_k > 1.0) {
                        let delta_k = tir_phase_delta(n1k, cos_i, sin_i);
                        refl = mueller_tir_retardation(delta_k);
                    } else {
                        let cos_t_k = sqrt(max(1.0 - sin2_t_k, 0.0));
                        let r_s_k = fma(n2k, -cos_t_k, n1k * cos_i) / fma(n2k, cos_t_k, n1k * cos_i);
                        let r_p_k = fma(n1k, -cos_t_k, n2k * cos_i) / fma(n1k, cos_t_k, n2k * cos_i);
                        let r_unpol_k = clamp(0.5 * fma(r_p_k, r_p_k, r_s_k * r_s_k), R_UNPOL_MIN, R_UNPOL_MAX);
                        (*path_pdf)[k] = (*path_pdf)[k] * r_unpol_k;
                        refl = mueller_fresnel_reflection(r_s_k, r_p_k);
                    }
                    (*stokes)[k] = (refl * (*stokes)[k]) * (1.0 / r_unpol);
                }
                // reflects the WAVE NORMAL `k` (not `S`), then re-derives `S'` for
                // the reflected `k'` via `poynting_dir_for_mode` -- see this file's own
                // design note above `wave_dir_at_bounce`.
                let k_prime = wave_dir_at_bounce - 2.0 * dot(wave_dir_at_bounce, normal) * normal;
                let s_prime = poynting_dir_for_mode(
                    is_anisotropic, is_biaxial, (*inside_gem), (*is_extraordinary), k_prime, c_axis,
                    n_o_hero, n_e_hero, n_alpha_hero, n_beta_hero, n_gamma_hero, biax_ax0, biax_ax1, biax_ax2,
                );
                (*current_origin) = hit_point + s_prime * RAY_EPS;
                (*current_dir) = s_prime;
                (*current_k) = k_prime;

                // This arm never changes `(*inside_gem)` (unlike the refract arm
                // below, which always flips it), so `(*inside_gem)` here still holds its
                // pre-bounce value -- an internal reflection iff it was already true.
                if (is_anisotropic && (*inside_gem)) {
                    // No stokes/path_pdf scaling -- relabeling, not a split; see
                    // internal_mode_coupling_draw's doc comment. This arm is only
                    // reached for a biaxial material or a degenerate-axis uniaxial one
                    // (the non-degenerate uniaxial case takes the closed-form
                    // `uniaxial_nondegenerate` branch above instead), so no exact p_o
                    // is available here -- falls back to the polarization-projection
                    // heuristic exactly as before.
                    (*is_extraordinary) = internal_mode_coupling_draw(
                        c_axis, is_biaxial, current_plane_normal, k_prime,
                        (*stokes)[0].x, (*stokes)[0].y, (*stokes)[0].z, false, 0.0, seed0, bounce,
                    );
                }
            } else {
                // REFRACT -- optics::raytracer::apply_refract_bounce.
                // `entering_anisotropic` and `use_extraordinary` are ALREADY resolved
                // above (shared with the REFLECT arm's own r_unpol computation) -- no
                // fresh draw here. On an air->crystal entry into an anisotropic
                // material, the selected mode's own Fresnel transmittance is applied
                // to that mode's PROJECTED Stokes state (see the per-channel loop
                // below) at its own unscaled intensity: the selection probability is
                // set to match this mode's own true physical energy fraction exactly,
                // so multiplying by that fraction and dividing by the identical
                // selection probability cancel -- not the naive `1/0.5` a disjoint
                // split would need. For a cubic material (or any bounce that is not an
                // anisotropic entry) `entering_anisotropic` is false and
                // `use_extraordinary` keeps whatever `(*is_extraordinary)` already was.

                // Direction: the mode-A eigenmode uses n_mode_a and (uniaxial only) is
                // never walked off; the mode-B eigenmode's ENERGY (Poynting) direction
                // is displaced by the walk-off angle -- computed BEFORE the per-channel
                // loop below so each companion channel's own hypothetical direction can
                // be compared against this SAME hero-driven direction. For a biaxial material entering the crystal,
                // NEITHER mode is a plain constant-index Snell refraction -- BOTH modes
                // walk off via `biaxial_mode_poynting_dir`, using `n_biax_a_hero`/
                // `n_biax_b_hero` (the SAME looked-up scalars the per-channel loop's own
                // `k == hero_idx` iteration uses) for self-consistency.
                // `refr_wave_dir` is the SNELL-REFRACTED WAVE NORMAL `k'` (fed from
                // `wave_dir_at_bounce`, not `(*current_dir)`/`S` -- see this file's
                // own design note above `wave_dir_at_bounce`, and rule 4 in particular:
                // "at exit into air, refract k (not S)"). Captured into `new_k` alongside
                // the Poynting-converted `final_refr_dir` (`S'`), since the caller needs
                // BOTH from here on.
                var new_k: vec3<f32>;
                var final_refr_dir: vec3<f32>;
                if (entering_anisotropic && is_biaxial) {
                    let n2_hero_dir = select(n_biax_a_hero, n_biax_b_hero, use_extraordinary);
                    let eta_dir = n1 / n2_hero_dir;
                    let sin2_t_dir = min(eta_dir * eta_dir * fma(-cos_i, cos_i, 1.0), 1.0);
                    let cos_t_dir = sqrt(max(1.0 - sin2_t_dir, 0.0));
                    let refr_wave_dir = normalize(
                        eta_dir * wave_dir_at_bounce + fma(eta_dir, cos_i, -cos_t_dir) * normal,
                    );
                    new_k = refr_wave_dir;
                    final_refr_dir = biaxial_mode_poynting_dir(n_alpha_hero, n_beta_hero, n_gamma_hero, biax_ax0, biax_ax1, biax_ax2, refr_wave_dir, use_extraordinary);
                } else {
                    let n2_hero_dir = select(n2, n_o_hero, entering_anisotropic && !use_extraordinary);
                    let eta_dir = n1 / n2_hero_dir;
                    let sin2_t_dir = min(eta_dir * eta_dir * fma(-cos_i, cos_i, 1.0), 1.0);
                    let cos_t_dir = sqrt(max(1.0 - sin2_t_dir, 0.0));
                    let refr_wave_dir = normalize(
                        eta_dir * wave_dir_at_bounce + fma(eta_dir, cos_i, -cos_t_dir) * normal,
                    );
                    new_k = refr_wave_dir;
                    final_refr_dir = refr_wave_dir;
                    if (entering_anisotropic && use_extraordinary) {
                        final_refr_dir = extraordinary_poynting_dir(refr_wave_dir, c_axis, n_o_hero, n_e_hero);
                    }
                }

                // `is_exit_event` mirrors apply_refract_channel's own
                // `(*inside_gem) && !entering_anisotropic` -- since `entering_anisotropic`
                // is only ever true while `!(*inside_gem)`, this reduces to plain
                // `(*inside_gem)` (still the PRE-flip value here, unchanged until after
                // this loop). Interior (entry) mismatches instead narrow `compat`,
                // never split -- see this file's own header comment.
                let is_exit_event = (*inside_gem);
                var scalar_dirs: array<vec3<f32>, 8>;
                var scalar_dirs_valid: array<bool, 8>;
                var scalar_hero_match: array<bool, 8>;
                for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
                    // captured before any mutation below, so the mismatch/split
                    // branch (which needs the ORIGINAL incident value after `(*stokes)[k]`
                    // has already been zeroed by the unchanged chromatic-termination
                    // bookkeeping) can still compute k's own transmission.
                    let original_stokes_k = (*stokes)[k];
                    let n1k = select(1.0, n_medium_ch[k], (*inside_gem));
                    var n2k = select(n_medium_ch[k], 1.0, (*inside_gem));
                    if (entering_anisotropic && !use_extraordinary) {
                        if (is_biaxial) {
                            n2k = n_biax_a_ch[k];
                        } else {
                            n2k = n_o_hoisted[k];
                        }
                    }
                    let ratio_k = n1k / n2k;
                    let sin2_t_k = (ratio_k * ratio_k) * fma(-cos_i, cos_i, 1.0);
                    if (sin2_t_k > 1.0) {
                        // Chromatic termination: channel k cannot transmit at this
                        // angle even though the hero-driven path did. Both the Stokes
                        // contribution AND the path_pdf are dropped to exactly 0 --
                        // never just down-weighted (see this file's header comment).
                        (*stokes)[k] = (*stokes)[k] * 0.0;
                        (*path_pdf)[k] = 0.0;
                        continue;
                    }
                    let cos_t_k = sqrt(max(1.0 - sin2_t_k, 0.0));
                    let refr_wave_dir_k = normalize(
                        ratio_k * wave_dir_at_bounce + fma(ratio_k, cos_i, -cos_t_k) * normal,
                    );
                    // Fix G (Part 2) / the direction-match identity trap: channel k's own
                    // walk-off, using k's own per-channel indices, compared against the
                    // STORED `final_refr_dir` above -- never a second recomputation of
                    // the hero's own direction (which would be a few ULP different and
                    // chromatically self-terminate the hero channel against itself).
                    // channel k's own biaxial walk-off, using k's own
                    // (n_alpha_ch[k], n_beta_ch := n_o_hoisted[k], n_gamma_ch[k]) evaluated
                    // at k's own single-shot refracted wave direction -- the direct
                    // per-channel generalization of the uniaxial extraordinary_poynting_dir
                    // call below.
                    var final_dir_k = refr_wave_dir_k;
                    if (entering_anisotropic && is_biaxial) {
                        final_dir_k = biaxial_mode_poynting_dir(n_alpha_ch[k], n_o_hoisted[k], n_gamma_ch[k], biax_ax0, biax_ax1, biax_ax2, refr_wave_dir_k, use_extraordinary);
                    } else if (entering_anisotropic && use_extraordinary) {
                        let n_e_k = n_o_hoisted[k] + birefringence_delta;
                        final_dir_k = extraordinary_poynting_dir(refr_wave_dir_k, c_axis, n_o_hoisted[k], n_e_k);
                    }
                    scalar_dirs[k] = final_dir_k;
                    scalar_dirs_valid[k] = true;
                    let direction_matches = dot(final_dir_k, final_refr_dir) >= DIRECTION_MATCH_COS_TOL;
                    scalar_hero_match[k] = direction_matches;
                    if (direction_matches) {
                        let t_s_k = (2.0 * n1k * cos_i) / fma(n2k, cos_t_k, n1k * cos_i);
                        let t_p_k = (2.0 * n1k * cos_i) / fma(n1k, cos_t_k, n2k * cos_i);
                        let trans = mueller_fresnel_transmission(n1k, n2k, cos_i, cos_t_k, t_s_k, t_p_k);
                        // project this channel's incident Stokes state onto the
                        // SELECTED eigenmode before transmission -- see
                        // entry_eigenmode_selection's doc comment for the full
                        // derivation. Fully linear along the mode's own axis (`w`/`V`
                        // zeroed), at this channel's OWN unscaled intensity (the
                        // mode-selection draw's probability matches this mode's true
                        // physical energy fraction exactly, so multiplying by that
                        // fraction and dividing by the identical selection probability
                        // cancel). `entry_valid == 0u` (biaxial, or negligible linear
                        // polarization) leaves `(*stokes)[k]` unprojected.
                        var incident_k = (*stokes)[k];
                        if (entering_anisotropic && entry_valid != 0u) {
                            let i_k = (*stokes)[k].x;
                            incident_k = vec4<f32>(i_k, i_k * entry_cos_2psi_x, i_k * entry_sin_2psi_x, 0.0);
                        }
                        // No `/ split_pdf` -- see the entering_anisotropic comment above.
                        (*stokes)[k] = (trans * incident_k) * (1.0 / (1.0 - r_unpol));

                        let r_s_k = fma(n2k, -cos_t_k, n1k * cos_i) / fma(n2k, cos_t_k, n1k * cos_i);
                        let r_p_k = fma(n1k, -cos_t_k, n2k * cos_i) / fma(n1k, cos_t_k, n2k * cos_i);
                        let r_unpol_k = clamp(0.5 * fma(r_p_k, r_p_k, r_s_k * r_s_k), R_UNPOL_MIN, R_UNPOL_MAX);
                        // No `* split_pdf` -- scale-invariant under a uniform per-channel
                        // factor, was a pure no-op on the MIS weight; see refraction.rs.
                        (*path_pdf)[k] = (*path_pdf)[k] * (1.0 - r_unpol_k);
                    } else {
                        // chromatic termination -- UNCHANGED radiance handling
                        // (mismatched channel loses its own Stokes contribution
                        // either way), but `(*path_pdf)[k]` keeps accumulating this
                        // event's own transmit factor and, at an EXIT event, the
                        // channel additionally resolves its own transmitted radiance
                        // along its own refracted direction via
                        // `try_split_exit_channel` -- see this file's own header
                        // comment and `apply_refract_channel`'s CPU-side `else` branch.
                        let prefix_path_pdf_k = (*path_pdf)[k];
                        (*stokes)[k] = (*stokes)[k] * 0.0;
                        (*path_pdf)[k] = 0.0;

                        let azimuth_valid_here = entering_anisotropic && entry_valid != 0u;
                        let ct = compute_channel_transmission(
                            n1k, n2k, cos_i, cos_t_k, r_unpol, entering_anisotropic,
                            azimuth_valid_here, entry_cos_2psi_x, entry_sin_2psi_x, original_stokes_k,
                        );
                        (*path_pdf)[k] = prefix_path_pdf_k * (1.0 - ct.r_unpol_k);
                        if (is_exit_event && original_stokes_k.x > 0.0) {
                            try_split_exit_channel(
                                split_radiance, hit_point, k, (*lambdas)[k], refr_wave_dir_k,
                                ct.transmitted.x, studio_key_dir, studio_fill_dir, studio_sin_lp, observer,
                            );
                        }
                    }
                }
                // an INTERIOR dispersive event (an entry into the gem) narrows
                // every channel's MIS family; the exit event never does -- see
                // `narrow_compat`'s own comment in `transport_physics.wgsl`.
                if (!(*inside_gem)) {
                    narrow_compat(compat, scalar_dirs, scalar_dirs_valid, scalar_hero_match);
                }
                (*current_origin) = hit_point + final_refr_dir * RAY_EPS;
                (*current_dir) = final_refr_dir;
                (*current_k) = new_k;
                // `(*inside_gem)` still holds its PRE-bounce value here
                // (flipped just below) -- true only for a genuine transmit-out (never
                // true simultaneously with `entering_anisotropic`, which requires
                // `!inside_gem`) -- so pass a live incoming carry through unchanged,
                // mirroring `dispatch_bounce`'s identical CPU-side carry-through for
                // this same generic (isotropic/biaxial) exit arm.
                if ((*inside_gem)) {
                    (*pending_light_mis) = phase_pdf_this_check;
                    (*pending_light_mis_dir) = phase_dir_this_check;
                }
                (*inside_gem) = !(*inside_gem);
                if (entering_anisotropic) {
                    (*is_extraordinary) = use_extraordinary;
                }
            }
        }

        if (bounce > 4u) {
            var max_intensity: f32 = 0.0;
            for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
                max_intensity = max(max_intensity, max((*stokes)[k].x, 0.0));
            }
            let q = clamp(max_intensity, RR_FLOOR, 1.0);
            let rr_rand = f32(hash_u32(seed0 ^ hash_u32(bounce ^ RUSSIAN_ROULETTE_STREAM))) / 4294967295.0;
            if (rr_rand > q) {
                return BOUNCE_STATUS_TERMINATE;
            }
            // (Bias A fix): `split_radiance` rides along on the same `1/q` survival
            // rescale as `stokes` -- see `apply_russian_roulette`'s CPU-side doc comment.
            for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
                (*stokes)[k] = (*stokes)[k] * (1.0 / q);
                (*split_radiance)[k] = (*split_radiance)[k] / q;
            }
        }

    // Natural fall-through of the megakernel's old loop body (no break/continue hit
    // above): survive to the next bounce.
    return BOUNCE_STATUS_CONTINUE;
}

// optics::raytracer::color::integrate_channels_to_xyz_families, plus this ray's staged
// exit-split commit, the final white-balance/clamp, and the four output-buffer writes
// (`out_xyz` unconditionally, the three per-channel debug buffers plus `out_compat`
// when `params.write_debug_buffers != 0u`) -- the megakernel's entire per-ray tail from
// the end of the bounce loop onward, factored out (see this file's header comment) so
// BOTH `transport_main` (the megakernel, at the natural end of its bounce loop -- a ray
// that survives every bounce without escaping or dying still reaches this exactly as
// before) and `wavefront_bounce`/`wavefront_finalize_survivors`
// (`wavefront_transport.wgsl`) finalize a ray identically, writing into the SAME
// `out_xyz`/`out_radiance`/`out_lambdas`/`out_path_pdf`/`out_compat` bindings either
// way -- `idx` means exactly the same thing in both callers (dispatch-local
// `pixel_in_chunk * camera.num_samples + sample_in_chunk`), so no separate
// wavefront-only output buffer is needed.
//
// Mutates `*radiance` in place (folding in `split_radiance` when `path_escaped`) rather
// than a local copy -- the debug-buffer write below reads the POST-commit `radiance`,
// exactly as `transport_main` always has.
fn transport_finalize_ray(
    idx: u32,
    lambdas: array<f32, 8>,
    radiance: ptr<function, array<f32, 8>>,
    split_radiance: array<f32, 8>,
    path_pdf: array<f32, 8>,
    compat: array<u32, 8>,
    path_escaped: bool,
) {
    // commit the staged exit-split contributions into the REAL `radiance` array ONLY if
    // the shared/hero path itself reached its own environment lookup -- see
    // `split_radiance`'s own comment in `transport_bounce_step` above and
    // `ExitSplitCtx::split_radiance`'s CPU-side doc comment for why this all-or-nothing
    // gate is required. A no-op whenever nothing ever split, or every split channel's
    // own probe declined.
    if (path_escaped) {
        for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
            (*radiance)[k] = (*radiance)[k] + split_radiance[k];
        }
    }

    // Every channel that is alive at the exit (matching or split) contributes its own
    // radiance, so each channel's balance-heuristic weight is normalised over exactly
    // the techniques (hero choices) under which THAT channel would have stayed alive on
    // this same geometric path -- its own family, `compat[k]` -- rather than the
    // hero's.
    var xyz = vec3<f32>(0.0, 0.0, 0.0);
    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
        var family_pdf: f32 = 0.0;
        for (var j: u32 = 0u; j < NUM_CHANNELS; j = j + 1u) {
            if ((compat[k] & (1u << j)) != 0u) {
                family_pdf = family_pdf + path_pdf[j];
            }
        }
        // Same "should not happen" fallback as the shared `spectral_mis_weight`.
        var weight_k: f32 = 1.0;
        if (family_pdf > 1e-12) {
            weight_k = f32(NUM_CHANNELS) * path_pdf[0] / family_pdf;
        }
        let cmf = cie_1931_cmf(lambdas[k]);
        let weighted = (*radiance)[k] * weight_k;
        xyz = xyz + cmf * (weighted * NORM_FACTOR);
    }
    if (params.env_mode == 1u) {
        // Bradford-LMS-space von Kries adaptation, not a raw XYZ scale -- see
        // `apply_von_kries_white_balance`'s doc comment above and
        // `optics::raytracer::apply_von_kries_white_balance` on the CPU side.
        xyz = apply_von_kries_white_balance(xyz, params.white_balance);
    }
    // Mirrors `transport::trace_spectral_ray_inner`'s unconditional `.max(Vec3::ZERO)`.
    // Applied outside the `env_mode` branch exactly as the CPU does; pre-white-balance
    // `xyz` is a sum of non-negative terms, so the clamp is a no-op there.
    xyz = max(xyz, vec3<f32>(0.0));

    out_xyz[idx * 3u + 0u] = xyz.x;
    out_xyz[idx * 3u + 1u] = xyz.y;
    out_xyz[idx * 3u + 2u] = xyz.z;
    // `out_xyz` above is the only buffer a production dispatch
    // (`GpuFrameRenderer::accumulate`) ever reads back -- the four per-channel debug
    // buffers below exist for Tier 2/spectral-debug self-tests only. Guarding their
    // writes on `params.write_debug_buffers` (nonzero for every self-test) lets a
    // production dispatch bind tiny fixed-size dummy buffers for them instead of
    // buffers sized like `out_xyz` -- 9x less write traffic and 9x more samples per
    // chunk-budget dispatch.
    if (params.write_debug_buffers != 0u) {
        for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
            out_radiance[idx * 8u + k] = (*radiance)[k];
            out_lambdas[idx * 8u + k] = lambdas[k];
            out_path_pdf[idx * 8u + k] = path_pdf[k];
            out_compat[idx * 8u + k] = compat[k];
        }
    }
}

// This ray's deterministic camera-ray/hero-wavelength setup -- the megakernel's old
// per-thread prologue up through the `lambdas` derivation, factored out (see this
// file's header comment) so `wavefront_generate` (`wavefront_transport.wgsl`) draws
// EXACTLY the same RNG sequence from the same `idx` that `transport_main` always has.
// `idx` is this dispatch's LOCAL `(pixel_in_chunk, sample_in_chunk)` tuple index --
// `params.pixel_offset` is added here to recover the GLOBAL pixel index camera-ray
// generation and the per-pixel Cranley-Patterson rotations need, exactly as
// `transport_main` always did; output/ray-state slots stay indexed by the caller's own
// `idx`.
//
// Deliberately does NOT compute the per-ray hoisted dispersion/absorption arrays
// (`n_o_hoisted` and friends) or the biaxial axis frame/studio-rig directions: those
// depend only on `lambdas`/`material`/`params` (never on anything ELSE per-ray), so
// `transport_bounce_step`'s callers recompute them fresh from this function's
// `lambdas` output on every bounce instead of paying to store them in the wavefront
// ray-state buffers -- see `wavefront_transport.wgsl`'s module doc comment for the
// buffer-traffic trade-off this makes.
struct GeneratedRay {
    origin: vec3<f32>,
    dir: vec3<f32>,
    lambdas: array<f32, 8>,
    seed0: u32,
}

fn transport_generate_ray(idx: u32) -> GeneratedRay {
    let pixel = idx / camera.num_samples + params.pixel_offset;
    let local_sample = idx % camera.num_samples;
    let sample_num = local_sample + params.sample_offset;

    let seed0 = hash_u32((pixel * 0x9e3779b9u) ^ (sample_num * 0x85ebca6bu));

    // Stratified pixel jitter and hero wavelength (optics::raytracer::
    // {low_discrepancy_base2, cranley_patterson_rotate}), not an unstratified
    // hash-uniform. `seed0` above still seeds every per-bounce draw in
    // `transport_bounce_step` (Fresnel branch, Russian roulette, birefringent split);
    // only jx/jy/hero_rand come from this construction.
    let rot_jx = low_discrepancy_base2(hash_u32(pixel ^ PIXEL_JITTER_X_ROTATION_STREAM));
    let rot_jy = low_discrepancy_base2(hash_u32(pixel ^ PIXEL_JITTER_Y_ROTATION_STREAM));
    let rot_hero = low_discrepancy_base2(hash_u32(pixel ^ HERO_WAVELENGTH_ROTATION_STREAM));
    let jx = cranley_patterson_rotate(low_discrepancy_base2(sample_num), rot_jx) - 0.5;
    let jy = cranley_patterson_rotate(radical_inverse_base(sample_num, 3u), rot_jy) - 0.5;
    let hero_rand = cranley_patterson_rotate(radical_inverse_base(sample_num, 5u), rot_hero);
    let raygen = generate_camera_ray(pixel, jx, jy);

    let channel_width = SPECTRUM_SPAN / f32(NUM_CHANNELS);
    let lambda_hero = fma(hero_rand, SPECTRUM_SPAN, SPECTRUM_MIN);
    var lambdas: array<f32, 8>;
    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
        let offset = fma(f32(k), channel_width, lambda_hero - SPECTRUM_MIN);
        lambdas[k] = SPECTRUM_MIN + (offset % SPECTRUM_SPAN);
    }

    var result: GeneratedRay;
    result.origin = raygen.origin;
    result.dir = raygen.dir;
    result.lambdas = lambdas;
    result.seed0 = seed0;
    return result;
}

// This ray's material-derived "hoisted" constants -- everything `transport_bounce_step`
// needs that depends only on `lambdas`/`material`/`MATERIAL_CLASS`/`params`, never on
// anything that varies bounce-to-bounce. Factored out (see this file's header comment)
// so `transport_main`'s prologue and `wavefront_bounce` (`wavefront_transport.wgsl`)
// compute it identically -- `transport_main` calls this ONCE per ray, exactly as its
// own inline prologue always did; `wavefront_bounce` calls it fresh on EVERY bounce
// dispatch instead of storing the result in the wavefront ray-state buffers -- see
// `wavefront_transport.wgsl`'s module doc comment for why that trade-off is bit-identical
// (a pure function of this ray's own already-stored `lambdas`) and what it costs.
struct RayConstants {
    c_axis: vec3<f32>,
    birefringence_delta: f32,
    is_anisotropic: bool,
    is_biaxial: bool,
    biax_ax0: vec3<f32>,
    biax_ax1: vec3<f32>,
    biax_ax2: vec3<f32>,
    n_o_hero_seed: f32,
    n_beta_hero: f32,
    n_alpha_hero: f32,
    n_gamma_hero: f32,
    n_e_hero_seed: f32,
    n_o_hoisted: array<f32, 8>,
    alpha_o_hoisted: array<f32, 8>,
    alpha_e_hoisted: array<f32, 8>,
    alpha_beta_hoisted: array<f32, 8>,
    studio_key_dir: vec3<f32>,
    studio_fill_dir: vec3<f32>,
    studio_sin_lp: f32,
}

fn transport_hoist_ray_constants(lambdas: array<f32, 8>) -> RayConstants {
    let c_axis = material.dispersion.c_axis_and_birefringence.xyz;
    let birefringence_delta = material.dispersion.c_axis_and_birefringence.w;
    // ANDed with the pipeline-overridable MATERIAL_CLASS -- a no-op for
    // MATERIAL_CLASS_GENERIC, forced `false` for MATERIAL_CLASS_ISOTROPIC regardless of
    // the material buffer's own flag.
    let is_anisotropic = (material.dispersion.is_anisotropic != 0u)
        && (MATERIAL_CLASS != MATERIAL_CLASS_ISOTROPIC);
    // Whether this material's anisotropy is genuinely biaxial (three distinct principal
    // indices) rather than the uniaxial ordinary/extraordinary approximation -- hoisted
    // per-ray, constant across every bounce, like `c_axis` above. Never true for
    // MATERIAL_CLASS_ISOTROPIC/UNIAXIAL's routed materials anyway (see
    // `renderer::gpu::frame::classify_material`), so ANDing with MATERIAL_CLASS only
    // removes dead-by-construction reachability.
    let is_biaxial = (material.dispersion.has_biaxial_delta != 0u)
        && (MATERIAL_CLASS == MATERIAL_CLASS_GENERIC || MATERIAL_CLASS == MATERIAL_CLASS_BIAXIAL);
    // The biaxial principal-axis frame (alpha, beta, gamma world directions) depends
    // only on `c_axis`, so it is computed once per ray rather than every bounce.
    let biax_axes = biaxial_axes_from_gamma(c_axis);
    let biax_ax0 = biax_axes.ax0;
    let biax_ax1 = biax_axes.ax1;
    let biax_ax2 = biax_axes.ax2;
    // The hero channel's base dispersion value and, from it, the hero indicatrix's
    // three principal indices (`optics::materials::GemMaterial::biaxial_indicatrix`'s
    // convention: `n_beta := dispersion.evaluate(lambda)`, `n_alpha := n_beta -
    // biaxial_delta_beta_alpha`, `n_gamma := n_alpha + birefringence_delta`). Computed
    // unconditionally: harmless when `!is_biaxial` since `biaxial_delta_beta_alpha` is
    // `0.0` for every non-biaxial material, so `n_alpha_hero == n_beta_hero ==
    // n_o_hero_seed`, and no `biaxial_*` function is called with these values unless
    // `is_biaxial` guards it.
    let n_o_hero_seed = dispersion_evaluate(material.dispersion.model_type, material.dispersion.param_a, material.dispersion.param_b, lambdas[0]);
    let n_beta_hero = n_o_hero_seed;
    let n_alpha_hero = n_beta_hero - material.dispersion.biaxial_delta_beta_alpha;
    let n_gamma_hero = n_alpha_hero + birefringence_delta;
    // P1/P5: the hero channel's ACCURATE extraordinary index -- a genuine independent
    // e-ray dispersion curve evaluation when the material carries one
    // (Quartz/Amethyst/Citrine/Rutile), else the constant-offset `n_o_hero_seed +
    // birefringence_delta` fallback -- mirroring `GemMaterial::extraordinary_index_at`
    // exactly.
    var n_e_hero_seed: f32;
    if (material.has_extraordinary_dispersion != 0u) {
        n_e_hero_seed = extraordinary_dispersion_evaluate(material.extraordinary_model_type, material.extraordinary_param_a, material.extraordinary_param_b, lambdas[0]);
    } else {
        n_e_hero_seed = n_o_hero_seed + birefringence_delta;
    }

    // `n_o_ch[k]` and the per-channel pleochroic absorption coefficients depend only on
    // `lambdas[k]` (fixed for the whole ray) and the material's dispersion/band data,
    // never on anything that varies per bounce.
    var n_o_hoisted: array<f32, 8>;
    var alpha_o_hoisted: array<f32, 8>;
    var alpha_e_hoisted: array<f32, 8>;
    var alpha_beta_hoisted: array<f32, 8>;
    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
        n_o_hoisted[k] = dispersion_evaluate(material.dispersion.model_type, material.dispersion.param_a, material.dispersion.param_b, lambdas[k]);
        alpha_o_hoisted[k] = spectral_absorption(material.o_ray_bands, material.o_ray_band_count, lambdas[k]);
        alpha_e_hoisted[k] = spectral_absorption(material.e_ray_bands, material.e_ray_band_count, lambdas[k]);
        alpha_beta_hoisted[k] = 0.0;
    }
    if (is_biaxial) {
        for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
            alpha_beta_hoisted[k] = spectral_absorption(material.beta_ray_bands, material.beta_ray_band_count, lambdas[k]);
        }
    }

    // Studio rig key/fill/ring directions: a pure function of `params` alone. Unused
    // when `params.env_mode != transport_env_mode::STUDIO_RIG`.
    let studio_key_dir = studio_rig_key_dir(params.studio_light_yaw, params.studio_light_pitch);
    let studio_fill_dir = studio_rig_fill_dir(params.studio_light_yaw, params.studio_light_pitch);
    let studio_sin_lp = sin(params.studio_light_pitch);

    var result: RayConstants;
    result.c_axis = c_axis;
    result.birefringence_delta = birefringence_delta;
    result.is_anisotropic = is_anisotropic;
    result.is_biaxial = is_biaxial;
    result.biax_ax0 = biax_ax0;
    result.biax_ax1 = biax_ax1;
    result.biax_ax2 = biax_ax2;
    result.n_o_hero_seed = n_o_hero_seed;
    result.n_beta_hero = n_beta_hero;
    result.n_alpha_hero = n_alpha_hero;
    result.n_gamma_hero = n_gamma_hero;
    result.n_e_hero_seed = n_e_hero_seed;
    result.n_o_hoisted = n_o_hoisted;
    result.alpha_o_hoisted = alpha_o_hoisted;
    result.alpha_e_hoisted = alpha_e_hoisted;
    result.alpha_beta_hoisted = alpha_beta_hoisted;
    result.studio_key_dir = studio_key_dir;
    result.studio_fill_dir = studio_fill_dir;
    result.studio_sin_lp = studio_sin_lp;
    return result;
}
