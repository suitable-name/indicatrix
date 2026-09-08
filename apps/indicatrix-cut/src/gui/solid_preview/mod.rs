//! Solid inspection preview: a flat-shaded, edge-outlined CPU render of the current
//! design's [`indicatrix::geometry::stone_metrics::SolidMesh`], independent of the
//! GPU path tracer.
//!
//! [`raster`] is a pure-Rust, Slint-free rasterizer with its own unit tests.
//! [`to_pixel_buffer`] is the one bridge into Slint types, kept here so `raster.rs`
//! stays independently testable and reusable as the seam for a possible future `wgpu`
//! backend.
//!
//! [`mesh_cache`] caches the `SolidMesh` build plus a one-time ring-simplification
//! pass against the design's plane-set hash. [`preview_state`] is the editor-side
//! controller: one dedicated worker thread owning the cache and rasterizer,
//! coalescing redraw requests via `RedrawGate`, handing finished frames to a
//! [`preview_state::PreviewSink`] kept generic over Slint rather than depending on
//! `MainWindow` directly.
//!
//! [`facet_map`] maps a rasterized `facet_id` back to its tier/orbit-member for
//! hover/click/selection and the critical-angle overlay; [`live_update`] decides
//! which geometry to draw after an edit (`plan_preview`, budgeted `resolve_dirty` with
//! a pinned/fresh/stale/unsolvable outcome); [`edges_layer`] is the "Both" view mode's
//! transparent-fill, opaque-edges render; [`diagram2d`] is view mode 3's GemCAD-style
//! three-panel (crown/pavilion/profile) 2D facet diagram, with its own per-pixel
//! facet-picking buffer. All four are Slint-free; `facet_map`/`live_update` are gated
//! on `indicatrix-cut-core`, `edges_layer`/`diagram2d` are not.

pub mod mesh_cache;
pub mod preview_state;
pub mod raster;

pub mod diagram2d;
pub mod diagram_wiring;
pub mod edges_layer;
#[cfg(feature = "editor")]
pub mod facet_map;
#[cfg(feature = "editor")]
pub mod live_update;

/// Converts a finished [`raster::SolidRasterizer`] frame into a `Send`-safe pixel
/// buffer for [`preview_state::PreviewSink::apply`] to hand to the UI thread.
///
/// Deliberately a `slint::SharedPixelBuffer<slint::Rgba8Pixel>`, not a `slint::Image`
/// (not `Send` in this Slint version -- moving one into an `upgrade_in_event_loop`
/// closure fails to compile): `slint::Image::from_rgba8` is called only on the UI
/// thread, matching this crate's other cross-thread frame handoffs.
#[must_use]
pub fn to_pixel_buffer(
    rasterizer: &raster::SolidRasterizer,
) -> slint::SharedPixelBuffer<slint::Rgba8Pixel> {
    let mut buffer =
        slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(rasterizer.width, rasterizer.height);
    let dst: &mut [slint::Rgba8Pixel] = buffer.make_mut_slice();
    let src: &[slint::Rgba8Pixel] = bytemuck::cast_slice(&rasterizer.color);
    dst.copy_from_slice(src);
    buffer
}

/// [`to_pixel_buffer`]'s twin for a [`diagram2d::DiagramFrame`] -- same reasoning
/// (a raw `SharedPixelBuffer`, never a `slint::Image`, so the worker thread's
/// result stays `Send`).
#[must_use]
pub fn to_diagram_pixel_buffer(
    frame: &diagram2d::DiagramFrame,
) -> slint::SharedPixelBuffer<slint::Rgba8Pixel> {
    let mut buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(frame.width, frame.height);
    let dst: &mut [slint::Rgba8Pixel] = buffer.make_mut_slice();
    let src: &[slint::Rgba8Pixel] = bytemuck::cast_slice(&frame.color);
    dst.copy_from_slice(src);
    buffer
}
