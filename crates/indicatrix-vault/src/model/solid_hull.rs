//! The cached finished-solid convex hull vertices stored per design for the Rough Planner.
//!
//! See `crate::db::sqlite::Database::save_solid_hull`/`solid_hulls_for` for the storage
//! side and `crate::db::sqlite::Database::migrate_diagram_solid_hull_table` for why this
//! is a side table keyed by `entry_id`.
//!
//! Stored in model units, design frame, as measured. The app rotates these vertices into
//! the caliper frame as needed for fitting.

/// The hull format's measuring version.
///
/// Bump it whenever the format or measuring rule that produces [`SolidHull`] changes:
/// a stored row whose `hull_version` differs from this is treated as missing by
/// `Database::solid_hulls_for`, so the scan re-measures it.
pub const SOLID_HULL_VERSION: u32 = 1;

/// The error type returned when decoding a [`SolidHull`] from raw bytes fails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SolidHullError {
    /// The input slice length is not a multiple of 12 bytes (3 x 4-byte `f32`).
    InvalidByteLength {
        /// Actual length of the slice in bytes.
        len: usize,
    },
    /// A vertex has a NaN or infinite coordinate, which no measured hull contains.
    NonFiniteVertex {
        /// Index of the first offending vertex.
        index: usize,
    },
}

impl std::fmt::Display for SolidHullError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidByteLength { len } => {
                write!(
                    f,
                    "solid hull byte length {len} is not a multiple of 12 bytes (3 x 4-byte f32 per vertex)"
                )
            }
            Self::NonFiniteVertex { index } => {
                write!(f, "solid hull vertex {index} has a non-finite coordinate")
            }
        }
    }
}

impl std::error::Error for SolidHullError {}

/// One finished design's convex hull vertices in model units, design frame.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SolidHull {
    /// The distinct vertices of the convex hull, in model units, design frame.
    pub vertices: Vec<[f32; 3]>,
}

impl SolidHull {
    /// Packs this convex hull into bytes as consecutive little-endian `[f32; 3]` triples.
    ///
    /// [`Self::from_bytes`] is the exact inverse.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.vertices.len() * 12);
        for &[x, y, z] in &self.vertices {
            out.extend_from_slice(&x.to_le_bytes());
            out.extend_from_slice(&y.to_le_bytes());
            out.extend_from_slice(&z.to_le_bytes());
        }
        out
    }

    /// Unpacks `bytes` (as written by [`Self::to_bytes`]) back into a [`SolidHull`].
    ///
    /// # Errors
    ///
    /// Returns [`SolidHullError::InvalidByteLength`] if `bytes.len()` is not a multiple of 12,
    /// and [`SolidHullError::NonFiniteVertex`] if any coordinate is NaN or infinite.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, SolidHullError> {
        if !bytes.len().is_multiple_of(12) {
            return Err(SolidHullError::InvalidByteLength { len: bytes.len() });
        }
        let (chunks, _) = bytes.as_chunks::<12>();
        let mut vertices = Vec::with_capacity(chunks.len());
        for (index, chunk) in chunks.iter().enumerate() {
            let x = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
            let y = f32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]);
            let z = f32::from_le_bytes([chunk[8], chunk[9], chunk[10], chunk[11]]);
            if !(x.is_finite() && y.is_finite() && z.is_finite()) {
                return Err(SolidHullError::NonFiniteVertex { index });
            }
            vertices.push([x, y, z]);
        }
        Ok(Self { vertices })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_hull_round_trips_to_empty_bytes() {
        let hull = SolidHull::default();
        let bytes = hull.to_bytes();
        assert_eq!(bytes, [] as [u8; 0]);
        let decoded = SolidHull::from_bytes(&bytes).expect("decode empty hull");
        assert_eq!(decoded, hull);
    }

    #[test]
    fn vertices_round_trip_little_endian() {
        let hull = SolidHull {
            vertices: vec![
                [1.0, 2.5, -3.75],
                [0.0, -0.0, 1000.125],
                [-42.25, 123.456, 0.0001],
            ],
        };
        let bytes = hull.to_bytes();
        assert_eq!(bytes.len(), 3 * 12);
        let decoded = SolidHull::from_bytes(&bytes).expect("decode vertices");
        assert_eq!(decoded, hull);
    }

    #[test]
    fn invalid_byte_length_is_rejected() {
        let err = SolidHull::from_bytes(&[0u8; 11]).unwrap_err();
        assert_eq!(err, SolidHullError::InvalidByteLength { len: 11 });
        let err = SolidHull::from_bytes(&[0u8; 13]).unwrap_err();
        assert_eq!(err, SolidHullError::InvalidByteLength { len: 13 });
    }

    #[test]
    fn non_finite_coordinates_are_rejected() {
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let hull = SolidHull {
                vertices: vec![[1.0, 2.0, 3.0], [4.0, bad, 6.0]],
            };
            let err = SolidHull::from_bytes(&hull.to_bytes()).unwrap_err();
            assert_eq!(err, SolidHullError::NonFiniteVertex { index: 1 });
        }
    }
}
