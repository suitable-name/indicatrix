//! [`CutSheetRow`] and [`CuttingSheet`]: the printable cutting-sheet data and
//! its [`CuttingSheet::to_text`] rendering. Built by [`super::build`]; the
//! shapes themselves carry no solver logic of their own.

use std::fmt::Write as _;

/// One line of a printable cutting sequence: everything a cutter needs to
/// know about a single facet tier, already in cutting order (see
/// [`CuttingSheet::rows`]).
#[derive(Debug, Clone, PartialEq)]
pub struct CutSheetRow {
    /// 1-based position in the cutting sequence -- `design.tiers`' own
    /// order, the same order a real `.asc` file lists tiers in and the order
    /// [`crate::design::Design::to_asc_schedule_from_solved`] preserves.
    pub sequence: usize,
    /// The tier's own name(s), verbatim (see [`crate::design::ConstraintTier::name`]).
    /// Empty for an unnamed tier.
    pub name: String,
    /// Signed angle from the girdle plane, in degrees -- the same `GemCad`
    /// convention [`crate::design::ConstraintTier::angle_deg`] documents.
    pub angle_deg: f64,
    /// Index-wheel positions this facet occurs at, verbatim from
    /// [`crate::design::ConstraintTier::indices`]. Empty means a single facet at azimuth 0.
    pub indices: Vec<f64>,
    /// The solved mast (cutting depth), in the design's mast units -- from
    /// `SolvedTier::mast`.
    pub mast: f64,
    /// What this facet's plane closes against, as printable prose. Prefers
    /// the tier's own recorded `.asc` notes while its constraint is still
    /// the pinned import value (see [`crate::design::ConstraintTier::original_notes`]),
    /// else synthesizes text from its current `MeetConstraint` -- never
    /// empty, unlike the raw `.asc` `G` field, since a blank entry on a
    /// printed sheet reads as "nothing recorded" rather than the actual
    /// default a cutter assumes ("meets whatever the solver finds").
    pub meet_instruction: String,
    /// Which other tiers (indices into `design.tiers`) this row's own
    /// `MeetConstraint::MeetNamed` resolves to -- see
    /// [`crate::design::Design::facet_meets`]. Always empty for
    /// `MeetConstraint::MeetExisting`/`MeetConstraint::ScaleReference`, which
    /// have no named target.
    pub meets_tiers: Vec<usize>,
    /// This row's own cheater/azimuth offset (degrees), from
    /// [`crate::design::Design::cheater_offset_deg`] -- `None` when the tier has no recorded
    /// offset (the common case). A cutter needs this printed alongside the
    /// angle/index/mast figures. Since [`crate::design::export`]'s
    /// `apply_cheater_offsets` rotates this tier's own facet plane(s) by this
    /// figure, the printed sheet and the picture the solid/facet map/tracer show
    /// agree.
    pub cheater_offset_deg: Option<f64>,
    /// This facet's angle of elevation from the girdle plane, unsigned -- the
    /// magnitude of [`Self::angle_deg`], for a cutter reading a machine's dial
    /// (which has no notion of crown/pavilion sign, only "how far up from
    /// level").
    pub angle_of_elevation_deg: f64,
    /// This facet's cutting depth in real millimetres, via
    /// [`crate::yield_metrics::mm_per_unit`] -- `None` whenever
    /// [`crate::design::Design::girdle_diameter_mm`] is unset or the design does not currently
    /// measure as a closed solid (the same conditions
    /// [`crate::yield_metrics::YieldReport::mm_per_unit`] itself returns `None`
    /// under). The cut sheet shows "depth in mm and dial readings", not just the
    /// dimensionless mast.
    pub depth_mm: Option<f64>,
}

/// A design's printable cutting sequence: every tier, in cutting order, with
/// enough information to cut it.
///
/// See [`super`]'s module docs for the problem this solves. Built once by
/// [`crate::design::Design::cutting_sheet`] from an already-solved mast list.
#[derive(Debug, Clone, PartialEq)]
pub struct CuttingSheet {
    /// Schedule-wide context a cutter needs before the first tier: material,
    /// effective refractive index, index-gear tooth count and symmetry, and
    /// the real-world girdle diameter when one is set. One entry per printed
    /// line, in this order -- see [`crate::design::Design::cutting_sheet`] for exactly what
    /// populates it.
    pub header: Vec<String>,
    /// Every tier, in cutting order -- see [`CutSheetRow`].
    pub rows: Vec<CutSheetRow>,
}

impl CuttingSheet {
    /// Renders the sheet as plain text ready to print or save: the header
    /// block, a blank line, then one line per [`CutSheetRow`] with fixed
    /// field labels (sequence, name, angle, indices, mast, meet). See this
    /// module's own tests for a worked example of the exact layout.
    ///
    /// Deterministic: identical input always produces a byte-identical
    /// string, matching this crate's own determinism requirement -- every
    /// field is either already a plain value or formatted with a fixed
    /// float precision, and rows are printed in `self.rows`' own order
    /// (never re-sorted).
    #[must_use]
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        for line in &self.header {
            out.push_str(line);
            out.push('\n');
        }
        if !self.header.is_empty() {
            out.push('\n');
        }
        for row in &self.rows {
            let indices = format_indices(&row.indices);
            let name = if row.name.is_empty() {
                "(unnamed)"
            } else {
                row.name.as_str()
            };
            // `write!` into a `String` is infallible; nothing to propagate.
            let _ = write!(
                out,
                "{:>3}. {name:<16} angle {:>7.2} deg (elevation {:>6.2} deg)  indices [{indices}]  \
                 mast {:>8.4}",
                row.sequence, row.angle_deg, row.angle_of_elevation_deg, row.mast
            );
            if let Some(depth_mm) = row.depth_mm {
                let _ = write!(out, "  depth {depth_mm:>6.2} mm");
            }
            let _ = write!(out, "  meet: {}", row.meet_instruction);
            if let Some(cheater) = row.cheater_offset_deg {
                let _ = write!(out, "  cheater: {cheater:+.2} deg");
            }
            out.push('\n');
        }
        out
    }
}

/// Formats a tier's whole index list for [`CuttingSheet::to_text`]: `"-"`
/// for an empty list (a single facet at azimuth 0), else each position
/// through [`format_index`] joined with `", "`.
pub(super) fn format_indices(indices: &[f64]) -> String {
    if indices.is_empty() {
        return "-".to_string();
    }
    indices
        .iter()
        .map(|&i| format_index(i))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Formats one index-wheel position: a whole tooth prints with no decimal
/// point (`"12"`, matching how a cutter reads an index-wheel scale), a
/// fractional position keeps two decimals (`"11.50"`).
fn format_index(index: f64) -> String {
    if (index - index.round()).abs() < 1e-9 {
        format!("{:.0}", index.round())
    } else {
        format!("{index:.2}")
    }
}
