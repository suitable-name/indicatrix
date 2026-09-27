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

/// Bytes per `f32` value: the number of byte planes.
pub const PLANES: usize = 4;

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

#[cfg(test)]
mod tests {
    use super::{super::test_support::*, *};

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
