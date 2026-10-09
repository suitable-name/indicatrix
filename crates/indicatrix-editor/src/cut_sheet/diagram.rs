//! Renders a design's solved masts into a 2D crown/pavilion/profile faceting diagram
//! and PNG-encodes it, with no Slint/file/dialog dependency of any kind.

use image::{ExtendedColorType, ImageEncoder, codecs::png::PngEncoder};
use indicatrix::geometry::{
    meet_solver::SolvedTier,
    stone_metrics::{SolidStatus, build_solid_mesh},
};
use indicatrix_cut_core::Design;
use indicatrix_solid::diagram2d::{self, DiagramConfig, DiagramFrame, DiagramStyle, PanelKind};

/// A rendered 2D faceting diagram, already PNG-encoded.
///
/// [`render_cut_diagram`]'s output, shared by [`super::html::cutting_sheet_html`]
/// (embedded as a `data:` URI) and the desktop's diagram export (written to disk as-is).
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

/// Builds `design`'s three-panel (crown/pavilion/profile) 2D faceting diagram at
/// `width` x `height` and PNG-encodes it.
///
/// It is drawn from the already-[`Design::solve`]'d (or
/// [`Design::resolve_dirty`]'d) `solved` masts.
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
    let config = diagram_config(design, width, height);
    let style = diagram_style(design, solved);
    let frame = diagram2d::render_diagram(&mesh, &config, &style);
    frame_image(&frame)
}

/// The four views a printed cutting sheet shows, each a single-panel diagram.
///
/// The labels are the cutting-instructions template's, and they name the side the stone is
/// seen from. The template's axes are the stone's own (`Z` along the stone's axis); the
/// code's world axes have the stone's axis on `Y`, so the template's `(X, Y, Z)` are the
/// world's `(Z, X, Y)`:
///
/// | Label | Field | Seen from (world) | Panel |
/// |---|---|---|---|
/// | `down +Z` | [`Self::top`] | `+Y`, above | crown |
/// | `down +X` | [`Self::side`] | `+Z` | profile |
/// | `down -Y` | [`Self::end`] | `-X` | end view |
/// | `down -Z` | [`Self::bottom`] | `-Y`, below | pavilion |
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CutViews {
    /// `down +Z`: the crown, seen from above.
    pub top: DiagramImage,
    /// `down +X`: the side, the profile panel.
    pub side: DiagramImage,
    /// `down -Y`: the end, the side view a quarter turn from `side`.
    pub end: DiagramImage,
    /// `down -Z`: the pavilion, seen from below.
    pub bottom: DiagramImage,
}

/// Builds `design`'s four sheet views, each `width` x `height` pixels, PNG-encoded.
///
/// `None` under the same condition as [`render_cut_diagram`]: the design's planes do not
/// close to a solid. Facet labels are the same canonical tier codes the three-panel diagram
/// prints.
///
/// # Panics
///
/// Same alignment contract as [`Design::planes_from_solved`]: `solved` must have one entry
/// per tier `design` currently has, in the same order.
#[must_use]
pub fn render_cut_views(
    design: &Design,
    solved: &[SolvedTier],
    width: u32,
    height: u32,
) -> Option<CutViews> {
    let planes = design.planes_from_solved(solved);
    let SolidStatus::Closed(mesh) = build_solid_mesh(&planes) else {
        return None;
    };
    let config = diagram_config(design, width, height);
    let style = diagram_style(design, solved);
    let single = |panel| {
        frame_image(&diagram2d::render_diagram_single_panel(
            &mesh, &config, &style, panel,
        ))
    };
    Some(CutViews {
        top: single(PanelKind::Crown)?,
        side: single(PanelKind::Profile)?,
        end: frame_image(&diagram2d::render_diagram_end_view(&mesh, &config, &style))?,
        bottom: single(PanelKind::Pavilion)?,
    })
}

/// The diagram's frame size and index wheel for `design`.
const fn diagram_config(design: &Design, width: u32, height: u32) -> DiagramConfig {
    DiagramConfig {
        width,
        height,
        gear_teeth: design.meta.gear_teeth_abs(),
        // `f32`: `DiagramConfig`'s own field, matching every other in-app
        // consumer of `ScheduleMeta::gear_reference_angle` (an `f64`) narrowed
        // to feed this Slint-free rendering path.
        gear_reference_angle: design.meta.gear_reference_angle as f32,
        symmetry_order: design.meta.symmetry_order,
        mirror: design.meta.mirror,
    }
}

/// The diagram's style for `design`: the default look with every facet labelled by its tier.
fn diagram_style(design: &Design, solved: &[SolvedTier]) -> DiagramStyle {
    let mut style = DiagramStyle::default();
    let facet_map = indicatrix_solid::facet_map::FacetMap::from_design(design, solved);
    style.facet_labels = (0..facet_map.facet_count())
        .map(|id| facet_map.facet_label(id))
        .collect();
    style
}

/// `frame` PNG-encoded, `None` when the encoder refuses it.
fn frame_image(frame: &DiagramFrame) -> Option<DiagramImage> {
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
/// the desktop's diagram export, never saved directly by this function).
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
