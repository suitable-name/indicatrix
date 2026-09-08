//! Renders the two cached catalogue-preview images (front + top) for one design's own
//! geometry, and picks which built-in [`GemMaterial`] preset to render it in.
//!
//! This is the Rust-side half of `indicatrix_vault::model::preview` -- that crate owns
//! *storage* (`PreviewImages`, `Database::save_preview_images`/`get_preview_images`/
//! `ensure_preview_material`) and is deliberately `indicatrix`-free (see
//! `indicatrix_vault::model::material_match`'s module doc comment), so the actual
//! rendering and the `GemMaterial::all_materials()` -> `RiPresetCandidate` adaptation
//! both have to live on this side of the boundary. `gui::preview_batch` is the one
//! caller: it resolves a design's planes/material and calls [`render_view`] (locally)
//! or [`render_view_remote`] once per [`PreviewView`] -- up to twice per design, from
//! whichever of its local/remote lanes claims each view -- and persists whatever comes
//! back via `Database::save_preview_images` once both views are accounted for.
//!
//! # Camera convention: front = pitch 0, top = pitch 90 degrees
//!
//! `indicatrix::optics::raytracer::Camera::new`'s own origin formula --
//! `(d*cos(p)*sin(y), d*sin(p), d*cos(p)*cos(y))` -- puts the camera directly overhead
//! (`origin = (0, d, 0)`) at `pitch = 90 deg` regardless of `yaw` (azimuth is provably
//! irrelevant at that pole), and squarely on the horizontal plane at `pitch = 0`. That is
//! the SAME pole `indicatrix::color::metrics::evaluate_full_axis_profile_at_azimuth` calls
//! "the shared table-up (pitch 90) pole" in its own doc comment -- [`PreviewView::pitch`]
//! reuses exactly that convention rather than inventing a second one, so "top" here means
//! the same thing it means everywhere else in this codebase: table-up, face-up.
//!
//! # Sizing: measured cost and the chosen default
//!
//! Single-threaded release-build cost for one square preview render of a round
//! brilliant at 12 bounces:
//!
//! | dim | spp | per image (1 thread) |
//! |---|---|---|
//! | 96  | 64  | 1.67 s |
//! | 128 | 64  | 2.97 s |
//! | 128 | 256 | 11.54 s |
//! | 160 | 128 | 8.88 s |
//! | 160 | 256 | 17.78 s |
//!
//! Divide by core count for the real wall-clock figure. `settings::model::{
//! DEFAULT_PREVIEW_SIZE, DEFAULT_PREVIEW_SPP}` default to 160x160 at 256 spp -- about
//! 1.1s/image on 16 cores, small enough for a diagram-list card while staying clean
//! rather than noisy at that size. Both numbers are exposed as settings
//! (`AppSettings::preview_size`/`preview_spp`), not hardcoded, since this is a
//! taste/patience trade-off, not a fixed requirement.
//!
//! # Render path: GPU with a CPU scanline fallback, one full-resolution dispatch
//!
//! Each view is one call: try [`GpuBackend::try_accumulate`] for the whole `spp` budget
//! in a single dispatch, falling back to [`export_thread::batch::render_batch`] (the
//! exact CPU scanline tracer the export path uses) if the GPU declines. Unlike an
//! export, a preview never chunks a view's samples into multiple batches --
//! `gui::preview_batch`'s progress reports "which view"/"which lane", not "how
//! converged", so there is nothing to report more finely than "done", and a ~1-2s
//! render has no cancellation-latency argument for chunking either.
//! `gui::preview_batch` gives cancellation and per-view panic isolation their own,
//! coarser granularity (between items) instead.

use crate::{
    bridge::{
        export_thread::{self, SceneSnapshot},
        remote::remote_render::{self, RemoteRenderRequest, RemoteUpdate},
    },
    settings::WorkerSettings,
};
use glam::Vec3;
use indicatrix::{
    geometry::plane::GpuFacetPlane,
    optics::{
        materials::GemMaterial,
        raytracer::{Camera, LightingPreset},
    },
    renderer::{
        gpu_backend::{GpuBackend, GpuSceneRef},
        tonemap::tonemap_to_rgba,
    },
};
use indicatrix_net::{SceneState, client::Accumulator};
use indicatrix_vault::model::material_match::RiPresetCandidate;
use std::{
    io::Cursor,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, RecvTimeoutError},
    },
    time::Duration,
};

/// Refractive index is conventionally quoted at the sodium D line -- see
/// `indicatrix_vault::model::material_match`'s module doc comment for why every
/// [`RiPresetCandidate`] built here must evaluate `GemMaterial::dispersion` at exactly
/// this wavelength to match the convention a design's own stored (scraped)
/// `refractive_index` already uses.
///
/// `pub` (effectively crate-visible only -- clippy's `redundant_pub_crate` prefers
/// plain `pub` over `pub(crate)` here): `gui::material_quality`'s `on_material_changed`
/// handler also needs this exact constant, not a bare `589.3` magic number.
pub const SODIUM_D_NM: f32 = 589.3;

/// Evaluates `material`'s refractive index at the sodium D line ([`SODIUM_D_NM`]) --
/// the per-material conversion [`ri_candidates`] applies to every built-in preset,
/// factored out for a caller that only needs one material's RI. Used by
/// `gui::material_quality`'s `on_material_changed` handler to default the
/// RI-tolerance filter's centre to the material currently loaded in the viewport.
#[must_use]
pub fn material_ri_at_sodium_d(material: &GemMaterial) -> f64 {
    f64::from(material.dispersion.evaluate(SODIUM_D_NM))
}

/// Camera pose shared by both preview views -- see this module's doc comment for the
/// pitch convention. `yaw`/`distance`/`light_yaw`/`light_pitch`/`exposure` all match
/// `settings::model::app_settings::DEFAULT_CAMERA_*`/`DEFAULT_LIGHT_*` (the live
/// viewport's own fresh-install defaults), so a preview thumbnail looks like a plain,
/// undialled-in render of the design -- not a special "thumbnail" look a user has never
/// otherwise seen.
const PREVIEW_YAW: f32 = 0.60;
const PREVIEW_DISTANCE: f32 = 2.4;
/// `pub` (effectively crate-visible only -- see [`SODIUM_D_NM`]'s note): `gui::tilt_batch`'s
/// batch tilt-curve computation reuses this same light position for every design,
/// rather than whatever angle happens to be dialled into the live viewport. Deliberate,
/// not a shortcut: `crate::model::performance`'s search filters compare a threshold
/// against a design's stored curves, and a filter like "windowing stays under 20%
/// within +/-45 degrees" only means the same thing across the catalogue if every
/// design's curves were swept under the identical light -- letting the batch inherit a
/// per-session light pose would make stored curves silently depend on who last ran it.
pub const PREVIEW_LIGHT_YAW: f32 = 0.85;
pub const PREVIEW_LIGHT_PITCH: f32 = 0.95;
const PREVIEW_EXPOSURE: f32 = 1.0;
/// Matches `bridge::export_thread::run_export`'s own `Camera::new(..., 42.0)` call --
/// one field of view for every still render this app produces, live viewport included
/// (`RenderContext::default`'s camera setup uses the same figure).
const PREVIEW_FOV_DEG: f32 = 42.0;
/// The lighting rig every preview renders under -- `RingLights` is this app's own
/// default lighting-preset label (`DEFAULT_LIGHTING_RIG`), matching `PREVIEW_YAW`'s own
/// "look like an undialled-in render" reasoning above.
const PREVIEW_LIGHTING_PRESET: LightingPreset = LightingPreset::RingLights;

/// Which of the two cached preview images a [`render_view`] call produces -- see this
/// module's doc comment for the pitch each maps to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewView {
    /// Edge-on / profile view: `pitch = 0`, on the horizontal plane.
    Front,
    /// Table-up / face-up view: `pitch = 90 degrees`, straight overhead.
    Top,
}

impl PreviewView {
    #[must_use]
    const fn pitch(self) -> f32 {
        match self {
            Self::Front => 0.0,
            Self::Top => std::f32::consts::FRAC_PI_2,
        }
    }
}

/// Builds the `RiPresetCandidate` list `indicatrix_vault::model::material_match::
/// pick_ri_preset` matches a design's own refractive index against -- one candidate per
/// `GemMaterial::all_materials()` entry, evaluated at the sodium D line (see
/// [`SODIUM_D_NM`]). The one place in this crate that performs this adaptation, since
/// `indicatrix_vault` is deliberately `indicatrix`-free.
#[must_use]
pub fn ri_candidates() -> Vec<RiPresetCandidate> {
    GemMaterial::all_materials()
        .into_iter()
        .map(|material| {
            let refractive_index = material_ri_at_sodium_d(&material);
            RiPresetCandidate {
                name: material.name,
                refractive_index,
            }
        })
        .collect()
}

/// Everything one [`render_view`] call needs about the design being rendered, bundled
/// so that function's signature stays short. `planes`/`material` are already fully
/// resolved by the caller (`gui::preview_batch`); this module only renders what it's
/// given.
pub struct PreviewJob<'a> {
    pub planes: &'a [GpuFacetPlane],
    pub material: &'a GemMaterial,
    /// Square render dimension in pixels -- `AppSettings::preview_size` (or a caller-
    /// chosen override), see this module's doc comment for the measured cost table.
    pub size: u32,
    /// Samples per pixel -- `AppSettings::preview_spp`.
    pub spp: u32,
    /// `AppSettings::default().max_bounces`-equivalent bounce cap for this render. Not
    /// itself a settings-file field (a preview's bounce cap is fixed, not user-tunable
    /// the way size/spp are -- see `gui::preview_batch::PREVIEW_MAX_BOUNCES`'s own
    /// doc comment for why 12 was chosen and left off the settings surface).
    pub max_bounces: u32,
}

/// Renders `job`'s design from `view`'s camera pose, tone-maps it via
/// `indicatrix::renderer::tonemap::tonemap_to_rgba` (the same path every other sRGB
/// render in this app uses), and returns PNG bytes ready for
/// `Database::save_preview_images`.
///
/// The PNG encode step can theoretically fail on an internal buffer-size mismatch;
/// that case returns `None` rather than panicking, and `gui::preview_batch` treats it
/// like a caught panic for this one view. A genuine panic inside the tracer itself is
/// not caught here -- `gui::preview_batch::render_item_local` already wraps each call
/// in `catch_unwind` at the single-item granularity its progress reports at.
#[must_use]
pub fn render_view(job: &PreviewJob<'_>, view: PreviewView, gpu: &GpuBackend) -> Option<Vec<u8>> {
    let scene = SceneSnapshot {
        yaw: PREVIEW_YAW,
        pitch: view.pitch(),
        distance: PREVIEW_DISTANCE,
        light_yaw: PREVIEW_LIGHT_YAW,
        light_pitch: PREVIEW_LIGHT_PITCH,
        material: job.material.clone(),
        lighting_preset: PREVIEW_LIGHTING_PRESET,
        max_bounces: job.max_bounces,
        exposure: PREVIEW_EXPOSURE,
        active_planes: job.planes.to_vec(),
        // No frosted-girdle finish for a catalogue thumbnail -- matches the export's
        // own "empty means every facet Polished" convention
        // (`trace_spectral_ray_with_finish`'s doc comment) rather than reading a
        // per-design setting that doesn't exist for a design that has never been
        // opened in the live viewport this session.
        facet_finishes: Vec::new(),
        // Catalogue thumbnails always use the analytic studio rig, same reasoning as
        // `facet_finishes` above: there is no per-design HDR map setting to read, and a
        // batch-generated thumbnail for all ~3,187 designs must not depend on whatever
        // HDR map happened to be loaded in the live viewport during the session that
        // triggered the batch.
        env_map: None,
    };
    let camera = Camera::new(scene.yaw, scene.pitch, scene.distance, PREVIEW_FOV_DEG);
    let environment =
        scene
            .lighting_preset
            .studio(scene.exposure, scene.light_yaw, scene.light_pitch);
    let gpu_scene = GpuSceneRef {
        camera: &camera,
        width: job.size,
        height: job.size,
        planes: &scene.active_planes,
        facet_finishes: &scene.facet_finishes,
        material: &scene.material,
        max_bounces: scene.max_bounces,
        environment,
    };

    let pixel_count = (job.size as usize) * (job.size as usize);
    let mut accum = vec![Vec3::ZERO; pixel_count];
    if !gpu.try_accumulate(&gpu_scene, 0, job.spp, &mut accum) {
        export_thread::batch::render_batch(
            job.size, job.size, job.spp, 0, &camera, &scene, &mut accum,
        );
    }

    // `1.0 / job.spp as f32`: same `inv_samples` convention every other tone-mapping
    // call site in this crate uses (see `export_thread::tonemap_png::tonemap_to_rgba`,
    // which this function's own doc comment already notes this reuses).
    let rgba = tonemap_to_rgba(&accum, 1.0 / job.spp as f32);
    encode_png(job.size, job.size, &rgba)
}

/// A single fixed request id for every preview remote dispatch -- safe for the same
/// reason `export_thread::remote::REQUEST_ID` gives: [`remote_render::spawn_remote_render`]
/// opens its OWN fresh one-shot connection per call, so nothing is ever pipelined
/// behind an unrelated request on the same socket the way the live viewport's
/// persistent-connection `next_request_id` counter has to guard against.
const PREVIEW_REQUEST_ID: u32 = 1;

/// The remote counterpart of [`render_view`]: dispatches `job`'s view as one
/// `RenderRequest` covering the whole `job.spp` budget in a single request (a preview
/// is small enough to never need an export's batched-request chunking) against
/// `worker`, blocking until it finishes, fails, or `cancel` is observed.
///
/// Returns `None` on any kind of shortfall -- connection failure, worker rejection,
/// cancellation, or fewer samples than asked for -- so `gui::preview_batch`'s remote
/// lane can decide what's next: under `LiveComputeTarget::Both` it requeues the item
/// for a guaranteed local retry via [`render_view`]; under `RemoteOnly` the failure is
/// final and surfaced. Never partially reports: unlike an export (which keeps a
/// worker's partial contribution and traces only the shortfall locally), a preview is
/// cheap enough that a partial remote result is discarded and the whole image is
/// retraced locally.
#[must_use]
pub fn render_view_remote(
    job: &PreviewJob<'_>,
    view: PreviewView,
    worker: &WorkerSettings,
    cancel: &AtomicBool,
) -> Option<Vec<u8>> {
    let scene = SceneState {
        width: job.size,
        height: job.size,
        yaw: PREVIEW_YAW,
        pitch: view.pitch(),
        distance: PREVIEW_DISTANCE,
        light_yaw: PREVIEW_LIGHT_YAW,
        light_pitch: PREVIEW_LIGHT_PITCH,
        exposure: PREVIEW_EXPOSURE,
        max_bounces: job.max_bounces,
        lighting_preset: PREVIEW_LIGHTING_PRESET,
        material: job.material.clone(),
        planes: job.planes.to_vec(),
        girdle_frosted: false,
    };
    let accumulator = Arc::new(Mutex::new(Accumulator::new(job.size, job.size)));
    let (tx, rx) = mpsc::channel::<RemoteUpdate>();
    let handle = remote_render::spawn_remote_render(
        RemoteRenderRequest {
            worker: worker.clone(),
            request_id: PREVIEW_REQUEST_ID,
            scene,
            first_sample: 0,
            samples: job.spp,
            width: job.size,
            height: job.size,
        },
        Arc::clone(&accumulator),
        move |update| {
            let _ = tx.send(update);
        },
    );

    let mut cancel_sent = false;
    loop {
        if !cancel_sent && cancel.load(Ordering::Relaxed) {
            handle.cancel();
            cancel_sent = true;
        }
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(
                RemoteUpdate::Done {
                    cancelled: true, ..
                }
                | RemoteUpdate::Failed { .. },
            ) => {
                return None;
            }
            Ok(RemoteUpdate::Done {
                cancelled: false, ..
            }) => break,
            Ok(
                RemoteUpdate::Connected { .. }
                | RemoteUpdate::Preview { .. }
                | RemoteUpdate::Frame { .. }
                | RemoteUpdate::Progress { .. },
            )
            | Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return None,
        }
    }

    let acc = accumulator
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if acc.samples_done() < job.spp {
        return None;
    }
    let rgba = tonemap_to_rgba(acc.buffer(), 1.0 / job.spp as f32);
    drop(acc);
    encode_png(job.size, job.size, &rgba)
}

/// PNG-encodes `rgba` (`width * height * 4` bytes) into an in-memory buffer -- the
/// preview path's counterpart of `export_thread::tonemap_png::save_png`'s `Srgb` branch,
/// duplicated in miniature (no ICC profile, no file I/O) because a cached preview is
/// always sRGB (there is no colour-space picker for a background thumbnail the way
/// `export_dialog.slint` offers one for a deliberate export) and is stored as bytes in
/// SQLite, never written to a path on disk.
fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Option<Vec<u8>> {
    let image = image::RgbaImage::from_raw(width, height, rgba.to_vec())?;
    let mut bytes = Vec::new();
    image
        .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
        .ok()?;
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_view_pitch_matches_the_documented_front_and_top_convention() {
        assert_eq!(PreviewView::Front.pitch(), 0.0);
        assert!((PreviewView::Top.pitch() - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
    }

    /// `Camera::new`'s own origin formula (`d*sin(pitch)` for the Y component) must
    /// place `PreviewView::Top`'s camera directly overhead -- pins the actual
    /// consequence of the pitch convention above, not just the raw angle.
    #[test]
    fn top_pitch_places_the_camera_directly_overhead() {
        let camera = Camera::new(
            0.0,
            PreviewView::Top.pitch(),
            PREVIEW_DISTANCE,
            PREVIEW_FOV_DEG,
        );
        assert!((camera.origin.x).abs() < 1e-3);
        assert!((camera.origin.y - PREVIEW_DISTANCE).abs() < 1e-3);
        assert!((camera.origin.z).abs() < 1e-3);
    }

    /// `Front`'s pitch must place the camera ON the horizontal plane (zero Y), for any
    /// yaw -- an edge-on/profile view by construction, not merely "not the top pole".
    #[test]
    fn front_pitch_places_the_camera_on_the_horizontal_plane() {
        for yaw in [0.0, 0.6, 3.0] {
            let camera = Camera::new(
                yaw,
                PreviewView::Front.pitch(),
                PREVIEW_DISTANCE,
                PREVIEW_FOV_DEG,
            );
            assert!((camera.origin.y).abs() < 1e-3, "yaw={yaw}");
        }
    }

    #[test]
    fn ri_candidates_covers_every_built_in_material_with_a_finite_ri() {
        let candidates = ri_candidates();
        assert_eq!(candidates.len(), GemMaterial::all_materials().len());
        for c in &candidates {
            assert!(
                c.refractive_index.is_finite() && c.refractive_index > 1.0,
                "material {:?} produced an implausible RI {}",
                c.name,
                c.refractive_index
            );
        }
    }

    /// `ri_candidates` must agree with this crate's own documented convention
    /// (`material.dispersion.evaluate(589.3)`, the sodium D line every other RI-reading
    /// call site in `indicatrix::optics::materials` uses) for a material whose RI is
    /// well known -- Diamond's is ~2.417.
    #[test]
    fn ri_candidates_evaluates_at_the_sodium_d_line() {
        let candidates = ri_candidates();
        let diamond = candidates
            .iter()
            .find(|c| c.name == "Diamond")
            .expect("Diamond is a built-in material");
        assert!(
            (diamond.refractive_index - 2.417).abs() < 0.01,
            "got {}",
            diamond.refractive_index
        );
    }

    #[test]
    fn render_view_produces_a_decodable_png_at_the_requested_size() {
        use indicatrix::geometry::cuts::StandardGemCuts;

        let planes = StandardGemCuts::standard_round_brilliant();
        let material = GemMaterial::diamond();
        let job = PreviewJob {
            planes: &planes,
            material: &material,
            size: 8,
            spp: 1,
            max_bounces: 4,
        };
        let gpu = GpuBackend::disabled();
        let png = render_view(&job, PreviewView::Top, &gpu).expect("render must succeed");

        let decoded = image::load_from_memory(&png).expect("must be a valid PNG");
        assert_eq!(decoded.width(), 8);
        assert_eq!(decoded.height(), 8);
    }
}
