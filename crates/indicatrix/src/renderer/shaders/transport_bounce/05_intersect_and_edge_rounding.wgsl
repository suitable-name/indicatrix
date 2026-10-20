// optics::raytracer::intersect_polyhedron

// `HitInfo::side`: which way the ray crossed the surface it hit. `NONE` for a stone without
// tools (the bounce's own inside/outside toggle is exact there, nothing is computed); with
// tools, `ENTRY` when the stretch of ray before the hit is air or a cavity and `EXIT` when it
// is the material. Twin of the `Option<bool>` that `intersect_stone_soa_side` returns.
const HIT_SIDE_NONE: u32 = 0u;
const HIT_SIDE_ENTRY: u32 = 1u;
const HIT_SIDE_EXIT: u32 = 2u;

struct HitInfo {
    hit: bool,
    t: f32,
    normal: vec3<f32>,
    // optics::raytracer::HitRecord::facet_idx -- lets the frosted-finish lookup below
    // index `facet_finishes`. Mirrors `near_facet`/`far_facet`'s `.unwrap_or(0)`
    // fallback: `0u` whenever a hit is never reported either.
    facet_idx: u32,
    // `HIT_SIDE_*`, see above.
    side: u32,
}

// optics::raytracer::intersect_stone::carve, for a stone with concave tools (`params.num_tools
// > 0`): the first change of the "inside the stone" state past `1e-4` along the ray, where
// the stone is the slab `[a, b]` of the planes minus the tool intervals. The slab (`a`/`b`
// and the planes that bound them) comes from `intersect_ray`'s own plane loop. The scan
// logic is the shared, pure `ct_*` code (`transport_physics/10_concave_tools.wgsl`); this
// function only fetches the tool records. A tool hit has facet id `plane_count + k` and the
// stone's outward normal, which is the negated tool normal.
fn intersect_ray_tools(
    origin: vec3<f32>,
    dir: vec3<f32>,
    plane_count: u32,
    t_near: f32,
    near_idx: u32,
    near_normal: vec3<f32>,
    t_far: f32,
    far_idx: u32,
    far_normal: vec3<f32>,
) -> HitInfo {
    var result: HitInfo;
    result.hit = false;
    result.t = 0.0;
    result.normal = vec3<f32>(0.0, 0.0, 0.0);
    result.facet_idx = 0u;
    result.side = HIT_SIDE_NONE;
    let n_tools = params.num_tools;
    var st = ct_carve_begin(t_near, t_far);
    // Each scan either settles the answer or moves `pos` past one more tool interval, so
    // `n_tools + 1` scans always suffice.
    for (var scan: u32 = 0u; scan <= n_tools; scan = scan + 1u) {
        if (st.state != CT_STATE_SCAN) {
            break;
        }
        var acc = ct_scan_init();
        for (var k: u32 = 0u; k < n_tools; k = k + 1u) {
            let tool = scene_tool(plane_count, k);
            if (ct_ray_may_hit(tool, origin, dir)) {
                acc = ct_scan_add(acc, k, ct_tool_ray_interval(tool, origin, dir), st.pos);
            }
        }
        st = ct_carve_step(st, acc, t_far);
    }
    if (st.state == CT_STATE_HIT_NEAR) {
        result.hit = true;
        result.t = st.hit_t;
        result.normal = near_normal;
        result.facet_idx = near_idx;
    } else if (st.state == CT_STATE_HIT_FAR) {
        result.hit = true;
        result.t = st.hit_t;
        result.normal = far_normal;
        result.facet_idx = far_idx;
    } else if (st.state == CT_STATE_HIT_TOOL) {
        let tool = scene_tool(plane_count, st.hit_k);
        let point = origin + dir * st.hit_t;
        result.hit = true;
        result.t = st.hit_t;
        result.normal = -ct_tool_outward_normal(tool, point);
        result.facet_idx = plane_count + st.hit_k;
    }
    // The carve's own entry/exit (`CtCarve::entering`), for every kind of hit.
    if (result.hit) {
        if (st.entering) {
            result.side = HIT_SIDE_ENTRY;
        } else {
            result.side = HIT_SIDE_EXIT;
        }
    }
    return result;
}

fn intersect_ray(origin: vec3<f32>, dir: vec3<f32>) -> HitInfo {
    var t_near: f32 = -1e30;
    var t_far: f32 = 1e30;
    var near_normal = vec3<f32>(0.0, 0.0, 0.0);
    var far_normal = vec3<f32>(0.0, 0.0, 0.0);
    var near_idx: u32 = 0u;
    var far_idx: u32 = 0u;
    var result: HitInfo;
    // The real planes only: with tools, `planes` also holds their records.
    let num_planes = scene_plane_count();
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
            result.side = HIT_SIDE_NONE;
            return result;
        }
    }
    if (t_near > t_far) {
        result.hit = false;
        result.t = 0.0;
        result.normal = vec3<f32>(0.0, 0.0, 0.0);
        result.facet_idx = 0u;
        result.side = HIT_SIDE_NONE;
    } else if (params.num_tools != 0u) {
        // Concave stone. Planar stones (the default) never take this branch.
        result = intersect_ray_tools(
            origin, dir, num_planes, t_near, near_idx, near_normal, t_far, far_idx, far_normal,
        );
    } else if (t_near > 1e-4) {
        result.hit = true;
        result.t = t_near;
        result.normal = near_normal;
        result.facet_idx = near_idx;
        result.side = HIT_SIDE_NONE;
    } else if (t_far > 1e-4) {
        result.hit = true;
        result.t = t_far;
        result.normal = far_normal;
        result.facet_idx = far_idx;
        result.side = HIT_SIDE_NONE;
    } else {
        result.hit = false;
        result.t = 0.0;
        result.normal = vec3<f32>(0.0, 0.0, 0.0);
        result.facet_idx = 0u;
        result.side = HIT_SIDE_NONE;
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
    // optics::raytracer::refraction::ExitSplitCtx::split_mis_weight -- the balance-
    // heuristic weight against a live incoming NEE carry from the scatter event that
    // led into THIS bounce (`1.0` outside that case, reproducing the unweighted
    // addition exactly). Computed once per bounce by the caller (mirrors
    // `dispatch_bounce`'s CPU-side `exit.split_mis_weight` assignment), not here --
    // this function stays a pure per-channel fan-out primitive.
    split_mis_weight: f32,
) {
    let probe = intersect_ray(hit_point + dir_k * 1e-4, dir_k);
    if (probe.hit) {
        // Bounded re-entry: decline to trace further (a pure energy-loss truncation).
        return;
    }
    let env_spectral = sample_environment_with_rig(dir_k, lambda_k, key_dir, fill_dir, sin_lp, observer);
    (*split_radiance)[k] = fma(max(transmitted_intensity, 0.0) * split_mis_weight, env_spectral, (*split_radiance)[k]);
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
    let num_planes = scene_plane_count();
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

// Whether any concave tool contains `p` (`ToolPrimitive::contains`, tools tried in order).
fn scene_any_tool_contains(plane_count: u32, p: vec3<f32>) -> bool {
    let n_tools = params.num_tools;
    for (var k: u32 = 0u; k < n_tools; k = k + 1u) {
        let tool = scene_tool(plane_count, k);
        if (ct_point_may_be_inside(tool, p) && ct_tool_contains(tool, p)) {
            return true;
        }
    }
    return false;
}

// optics::raytracer::transport::inner::shading_normal_for_hit: the edge-rounded shading
// normal of a hit on a (possibly concave) stone. A planar stone, or no rounding, is
// `shading_normal_near_edge` unchanged. With tools, rounding is skipped (v1) for a hit on a
// tool surface, and for a plane hit within `rounding_radius` of a tool:
// `shading_normal_near_edge` blends toward the nearest plane normal, which would be the wrong
// neighbour there. The tool-proximity test probes the four in-plane points one radius away.
fn shading_normal_for_hit(hit_point: vec3<f32>, hit_facet_idx: u32, hit_normal: vec3<f32>, rounding_radius: f32) -> vec3<f32> {
    if (params.num_tools == 0u || rounding_radius <= 0.0) {
        return shading_normal_near_edge(hit_point, hit_facet_idx, hit_normal, rounding_radius);
    }
    let plane_count = scene_plane_count();
    if (hit_facet_idx >= plane_count) {
        return hit_normal;
    }
    let basis = frosted_orthonormal_basis(hit_normal);
    if (scene_any_tool_contains(plane_count, hit_point + basis.t * rounding_radius)
        || scene_any_tool_contains(plane_count, hit_point + (-basis.t) * rounding_radius)
        || scene_any_tool_contains(plane_count, hit_point + basis.b * rounding_radius)
        || scene_any_tool_contains(plane_count, hit_point + (-basis.b) * rounding_radius)) {
        return hit_normal;
    }
    return shading_normal_near_edge(hit_point, hit_facet_idx, hit_normal, rounding_radius);
}

