//! The window-free work of the first wizard steps: preparing a view's photos (decode, calibrate,
//! working resolution, masks), the camera and backlight calibration, and the surface map of a
//! view.
//!
//! Everything here runs on a worker thread and returns plain data.

use super::{brush, filters::load_filter_set};
use glam::DVec2;
use indicatrix::color::led::LedKind;
use indicatrix_cut_core::rough_plan::{
    camera_spectral::{BacklightSpectrum, CameraResponse, calibrate_from_filters},
    colour_fit::forward::WhiteFrame,
    locate::{DEFAULT_MARGIN_MM, InclusionShell, RigProfile, Rigid, Scene},
    photometry::{
        CalibrationFrames, ConsistencyOptions, ConsistencyReport, DEFAULT_WORKING_PX, HdrInput,
        HdrOptions, InclusionMarker, LinearImage, MeshMaskOptions, NoiseOptions, PixelMask,
        ResampleOptions, ResampledImage, TransmittanceOptions, ViewCalibration, WorkingGrid,
        check_view, decode_file, flag, merge_hdr, mesh_masks, stone_region,
    },
    shape::RoughMesh,
};
use std::{path::PathBuf, sync::Arc};

/// The files of one view.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ViewFiles {
    /// The stone photo.
    pub stone: Option<PathBuf>,
    /// White frames (empty rig, backlight on).
    pub white: Vec<PathBuf>,
    /// Dark frames (backlight off).
    pub dark: Vec<PathBuf>,
    /// An optional second, shorter exposure of the stone.
    pub second: Option<PathBuf>,
}

impl ViewFiles {
    /// Whether the view can be prepared: a stone photo and a white frame.
    #[must_use]
    pub const fn is_complete(&self) -> bool {
        self.stone.is_some() && !self.white.is_empty()
    }
}

/// The mesh, rig and alignment a worker needs, owned so it can move to a thread.
#[derive(Debug, Clone)]
pub struct SceneData {
    /// The scanned rough.
    pub mesh: Arc<RoughMesh>,
    /// The camera rig.
    pub rig: RigProfile,
    /// Mesh to rig.
    pub alignment: Rigid,
    /// The located inclusions as shells (mesh frame).
    pub shells: Vec<InclusionShell>,
    /// The same as spheres for the masks.
    pub markers: Vec<InclusionMarker>,
}

impl SceneData {
    /// The data of a mesh, rig and alignment; the inclusions come from the mesh.
    #[must_use]
    pub fn new(mesh: Arc<RoughMesh>, rig: RigProfile, alignment: Rigid) -> Self {
        let shells: Vec<InclusionShell> = mesh
            .inclusions()
            .iter()
            .map(|body| InclusionShell {
                points: body.vertices().to_vec(),
                triangles: body.triangles().to_vec(),
                margin_mm: DEFAULT_MARGIN_MM,
            })
            .collect();
        let markers = shells
            .iter()
            .filter_map(InclusionMarker::from_shell)
            .collect();
        Self {
            mesh,
            rig,
            alignment,
            shells,
            markers,
        }
    }

    /// The scene (borrowing).
    #[must_use]
    pub fn scene(&self) -> Scene<'_> {
        Scene::new(&self.mesh, &self.rig, self.alignment)
    }
}

/// A view ready for the later steps.
#[derive(Debug, Clone)]
pub struct PreparedView {
    /// The rig view index.
    pub view: usize,
    /// The view's name.
    pub name: String,
    /// The working grid.
    pub grid: WorkingGrid,
    /// The transmittance on the working grid; its mask holds the sampling flags (saturated,
    /// below the noise floor).
    pub observed: ResampledImage,
    /// The flags the mesh gives (outline, edge band, inclusion, ghost).
    pub auto_mask: PixelMask,
    /// The user's brush (only the `USER` flag).
    pub user_mask: PixelMask,
    /// The first triangle each working pixel sees, `u32::MAX` for none.
    pub triangles: Vec<u32>,
    /// The checks of the view's frames.
    pub consistency: ConsistencyReport,
    /// The backlight flat field of the view. `None` for a view restored from the plan's cached
    /// working-resolution photo: the white frame is not cached, so such a view cannot be fitted
    /// again until its original photos are loaded and prepared.
    pub white_frame: Option<WhiteFrame>,
    /// The mean corrected white frame colour (white minus dark), for the backlight fit. `None`
    /// for a restored view.
    pub white_rgb: Option<[f64; 3]>,
}

impl PreparedView {
    /// A view restored from the cached working-resolution photo of a saved plan.
    ///
    /// `image` is what was cached at Accept: its mask already holds every flag (sampling, mesh and
    /// brush), so the mesh and brush masks start empty and [`combined_mask`](Self::combined_mask)
    /// gives the cached mask back. There is no triangle map (no window painting on this view), no
    /// white frame and no consistency findings.
    #[must_use]
    pub fn from_cache(view: usize, name: &str, image: ResampledImage) -> Self {
        let (width, height) = (image.grid.width, image.grid.height);
        Self {
            view,
            name: name.to_owned(),
            grid: image.grid,
            observed: image,
            auto_mask: PixelMask::new(width, height),
            user_mask: PixelMask::new(width, height),
            triangles: vec![u32::MAX; width * height],
            consistency: ConsistencyReport::default(),
            white_frame: None,
            white_rgb: None,
        }
    }

    /// Whether this view came from the plan's cache (no original photo behind it).
    #[must_use]
    pub const fn is_cached(&self) -> bool {
        self.white_frame.is_none()
    }

    /// Every flag together: sampling, mesh and brush.
    #[must_use]
    pub fn combined_mask(&self) -> PixelMask {
        let mut mask = self.observed.mask.clone();
        let _ = mask.merge(&self.auto_mask);
        let _ = mask.merge(&self.user_mask);
        mask
    }

    /// The share of working pixels without any flag.
    #[must_use]
    pub fn usable_fraction(&self) -> f64 {
        let mask = self.combined_mask();
        let total = (mask.width() * mask.height()).max(1);
        mask.clear_count() as f64 / total as f64
    }

    /// One line about the mask: how much is left and what takes the rest.
    #[must_use]
    pub fn mask_note(&self) -> String {
        let mask = self.combined_mask();
        let pct = |bits: u8| {
            mask.count(bits) as f64 * 100.0 / (mask.width() * mask.height()).max(1) as f64
        };
        format!(
            "{}: {:.0} % usable; outside {:.0} %, edge band {:.0} %, saturated {:.0} %, below noise {:.0} %, inclusion {:.0} %, your brush {:.0} %",
            self.name,
            self.usable_fraction() * 100.0,
            pct(flag::OUTSIDE_OUTLINE),
            pct(flag::EDGE_BAND),
            pct(flag::SATURATED),
            pct(flag::BELOW_NOISE),
            pct(flag::INCLUSION | flag::GHOST),
            pct(flag::USER),
        )
    }

    /// Recomputes the mesh flags (a new edge band width).
    pub fn recompute_auto(&mut self, data: &SceneData, edge_band_px: usize) {
        self.auto_mask = mesh_masks(
            &data.scene(),
            self.view,
            &self.grid,
            &data.markers,
            &MeshMaskOptions {
                edge_band_px,
                ..MeshMaskOptions::default()
            },
        );
    }

    /// Excludes or restores a patch with the user brush.
    pub fn paint(&mut self, centre: [f64; 2], radius_px: f64, mode: brush::BrushMode) -> usize {
        brush::apply(&mut self.user_mask, centre, radius_px, mode)
    }
}

/// The first triangle each working pixel of `grid` sees in `view`.
#[must_use]
pub fn triangle_map(scene: &Scene<'_>, view: usize, grid: &WorkingGrid) -> Vec<u32> {
    let mut out = vec![u32::MAX; grid.width * grid.height];
    for y in 0..grid.height {
        for x in 0..grid.width {
            let centre: DVec2 = grid.centre(x, y);
            if let Some((origin, dir)) = scene.camera_ray(view, centre)
                && let Some(hit) = scene.mesh.first_hit(origin, dir, 1e-9)
            {
                out[y * grid.width + x] = hit.triangle;
            }
        }
    }
    out
}

/// The stone's exclusion mask at the stone photo's full resolution, for the compression-noise
/// estimate of the transmittance.
///
/// The mesh flags (outline, edge band, inclusion, ghost) exist only on the working grid, so they
/// are upsampled by nearest neighbour: every full-resolution pixel takes the flags of the working
/// pixel that contains its centre, and a pixel outside the grid (the margin around the stone's
/// region) is flagged as outside the outline. Nearest neighbour is exact enough here, the
/// estimator only needs to keep the flat backlight and the stone's edge out; it also skips the
/// four neighbours of every flagged pixel.
#[must_use]
pub fn full_resolution_exclusion(
    grid: &WorkingGrid,
    working: &PixelMask,
    width: usize,
    height: usize,
) -> PixelMask {
    let mut out = PixelMask::new(width, height);
    for y in 0..height {
        for x in 0..width {
            let centre = DVec2::new(x as f64 + 0.5, y as f64 + 0.5);
            let bits = match grid.working_coordinates(centre) {
                Some((wx, wy)) => {
                    #[expect(
                        clippy::cast_possible_truncation,
                        clippy::cast_sign_loss,
                        reason = "`as` saturates (negative and NaN give 0) and both indices are clamped to the mask size right below"
                    )]
                    let (col_idx, row_idx) = (wx as usize, wy as usize);
                    working.get(
                        col_idx.min(working.width().saturating_sub(1)),
                        row_idx.min(working.height().saturating_sub(1)),
                    )
                }
                None => flag::OUTSIDE_OUTLINE,
            };
            if bits != 0 {
                out.set(x, y, bits);
            }
        }
    }
    out
}

fn read_all(paths: &[PathBuf]) -> Result<Vec<LinearImage>, String> {
    paths
        .iter()
        .map(|p| decode_file(p).map_err(|e| format!("{}: {e}", p.display())))
        .collect()
}

fn mean_rgb(frames: &CalibrationFrames) -> Result<[f64; 3], String> {
    let (white, _) = frames.mean_white().map_err(|e| e.to_string())?;
    let (dark, _) = frames.mean_dark().map_err(|e| e.to_string())?;
    let mut sum = [0.0_f64; 3];
    for (w, d) in white.pixels.iter().zip(&dark.pixels) {
        for c in 0..3 {
            sum[c] += f64::from((w[c] - d[c]).max(0.0));
        }
    }
    let n = white.pixels.len().max(1) as f64;
    Ok([sum[0] / n, sum[1] / n, sum[2] / n])
}

/// Prepares one view: decodes the photos, calibrates, resamples to the working grid and
/// computes the masks.
///
/// # Errors
///
/// A sentence naming the view and what failed.
pub fn prepare_view(
    view: usize,
    name: &str,
    files: &ViewFiles,
    data: &SceneData,
    edge_band_px: usize,
) -> Result<PreparedView, String> {
    let fail = |e: &dyn std::fmt::Display| format!("{name}: {e}");
    let stone_path = files
        .stone
        .as_ref()
        .ok_or_else(|| format!("{name}: no stone photo."))?;
    let stone = decode_file(stone_path).map_err(|e| fail(&e))?;
    let frames = CalibrationFrames {
        white: read_all(&files.white).map_err(|e| fail(&e))?,
        dark: read_all(&files.dark).map_err(|e| fail(&e))?,
    };
    let calibration =
        ViewCalibration::new(frames, &NoiseOptions::default()).map_err(|e| fail(&e))?;
    let consistency = check_view(&stone, &calibration.frames, &ConsistencyOptions::default());
    let white_rgb = mean_rgb(&calibration.frames).map_err(|e| fail(&e))?;
    // A black channel would make the flat field meaningless; refuse rather than substitute a
    // reference.
    if !white_rgb.iter().all(|v| *v > 0.0) {
        return Err(fail(
            &"the white frame is black in at least one color channel (check the exposure and that the backlight was on)",
        ));
    }
    let white_frame =
        WhiteFrame::from_calibration(&calibration.frames, white_rgb, 512).map_err(|e| fail(&e))?;

    let (stone, extra) = if let Some(second_path) = &files.second {
        let second = decode_file(second_path).map_err(|e| fail(&e))?;
        let merged = merge_hdr(
            &[
                HdrInput {
                    image: &stone,
                    exposure: None,
                },
                HdrInput {
                    image: &second,
                    exposure: None,
                },
            ],
            &calibration.noise,
            &HdrOptions::default(),
        )
        .map_err(|e| fail(&e))?;
        (merged.image, Some(merged.mask))
    } else {
        (stone, None)
    };
    // The geometry comes first: the stone's silhouette (outline, edge band, inclusions) is needed
    // for the transmittance, whose compression-noise estimate must not see the flat backlight
    // around the stone (it would drive the robust spread toward zero).
    let scene = data.scene();
    let region = stone_region(&scene, view, 0.05)
        .ok_or_else(|| fail(&"the stone is outside this camera's picture"))?;
    let grid = WorkingGrid::fit(region, DEFAULT_WORKING_PX).map_err(|e| fail(&e))?;
    let auto_mask = mesh_masks(
        &scene,
        view,
        &grid,
        &data.markers,
        &MeshMaskOptions {
            edge_band_px,
            ..MeshMaskOptions::default()
        },
    );
    let exclusion = full_resolution_exclusion(&grid, &auto_mask, stone.width, stone.height);
    let mut transmittance = calibration
        .transmittance_masked(&stone, &TransmittanceOptions::default(), Some(&exclusion))
        .map_err(|e| fail(&e))?;
    if let Some(extra) = &extra {
        let _ = transmittance.mask.merge(extra);
    }
    drop(stone);
    let observed = transmittance
        .to_working(&grid, &ResampleOptions::default())
        .map_err(|e| fail(&e))?;
    let triangles = triangle_map(&scene, view, &grid);
    Ok(PreparedView {
        view,
        name: name.to_owned(),
        user_mask: PixelMask::new(grid.width, grid.height),
        grid,
        observed,
        auto_mask,
        triangles,
        consistency,
        white_frame: Some(white_frame),
        white_rgb: Some(white_rgb),
    })
}

// --- Calibration -----------------------------------------------------------------------------

/// How the camera's spectral sensitivities are known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CameraChoice {
    /// The sRGB matrix (least certain).
    Fallback,
    /// Measured curves, a `wavelength, r, g, b` file.
    Curves(PathBuf),
    /// A reference filter set, see `filters`.
    Filters(PathBuf),
}

/// How the backlight spectrum is known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BacklightChoice {
    /// The closest CIE LED kind to the white frame, with a fitted tilt.
    Auto,
    /// A CIE LED kind.
    Led(LedKind),
    /// A measured `wavelength, power` file.
    Csv(PathBuf),
}

/// The calibration in effect.
#[derive(Debug, Clone)]
pub struct Calibration {
    /// The camera response.
    pub camera: CameraResponse,
    /// The backlight spectrum.
    pub backlight: BacklightSpectrum,
    /// What was done, one line each.
    pub notes: Vec<String>,
}

/// Builds the camera response and the backlight.
///
/// # Errors
///
/// A sentence about the file or the numbers that failed.
pub fn build_calibration(
    camera: &CameraChoice,
    light: &BacklightChoice,
    white_rgb: [f64; 3],
) -> Result<Calibration, String> {
    let mut notes = Vec::new();
    let read = |p: &PathBuf| {
        std::fs::read_to_string(p).map_err(|e| format!("Could not read {}: {e}", p.display()))
    };
    let base = match camera {
        CameraChoice::Curves(path) => {
            CameraResponse::from_csv(&read(path)?).map_err(|e| format!("Camera curves: {e}"))?
        }
        _ => CameraResponse::from_srgb().map_err(|e| format!("Camera fallback: {e}"))?,
    };
    let backlight = match light {
        BacklightChoice::Led(kind) => {
            notes.push(format!("Backlight: {}.", kind.name()));
            BacklightSpectrum::from_led(*kind).map_err(|e| format!("Backlight: {e}"))?
        }
        BacklightChoice::Csv(path) => {
            notes.push(format!("Backlight: measured spectrum {}.", path.display()));
            BacklightSpectrum::from_csv(&read(path)?).map_err(|e| format!("Backlight: {e}"))?
        }
        BacklightChoice::Auto => {
            let fit = BacklightSpectrum::closest_to_white_frame(white_rgb, &base)
                .map_err(|e| format!("Backlight from the white frame: {e}"))?;
            notes.push(format!(
                "Backlight: closest CIE LED is {} with a fitted tilt (slope {:.2}, curvature {:.2}); chromaticity distance {:.3}.",
                fit.kind.name(),
                fit.tilt.slope,
                fit.tilt.curvature,
                fit.chromaticity_distance
            ));
            fit.backlight
        }
    };
    let camera = match camera {
        CameraChoice::Filters(path) => {
            let filters = load_filter_set(path)?;
            let (response, report) = calibrate_from_filters(&filters, backlight.spd())
                .map_err(|e| format!("Camera calibration: {e}"))?;
            notes.push(format!(
                "Camera: calibrated from {} filters, residual rms {:.4}, effective degrees of freedom {:.1}, condition {:.1e}.",
                report.filter_count, report.residual_rms, report.effective_dof, report.condition_estimate
            ));
            response
        }
        CameraChoice::Curves(path) => {
            notes.push(format!("Camera: measured curves from {}.", path.display()));
            base
        }
        CameraChoice::Fallback => {
            notes.push(
                "Camera: sRGB matrix fallback. Colors are less certain; load curves or a filter set for better ones."
                    .to_owned(),
            );
            base
        }
    };
    Ok(Calibration {
        camera,
        backlight,
        notes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_view_is_complete_with_a_stone_and_a_white_frame() {
        let mut files = ViewFiles::default();
        assert!(!files.is_complete());
        files.stone = Some(PathBuf::from("a.png"));
        assert!(!files.is_complete());
        files.white.push(PathBuf::from("w.png"));
        assert!(files.is_complete());
    }

    #[test]
    fn a_view_from_the_cache_has_the_cached_mask_and_no_originals_behind_it() {
        let grid = WorkingGrid::fit([0, 0, 4, 3], 4).unwrap();
        let n = grid.width * grid.height;
        let image = ResampledImage {
            grid,
            values: vec![[0.5; 3]; n],
            variance: vec![[0.01; 3]; n],
            coverage: vec![1.0; n],
            mask: PixelMask::new(grid.width, grid.height),
        };
        let view = PreparedView::from_cache(2, "Left", image);
        assert!(view.is_cached());
        assert_eq!((view.view, view.name.as_str()), (2, "Left"));
        assert_eq!(view.triangles.len(), n);
        assert!(view.white_frame.is_none() && view.white_rgb.is_none());
        assert_eq!(view.combined_mask().clear_count(), n);
        assert!((view.usable_fraction() - 1.0).abs() < 1e-12);
    }

    #[test]
    fn the_exclusion_mask_upsamples_the_working_flags_and_flags_the_margin() {
        // Region [10, 30) squared at 10 working pixels: two full-resolution pixels per cell.
        let grid = WorkingGrid::fit([10, 10, 30, 30], 10).unwrap();
        assert!((grid.scale - 2.0).abs() < 1e-12);
        let mut working = PixelMask::new(grid.width, grid.height);
        working.set(0, 0, flag::EDGE_BAND);
        let full = full_resolution_exclusion(&grid, &working, 40, 40);
        assert_eq!((full.width(), full.height()), (40, 40));
        // The flagged cell covers (10..12, 10..12); its neighbours are clear.
        for (x, y) in [(10, 10), (11, 10), (10, 11), (11, 11)] {
            assert!(full.has(x, y, flag::EDGE_BAND), "({x}, {y})");
        }
        assert!(full.is_clear(12, 10) && full.is_clear(20, 20));
        // Outside the region.
        assert!(full.has(5, 5, flag::OUTSIDE_OUTLINE));
        assert!(full.has(35, 20, flag::OUTSIDE_OUTLINE));
    }

    /// F1: a frame that is mostly flat backlight with a textured stone gives the stone region's
    /// compression estimate once the silhouette is masked; unmasked, the flat pixels drag the
    /// robust spread to zero.
    #[test]
    fn the_stone_compression_estimate_ignores_the_flat_background() {
        use indicatrix_cut_core::rough_plan::photometry::{
            NoiseModel, SourceKind, estimate_stone_compression_sigma, srgb_to_linear,
        };
        let noise = NoiseModel {
            a: [1e-12; 3],
            b: [0.0; 3],
        };
        let (w, h) = (60_usize, 60_usize);
        let (x0, y0, side) = (20_usize, 20_usize, 20_usize);
        // Deterministic uniform noise of about +-3 codes in the encoded domain on the stone.
        let mut seed = 0x2545_F491_4F6C_DD1D_u64;
        let mut stone_pixels = Vec::new();
        for _ in 0..side * side {
            let mut p = [0.0_f32; 3];
            for v in &mut p {
                seed = seed
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                let unit = ((seed >> 40) as f32 / (1_u64 << 24) as f32).mul_add(2.0, -1.0);
                *v = srgb_to_linear((0.5 + unit * 3.0 / 255.0).clamp(0.0, 1.0));
            }
            stone_pixels.push(p);
        }
        let flat = srgb_to_linear(0.4);
        let mut pixels = vec![[flat; 3]; w * h];
        for y in 0..side {
            for x in 0..side {
                pixels[(y0 + y) * w + x0 + x] = stone_pixels[y * side + x];
            }
        }
        let frame = LinearImage::from_pixels(w, h, pixels, SourceKind::Rgb8).unwrap();
        let crop = LinearImage::from_pixels(side, side, stone_pixels, SourceKind::Rgb8).unwrap();
        // Everything outside the stone is flagged, as the silhouette mask does.
        let mut exclusion = PixelMask::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let inside = (x0..x0 + side).contains(&x) && (y0..y0 + side).contains(&y);
                if !inside {
                    exclusion.set(x, y, flag::OUTSIDE_OUTLINE);
                }
            }
        }
        let alone = estimate_stone_compression_sigma(&crop, &noise, None, 0.98).unwrap();
        let masked =
            estimate_stone_compression_sigma(&frame, &noise, Some(&exclusion), 0.98).unwrap();
        let unmasked = estimate_stone_compression_sigma(&frame, &noise, None, 0.98).unwrap();
        for c in 0..3 {
            assert!(alone[c] > 1e-3, "the test texture must show: {alone:?}");
            assert!(
                (masked[c] - alone[c]).abs() <= 0.1 * alone[c],
                "channel {c}: masked {} against the stone alone {}",
                masked[c],
                alone[c]
            );
            assert!(
                unmasked[c] < 0.1 * alone[c],
                "channel {c}: the unmasked estimate {} should collapse",
                unmasked[c]
            );
        }
    }

    #[test]
    fn the_fallback_calibration_works_without_files() {
        let calibration = build_calibration(
            &CameraChoice::Fallback,
            &BacklightChoice::Led(LedKind::B3),
            [1.0, 1.0, 1.0],
        )
        .unwrap();
        assert!(calibration.notes.iter().any(|n| n.contains("LED-B3")));
        assert!(calibration.notes.iter().any(|n| n.contains("fallback")));
    }

    #[test]
    fn the_automatic_backlight_picks_an_led_from_a_white_frame() {
        let camera = CameraResponse::from_srgb().unwrap();
        let backlight = BacklightSpectrum::from_led(LedKind::B5).unwrap();
        let white = backlight.camera_rgb(&camera);
        let calibration =
            build_calibration(&CameraChoice::Fallback, &BacklightChoice::Auto, white).unwrap();
        assert!(calibration.notes[0].starts_with("Backlight: closest CIE LED is"));
    }

    #[test]
    fn missing_files_and_bad_whites_are_sentences() {
        let missing = PathBuf::from("/no/such/file.csv");
        let e = build_calibration(
            &CameraChoice::Curves(missing.clone()),
            &BacklightChoice::Led(LedKind::B3),
            [1.0; 3],
        )
        .unwrap_err();
        assert!(e.starts_with("Could not read"));
        let e = build_calibration(
            &CameraChoice::Fallback,
            &BacklightChoice::Csv(missing),
            [1.0; 3],
        )
        .unwrap_err();
        assert!(e.starts_with("Could not read"));
        let e = build_calibration(&CameraChoice::Fallback, &BacklightChoice::Auto, [0.0; 3])
            .unwrap_err();
        assert!(e.starts_with("Backlight from the white frame"));
    }
}
