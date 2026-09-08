// Finding G5 Part B: the wavefront transport pipeline -- an ALTERNATIVE, selectable path
// to the megakernel (`spectral_transport.wgsl`'s `transport_main`), not a replacement.
// Ray state lives in storage buffers instead of per-thread local variables, and one
// bounce of every still-alive ray is a SEPARATE dispatch, so a divergent material
// (uniaxial/biaxial boundary solves, up to 7 exit-event polyhedron probes) no longer
// keeps every thread in a warp/wavefront resident through the whole bounce loop --
// threads that die early free their warp slot for the next `wavefront_generate` chunk's
// rays instead of idling until their warp's slowest ray finishes. See
// `renderer::gpu::frame`'s module doc comment ("Wavefront pipeline") for the buffer
// layout, per-bounce traffic estimate, and when to prefer this over the megakernel;
// `renderer::gpu_backend`/`renderer::gpu::frame::GpuPipelineKind` select it, default
// `Megakernel` until measured.
//
// # Kernel sequence (driven by `renderer::gpu::frame`'s host dispatch loop)
//
// 1. `wavefront_generate`: one invocation per (pixel, sample) tuple in the chunk (same
//    domain as the megakernel's own dispatch). Draws the exact same RNG sequence as the
//    megakernel's prologue (`transport_generate_ray`, `shaders/transport_bounce.wgsl`)
//    and writes the ray's initial state into the SoA buffers below, plus seeds
//    `active_ray_indices` with the identity permutation (every ray starts alive).
// 2. `wavefront_bounce`, called once per bounce round over the CURRENT active list:
//    each invocation loads one ray's state, recomputes that ray's material-derived
//    hoisted constants (`transport_hoist_ray_constants`) fresh from its own stored
//    `lambdas` -- see that function's own doc comment for why this is cheap-enough and
//    bit-identical -- calls the SAME `transport_bounce_step`
//    (`shaders/transport_bounce.wgsl`) the megakernel's loop calls, and either writes
//    the ray's state back (still alive) or finalizes it via `transport_finalize_ray`
//    (miss or Russian-roulette death) and clears its alive flag.
// 3. `wavefront_compact_scan` + `wavefront_compact_scatter`: order-preserving stream
//    compaction of the active list by alive flag -- see their own doc comments for the
//    two-pass block-prefix-sum this uses and why it stays deterministic.
// 4. Repeat 2-3 until the active count reaches zero or `params.max_bounces` bounce
//    rounds have run.
// 5. `wavefront_finalize_survivors`, over whatever remains of the active list after
//    step 4's loop ends by exhausting the bounce budget (NOT by every ray dying): mirrors
//    the megakernel's own unconditional tail -- a ray that survives every bounce without
//    ever escaping or dying still gets exactly one `out_xyz` write, with whatever
//    `radiance` it accumulated (typically from NEE contributions along the way, or zero).
// 6. `reduce_xyz.wgsl`'s existing `reduce_xyz_main`, UNCHANGED: every finalize site
//    above (`wavefront_bounce`'s death branch, `wavefront_finalize_survivors`) writes
//    into the SAME `out_xyz`/`out_radiance`/`out_lambdas`/`out_path_pdf`/`out_compat`
//    bindings (inherited from `transport_bounce.wgsl`'s shared prelude, bindings 4-7/9)
//    the megakernel writes into, at the SAME `idx` a ray would have used in the
//    megakernel (`pixel_in_chunk * camera.num_samples + sample_in_chunk`) -- so no
//    wavefront-specific output buffer or reduction kernel is needed at all.
//
// # Determinism
//
// Mirrors `spectral_transport.wgsl`'s own header comment: a ray's result depends only
// on (pixel, sample, seed, bounce) -- never on which OTHER rays share its dispatch,
// which workgroup it lands in, or what order `wavefront_bounce`/compaction process rays
// in. `transport_bounce_step` reads/writes only ITS OWN invocation's local copy of one
// ray's state (loaded from, and written back to, that ray's own fixed slot,
// `ray_idx`, in the SoA buffers below) -- no cross-thread communication, no atomics
// anywhere in the physics itself. `out_xyz[ray_idx]` is written exactly once, by
// whichever kernel invocation finalizes that ray, so which bounce round or which
// workgroup did it never affects the value written. `wavefront_compact_*`'s prefix sum
// is likewise a PURE function of the (fixed-order) alive-flag array, not of scheduling,
// so re-running the whole pipeline on identical input reproduces identical active-list
// orderings and identical `out_xyz` contents every time -- see
// `renderer::gpu::transport_check::run_wavefront_determinism` (or
// `determinism_check.rs`) for the two-runs-identical check, and
// `run_pipeline_equivalence` for the megakernel-vs-wavefront bit-identity check.
//
// # Ray-state SoA buffer layout
//
// Bindings 0-14 (camera/params/material/planes/output buffers/facet finishes/HDR
// environment) are `transport_bounce.wgsl`'s shared scene bindings, unchanged --
// EXACTLY the megakernel's own binding shapes, so `renderer::gpu::frame`'s host code
// builds this pipeline's scene bind group with the same buffers/helper
// (`build_chunk_bind_group`-style) it already uses for the megakernel. Bindings 15+
// below are wavefront-only. Every per-ray array is flat: a scalar field is indexed by
// `ray_idx` directly, an 8-channel field by `ray_idx * 8u + k`. Sized for
// `chunk_pixels * spp` rays (`chunk_rays`, `wf_params.chunk_rays`).
//
// Deliberately NOT stored per-ray (see `transport_hoist_ray_constants`'s own doc
// comment): the material-derived hoisted constants (`c_axis`, `is_anisotropic`,
// `is_biaxial`, the biaxial axis frame, hero indices, the four per-channel
// dispersion/absorption arrays, the studio rig directions) -- `wavefront_bounce`
// recomputes them fresh every bounce from `ray_lambdas`/`material` instead, trading a
// modest amount of recomputed ALU (a handful of dispersion evaluations and band
// summations per bounce) for a meaningfully smaller ray-state buffer and simpler
// generate/bounce kernels. Revisit if profiling shows this recomputation dominates.
struct WavefrontParams {
    chunk_rays: u32,
    active_count: u32,
    bounce: u32,
    workgroup_count: u32,
}

@group(0) @binding(15) var<uniform> wf_params: WavefrontParams;

@group(0) @binding(16) var<storage, read_write> ray_origin: array<vec4<f32>>;
@group(0) @binding(17) var<storage, read_write> ray_dir: array<vec4<f32>>;
@group(0) @binding(18) var<storage, read_write> ray_k: array<vec4<f32>>;
@group(0) @binding(19) var<storage, read_write> ray_prev_plane_normal: array<vec4<f32>>;
// Bit 0 `inside_gem`, bit 1 `is_extraordinary`, bit 2 `have_prev_plane_normal`, bit 3
// `path_escaped`, bit 4 `alive` -- see the `RAY_FLAG_*` constants below.
@group(0) @binding(20) var<storage, read_write> ray_flags: array<u32>;
@group(0) @binding(21) var<storage, read_write> ray_stokes: array<vec4<f32>>;
@group(0) @binding(22) var<storage, read_write> ray_radiance: array<f32>;
@group(0) @binding(23) var<storage, read_write> ray_path_pdf: array<f32>;
@group(0) @binding(24) var<storage, read_write> ray_split_radiance: array<f32>;
@group(0) @binding(25) var<storage, read_write> ray_compat: array<u32>;
@group(0) @binding(26) var<storage, read_write> ray_pending_light_mis: array<f32>;
@group(0) @binding(27) var<storage, read_write> ray_lambdas: array<f32>;
@group(0) @binding(28) var<storage, read_write> ray_seed: array<u32>;

// The current bounce round's live ray indices, `wf_params.active_count` of them
// (`wavefront_generate` seeds this with the identity permutation `0..chunk_rays`; each
// `wavefront_compact_scatter` round replaces it with that round's survivors -- see
// `renderer::gpu::frame`'s host loop for the buffer-swap between rounds).
@group(0) @binding(29) var<storage, read_write> active_ray_indices: array<u32>;
// `wavefront_compact_scatter`'s destination for this round's survivors -- becomes
// `active_ray_indices` for the NEXT bounce round after the host swaps the two buffer
// bindings (or copies, on a backend where an in-place binding swap is inconvenient).
@group(0) @binding(30) var<storage, read_write> active_ray_indices_next: array<u32>;
// Per-(active-list-position) LOCAL exclusive prefix count of alive rays within that
// ray's own workgroup -- `wavefront_compact_scan`'s output, `wavefront_compact_scatter`'s
// input.
@group(0) @binding(31) var<storage, read_write> compact_local_offset: array<u32>;
// One entry per workgroup: that workgroup's total alive-ray count --
// `wavefront_compact_scan`'s output. The host reads this back, computes its EXCLUSIVE
// prefix sum on the CPU (a simple, deterministic sequential loop over
// `wf_params.workgroup_count` entries -- see `renderer::gpu::frame`'s host loop), and
// uploads the result as `compact_block_offset` below. The same readback also gives the
// host this round's total survivor count (the last workgroup's offset plus its own
// count) for the "stop early when the live count hits zero" check.
@group(0) @binding(32) var<storage, read_write> compact_block_alive_count: array<u32>;
// The host-computed exclusive prefix sum of `compact_block_alive_count` -- see that
// binding's own doc comment.
@group(0) @binding(33) var<storage, read> compact_block_offset: array<u32>;

const RAY_FLAG_INSIDE_GEM: u32 = 0x1u;
const RAY_FLAG_IS_EXTRAORDINARY: u32 = 0x2u;
const RAY_FLAG_HAVE_PREV_PLANE_NORMAL: u32 = 0x4u;
const RAY_FLAG_PATH_ESCAPED: u32 = 0x8u;
const RAY_FLAG_ALIVE: u32 = 0x10u;

// ---------------------------------------------------------------------------------
// 1. wavefront_generate
// ---------------------------------------------------------------------------------

@compute @workgroup_size(64)
fn wavefront_generate(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= wf_params.chunk_rays) {
        return;
    }

    // Identical RNG draws to the megakernel's own prologue -- see
    // `transport_generate_ray`'s doc comment (`shaders/transport_bounce.wgsl`).
    let gen = transport_generate_ray(idx);

    ray_origin[idx] = vec4<f32>(gen.origin, 0.0);
    ray_dir[idx] = vec4<f32>(gen.dir, 0.0);
    ray_k[idx] = vec4<f32>(gen.dir, 0.0);
    ray_prev_plane_normal[idx] = vec4<f32>(0.0, 0.0, 0.0, 0.0);
    // Every ray starts outside the gem, ordinary-mode, no previous plane normal, not
    // escaped -- ALIVE is the only bit set, mirroring `transport_main`'s own initial
    // `inside_gem = false; is_extraordinary = false; have_prev_plane_normal = false;
    // path_escaped = false;`.
    ray_flags[idx] = RAY_FLAG_ALIVE;
    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
        ray_stokes[idx * 8u + k] = vec4<f32>(1.0, 0.0, 0.0, 0.0);
        ray_radiance[idx * 8u + k] = 0.0;
        ray_path_pdf[idx * 8u + k] = 1.0;
        ray_split_radiance[idx * 8u + k] = 0.0;
        ray_compat[idx * 8u + k] = 0xFFu;
        ray_lambdas[idx * 8u + k] = gen.lambdas[k];
    }
    ray_pending_light_mis[idx] = 0.0;
    ray_seed[idx] = gen.seed0;

    // Identity permutation: every ray starts alive, in ascending `ray_idx` order --
    // see this file's header comment on why compaction order matters (determinism
    // auditability) even though a ray's own result never depends on it.
    active_ray_indices[idx] = idx;
}

// ---------------------------------------------------------------------------------
// 2. wavefront_bounce
// ---------------------------------------------------------------------------------

@compute @workgroup_size(64)
fn wavefront_bounce(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(local_invocation_index) local_index: u32,
) {
    // Finding G5 Part A's workgroup-shared plane cache applies here exactly as in the
    // megakernel -- `transport_bounce_step` (via `intersect_ray`,
    // `shaders/transport_bounce.wgsl`) reads `planes_shared`/`planes` the same way
    // regardless of which entry point filled it, so this dispatch fills it itself,
    // before the out-of-range early return below, for the same
    // every-invocation-reaches-the-barrier reason `transport_main` does.
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

    let active_pos = gid.x;
    if (active_pos >= wf_params.active_count) {
        return;
    }
    let ray_idx = active_ray_indices[active_pos];

    var lambdas: array<f32, 8>;
    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
        lambdas[k] = ray_lambdas[ray_idx * 8u + k];
    }
    let seed0 = ray_seed[ray_idx];
    // See `transport_hoist_ray_constants`'s own doc comment: recomputed fresh every
    // bounce round from this ray's own stored `lambdas`, rather than stored.
    let rc = transport_hoist_ray_constants(lambdas);

    var current_origin = ray_origin[ray_idx].xyz;
    var current_dir = ray_dir[ray_idx].xyz;
    var current_k = ray_k[ray_idx].xyz;
    var prev_plane_normal = ray_prev_plane_normal[ray_idx].xyz;
    let flags = ray_flags[ray_idx];
    var inside_gem = (flags & RAY_FLAG_INSIDE_GEM) != 0u;
    var is_extraordinary = (flags & RAY_FLAG_IS_EXTRAORDINARY) != 0u;
    var have_prev_plane_normal = (flags & RAY_FLAG_HAVE_PREV_PLANE_NORMAL) != 0u;
    var path_escaped = (flags & RAY_FLAG_PATH_ESCAPED) != 0u;

    var stokes: array<vec4<f32>, 8>;
    var radiance: array<f32, 8>;
    var path_pdf: array<f32, 8>;
    var split_radiance: array<f32, 8>;
    var compat: array<u32, 8>;
    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
        stokes[k] = ray_stokes[ray_idx * 8u + k];
        radiance[k] = ray_radiance[ray_idx * 8u + k];
        path_pdf[k] = ray_path_pdf[ray_idx * 8u + k];
        split_radiance[k] = ray_split_radiance[ray_idx * 8u + k];
        compat[k] = ray_compat[ray_idx * 8u + k];
    }
    var pending_light_mis = ray_pending_light_mis[ray_idx];

    // One bounce of the SAME shared body `transport_main`'s loop calls -- see
    // `transport_bounce_step`'s own doc comment (`shaders/transport_bounce.wgsl`).
    let status = transport_bounce_step(
        wf_params.bounce, seed0, &lambdas, rc.c_axis, rc.birefringence_delta, rc.is_anisotropic, rc.is_biaxial,
        rc.biax_ax0, rc.biax_ax1, rc.biax_ax2, rc.n_o_hero_seed, rc.n_beta_hero, rc.n_alpha_hero, rc.n_gamma_hero,
        rc.n_e_hero_seed, rc.n_o_hoisted, rc.alpha_o_hoisted, rc.alpha_e_hoisted, rc.alpha_beta_hoisted,
        rc.studio_key_dir, rc.studio_fill_dir, rc.studio_sin_lp,
        &stokes, &radiance, &path_pdf, &current_origin, &current_dir, &current_k,
        &inside_gem, &is_extraordinary, &prev_plane_normal, &have_prev_plane_normal,
        &split_radiance, &compat, &path_escaped, &pending_light_mis,
    );

    if (status == BOUNCE_STATUS_TERMINATE) {
        // Mirrors `transport_main`'s own unconditional tail call at the natural end of
        // its bounce loop -- writes `out_xyz`/the debug buffers at `ray_idx`, the same
        // slot the megakernel would have used for this (pixel, sample) tuple.
        transport_finalize_ray(ray_idx, lambdas, &radiance, split_radiance, path_pdf, compat, path_escaped);
        ray_flags[ray_idx] = flags & ~RAY_FLAG_ALIVE;
        return;
    }

    // Still alive: write the mutated state back for the next bounce round.
    ray_origin[ray_idx] = vec4<f32>(current_origin, 0.0);
    ray_dir[ray_idx] = vec4<f32>(current_dir, 0.0);
    ray_k[ray_idx] = vec4<f32>(current_k, 0.0);
    ray_prev_plane_normal[ray_idx] = vec4<f32>(prev_plane_normal, 0.0);
    var new_flags = RAY_FLAG_ALIVE;
    if (inside_gem) {
        new_flags = new_flags | RAY_FLAG_INSIDE_GEM;
    }
    if (is_extraordinary) {
        new_flags = new_flags | RAY_FLAG_IS_EXTRAORDINARY;
    }
    if (have_prev_plane_normal) {
        new_flags = new_flags | RAY_FLAG_HAVE_PREV_PLANE_NORMAL;
    }
    if (path_escaped) {
        new_flags = new_flags | RAY_FLAG_PATH_ESCAPED;
    }
    ray_flags[ray_idx] = new_flags;
    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
        ray_stokes[ray_idx * 8u + k] = stokes[k];
        ray_radiance[ray_idx * 8u + k] = radiance[k];
        ray_path_pdf[ray_idx * 8u + k] = path_pdf[k];
        ray_split_radiance[ray_idx * 8u + k] = split_radiance[k];
        ray_compat[ray_idx * 8u + k] = compat[k];
    }
    ray_pending_light_mis[ray_idx] = pending_light_mis;
}

// ---------------------------------------------------------------------------------
// 3. wavefront_compact: order-preserving stream compaction of `active_ray_indices` by
//    each ray's current alive flag, via a deterministic two-pass block prefix sum.
//    `wavefront_compact_scan` computes, per workgroup, each ray's LOCAL exclusive
//    prefix count of alive predecessors within that same workgroup plus the
//    workgroup's own total; the host then computes the EXCLUSIVE prefix sum of every
//    workgroup's total (a trivial sequential CPU loop over
//    `wf_params.workgroup_count` values) and uploads it as `compact_block_offset`;
//    `wavefront_compact_scatter` then writes each alive ray to
//    `compact_block_offset[workgroup] + compact_local_offset[active_pos]`. Two
//    dispatches plus one small CPU pass rather than a single-dispatch global atomic
//    counter, deliberately: an atomic counter gives a valid COUNT but a
//    scheduling-dependent (non-reproducible) ORDER, and reproducible ordering is worth
//    keeping even though (see this file's header comment) no ray's OWN result depends
//    on it -- it keeps the wavefront pipeline's intermediate state, not just its final
//    `out_xyz`, auditable and diffable across runs.
// ---------------------------------------------------------------------------------

var<workgroup> compact_wg_alive: array<u32, 64>;
var<workgroup> compact_wg_offset: array<u32, 64>;
var<workgroup> compact_wg_total: u32;

@compute @workgroup_size(64)
fn wavefront_compact_scan(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(local_invocation_index) local_index: u32,
    @builtin(workgroup_id) wg_id: vec3<u32>,
) {
    let active_pos = gid.x;
    var alive: u32 = 0u;
    if (active_pos < wf_params.active_count) {
        let ray_idx = active_ray_indices[active_pos];
        alive = (ray_flags[ray_idx] >> 4u) & 1u;
    }
    compact_wg_alive[local_index] = alive;
    workgroupBarrier();

    // A single lane does the workgroup's own sequential exclusive prefix sum --
    // correctness-first: only 64 entries, and this scan runs at most once per bounce
    // round, not per bounce PER RAY. A parallel (Hillis-Steele) scan is a documented
    // future optimization if profiling ever shows this serialized step matters.
    if (local_index == 0u) {
        var running: u32 = 0u;
        for (var i: u32 = 0u; i < 64u; i = i + 1u) {
            compact_wg_offset[i] = running;
            running = running + compact_wg_alive[i];
        }
        compact_wg_total = running;
    }
    workgroupBarrier();

    if (active_pos < wf_params.active_count) {
        compact_local_offset[active_pos] = compact_wg_offset[local_index];
    }
    if (local_index == 0u) {
        compact_block_alive_count[wg_id.x] = compact_wg_total;
    }
}

@compute @workgroup_size(64)
fn wavefront_compact_scatter(@builtin(global_invocation_id) gid: vec3<u32>) {
    let active_pos = gid.x;
    if (active_pos >= wf_params.active_count) {
        return;
    }
    let ray_idx = active_ray_indices[active_pos];
    let alive = (ray_flags[ray_idx] >> 4u) & 1u;
    if (alive == 0u) {
        return;
    }
    let workgroup_idx = active_pos / 64u;
    let dest = compact_block_offset[workgroup_idx] + compact_local_offset[active_pos];
    active_ray_indices_next[dest] = ray_idx;
}

// ---------------------------------------------------------------------------------
// 5. wavefront_finalize_survivors -- see this file's header comment, step 5. Mirrors
//    `transport_main`'s unconditional tail for a ray that survives every bounce
//    without ever hitting `transport_bounce_step`'s `BOUNCE_STATUS_TERMINATE` --
//    `wf_params.active_count`/`active_ray_indices` here are whatever remained after
//    the host's bounce-round loop stopped because it ran out of bounce budget, NOT
//    because every ray died (that path is already fully finalized inside
//    `wavefront_bounce` itself).
// ---------------------------------------------------------------------------------

@compute @workgroup_size(64)
fn wavefront_finalize_survivors(@builtin(global_invocation_id) gid: vec3<u32>) {
    let active_pos = gid.x;
    if (active_pos >= wf_params.active_count) {
        return;
    }
    let ray_idx = active_ray_indices[active_pos];

    var radiance: array<f32, 8>;
    var path_pdf: array<f32, 8>;
    var split_radiance: array<f32, 8>;
    var compat: array<u32, 8>;
    var lambdas: array<f32, 8>;
    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
        radiance[k] = ray_radiance[ray_idx * 8u + k];
        path_pdf[k] = ray_path_pdf[ray_idx * 8u + k];
        split_radiance[k] = ray_split_radiance[ray_idx * 8u + k];
        compat[k] = ray_compat[ray_idx * 8u + k];
        lambdas[k] = ray_lambdas[ray_idx * 8u + k];
    }
    let path_escaped = (ray_flags[ray_idx] & RAY_FLAG_PATH_ESCAPED) != 0u;

    transport_finalize_ray(ray_idx, lambdas, &radiance, split_radiance, path_pdf, compat, path_escaped);
    ray_flags[ray_idx] = ray_flags[ray_idx] & ~RAY_FLAG_ALIVE;
}
