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

const SPECTRUM_MIN: f32 = 380.0;
const SPECTRUM_SPAN: f32 = 400.0;
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

