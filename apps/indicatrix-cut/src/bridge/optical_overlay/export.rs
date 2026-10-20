//! Tiled export composition after denoise/tone mapping, with frozen settings and pose.
//!
//! The pass traces 64-row tiles of the output, four samples per pixel. Tiles are
//! independent, so they are traced on every available core; each tile is composited into
//! its own rows of the picture, which makes the result identical to a sequential run.
use crate::bridge::export_thread::SceneSnapshot;
use glam::{Mat3, Vec3};
use indicatrix::{
    color::{
        ColorSpace, ToneMap,
        metrics::{
            MAX_DIAGNOSTIC_PIXELS, OPTICAL_MODEL_VERSION, OpticalImage, OpticalImageError,
            OpticalImageRequest, SCINTILLATION_WINDOW_RADIUS, evaluate_optical_image,
        },
    },
    geometry::tool::StoneGeometry,
    optics::raytracer::{Camera, DEFAULT_FOV_DEG, EnvironmentSource, LightingPreset},
    renderer::optical_overlay::{OpticalPresentation, composite_optical, optical_rgba},
};
use std::{
    num::NonZeroUsize,
    sync::{
        Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
};

/// The tallest tile, in output rows (further limited by [`MAX_DIAGNOSTIC_PIXELS`]).
const MAX_TILE_ROWS: usize = 64;

/// The finished beauty picture a diagnostic pass is composited onto.
pub struct Picture<'a> {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// The space `rgba` is encoded in; the overlay palette is converted to it.
    pub color_space: ColorSpace,
    /// `width * height * 4` bytes, tone-mapped and encoded. Changed in place.
    pub rgba: &'a mut [u8],
}

/// Why `scene` cannot be exported with optical diagnostics, or `None` when it can (or when it
/// does not ask for them). Check this before any beauty render is queued: [`apply`] repeats
/// the check as its last line of defence, but by then the picture is already rendered.
#[must_use]
pub fn refusal(scene: &SceneSnapshot) -> Option<&'static str> {
    let style = scene.optical_diagnostics?;
    if scene.env_map.is_none() && scene.lighting_preset == LightingPreset::Aset {
        Some("Optical Diagnostics cannot be combined with ASET")
    } else if scene.active_planes.is_empty() {
        Some("Optical Diagnostics need a stone with facets")
    } else if !style.map_mode && !style.scintillation && !style.layers.iter().any(|on| *on) {
        Some("Select an Optical Diagnostics layer or the Diagnostic Map before exporting")
    } else {
        None
    }
}

/// The lighting the diagnostic classes are scored under: the scene's HDR map, or its studio
/// rig. The metrics probe at a fixed exposure, so exposure is not part of it. One definition
/// for the live overlay, the tilt hover preview and the exports. The scene's tray radiance
/// is carried through but never read by the metrics (a window stays a window whatever lies
/// below the stone).
#[must_use]
pub fn diagnostic_environment(scene: &SceneSnapshot) -> EnvironmentSource<'_> {
    scene.env_map.as_deref().map_or_else(
        || {
            scene
                .lighting_preset
                .studio(1.0, scene.light_yaw, scene.light_pitch)
                .with_backdrop(scene.backdrop)
                .with_surface_glare(scene.surface_glare)
                .with_head_shadow(scene.head_shadow_deg)
        },
        EnvironmentSource::HdrMap,
    )
}

/// One plain sentence for a failed diagnostic image.
#[must_use]
pub const fn error_text(error: OpticalImageError) -> &'static str {
    match error {
        OpticalImageError::EmptyGeometry => "the stone has no geometry to analyze",
        OpticalImageError::InvalidInput => "the scene is not valid for the analysis",
        OpticalImageError::ResourceLimit => "the picture is too large for the analysis",
        OpticalImageError::Cancelled => "cancelled",
    }
}

/// How the pass is cut up and scheduled. The result must not depend on either number.
#[derive(Clone, Copy)]
struct Plan {
    tile_rows: usize,
    workers: usize,
}

/// Returns companion metadata; None means the opt-in was off and bytes were untouched.
///
/// `report` receives the finished fraction (0..=1) of the pass after each tile, on the
/// calling thread.
///
/// # Errors
///
/// A refusal ([`refusal`]), invalid dimensions, a failed analysis, or cancellation.
pub fn apply(
    scene: &SceneSnapshot,
    pose: [f32; 2],
    picture: Picture<'_>,
    cancel: &AtomicBool,
    report: &mut dyn FnMut(f32),
) -> Result<Option<String>, String> {
    let workers = thread::available_parallelism().map_or(1, NonZeroUsize::get);
    apply_planned(
        scene,
        pose,
        picture,
        cancel,
        report,
        Plan {
            tile_rows: MAX_TILE_ROWS,
            workers,
        },
    )
}

fn apply_planned(
    scene: &SceneSnapshot,
    pose: [f32; 2],
    picture: Picture<'_>,
    cancel: &AtomicBool,
    report: &mut dyn FnMut(f32),
    plan: Plan,
) -> Result<Option<String>, String> {
    let Some(style) = scene.optical_diagnostics else {
        return Ok(None);
    };
    if let Some(reason) = refusal(scene) {
        return Err(reason.into());
    }
    let Picture {
        width,
        height,
        color_space,
        rgba,
    } = picture;
    let row_bytes = (width as usize).checked_mul(4);
    if width == 0
        || height == 0
        || row_bytes.and_then(|row| row.checked_mul(height as usize)) != Some(rgba.len())
    {
        return Err("Invalid diagnostic export dimensions".into());
    }
    let row_bytes = row_bytes.unwrap_or(4);
    // Scintillation's spatial term reads a window of neighbouring pixels, so a tile is
    // traced with a few extra rows on each side (clipped to the picture) and only its own
    // rows are drawn: the result must not depend on how the picture is cut into tiles.
    let margin = if style.scintillation {
        SCINTILLATION_WINDOW_RADIUS
    } else {
        0
    };
    let tile_rows = (MAX_DIAGNOSTIC_PIXELS / (width as usize))
        .saturating_sub(2 * margin)
        .min(plan.tile_rows);
    if tile_rows == 0 {
        return Err("Diagnostic export exceeds tile memory budget".into());
    }
    let camera = Camera::new(pose[0], pose[1], scene.distance, DEFAULT_FOV_DEG);
    let environment = diagnostic_environment(scene);
    let render_tile = |y: usize, tile: &mut [u8]| -> Result<(), String> {
        let rows = tile.len() / row_bytes;
        let above = margin.min(y);
        let below = margin.min((height as usize).saturating_sub(y + rows));
        let request = OpticalImageRequest {
            camera: &camera,
            width,
            height: u32::try_from(rows + above + below).map_err(|e| e.to_string())?,
            samples: 4,
            projection_size: Some([width, height]),
            tile_origin: Some([0, u32::try_from(y - above).map_err(|e| e.to_string())?]),
            geometry: StoneGeometry {
                planes: &scene.active_planes,
                tools: &scene.tools,
            },
            material: &scene.material,
            environment,
            want_fire: style.layers[3],
            want_scintillation: style.scintillation,
            pose: Some([pose[0], pose[1], scene.distance]),
        };
        let map = evaluate_optical_image(&request, || cancel.load(Ordering::Acquire)).map_err(
            |e| match e {
                OpticalImageError::Cancelled => "Diagnostic export cancelled".to_string(),
                other => format!("Diagnostic export: {}", error_text(other)),
            },
        )?;
        compose_tile_rows(tile, &map, style, color_space, above * row_bytes);
        Ok(())
    };
    let tiles: Vec<(usize, &mut [u8])> = rgba
        .chunks_mut(tile_rows * row_bytes)
        .enumerate()
        .map(|(index, tile)| (index * tile_rows, tile))
        .collect();
    trace_tiles(tiles, plan.workers, &render_tile, report)?;
    Ok(Some(metadata(scene, Some(pose), width, height, style)))
}

/// Renders one tile: its first row index and its rows of the picture.
///
/// The trait object carries an explicit lifetime: a bare `dyn Trait` in a type alias defaults
/// to `'static`, which would demand that the renderer own everything it borrows (the scene,
/// the cancel flag). The scoped worker threads only need it to outlive the scope.
type TileRenderer<'a> = dyn Fn(usize, &mut [u8]) -> Result<(), String> + Sync + 'a;

/// Runs `render_tile` for every tile on up to `workers` threads, reporting the finished
/// fraction on the calling thread. The first failure stops the others and is returned.
fn trace_tiles(
    tiles: Vec<(usize, &mut [u8])>,
    workers: usize,
    render_tile: &TileRenderer<'_>,
    report: &mut dyn FnMut(f32),
) -> Result<(), String> {
    let tile_count = tiles.len();
    let queue = Mutex::new(tiles.into_iter());
    let failure: Mutex<Option<String>> = Mutex::new(None);
    let (done_tx, done_rx) = mpsc::channel::<()>();
    thread::scope(|scope| {
        for _ in 0..workers.clamp(1, tile_count.max(1)) {
            let done = done_tx.clone();
            let (queue, failure) = (&queue, &failure);
            scope.spawn(move || {
                loop {
                    if failure
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .is_some()
                    {
                        break;
                    }
                    let next = queue.lock().unwrap_or_else(PoisonError::into_inner).next();
                    let Some((y, tile)) = next else {
                        break;
                    };
                    match render_tile(y, tile) {
                        Ok(()) => {
                            if done.send(()).is_err() {
                                break;
                            }
                        }
                        Err(message) => {
                            failure
                                .lock()
                                .unwrap_or_else(PoisonError::into_inner)
                                .get_or_insert(message);
                            break;
                        }
                    }
                }
            });
        }
        drop(done_tx);
        let mut finished = 0_usize;
        while done_rx.recv().is_ok() {
            finished += 1;
            report(finished as f32 / tile_count as f32);
        }
    });
    failure
        .into_inner()
        .unwrap_or_else(PoisonError::into_inner)
        .map_or(Ok(()), Err)
}

/// Draws one analyzed tile onto its rows of the picture. The map may be traced with extra
/// rows above the tile: the first `skip` bytes of the overlay belong to those rows and are
/// not drawn.
fn compose_tile_rows(
    rows: &mut [u8],
    map: &OpticalImage,
    style: OpticalPresentation,
    color_space: ColorSpace,
    skip: usize,
) {
    let mut overlay = optical_rgba(map, style);
    if color_space != ColorSpace::Srgb {
        to_display_space(&mut overlay, color_space);
    }
    let overlay = overlay.get(skip..).unwrap_or_default();
    let len = overlay.len().min(rows.len());
    composite_optical(&mut rows[..len], &overlay[..len]);
}

/// Re-encodes an sRGB overlay palette into `color_space`. Fully transparent pixels carry no
/// color and are left alone.
fn to_display_space(overlay: &mut [u8], color_space: ColorSpace) {
    let rows = ColorSpace::Srgb.xyz_to_rgb_matrix();
    let rgb_to_xyz = Mat3::from_cols_array_2d(&rows).transpose().inverse();
    let tf = ColorSpace::Srgb.transfer_function();
    for pixel in overlay.as_chunks_mut::<4>().0 {
        if pixel[3] == 0 {
            continue;
        }
        let rgb = Vec3::new(
            f32::from(pixel[0]),
            f32::from(pixel[1]),
            f32::from(pixel[2]),
        ) / 255.0;
        let rgb = Vec3::new(tf.decode(rgb.x), tf.decode(rgb.y), tf.decode(rgb.z));
        let encoded = color_space.encode(rgb_to_xyz * rgb, ToneMap::None);
        pixel[..3].copy_from_slice(&encoded[..3]);
    }
}

/// The one legend file of a tilt video, written beside its frames: the same text as a still's
/// companion file, with the pose line saying the pose varies, then `sweep`. `None` when the
/// scene asks for no diagnostics.
///
/// It is also the resume key: frames already on disk may be kept only when this text is
/// unchanged, because every setting that shaped them is in it.
#[must_use]
pub fn sweep_legend(scene: &SceneSnapshot, width: u32, height: u32, sweep: &str) -> Option<String> {
    let style = scene.optical_diagnostics?;
    Some(format!(
        "{}{sweep}\n",
        metadata(scene, None, width, height, style)
    ))
}

/// The legend line of the scintillation layer (violet diamond pattern).
const SCINTILLATION_LEGEND: &str = "Scintillation (violet diamonds): flicker across +/-3 degrees of yaw and local contrast over 5 x 5 pixels, fixed scale 0-100; map domain, not the HUD value.\n";

/// The companion text file of a diagnostic export. `pose` is `None` for a video's legend,
/// which covers every frame.
fn metadata(
    scene: &SceneSnapshot,
    pose: Option<[f32; 2]>,
    width: u32,
    height: u32,
    style: OpticalPresentation,
) -> String {
    let pose_line = pose.map_or_else(
        || format!("Pose varies per frame; distance {:.5}.", scene.distance),
        |pose| {
            format!(
                "Pose yaw {:.7}, pitch {:.7} radians; distance {:.5}.",
                pose[0], pose[1], scene.distance
            )
        },
    );
    // Present only when the layer is on, so a legend without it (and the resume key a video
    // folder derives from it) is byte for byte what it was.
    let scintillation_line = if style.scintillation {
        SCINTILLATION_LEGEND
    } else {
        ""
    };
    format!(
        "Optical diagnostics model {OPTICAL_MODEL_VERSION}\nAnalytical light paths; Fire = dispersion potential, fixed range 0-0.1 weighted degrees.\nSampling: {width} x {height}, 4 deterministic samples/pixel, ten-bounce policy; finished geometry.\n{pose_line}\nMaterial: {}\nLighting: {}{}; head-shadow {:.3} degrees.\nLayers [windowing, extinction, brilliance, Fire]: {:?}; opacity {}; map mode {}; high contrast {}.\nLegend: cyan / windowing (light that leaves through the pavilion; the render shows the backdrop there); magenta X extinction; green dots returned; gold stipple dispersion potential.\nBase fractions count hit rays, not radiance; Fire is independent. Unknown observations are retained.\nGeometric estimate: body absorption, zoning, polarization, birefringence, fluorescence, scattering, frosting, rounding and surface glare are not reproduced. Bounce-budget termination is an estimate, not proven darkness.\n{scintillation_line}",
        scene.material.name,
        scene.lighting_preset.label(),
        if scene.env_map.is_some() {
            " (HDR replaces preset)"
        } else {
            ""
        },
        scene.head_shadow_deg,
        style.layers,
        style.opacity,
        style.map_mode,
        style.high_contrast
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::render_thread::RenderContext;
    use indicatrix::color::metrics::OpticalPixel;
    use std::sync::Mutex;

    fn picture(width: u32, height: u32, rgba: &mut [u8]) -> Picture<'_> {
        Picture {
            width,
            height,
            color_space: ColorSpace::Srgb,
            rgba,
        }
    }

    fn default_scene() -> SceneSnapshot {
        SceneSnapshot::capture(&Mutex::new(RenderContext::default())).unwrap()
    }

    #[test]
    fn clean_export_returns_before_diagnostic_validation_or_allocation() {
        let scene = default_scene();
        let mut bytes = [11, 22, 33, 44];
        let cancel = AtomicBool::new(true);
        let mut reports = 0;
        assert_eq!(
            apply(
                &scene,
                [0.0, 0.0],
                picture(u32::MAX, u32::MAX, &mut bytes),
                &cancel,
                &mut |_| reports += 1
            )
            .unwrap(),
            None
        );
        assert_eq!(bytes, [11, 22, 33, 44]);
        assert_eq!(reports, 0);
    }

    #[test]
    fn diagnostic_export_has_explicit_quality_metadata_and_preserves_transparency() {
        let mut scene = default_scene();
        scene.optical_diagnostics = Some(OpticalPresentation {
            layers: [true; 4],
            ..OpticalPresentation::default()
        });
        let mut bytes = vec![0; 16 * 16 * 4];
        let cancel = AtomicBool::new(false);
        let text = apply(
            &scene,
            [scene.yaw, scene.pitch],
            picture(16, 16, &mut bytes),
            &cancel,
            &mut |_| {},
        )
        .unwrap()
        .unwrap();
        assert!(text.contains("4 deterministic samples/pixel"));
        assert!(text.contains("dispersion potential"));
        assert!(bytes.iter().all(|b| *b == 0));
        scene.lighting_preset = LightingPreset::Aset;
        assert!(
            apply(
                &scene,
                [0.0, 0.0],
                picture(16, 16, &mut bytes),
                &cancel,
                &mut |_| {},
            )
            .is_err()
        );
    }

    /// The tiles are traced on several threads and in different heights; the picture must
    /// not depend on either, and the overlay must land on opaque pixels.
    #[test]
    fn parallel_tiles_match_the_sequential_reference_and_report_progress() {
        let mut scene = default_scene();
        scene.optical_diagnostics = Some(OpticalPresentation {
            layers: [true; 4],
            opacity: 1.0,
            ..OpticalPresentation::default()
        });
        let pose = [scene.yaw, scene.pitch];
        let cancel = AtomicBool::new(false);
        let (width, height) = (64_u32, 40_u32);
        let run = |plan: Plan, fractions: &mut Vec<f32>| {
            let mut bytes = vec![255; 64 * 40 * 4];
            apply_planned(
                &scene,
                pose,
                picture(width, height, &mut bytes),
                &cancel,
                &mut |fraction| fractions.push(fraction),
                plan,
            )
            .unwrap();
            bytes
        };
        let mut sequential_progress = Vec::new();
        let sequential = run(
            Plan {
                tile_rows: 64,
                workers: 1,
            },
            &mut sequential_progress,
        );
        let mut parallel_progress = Vec::new();
        let parallel = run(
            Plan {
                tile_rows: 7,
                workers: 4,
            },
            &mut parallel_progress,
        );
        assert!(
            sequential != vec![255; 64 * 40 * 4],
            "the overlay must change opaque beauty pixels"
        );
        assert_eq!(sequential, parallel);
        assert_eq!(sequential_progress, vec![1.0]);
        assert_eq!(parallel_progress.len(), 6, "ceil(40 / 7) tiles");
        assert!(parallel_progress.windows(2).all(|pair| pair[0] < pair[1]));
        assert!((parallel_progress[5] - 1.0).abs() < f32::EPSILON);
    }

    /// Scintillation reads a window of neighbouring pixels; a tile is traced with extra rows
    /// so the picture still does not depend on the tile height or the thread count.
    #[test]
    fn scintillation_tiles_match_the_sequential_reference() {
        let mut scene = default_scene();
        let style = OpticalPresentation {
            layers: [true, false, false, false],
            opacity: 1.0,
            scintillation: true,
            ..OpticalPresentation::default()
        };
        scene.optical_diagnostics = Some(style);
        let pose = [scene.yaw, scene.pitch];
        let cancel = AtomicBool::new(false);
        let (width, height) = (64_u32, 40_u32);
        let run = |scene: &SceneSnapshot, plan: Plan| {
            let mut bytes = vec![255; 64 * 40 * 4];
            apply_planned(
                scene,
                pose,
                picture(width, height, &mut bytes),
                &cancel,
                &mut |_| {},
                plan,
            )
            .unwrap();
            bytes
        };
        let sequential = run(
            &scene,
            Plan {
                tile_rows: 64,
                workers: 1,
            },
        );
        let tiled = run(
            &scene,
            Plan {
                tile_rows: 7,
                workers: 4,
            },
        );
        assert_eq!(sequential, tiled);
        scene.optical_diagnostics = Some(OpticalPresentation {
            scintillation: false,
            ..style
        });
        let without = run(
            &scene,
            Plan {
                tile_rows: 64,
                workers: 1,
            },
        );
        assert_ne!(sequential, without, "the violet diamonds must be drawn");
    }

    #[test]
    fn scintillation_alone_is_a_valid_layer_choice_and_the_legend_names_it() {
        let mut scene = default_scene();
        scene.optical_diagnostics = Some(OpticalPresentation {
            layers: [false; 4],
            scintillation: true,
            ..OpticalPresentation::default()
        });
        assert_eq!(refusal(&scene), None);
        let with = sweep_legend(&scene, 8, 8, "Sweep").unwrap();
        assert!(with.contains("Scintillation (violet diamonds)"), "{with}");
        assert!(with.contains("not the HUD value"), "{with}");
        // Without the layer the legend (the video resume key) is exactly the former text.
        scene.optical_diagnostics = Some(OpticalPresentation {
            layers: [false; 4],
            map_mode: true,
            ..OpticalPresentation::default()
        });
        let without = sweep_legend(&scene, 8, 8, "Sweep").unwrap();
        assert!(!without.contains("Scintillation"), "{without}");
        assert!(
            without.ends_with("not proven darkness.\nSweep\n"),
            "{without}"
        );
    }

    #[test]
    fn refusals_name_aset_empty_stones_and_empty_layer_choices() {
        let mut scene = default_scene();
        assert_eq!(refusal(&scene), None, "no opt-in, nothing to refuse");
        scene.optical_diagnostics = Some(OpticalPresentation::default());
        assert_eq!(refusal(&scene), None);
        let style = scene.optical_diagnostics.as_mut().unwrap();
        style.layers = [false; 4];
        assert!(refusal(&scene).unwrap().contains("layer"));
        scene.optical_diagnostics.as_mut().unwrap().map_mode = true;
        assert_eq!(refusal(&scene), None, "the map alone is a valid choice");
        scene.lighting_preset = LightingPreset::Aset;
        assert!(refusal(&scene).unwrap().contains("ASET"));
        scene.optical_diagnostics = None;
        assert_eq!(refusal(&scene), None);
    }

    #[test]
    fn the_video_legend_has_no_pose_and_changes_with_every_setting() {
        let mut scene = default_scene();
        assert_eq!(sweep_legend(&scene, 8, 8, "Sweep"), None);
        scene.optical_diagnostics = Some(OpticalPresentation::default());
        let base = sweep_legend(&scene, 8, 8, "Sweep axis 0").unwrap();
        assert!(base.contains("Pose varies per frame"));
        assert!(base.ends_with("Sweep axis 0\n"));
        for changed in [
            OpticalPresentation {
                opacity: 0.8,
                ..OpticalPresentation::default()
            },
            OpticalPresentation {
                layers: [true, true, false, false],
                ..OpticalPresentation::default()
            },
            OpticalPresentation {
                map_mode: true,
                ..OpticalPresentation::default()
            },
            OpticalPresentation {
                high_contrast: true,
                ..OpticalPresentation::default()
            },
        ] {
            scene.optical_diagnostics = Some(changed);
            assert_ne!(sweep_legend(&scene, 8, 8, "Sweep axis 0").unwrap(), base);
        }
    }

    fn one_pixel_map() -> OpticalImage {
        OpticalImage {
            width: 1,
            height: 1,
            samples: 4,
            fire_computed: false,
            scint_computed: false,
            // x + y = 1: off the 7-pixel hatch, so the palette color is unshaded.
            origin: [1, 0],
            pixels: vec![OpticalPixel {
                hits: 4,
                classes: [4, 0, 0, 0],
                ..OpticalPixel::default()
            }],
            total: OpticalPixel::default(),
        }
    }

    fn windowing_only() -> OpticalPresentation {
        OpticalPresentation {
            layers: [true, false, false, false],
            opacity: 1.0,
            map_mode: false,
            high_contrast: false,
            scintillation: false,
        }
    }

    fn composed(color_space: ColorSpace) -> [u8; 4] {
        let mut pixel = [0, 0, 0, 255];
        compose_tile_rows(
            &mut pixel,
            &one_pixel_map(),
            windowing_only(),
            color_space,
            0,
        );
        pixel
    }

    #[test]
    fn palette_conversion_is_identity_for_srgb_and_moves_primaries_for_display_p3() {
        let srgb = composed(ColorSpace::Srgb);
        assert_eq!(srgb, [0, 166, 255, 255], "the windowing palette byte");

        let p3 = composed(ColorSpace::DisplayP3);
        assert!(
            p3[0] > 0,
            "sRGB cyan has a red component in the wider gamut"
        );
        assert_ne!(p3[..3], srgb[..3]);
        assert_eq!(p3[3], 255);

        let aces = composed(ColorSpace::AcesCg);
        assert!(
            aces[1] < srgb[1],
            "a linear transfer encodes the mid green lower than sRGB: {aces:?} vs {srgb:?}"
        );
    }

    #[test]
    fn transparent_overlay_pixels_are_not_converted() {
        let mut overlay = [9, 8, 7, 0, 0, 166, 255, 255];
        to_display_space(&mut overlay, ColorSpace::DisplayP3);
        assert_eq!(overlay[..4], [9, 8, 7, 0]);
        assert_ne!(overlay[4..7], [0, 166, 255]);
    }
}
