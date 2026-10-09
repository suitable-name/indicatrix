//! The window-free work of the Fit step: the forward and fit inputs from the prepared views,
//! the fit with its display records, the zone-geometry refinement, zone suggestions and zones
//! from marks.
//!
//! Runs on worker threads.

use super::work::{Calibration, PreparedView, SceneData};
use indicatrix::optics::zoning::{Zone, ZoneShape, ZonedAbsorption};
use indicatrix_cut_core::rough_plan::{
    colour_fit::{
        ColourRig,
        forward::{
            ForwardInput, ForwardOptions, PanelGeom, RigLighting, StoneIndex, SurfaceClass,
            SurfaceMap, ViewPrediction, ViewTraceInput, WhiteFrame, evaluate, trace_rig,
        },
        solve::{
            ColourFit, FitConfig, FitInputs, FitProgress, ModelKind, ObservedView, RoughnessSearch,
            fit_colour,
        },
        zones::{
            BoundaryPoints, FittedZoneShape, RadialRole, RefineOptions, RefineProgress,
            RefineResult, SuggestOptions, SuggestView, ZoneLocks, ZoneSuggestion, fit_cylinder,
            fit_half_space, fit_prism, fit_sector, fit_slab, refine_zone_geometry, suggest_zones,
        },
    },
    locate::{LocateOptions, ViewPolyline},
    photometry::{PixelMask, WorkingGrid},
};
use std::{fmt::Write as _, path::PathBuf, sync::atomic::AtomicBool};

use super::zone_rows::ShapeKind;

/// How the stone's surface is modelled.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SurfaceChoice {
    /// Every triangle polished.
    Polished,
    /// Frosted with this GGX alpha.
    Frosted(f32),
    /// Frosted; the roughness is searched.
    FrostedAuto,
}

/// What the window's earlier steps decided, owned for a worker.
#[derive(Debug, Clone)]
pub struct FitSetup {
    /// Mesh, rig, alignment.
    pub data: SceneData,
    /// The prepared views (only the ones to fit).
    pub views: Vec<PreparedView>,
    /// Camera and backlight.
    pub calibration: Calibration,
    /// The surface model.
    pub surface: SurfaceChoice,
    /// Triangles painted as polished windows.
    pub windows: Vec<u32>,
    /// How far behind the stone the backlight panel is, mm.
    pub panel_setback_mm: f64,
    /// The panel's side, mm.
    pub panel_size_mm: f64,
    /// The disk cache of the traces.
    pub cache_dir: Option<PathBuf>,
    /// The host species (only with the physical colour feature).
    pub host_id: Option<String>,
    /// The planned stone width, mm.
    pub planned_mm: f64,
}

/// A fit and what the Compare step shows.
#[derive(Debug, Clone)]
pub struct FitOutput {
    /// The result.
    pub fit: ColourFit,
    /// The zones with the fitted absorptions (mesh frame, mm).
    pub zoned: ZonedAbsorption,
    /// The render of every fitted view.
    pub predictions: Vec<ViewPrediction>,
    /// The gain of every fitted view `(rig view, gain)`.
    pub gains: Vec<(usize, f32)>,
}

/// The surface map for a choice and alpha, with the windows polished.
#[must_use]
pub fn surface_map(choice: SurfaceChoice, alpha: Option<f32>, windows: &[u32]) -> SurfaceMap {
    let mut map = match (choice, alpha) {
        (SurfaceChoice::Polished, _) => SurfaceMap::polished(),
        (_, Some(a)) | (SurfaceChoice::Frosted(a), None) => SurfaceMap::frosted(a),
        (SurfaceChoice::FrostedAuto, None) => SurfaceMap::frosted(0.2),
    };
    if !matches!(choice, SurfaceChoice::Polished) {
        for &t in windows {
            map = map.with_override(t, SurfaceClass::Polished);
        }
    }
    map
}

struct Built {
    rig: ColourRig,
    observed: Vec<ObservedView>,
    masks: Vec<PixelMask>,
    grids: Vec<WorkingGrid>,
    view_ids: Vec<usize>,
}

fn build(setup: &FitSetup) -> Result<Built, String> {
    let rig = &setup.data.rig;
    let mut panels = Vec::with_capacity(rig.views.len());
    let mut whites: Vec<Option<WhiteFrame>> = vec![None; rig.views.len()];
    for pose in &rig.views {
        let distance = pose.position_vec().length() + setup.panel_setback_mm;
        panels.push(PanelGeom::facing_camera(
            pose,
            distance,
            [setup.panel_size_mm, setup.panel_size_mm],
        ));
    }
    let mut observed = Vec::new();
    let mut masks = Vec::new();
    let mut grids = Vec::new();
    let mut view_ids = Vec::new();
    for prepared in &setup.views {
        if prepared.view >= whites.len() {
            return Err(format!("{}: the rig has no such view.", prepared.name));
        }
        whites[prepared.view] = Some(prepared.white_frame.clone());
        let mask = prepared.combined_mask();
        let mut view = ObservedView::from_resampled(prepared.view, &prepared.observed);
        for (i, var) in view.variance.iter_mut().enumerate() {
            if mask.get(i % mask.width(), i / mask.width()) != 0 {
                *var = [f32::INFINITY; 3];
            }
        }
        observed.push(view);
        masks.push(mask);
        grids.push(prepared.grid);
        view_ids.push(prepared.view);
    }
    if observed.len() < 2 {
        return Err("Fit needs at least two prepared views.".to_owned());
    }
    let lighting = RigLighting::backlight(panels, whites);
    Ok(Built {
        rig: ColourRig::new(rig.clone(), lighting),
        observed,
        masks,
        grids,
        view_ids,
    })
}

fn with_input<R>(
    setup: &FitSetup,
    built: &Built,
    surfaces: &SurfaceMap,
    zones: Option<&ZonedAbsorption>,
    config: &FitConfig,
    roughness_surfaces: Option<&(dyn Fn(f32) -> SurfaceMap + Sync)>,
    run: impl FnOnce(&FitInputs<'_>) -> R,
) -> R {
    let views: Vec<ViewTraceInput<'_>> = built
        .view_ids
        .iter()
        .zip(&built.grids)
        .zip(&built.masks)
        .map(|((&view, &grid), mask)| ViewTraceInput::new(view, grid).with_mask(mask))
        .collect();
    let options = ForwardOptions::default();
    let index = StoneIndex::Rig;
    let forward = ForwardInput {
        mesh: &setup.data.mesh,
        alignment: setup.data.alignment,
        rig: &built.rig,
        surfaces,
        index: &index,
        zones,
        inclusions: &setup.data.shells,
        camera: &setup.calibration.camera,
        backlight: &setup.calibration.backlight,
        views: &views,
        options: &options,
        cache_dir: setup.cache_dir.as_deref(),
    };
    let inputs = FitInputs {
        forward,
        observed: &built.observed,
        config,
        surfaces_for_roughness: roughness_surfaces,
    };
    run(&inputs)
}

fn config_for(setup: &FitSetup) -> FitConfig {
    FitConfig {
        host_id: setup.host_id.clone(),
        preferred_model: setup.host_id.is_none().then_some(ModelKind::SmoothBasis),
        roughness: matches!(setup.surface, SurfaceChoice::FrostedAuto)
            .then(RoughnessSearch::default),
        prediction: indicatrix_cut_core::rough_plan::colour_fit::solve::PredictionConfig {
            caller_mm: setup.planned_mm,
            ..Default::default()
        },
        ..FitConfig::default()
    }
}

/// Whether the setup searches the roughness (for the progress weights).
#[must_use]
pub const fn searches_roughness(setup: &FitSetup) -> bool {
    matches!(setup.surface, SurfaceChoice::FrostedAuto)
}

/// The zones with the fitted absorptions: base first, then each zone of `geometry`.
#[must_use]
pub fn assemble_zoned(geometry: &ZonedAbsorption, fit: &ColourFit) -> Option<ZonedAbsorption> {
    let absorptions = fit.zone_absorptions();
    let mut zoned = geometry.clone();
    zoned.base = absorptions.first()?.clone();
    for (i, zone) in zoned.zones.iter_mut().enumerate() {
        zone.absorption = absorptions.get(i + 1)?.clone();
    }
    Some(zoned)
}

/// Runs the whole fit and renders the result for the Compare step.
///
/// # Errors
///
/// A sentence for the window.
pub fn run_fit(
    setup: &FitSetup,
    geometry: &ZonedAbsorption,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(FitProgress),
) -> Result<FitOutput, String> {
    let built = build(setup)?;
    let config = config_for(setup);
    let zones = (!geometry.zones.is_empty()).then_some(geometry);
    let windows = setup.windows.clone();
    let choice = setup.surface;
    let rough_fn = move |alpha: f32| surface_map(choice, Some(alpha), &windows);
    let base_alpha = match choice {
        SurfaceChoice::Frosted(a) => Some(a),
        _ => None,
    };
    let surfaces = surface_map(choice, base_alpha, &setup.windows);
    let search = matches!(choice, SurfaceChoice::FrostedAuto);
    let fit = with_input(
        setup,
        &built,
        &surfaces,
        zones,
        &config,
        search.then_some(&rough_fn as &(dyn Fn(f32) -> SurfaceMap + Sync)),
        |inputs| fit_colour(inputs, cancel, progress),
    )
    .map_err(|e| format!("The fit failed: {e:?}"))?;

    // The picture of the result: trace again at the roughness the fit chose (cached on disk).
    let shown = surface_map(
        choice,
        fit.roughness.as_ref().map(|r| r.roughness).or(base_alpha),
        &setup.windows,
    );
    let absorptions = fit.zone_absorptions();
    let predictions = with_input(setup, &built, &shown, zones, &config, None, |inputs| {
        trace_rig(&inputs.forward, cancel, &mut |_| {}).map(|records| {
            evaluate(&records, &|zone, lambda| {
                absorptions.get(zone).map_or(0.0, |z| z.alpha(lambda, None))
            })
        })
    })
    .map_err(|e| format!("The render of the result failed: {e:?}"))?;
    let gains = fit
        .chosen_fit()
        .view_ids
        .iter()
        .copied()
        .zip(fit.chosen_fit().gains())
        .map(|(v, g)| (v, g as f32))
        .collect();
    let zoned = assemble_zoned(geometry, &fit)
        .ok_or_else(|| "The fit returned fewer zones than the geometry has.".to_owned())?;
    Ok(FitOutput {
        fit,
        zoned,
        predictions,
        gains,
    })
}

/// Refines the zone geometry (`options.max_iterations` rounds at most).
///
/// # Errors
///
/// A sentence for the window.
pub fn run_refine(
    setup: &FitSetup,
    geometry: &ZonedAbsorption,
    locks: &ZoneLocks,
    options: &RefineOptions,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(RefineProgress),
) -> Result<RefineResult, String> {
    let built = build(setup)?;
    let mut config = config_for(setup);
    config.roughness = None;
    let alpha = match setup.surface {
        SurfaceChoice::Frosted(a) => Some(a),
        _ => None,
    };
    let surfaces = surface_map(setup.surface, alpha, &setup.windows);
    with_input(setup, &built, &surfaces, None, &config, None, |inputs| {
        refine_zone_geometry(inputs, geometry, locks, options, cancel, progress)
    })
    .map_err(|e| format!("The refinement failed: {e:?}"))
}

/// Suggests zone boundaries from the fit's residual maps (or, without a fit, from the photos).
///
/// # Errors
///
/// A sentence for the window.
pub fn suggest(setup: &FitSetup, fit: Option<&ColourFit>) -> Result<Vec<ZoneSuggestion>, String> {
    let views: Vec<SuggestView> = setup
        .views
        .iter()
        .map(|prepared| {
            fit.and_then(|f| f.residuals.iter().find(|m| m.view == prepared.view))
                .map_or_else(
                    || {
                        let mut observed =
                            ObservedView::from_resampled(prepared.view, &prepared.observed);
                        let mask = prepared.combined_mask();
                        for (i, var) in observed.variance.iter_mut().enumerate() {
                            if mask.get(i % mask.width(), i / mask.width()) != 0 {
                                *var = [f32::INFINITY; 3];
                            }
                        }
                        SuggestView::from_observed(prepared.grid, &observed)
                    },
                    |map| SuggestView::from_residual(prepared.grid, map),
                )
        })
        .collect();
    let report = suggest_zones(&setup.data.scene(), &views, &SuggestOptions::default())
        .map_err(|e| format!("No suggestion: {e:?}"))?;
    Ok(report.suggestions)
}

/// A boundary fitted from marks: the shape and its quality line.
#[derive(Debug, Clone, PartialEq)]
pub struct MarkedZone {
    /// The shape (mesh frame).
    pub shape: ZoneShape,
    /// `"rms 0.08 mm, leave-one-view-out 0.12 mm"`.
    pub quality: String,
}

fn quality(fitted: &FittedZoneShape) -> String {
    let mut text = format!("rms {:.2} mm", fitted.rms_mm);
    if let Some(l) = fitted.leave_one_view_out_rms {
        let _ = write!(text, ", leave-one-view-out {l:.2} mm");
    }
    text
}

/// Fits a primitive to a boundary marked in the photos. `polylines` holds the marks of the
/// boundary (at least two views, the same vertices in the same order); a slab needs a second set.
///
/// # Errors
///
/// A sentence for the window.
pub fn zone_from_marks(
    setup: &FitSetup,
    kind: ShapeKind,
    polylines: &[ViewPolyline],
    second: &[ViewPolyline],
) -> Result<MarkedZone, String> {
    let scene = setup.data.scene();
    let locate = |lines: &[ViewPolyline]| {
        BoundaryPoints::locate(&scene, lines, false, &LocateOptions::default())
            .map_err(|e| format!("The marks could not be located: {e:?}"))
    };
    let points = locate(polylines)?;
    let fitted = match kind {
        ShapeKind::HalfSpace => fit_half_space(&points, None),
        ShapeKind::Slab => fit_slab(&points, &locate(second)?),
        ShapeKind::Cylinder => fit_cylinder(&points, None, RadialRole::Core),
        ShapeKind::Prism => fit_prism(&points, 3, None, RadialRole::Core),
        ShapeKind::Sector => fit_sector(&points, &locate(second)?, None, None),
    }
    .map_err(|e| format!("The boundary could not be fitted: {e:?}"))?;
    Ok(MarkedZone {
        quality: quality(&fitted),
        shape: fitted.shape,
    })
}

/// A zone from a shape, with the placeholder absorption.
#[must_use]
pub fn zone_of(shape: ZoneShape) -> Zone {
    super::zone_rows::placeholder_zone(shape)
}
