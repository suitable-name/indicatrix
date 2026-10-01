//! Rendering both sides of a comparison, off the UI thread.
//!
//! # Two workers per session, latest request wins
//!
//! Each open session owns two [`LatestWorker`]s -- one for the solid raster, one for
//! the CPU tracer -- each a single `std::thread` fed through a one-slot mailbox: a
//! new request overwrites any request the thread has not picked up yet, so a burst
//! of orbit events costs one render per thread wake-up, never a queue of stale
//! ones. The two are separate so a multi-second traced pair never delays the
//! instant solid pair that keeps orbiting fluid. Neither ever touches the main
//! viewport's `SolidPreviewState`, its mesh cache, or `solid_last_solved`: the
//! solid worker owns two [`MeshCache`]s of its own (one per side), built from the
//! session's own planes.
//!
//! Every request carries the view generation it was issued at; a worker checks the
//! shared `latest` generation before (and, for the tracer, between) the expensive
//! steps and skips work that is already stale, and the UI thread checks again on
//! delivery (`super::session::FrameBook`). Finished pixels travel back as
//! `SharedPixelBuffer`s (`Send`, unlike `slint::Image`) through the `post` function
//! the wiring supplies, which hops onto the event loop.
//!
//! The solid worker also derives the overlay comparison's difference layer
//! (`super::overlay`) from the two rasterizers' pick buffers in the same pass, so
//! the layer always describes exactly the pair it is delivered with.
//!
//! The pure halves -- [`render_solid_rgba`], [`render_traced_rgba`],
//! [`traced_size`], [`pixel_size`], [`placeholder_rgba`] -- take only planes, pose
//! and size, so `super::tests` exercises them without Slint or threads.

use super::{overlay::difference_overlay, session::CompareSide};
use crate::{
    bridge::preview_render::render_rgba_at_pose,
    gui::solid_preview::{
        mesh_cache::MeshCache,
        preview_state::CameraPose,
        raster::{SolidRasterizer, SolidStyle},
        to_pixel_buffer,
    },
};
use glam::Vec3;
use indicatrix::{
    geometry::{plane::GpuFacetPlane, stone_metrics::SolidMesh},
    optics::{
        materials::GemMaterial,
        raytracer::{Camera, DEFAULT_FOV_DEG, DEFAULT_MAX_BOUNCES},
    },
};
use slint::{Rgba8Pixel, SharedPixelBuffer};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use tracing::error;

/// Samples per pixel for a traced comparison -- modest and fixed: enough to read
/// brightness and windowing differences between the two sides, cheap enough to
/// finish in about a second per side on a desktop CPU at [`TRACED_MAX_EDGE`].
pub(super) const TRACED_SPP: u32 = 48;

/// The traced image's longest edge, in pixels -- see [`traced_size`].
pub(super) const TRACED_MAX_EDGE: u32 = 384;

/// Bounce cap for the traced mode, the same figure the catalogue previews use
/// (`gui::batch::preview::engine::PREVIEW_MAX_BOUNCES`): the raytracer's
/// [`DEFAULT_MAX_BOUNCES`].
const TRACED_MAX_BOUNCES: u32 = DEFAULT_MAX_BOUNCES;

/// A solid image's longest allowed edge in pixels -- a guard against a huge
/// high-DPI window allocating an absurd raster, far above any real window.
const MAX_SOLID_EDGE: u32 = 4096;

/// The solid raster's opaque background, `Theme.bg-input` (`#12141c`). Opaque on
/// purpose: the split view overlays the two images, and a transparent background
/// would let the other side show through around the stone.
const SOLID_BACKGROUND: [u8; 4] = [0x12, 0x14, 0x1c, 0xff];

/// The placeholder hatch's two colours and stripe period (pixels), for a side with
/// nothing honest to draw.
const PLACEHOLDER_DARK: [u8; 4] = SOLID_BACKGROUND;
const PLACEHOLDER_LIGHT: [u8; 4] = [0x25, 0x2b, 0x3b, 0xff];
const PLACEHOLDER_PERIOD: u32 = 10;

/// A finished frame's pixels, `Send` so they can cross back to the UI thread.
pub(super) type Pixels = SharedPixelBuffer<Rgba8Pixel>;

/// The flat grey solid style: the Edit tab's Solid view defaults (Lambert + rim on
/// a light grey base, preform planes tinted) on an opaque background, with no
/// selection/hover overlays -- a comparison has no selection of its own.
#[must_use]
pub(super) fn solid_style(preform_planes: usize) -> SolidStyle {
    SolidStyle {
        background: SOLID_BACKGROUND,
        preform_plane_count: preform_planes,
        ..SolidStyle::default()
    }
}

/// The camera both renderers use for `pose` -- `Camera::new(.., DEFAULT_FOV_DEG)`, the
/// exact call every viewport in this app builds, so solid and traced frames line up.
fn camera_for(pose: CameraPose) -> Camera {
    Camera::new(pose.yaw, pose.pitch, pose.distance, DEFAULT_FOV_DEG)
}

/// One side's solid renderer: its own mesh cache (rebuilt only when the planes
/// change, i.e. never within a session) and rasterizer (resized per request).
pub(super) struct SolidSideRenderer {
    cache: MeshCache,
    raster: SolidRasterizer,
}

impl Default for SolidSideRenderer {
    fn default() -> Self {
        Self {
            cache: MeshCache::default(),
            raster: SolidRasterizer::new(1, 1),
        }
    }
}

impl SolidSideRenderer {
    /// Rasterizes `planes` at `pose` into a `size` frame; returns whether the
    /// planes closed into a solid (when not, the frame is background only).
    pub(super) fn render(
        &mut self,
        planes: &[(Vec3, f32)],
        preform_planes: usize,
        pose: CameraPose,
        size: (u32, u32),
    ) -> bool {
        self.raster.resize(size.0.max(1), size.1.max(1));
        let camera = camera_for(pose);
        let style = solid_style(preform_planes);
        if let Some(cached) = self.cache.get_or_build(planes) {
            self.raster.render_prepared(cached, &camera, &style);
            true
        } else {
            self.raster.render(&SolidMesh::default(), &camera, &style);
            false
        }
    }
}

/// [`SolidSideRenderer::render`] on a fresh renderer, returning the RGBA8 buffer --
/// the pure planes + pose + size -> pixels form the tests pin (the worker keeps its
/// renderers across requests instead, so the mesh is built once per session).
#[cfg(test)]
#[must_use]
pub(super) fn render_solid_rgba(
    planes: &[(Vec3, f32)],
    preform_planes: usize,
    pose: CameraPose,
    size: (u32, u32),
) -> Vec<u8> {
    let mut renderer = SolidSideRenderer::default();
    renderer.render(planes, preform_planes, pose, size);
    renderer.raster.color
}

/// [`render_solid_rgba`]'s twin for the pick buffer (`facet_id + 1` per pixel, `0`
/// uncovered) -- what the difference overlay is computed from.
#[cfg(test)]
#[must_use]
pub(super) fn render_solid_pick(
    planes: &[(Vec3, f32)],
    preform_planes: usize,
    pose: CameraPose,
    size: (u32, u32),
) -> Vec<u32> {
    let mut renderer = SolidSideRenderer::default();
    renderer.render(planes, preform_planes, pose, size);
    renderer.raster.pick
}

/// The traced frame size for a `size` image slot: unchanged up to
/// [`TRACED_MAX_EDGE`] on the longest edge, else scaled down to it keeping the
/// aspect ratio -- the camera's field of view is vertical and its horizontal extent
/// follows the aspect, so a proportional scale frames the stone exactly as the
/// solid frame of the full slot does.
#[must_use]
pub(super) fn traced_size(size: (u32, u32)) -> (u32, u32) {
    let (width, height) = (size.0.max(1), size.1.max(1));
    let longest = width.max(height);
    if longest <= TRACED_MAX_EDGE {
        return (width, height);
    }
    let scale = f64::from(TRACED_MAX_EDGE) / f64::from(longest);
    let scaled = |edge: u32| ((f64::from(edge) * scale).round() as u32).max(1);
    (scaled(width), scaled(height))
}

/// A logical image-slot size (`CompareModel.image_width`/`image_height`) in
/// physical pixels at `scale_factor`, each edge clamped to `[0, 4096]`.
#[must_use]
pub(super) fn pixel_size(width: f32, height: f32, scale_factor: f32) -> (u32, u32) {
    let edge = |logical: f32| {
        let physical = (logical * scale_factor).round();
        if physical.is_finite() && physical > 0.0 {
            (physical as u32).min(MAX_SOLID_EDGE)
        } else {
            0
        }
    };
    (edge(width), edge(height))
}

/// Traces `planes` in `material` at `pose` into a `size` frame through
/// `bridge::preview_render`'s CPU path (the catalogue previews' own lighting,
/// backdrop and tone-mapping), at [`TRACED_SPP`].
#[must_use]
pub(super) fn render_traced_rgba(
    planes: &[(Vec3, f32)],
    material: &GemMaterial,
    pose: CameraPose,
    size: (u32, u32),
) -> Vec<u8> {
    // `(n, m)` with `n . x <= m` back to `GpuFacetPlane`'s `n . x + d = 0`, `d = -m`
    // -- the inverse of the conversion `CompareSide::build` made.
    let gpu_planes: Vec<GpuFacetPlane> = planes
        .iter()
        .map(|&(normal, offset)| GpuFacetPlane::new(normal, -offset))
        .collect();
    render_rgba_at_pose(
        &gpu_planes,
        material,
        (pose.yaw, pose.pitch, pose.distance),
        size,
        TRACED_SPP,
        TRACED_MAX_BOUNCES,
    )
}

/// A diagonal-hatch placeholder for a side with nothing honest to draw (it does
/// not solve, or -- traced -- its material does not resolve). The status line says
/// which and why.
#[must_use]
pub(super) fn placeholder_rgba(size: (u32, u32)) -> Vec<u8> {
    let (width, height) = (size.0.max(1), size.1.max(1));
    let mut rgba = Vec::with_capacity((width as usize) * (height as usize) * 4);
    for y in 0..height {
        for x in 0..width {
            let stripe = ((x + y) / PLACEHOLDER_PERIOD).is_multiple_of(2);
            rgba.extend_from_slice(if stripe {
                &PLACEHOLDER_LIGHT
            } else {
                &PLACEHOLDER_DARK
            });
        }
    }
    rgba
}

/// Copies an RGBA8 buffer into a [`Pixels`] of `size`; a length mismatch (never
/// expected) yields the placeholder instead of panicking on the worker thread.
fn rgba_to_pixels(rgba: &[u8], size: (u32, u32)) -> Pixels {
    let (width, height) = (size.0.max(1), size.1.max(1));
    let expected = (width as usize) * (height as usize) * 4;
    let mut buffer = Pixels::new(width, height);
    if rgba.len() == expected {
        buffer.make_mut_bytes().copy_from_slice(rgba);
    } else {
        buffer
            .make_mut_bytes()
            .copy_from_slice(&placeholder_rgba((width, height)));
    }
    buffer
}

/// What one worker needs about one side, cloned out of the [`CompareSide`] once
/// when the session is ready.
#[derive(Clone)]
pub(super) struct SideGeometry {
    planes: Vec<(Vec3, f32)>,
    preform_planes: usize,
    material: Option<GemMaterial>,
}

impl SideGeometry {
    /// The render-relevant part of `side`: empty planes for a side that does not
    /// solve, `None` material for one whose material does not resolve.
    #[must_use]
    pub(super) fn from_side(side: &CompareSide) -> Self {
        Self {
            planes: side.planes.clone(),
            preform_planes: side.preform_planes,
            material: side.material.as_ref().ok().cloned(),
        }
    }
}

/// One view to render: its generation, the shared pose, and the image-slot size in
/// physical pixels.
#[derive(Debug, Clone, Copy)]
pub(super) struct ViewRequest {
    /// The view generation this request was issued at.
    pub(super) generation: u64,
    /// The shared camera pose.
    pub(super) pose: CameraPose,
    /// The image-slot size in physical pixels.
    pub(super) size: (u32, u32),
}

/// What a worker reports back.
pub(super) enum FrameEvent {
    /// A solid pair.
    Solid {
        /// The before side.
        before: Pixels,
        /// The after side.
        after: Pixels,
        /// The transparent RGBA8 difference layer for the overlay comparison, four
        /// bytes per pixel of `before`'s own size; fully transparent when either
        /// side has no solid to compare.
        overlay: Vec<u8>,
    },
    /// The tracer finished the before side and moved on to side `side`.
    TracedProgress {
        /// The side now being traced, 1-based.
        side: u8,
    },
    /// A traced pair.
    Traced {
        /// The before side.
        before: Pixels,
        /// The after side.
        after: Pixels,
    },
    /// A worker panicked on a request; its thread keeps serving later views, but the
    /// request it was on produced no frame.
    WorkerFailed {
        /// Which preview failed: `"solid"` or `"traced"`.
        worker: &'static str,
    },
}

/// A worker report, tagged with the session and view it belongs to.
pub(super) struct FrameMsg {
    /// The session that issued the request -- a replaced session's late frames are
    /// dropped on this alone.
    pub(super) session_id: u64,
    /// The view generation the request was issued at.
    pub(super) generation: u64,
    /// The report itself.
    pub(super) event: FrameEvent,
}

pub(super) use crate::gui::latest_worker::LatestWorker;

/// Tells the UI thread that `worker` panicked on a request of session `session_id`.
/// The report carries generation 0: it is shown whatever view is current.
fn report_worker_failure(post: fn(FrameMsg), session_id: u64, worker: &'static str) {
    post(FrameMsg {
        session_id,
        generation: 0,
        event: FrameEvent::WorkerFailed { worker },
    });
}

/// Whether `generation` is still the newest view.
fn is_current(latest: &AtomicU64, generation: u64) -> bool {
    latest.load(Ordering::Acquire) == generation
}

/// One side's solid pixels -- the rasterized solid, or the placeholder for a side
/// that does not solve -- and whether the renderer's pick buffer holds a real solid
/// (`false` for the placeholder and for planes that do not close).
fn solid_side_pixels(
    renderer: &mut SolidSideRenderer,
    side: &SideGeometry,
    request: ViewRequest,
) -> (Pixels, bool) {
    if side.planes.is_empty() {
        return (
            rgba_to_pixels(&placeholder_rgba(request.size), request.size),
            false,
        );
    }
    let closed = renderer.render(
        &side.planes,
        side.preform_planes,
        request.pose,
        request.size,
    );
    (to_pixel_buffer(&renderer.raster), closed)
}

/// The difference layer for the pair just rasterized into `before` and `after`, or
/// a fully transparent layer of `size` when either side has no solid: tinting the
/// whole other side as "added" or "removed" would say nothing true.
fn solid_overlay(
    before: (&SolidSideRenderer, bool),
    after: (&SolidSideRenderer, bool),
    size: (u32, u32),
) -> Vec<u8> {
    if before.1 && after.1 {
        difference_overlay(&before.0.raster.pick, &after.0.raster.pick, None, size)
    } else {
        vec![0u8; (size.0 as usize) * (size.1 as usize) * 4]
    }
}

/// One side's traced pixels at `size`, or the placeholder when there is no
/// geometry or no resolved material to trace.
fn traced_side_pixels(side: &SideGeometry, pose: CameraPose, size: (u32, u32)) -> Pixels {
    match &side.material {
        Some(material) if !side.planes.is_empty() => rgba_to_pixels(
            &render_traced_rgba(&side.planes, material, pose, size),
            size,
        ),
        _ => rgba_to_pixels(&placeholder_rgba(size), size),
    }
}

/// Spawns a session's solid worker. `post` hands a finished report to the UI
/// thread (see `super::wiring::post_frame`).
#[must_use]
pub(super) fn spawn_solid_worker(
    session_id: u64,
    sides: (SideGeometry, SideGeometry),
    latest: Arc<AtomicU64>,
    post: fn(FrameMsg),
) -> LatestWorker<ViewRequest> {
    let (before_side, after_side) = sides;
    let mut before_renderer = SolidSideRenderer::default();
    let mut after_renderer = SolidSideRenderer::default();
    LatestWorker::spawn_with_panic_hook(
        "compare-solid",
        move |request: ViewRequest| {
            if !is_current(&latest, request.generation) {
                return;
            }
            let (before, before_solid) =
                solid_side_pixels(&mut before_renderer, &before_side, request);
            let (after, after_solid) = solid_side_pixels(&mut after_renderer, &after_side, request);
            let overlay = solid_overlay(
                (&before_renderer, before_solid),
                (&after_renderer, after_solid),
                (before.width(), before.height()),
            );
            post(FrameMsg {
                session_id,
                generation: request.generation,
                event: FrameEvent::Solid {
                    before,
                    after,
                    overlay,
                },
            });
        },
        move || {
            error!(
                "The solid comparison render of session {session_id} failed; the next view change renders again."
            );
            report_worker_failure(post, session_id, "solid");
        },
    )
}

/// Spawns a session's traced worker -- same contract as [`spawn_solid_worker`],
/// tracing at [`traced_size`] of the requested slot and reporting progress after
/// the first side.
#[must_use]
pub(super) fn spawn_traced_worker(
    session_id: u64,
    sides: (SideGeometry, SideGeometry),
    latest: Arc<AtomicU64>,
    post: fn(FrameMsg),
) -> LatestWorker<ViewRequest> {
    let (before_side, after_side) = sides;
    LatestWorker::spawn_with_panic_hook(
        "compare-traced",
        move |request: ViewRequest| {
            if !is_current(&latest, request.generation) {
                return;
            }
            let size = traced_size(request.size);
            let before = traced_side_pixels(&before_side, request.pose, size);
            if !is_current(&latest, request.generation) {
                return;
            }
            post(FrameMsg {
                session_id,
                generation: request.generation,
                event: FrameEvent::TracedProgress { side: 2 },
            });
            let after = traced_side_pixels(&after_side, request.pose, size);
            if !is_current(&latest, request.generation) {
                return;
            }
            post(FrameMsg {
                session_id,
                generation: request.generation,
                event: FrameEvent::Traced { before, after },
            });
        },
        move || {
            error!(
                "The traced comparison render of session {session_id} failed; the next view change renders again."
            );
            report_worker_failure(post, session_id, "traced");
        },
    )
}
