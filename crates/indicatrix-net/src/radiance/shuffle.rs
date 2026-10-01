//! The byte shuffle in front of every compressed payload encoding (v14).
//!
//! It reorders a sequence of little-endian `f32` values into 4 byte planes (all byte
//! 0s, then all byte 1s, then all byte 2s, then all byte 3s). Exponent and high-mantissa bytes of
//! neighbouring radiance values are similar, so grouping them lets a general-purpose
//! codec find the redundancy the interleaved layout hides (measured ratio 1.00 ->
//! 1.41 for LZ4 from the shuffle alone).
//!
//! Pure byte permutations: bit-exact for every `f32` bit pattern (NaN payloads, signed
//! zeros, subnormals, infinities), since no value is ever interpreted as a float.

use glam::Vec3;

/// Bytes per `f32` value: the number of byte planes.
pub const PLANES: usize = 4;

/// Whether a received radiance sample may be summed.
///
/// Every component must be finite and not negative. A sample failing this is dropped (it
/// adds nothing) but still counts towards the frame's sample total, the same rule every
/// render backend applies to its own samples, so one hostile or corrupt value can never
/// poison a running sum.
#[inline]
#[must_use]
pub fn is_valid_sample(v: Vec3) -> bool {
    v.is_finite() && v.x >= 0.0 && v.y >= 0.0 && v.z >= 0.0
}

/// Byte-shuffles `src` (little-endian `f32` values) into `dst`: all byte 0s, then all
/// byte 1s, then all byte 2s, then all byte 3s. The inverse of [`unshuffle_into`].
///
/// # Panics
///
/// Panics if `src.len()` is not a multiple of 4 or `dst.len() != src.len()`.
pub fn shuffle_into(src: &[u8], dst: &mut [u8]) {
    assert!(
        src.len().is_multiple_of(PLANES),
        "payload is not a whole number of f32 values"
    );
    assert_eq!(
        src.len(),
        dst.len(),
        "shuffle output must match the input length"
    );
    let n = src.len() / PLANES;
    let (p0, rest) = dst.split_at_mut(n);
    let (p1, rest) = rest.split_at_mut(n);
    let (p2, p3) = rest.split_at_mut(n);
    for (i, v) in src.as_chunks::<PLANES>().0.iter().enumerate() {
        p0[i] = v[0];
        p1[i] = v[1];
        p2[i] = v[2];
        p3[i] = v[3];
    }
}

/// Re-interleaves 4 byte planes (as written by [`shuffle_into`]) back into
/// little-endian `f32` values.
///
/// # Panics
///
/// Panics if `src.len()` is not a multiple of 4 or `dst.len() != src.len()`.
pub fn unshuffle_into(src: &[u8], dst: &mut [u8]) {
    assert!(
        src.len().is_multiple_of(PLANES),
        "payload is not a whole number of f32 values"
    );
    assert_eq!(
        src.len(),
        dst.len(),
        "unshuffle output must match the input length"
    );
    let n = src.len() / PLANES;
    let (p0, rest) = src.split_at(n);
    let (p1, rest) = rest.split_at(n);
    let (p2, p3) = rest.split_at(n);
    for (i, v) in dst.as_chunks_mut::<PLANES>().0.iter_mut().enumerate() {
        v[0] = p0[i];
        v[1] = p1[i];
        v[2] = p2[i];
        v[3] = p3[i];
    }
}

/// Adds the `f32` values encoded in shuffled `planes` onto `acc`, element by element.
///
/// [`unshuffle_into`] fused with the accumulator's `+=`, so a compressed FRAME delta is
/// summed without a second full-frame scratch buffer. Bit-identical to unshuffling into
/// a `[f32]` and adding that (one IEEE `f32` add per element either way).
///
/// # Panics
///
/// Panics if `planes.len()` is not a multiple of 4 or `planes.len() != acc.len() * 4`.
pub fn add_unshuffled(planes: &[u8], acc: &mut [f32]) {
    assert!(
        planes.len().is_multiple_of(PLANES),
        "payload is not a whole number of f32 values"
    );
    assert_eq!(
        planes.len(),
        acc.len() * PLANES,
        "planes must hold exactly one f32 per accumulator element"
    );
    let n = acc.len();
    let (p0, rest) = planes.split_at(n);
    let (p1, rest) = rest.split_at(n);
    let (p2, p3) = rest.split_at(n);
    for (i, a) in acc.iter_mut().enumerate() {
        *a += f32::from_le_bytes([p0[i], p1[i], p2[i], p3[i]]);
    }
}

/// Adds the pixels encoded in shuffled `planes` onto `acc`, skipping every pixel that
/// fails [`is_valid_sample`]. Returns how many pixels were skipped.
///
/// For valid pixels this is bit-identical to [`add_unshuffled`] over the same floats (one
/// IEEE `f32` add per component).
///
/// # Panics
///
/// Panics if `planes.len() != acc.len() * 12`.
pub fn add_unshuffled_valid(planes: &[u8], acc: &mut [Vec3]) -> u32 {
    assert_eq!(
        planes.len(),
        acc.len() * 3 * PLANES,
        "planes must hold exactly three f32 per accumulator pixel"
    );
    let n = acc.len() * 3;
    let (p0, rest) = planes.split_at(n);
    let (p1, rest) = rest.split_at(n);
    let (p2, p3) = rest.split_at(n);
    let at = |i: usize| f32::from_le_bytes([p0[i], p1[i], p2[i], p3[i]]);
    let mut dropped = 0u32;
    for (pixel, a) in acc.iter_mut().enumerate() {
        let base = pixel * 3;
        let v = Vec3::new(at(base), at(base + 1), at(base + 2));
        if is_valid_sample(v) {
            *a += v;
        } else {
            dropped = dropped.saturating_add(1);
        }
    }
    dropped
}

#[cfg(test)]
mod tests {
    use super::{super::test_support::*, *};

    /// Non-finite and negative pixels are skipped and counted; valid pixels sum exactly
    /// like the unguarded add.
    #[test]
    fn add_unshuffled_valid_skips_and_counts_invalid_pixels() {
        let pixels = [
            Vec3::new(1.0, 2.0, 3.0),
            Vec3::new(f32::NAN, 1.0, 1.0),
            Vec3::new(1.0, f32::INFINITY, 1.0),
            Vec3::new(1.0, 1.0, -0.5),
            Vec3::new(0.0, -0.0, 4.0),
        ];
        let raw: Vec<u8> = pixels
            .iter()
            .flat_map(Vec3::to_array)
            .flat_map(f32::to_le_bytes)
            .collect();
        let mut planes = vec![0; raw.len()];
        shuffle_into(&raw, &mut planes);

        let mut acc = vec![Vec3::ONE; pixels.len()];
        let dropped = add_unshuffled_valid(&planes, &mut acc);
        assert_eq!(dropped, 3);
        assert_eq!(acc[0], Vec3::new(2.0, 3.0, 4.0));
        assert_eq!(acc[1], Vec3::ONE);
        assert_eq!(acc[2], Vec3::ONE);
        assert_eq!(acc[3], Vec3::ONE);
        assert_eq!(acc[4], Vec3::new(1.0, 1.0, 5.0));
    }

    /// Property: for many lengths and seeds, unshuffle(shuffle(x)) == x byte for byte.
    #[test]
    fn shuffle_round_trips_bit_exactly_over_random_and_special_patterns() {
        for (len, seed) in [(0, 1), (1, 2), (3, 3), (12, 4), (997, 5), (3 * 4096, 6)] {
            let raw = to_bytes(&mixed_bits(len, seed));
            let mut shuffled = vec![0; raw.len()];
            let mut back = vec![0; raw.len()];
            shuffle_into(&raw, &mut shuffled);
            unshuffle_into(&shuffled, &mut back);
            assert_eq!(back, raw, "len={len} seed={seed}");
        }
    }

    #[test]
    fn shuffle_groups_bytes_by_plane() {
        let raw = [0x10, 0x11, 0x12, 0x13, 0x20, 0x21, 0x22, 0x23];
        let mut shuffled = [0; 8];
        shuffle_into(&raw, &mut shuffled);
        assert_eq!(shuffled, [0x10, 0x20, 0x11, 0x21, 0x12, 0x22, 0x13, 0x23]);
    }

    /// The fused add agrees bit for bit with unshuffle-then-add, NaN payloads and
    /// signed zeros included.
    #[test]
    fn add_unshuffled_matches_unshuffle_then_add_bit_for_bit() {
        let delta_bits = mixed_bits(3 * 331, 9);
        let start_bits = mixed_bits(3 * 331, 10);
        let raw = to_bytes(&delta_bits);
        let mut planes = vec![0; raw.len()];
        shuffle_into(&raw, &mut planes);

        let mut fused: Vec<f32> = start_bits.iter().map(|b| f32::from_bits(*b)).collect();
        add_unshuffled(&planes, &mut fused);

        let mut reference: Vec<f32> = start_bits.iter().map(|b| f32::from_bits(*b)).collect();
        for (r, d) in reference.iter_mut().zip(&delta_bits) {
            *r += f32::from_bits(*d);
        }
        let fused_bits: Vec<u32> = fused.iter().map(|f| f.to_bits()).collect();
        let reference_bits: Vec<u32> = reference.iter().map(|f| f.to_bits()).collect();
        assert_eq!(fused_bits, reference_bits);
    }

    #[test]
    #[should_panic(expected = "whole number of f32 values")]
    fn shuffle_rejects_a_partial_value() {
        shuffle_into(&[1, 2, 3], &mut [0; 3]);
    }
}
