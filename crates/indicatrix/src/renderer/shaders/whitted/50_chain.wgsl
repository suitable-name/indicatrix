struct WtBranch {
    origin: vec3<f32>, dir: vec3<f32>, weight: vec3<f32>,
    inside: bool, depth: u32, primary: bool,
}
// Scalar slots, not a function-local array: avoids AMD/FXC array lowering failures.
struct WtStack { a: WtBranch, b: WtBranch, c: WtBranch, d: WtBranch, count: u32 }
fn wt_push(stack: ptr<function, WtStack>, branch: WtBranch) {
    if (!wt_keeps(branch.weight, branch.depth)) { return; }
    switch ((*stack).count) {
        case 0u: { (*stack).a = branch; }
        case 1u: { (*stack).b = branch; }
        case 2u: { (*stack).c = branch; }
        case 3u: { (*stack).d = branch; }
        default: { return; }
    }
    (*stack).count = (*stack).count + 1u;
}
fn wt_pop(stack: ptr<function, WtStack>) -> WtBranch {
    (*stack).count = (*stack).count - 1u;
    switch ((*stack).count) {
        case 0u: { return (*stack).a; }
        case 1u: { return (*stack).b; }
        case 2u: { return (*stack).c; }
        default: { return (*stack).d; }
    }
}
fn wt_emit(stack: ptr<function, WtStack>, sum: ptr<function, vec3<f32>>,
    point: vec3<f32>, dir: vec3<f32>, weight: vec3<f32>, observer: vec3<f32>, depth: u32) {
    if (wt.counts.y == 0u || (*stack).count >= 3u) {
        (*sum) = (*sum) + weight * wt_leaf(dir, observer);
    } else {
        wt_push(stack, WtBranch(wt_scale_add(dir, 1e-4, point), dir, weight, false, depth, false));
    }
}
fn wt_chain(origin: vec3<f32>, direction: vec3<f32>, n: f32, observer: vec3<f32>) -> vec3<f32> {
    let empty = WtBranch(vec3<f32>(0.0), vec3<f32>(0.0), vec3<f32>(0.0), false, 0u, false);
    var stack = WtStack(empty, empty, empty, empty, 0u);
    wt_push(&stack, WtBranch(origin, direction, vec3<f32>(1.0), false, 0u, true));
    var sum = vec3<f32>(0.0);
    loop {
        if (stack.count == 0u) { break; }
        var branch = wt_pop(&stack);
        let hit = wt_intersect(branch.origin, branch.dir);
        if (!hit.valid) {
            // One lookup for every escape: a primary miss sees the same lit tray a window does.
            sum = sum + branch.weight * wt_leaf(branch.dir, observer);
            continue;
        }
        if (hit.side != 0u) { branch.inside = hit.side == 2u; }
        if (branch.inside) {
            let attenuation = -wt_alpha(branch.dir) * hit.t;
            branch.weight = branch.weight * vec3<f32>(exp_poly(attenuation.x), exp_poly(attenuation.y), exp_poly(attenuation.z));
        }
        let frost = wt_frost[hit.facet];
        if (frost.w != 0.0) { sum = sum + branch.weight * frost.xyz; continue; }
        let point = wt_scale_add(branch.dir, hit.t, branch.origin);
        let normal = select(hit.normal, -hit.normal, branch.inside);
        let ci = clamp(-wt_dot(branch.dir, normal), 0.0, 1.0);
        let eta = select(1.0 / n, n, branch.inside);
        let r = wt_fresnel(ci, eta);
        let reflected = wt_reflect(branch.dir, normal);
        let transmitted = wt_refract(branch.dir, normal, eta);
        let depth = branch.depth + 1u;
        if (branch.inside) {
            if (transmitted.valid) {
                wt_emit(&stack, &sum, point, transmitted.dir, branch.weight * (1.0 - r), observer, depth);
            }
            wt_push(&stack, WtBranch(wt_scale_add(reflected, 1e-4, point), reflected,
                branch.weight * r, true, depth, false));
        } else {
            let glare = select(1.0, wt.values.x, branch.primary);
            wt_emit(&stack, &sum, point, reflected, branch.weight * (r * glare), observer, depth);
            if (transmitted.valid) {
                wt_push(&stack, WtBranch(wt_scale_add(transmitted.dir, 1e-4, point), transmitted.dir,
                    branch.weight * (1.0 - r), true, depth, false));
            }
        }
    }
    return sum;
}
