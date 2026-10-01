//! `.gem` (`GemCAD`'s binary save file) and `.gcs` (Gem Cut Studio) designs,
//! converted to `.asc` cutting instructions so Import and the editor's Open... can
//! hand them to the SAME `.asc` path every other design takes.
//!
//! The readers and converters live in `indicatrix_formats::gem`/`gcs`; the
//! conversion to `.asc` text is `indicatrix_vault::local::design_file_to_asc_text`,
//! the same one the editor's catalogue record loader and the worker's
//! `FetchDesignSource` use. This module only picks the format from a file path.

use indicatrix_vault::local::{DesignFileKind, design_file_to_asc_text};
use std::path::Path;

pub use indicatrix_vault::local::converted_asc_file_name;

/// A faceting-design format other than `.asc` that Import and Open... read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForeignFormat {
    /// `GemCAD`'s binary `.gem` save file.
    Gem,
    /// Gem Cut Studio's XML `.gcs` file.
    Gcs,
}

impl ForeignFormat {
    /// The format `path`'s extension names (case-insensitive), or `None` for any
    /// other extension.
    #[must_use]
    pub fn from_path(path: &Path) -> Option<Self> {
        match DesignFileKind::from_file_name(&path.file_name()?.to_string_lossy())? {
            DesignFileKind::Gem => Some(Self::Gem),
            DesignFileKind::Gcs => Some(Self::Gcs),
            DesignFileKind::Asc => None,
        }
    }

    /// The format's file extension with its leading dot, for messages.
    #[must_use]
    pub const fn dotted_extension(self) -> &'static str {
        self.kind().dotted_extension()
    }

    /// The matching `indicatrix_vault` design-file kind.
    const fn kind(self) -> DesignFileKind {
        match self {
            Self::Gem => DesignFileKind::Gem,
            Self::Gcs => DesignFileKind::Gcs,
        }
    }
}

/// A `.gem`/`.gcs` design converted to `.asc` cutting instructions.
#[derive(Debug, Clone)]
pub struct ConvertedDesign {
    /// The converted cutting instructions written as `.asc` text.
    pub asc_text: String,
    /// What the reader and converter noted about the source file: a `.gem`
    /// preform section that is not converted, `.gcs` hidden or guide tiers, a
    /// missing refractive index, unknown `.gcs` elements or attributes. Empty for
    /// a clean file.
    pub warnings: Vec<String>,
}

/// Parses `bytes` as a `format` file and converts it to `.asc` cutting
/// instructions (`indicatrix_vault::local::design_file_to_asc_text`).
///
/// # Errors
///
/// A message naming the format and the reader's typed error when the file does
/// not parse, when it describes no facets at all, or when the converted schedule
/// cannot be written as `.asc`.
pub fn convert_foreign_design(
    format: ForeignFormat,
    bytes: &[u8],
) -> Result<ConvertedDesign, String> {
    design_file_to_asc_text(format.dotted_extension(), format.kind(), bytes).map(|file| {
        ConvertedDesign {
            asc_text: file.asc_text,
            warnings: file.warnings,
        }
    })
}

/// A synthetic `.gem` design and a tiny `.gem` encoder, shared by the import tests
/// and the editor's catalogue-record loader tests (the formats crate's own
/// encoder is private to its tests).
#[cfg(test)]
pub mod test_gem {
    /// One tier of [`encode_gem`]'s synthetic design: name, signed `.asc` angle
    /// (crown `+`, pavilion `-`), mast, and index-wheel positions.
    pub type GemTierSpec<'a> = (&'a str, f64, f64, &'a [f64]);

    /// The synthetic four-tier design the `.gem` tests import: pavilion, girdle,
    /// crown and table on a 96 gear, a bounded stone.
    pub const SYNTHETIC_GEM_TIERS: &[GemTierSpec<'static>] = &[
        (
            "P1",
            -41.0,
            0.75,
            &[96.0, 12.0, 24.0, 36.0, 48.0, 60.0, 72.0, 84.0],
        ),
        (
            "G1",
            -90.0,
            1.0,
            &[
                96.0, 6.0, 12.0, 18.0, 24.0, 30.0, 36.0, 42.0, 48.0, 54.0, 60.0, 66.0, 72.0, 78.0,
                84.0, 90.0,
            ],
        ),
        (
            "C1",
            35.0,
            0.9,
            &[96.0, 12.0, 24.0, 36.0, 48.0, 60.0, 72.0, 84.0],
        ),
        ("T", 0.0, 0.6, &[96.0]),
    ];

    /// Appends one length-prefixed (`u8`) string, as `.gem` stores labels and
    /// headings.
    fn push_gem_string(out: &mut Vec<u8>, text: &str) {
        out.push(u8::try_from(text.len()).expect("test strings are short"));
        out.extend_from_slice(text.as_bytes());
    }

    /// A tiny `.gem` encoder (the formats crate's own encoder is test-only there):
    /// per facet `f64 px, py, pz` (`p = n / mast`, `phi = 90° - 360°·i/gear`), `i32`
    /// tier, a length-prefixed `name\tinstructions` label (the name on the tier's
    /// first facet only, as `GemCAD` writes it) and an empty vertex list (`i32 0`);
    /// then the `-99999.0` sentinel and the trailer (`i32 symmetry, i32 mirror, i32
    /// gear, f64 ri, u32 0x7FFF, f64 offset`, eight length-prefixed strings with the
    /// title first).
    pub fn encode_gem(tiers: &[GemTierSpec<'_>], gear: i32, title: &str) -> Vec<u8> {
        let mut out = Vec::new();
        for (tier_number, (name, angle, mast, indices)) in (1_i32..).zip(tiers) {
            let theta = angle.abs().to_radians();
            let z_sign = if angle.is_sign_negative() { -1.0 } else { 1.0 };
            for (k, index) in indices.iter().enumerate() {
                let phi = (90.0 - 360.0 * index / f64::from(gear)).to_radians();
                let normal = [
                    theta.sin() * phi.cos(),
                    theta.sin() * phi.sin(),
                    z_sign * theta.cos(),
                ];
                for component in normal {
                    out.extend_from_slice(&(component / mast).to_le_bytes());
                }
                out.extend_from_slice(&tier_number.to_le_bytes());
                let label = if k == 0 {
                    format!("{name}\t")
                } else {
                    "\t".to_string()
                };
                push_gem_string(&mut out, &label);
                out.extend_from_slice(&0_i32.to_le_bytes());
            }
        }
        out.extend_from_slice(&(-99_999.0_f64).to_le_bytes());
        out.extend_from_slice(&8_i32.to_le_bytes());
        out.extend_from_slice(&1_i32.to_le_bytes());
        out.extend_from_slice(&gear.to_le_bytes());
        out.extend_from_slice(&1.54_f64.to_le_bytes());
        out.extend_from_slice(&0x7FFF_u32.to_le_bytes());
        out.extend_from_slice(&0.0_f64.to_le_bytes());
        push_gem_string(&mut out, title);
        for _ in 0..7 {
            push_gem_string(&mut out, "");
        }
        out
    }
}
