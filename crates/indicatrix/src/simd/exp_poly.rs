//! Vectorized 8-lane `f32` exponential ([`exp_f32x8`]): the spectral
//! tracer's per-channel Beer-Lambert/transmittance primitive, with its
//! polynomial scalar reference and AVX2+FMA kernel.

use super::{SimdLevel, simd_level};

// Cephes expf constants (Moshier), the standard vectorizable exponential.
const EXP_LOG2E: f32 = std::f32::consts::LOG2_E;
// Cody-Waite split of ln(2): C1 is exactly representable (0x3F318000).
const EXP_C1: f32 = f32::from_bits(0x3F31_8000);
const EXP_C2: f32 = -2.121_944_4e-4;
const EXP_P0: f32 = 1.987_569_1e-4;
const EXP_P1: f32 = 1.398_199_9e-3;
const EXP_P2: f32 = 8.333_452e-3;
const EXP_P3: f32 = 4.166_579_6e-2;
const EXP_P4: f32 = 1.666_666_5e-1;
const EXP_P5: f32 = 5.000_000_3e-1;
const EXP_HI: f32 = 88.028_75;
const EXP_LO: f32 = -87.336_54;

/// One lane of the polynomial exponential; the scalar fallback and the
/// reference the vector path is tested bit-identical against.
fn exp_lane(x: f32) -> f32 {
    let x = x.clamp(EXP_LO, EXP_HI);
    let n = x.mul_add(EXP_LOG2E, 0.5).floor();
    let x = n.mul_add(-EXP_C1, x);
    let x = n.mul_add(-EXP_C2, x);
    let z = x * x;
    let mut p = EXP_P0;
    p = p.mul_add(x, EXP_P1);
    p = p.mul_add(x, EXP_P2);
    p = p.mul_add(x, EXP_P3);
    p = p.mul_add(x, EXP_P4);
    p = p.mul_add(x, EXP_P5);
    let p = p.mul_add(z, x) + 1.0;
    // 2^n by exponent-bit construction; n is integral and within the f32
    // exponent range after the clamp above.
    let pow2n = f32::from_bits((((n as i32) + 127) << 23) as u32);
    p * pow2n
}

fn exp_f32x8_scalar(x: [f32; 8]) -> [f32; 8] {
    let mut out = [0f32; 8];
    for (o, v) in out.iter_mut().zip(x) {
        *o = exp_lane(v);
    }
    out
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2,fma")]
fn exp_f32x8_avx2(lanes: [f32; 8]) -> [f32; 8] {
    use std::arch::x86_64::{
        _MM_FROUND_NO_EXC, _MM_FROUND_TO_NEG_INF, _mm256_add_epi32, _mm256_add_ps,
        _mm256_castsi256_ps, _mm256_cvtps_epi32, _mm256_fmadd_ps, _mm256_fnmadd_ps,
        _mm256_loadu_ps, _mm256_max_ps, _mm256_min_ps, _mm256_mul_ps, _mm256_round_ps,
        _mm256_set1_epi32, _mm256_set1_ps, _mm256_slli_epi32, _mm256_storeu_ps,
    };
    let mut out = [0f32; 8];
    // SAFETY: fixed-size 8-lane array loads/stores; all in bounds.
    unsafe {
        let val = _mm256_loadu_ps(lanes.as_ptr());
        let val = _mm256_max_ps(
            _mm256_min_ps(val, _mm256_set1_ps(EXP_HI)),
            _mm256_set1_ps(EXP_LO),
        );
        // n = floor(fma(x, LOG2E, 0.5)); floor rounding matches scalar f32::floor,
        // and every fused op below matches the scalar lane's mul_add exactly.
        let nfl = _mm256_round_ps::<{ _MM_FROUND_TO_NEG_INF | _MM_FROUND_NO_EXC }>(
            _mm256_fmadd_ps(val, _mm256_set1_ps(EXP_LOG2E), _mm256_set1_ps(0.5)),
        );
        let val = _mm256_fnmadd_ps(nfl, _mm256_set1_ps(EXP_C1), val);
        let val = _mm256_fnmadd_ps(nfl, _mm256_set1_ps(EXP_C2), val);
        let zsq = _mm256_mul_ps(val, val);
        let mut poly = _mm256_set1_ps(EXP_P0);
        poly = _mm256_fmadd_ps(poly, val, _mm256_set1_ps(EXP_P1));
        poly = _mm256_fmadd_ps(poly, val, _mm256_set1_ps(EXP_P2));
        poly = _mm256_fmadd_ps(poly, val, _mm256_set1_ps(EXP_P3));
        poly = _mm256_fmadd_ps(poly, val, _mm256_set1_ps(EXP_P4));
        poly = _mm256_fmadd_ps(poly, val, _mm256_set1_ps(EXP_P5));
        let poly = _mm256_add_ps(_mm256_fmadd_ps(poly, zsq, val), _mm256_set1_ps(1.0));
        // 2^n via exponent bits; cvtps rounds-to-nearest, exact on integral n.
        let n_int = _mm256_cvtps_epi32(nfl);
        let pow2n = _mm256_castsi256_ps(_mm256_slli_epi32::<23>(_mm256_add_epi32(
            n_int,
            _mm256_set1_epi32(127),
        )));
        _mm256_storeu_ps(out.as_mut_ptr(), _mm256_mul_ps(poly, pow2n));
    }
    out
}

/// Vectorized `exp` over 8 `f32` lanes -- the spectral tracer's per-channel
/// Beer-Lambert/transmittance primitive.
///
/// Bit-identical across dispatch levels
/// (AVX-512 machines use the 8-lane AVX2 body; 8 lanes fill one `ymm`), but
/// **not** bit-identical to `f32::exp` -- accuracy is a few ULP (see the
/// module docs and this function's unit tests). Inputs outside
/// `[-87.34, 88.03]` are clamped.
#[must_use]
pub fn exp_f32x8(x: [f32; 8]) -> [f32; 8] {
    match simd_level() {
        #[cfg(target_arch = "x86_64")]
        // SAFETY: dispatch reached only after runtime feature detection
        // (AVX-512 implies AVX2).
        SimdLevel::Avx512 | SimdLevel::Avx2 => unsafe { exp_f32x8_avx2(x) },
        #[cfg(not(target_arch = "x86_64"))]
        SimdLevel::Avx512 | SimdLevel::Avx2 => exp_f32x8_scalar(x),
        SimdLevel::Scalar => exp_f32x8_scalar(x),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `exp_f32x8` must be bit-identical across dispatch levels, and within a
    /// tight relative error of the `f64` exponential over the tracer's
    /// operating range.
    #[test]
    fn exp8_is_level_identical_and_accurate() {
        let mut worst_rel = 0f64;
        let mut step = 0i32;
        loop {
            let x = (step as f32).mul_add(0.37, -87.0f32);
            if x >= 10.0 {
                break;
            }
            let lanes = [
                x,
                x + 0.111,
                x + 0.222,
                x + 0.333,
                -x * 0.1,
                x * 0.5,
                x + 0.777,
                0.0,
            ];
            let got = exp_f32x8(lanes);
            let scalar = exp_f32x8_scalar(lanes);
            for lane in 0..8 {
                assert_eq!(
                    got[lane].to_bits(),
                    scalar[lane].to_bits(),
                    "lane {lane} at x={x}"
                );
                let reference = f64::from(lanes[lane]).exp();
                if reference > 1e-30 {
                    let rel = (f64::from(got[lane]) - reference).abs() / reference;
                    worst_rel = worst_rel.max(rel);
                }
            }
            step += 1;
        }
        assert!(
            worst_rel < 5e-7,
            "exp8 worst relative error {worst_rel} exceeds 5e-7"
        );
    }
}
