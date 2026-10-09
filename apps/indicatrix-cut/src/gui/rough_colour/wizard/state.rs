//! Everything the wizard remembers between steps, on the UI thread, and the pictures it derives
//! from it. No window types here: [`super::view_model`] pushes it, [`super::actions`] changes it.

use super::{
    context::{AlignmentSnapshot, PlannerContext},
    fitwork::{FitOutput, FitSetup, SurfaceChoice},
    raster::{self, Rgba},
    steps::{Step, StepFacts},
    work::{BacklightChoice, Calibration, CameraChoice, PreparedView, SceneData, ViewFiles},
    zone_rows,
};
use crate::gui::rough_colour::handles::{Handle, handles_for};
use glam::DVec2;
use indicatrix::{color::led::LedKind, optics::zoning::ZonedAbsorption};
use indicatrix_cut_core::rough_plan::{
    colour_fit::zones::{
        OverlayOptions, ZoneEdit, ZoneEditError, ZoneLocks, ZoneSuggestion, apply_with_locks,
        project_overlay,
    },
    locate::{RigProfile, ViewPolyline},
    photometry::PixelMask,
};
use std::{
    collections::BTreeSet,
    sync::{Arc, atomic::AtomicBool},
};

/// The default width of the mesh edge band in working pixels.
pub const DEFAULT_EDGE_BAND_PX: usize = 3;

/// The number of views assumed before a rig is picked.
pub const DEFAULT_VIEWS: usize = 8;

/// The colours of the zone outlines, by zone number.
pub const ZONE_COLOURS: [[u8; 3]; 5] = [
    [255, 255, 255],
    [0, 255, 255],
    [255, 200, 0],
    [255, 80, 200],
    [120, 255, 120],
];

/// How many zone edits the Undo button remembers.
pub const ZONE_UNDO_DEPTH: usize = 50;

/// The zones as they were before an edit: what the Undo button restores.
#[derive(Debug, Clone, PartialEq)]
pub struct ZoneSnapshot {
    /// The geometry.
    pub geometry: Option<ZonedAbsorption>,
    /// The locked parameters.
    pub locks: ZoneLocks,
    /// The selected zone.
    pub selected_zone: usize,
}

/// A running job.
pub struct Job {
    /// The ticket: a result with another ticket is dropped.
    pub ticket: u64,
    /// Raised by the Cancel button.
    pub cancel: Arc<AtomicBool>,
}

/// The marks of a zone boundary, per view, in full-resolution photo pixels.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Marks {
    /// One list of clicked points per view.
    pub views: Vec<Vec<[f64; 2]>>,
}

impl Marks {
    /// Makes room for `count` views.
    pub fn resize(&mut self, count: usize) {
        self.views.resize_with(count, Vec::new);
    }

    /// Adds a point to a view.
    pub fn click(&mut self, view: usize, pixel: [f64; 2]) {
        if let Some(points) = self.views.get_mut(view) {
            points.push(pixel);
        }
    }

    /// Removes the last point of a view.
    pub fn undo(&mut self, view: usize) {
        if let Some(points) = self.views.get_mut(view) {
            points.pop();
        }
    }

    /// Removes everything.
    pub fn clear(&mut self) {
        for points in &mut self.views {
            points.clear();
        }
    }

    /// The marked views as polylines for the locator.
    #[must_use]
    pub fn polylines(&self) -> Vec<ViewPolyline> {
        self.views
            .iter()
            .enumerate()
            .filter(|(_, points)| !points.is_empty())
            .map(|(view, points)| ViewPolyline {
                view,
                pixels: points.clone(),
            })
            .collect()
    }

    /// A line for the window: how many views have points and how many in total.
    #[must_use]
    pub fn summary(&self) -> String {
        let marked = self.views.iter().filter(|p| !p.is_empty()).count();
        let total: usize = self.views.iter().map(Vec::len).sum();
        match marked {
            0 => "No marks yet.".to_owned(),
            1 => format!(
                "{total} points in 1 view. Mark the same boundary in at least one more view."
            ),
            n => format!("{total} points in {n} views."),
        }
    }
}

/// The wizard's state.
pub struct State {
    /// The stored rigs.
    pub rigs: Vec<RigProfile>,
    /// The picked rig.
    pub rig_index: Option<usize>,
    /// The planner's rough.
    pub context: Option<PlannerContext>,
    /// The locate window's alignment.
    pub alignment: Option<AlignmentSnapshot>,
    /// The step shown.
    pub step: Step,
    /// The view on the picture.
    pub active_view: usize,
    /// The files of every view.
    pub files: Vec<ViewFiles>,
    /// The prepared views.
    pub prepared: Vec<Option<PreparedView>>,
    /// The camera choice.
    pub camera: CameraChoice,
    /// The backlight choice.
    pub backlight: BacklightChoice,
    /// The LED kind for the LED choice.
    pub led: LedKind,
    /// The calibration in effect.
    pub calibration: Option<Calibration>,
    /// The edge band in working pixels.
    pub edge_band_px: usize,
    /// The surface model.
    pub surface: SurfaceChoice,
    /// Triangles painted polished.
    pub windows: BTreeSet<u32>,
    /// The zone geometry (mesh frame, mm).
    pub geometry: Option<ZonedAbsorption>,
    /// The locked zone parameters.
    pub locks: ZoneLocks,
    /// The selected zone (0 = none).
    pub selected_zone: usize,
    /// The boundary marks.
    pub marks: Marks,
    /// The first boundary of a slab or sector while the second is being marked.
    pub pending_first: Option<Vec<ViewPolyline>>,
    /// The last suggestions.
    pub suggestions: Vec<ZoneSuggestion>,
    /// The fit and its renders.
    pub fit: Option<FitOutput>,
    /// The centre of the compare zoom, as fractions.
    pub compare_centre: [f64; 2],
    /// The running job.
    pub job: Option<Job>,
    /// The zones before the 3D handle drag that is under way, if one is.
    pub drag_origin: Option<ZoneSnapshot>,
    /// The zones before each earlier edit, newest last (at most [`ZONE_UNDO_DEPTH`]).
    pub zone_undo: Vec<ZoneSnapshot>,
    /// The scene data, built from the context, rig and alignment.
    scene: Option<Arc<SceneData>>,
    /// The outlines of the zones in every view, in working pixels of that view.
    overlay: Option<Vec<ViewOverlay>>,
}

/// The outline polylines of one view's zones, as `(zone, polyline)` pairs.
type ViewOverlay = Vec<(usize, Vec<[f64; 2]>)>;

impl State {
    /// A fresh state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            rigs: Vec::new(),
            rig_index: None,
            context: None,
            alignment: None,
            step: Step::Checklist,
            active_view: 0,
            files: vec![ViewFiles::default(); DEFAULT_VIEWS],
            prepared: (0..DEFAULT_VIEWS).map(|_| None).collect(),
            camera: CameraChoice::Fallback,
            backlight: BacklightChoice::Auto,
            led: LedKind::B3,
            calibration: None,
            edge_band_px: DEFAULT_EDGE_BAND_PX,
            surface: SurfaceChoice::Polished,
            windows: BTreeSet::new(),
            geometry: None,
            locks: ZoneLocks::new(),
            selected_zone: 0,
            marks: Marks::default(),
            pending_first: None,
            suggestions: Vec::new(),
            fit: None,
            compare_centre: [0.5, 0.5],
            job: None,
            drag_origin: None,
            zone_undo: Vec::new(),
            scene: None,
            overlay: None,
        }
    }

    /// The picked rig.
    #[must_use]
    pub fn rig(&self) -> Option<&RigProfile> {
        self.rig_index.and_then(|i| self.rigs.get(i))
    }

    /// The number of views.
    #[must_use]
    pub fn view_count(&self) -> usize {
        self.rig().map_or(DEFAULT_VIEWS, |rig| rig.views.len())
    }

    /// The name of a view.
    #[must_use]
    pub fn view_name(&self, view: usize) -> String {
        self.rig()
            .and_then(|rig| rig.views.get(view))
            .map_or_else(|| format!("View {}", view + 1), |pose| pose.name.clone())
    }

    /// Makes every per-view list match the rig's view count.
    pub fn sync_views(&mut self) {
        let count = self.view_count();
        self.files.resize_with(count, ViewFiles::default);
        self.prepared.resize_with(count, || None);
        self.marks.resize(count);
        self.active_view = self.active_view.min(count.saturating_sub(1));
    }

    /// Forgets everything computed from the rig, alignment or photos.
    pub fn forget_computed(&mut self) {
        self.scene = None;
        self.overlay = None;
        self.prepared.fill(None);
        self.calibration = None;
        self.fit = None;
        self.suggestions.clear();
    }

    /// Forgets the fit (an input of it changed).
    pub fn forget_fit(&mut self) {
        self.fit = None;
    }

    /// Forgets the zone outlines (the geometry changed).
    pub fn forget_overlay(&mut self) {
        self.overlay = None;
    }

    /// Whether the locate alignment belongs to the picked rig and the planner's rough.
    #[must_use]
    pub fn aligned(&self) -> bool {
        match (&self.alignment, &self.context, self.rig()) {
            (Some(alignment), Some(context), Some(rig)) => {
                let same_mesh = alignment.mesh_id == context.mesh_id;
                let same_rig = alignment.rig_name == rig.name;
                same_mesh && same_rig
            }
            _ => false,
        }
    }

    /// The scene data, built on first use.
    pub fn scene_data(&mut self) -> Option<Arc<SceneData>> {
        if self.scene.is_none() && self.aligned() {
            let (context, alignment, rig) = (
                self.context.as_ref()?,
                self.alignment.as_ref()?,
                self.rig()?.clone(),
            );
            self.scene = Some(Arc::new(SceneData::new(
                Arc::clone(&context.mesh),
                rig,
                alignment.transform,
            )));
        }
        self.scene.clone()
    }

    /// The facts the step state machine needs.
    #[must_use]
    pub fn facts(&self) -> StepFacts {
        StepFacts {
            rig_picked: self.rig().is_some(),
            aligned: self.aligned(),
            views_total: self.view_count(),
            stone_photos: self.files.iter().filter(|f| f.stone.is_some()).count(),
            with_white_frame: self.files.iter().filter(|f| f.is_complete()).count(),
            prepared_views: self.prepared.iter().flatten().count(),
            calibration_applied: self.calibration.is_some(),
            fit_done: self.fit.is_some(),
            busy: self.job.is_some(),
        }
    }

    /// The mean white-frame colour over the prepared views, for the automatic backlight.
    #[must_use]
    pub fn mean_white_rgb(&self) -> Option<[f64; 3]> {
        let views: Vec<_> = self.prepared.iter().flatten().collect();
        if views.is_empty() {
            return None;
        }
        let mut sum = [0.0; 3];
        for v in &views {
            for (total, channel) in sum.iter_mut().zip(v.white_rgb) {
                *total += channel;
            }
        }
        let n = views.len() as f64;
        Some([sum[0] / n, sum[1] / n, sum[2] / n])
    }

    /// The setup for a fit, from the prepared views and the choices.
    ///
    /// # Errors
    ///
    /// A sentence when something is missing.
    pub fn fit_setup(
        &mut self,
        panel_setback_mm: f64,
        panel_size_mm: f64,
        planned_mm: f64,
        cache_dir: Option<std::path::PathBuf>,
        host_id: Option<String>,
    ) -> Result<FitSetup, String> {
        let data = self
            .scene_data()
            .ok_or_else(|| "Align the mesh to the rig first.".to_owned())?;
        let calibration = self
            .calibration
            .clone()
            .ok_or_else(|| "Apply the calibration first.".to_owned())?;
        let views: Vec<PreparedView> = self.prepared.iter().flatten().cloned().collect();
        if views.len() < 2 {
            return Err("Prepare at least two views first.".to_owned());
        }
        Ok(FitSetup {
            data: (*data).clone(),
            views,
            calibration,
            surface: self.surface,
            windows: self.windows.iter().copied().collect(),
            panel_setback_mm,
            panel_size_mm,
            cache_dir,
            host_id,
            planned_mm,
        })
    }

    // --- Zones: undo, 3D handle drags, painted windows -----------------------------------------

    /// The zones as they are now.
    #[must_use]
    pub fn snapshot_zones(&self) -> ZoneSnapshot {
        ZoneSnapshot {
            geometry: self.geometry.clone(),
            locks: self.locks.clone(),
            selected_zone: self.selected_zone,
        }
    }

    /// Remembers the zones as they are now, for Undo (an edit is about to change them).
    pub fn push_zone_undo(&mut self) {
        let snapshot = self.snapshot_zones();
        self.remember_zones(snapshot);
    }

    fn remember_zones(&mut self, snapshot: ZoneSnapshot) {
        if self.zone_undo.len() >= ZONE_UNDO_DEPTH {
            self.zone_undo.remove(0);
        }
        self.zone_undo.push(snapshot);
    }

    /// Takes back the newest zone edit. Returns whether there was one.
    pub fn undo_zone_edit(&mut self) -> bool {
        let Some(previous) = self.zone_undo.pop() else {
            return false;
        };
        self.geometry = previous.geometry;
        self.locks = previous.locks;
        self.selected_zone = previous.selected_zone;
        self.drag_origin = None;
        self.forget_overlay();
        self.forget_fit();
        true
    }

    /// A 3D handle drag starts: the zones as they are become the origin every edit of the drag is
    /// applied to. A drag already under way keeps its origin.
    pub fn begin_zone_drag(&mut self) {
        if self.drag_origin.is_none() {
            self.drag_origin = Some(self.snapshot_zones());
        }
    }

    /// Applies `edit` to the zones as they were when the drag started (through
    /// `apply_with_locks`, so a locked parameter refuses). The geometry follows the pointer; the
    /// fit and the outlines are forgotten.
    ///
    /// Nothing changes when the edit is refused.
    ///
    /// # Errors
    ///
    /// The edit is refused (no drag under way, a locked parameter, an invalid result).
    pub fn drag_zone(&mut self, edit: &ZoneEdit) -> Result<(), ZoneEditError> {
        let Some(origin) = self.drag_origin.as_ref() else {
            return Err(ZoneEditError::NoSuchZone { zone: 0 });
        };
        let base = origin
            .geometry
            .clone()
            .unwrap_or_else(|| self.geometry_or_empty());
        let (zoned, locks) = apply_with_locks(&base, &origin.locks, edit)?;
        self.geometry = Some(zoned);
        self.locks = locks;
        self.forget_overlay();
        self.forget_fit();
        Ok(())
    }

    /// The drag ended: one undo step if the zones changed, none if the pointer came back to
    /// where it started. Returns whether the zones changed.
    pub fn end_zone_drag(&mut self) -> bool {
        let Some(origin) = self.drag_origin.take() else {
            return false;
        };
        let changed = origin.geometry != self.geometry || origin.locks != self.locks;
        if changed {
            self.remember_zones(origin);
        }
        changed
    }

    /// The width of the rough along its longest side, mm (10 mm without a mesh).
    #[must_use]
    pub fn extent_mm(&self) -> f64 {
        self.context.as_ref().map_or(10.0, |c| {
            let (lo, hi) = c.mesh.bounds();
            (hi - lo).max_element()
        })
    }

    /// The 3D handles of the selected zone: only in the Fit step, with a zone selected.
    #[must_use]
    pub fn zone_handles(&self) -> Vec<Handle> {
        match (&self.geometry, self.step) {
            (Some(geometry), Step::Fit) if self.selected_zone > 0 => {
                handles_for(geometry, &self.locks, self.selected_zone, self.extent_mm())
            }
            _ => Vec::new(),
        }
    }

    /// Whether a click on the 3D mesh paints polished windows: the Surfaces step is shown and a
    /// paint or erase mode is on (`paint_mode` is the window's mode: 1 paint, 2 erase).
    #[must_use]
    pub fn paints_on_mesh(&self, paint_mode: i32) -> bool {
        self.step == Step::Surfaces && matches!(paint_mode, 1 | 2)
    }

    /// Paints (`paint`) or erases triangles of the polished windows, the one set both the photo
    /// brush and the 3D brush edit, and forgets the fit. Returns how many triangles changed.
    pub fn paint_triangles(
        &mut self,
        triangles: impl IntoIterator<Item = u32>,
        paint: bool,
    ) -> usize {
        let mut changed = 0;
        for triangle in triangles {
            let did = if paint {
                self.windows.insert(triangle)
            } else {
                self.windows.remove(&triangle)
            };
            changed += usize::from(did);
        }
        if changed > 0 {
            self.forget_fit();
        }
        changed
    }

    /// The zone geometry or an empty one (the base zone only).
    #[must_use]
    pub fn geometry_or_empty(&self) -> ZonedAbsorption {
        self.geometry.clone().unwrap_or_else(|| {
            ZonedAbsorption::new(
                zone_rows::placeholder_zone(zone_rows::default_shape(
                    zone_rows::ShapeKind::HalfSpace,
                    glam::DVec3::ZERO,
                    1.0,
                ))
                .absorption,
            )
        })
    }

    // --- Pictures ----------------------------------------------------------------------------

    /// The prepared view on the picture.
    #[must_use]
    pub fn active_prepared(&self) -> Option<&PreparedView> {
        self.prepared.get(self.active_view).and_then(Option::as_ref)
    }

    /// The zone outlines of view `view` in its working pixels (cached).
    pub fn zone_outlines(&mut self, view: usize) -> Vec<(usize, Vec<[f64; 2]>)> {
        if self.overlay.is_none() {
            let computed = self.compute_overlay();
            self.overlay = Some(computed);
        }
        self.overlay
            .as_ref()
            .and_then(|all| all.get(view))
            .cloned()
            .unwrap_or_default()
    }

    fn compute_overlay(&mut self) -> Vec<Vec<(usize, Vec<[f64; 2]>)>> {
        let count = self.view_count();
        let mut out: Vec<Vec<(usize, Vec<[f64; 2]>)>> = vec![Vec::new(); count];
        let Some(geometry) = self.geometry.clone().filter(|g| !g.zones.is_empty()) else {
            return out;
        };
        let Some(data) = self.scene_data() else {
            return out;
        };
        let scene = data.scene();
        for overlay in project_overlay(&geometry, &scene, &OverlayOptions::default()) {
            let Some(Some(prepared)) = self.prepared.get(overlay.view) else {
                continue;
            };
            let grid = prepared.grid;
            for line in overlay.polylines {
                let points: Vec<[f64; 2]> = line
                    .pixels
                    .iter()
                    .map(|p| {
                        [
                            (p[0] - grid.origin[0]) / grid.scale,
                            (p[1] - grid.origin[1]) / grid.scale,
                        ]
                    })
                    .collect();
                if let Some(slot) = out.get_mut(overlay.view) {
                    slot.push((line.zone, points));
                }
            }
        }
        out
    }

    /// The picture of the active view for the current step: the transmittance photo with the
    /// mask (Masks step), the polished windows (Surfaces) or the zone outlines (Fit).
    pub fn canvas_image(&mut self) -> Option<Rgba> {
        let step = self.step;
        let view = self.active_view;
        let (mut image, mask, triangles) = {
            let prepared = self.active_prepared()?;
            let image = raster::linear_image(
                &prepared.observed.values,
                prepared.grid.width,
                prepared.grid.height,
                1.0,
            );
            (image, prepared.combined_mask(), prepared.triangles.clone())
        };
        match step {
            Step::Masks => raster::mask_overlay(&mut image, &mask, 0.55),
            Step::Surfaces => {
                for (i, &t) in triangles.iter().enumerate() {
                    if t != u32::MAX && self.windows.contains(&t) {
                        image.blend(i % image.width, i / image.width, [0, 200, 255], 0.5);
                    }
                }
            }
            Step::Fit => {
                for (zone, points) in self.zone_outlines(view) {
                    let colour = ZONE_COLOURS[zone.min(ZONE_COLOURS.len() - 1)];
                    raster::draw_polyline(&mut image, &points, false, colour);
                }
                if let Some(points) = self.marks.views.get(view)
                    && let Some(prepared) = self.active_prepared()
                {
                    let grid = prepared.grid;
                    for p in points {
                        raster::draw_disc(
                            &mut image,
                            [
                                (p[0] - grid.origin[0]) / grid.scale,
                                (p[1] - grid.origin[1]) / grid.scale,
                            ],
                            2.0,
                            [255, 0, 255],
                            1.0,
                        );
                    }
                }
            }
            _ => {}
        }
        Some(image)
    }

    /// The working pixel (fractions of the picture to working coordinates) and the full-resolution
    /// photo pixel under a click on the active view.
    #[must_use]
    pub fn click_pixels(&self, fx: f32, fy: f32) -> Option<([f64; 2], [f64; 2])> {
        let prepared = self.active_prepared()?;
        let working =
            super::brush::working_position(fx, fy, prepared.grid.width, prepared.grid.height)?;
        let full = DVec2::new(
            working[0].mul_add(prepared.grid.scale, prepared.grid.origin[0]),
            working[1].mul_add(prepared.grid.scale, prepared.grid.origin[1]),
        );
        Some((working, [full.x, full.y]))
    }

    /// The three pictures of the Compare step for the active view, cropped to the zoom:
    /// photo, render and difference, with the difference statistics line.
    #[must_use]
    pub fn compare_images(&self, zoom: u32) -> Option<(Rgba, Rgba, Rgba, String)> {
        self.compare_images_for(self.active_view, zoom)
    }

    /// [`compare_images`](Self::compare_images) for view `view`.
    #[must_use]
    pub fn compare_images_for(&self, view: usize, zoom: u32) -> Option<(Rgba, Rgba, Rgba, String)> {
        let prepared = self.prepared.get(view).and_then(Option::as_ref)?;
        let fit = self.fit.as_ref()?;
        let prediction = fit.predictions.iter().find(|p| p.view == prepared.view)?;
        let gain = fit
            .gains
            .iter()
            .find(|(v, _)| *v == prepared.view)
            .map_or(1.0, |(_, g)| *g);
        let (w, h) = (prepared.grid.width, prepared.grid.height);
        let photo = raster::linear_image(&prepared.observed.values, w, h, 1.0);
        let render = raster::linear_image(&prediction.rgb, w, h, gain);
        let mask = prepared.combined_mask();
        let valid: Vec<bool> = (0..w * h)
            .map(|i| {
                prediction.valid.get(i).copied().unwrap_or(false) && mask.is_clear(i % w, i / w)
            })
            .collect();
        let (delta, used, stats) =
            super::compare::delta_e_map(&prepared.observed.values, &prediction.rgb, &valid, gain);
        let heat = raster::heat_image(&delta, &used, w, h);
        let centre = self.compare_centre;
        Some((
            raster::crop_zoom(&photo, centre, zoom),
            raster::crop_zoom(&render, centre, zoom),
            raster::crop_zoom(&heat, centre, zoom),
            super::compare::stats_text(&stats),
        ))
    }

    /// The combined masks as notes for the report.
    #[must_use]
    pub fn mask_notes(&self) -> Vec<String> {
        self.prepared
            .iter()
            .flatten()
            .map(PreparedView::mask_note)
            .collect()
    }
}

impl Default for State {
    fn default() -> Self {
        Self::new()
    }
}

/// An empty mask is the wrong size for nothing; this keeps the type in the imports used.
#[must_use]
pub fn blank_mask(width: usize, height: usize) -> PixelMask {
    PixelMask::new(width, height)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_collect_per_view() {
        let mut marks = Marks::default();
        marks.resize(3);
        assert_eq!(marks.summary(), "No marks yet.");
        marks.click(0, [1.0, 2.0]);
        marks.click(0, [3.0, 4.0]);
        assert!(marks.summary().contains("1 view"));
        marks.click(2, [5.0, 6.0]);
        marks.click(9, [0.0, 0.0]);
        assert_eq!(marks.summary(), "3 points in 2 views.");
        let lines = marks.polylines();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1].view, 2);
        marks.undo(0);
        assert_eq!(marks.views[0].len(), 1);
        marks.clear();
        assert_eq!(marks.polylines(), [] as [ViewPolyline; 0]);
    }

    #[test]
    fn a_fresh_state_has_the_default_views_and_nothing_reachable_but_the_checklist() {
        let state = State::new();
        assert_eq!(state.view_count(), DEFAULT_VIEWS);
        assert_eq!(state.view_name(2), "View 3");
        let facts = state.facts();
        assert!(!facts.rig_picked && !facts.aligned && !facts.fit_done);
        assert_eq!(facts.views_total, DEFAULT_VIEWS);
        assert!(state.mean_white_rgb().is_none());
        assert!(state.active_prepared().is_none());
        assert!(!state.aligned());
    }

    #[test]
    fn the_empty_geometry_has_only_the_base_zone() {
        let state = State::new();
        let geometry = state.geometry_or_empty();
        assert_eq!(geometry.zones, [] as [indicatrix::optics::zoning::Zone; 0]);
        assert!(geometry.validate().is_ok());
    }

    fn with_cylinder() -> State {
        use crate::gui::rough_colour::wizard::zone_rows::{
            ShapeKind, default_shape, placeholder_zone,
        };
        use indicatrix_cut_core::rough_plan::colour_fit::zones::apply;
        let mut state = State::new();
        let zoned = apply(
            &state.geometry_or_empty(),
            &ZoneEdit::Add {
                zone: placeholder_zone(default_shape(ShapeKind::Cylinder, glam::DVec3::ZERO, 12.0)),
                position: None,
            },
        )
        .unwrap();
        state.geometry = Some(zoned);
        state.selected_zone = 1;
        state.step = Step::Fit;
        state
    }

    fn outer_radius(state: &State) -> f64 {
        use indicatrix::optics::zoning::ZoneShape;
        match state.geometry.as_ref().unwrap().zones[0].shape {
            ZoneShape::CoaxialCylinder { r_out, .. } => r_out,
            _ => panic!("a cylinder"),
        }
    }

    #[test]
    fn a_handle_drag_is_one_undo_step_whatever_the_number_of_moves() {
        use indicatrix_cut_core::rough_plan::colour_fit::zones::ZoneParameter;
        let mut state = with_cylinder();
        let before = outer_radius(&state);
        state.begin_zone_drag();
        for value in [3.5, 4.0, 4.5, 5.0] {
            state
                .drag_zone(&ZoneEdit::SetParameter {
                    zone: 1,
                    parameter: ZoneParameter::ROut,
                    value,
                })
                .unwrap();
        }
        assert!((outer_radius(&state) - 5.0).abs() < 1e-12);
        assert_eq!(
            state.zone_undo,
            [] as [ZoneSnapshot; 0],
            "nothing is remembered mid-drag"
        );
        assert!(state.end_zone_drag());
        assert_eq!(state.zone_undo.len(), 1, "one step per drag");
        assert!(state.drag_origin.is_none());
        assert!(state.undo_zone_edit());
        assert!((outer_radius(&state) - before).abs() < 1e-12);
        assert!(!state.undo_zone_edit(), "nothing left to undo");
    }

    #[test]
    fn a_drag_that_returns_to_where_it_started_leaves_no_undo_step() {
        use indicatrix_cut_core::rough_plan::colour_fit::zones::ZoneParameter;
        let mut state = with_cylinder();
        let before = outer_radius(&state);
        state.begin_zone_drag();
        state
            .drag_zone(&ZoneEdit::SetParameter {
                zone: 1,
                parameter: ZoneParameter::ROut,
                value: before,
            })
            .unwrap();
        assert!(!state.end_zone_drag());
        assert_eq!(state.zone_undo, [] as [ZoneSnapshot; 0]);
    }

    #[test]
    fn a_drag_edits_the_zones_as_they_were_at_the_start_and_a_locked_parameter_refuses() {
        use indicatrix_cut_core::rough_plan::colour_fit::zones::ZoneParameter;
        let mut state = with_cylinder();
        let (zoned, locks) = apply_with_locks(
            &state.geometry_or_empty(),
            &state.locks,
            &ZoneEdit::Lock {
                zone: 1,
                parameter: ZoneParameter::ROut,
            },
        )
        .unwrap();
        state.geometry = Some(zoned);
        state.locks = locks;
        let before = outer_radius(&state);
        state.begin_zone_drag();
        let refused = state.drag_zone(&ZoneEdit::SetParameter {
            zone: 1,
            parameter: ZoneParameter::ROut,
            value: 9.0,
        });
        assert!(matches!(refused, Err(ZoneEditError::Locked { .. })));
        assert!((outer_radius(&state) - before).abs() < 1e-12);
        // Without a drag under way nothing is applied.
        state.end_zone_drag();
        assert!(
            state
                .drag_zone(&ZoneEdit::SetSoftness { millimetres: 1.0 })
                .is_err()
        );
    }

    #[test]
    fn the_handles_are_offered_in_the_fit_step_for_the_selected_zone_only() {
        let mut state = with_cylinder();
        assert_ne!(state.zone_handles(), [] as [Handle; 0]);
        state.selected_zone = 0;
        assert_eq!(state.zone_handles(), [] as [Handle; 0], "no zone selected");
        state.selected_zone = 1;
        state.step = Step::Masks;
        assert_eq!(
            state.zone_handles(),
            [] as [Handle; 0],
            "not in the Fit step"
        );
    }

    #[test]
    fn the_undo_list_is_capped() {
        let mut state = State::new();
        for _ in 0..(ZONE_UNDO_DEPTH + 5) {
            state.push_zone_undo();
        }
        assert_eq!(state.zone_undo.len(), ZONE_UNDO_DEPTH);
    }

    #[test]
    fn photo_and_mesh_painting_edit_the_same_set_of_windows() {
        let mut state = State::new();
        assert_eq!(state.paint_triangles([4, 5, 6], true), 3);
        assert_eq!(
            state.paint_triangles([6, 7], true),
            1,
            "6 was painted already"
        );
        assert_eq!(
            state.windows.iter().copied().collect::<Vec<_>>(),
            vec![4, 5, 6, 7]
        );
        assert_eq!(
            state.paint_triangles([5, 99], false),
            1,
            "99 was never painted"
        );
        assert_eq!(
            state.windows.iter().copied().collect::<Vec<_>>(),
            vec![4, 6, 7]
        );
        // The mesh brush only paints in the Surfaces step with a mode on.
        state.step = Step::Surfaces;
        assert!(state.paints_on_mesh(1) && state.paints_on_mesh(2));
        assert!(!state.paints_on_mesh(0));
        state.step = Step::Masks;
        assert!(!state.paints_on_mesh(1));
    }

    #[test]
    fn zone_colours_cover_the_four_zones_and_the_base() {
        assert_eq!(ZONE_COLOURS.len(), 5);
        assert_eq!(blank_mask(2, 2).width(), 2);
    }
}
