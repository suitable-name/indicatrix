//! Builds a design's printable cutting sheet as one self-contained HTML string --
//! the header meta table, an embedded `data:` PNG of its 2D faceting diagram, and one
//! table row per tier in cutting order.

use super::diagram::DiagramImage;
use indicatrix::{
    geometry::meet_solver::{Block, SolvedTier, classify_blocks},
    optics::materials::GemMaterial,
};
use indicatrix_cut_core::{CutSheetRow, Design};
use std::fmt::Write as _;

/// The standard (RFC 4648) base64 alphabet, padded -- this crate's own encoder
/// rather than a new dependency (`base64` is not a workspace dependency
/// anywhere in this repo, and the whole reason for choosing HTML over a PDF
/// library for the cutting sheet was avoiding one). A `data:` URI is the only
/// place this codebase needs base64 at all, so a ~20-line hand-rolled encoder
/// is simpler than justifying a new crate for it.
const BASE64_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Encodes `data` as base64 with `=` padding, per RFC 4648 -- see
/// [`BASE64_ALPHABET`]'s own doc comment for why this is hand-rolled.
///
/// `pub(super)` so this module's own tests (`cut_sheet::tests`) can check the
/// encoder directly against known vectors.
#[must_use]
pub(super) fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = chunk.get(1).copied().map_or(0, u32::from);
        let b2 = chunk.get(2).copied().map_or(0, u32::from);
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(BASE64_ALPHABET[((n >> 18) & 0x3f) as usize] as char);
        out.push(BASE64_ALPHABET[((n >> 12) & 0x3f) as usize] as char);
        out.push(if chunk.len() > 1 {
            BASE64_ALPHABET[((n >> 6) & 0x3f) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            BASE64_ALPHABET[(n & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// Builds `image`'s `data:` URI -- what keeps [`cutting_sheet_html`]'s output a
/// single self-contained file, rather than an HTML file plus a sibling PNG
/// that a cutter could separate or lose.
fn png_data_uri(image: &DiagramImage) -> String {
    format!("data:image/png;base64,{}", base64_encode(&image.png_bytes))
}

/// Escapes `s` for safe interpolation into HTML text/attribute content -- a
/// tier name or meet note is free text a cutter authored (or a `.asc` file's own
/// `G`-field text, verbatim -- see `CutSheetRow::meet_instruction`'s doc
/// comment), never assumed free of `&`/`<`/`>`/quote characters.
///
/// `pub(super)` so this module's own tests can check the escaping directly.
pub(super) fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// [`Block`]'s printable label -- the Crown/Pavilion/Girdle column text.
const fn block_label(block: Block) -> &'static str {
    match block {
        Block::Crown => "Crown",
        Block::Pavilion => "Pavilion",
        Block::Girdle => "Girdle",
    }
}

/// [`Block`]'s CSS class suffix (`"block-crown"`/`"block-pavilion"`/
/// `"block-girdle"`), lowercase to match this module's own stylesheet.
const fn block_css_class(block: Block) -> &'static str {
    match block {
        Block::Crown => "block-crown",
        Block::Pavilion => "block-pavilion",
        Block::Girdle => "block-girdle",
    }
}

/// Formats one tier's whole index list for the printed sheet: `"-"` for an
/// empty list (a single facet at azimuth 0), else each position through
/// [`format_index`] joined with `", "` -- mirrors
/// `indicatrix_cut_core::cutting_sheet`'s own private formatter exactly (same
/// whole-tooth-vs-fractional convention), duplicated here rather than exposed
/// from that crate purely for the print layout to own its own text formatting.
fn format_indices_html(indices: &[f64]) -> String {
    if indices.is_empty() {
        return "-".to_string();
    }
    indices
        .iter()
        .map(|&i| format_index(i))
        .collect::<Vec<_>>()
        .join(", ")
}

/// See [`format_indices_html`].
fn format_index(index: f64) -> String {
    if (index - index.round()).abs() < 1e-9 {
        format!("{:.0}", index.round())
    } else {
        format!("{index:.2}")
    }
}

/// Builds `design`'s printable cutting sheet as a single self-contained HTML string.
///
/// A header block (material, refractive index, index gear/symmetry, girdle diameter when
/// set), the 2D diagram embedded as a `data:` PNG when `diagram` is `Some` (build one via
/// [`super::diagram::render_cut_diagram`]; `None` prints a plain-text notice instead of a
/// broken image), then one table row per tier in cutting order -- step number, name,
/// Crown/Pavilion/Girdle label, signed angle, index list, mast (model units, plus mm when
/// [`Design::girdle_diameter_mm`] is set), and meet note.
///
/// Pure: same `design`/`solved`/`diagram`/`custom` in always produces the same
/// string out, no file/dialog/clock access -- see this module's own doc comment.
///
/// `custom` -- a caller's own resolved catalogue materials, e.g.
/// `RenderContext::custom_materials` -- is consulted for the printed "Refractive
/// index" line exactly like [`Design::cutting_sheet_with`]/
/// [`Design::effective_refractive_index_with`], so a design on a CUSTOM catalogue
/// material (not one of the built-ins) prints that material's own `n_D` rather than
/// the legacy schedule RI. Pass `&[]` for a caller with no catalogue on hand (same
/// built-ins-only behaviour as `Design::cutting_sheet`/`effective_refractive_index`).
///
/// # Panics
///
/// Same alignment contract as [`Design::cutting_sheet`]: `solved` must have one
/// entry per tier `design` currently has, in the same order.
#[must_use]
pub fn cutting_sheet_html(
    design: &Design,
    solved: &[SolvedTier],
    diagram: Option<&DiagramImage>,
    custom: &[GemMaterial],
) -> String {
    let sheet = design.cutting_sheet_with(solved, custom);
    let blocks = classify_blocks(&design.meet_tier_inputs());
    let mm_per_unit = design.yield_report(solved).mm_per_unit;
    let show_mm_column = design.girdle_diameter_mm.is_some();
    // Shows the cheater-offset column only when some tier actually carries one --
    // an ordinary design's sheet should not gain a column of dashes for a feature
    // it does not use.
    let show_cheater_column = sheet
        .rows
        .iter()
        .any(|row| row.cheater_offset_deg.is_some());

    let mut out = String::new();
    out.push_str(HTML_HEAD);
    out.push_str("<h1>Cutting Sheet</h1>\n");
    push_header_block(&mut out, design, custom);
    push_carat_weight_row(&mut out, design, solved);
    push_diagram_block(&mut out, diagram);
    push_tier_table(
        &mut out,
        &sheet.rows,
        &blocks,
        TierTableColumns {
            show_mm: show_mm_column,
            show_cheater: show_cheater_column,
        },
        mm_per_unit,
    );
    out.push_str(HTML_TAIL);
    out
}

/// [`cutting_sheet_html`]'s header meta table: material, refractive index,
/// index gear/symmetry, and girdle diameter when set. `custom` -- see
/// [`cutting_sheet_html`]'s own doc comment -- decides which RI the "Refractive
/// index" row prints for a CUSTOM catalogue material.
fn push_header_block(out: &mut String, design: &Design, custom: &[GemMaterial]) {
    out.push_str("<table class=\"meta\">\n");
    let n_d = design.effective_refractive_index_with(custom);
    push_meta_row(out, "Material", &material_header_text(design, n_d));
    push_meta_row(out, "Refractive index", &format!("{n_d:.4}"));
    push_meta_row(
        out,
        "Index gear",
        &format!(
            "{} teeth, symmetry {}{}",
            design.meta.gear_teeth_abs(),
            design.meta.symmetry_order,
            if design.meta.mirror { ", mirrored" } else { "" }
        ),
    );
    if let Some(mm) = design.girdle_diameter_mm {
        push_meta_row(out, "Girdle diameter", &format!("{mm:.3} mm"));
    }
    out.push_str("</table>\n");
}

/// [`cutting_sheet_html`]'s carat-weight line, when `solved` produces one --
/// `None` under the same conditions
/// [`indicatrix_cut_core::YieldReport::carat_weight`] itself is `None` (no
/// girdle diameter set, or the design does not currently measure). Printed with
/// the same understated-weight caveat
/// [`indicatrix_cut_core::design::Design::cutting_sheet`]'s own header line
/// carries: shown inline next to the number, not left to a doc comment nobody
/// printing a sheet reads.
fn push_carat_weight_row(out: &mut String, design: &Design, solved: &[SolvedTier]) {
    let Some(carat) = design.yield_report(solved).carat_weight else {
        return;
    };
    out.push_str("<table class=\"meta\">\n");
    push_meta_row(
        out,
        "Carat weight (estimate)",
        &format!("{carat:.3} ct -- assumes the authored facets reach the preform's own walls"),
    );
    out.push_str("</table>\n");
}

/// The cut sheet header's "Material" row text -- `design.material.name` when
/// set, else the SAME nearest-n_D guess the editor's own material-guess badge
/// shows. Formatted identically ("Sapphire? (from RI 1.76)") so a printed sheet
/// and the editor never disagree about what is fact versus guess. "(unset)" when
/// nothing built in is close enough to guess at all. Unlike the editor's badge,
/// which shows nothing, a printed sheet always needs SOME text in this
/// cell.
///
/// `pub(super)` so this module's own tests can check the wording directly.
pub(super) fn material_header_text(design: &Design, n_d: f64) -> String {
    if let Some(name) = design.material.name.as_deref() {
        return name.to_string();
    }
    crate::material_lookup::material_for_refractive_index(n_d).map_or_else(
        || "(unset)".to_string(),
        |(name, _)| format!("{name}? (from RI {n_d:.2})"),
    )
}

/// Pushes one `<tr>` of [`push_header_block`]'s meta table -- `label` bold in
/// its own cell, `value` (already caller-formatted, not caller-escaped --
/// every call site here passes either a fixed string or a `{:.N}`-formatted
/// number, never free text) in the next.
fn push_meta_row(out: &mut String, label: &str, value: &str) {
    let _ = writeln!(
        out,
        "<tr><td class=\"label\">{label}</td><td>{value}</td></tr>"
    );
}

/// [`cutting_sheet_html`]'s diagram section: the embedded `data:` PNG when
/// `diagram` is `Some`, else the "not a closed solid" notice.
fn push_diagram_block(out: &mut String, diagram: Option<&DiagramImage>) {
    match diagram {
        Some(image) => {
            let _ = writeln!(
                out,
                "<div class=\"diagram\"><img src=\"{}\" width=\"{}\" height=\"{}\" \
                 alt=\"2D faceting diagram\"></div>",
                png_data_uri(image),
                image.width,
                image.height
            );
        }
        None => out.push_str(
            "<p class=\"diagram-missing\">Diagram unavailable: this design does not \
             currently close to a solid.</p>\n",
        ),
    }
}

/// Which optional columns this sheet carries. Both are decided once from the whole
/// schedule rather than per row, so the header and every row agree by construction.
#[derive(Clone, Copy)]
struct TierTableColumns {
    /// The mast repeated in millimetres -- only when a girdle diameter anchors a
    /// real scale.
    show_mm: bool,
    /// The per-tier cheater/azimuth offset -- only when some tier has one.
    show_cheater: bool,
}

/// [`cutting_sheet_html`]'s tier table: header row (the "Mast (mm)" column
/// only when `columns` says so), then one `<tr>` per `rows`/`blocks` pair --
/// see [`push_tier_row`].
fn push_tier_table(
    out: &mut String,
    rows: &[CutSheetRow],
    blocks: &[Block],
    columns: TierTableColumns,
    mm_per_unit: Option<f64>,
) {
    out.push_str("<table class=\"tiers\">\n<thead><tr>");
    out.push_str(
        "<th>#</th><th>Tier</th><th>Block</th><th>Angle</th><th>Elevation</th><th>Indices</th>\
         <th>Mast</th>",
    );
    if columns.show_mm {
        out.push_str("<th>Mast (mm)</th>");
    }
    if columns.show_cheater {
        out.push_str("<th>Cheater</th>");
    }
    out.push_str("<th>Meet</th></tr></thead>\n<tbody>\n");
    for (row, &block) in rows.iter().zip(blocks) {
        push_tier_row(out, row, block, columns, mm_per_unit);
    }
    out.push_str("</tbody>\n</table>\n");
}

/// One `<tr>` of [`push_tier_table`]: step number, tier name, Crown/Pavilion/
/// Girdle label (from `block`, never the sign of `row.angle_deg` -- see this
/// module's own doc comment), signed angle, index list, mast in model units,
/// mast in mm (only when `columns.show_mm`; `"-"` when `mm_per_unit` itself
/// could not be resolved), and the meet note.
fn push_tier_row(
    out: &mut String,
    row: &CutSheetRow,
    block: Block,
    columns: TierTableColumns,
    mm_per_unit: Option<f64>,
) {
    let name = if row.name.is_empty() {
        "(unnamed)"
    } else {
        row.name.as_str()
    };
    out.push_str("<tr>");
    let _ = write!(out, "<td>{}</td>", row.sequence);
    let _ = write!(out, "<td>{}</td>", html_escape(name));
    let _ = write!(
        out,
        "<td class=\"{}\">{}</td>",
        block_css_class(block),
        block_label(block)
    );
    let _ = write!(out, "<td>{:+.2}&deg;</td>", row.angle_deg);
    let _ = write!(out, "<td>{:.2}&deg;</td>", row.angle_of_elevation_deg);
    let _ = write!(out, "<td>{}</td>", format_indices_html(&row.indices));
    let _ = write!(out, "<td>{:.4}</td>", row.mast);
    if columns.show_mm {
        let cell = mm_per_unit.map_or_else(
            || "-".to_string(),
            |scale| format!("{:.4}", row.mast * scale),
        );
        let _ = write!(out, "<td>{cell}</td>");
    }
    if columns.show_cheater {
        // A tier with no recorded offset reads "-", not "+0.00", so "deliberately
        // on-tooth" is distinguishable from "nobody set one".
        let cell = row
            .cheater_offset_deg
            .map_or_else(|| "-".to_string(), |deg| format!("{deg:+.2}&deg;"));
        let _ = write!(out, "<td>{cell}</td>");
    }
    let _ = write!(out, "<td>{}</td>", html_escape(&row.meet_instruction));
    out.push_str("</tr>\n");
}

/// [`cutting_sheet_html`]'s opening half: `<!DOCTYPE html>` through the print
/// stylesheet and `<body>`. A print stylesheet that actually works on paper --
/// a sane page size, no dark background burning ink on a real printer, and a
/// tier's row never splitting across a page break (`page-break-inside: avoid`
/// plus its modern `break-inside` alias, since printer/browser support for the
/// two still varies).
const HTML_HEAD: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>Cutting Sheet</title>
<style>
  :root { color-scheme: light; }
  * { box-sizing: border-box; }
  body {
    font-family: Arial, Helvetica, sans-serif;
    background: #ffffff;
    color: #111111;
    margin: 24px;
  }
  h1 { font-size: 20px; margin: 0 0 12px; }
  table.meta { border-collapse: collapse; margin-bottom: 16px; }
  table.meta td { padding: 2px 12px 2px 0; font-size: 13px; vertical-align: top; }
  table.meta td.label { font-weight: 700; color: #333333; white-space: nowrap; }
  .diagram { text-align: center; margin: 8px 0 20px; }
  .diagram img { max-width: 100%; height: auto; }
  .diagram-missing { color: #b91c1c; font-style: italic; }
  table.tiers { border-collapse: collapse; width: 100%; font-size: 12px; }
  table.tiers th, table.tiers td {
    border: 1px solid #999999;
    padding: 4px 6px;
    text-align: left;
  }
  table.tiers thead { background: #eeeeee; }
  table.tiers tr { page-break-inside: avoid; break-inside: avoid; }
  td.block-crown { color: #92400e; font-weight: 600; }
  td.block-pavilion { color: #1e3a8a; font-weight: 600; }
  td.block-girdle { color: #444444; font-weight: 600; }
  @page { size: A4; margin: 14mm; }
  @media print {
    body { margin: 0; background: #ffffff; }
    table.tiers thead { display: table-header-group; }
    .diagram { break-inside: avoid; page-break-inside: avoid; }
  }
</style>
</head>
<body>
"#;

/// [`cutting_sheet_html`]'s closing half.
const HTML_TAIL: &str = "</body>\n</html>\n";
