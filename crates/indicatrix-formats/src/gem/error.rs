//! [`GemParseError`]: every way [`super::parse_gem`] can fail to frame a `.gem` file.

use std::fmt;

/// Everything that can go wrong parsing a `.gem` file with [`super::parse_gem`].
///
/// Every variant except [`Self::EmptyInput`] carries the byte offset where the
/// framing broke, so a failing file can be inspected at the exact spot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GemParseError {
    /// The input was empty.
    EmptyInput,
    /// The file ended in the middle of a field.
    UnexpectedEof {
        /// Byte offset where the field started.
        offset: usize,
        /// The field that was expected there.
        expected: &'static str,
    },
    /// A vertex loop held a continuation flag other than `1` (another vertex) or
    /// `0` (end of the facet's vertices).
    BadVertexFlag {
        /// Byte offset of the flag.
        offset: usize,
        /// The value found.
        found: i32,
    },
    /// A facet's plane vector was not finite, or was the zero vector.
    InvalidPlane {
        /// Byte offset of the facet record.
        offset: usize,
    },
    /// The trailer's mirror flag was neither `0` nor `1`.
    BadMirrorFlag {
        /// Byte offset of the flag.
        offset: usize,
        /// The value found.
        found: i32,
    },
    /// A varint string length ran past five bytes or overflowed `u32`.
    InvalidStringLength {
        /// Byte offset of the length prefix.
        offset: usize,
    },
    /// Bytes followed the trailer strings that are neither end of file nor a
    /// `preform` section.
    TrailingData {
        /// Byte offset of the first unexpected byte.
        offset: usize,
        /// How many bytes were left.
        remaining: usize,
    },
    /// A `preform` tag opened inside a `preform` section. A file holds at most one
    /// preform, directly after the main design; accepting a chain of them would let
    /// a crafted file nest without bound.
    NestedPreform {
        /// Byte offset of the inner `preform` tag.
        offset: usize,
    },
}

impl fmt::Display for GemParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyInput => write!(f, "empty input"),
            Self::UnexpectedEof { offset, expected } => {
                write!(f, "byte {offset}: file ends inside {expected}")
            }
            Self::BadVertexFlag { offset, found } => write!(
                f,
                "byte {offset}: vertex flag {found}, expected 1 (vertex) or 0 (end of vertices)"
            ),
            Self::InvalidPlane { offset } => write!(
                f,
                "byte {offset}: facet plane vector is not finite or is the zero vector"
            ),
            Self::BadMirrorFlag { offset, found } => {
                write!(f, "byte {offset}: mirror flag {found}, expected 0 or 1")
            }
            Self::InvalidStringLength { offset } => {
                write!(
                    f,
                    "byte {offset}: string length prefix is not a valid varint"
                )
            }
            Self::TrailingData { offset, remaining } => write!(
                f,
                "byte {offset}: {remaining} byte(s) after the trailer are not a preform section"
            ),
            Self::NestedPreform { offset } => {
                write!(
                    f,
                    "byte {offset}: a preform section inside a preform section"
                )
            }
        }
    }
}

impl std::error::Error for GemParseError {}
