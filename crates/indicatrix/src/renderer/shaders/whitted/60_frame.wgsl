fn wt_white_balance(xyz: vec3<f32>) -> vec3<f32> {
    let lms = vec3<f32>(
        fma(0.8951, xyz.x, fma(0.2664, xyz.y, -0.1614 * xyz.z)),
        fma(-0.7502, xyz.x, fma(1.7135, xyz.y, 0.0367 * xyz.z)),
        fma(0.0389, xyz.x, fma(-0.0685, xyz.y, 1.0296 * xyz.z))) * wt.white_balance.xyz;
    return vec3<f32>(
        fma(0.986993, lms.x, fma(-0.147054, lms.y, 0.159963 * lms.z)),
        fma(0.432305, lms.x, fma(0.518360, lms.y, 0.049291 * lms.z)),
        fma(-0.008529, lms.x, fma(0.040043, lms.y, 0.968487 * lms.z)));
}
fn wt_tap(pixel: u32, tap: u32) -> vec3<f32> {
    let width = wt_dims.z;
    let jitter = wt.aa[tap % wt.counts.z].xy;
    let aspect = wt_camera.width / wt_camera.height;
    let u = ((f32(pixel % width) + jitter.x) / wt_camera.width - 0.5) * 2.0 * aspect * wt_camera.fov_tan;
    let v = (0.5 - (f32(pixel / width) + jitter.y) / wt_camera.height) * 2.0 * wt_camera.fov_tan;
    let dir = normalize(wt_camera.forward + wt_camera.right * u + wt_camera.up * v);
    var rgb = vec3<f32>(0.0);
    for (var c: u32 = 0u; c < wt.counts.w; c = c + 1u) {
        rgb = rgb + wt.masks[c].xyz * wt_chain(wt_camera.origin, dir, wt.indices[c], -dir);
    }
    // Preserve the CPU column-matrix summation order; Bradford explicitly fuses.
    var xyz = (wt.rgb_to_xyz[0].xyz * rgb.x + wt.rgb_to_xyz[1].xyz * rgb.y) + wt.rgb_to_xyz[2].xyz * rgb.z;
    if (wt.modes.w != 0u) { xyz = wt_white_balance(xyz); }
    return max(xyz, vec3<f32>(0.0));
}
@compute @workgroup_size(64)
fn whitted_main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= wt.range.y) { return; }
    let pixel = wt.range.x + id.x;
    var sum = vec3<f32>(0.0);
    for (var s: u32 = 0u; s < wt.range.w; s = s + 1u) {
        let xyz = wt_tap(pixel, wt.range.z + s);
        // Finite-sample filtering matches add_finite_sample, including the count.
        if (all(abs(xyz) <= vec3<f32>(3.402823466e38))) { sum = sum + xyz; }
    }
    out_xyz[id.x * 3u] = sum.x;
    out_xyz[id.x * 3u + 1u] = sum.y;
    out_xyz[id.x * 3u + 2u] = sum.z;
}
