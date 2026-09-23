//! A design's printable cutting sequence as a single self-contained HTML file --
//! printable HTML, no new dependency, prints straight from the browser -- and the
//! standalone 2D-diagram export that shares its rendering pipeline.
//!
//! # Pure-function-plus-thin-callback
//!
//! Everything that decides what the sheet says is a pure function of `&Design` +
//! `&[SolvedTier]`: [`render_cut_diagram`] (mesh -> rendered/PNG-encoded diagram,
//! reusing `gui::solid_preview::diagram2d`, the same rasterizer the Edit tab's
//! "Diagram" view mode already draws) and [`cutting_sheet_html`] (that diagram
//! plus [`indicatrix_cut_core::Design::cutting_sheet`]'s own rows -> one HTML
//! string). Neither touches Slint, a file system, or a dialog, so both are
//! testable with a plain `Design` fixture and no window -- see this module's own
//! tests.
//!
//! [`write_cutting_sheet_html`]/[`write_diagram_png`] are the "thin callback"
//! half: they call the pure functions above, then do exactly the file-system work
//! (an `rfd` save dialog, one `std::fs::write`) that has to live somewhere. Wiring
//! either into an actual `EditorModel` callback is outside this file -- see the
//! FIX-lane report's handoff notes for the exact `gui/editor/mod.rs`,
//! `ui/models/editor.slint` and `editor_command_bar.slint` additions that wiring
//! needs.
//!
//! # Crown/Pavilion labelling
//!
//! The per-row Crown/Pavilion/Girdle label comes from the solver's own
//! [`classify_blocks`] (`design.meet_tier_inputs()` in, one [`Block`] per tier
//! out, aligned with [`indicatrix_cut_core::Design::cutting_sheet`]'s own
//! one-row-per-tier order) -- never derived from the sign of a formatted angle
//! string, a mistake this codebase already made and fixed once (see
//! `design/missing_anchor.rs`'s own [`Block`] usage for the established
//! precedent).
//!
//! # Model-unit vs. mm masts
//!
//! The "Mast (mm)" column only appears when [`indicatrix_cut_core::Design::girdle_diameter_mm`]
//! is set -- not when the mm conversion actually resolves. A design with a
//! stated girdle diameter that currently fails to close (or measures under the
//! trusted-width floor -- see `yield_metrics::scale::mm_per_unit`'s own doc
//! comment) still shows the column, with `"-"` cells, rather than silently
//! reverting to a model-units-only sheet the moment a design goes briefly
//! unsolved.

use crate::gui::solid_preview::diagram2d::{self, DiagramConfig, DiagramStyle};
use image::{ExtendedColorType, ImageEncoder, codecs::png::PngEncoder};
use indicatrix::{
    geometry::{
        meet_solver::{Block, SolvedTier, classify_blocks},
        stone_metrics::{SolidStatus, build_solid_mesh},
    },
    optics::materials::GemMaterial,
};
use indicatrix_cut_core::{CutSheetRow, Design};
use std::{fmt::Write as _, path::Path};

/// Three-panel diagram resolution embedded in a cutting sheet -- small enough
/// that the resulting `data:` URI keeps the HTML file a reasonable size (a few
/// hundred KB, not several MB) while still being legible printed at A4/Letter
/// width; a cutter wanting a larger image uses [`write_diagram_png`] instead.
const SHEET_DIAGRAM_WIDTH: u32 = 900;
/// See [`SHEET_DIAGRAM_WIDTH`].
const SHEET_DIAGRAM_HEIGHT: u32 = 360;

/// The standalone diagram export's resolution -- print-quality (roughly
/// 150 DPI at A4 landscape width) rather than the cutting sheet's own smaller
/// embed, since this is the one path meant to be viewed/printed at full size on
/// its own.
pub const DIAGRAM_EXPORT_WIDTH: u32 = 1800;
/// See [`DIAGRAM_EXPORT_WIDTH`].
pub const DIAGRAM_EXPORT_HEIGHT: u32 = 720;

/// A rendered 2D facet diagram, already PNG-encoded -- [`render_cut_diagram`]'s
/// output, shared by [`cutting_sheet_html`] (embedded as a `data:` URI) and
/// [`write_diagram_png`] (written to disk as-is).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagramImage {
    /// Pixel width, matching what was requested of [`render_cut_diagram`].
    pub width: u32,
    /// Pixel height, matching what was requested of [`render_cut_diagram`].
    pub height: u32,
    /// The frame, already PNG-encoded (RGBA8, no ICC profile -- an untagged PNG
    /// is read as sRGB by every viewer/printer, matching this diagram's fixed,
    /// non-photographic color palette).
    pub png_bytes: Vec<u8>,
}

/// Builds `design`'s three-panel (crown/pavilion/profile) 2D facet diagram at
/// `width` x `height` from its already-[`Design::solve`]'d (or
/// [`Design::resolve_dirty`]'d) `solved` masts, and PNG-encodes it.
///
/// `None` when the design's current planes don't close to a real solid
/// ([`build_solid_mesh`] reports [`SolidStatus::Unbounded`]/[`SolidStatus::Degenerate`]
/// rather than [`SolidStatus::Closed`]) -- there is no meaningful diagram to draw
/// for a design that isn't currently a closed stone, the same condition the Edit
/// tab's own Solid/Diagram viewport already handles by showing its last-good
/// frame instead. A caller (a printed sheet, or a standalone diagram export)
/// shows this design-not-closed message itself rather than a blank or stale
/// image.
///
/// # Panics
///
/// Same alignment contract as [`Design::planes_from_solved`]: `solved` must
/// have one entry per tier `design` currently has, in the same order.
#[must_use]
pub fn render_cut_diagram(
    design: &Design,
    solved: &[SolvedTier],
    width: u32,
    height: u32,
) -> Option<DiagramImage> {
    let planes = design.planes_from_solved(solved);
    let SolidStatus::Closed(mesh) = build_solid_mesh(&planes) else {
        return None;
    };
    let config = DiagramConfig {
        width,
        height,
        gear_teeth: design.meta.gear_teeth_abs(),
        // `f32`: `DiagramConfig`'s own field, matching every other in-app
        // consumer of `ScheduleMeta::gear_reference_angle` (an `f64`) narrowed
        // to feed this Slint-free rendering path.
        gear_reference_angle: design.meta.gear_reference_angle as f32,
        symmetry_order: design.meta.symmetry_order,
        mirror: design.meta.mirror,
    };
    let frame = diagram2d::render_diagram(&mesh, &config, &DiagramStyle::default());
    let png_bytes = encode_png(frame.width, frame.height, &frame.color).ok()?;
    Some(DiagramImage {
        width: frame.width,
        height: frame.height,
        png_bytes,
    })
}

/// PNG-encodes a raw RGBA8 `width * height * 4`-byte buffer -- the same
/// `PngEncoder`/`ExtendedColorType::Rgba8` call
/// `bridge::export_thread::tonemap_png::save_png` uses for its own untagged
/// (sRGB) branch, just encoding to an in-memory buffer instead of a file (this
/// diagram's bytes end up embedded in HTML or written out by
/// [`write_diagram_png`], never saved directly by this function).
///
/// # Errors
///
/// The underlying encoder's error, stringified -- unreachable in practice for a
/// well-formed RGBA8 buffer matching `width`/`height`, but threaded through
/// rather than unwrapped.
fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    PngEncoder::new(&mut bytes)
        .write_image(rgba, width, height, ExtendedColorType::Rgba8)
        .map_err(|e| e.to_string())?;
    Ok(bytes)
}

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
#[must_use]
fn base64_encode(data: &[u8]) -> String {
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
fn html_escape(s: &str) -> String {
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

/// Builds `design`'s printable cutting sheet as a single self-contained HTML
/// string: a header block (material, refractive index, index gear/symmetry,
/// girdle diameter when set), the 2D diagram embedded as a `data:` PNG when
/// `diagram` is `Some` (build one via [`render_cut_diagram`]; `None` prints a
/// plain-text notice instead of a broken image), then one table row per tier in
/// cutting order -- step number, name, Crown/Pavilion/Girdle label, signed
/// angle, index list, mast (model units, plus mm when
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
fn material_header_text(design: &Design, n_d: f64) -> String {
    if let Some(name) = design.material.name.as_deref() {
        return name.to_string();
    }
    super::material_lookup::material_for_refractive_index(n_d).map_or_else(
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
                 alt=\"2D facet diagram\"></div>",
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

/// Builds `design`/`solved`'s cutting sheet (with an embedded diagram, when the
/// design currently closes) and writes it to `dest`.
///
/// The save-as file-picker dialog is handled by `native_io`'s own job
/// (`native_io::pick_file`, on a background thread), so this stays callable with
/// no `EditorState`/UI dependency at all, and safely callable from a background
/// thread itself. The actual `std::fs::write` also belongs on a background thread.
///
/// `custom` is threaded straight through to [`cutting_sheet_html`] -- see that
/// function's own doc comment.
///
/// # Errors
///
/// The write failure, ready to show as a toast.
pub fn write_cutting_sheet_html(
    design: &Design,
    solved: &[SolvedTier],
    dest: &Path,
    custom: &[GemMaterial],
) -> Result<(), String> {
    let diagram = render_cut_diagram(design, solved, SHEET_DIAGRAM_WIDTH, SHEET_DIAGRAM_HEIGHT);
    let html = cutting_sheet_html(design, solved, diagram.as_ref(), custom);

    if let Some(parent) = dest.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(dest, html).map_err(|e| format!("Failed to write {}: {e}", dest.display()))
}

/// The "thin callback" half of the standalone diagram export:
/// [`write_cutting_sheet_html`]'s narrower sibling -- the 2D diagram alone, at
/// print resolution ([`DIAGRAM_EXPORT_WIDTH`] x [`DIAGRAM_EXPORT_HEIGHT`]),
/// written to `dest`. Shares [`render_cut_diagram`] with the cutting sheet rather
/// than a second rendering path.
///
/// The save-as file-picker is handled by `native_io`, not by this function --
/// see [`write_cutting_sheet_html`]'s own doc comment.
///
/// # Errors
///
/// `Err` when `design` does not currently close to a solid (nothing to
/// diagram -- named explicitly rather than silently saving a blank/stale
/// image) or when the write itself fails.
pub fn write_diagram_png(
    design: &Design,
    solved: &[SolvedTier],
    dest: &Path,
) -> Result<(), String> {
    let Some(image) =
        render_cut_diagram(design, solved, DIAGRAM_EXPORT_WIDTH, DIAGRAM_EXPORT_HEIGHT)
    else {
        return Err(
            "This design does not currently close to a solid -- nothing to diagram.".to_string(),
        );
    };

    if let Some(parent) = dest.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(dest, &image.png_bytes)
        .map_err(|e| format!("Failed to write {}: {e}", dest.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix_cut_core::{ConstraintTier, MaterialSelection, PreformSpec, ScheduleMeta};

    /// The same round-brilliant fixture `indicatrix_cut_core::cutting_sheet`'s
    /// own tests use: 8 tiers, all `ScaleReference`, always solves and closes --
    /// see that crate's `design/tier.rs::standard_round_brilliant_template_solves_and_closes`.
    fn round_brilliant_design() -> Design {
        Design::new(
            PreformSpec::block(2.0, 1.0, 2.0),
            ScheduleMeta::standard_round_brilliant(),
            ConstraintTier::standard_round_brilliant(),
        )
    }

    fn custom_garnet(n_d: f32) -> GemMaterial {
        let mut gem = GemMaterial::diamond();
        gem.name = "My Garnet".to_string();
        gem.dispersion = indicatrix::optics::dispersion::DispersionModel::Cauchy {
            a: n_d,
            b: 0.0,
            c: 0.0,
        };
        gem
    }

    /// A design on a CUSTOM catalogue material must print that material's own
    /// `n_D` in the printed sheet's "Refractive index" row when `custom` is
    /// supplied -- the bug this module fixes. `&[]` (no catalogue) must still fall
    /// back to the legacy schedule RI, matching the built-ins-only behaviour every
    /// other test in this module already relies on.
    #[test]
    fn cutting_sheet_html_prints_a_custom_materials_own_refractive_index() {
        let mut design = round_brilliant_design();
        design.material = MaterialSelection {
            name: Some("My Garnet".to_string()),
            specific_gravity_override: None,
            refractive_index_override: None,
        };
        let solved = design.solve().expect("every tier is pinned");
        let custom = [custom_garnet(1.9)];

        let with_catalogue = cutting_sheet_html(&design, &solved, None, &custom);
        assert!(
            with_catalogue.contains("<td>1.9000</td>"),
            "{with_catalogue}"
        );

        let without_catalogue = cutting_sheet_html(&design, &solved, None, &[]);
        assert!(
            without_catalogue.contains(&format!("<td>{:.4}</td>", design.meta.refractive_index)),
            "{without_catalogue}"
        );
    }

    /// An unnamed design (every untouched import) whose effective RI sits at
    /// sapphire's own must print the guess label "Sapphire? (from RI 1.76)",
    /// never a bare number passed off as fact.
    #[test]
    fn material_header_text_shows_a_guess_for_an_unnamed_design_near_a_builtin() {
        let mut design = round_brilliant_design();
        design.meta.refractive_index = 1.76;
        design.material = MaterialSelection::none();
        let n_d = design.effective_refractive_index_with(&[]);
        assert_eq!(
            material_header_text(&design, n_d),
            "Sapphire? (from RI 1.76)"
        );
    }

    /// Once a material name IS set, the header must print it plainly -- no
    /// guess wording once something is a stated fact.
    #[test]
    fn material_header_text_prints_the_name_plainly_once_set() {
        let mut design = round_brilliant_design();
        design.material = MaterialSelection {
            name: Some("Sapphire".to_string()),
            specific_gravity_override: None,
            refractive_index_override: None,
        };
        let n_d = design.effective_refractive_index_with(&[]);
        assert_eq!(material_header_text(&design, n_d), "Sapphire");
    }

    /// The generated sheet must carry one row per tier, in cutting order, with
    /// the step number, tier name and formatted mast all present -- a basic
    /// "does the known schedule show up" check before the more targeted tests
    /// below.
    #[test]
    fn cutting_sheet_html_contains_expected_rows() {
        let design = round_brilliant_design();
        let solved = design.solve().expect("every tier is pinned");
        let html = cutting_sheet_html(&design, &solved, None, &[]);

        assert!(html.contains("<h1>Cutting Sheet</h1>"));
        assert!(html.contains("Table"));
        assert!(html.contains("Crown Main"));
        assert!(html.contains("Pavilion Main"));
        assert!(html.contains("Culet"));
        // One row per tier: count `<tr>` tags inside `<tbody>...</tbody>` only,
        // so the meta table's own rows and the tier table's `<thead>` row
        // (which also opens with `<tr>`) are never counted.
        let tbody_start = html.find("<tbody>").expect("tbody present") + "<tbody>".len();
        let tbody_end = html.find("</tbody>").expect("tbody closes");
        let tr_count = html[tbody_start..tbody_end].matches("<tr>").count();
        assert_eq!(tr_count, design.tiers.len());
    }

    /// Every row must print an "Elevation" figure (the unsigned magnitude of its
    /// angle) alongside the signed angle.
    #[test]
    fn elevation_column_prints_the_unsigned_angle_magnitude() {
        let design = round_brilliant_design();
        let solved = design.solve().expect("every tier is pinned");
        let html = cutting_sheet_html(&design, &solved, None, &[]);
        assert!(html.contains("<th>Elevation</th>"), "{html}");
        // "Pavilion Main" is authored at -41.0 degrees; its elevation cell must
        // print the unsigned magnitude, not the signed angle.
        assert!(html.contains("<td>41.00&deg;</td>"), "{html}");
    }

    /// A carat-weight row must appear only once a girdle diameter anchors a real
    /// mm scale -- matching the mm column's own gating.
    #[test]
    fn carat_weight_row_appears_only_when_girdle_diameter_is_set() {
        let design_no_mm = round_brilliant_design();
        let solved = design_no_mm.solve().expect("every tier is pinned");
        let html_no_mm = cutting_sheet_html(&design_no_mm, &solved, None, &[]);
        assert!(!html_no_mm.contains("Carat weight"));

        let mut design_with_mm = round_brilliant_design();
        design_with_mm.girdle_diameter_mm = Some(6.5);
        // A carat weight needs a specific gravity too, so a material must be
        // named -- the same fixture cut-core's own header test uses.
        design_with_mm.material.name = Some("Diamond".to_string());
        let html_with_mm = cutting_sheet_html(&design_with_mm, &solved, None, &[]);
        assert!(
            html_with_mm.contains("Carat weight (estimate)"),
            "{html_with_mm}"
        );
    }

    /// The "Mast (mm)" column -- and any mm-formatted cell -- must appear only
    /// when `girdle_diameter_mm` is set, never derived from whether the mm
    /// conversion happens to resolve.
    #[test]
    fn mm_column_appears_only_when_girdle_diameter_is_set() {
        let design_no_mm = round_brilliant_design();
        let solved = design_no_mm.solve().expect("every tier is pinned");
        let html_no_mm = cutting_sheet_html(&design_no_mm, &solved, None, &[]);
        assert!(!html_no_mm.contains("Mast (mm)"));

        let mut design_with_mm = round_brilliant_design();
        design_with_mm.girdle_diameter_mm = Some(6.5);
        let solved_mm = design_with_mm.solve().expect("every tier is pinned");
        let html_with_mm = cutting_sheet_html(&design_with_mm, &solved_mm, None, &[]);
        assert!(html_with_mm.contains("Mast (mm)"));
        assert!(html_with_mm.contains("Girdle diameter"));
    }

    /// Crown/Pavilion/Girdle labels must come from the solver's own
    /// `classify_blocks`, not the sign of the printed angle -- the mistake this
    /// codebase already made once. `standard_round_brilliant`'s own tiers give
    /// a known split: Table/Star/Crown Main/Upper Girdle are Crown, the 90-degree
    /// `Girdle` tier is Girdle, and Pavilion Main/Lower Girdle/Culet (the last at
    /// an unsigned-negative-zero angle, inheriting the pavilion side) are
    /// Pavilion.
    #[test]
    fn block_labels_match_classify_blocks_not_angle_sign() {
        let design = round_brilliant_design();
        let solved = design.solve().expect("every tier is pinned");
        let blocks = classify_blocks(&design.meet_tier_inputs());
        assert_eq!(
            blocks,
            vec![
                Block::Crown,
                Block::Crown,
                Block::Crown,
                Block::Crown,
                Block::Girdle,
                Block::Pavilion,
                Block::Pavilion,
                Block::Pavilion,
            ]
        );

        let html = cutting_sheet_html(&design, &solved, None, &[]);
        // "Culet" sits at angle `-0.0` -- a naive sign-of-the-formatted-string
        // check would call that Crown (`-0.0` formats with no visible sign in
        // some locales, or as positive zero); `classify_blocks`'s unsigned-zero
        // rule correctly inherits Pavilion from the tier above it, and that is
        // what the printed label must say.
        let culet_row_start = html.find("Culet").expect("Culet row present");
        let row_html =
            &html[culet_row_start..culet_row_start + 400.min(html.len() - culet_row_start)];
        assert!(
            row_html.contains("block-pavilion"),
            "Culet's row must be labelled Pavilion, got: {row_html}"
        );
    }

    /// `render_cut_diagram` must produce a non-empty PNG for a design that
    /// closes, sized exactly as requested.
    #[test]
    fn render_cut_diagram_produces_a_sized_png_for_a_closed_design() {
        let design = round_brilliant_design();
        let solved = design.solve().expect("every tier is pinned");
        let image = render_cut_diagram(&design, &solved, 200, 100)
            .expect("standard round brilliant closes to a solid");
        assert_eq!(image.width, 200);
        assert_eq!(image.height, 100);
        assert_ne!(image.png_bytes, [] as [u8; 0]);
        // PNG magic bytes.
        assert_eq!(&image.png_bytes[..8], &[137, 80, 78, 71, 13, 10, 26, 10]);
    }

    /// The embedded `data:` URI must actually appear in the sheet when a
    /// diagram is supplied, and the "unavailable" notice must appear instead
    /// when it is not.
    #[test]
    fn diagram_embedding_switches_on_the_option() {
        // The ELEMENT, not the bare class name: `.diagram-missing` is also a rule in
        // the sheet's own stylesheet, so a substring check on the class alone
        // matches even when the diagram is present.
        const NOTICE: &str = "<p class=\"diagram-missing\">";

        let design = round_brilliant_design();
        let solved = design.solve().expect("every tier is pinned");
        let image = render_cut_diagram(&design, &solved, 64, 32).expect("closes to a solid");

        let with_image = cutting_sheet_html(&design, &solved, Some(&image), &[]);
        assert!(with_image.contains("data:image/png;base64,"));
        assert!(!with_image.contains(NOTICE));

        let without_image = cutting_sheet_html(&design, &solved, None, &[]);
        assert!(without_image.contains(NOTICE));
        assert!(!without_image.contains("data:image/png;base64,"));
    }

    /// The hand-rolled base64 encoder must match RFC 4648 on values covering
    /// both padding cases (`chunks(3)` remainders of 1 and 2 bytes) and the
    /// empty input.
    #[test]
    fn base64_encode_matches_known_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    /// HTML-unsafe characters in a tier name must be escaped, not interpolated
    /// raw -- a `.asc` file's own free-text header/notes can legally contain
    /// `<`/`&`/quotes.
    #[test]
    fn html_escape_handles_unsafe_characters() {
        assert_eq!(
            html_escape("A & B <tag> \"quoted\" 'apos'"),
            "A &amp; B &lt;tag&gt; &quot;quoted&quot; &#39;apos&#39;"
        );
    }
}
