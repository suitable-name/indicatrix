//! Builds a design's printable cutting instructions as one self-contained HTML string,
//! laid out like the owner's fantasy-cut template: a header, Facet Data, Size Data, Design
//! Data, four views, comments, then a Pavilion and a Crown section of rows in cutting order.
//!
//! Every number and row comes from [`SheetLayout`]; this module only writes it as HTML.

use super::{
    details::SheetDetails,
    diagram::{CutViews, DiagramImage},
    layout::SheetLayout,
};
use indicatrix::{
    geometry::meet_solver::{Block, SolvedTier},
    optics::materials::GemMaterial,
};
use indicatrix_cut_core::{ConcaveRowInfo, CutSheetRow, Design, format_sheet_indices};
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

/// What the Views block shows.
#[derive(Clone, Copy)]
enum ViewsBlock<'a> {
    /// The design does not close to a solid: a plain-text notice instead of pictures.
    Missing,
    /// One picture, the legacy three-panel diagram ([`cutting_sheet_html`]).
    Single(&'a DiagramImage),
    /// The four labelled views of the template.
    Four(&'a CutViews),
}

/// Builds `design`'s printable cutting instructions as a single self-contained HTML string,
/// with the header words from [`SheetDetails::from_design`].
///
/// `diagram` is one already-rendered picture (build one via
/// [`super::diagram::render_cut_diagram`]); it is printed in the Views block, and `None`
/// prints a plain-text notice instead of a broken image. The four labelled views of the
/// template come from [`cutting_sheet_html_with`].
///
/// Pure: same `design`/`solved`/`diagram`/`custom` in always produces the same
/// string out, no file/dialog/clock access -- see this module's own doc comment.
///
/// `custom` -- a caller's own resolved catalogue materials, e.g.
/// `RenderContext::custom_materials` -- is consulted for the printed "RI" line exactly like
/// [`Design::cutting_sheet_with`]/[`Design::effective_refractive_index_with`], so a design on
/// a CUSTOM catalogue material (not one of the built-ins) prints that material's own `n_D`
/// rather than the legacy schedule RI. Pass `&[]` for a caller with no catalogue on hand.
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
    let layout = SheetLayout::build(design, solved, custom, &SheetDetails::from_design(design));
    build_html(
        &layout,
        diagram.map_or(ViewsBlock::Missing, ViewsBlock::Single),
    )
}

/// Builds `design`'s printable cutting instructions with explicit header `details` and the
/// four labelled `views` (`None` prints a notice: the design does not close to a solid).
///
/// Top to bottom: header (title, subtitle, "by" author, date), Facet Data, Size Data, Design
/// Data, Views, Comments, a Pavilion section and a Crown section. Each section is a table of
/// step number, label (the tier's code), angle, index list, instruction, mast (model units,
/// plus mm when [`Design::girdle_diameter_mm`] is set) and the cheater offset when some tier
/// has one. The girdle rows are in the Pavilion section; the table is the last row of the
/// Crown section.
///
/// # Panics
///
/// Same alignment contract as [`cutting_sheet_html`].
#[must_use]
pub fn cutting_sheet_html_with(
    design: &Design,
    solved: &[SolvedTier],
    views: Option<&CutViews>,
    custom: &[GemMaterial],
    details: &SheetDetails,
) -> String {
    let layout = SheetLayout::build(design, solved, custom, details);
    build_html(&layout, views.map_or(ViewsBlock::Missing, ViewsBlock::Four))
}

/// The whole page for `layout`.
fn build_html(layout: &SheetLayout, views: ViewsBlock<'_>) -> String {
    let has_concave = layout.sheet.rows.iter().any(|row| row.concave.is_some());
    let mut out = HTML_HEAD.replacen("{TITLE}", &html_escape(layout.heading()), 1);
    if has_concave {
        // Only a design with concave tiers gains the banding rules, so every planar
        // sheet stays free of them.
        out = out.replacen("</style>", CONCAVE_STYLE, 1);
    }
    push_header(&mut out, layout);
    push_facet_data(&mut out, layout);
    push_size_data(&mut out, layout);
    push_design_data(&mut out, layout);
    push_views(&mut out, views);
    push_comments(&mut out, layout);

    let columns = TierTableColumns {
        show_mm: layout.girdle_diameter_mm.is_some(),
        show_cheater: layout.has_cheater_column(),
        banded: has_concave,
    };
    for (heading, rows) in [
        ("Pavilion", layout.pavilion_rows()),
        ("Crown", layout.crown_rows()),
    ] {
        push_section(&mut out, heading, &rows, columns, layout);
    }
    out.push_str(HTML_TAIL);
    out
}

/// The heading block: the title (or "Cutting instructions"), then subtitle, "by" author and
/// date, each only when present.
fn push_header(out: &mut String, layout: &SheetLayout) {
    let _ = writeln!(out, "<h1>{}</h1>", html_escape(layout.heading()));
    let details = &layout.details;
    for (class, text) in [
        ("subtitle", details.subtitle.trim().to_string()),
        (
            "byline",
            if details.author.trim().is_empty() {
                String::new()
            } else {
                format!("by {}", details.author.trim())
            },
        ),
        ("date", details.date_text.trim().to_string()),
    ] {
        if !text.is_empty() {
            let _ = writeln!(out, "<p class=\"{class}\">{}</p>", html_escape(&text));
        }
    }
}

/// Facet Data: tiers and facets per block, the crown as `N+1` when there is a table, totals.
fn push_facet_data(out: &mut String, layout: &SheetLayout) {
    let facets = &layout.facets;
    out.push_str("<h2>Facet Data</h2>\n<table class=\"data facets\">\n");
    out.push_str("<tr><th></th><th>Tiers</th><th>Facets</th></tr>\n");
    for (name, tiers, count) in [
        (
            "Pavilion",
            facets.pavilion.tiers.to_string(),
            facets.pavilion.facets.to_string(),
        ),
        (
            "Girdle",
            facets.girdle.tiers.to_string(),
            facets.girdle.facets.to_string(),
        ),
        (
            "Crown",
            facets.crown_tiers_text(),
            facets.crown_facets_text(),
        ),
        (
            "Total",
            facets.total_tiers().to_string(),
            facets.total_facets().to_string(),
        ),
    ] {
        let _ = writeln!(
            out,
            "<tr><td class=\"label\">{name}</td><td>{tiers}</td><td>{count}</td></tr>"
        );
    }
    out.push_str("</table>\n");
}

/// Size Data: the six ratios, one header cell and one value cell each.
fn push_size_data(out: &mut String, layout: &SheetLayout) {
    out.push_str("<h2>Size Data</h2>\n<table class=\"data sizes\">\n<tr>");
    let cells = layout.sizes.cells();
    for (label, _) in &cells {
        let _ = write!(out, "<th>{label}</th>");
    }
    out.push_str("</tr>\n<tr>");
    for (_, value) in &cells {
        let _ = write!(out, "<td>{value}</td>");
    }
    out.push_str("</tr>\n</table>\n");
}

/// Design Data, then the girdle diameter and carat estimate lines.
fn push_design_data(out: &mut String, layout: &SheetLayout) {
    out.push_str("<h2>Design Data</h2>\n<table class=\"meta\">\n");
    for (label, value) in layout.design_data.iter().chain(&layout.extras) {
        push_meta_row(out, label, value);
    }
    out.push_str("</table>\n");
}

/// Pushes one `<tr>` of a meta table: `label` bold in its own cell, `value` in the next.
/// Both are escaped, since the shape and material text are free text.
fn push_meta_row(out: &mut String, label: &str, value: &str) {
    let _ = writeln!(
        out,
        "<tr><td class=\"label\">{}</td><td>{}</td></tr>",
        html_escape(label),
        html_escape(value)
    );
}

/// The Views block: four labelled pictures in a 2 x 2 grid, the legacy single picture, or
/// the "not a closed solid" notice.
fn push_views(out: &mut String, views: ViewsBlock<'_>) {
    out.push_str("<h2>Views</h2>\n");
    match views {
        ViewsBlock::Four(views) => {
            out.push_str("<div class=\"views\">\n");
            for (label, image) in [
                ("down +Z", &views.top),
                ("down +X", &views.side),
                ("down -Y", &views.end),
                ("down -Z", &views.bottom),
            ] {
                let _ = writeln!(
                    out,
                    "<figure><img src=\"{}\" width=\"{}\" height=\"{}\" alt=\"{label}\">\
                     <figcaption>{label}</figcaption></figure>",
                    png_data_uri(image),
                    image.width,
                    image.height
                );
            }
            out.push_str("</div>\n");
        }
        ViewsBlock::Single(image) => {
            let _ = writeln!(
                out,
                "<div class=\"diagram\"><img src=\"{}\" width=\"{}\" height=\"{}\" \
                 alt=\"2D faceting diagram\"></div>",
                png_data_uri(image),
                image.width,
                image.height
            );
        }
        ViewsBlock::Missing => out.push_str(
            "<p class=\"diagram-missing\">Diagram unavailable: this design does not \
             currently close to a solid.</p>\n",
        ),
    }
}

/// The comment lines as a list; nothing at all when there are none.
fn push_comments(out: &mut String, layout: &SheetLayout) {
    if layout.details.comments.is_empty() {
        return;
    }
    out.push_str("<h2>Comments</h2>\n<ul class=\"comments\">\n");
    for comment in &layout.details.comments {
        let _ = writeln!(out, "<li>{}</li>", html_escape(comment));
    }
    out.push_str("</ul>\n");
}

/// Which optional columns this sheet carries. All are decided once from the whole
/// schedule rather than per row, so the header and every row agree by construction.
#[derive(Clone, Copy)]
struct TierTableColumns {
    /// The mast repeated in millimetres -- only when a girdle diameter anchors a
    /// real scale.
    show_mm: bool,
    /// The per-tier cheater/azimuth offset -- only when some tier has one.
    show_cheater: bool,
    /// One `<tbody>` per tier with a zebra class on it, so a concave tier's two lines
    /// share one band -- only when the design has concave tiers.
    banded: bool,
}

impl TierTableColumns {
    /// How many cells a full-width row of the table has: step, label, angle, indices,
    /// instruction and mast, plus the optional columns.
    fn width(self) -> usize {
        6 + usize::from(self.show_mm) + usize::from(self.show_cheater)
    }
}

/// One section (`Pavilion` or `Crown`): its heading and its table. Nothing for a section
/// with no rows.
fn push_section(
    out: &mut String,
    heading: &str,
    rows: &[(&CutSheetRow, Block)],
    columns: TierTableColumns,
    layout: &SheetLayout,
) {
    if rows.is_empty() {
        return;
    }
    let _ = writeln!(out, "<section class=\"cut-section\">\n<h2>{heading}</h2>");
    out.push_str("<table class=\"tiers\">\n<thead><tr>");
    out.push_str(
        "<th>#</th><th>Label</th><th>Angle</th><th>Indices</th><th>Instruction</th><th>Mast</th>",
    );
    if columns.show_mm {
        out.push_str("<th>Mast (mm)</th>");
    }
    if columns.show_cheater {
        out.push_str("<th>Cheater</th>");
    }
    out.push_str("</tr></thead>\n");
    if columns.banded {
        // One `<tbody>` per tier, striped per tier rather than per line: a concave
        // tier's tool line belongs to the same band as its facet line, and
        // `break-inside: avoid` on the `<tbody>` keeps the pair on one page.
        for (position, &(row, _)) in rows.iter().enumerate() {
            let stripe = if position % 2 == 0 {
                "band-a"
            } else {
                "band-b"
            };
            let _ = writeln!(out, "<tbody class=\"{stripe}\">");
            push_tier_row(out, row, columns, layout.mm_per_unit);
            if let Some(concave) = &row.concave {
                push_tool_row(out, concave, columns, layout.girdle_diameter_mm);
            }
            out.push_str("</tbody>\n");
        }
    } else {
        out.push_str("<tbody>\n");
        for &(row, _) in rows {
            push_tier_row(out, row, columns, layout.mm_per_unit);
        }
        out.push_str("</tbody>\n");
    }
    out.push_str("</table>\n</section>\n");
}

/// A concave tier's second `<tr class="tool">`, under the facet line's columns: the
/// tool code under the label, theta under the angle, the displacement under the
/// indices, and the size and motion spanning the rest. The strings are
/// [`ConcaveRowInfo::second_line_fields`], the very ones the text sheet prints. With a
/// girdle diameter set, the diameter and displacement are also given in millimetres
/// (the ratios are over the stone width, which is the girdle diameter).
fn push_tool_row(
    out: &mut String,
    concave: &ConcaveRowInfo,
    columns: TierTableColumns,
    girdle_diameter_mm: Option<f64>,
) {
    let [code, theta, displacement, details] = concave.second_line_fields();
    out.push_str("<tr class=\"tool\"><td></td>");
    let _ = write!(out, "<td>{}</td>", html_escape(&code));
    let _ = write!(out, "<td>{}</td>", html_escape(&theta));
    let mut displacement_cell = html_escape(&displacement);
    let mut details_cell = html_escape(&details);
    if let Some(width_mm) = girdle_diameter_mm {
        let [x, y, z] = concave.displacement.map(|ratio| ratio * width_mm);
        let _ = write!(
            displacement_cell,
            " <span class=\"mm\">({x:.2}, {y:.2}, {z:.2} mm)</span>"
        );
        let _ = write!(
            details_cell,
            " <span class=\"mm\">(D = {:.2} mm)</span>",
            concave.diameter_ratio * width_mm
        );
    }
    let _ = write!(out, "<td>{displacement_cell}</td>");
    // Everything after the indices column: the instruction, the mast and the optional columns.
    let _ = write!(
        out,
        "<td colspan=\"{}\">{details_cell}</td>",
        columns.width() - 4
    );
    out.push_str("</tr>\n");
}

/// One `<tr>` of a section table: step number, the tier's code, unsigned positive angle,
/// index list, the instruction (the tier's name in front of the meet note), mast in model
/// units, mast in mm (only when `columns.show_mm`; `"-"` when `mm_per_unit` itself could not
/// be resolved) and the cheater offset (only when `columns.show_cheater`).
///
/// A concave tier has no mast, so its mast cells read `-`, as the text sheet's do.
fn push_tier_row(
    out: &mut String,
    row: &CutSheetRow,
    columns: TierTableColumns,
    mm_per_unit: Option<f64>,
) {
    out.push_str("<tr>");
    let _ = write!(out, "<td>{}</td>", row.sequence);
    let _ = write!(out, "<td>{}</td>", html_escape(row.label()));
    let _ = write!(out, "<td>{:.2}&deg;</td>", row.angle_deg.abs());
    let _ = write!(out, "<td>{}</td>", format_sheet_indices(&row.indices));
    let _ = write!(out, "<td>{}</td>", html_escape(&row.instruction()));
    let has_mast = row.concave.is_none();
    let mast = if has_mast {
        format!("{:.4}", row.mast)
    } else {
        "-".to_string()
    };
    let _ = write!(out, "<td>{mast}</td>");
    if columns.show_mm {
        let cell = mm_per_unit.filter(|_| has_mast).map_or_else(
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
    out.push_str("</tr>\n");
}

/// The opening half of the page: `<!DOCTYPE html>` through the print stylesheet and
/// `<body>`, with `{TITLE}` standing for the escaped heading. A print stylesheet that
/// actually works on paper -- a sane page size, no dark background burning ink on a real
/// printer, a tier's row never splitting across a page break, and no section, table or
/// view split by one either (`page-break-inside: avoid` plus its modern `break-inside`
/// alias, since printer/browser support for the two still varies).
const HTML_HEAD: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>{TITLE}</title>
<style>
  :root { color-scheme: light; }
  * { box-sizing: border-box; }
  body {
    font-family: Arial, Helvetica, sans-serif;
    background: #ffffff;
    color: #111111;
    margin: 24px;
  }
  h1 { font-size: 22px; margin: 0 0 6px; }
  h2 {
    font-size: 15px;
    margin: 16px 0 6px;
    break-after: avoid;
    page-break-after: avoid;
  }
  p.subtitle, p.byline, p.date { margin: 0 0 3px; font-size: 13px; }
  table.meta { border-collapse: collapse; margin-bottom: 8px; }
  table.meta td { padding: 2px 12px 2px 0; font-size: 13px; vertical-align: top; }
  table.meta td.label { font-weight: 700; color: #333333; white-space: nowrap; }
  table.data { border-collapse: collapse; margin-bottom: 8px; }
  table.data th, table.data td {
    border: 1px solid #999999;
    padding: 3px 10px;
    font-size: 12px;
    text-align: left;
  }
  table.data th { background: #eeeeee; }
  table.data td.label { font-weight: 700; }
  .views { display: grid; grid-template-columns: 1fr 1fr; gap: 8px; }
  .views figure { margin: 0; text-align: center; break-inside: avoid; page-break-inside: avoid; }
  .views img { width: 100%; height: auto; }
  .views figcaption { font-size: 12px; font-weight: 700; margin-top: 2px; }
  .diagram { text-align: center; margin: 8px 0 20px; }
  .diagram img { max-width: 100%; height: auto; }
  .diagram-missing { color: #b91c1c; font-style: italic; }
  ul.comments { font-size: 13px; margin: 0 0 8px; padding-left: 18px; }
  section.cut-section { break-inside: avoid; page-break-inside: avoid; }
  table.tiers { border-collapse: collapse; width: 100%; font-size: 12px; }
  table.tiers th, table.tiers td {
    border: 1px solid #999999;
    padding: 4px 6px;
    text-align: left;
  }
  table.tiers thead { background: #eeeeee; }
  table.tiers tr { page-break-inside: avoid; break-inside: avoid; }
  @page { size: A4; margin: 14mm; }
  @media print {
    body { margin: 0; background: #ffffff; }
    table.tiers thead { display: table-header-group; }
    .diagram, .views { break-inside: avoid; page-break-inside: avoid; }
  }
</style>
</head>
<body>
"#;

/// The extra rules a sheet with concave tiers adds just before `</style>`: the zebra
/// bands (one per `<tbody>`, so a tier's two lines share one) and the tool line's look.
const CONCAVE_STYLE: &str = r"  tbody.band-b { background: #f4f4f4; }
  tbody { page-break-inside: avoid; break-inside: avoid; }
  tr.tool td { border-top: none; color: #333333; font-style: italic; }
  tr.tool span.mm { color: #666666; }
</style>";

/// The closing half of the page.
const HTML_TAIL: &str = "</body>\n</html>\n";
