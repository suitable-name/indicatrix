//! Test-only helpers shared by the `radiance` submodules' tests: special `f32` bit
//! patterns and a deterministic pseudo-random generator over them.

/// Every special class of `f32` bit pattern a lossless encoding must carry unchanged.
pub const SPECIAL_BITS: [u32; 12] = [
    0x0000_0000, // +0
    0x8000_0000, // -0
    0x7fc0_0000, // canonical quiet NaN
    0x7fc0_1234, // quiet NaN with payload
    0xffc5_a5a5, // negative quiet NaN with payload
    0x7f80_0001, // signalling NaN
    0xff80_0001, // negative signalling NaN
    0x7f80_0000, // +inf
    0xff80_0000, // -inf
    0x0000_0001, // smallest subnormal
    0x807f_ffff, // largest negative subnormal
    0x3f80_0000, // 1.0
];

/// `len` pseudo-random `f32` bit patterns (xorshift32), every third one a special value
/// from [`SPECIAL_BITS`].
pub fn mixed_bits(len: usize, seed: u32) -> Vec<u32> {
    let mut state = seed | 1;
    (0..len)
        .map(|i| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            if i % 3 == 0 {
                SPECIAL_BITS[(i / 3) % SPECIAL_BITS.len()]
            } else {
                state
            }
        })
        .collect()
}

/// Little-endian bytes of `bits`, the wire layout of a radiance payload.
pub fn to_bytes(bits: &[u32]) -> Vec<u8> {
    bits.iter().flat_map(|b| b.to_le_bytes()).collect()
}

/// A smooth, compressible radiance-like payload of `pixels` pixels (a gradient with
/// low-bit noise), as raw little-endian bytes.
pub fn smooth_payload(pixels: usize) -> Vec<u8> {
    let mut state = 0x9e37_79b9_u32;
    (0..pixels * 3)
        .flat_map(|i| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let base = 1.0 + (i % 97) as f32 / 97.0;
            let noisy = f32::from_bits(base.to_bits() ^ (state & 0xff));
            noisy.to_le_bytes()
        })
        .collect()
}
