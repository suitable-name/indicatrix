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
    /// The backlight flat field of the view.
    pub white_frame: WhiteFrame,
    /// The mean corrected white frame colour (white minus dark), for the backlight fit.
    pub white_rgb: [f64; 3],
}

impl PreparedView {
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
    let reference = if white_rgb.iter().all(|v| *v > 0.0) {
        white_rgb
    } else {
        [1.0; 3]
    };
    let white_frame = WhiteFrame::from_calibration(&calibration.frames, reference, 512)
        .map_err(|e| fail(&format!("{e:?}")))?;

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
    let mut transmittance = calibration
        .transmittance(&stone, &TransmittanceOptions::default())
        .map_err(|e| fail(&e))?;
    if let Some(extra) = &extra {
        let _ = transmittance.mask.merge(extra);
    }
    drop(stone);
    let scene = data.scene();
    let region = stone_region(&scene, view, 0.05)
        .ok_or_else(|| fail(&"the stone is outside this camera's picture"))?;
    let grid = WorkingGrid::fit(region, DEFAULT_WORKING_PX).map_err(|e| fail(&e))?;
    let observed = transmittance
        .to_working(&grid, &ResampleOptions::default())
        .map_err(|e| fail(&e))?;
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
        white_frame,
        white_rgb,
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
                "Camera: sRGB matrix fallback. Colours are less certain; load curves or a filter set for better ones."
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
