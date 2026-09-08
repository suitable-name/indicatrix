// Full isotropic + uniaxial-birefringent + biaxial-birefringent spectral estimator, a
// direct translation of `optics::raytracer::trace_spectral_ray`. Driven by
// `renderer::gpu::estimator_check` (statistical image comparison, energy-conservation
// furnace anchor, spectral-space debug comparison) and `renderer::gpu::transport_check`
// (per-function ULP budgets, exercised via `shaders/transport_functions.wgsl`).
//
// Finding G5 Part B: this file now holds ONLY the megakernel entry point,
// `transport_main`: its per-thread prologue (camera ray generation, hero wavelength,
// hoisted per-channel dispersion/absorption arrays), a loop that calls
// `transport_bounce_step` (`shaders/transport_bounce.wgsl`) once per bounce, and the
// final per-channel-family XYZ integration and output writes. Every scene binding, and
// every helper function the old inline bounce loop called (`intersect_ray`,
// `try_split_exit_channel`, the NEE/environment-sampling machinery, the uniaxial/biaxial
// dispatch helpers), moved to `transport_bounce.wgsl` -- see that file's own header
// comment for why (in one sentence: `wavefront_transport.wgsl`'s `wavefront_bounce`
// kernel needs the exact same bindings and calls the exact same
// `transport_bounce_step` function, so both live where both callers can share them
// without a duplicate copy).
//
// This file's OWN doc comment used to describe the full transport physics in detail;
// that description now lives in `transport_bounce.wgsl`, next to the code it actually
// describes. What remains here is deliberately thin: `transport_main` is now a per-ray
// setup routine plus a bounce-count loop, bit-identical to before this split (see
// `transport_bounce_step`'s own doc comment for exactly what "bit-identical by
// construction" means here).
// optics::raytracer::trace_spectral_ray -- the isotropic-only estimator.

@compute @workgroup_size(64)
fn transport_main(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(local_invocation_index) local_index: u32,
) {
    // Finding G5 Part A: cooperative workgroup-shared plane fill -- deliberately BEFORE
    // the out-of-range early `return` below. `workgroupBarrier()` requires every
    // invocation in the workgroup to reach it, including the last (partial) workgroup's
    // threads whose `gid.x` is `>= total` and which return right after: an invocation
    // that returned before this point would never reach the barrier the rest of its
    // workgroup executes, which WGSL forbids. `plane_count`/the `<=` comparison are
    // uniform across the whole workgroup (see `planes_shared`'s doc comment above), so
    // this is uniform control flow on both sides of the barrier.
    let plane_count = arrayLength(&planes);
    if (plane_count <= PLANES_SHARED_CAPACITY) {
        var fill_i = local_index;
        loop {
            if (fill_i >= plane_count) {
                break;
            }
            planes_shared[fill_i] = planes[fill_i];
            fill_i = fill_i + 64u;
        }
    }
    workgroupBarrier();

    let idx = gid.x;
    let total = params.num_pixels * camera.num_samples;
    if (idx >= total) {
        return;
    }

    // Finding G5 Part B: camera-ray generation, hero wavelength, and the per-channel
    // `lambdas` derivation are now `transport_generate_ray` (`shaders/transport_bounce.wgsl`),
    // shared with `wavefront_generate`'s (`wavefront_transport.wgsl`) identical need for
    // the SAME deterministic RNG draws from the SAME `idx` -- see that function's own
    // doc comment. `idx` means exactly the same thing to both callers: this dispatch's
    // local `(pixel_in_chunk, sample_in_chunk)` tuple index, `params.pixel_offset`
    // added inside the shared function to recover the GLOBAL pixel index.
    let gen = transport_generate_ray(idx);
    // `var`, not `let`: the bounce loop below takes `&lambdas` to pass into
    // `transport_bounce_step` (it is never itself mutated, matching that parameter's
    // read-only-in-practice contract -- see `transport_bounce_step`'s own doc comment).
    var lambdas: array<f32, 8> = gen.lambdas;
    let seed0 = gen.seed0;

    var stokes: array<vec4<f32>, 8>;
    var radiance: array<f32, 8>;
    var path_pdf: array<f32, 8>;
    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
        stokes[k] = vec4<f32>(1.0, 0.0, 0.0, 0.0);
        radiance[k] = 0.0;
        path_pdf[k] = 1.0;
    }

    // Finding G5 Part B: the material-derived per-ray "hoisted" constants (dispersion,
    // absorption, biaxial axis frame, hero indices, studio rig directions) are now
    // `transport_hoist_ray_constants` (`shaders/transport_bounce.wgsl`), called ONCE
    // per ray here exactly as this prologue's own inline computation always did -- see
    // that function's own doc comment for why `wavefront_bounce` instead calls it fresh
    // every bounce.
    let rc = transport_hoist_ray_constants(lambdas);

    var current_origin = gen.origin;
    var current_dir = gen.dir;
    // The wave normal `k`, tracked alongside `current_dir` (the Poynting/energy
    // direction `S`). Starts equal to the ray's direction: outside the gem (air)
    // `k == S` always (isotropic medium, no walk-off).
    var current_k = gen.dir;
    var inside_gem = false;
    // Which eigenmode the ray currently inside the crystal was stochastically assigned
    // to at its most recent air->crystal entry. Meaningless while `!inside_gem`;
    // carried across internal bounces exactly as the CPU carries it.
    var is_extraordinary = false;
    var prev_plane_normal = vec3<f32>(0.0, 0.0, 0.0);
    var have_prev_plane_normal = false;

    // Exit-event spectral splitting's per-trace state (see this file's header).
    // `split_radiance` is a staging accumulator folded into `radiance` only if
    // `path_escaped` ends up true; `compat` tracks which channels have matched at
    // every interior dispersive event so far (bit `j` of `compat[k]` set while `k`/`j`
    // remain compatible; all bits set before the first mismatch).
    var split_radiance: array<f32, 8>;
    var compat: array<u32, 8>;
    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
        split_radiance[k] = 0.0;
        compat[k] = 0xFFu;
    }
    var path_escaped = false;
    var pending_light_mis: f32 = 0.0;

    for (var bounce: u32 = 0u; bounce < params.max_bounces; bounce = bounce + 1u) {
        // Finding G5 Part B: one bounce of the shared `transport_bounce_step` body
        // (`shaders/transport_bounce.wgsl`) -- see that function's own doc comment for
        // the calling convention and why this is bit-identical to the old inline loop
        // body it replaces.
        let status = transport_bounce_step(
            bounce, seed0, &lambdas, rc.c_axis, rc.birefringence_delta, rc.is_anisotropic, rc.is_biaxial,
            rc.biax_ax0, rc.biax_ax1, rc.biax_ax2, rc.n_o_hero_seed, rc.n_beta_hero, rc.n_alpha_hero, rc.n_gamma_hero,
            rc.n_e_hero_seed, rc.n_o_hoisted, rc.alpha_o_hoisted, rc.alpha_e_hoisted, rc.alpha_beta_hoisted,
            rc.studio_key_dir, rc.studio_fill_dir, rc.studio_sin_lp,
            &stokes, &radiance, &path_pdf, &current_origin, &current_dir, &current_k,
            &inside_gem, &is_extraordinary, &prev_plane_normal, &have_prev_plane_normal,
            &split_radiance, &compat, &path_escaped, &pending_light_mis,
        );
        if (status == BOUNCE_STATUS_TERMINATE) {
            break;
        }
    }

    // Finding G5 Part B: the per-ray tail (commit staged exit-split contributions,
    // integrate per-channel-family XYZ, von Kries white balance, final clamp, and
    // every output-buffer write) is now `transport_finalize_ray`
    // (`shaders/transport_bounce.wgsl`), shared with `wavefront_bounce`'s /
    // `wavefront_finalize_survivors`'s own per-ray termination paths -- see that
    // function's own doc comment.
    transport_finalize_ray(idx, lambdas, &radiance, split_radiance, path_pdf, compat, path_escaped);
}
