//! [`CutSheetRow`] and [`CuttingSheet`]: the printable cutting-sheet data and
//! its [`CuttingSheet::to_text`] rendering. Built by [`super::build`]; the
//! shapes themselves carry no solver logic of their own.

use std::fmt::Write as _;

use crate::design::{ConcaveTier, ConcaveTool, ToolMotion};

/// Width of the name column in [`CuttingSheet::to_text`]. Hoisted so a concave
/// tier's second line can start its tool code in the same column by
/// construction instead of by a second hand-counted literal.
pub const SHEET_NAME_WIDTH: usize = 16;

/// Width of the leading "`{sequence:>3}. `" block of a text-sheet row; the
/// second line of a concave tier is indented by it so its tool code sits in the
/// name column.
const SEQUENCE_PREFIX_WIDTH: usize = 5;

/// Width of the θ column: formatted tool azimuth θ, right-aligned to match
/// the facet line's angle column.
const THETA_WIDTH: usize = 13;

/// Blank columns between the end of θ and the displacement, so the
/// displacement starts under the facet line's `indices` list.
/// Facet line prefix up to "indices [":
/// sequence (5) + name (16) + " angle " (7) + angle (6) + " deg  " (6) = 40.
/// Second line prefix before displacement:
/// indent (5) + code (16) + `THETA_WIDTH` (13) + `DISPLACEMENT_INDENT` (6) = 40.
const DISPLACEMENT_INDENT: usize = 6;

/// Width of the displacement column: `X = -0.250, Y = -0.250, Z = -0.250`, the
/// widest value it prints, so the details column is straight on every row.
const DISPLACEMENT_WIDTH: usize = 34;

/// What a concave tier adds to its [`CutSheetRow`]: the tool line's raw values,
/// for views that format them their own way.
///
/// The HTML sheet shows millimetres beside the ratio. The text and HTML sheets get their strings from
/// [`ConcaveRowInfo::second_line_fields`], which is
/// [`crate::design::ConcaveTier::second_line_fields`], so no output can
/// disagree with another.
#[derive(Debug, Clone, PartialEq)]
pub struct ConcaveRowInfo {
    /// Tool shape.
    pub tool: ConcaveTool,
    /// Tool azimuth θ, in degrees.
    pub tool_azimuth_deg: f64,
    /// Tool displacement `[x, y, z]`.
    pub displacement: [f64; 3],
    /// Tool diameter over stone width (D/W).
    pub diameter_ratio: f64,
    /// Included angle for cones and discs.
    pub tool_angle_deg: Option<f64>,
    /// How the tool moves while cutting.
    pub motion: ToolMotion,
}

impl ConcaveRowInfo {
    /// The tool line's four columns. Delegates to
    /// [`ConcaveTier::second_line_fields`] through a tier carrying only the
    /// tool's own fields (the formatter reads nothing else), rather than
    /// repeating its format strings here.
    #[must_use]
    pub fn second_line_fields(&self) -> [String; 4] {
        ConcaveTier {
            name: String::new(),
            angle_deg: 0.0,
            indices: Vec::new(),
            instructions: String::new(),
            tool: self.tool,
            tool_azimuth_deg: self.tool_azimuth_deg,
            displacement: self.displacement,
            diameter_ratio: self.diameter_ratio,
            tool_angle_deg: self.tool_angle_deg,
            motion: self.motion,
        }
        .second_line_fields()
    }
}

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
    /// `Some` for a concave tier, whose tool line follows the facet line;
    /// `None` for a flat tier. A concave row has `mast == 0.0` (it has no
    /// mast) and no depth, and its `meets_tiers` is empty.
    pub concave: Option<ConcaveRowInfo>,
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
                "{:>3}. {name:<SHEET_NAME_WIDTH$} angle {:>6.2} deg  indices [{indices}]  \
                 mast {:>8.4}",
                row.sequence,
                row.angle_deg.abs(),
                row.mast
            );
            if let Some(depth_mm) = row.depth_mm {
                let _ = write!(out, "  depth {depth_mm:>6.2} mm");
            }
            let _ = write!(out, "  meet: {}", row.meet_instruction);
            if let Some(cheater) = row.cheater_offset_deg {
                let _ = write!(out, "  cheater: {cheater:+.2} deg");
            }
            out.push('\n');
            if let Some(concave) = &row.concave {
                let [code, theta, displacement, details] = concave.second_line_fields();
                // Under the facet line's own columns: the tool code in the name
                // column, θ ending under φ, the displacement under the index
                // list. No sequence number: the pair is one tier.
                let _ = writeln!(
                    out,
                    "{:SEQUENCE_PREFIX_WIDTH$}{code:<SHEET_NAME_WIDTH$}{theta:>THETA_WIDTH$}\
                     {:DISPLACEMENT_INDENT$}{displacement:<DISPLACEMENT_WIDTH$}  {details}",
                    "", ""
                );
            }
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
pub fn format_index(index: f64) -> String {
    if (index - index.round()).abs() < 1e-9 {
        format!("{:.0}", index.round())
    } else {
        format!("{index:.2}")
    }
}
