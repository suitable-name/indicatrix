//! Solid inspection preview: a flat-shaded, edge-outlined CPU render of the current
//! design's [`indicatrix::geometry::stone_metrics::SolidMesh`], independent of the
//! GPU path tracer.
//!
//! [`raster`], [`diagram2d`], [`facet_map`], [`mesh_cache`], [`edges_layer`],
//! [`live_update`] and `pixel_font` (re-exported at `gui::pixel_font`, see
//! `gui::mod`) are all re-exported here, unchanged at their old paths, from the
//! shared [`indicatrix_solid`] crate -- see that crate's own doc comment for what
//! each one does. They moved out of this app (`crates/indicatrix-solid`) so the wasm web app
//! can reuse the exact same solid renderer and 2D diagram, with no behaviour
//! change here: every module below is the identical code, just imported rather
//! than declared locally.
//!
//! [`to_pixel_buffer`] is the one bridge into Slint types, kept here (rather than
//! moving with `raster`) so the shared crate stays independently testable and
//! reusable as the seam for a possible future `wgpu` backend, with no Slint
//! dependency at all.
//!
//! [`preview_state`] is the editor-side controller: one dedicated worker thread
//! owning the mesh cache and rasterizer, coalescing redraw requests via
//! `RedrawGate`, handing finished frames to a [`preview_state::PreviewSink`] kept
//! generic over Slint rather than depending on `MainWindow` directly. It (plus
//! [`diagram_wiring`], the Slint callback wiring) stays in this app: it owns
//! threads and Slint types, neither of which the shared crate may depend on.

pub use indicatrix_solid::{diagram2d, edges_layer, facet_map, live_update, mesh_cache, raster};

pub mod preview_state;

pub mod diagram_wiring;

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
