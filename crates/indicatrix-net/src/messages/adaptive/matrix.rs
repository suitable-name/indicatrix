//! Size classes and the encoding matrix: which encodings to prefer for a payload of a given
//! size on a link of a given bandwidth tier.
//!
//! The table itself is `messages/encoding_matrix.rs`, generated from benchmark
//! measurements; this module only gives it a typed, bounds-safe lookup.

use super::tier::{BandwidthTier, TIER_COUNT};
use crate::messages::{
    DisplayEncoding, PayloadEncoding,
    encoding_matrix::{DISPLAY_MATRIX, PAYLOAD_MATRIX},
};

/// Raw payloads below this many bytes are [`SizeClass::Small`].
pub const SMALL_LIMIT_BYTES: usize = 1 << 20;

/// Raw payloads below this many bytes (and at least [`SMALL_LIMIT_BYTES`]) are
/// [`SizeClass::Medium`]; anything larger is [`SizeClass::Large`].
pub const MEDIUM_LIMIT_BYTES: usize = 16 << 20;

/// Number of size classes (rows of the matrix).
pub const SIZE_CLASS_COUNT: usize = 3;

/// Bytes per pixel of a radiance payload (three `f32`).
const RADIANCE_BYTES_PER_PIXEL: usize = 12;

/// A payload size class, by RAW (uncompressed) payload bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SizeClass {
    /// Below [`SMALL_LIMIT_BYTES`] (1 MiB): up to roughly 295 x 295 pixels.
    Small,
    /// Below [`MEDIUM_LIMIT_BYTES`] (16 MiB): up to roughly 1180 x 1180 pixels.
    Medium,
    /// 16 MiB and more.
    Large,
}

impl SizeClass {
    /// Every class, smallest first (the row order of the matrix).
    pub const ALL: [Self; SIZE_CLASS_COUNT] = [Self::Small, Self::Medium, Self::Large];

    /// The class of a payload of `raw_bytes` uncompressed bytes.
    #[must_use]
    pub const fn of_payload_bytes(raw_bytes: usize) -> Self {
        if raw_bytes < SMALL_LIMIT_BYTES {
            Self::Small
        } else if raw_bytes < MEDIUM_LIMIT_BYTES {
            Self::Medium
        } else {
            Self::Large
        }
    }

    /// The class of a picture of `pixels` pixels, judged by the size of its RADIANCE
    /// payload (12 bytes per pixel) so a display frame and the radiance frame of the same
    /// view share a row.
    #[must_use]
    pub const fn of_pixels(pixels: usize) -> Self {
        Self::of_payload_bytes(pixels.saturating_mul(RADIANCE_BYTES_PER_PIXEL))
    }

    /// Row index into the matrix.
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::Small => 0,
            Self::Medium => 1,
            Self::Large => 2,
        }
    }

    /// Lowercase name, as used in the generated file's comments.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Small => "small",
            Self::Medium => "medium",
            Self::Large => "large",
        }
    }
}

/// One matrix row of payload preference lists, one per tier.
pub type PayloadRow = [&'static [PayloadEncoding]; TIER_COUNT];

/// One matrix row of display encodings, one per tier.
pub type DisplayRow = [DisplayEncoding; TIER_COUNT];

/// The (size class x tier) tables of preferred encodings.
#[derive(Debug, Clone, Copy)]
pub struct EncodingMatrix {
    payload: &'static [PayloadRow; SIZE_CLASS_COUNT],
    display: &'static [DisplayRow; SIZE_CLASS_COUNT],
}

impl EncodingMatrix {
    /// The matrix generated from the benchmark (`messages/encoding_matrix.rs`).
    pub const GENERATED: Self = Self::new(&PAYLOAD_MATRIX, &DISPLAY_MATRIX);

    /// A matrix over caller-provided tables (tests, experiments).
    #[must_use]
    pub const fn new(
        payload: &'static [PayloadRow; SIZE_CLASS_COUNT],
        display: &'static [DisplayRow; SIZE_CLASS_COUNT],
    ) -> Self {
        Self { payload, display }
    }

    /// The payload encodings to try, best first, for `class` on `tier`. A well-formed
    /// matrix ends every list in `Raw`.
    #[must_use]
    pub const fn payload_preference(
        &self,
        class: SizeClass,
        tier: BandwidthTier,
    ) -> &'static [PayloadEncoding] {
        self.payload[class.index()][tier.index()]
    }

    /// The display encoding for `class` on `tier`.
    #[must_use]
    pub const fn display_choice(&self, class: SizeClass, tier: BandwidthTier) -> DisplayEncoding {
        self.display[class.index()][tier.index()]
    }
}

impl Default for EncodingMatrix {
    fn default() -> Self {
        Self::GENERATED
    }
}
