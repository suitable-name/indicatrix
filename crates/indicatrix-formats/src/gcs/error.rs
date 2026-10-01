//! [`GcsParseError`]: everything that can go wrong parsing a `.gcs` design
//! with [`super::parse_gcs`].

use std::fmt;

/// Everything that can go wrong parsing a `.gcs` design with [`super::parse_gcs`].
///
/// Every numeric attribute must be a finite number: `nan` and `inf` are reported
/// through the matching `...NotNumeric` variant like any other garbled value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GcsParseError {
    /// `content` was empty or contained only whitespace.
    EmptyInput,
    /// A `<` was never closed by a matching `>` before the file ended.
    UnterminatedTag {
        /// 1-based line the unterminated tag started on.
        line: usize,
    },
    /// An attribute inside a tag had a key but no closing quote for its value.
    MalformedAttribute {
        /// 1-based line the tag containing the bad attribute started on.
        line: usize,
        /// The attribute's key, if one was found before the parser gave up.
        key: String,
    },
    /// The file did not open with a `<GemCutStudio ...>` root element.
    MissingRootElement,
    /// The root element had no `<index .../>` child.
    MissingIndexElement,
    /// The `<index>` element was missing a required attribute.
    IndexAttributeMissing {
        /// The missing attribute's name (only `"gear"` is required).
        attr: &'static str,
    },
    /// An `<index>` attribute's value did not parse as a finite number.
    IndexAttributeNotNumeric {
        /// The attribute's name.
        attr: &'static str,
        /// The offending raw value.
        value: String,
    },
    /// A `<tier>` element had no `angle`, or neither a `depth` nor any vertex to
    /// derive one from.
    TierAttributeMissing {
        /// 0-based position of this tier among the file's `<tier>` elements.
        tier_index: usize,
        /// The missing attribute's name (`"angle"` or `"depth"`).
        attr: &'static str,
    },
    /// A `<tier>` attribute's value did not parse as a finite number.
    TierAttributeNotNumeric {
        /// 0-based position of this tier among the file's `<tier>` elements.
        tier_index: usize,
        /// The attribute's name.
        attr: &'static str,
        /// The offending raw value.
        value: String,
    },
    /// A `<facet>` element had only part of a normal (`attr` names the first missing
    /// component), or neither a normal nor an `index_angle` (`attr` is
    /// `"index_angle"`).
    FacetAttributeMissing {
        /// 0-based position of this tier among the file's `<tier>` elements.
        tier_index: usize,
        /// The missing attribute's name (`"nx"`, `"ny"`, `"nz"`, or `"index_angle"`).
        attr: &'static str,
    },
    /// A `<facet>` attribute's value did not parse as a finite number.
    FacetAttributeNotNumeric {
        /// 0-based position of this tier among the file's `<tier>` elements.
        tier_index: usize,
        /// The attribute's name (`"nx"`, `"ny"`, `"nz"`, or `"index_angle"`).
        attr: &'static str,
        /// The offending raw value.
        value: String,
    },
    /// A `<vertex>` element was missing a required attribute.
    VertexAttributeMissing {
        /// 0-based position of this tier among the file's `<tier>` elements.
        tier_index: usize,
        /// The missing attribute's name (`"x"`, `"y"`, or `"z"`).
        attr: &'static str,
    },
    /// A `<vertex>` attribute's value did not parse as a finite number.
    VertexAttributeNotNumeric {
        /// 0-based position of this tier among the file's `<tier>` elements.
        tier_index: usize,
        /// The attribute's name (`"x"`, `"y"`, or `"z"`).
        attr: &'static str,
        /// The offending raw value.
        value: String,
    },
    /// A `<render>` attribute was present but did not parse as a finite number. Absent
    /// entirely is not an error (these fields are optional and default to `0.0`) --
    /// only a garbled value present on the attribute goes through this fallible path,
    /// same as [`Self::TierAttributeNotNumeric`].
    RenderAttributeNotNumeric {
        /// The attribute's name (`"refractive_index"`, `"dispersion"`, `"clarity"`, or
        /// `"density"`).
        attr: &'static str,
        /// The offending raw value.
        value: String,
    },
    /// A `<color>` attribute was present but did not parse as a finite number. Absent
    /// entirely is not an error (defaults to `0.0`, same rule as
    /// [`Self::RenderAttributeNotNumeric`]) -- only a garbled value present on the
    /// attribute goes through this fallible path.
    ColorAttributeNotNumeric {
        /// The attribute's name (`"r"`, `"g"`, or `"b"`).
        attr: &'static str,
        /// The offending raw value.
        value: String,
    },
    /// A `<tier>` or `<facet>` or `<render>` element was opened but the file ended
    /// (or the root closed) before its matching close tag appeared.
    UnterminatedElement {
        /// The element name (`"tier"`, `"facet"`, or `"render"`).
        name: &'static str,
    },
}

impl fmt::Display for GcsParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyInput => write!(f, "empty input"),
            Self::UnterminatedTag { line } => {
                write!(f, "line {line}: '<' was never closed by a matching '>'")
            }
            Self::MalformedAttribute { line, key } => write!(
                f,
                "line {line}: attribute {key:?} has no closing quote for its value"
            ),
            Self::MissingRootElement => {
                write!(f, "missing '<GemCutStudio ...>' root element")
            }
            Self::MissingIndexElement => write!(f, "missing '<index .../>' element"),
            Self::IndexAttributeMissing { attr } => {
                write!(f, "'<index>' is missing its {attr:?} attribute")
            }
            Self::IndexAttributeNotNumeric { attr, value } => write!(
                f,
                "'<index>' attribute {attr:?} value {value:?} is not a finite number"
            ),
            Self::TierAttributeMissing { tier_index, attr } => {
                write!(f, "tier #{tier_index} is missing its {attr:?} attribute")
            }
            Self::TierAttributeNotNumeric {
                tier_index,
                attr,
                value,
            } => write!(
                f,
                "tier #{tier_index} attribute {attr:?} value {value:?} is not a finite number"
            ),
            Self::FacetAttributeMissing { tier_index, attr } => write!(
                f,
                "tier #{tier_index}: facet is missing its {attr:?} attribute"
            ),
            Self::FacetAttributeNotNumeric {
                tier_index,
                attr,
                value,
            } => write!(
                f,
                "tier #{tier_index}: facet attribute {attr:?} value {value:?} is not a finite number"
            ),
            Self::VertexAttributeMissing { tier_index, attr } => write!(
                f,
                "tier #{tier_index}: vertex is missing its {attr:?} attribute"
            ),
            Self::VertexAttributeNotNumeric {
                tier_index,
                attr,
                value,
            } => write!(
                f,
                "tier #{tier_index}: vertex attribute {attr:?} value {value:?} is not a finite number"
            ),
            Self::RenderAttributeNotNumeric { attr, value } => write!(
                f,
                "'<render>' attribute {attr:?} value {value:?} is not a finite number"
            ),
            Self::ColorAttributeNotNumeric { attr, value } => write!(
                f,
                "'<color>' attribute {attr:?} value {value:?} is not a finite number"
            ),
            Self::UnterminatedElement { name } => {
                write!(f, "'<{name}>' was opened but never closed")
            }
        }
    }
}

impl std::error::Error for GcsParseError {}
