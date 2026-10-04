//! The cached finished-solid extents this crate stores per design, for the Rough Planner.
//!
//! See `crate::db::sqlite::Database::save_solid_extents`/`solid_extents_for`/
//! `delete_solid_extents` for the storage side and
//! `crate::db::sqlite::Database::migrate_diagram_solid_extents_table` for why this is a
//! side table keyed by `entry_id`.
//!
//! # Measuring rule
//!
//! The scan that fills this cache (in `apps/indicatrix-cut`, not this crate) measures a
//! design's FACET planes alone, never the preform's: a catalogue `.asc` gets a
//! deliberately oversized default preform cylinder, so a schedule that does not close on
//! its own would otherwise measure as a fat cylinder-sized blob with an inflated volume.
//!
//! - The design file's planes are split at the preform's plane count and only the facet
//!   planes are measured. If they close, the row is stored with
//!   [`SolidExtentsSource::DesignFile`]. Its widths, lengths and height are the flat facet
//!   stone's; its `volume` is that stone minus the design's concave tools (curved-tool
//!   cuts), so it equals the flat volume for a planar design.
//! - If they do not close, the row is stored with NULL extents and
//!   [`SolidExtentsSource::Unbounded`] ("measured, unusable"), so the scan does not retry
//!   it every run.
//! - A design with no design file at all falls back to the synthetic angle-table
//!   geometry and is stored with [`SolidExtentsSource::AngleTable`]; those extents are
//!   fabricated, so the planner excludes them by default.
//!
//! All lengths are in model ("mast") units, so only ratios are meaningful until a scale
//! is chosen.

/// The measuring rule's version.
///
/// Bump it whenever the rule that produces [`SolidExtents`] changes: a stored row whose
/// `extents_version` differs from this is treated as missing by
/// `Database::solid_extents_for`, so the scan re-measures it.
///
/// Version 2: the caliper sweep no longer calls the platform `hypot` (edge length is
/// `sqrt(fma(ez, ez, ex*ex))`), ties between equal widths go to the smaller edge angle, and
/// the power-of-two scale normalisation uses exponent extraction instead of
/// `log2`/`powi`. Extents measured under version 1 differ in the last bits.
///
/// Version 3: `volume` is the concave-carved volume (the facet stone minus the design's
/// concave tools); widths, lengths and height are still the flat facet stone's. A cached
/// version-2 volume is the convex one, so it is re-measured.
pub const SOLID_EXTENTS_VERSION: u32 = 3;

/// One finished stone's bounding dimensions and volume, in model units.
///
/// The horizontal pair used by the planner is [`Self::width_caliper`]/
/// [`Self::length_caliper`], the rotation-invariant minimum-width bounding rectangle of
/// the x/z outline (a stone can be dopped at any rotation about its table normal). The
/// axis-aligned pair is stored too, so that choice can be revisited without a re-scan.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SolidExtents {
    /// Minimum width of the x/z outline over all rotations about y; `<= length_caliper`.
    pub width_caliper: f64,
    /// The outline's extent perpendicular to the minimum-width direction.
    pub length_caliper: f64,
    /// The smaller of the two axis-aligned horizontal extents (`x` and `z`).
    pub width_axis: f64,
    /// The larger of the two axis-aligned horizontal extents (`x` and `z`).
    pub length_axis: f64,
    /// The y extent (table to culet).
    pub height: f64,
    /// The enclosed volume.
    pub volume: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_text_round_trips_and_rejects_unknown_text() {
        for source in [
            SolidExtentsSource::DesignFile,
            SolidExtentsSource::AngleTable,
            SolidExtentsSource::Unbounded,
        ] {
            assert_eq!(SolidExtentsSource::parse(source.as_str()), Some(source));
        }
        assert_eq!(SolidExtentsSource::parse("Design_File"), None);
        assert_eq!(SolidExtentsSource::parse(""), None);
    }
}

/// Where a design's cached [`SolidExtents`] geometry came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SolidExtentsSource {
    /// Measured from the design file's facet planes (the only trustworthy source).
    DesignFile,
    /// Measured from the synthetic angle-table fallback geometry; fabricated, not
    /// recorded masts.
    AngleTable,
    /// The design file's facet planes alone do not close, so it relies on its preform to
    /// bound it; stored with no extents.
    Unbounded,
}

impl SolidExtentsSource {
    /// The exact text stored in the `source` column.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DesignFile => "design_file",
            Self::AngleTable => "angle_table",
            Self::Unbounded => "unbounded",
        }
    }

    /// Parses the text stored in the `source` column; `None` for any other text.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "design_file" => Some(Self::DesignFile),
            "angle_table" => Some(Self::AngleTable),
            "unbounded" => Some(Self::Unbounded),
            _ => None,
        }
    }
}

/// One design's cached extents row, exactly as stored -- the value type of
/// `crate::db::sqlite::Database::solid_extents_for`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StoredSolidExtents {
    /// The measured extents, or `None` for "measured, unusable" (its planes did not
    /// close).
    pub extents: Option<SolidExtents>,
    /// Where the geometry came from.
    pub source: SolidExtentsSource,
}
