//! Size limits for the files the page reads.
//!
//! A picked or dropped file is copied whole into wasm memory, so its size is checked
//! first -- from `File.size`, before a single byte is read -- against these limits.

use crate::hdr::MAX_HDR_FILE_BYTES;

/// The largest design file the page reads, in bytes: 16 MiB.
///
/// A design file (`.indicatrix`, `.asc`, `.gem`, `.gcs`) is a few kilobytes; the
/// limit only stops a wrong file (a video, say) from being copied into memory.
pub const MAX_DESIGN_FILE_BYTES: u64 = 16 * 1024 * 1024;

/// The most bytes the page reads from a file: the `.hdr` cap for an environment map,
/// [`MAX_DESIGN_FILE_BYTES`] for anything else.
#[must_use]
pub const fn read_limit_bytes(is_hdr: bool) -> u64 {
    if is_hdr {
        MAX_HDR_FILE_BYTES
    } else {
        MAX_DESIGN_FILE_BYTES
    }
}

/// Why a file called `name` of `size_bytes` bytes (`File.size`, a float in JavaScript) is
/// not read, or `None` when it may be.
#[must_use]
pub fn size_refusal(name: &str, size_bytes: f64, is_hdr: bool) -> Option<String> {
    const MIB: f64 = 1024.0 * 1024.0;
    let limit = read_limit_bytes(is_hdr);
    // A NaN or negative size is not a size: let the read report what is wrong.
    if size_bytes.is_nan() || size_bytes <= limit as f64 {
        return None;
    }
    let kind = if is_hdr {
        "environment maps"
    } else {
        "design files"
    };
    Some(format!(
        "\"{name}\" is {:.1} MiB; {kind} are limited to {} MiB in the browser, so it was not read.",
        size_bytes / MIB,
        limit / (1024 * 1024)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_limits_are_the_documented_ones() {
        assert_eq!(read_limit_bytes(true), 64 * 1024 * 1024);
        assert_eq!(read_limit_bytes(false), 16 * 1024 * 1024);
    }

    #[test]
    fn a_file_up_to_the_limit_is_read_and_one_byte_over_is_refused() {
        let limit = MAX_DESIGN_FILE_BYTES as f64;
        assert_eq!(size_refusal("round.asc", 0.0, false), None);
        assert_eq!(size_refusal("round.asc", limit, false), None);
        let message = size_refusal("movie.asc", limit + 1.0, false).expect("refused");
        assert!(message.contains("\"movie.asc\""), "{message}");
        assert!(message.contains("16 MiB"), "{message}");
        assert!(message.contains("design files"), "{message}");
    }

    #[test]
    fn a_map_has_the_larger_limit_and_its_own_wording() {
        let limit = MAX_HDR_FILE_BYTES as f64;
        assert_eq!(size_refusal("sky.hdr", limit, true), None);
        assert_eq!(size_refusal("sky.hdr", 20_000_000.0, true), None);
        // Over the design limit but fine for a map; refused as a design file.
        assert!(size_refusal("sky.asc", 20_000_000.0, false).is_some());
        let message = size_refusal("huge.hdr", limit * 2.0, true).expect("refused");
        assert!(message.contains("environment maps"), "{message}");
        assert!(message.contains("64 MiB"), "{message}");
        assert!(message.contains("128.0 MiB"), "{message}");
    }

    #[test]
    fn a_size_that_is_not_a_number_is_left_to_the_read() {
        assert_eq!(size_refusal("odd.asc", f64::NAN, false), None);
        assert_eq!(size_refusal("odd.asc", -1.0, false), None);
    }
}
