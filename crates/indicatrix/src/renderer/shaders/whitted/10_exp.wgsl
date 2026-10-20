// Verbatim shared exponential polynomial (scalar exp_lane twin).
const EXP_LOG2E: f32 = 1.4426950408889634;
const EXP_C1: f32 = 0.693359375;
const EXP_C2: f32 = -2.1219444e-4;
const EXP_P0: f32 = 1.9875691e-4;
const EXP_P1: f32 = 1.3981999e-3;
const EXP_P2: f32 = 8.333452e-3;
const EXP_P3: f32 = 4.1665796e-2;
const EXP_P4: f32 = 1.6666665e-1;
const EXP_P5: f32 = 5.0000003e-1;
const EXP_HI: f32 = 88.02875;
const EXP_LO: f32 = -87.33654;

fn exp_poly(x_in: f32) -> f32 {
    let x0 = clamp(x_in, EXP_LO, EXP_HI);
    let n = floor(fma(x0, EXP_LOG2E, 0.5));
    let x1 = fma(n, -EXP_C1, x0);
    let x2 = fma(n, -EXP_C2, x1);
    let z = x2 * x2;
    var p = EXP_P0;
    p = fma(p, x2, EXP_P1);
    p = fma(p, x2, EXP_P2);
    p = fma(p, x2, EXP_P3);
    p = fma(p, x2, EXP_P4);
    p = fma(p, x2, EXP_P5);
    let poly = fma(p, z, x2) + 1.0;
    // 2^n by exponent-bit construction -- mirrors `exp_lane`'s
    // `f32::from_bits((((n as i32) + 127) << 23) as u32)` exactly; `n` is integral and
    // within the f32 exponent range after the clamp above.
    let pow2n = bitcast<f32>(u32(i32(n) + 127) << 23u);
    return poly * pow2n;
}

