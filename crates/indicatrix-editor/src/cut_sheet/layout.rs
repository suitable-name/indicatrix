//! [`SheetLayout`]: the blocks of a printed cutting sheet as plain data, in the order of the
//! owner's fantasy-cut template -- header, Facet Data, Size Data, Design Data, comments, then
//! a Pavilion and a Crown section of rows.
//!
//! The HTML sheet ([`super::html`]) and the plain-text sheet ([`SheetLayout::to_text`]) are
//! both written from this one value, so they cannot disagree about a count, a ratio or a
//! row. Building it is pure: no clock, no file, no dialog.

use super::details::SheetDetails;
use indicatrix::{
    geometry::meet_solver::{Block, SolvedTier, classify_blocks},
    optics::materials::GemMaterial,
};
use indicatrix_cut_core::{CutSheetRow, CuttingSheet, Design, design::TierRef};
use std::fmt::Write as _;

/// The heading a sheet gets when its design has no title.
pub const UNTITLED_HEADING: &str = "Cutting instructions";

/// The code of the table tier (see `Design::tier_codes`).
const TABLE_CODE: &str = "T";

/// How many tiers and facets one block of the stone has.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BlockCount {
    /// Tiers in the block.
    pub tiers: usize,
    /// Facets in the block: one per index of a tier, one for a tier with no index list.
    pub facets: usize,
}

/// The "Facet Data" block: facet and tier counts per block, and the totals.
///
/// A tier with an empty index list is one facet, otherwise it is one facet per index. A
/// concave tier counts its placements on its own side. The culet is a pavilion facet. The
/// table is counted apart, so the crown reads `N+1`: its facets plus the table.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FacetData {
    /// Pavilion tiers, including the culet and the concave tiers on the pavilion side.
    pub pavilion: BlockCount,
    /// Girdle tiers.
    pub girdle: BlockCount,
    /// Crown tiers without the table, including the concave tiers on the crown side.
    pub crown: BlockCount,
    /// The table: zero tiers and facets when the design has none.
    pub table: BlockCount,
}

impl FacetData {
    /// Counts the facets of `rows`, each paired with its block in `blocks`.
    #[must_use]
    pub fn count(rows: &[CutSheetRow], blocks: &[Block]) -> Self {
        let mut data = Self::default();
        for (row, &block) in rows.iter().zip(blocks) {
            let facets = row.indices.len().max(1);
            let is_table = block == Block::Crown && row.concave.is_none() && row.code == TABLE_CODE;
            let slot = if is_table {
                &mut data.table
            } else {
                match block {
                    Block::Pavilion => &mut data.pavilion,
                    Block::Girdle => &mut data.girdle,
                    Block::Crown => &mut data.crown,
                }
            };
            slot.tiers += 1;
            slot.facets += facets;
        }
        data
    }

    /// All tiers of the stone, the table included.
    #[must_use]
    pub const fn total_tiers(&self) -> usize {
        self.pavilion.tiers + self.girdle.tiers + self.crown.tiers + self.table.tiers
    }

    /// All facets of the stone, the table included.
    #[must_use]
    pub const fn total_facets(&self) -> usize {
        self.pavilion.facets + self.girdle.facets + self.crown.facets + self.table.facets
    }

    /// The crown's tier count as printed: `3`, or `3+1` when the design has a table.
    #[must_use]
    pub fn crown_tiers_text(&self) -> String {
        with_table(self.crown.tiers, self.table.tiers)
    }

    /// The crown's facet count as printed: `32`, or `32+1` when the design has a table.
    #[must_use]
    pub fn crown_facets_text(&self) -> String {
        with_table(self.crown.facets, self.table.facets)
    }
}

/// `count`, or `count+table` when there is a table.
fn with_table(count: usize, table: usize) -> String {
    if table == 0 {
        count.to_string()
    } else {
        format!("{count}+{table}")
    }
}

/// The "Size Data" block: the six ratios a cutter compares against a printed diagram.
///
/// All are over the stone width `W` (the smaller horizontal extent), measured on the same
/// solid the editor's status bar reads. `None` prints as `-`: the design does not measure,
/// or has no live girdle to take crown and pavilion from.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SizeData {
    /// Length over width.
    pub length_to_width: Option<f64>,
    /// Total height over width.
    pub height_to_width: Option<f64>,
    /// Volume over width cubed.
    pub volume_to_width_cubed: Option<f64>,
    /// Pavilion depth over width.
    pub pavilion_to_width: Option<f64>,
    /// Crown height over width.
    pub crown_to_width: Option<f64>,
    /// Pavilion depth over crown height, `(P/W) / (C/W)`.
    pub pavilion_to_crown: Option<f64>,
}

impl SizeData {
    /// Measures `design` from its `solved` masts. All `None` for a design with no tiers (a
    /// bare preform is not a stone), one that does not close, or a zero width.
    #[must_use]
    pub fn measure(design: &Design, solved: &[SolvedTier]) -> Self {
        if design.tiers.is_empty() {
            return Self::default();
        }
        let Some(metrics) = design.measure_from_solved_geom(solved).ok().flatten() else {
            return Self::default();
        };
        let width = metrics.width_axis;
        if width <= 1e-9 {
            return Self::default();
        }
        let pavilion = metrics.pavilion_depth.map(|depth| depth / width);
        let crown = metrics.crown_height.map(|height| height / width);
        Self {
            length_to_width: Some(metrics.length_axis / width),
            height_to_width: Some(metrics.total_height / width),
            volume_to_width_cubed: Some(metrics.volume / (width * width * width)),
            pavilion_to_width: pavilion,
            crown_to_width: crown,
            pavilion_to_crown: Option::zip(pavilion, crown)
                .filter(|&(_, crown)| crown > 1e-9)
                .map(|(pavilion, crown)| pavilion / crown),
        }
    }

    /// The six `(label, text)` cells in print order, three decimals, `-` when not measured.
    #[must_use]
    pub fn cells(&self) -> [(&'static str, String); 6] {
        let text =
            |value: Option<f64>| value.map_or_else(|| "-".to_string(), |v| format!("{v:.3}"));
        [
            ("L/W", text(self.length_to_width)),
            ("H/W", text(self.height_to_width)),
            ("V/W\u{b3}", text(self.volume_to_width_cubed)),
            ("P/W", text(self.pavilion_to_width)),
            ("C/W", text(self.crown_to_width)),
            ("P/C", text(self.pavilion_to_crown)),
        ]
    }
}

/// A number with at most two decimals and no trailing zeros (`6.5`, `12`).
fn trimmed(value: f64) -> String {
    let text = format!("{value:.2}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// The material a "RI" line names in brackets: the design's own material name, else the
/// nearest built-in species marked with a question mark (a guess, never a fact), else `None`.
///
/// Uses the same nearest-index guess as the editor's material badge, so a printed sheet and
/// the editor never disagree about what is fact and what is guess.
#[must_use]
pub(super) fn material_label(design: &Design, n_d: f64) -> Option<String> {
    if let Some(name) = design.material.name.as_deref() {
        return Some(name.to_string());
    }
    crate::material_lookup::material_for_refractive_index(n_d).map(|(name, _)| format!("{name}?"))
}

/// The Design Data rows as `(label, value)`: the RI range (else the design's own RI with its
/// material), the size range when given, symmetry, index gear and the shape when given.
fn design_data(design: &Design, n_d: f64, details: &SheetDetails) -> Vec<(String, String)> {
    let mut rows = Vec::new();
    if let Some((low, high)) = details.ri_range {
        rows.push(("RI range".to_string(), format!("{low:.2} - {high:.2}")));
    } else {
        let value = material_label(design, n_d).map_or_else(
            || format!("{n_d:.3}"),
            |material| format!("{n_d:.3} ({material})"),
        );
        rows.push(("RI".to_string(), value));
    }
    if let Some((small, large)) = details.size_range_mm {
        rows.push((
            "Size range".to_string(),
            format!("{} - {} mm", trimmed(small), trimmed(large)),
        ));
    }
    rows.push((
        "Symmetry".to_string(),
        format!(
            "{}-fold{}",
            design.meta.symmetry_order,
            if design.meta.mirror { ", mirror" } else { "" }
        ),
    ));
    rows.push((
        "Index gear".to_string(),
        format!("{} index", design.meta.gear_teeth_abs()),
    ));
    if !details.shape.trim().is_empty() {
        rows.push(("Shape".to_string(), details.shape.trim().to_string()));
    }
    rows
}

/// One [`Block`] per row of `design`'s cutting sheet, in the sheet's own row order.
///
/// The rows of every design, planar or concave, follow `cutting_order()`: a flat row takes
/// its tier's classification (the solver's own, by stored index), a concave row the side its
/// angle is on (a concave tier is never a girdle, its angle is strictly inside `(-90, 90)`).
#[must_use]
pub fn row_blocks(design: &Design) -> Vec<Block> {
    let blocks = classify_blocks(&design.meet_tier_inputs());
    design
        .cutting_order()
        .into_iter()
        .map(|tier_ref| match tier_ref {
            TierRef::Flat(i) => blocks.get(i).copied().unwrap_or(Block::Girdle),
            TierRef::Concave(i) => {
                if design.concave_tiers[i].is_crown_side() {
                    Block::Crown
                } else {
                    Block::Pavilion
                }
            }
        })
        .collect()
}

/// Every block of a printed cutting sheet, ready to be written as HTML or text.
#[derive(Debug, Clone)]
pub struct SheetLayout {
    /// The header words and the optional ranges, as supplied.
    pub details: SheetDetails,
    /// Facet Data.
    pub facets: FacetData,
    /// Size Data.
    pub sizes: SizeData,
    /// Design Data as `(label, value)`.
    pub design_data: Vec<(String, String)>,
    /// The girdle diameter and carat estimate lines printed after Design Data, as
    /// `(label, value)`; empty when the design has neither.
    pub extras: Vec<(String, String)>,
    /// The cutting sheet: every row in cutting order.
    pub sheet: CuttingSheet,
    /// The block of each row of [`Self::sheet`].
    pub blocks: Vec<Block>,
    /// Millimetres per mast unit, when a girdle diameter anchors a scale.
    pub mm_per_unit: Option<f64>,
    /// The design's girdle diameter in millimetres, when set.
    pub girdle_diameter_mm: Option<f64>,
}

impl SheetLayout {
    /// Builds the layout of `design` from its solved masts.
    ///
    /// `custom` is the caller's own resolved catalogue materials, consulted for the printed
    /// refractive index exactly as [`Design::cutting_sheet_with`] does.
    ///
    /// # Panics
    ///
    /// Same alignment contract as [`Design::cutting_sheet`]: `solved` must have one entry per
    /// tier `design` currently has, in the same order.
    #[must_use]
    pub fn build(
        design: &Design,
        solved: &[SolvedTier],
        custom: &[GemMaterial],
        details: &SheetDetails,
    ) -> Self {
        let sheet = design.cutting_sheet_with(solved, custom);
        let blocks = row_blocks(design);
        let facets = FacetData::count(&sheet.rows, &blocks);
        let yield_report = design.yield_report(solved);
        let n_d = design.effective_refractive_index_with(custom);

        let mut extras = Vec::new();
        if let Some(mm) = design.girdle_diameter_mm {
            extras.push(("Girdle diameter".to_string(), format!("{mm:.3} mm")));
        }
        // The same understated-weight caveat the text header carries, shown next to the
        // number rather than left to a doc comment nobody printing a sheet reads.
        if let Some(carat) = yield_report.carat_weight {
            extras.push((
                "Carat weight (estimate)".to_string(),
                format!(
                    "{carat:.3} ct -- assumes the authored facets reach the preform's own walls"
                ),
            ));
        }

        Self {
            details: details.clone(),
            facets,
            sizes: SizeData::measure(design, solved),
            design_data: design_data(design, n_d, details),
            extras,
            sheet,
            blocks,
            mm_per_unit: yield_report.mm_per_unit,
            girdle_diameter_mm: design.girdle_diameter_mm,
        }
    }

    /// The sheet's heading: the title, else [`UNTITLED_HEADING`].
    #[must_use]
    pub fn heading(&self) -> &str {
        let title = self.details.title.trim();
        if title.is_empty() {
            UNTITLED_HEADING
        } else {
            title
        }
    }

    /// The rows of the Pavilion section, in cutting order: pavilion tiers and the girdle.
    #[must_use]
    pub fn pavilion_rows(&self) -> Vec<(&CutSheetRow, Block)> {
        self.section_rows(|block| block != Block::Crown)
    }

    /// The rows of the Crown section, in cutting order, ending with the table.
    #[must_use]
    pub fn crown_rows(&self) -> Vec<(&CutSheetRow, Block)> {
        self.section_rows(|block| block == Block::Crown)
    }

    fn section_rows(&self, keep: impl Fn(Block) -> bool) -> Vec<(&CutSheetRow, Block)> {
        self.sheet
            .rows
            .iter()
            .zip(self.blocks.iter().copied())
            .filter(|&(_, block)| keep(block))
            .collect()
    }

    /// Whether any row carries a cheater/azimuth offset (then the table gets its column).
    #[must_use]
    pub fn has_cheater_column(&self) -> bool {
        self.sheet
            .rows
            .iter()
            .any(|row| row.cheater_offset_deg.is_some())
    }

    /// The sheet as plain text: the same blocks as the HTML sheet, with plain headings. The
    /// four diagram views are pictures and appear only on the web page.
    #[must_use]
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "{}", self.heading());
        let details = &self.details;
        if !details.subtitle.trim().is_empty() {
            let _ = writeln!(out, "{}", details.subtitle.trim());
        }
        if !details.author.trim().is_empty() {
            let _ = writeln!(out, "by {}", details.author.trim());
        }
        if !details.date_text.trim().is_empty() {
            let _ = writeln!(out, "{}", details.date_text.trim());
        }

        out.push_str("\nFacet Data\n");
        for (name, tiers, facets) in [
            (
                "Pavilion",
                self.facets.pavilion.tiers.to_string(),
                self.facets.pavilion.facets.to_string(),
            ),
            (
                "Girdle",
                self.facets.girdle.tiers.to_string(),
                self.facets.girdle.facets.to_string(),
            ),
            (
                "Crown",
                self.facets.crown_tiers_text(),
                self.facets.crown_facets_text(),
            ),
            (
                "Total",
                self.facets.total_tiers().to_string(),
                self.facets.total_facets().to_string(),
            ),
        ] {
            let _ = writeln!(out, "  {name:<9} tiers {tiers:<5} facets {facets}");
        }

        out.push_str("\nSize Data\n  ");
        let cells: Vec<String> = self
            .sizes
            .cells()
            .into_iter()
            .map(|(label, value)| format!("{label} {value}"))
            .collect();
        out.push_str(&cells.join("   "));
        out.push('\n');

        out.push_str("\nDesign Data\n");
        for (label, value) in self.design_data.iter().chain(&self.extras) {
            let _ = writeln!(out, "  {label}: {value}");
        }

        if !details.comments.is_empty() {
            out.push_str("\nComments\n");
            for comment in &details.comments {
                let _ = writeln!(out, "  {comment}");
            }
        }

        for (heading, rows) in [
            ("Pavilion", self.pavilion_rows()),
            ("Crown", self.crown_rows()),
        ] {
            if rows.is_empty() {
                continue;
            }
            let _ = writeln!(out, "\n{heading}");
            for (row, _) in rows {
                row.write_text(&mut out);
            }
        }
        out
    }
}
