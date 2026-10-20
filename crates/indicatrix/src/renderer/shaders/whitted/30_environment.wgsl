fn wt_studio_g(dir: vec3<f32>, observer: vec3<f32>) -> f32 {
    return wt_studio_dispatch(u32(wt.studio.w), normalize(dir), 1.0, wt.studio.x,
        wt.values.w, wt.key_dir.xyz, wt.fill_dir.xyz, wt.studio.z, wt.studio.y,
        observer, wt.shadow.xy, wt.tent, wt.extra.x, wt.values.z);
}
fn wt_hdr_texel(x: i32, y: i32) -> vec3<f32> {
    let width = i32(wt_dims.x);
    let xx = u32(((x % width) + width) % width);
    let yy = u32(clamp(y, 0, i32(wt_dims.y) - 1));
    return wt_env[yy * wt_dims.x + xx].xyz;
}
fn wt_hdr(dir: vec3<f32>) -> vec3<f32> {
    let d = normalize(dir);
    let v = acos(clamp(d.y, -1.0, 1.0)) / wt_PI;
    let u = fract(atan2(d.x, d.z) / (2.0 * wt_PI));
    let fx = fma(u, f32(wt_dims.x), -0.5);
    let fy = fma(clamp(v, 0.0, 1.0), f32(wt_dims.y), -0.5);
    let ix = i32(floor(fx)); let iy = i32(floor(fy));
    let tx = fx - floor(fx); let ty = fy - floor(fy);
    let a = wt_hdr_texel(ix, iy); let b = wt_hdr_texel(ix + 1, iy);
    let c = wt_hdr_texel(ix, iy + 1); let d2 = wt_hdr_texel(ix + 1, iy + 1);
    let top = fma(b, vec3<f32>(tx), a * (1.0 - tx));
    let bottom = fma(d2, vec3<f32>(tx), c * (1.0 - tx));
    return fma(bottom, vec3<f32>(ty), top * (1.0 - ty));
}
fn wt_leaf(dir: vec3<f32>, observer: vec3<f32>) -> vec3<f32> {
    switch (wt.modes.y) {
        case 1u: {
            let pos = (clamp(dir.y, -1.0, 1.0) + 1.0) * 511.5;
            let lo = u32(pos); let f = pos - f32(lo);
            return wt_env[lo].xyz + (wt_env[min(lo + 1u, 1023u)].xyz - wt_env[lo].xyz) * f;
        }
        case 2u: { return wt_hdr(dir); }
        case 3u: { return wt.env_rgb.xyz; }
        default: { return wt.env_rgb.xyz * wt_studio_g(dir, observer); }
    }
}
