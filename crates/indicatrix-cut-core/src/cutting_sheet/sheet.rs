//! [`CutSheetRow`] and [`CuttingSheet`]: the printable cutting-sheet data and
//! its [`CuttingSheet::to_text`] rendering. Built by [`super::build`]; the
//! shapes themselves carry no solver logic of their own.

use std::fmt::Write as _;

use crate::design::{ConcaveTier, ConcaveTool, ToolMotion, is_legacy_123_abc};

/// Width of the label column in [`CuttingSheet::to_text`] (the tier code).
///
/// Hoisted so a concave tier's second line can start its tool code in the same column by
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
    /// 1-based position in the cutting sequence, in [`crate::design::Design::cutting_order`]
    /// -- the one order the Cut slider, Cutting mode and "Build this design" follow too.
    pub sequence: usize,
    /// The tier's code in cutting order (`P1`, `G1`, `C1`, `T`, `Culet`, or a concave
    /// tier's `P4`; see [`crate::design::Design::tier_codes`]) -- the label a cutter reads
    /// in the sheet's label column. Empty only for a row built by hand.
    pub code: String,
    /// The tier's own name(s), verbatim (see [`crate::design::ConstraintTier::name`]).
    /// Empty for an unnamed tier. A sheet shows it in front of the instruction text (see
    /// [`Self::descriptive_name`]), not in the label column.
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
    /// printed verbatim, else synthesizes text from its current `MeetConstraint`
    /// (`Meet P1, P2, G1`, each name written as its target tier's code) -- never
    /// empty, unlike the raw `.asc` `G` field, since a blank entry on a
    /// printed sheet reads as "nothing recorded" rather than the actual
    /// default a cutter assumes ("meets whatever the solver finds").
    ///
    /// Without the tier's name: [`Self::instruction`] puts that in front.
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
    /// mast; [`CuttingSheet::to_text`] prints a dash in the mast column for it, never
    /// the placeholder) and no depth, and its `meets_tiers` is empty.
    pub concave: Option<ConcaveRowInfo>,
}

impl CutSheetRow {
    /// What a sheet prints in its label column: the code, else (for a row built by hand) the
    /// name, else `(unnamed)`.
    #[must_use]
    pub fn label(&self) -> &str {
        if !self.code.is_empty() {
            &self.code
        } else if !self.name.trim().is_empty() {
            self.name.trim()
        } else {
            "(unnamed)"
        }
    }

    /// The tier's own descriptive name, when it has one worth printing: anything that is not
    /// empty, not an old-style name (`1`, `A`, `G`, see [`is_legacy_123_abc`]) and not the
    /// row's own code (compared without regard to case).
    #[must_use]
    pub fn descriptive_name(&self) -> Option<&str> {
        let name = self.name.trim();
        (!name.is_empty() && !is_legacy_123_abc(name) && !name.eq_ignore_ascii_case(&self.code))
            .then_some(name)
    }

    /// The instruction text of the row: [`Self::meet_instruction`], with the tier's
    /// descriptive name in front when it has one (`Crown Main: Meet P1, P2`). The text and
    /// HTML sheets and Cutting mode all print this.
    #[must_use]
    pub fn instruction(&self) -> String {
        self.descriptive_name().map_or_else(
            || self.meet_instruction.clone(),
            |name| format!("{name}: {}", self.meet_instruction),
        )
    }
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
    /// field labels (sequence, code, angle, indices, mast, meet). The code is the tier's
    /// label (`P1`, `T`, ...), the indices are dash-separated ([`format_sheet_indices`]) and
    /// the meet text starts with the tier's descriptive name ([`CutSheetRow::instruction`]).
    /// See this module's own tests for a worked example of the exact layout.
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
            row.write_text(&mut out);
        }
        out
    }
}

impl CutSheetRow {
    /// Appends this row's text-sheet lines to `out`: the facet line, then (for a concave tier)
    /// its tool line, each ending in a line feed. [`CuttingSheet::to_text`] is these lines for
    /// every row in order.
    pub fn write_text(&self, out: &mut String) {
        let indices = format_sheet_indices(&self.indices);
        let label = self.label();
        // A concave tier has no mast; printing its placeholder `0.0` as `0.0000` would
        // read as "cut to depth zero". The column shows a dash, as the tier table does.
        let mast = if self.concave.is_some() {
            "-".to_string()
        } else {
            format!("{:.4}", self.mast)
        };
        // `write!` into a `String` is infallible; nothing to propagate.
        let _ = write!(
            out,
            "{:>3}. {label:<SHEET_NAME_WIDTH$} angle {:>6.2} deg  indices [{indices}]  \
             mast {mast:>8}",
            self.sequence,
            self.angle_deg.abs(),
        );
        if let Some(depth_mm) = self.depth_mm {
            let _ = write!(out, "  depth {depth_mm:>6.2} mm");
        }
        let _ = write!(out, "  meet: {}", self.instruction());
        if let Some(cheater) = self.cheater_offset_deg {
            let _ = write!(out, "  cheater: {cheater:+.2} deg");
        }
        out.push('\n');
        if let Some(concave) = &self.concave {
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
}

/// Formats a tier's whole index list for the printed sheets (the text sheet and the HTML sheet).
///
/// `"-"` for an empty list (a single facet at azimuth 0), else each position through
/// [`format_sheet_index`] joined with `"-"`, as the cutting-instructions template writes
/// them: `96-08-16-24`.
#[must_use]
pub fn format_sheet_indices(indices: &[f64]) -> String {
    if indices.is_empty() {
        return "-".to_string();
    }
    indices
        .iter()
        .map(|&i| format_sheet_index(i))
        .collect::<Vec<_>>()
        .join("-")
}

/// Formats one index-wheel position for the printed sheets.
///
/// A whole tooth is zero-padded to two digits (`"08"`, `"96"`; a three-digit tooth stays
/// `"120"`), a fractional position keeps two decimals with its whole part padded the same way
/// (`"03.50"`, `"11.50"`).
#[must_use]
pub fn format_sheet_index(index: f64) -> String {
    if (index - index.round()).abs() < 1e-9 {
        // Through an integer so a `-0.0` prints as `00`, not `-0`.
        format!("{:02}", index.round() as i64)
    } else {
        format!("{index:05.2}")
    }
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
