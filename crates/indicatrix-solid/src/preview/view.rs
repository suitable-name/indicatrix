//! Small pure helpers a Solid/Diagram view needs around the pipeline.
//!
//! The raster size for a view, pointer-to-pixel hit testing through `image-fit: contain`,
//! the "which panel was double-clicked" toggle, keyboard selection stepping, the
//! multi-selection overlay, and which tiers an edit touched (so a caller without
//! per-edit bookkeeping can still ask for a cheap subgraph re-solve).

use super::{
    request::{PlanJob, panel_kind_from_index},
    types::CameraPose,
};
use crate::diagram2d::PanelKind;
use indicatrix::{geometry::meet_solver::SolvedTier, optics::materials::GemMaterial};
use indicatrix_cut_core::Design;
use std::{collections::BTreeSet, sync::Arc};

/// The caps [`raster_size_for_view`] applies.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RasterLimits {
    /// The largest device pixel ratio honoured (a quadratic cost multiplier).
    pub max_device_pixel_ratio: f32,
    /// The smallest raster edge, in pixels (a view can report zero while laying out).
    pub min_edge: u32,
    /// The largest raster edge, in pixels.
    pub max_edge: u32,
}

/// The raster size for a view of `logical_width x logical_height` logical pixels.
///
/// On a display with `scale_factor`: the device-pixel size (the ratio clamped to
/// `1.0..=limits.max_device_pixel_ratio`), scaled down UNIFORMLY when its longer
/// edge exceeds `limits.max_edge` (so the raster keeps the view's aspect and
/// fills it under `image-fit: contain`), then each edge raised to at least
/// `limits.min_edge`.
///
/// A negative or NaN size counts as zero.
#[must_use]
pub fn raster_size_for_view(
    logical_width: f32,
    logical_height: f32,
    scale_factor: f32,
    limits: RasterLimits,
) -> (u32, u32) {
    let ratio = scale_factor.clamp(1.0, limits.max_device_pixel_ratio.max(1.0));
    let width = logical_width.max(0.0) * ratio;
    let height = logical_height.max(0.0) * ratio;
    let longest = width.max(height);
    let max_edge = limits.max_edge.max(limits.min_edge).max(1);
    let shrink = if longest > max_edge as f32 {
        max_edge as f32 / longest
    } else {
        1.0
    };
    let clamp = |edge: f32| ((edge * shrink).round() as u32).clamp(limits.min_edge, max_edge);
    (clamp(width), clamp(height))
}

/// `size` with its height reduced until the raster is at least `min_aspect` (width over
/// height) wide, each edge at least `min_edge`.
///
/// The Solid view's camera has a fixed VERTICAL field of view, so a view narrower than the
/// aspect the default pose was framed for (the desktop's 800 x 600) crops the stone's sides
/// out of the raster itself. Drawing a raster of that minimum aspect and letting
/// `image-fit: contain` letterbox it in the view (bars above and below) keeps the whole
/// stone visible in a narrow view; a view already wide enough is returned unchanged.
#[must_use]
pub fn with_min_aspect(size: (u32, u32), min_aspect: f32, min_edge: u32) -> (u32, u32) {
    let (width, height) = size;
    if min_aspect <= 0.0 || (width as f32) >= (height as f32) * min_aspect {
        return size;
    }
    let reduced = ((width as f32 / min_aspect).round() as u32).max(min_edge);
    (width, reduced.min(height))
}

/// The raster size for the Diagram view: [`raster_size_for_view`], scaled UP uniformly
/// until its width reaches `min_width`, then capped at `limits.max_edge`.
///
/// The diagram draws its facet labels in a 1-pixel-per-glyph bitmap font and decides what
/// fits from the facets' size in raster pixels, so a small raster (a narrow view beside a
/// dock) makes the labels of neighbouring facets pile on each other. The desktop's
/// diagram is drawn at a window-sized raster; a caller with a smaller view asks for a
/// raster of that size instead and lets `image-fit: contain` scale it down (the wheel
/// zoom then shows the labels at full size). The view's aspect is kept.
#[must_use]
pub fn diagram_raster_size(
    logical_width: f32,
    logical_height: f32,
    scale_factor: f32,
    limits: RasterLimits,
    min_width: u32,
) -> (u32, u32) {
    let (width, height) = raster_size_for_view(logical_width, logical_height, scale_factor, limits);
    if width >= min_width {
        return (width, height);
    }
    let grow = min_width as f32 / width.max(1) as f32;
    let max_edge = limits.max_edge.max(limits.min_edge).max(1);
    let longest = width.max(height) as f32 * grow;
    let shrink = if longest > max_edge as f32 {
        max_edge as f32 / longest
    } else {
        1.0
    };
    let fit =
        |edge: u32| ((edge as f32 * grow * shrink).round() as u32).clamp(limits.min_edge, max_edge);
    (fit(width), fit(height))
}

/// Where an image lands in a view under `image-fit: contain`.
///
/// The image is scaled uniformly to fit and centred: this is the view-space offset of
/// its top-left corner and the uniform scale from image pixels to logical view units.
///
/// This is the pointer <-> pick-frame mapping of the web's Solid view: the raster is
/// sized to the view's aspect but rounded and capped, so it may be letterboxed by a few
/// logical units (or by bars when the caps bite), and a pointer position must be mapped
/// through exactly the transform the `Image` draws with. [`Self::to_image`] is the
/// pointer -> pick-frame direction (float, not clamped, so a drag that leaves the image
/// keeps travelling), [`Self::to_view`] the way back that places the drag handles.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ContainFit {
    /// Logical x of the image's left edge.
    pub offset_x: f32,
    /// Logical y of the image's top edge.
    pub offset_y: f32,
    /// Logical units per image pixel.
    pub scale: f32,
}

impl ContainFit {
    /// The image position (fractional pixels, origin at the image's top-left corner)
    /// under the logical view position `(x, y)`. Not clamped to the image: a point in a
    /// letterbox bar maps outside `0..width`.
    #[must_use]
    pub fn to_image(self, x: f32, y: f32) -> (f32, f32) {
        (
            (x - self.offset_x) / self.scale,
            (y - self.offset_y) / self.scale,
        )
    }

    /// The logical view position an image position `(px, py)` is drawn at: the exact
    /// inverse of [`Self::to_image`].
    #[must_use]
    pub const fn to_view(self, px: f32, py: f32) -> (f32, f32) {
        (
            px.mul_add(self.scale, self.offset_x),
            py.mul_add(self.scale, self.offset_y),
        )
    }

    /// A length in logical view units expressed in image pixels (a grab radius that
    /// stays the same size on screen however the raster is scaled).
    #[must_use]
    pub fn length_to_image(self, logical: f32) -> f32 {
        logical / self.scale
    }
}

/// The `image-fit: contain` placement of an `image_width x image_height` image in a
/// `view_width x view_height` view (logical units); `None` for an empty view or image.
#[must_use]
pub fn contain_fit(
    view_width: f32,
    view_height: f32,
    image_width: u32,
    image_height: u32,
) -> Option<ContainFit> {
    if view_width <= 0.0 || view_height <= 0.0 || image_width == 0 || image_height == 0 {
        return None;
    }
    let (image_w, image_h) = (image_width as f32, image_height as f32);
    let scale = (view_width / image_w).min(view_height / image_h);
    Some(ContainFit {
        offset_x: image_w.mul_add(-scale, view_width) * 0.5,
        offset_y: image_h.mul_add(-scale, view_height) * 0.5,
        scale,
    })
}

/// The image pixel under a pointer at `(x, y)` (logical, relative to the view).
///
/// Applies when an `image_width x image_height` image is shown in a
/// `view_width x view_height` view with `image-fit: contain` (scaled uniformly to
/// fit, centred).
///
/// `None` outside the drawn image or for an empty view/image.
#[must_use]
pub fn contain_pixel(
    x: f32,
    y: f32,
    view_width: f32,
    view_height: f32,
    image_width: u32,
    image_height: u32,
) -> Option<(u32, u32)> {
    let fit = contain_fit(view_width, view_height, image_width, image_height)?;
    let (image_w, image_h) = (image_width as f32, image_height as f32);
    let (px, py) = fit.to_image(x, y);
    let (px, py) = (px.floor(), py.floor());
    if !(px >= 0.0 && py >= 0.0 && px < image_w && py < image_h) {
        return None;
    }
    Some((px as u32, py as u32))
}

/// The UI index (`0` Crown, `1` Pavilion, `2` Profile) of a diagram panel, the
/// inverse of [`panel_kind_from_index`].
#[must_use]
pub const fn panel_index(panel: PanelKind) -> i32 {
    match panel {
        PanelKind::Crown => 0,
        PanelKind::Pavilion => 1,
        PanelKind::Profile => 2,
    }
}

/// A double click on the diagram: the next enlarged-panel index.
///
/// From the three-panel layout (`current` not a
/// panel) it enlarges the `clicked` panel (`DiagramFrame::panel_at` under the
/// pointer), staying at three panels when the click missed every panel; from an
/// enlarged panel it always goes back to three panels (`-1`).
#[must_use]
pub const fn toggle_enlarged_panel(current: i32, clicked: Option<PanelKind>) -> i32 {
    if panel_kind_from_index(current).is_some() {
        return -1;
    }
    match clicked {
        Some(panel) => panel_index(panel),
        None => -1,
    }
}

/// Up/Down/PageUp/PageDown in the view: the desktop's `step_selection` rule.
///
/// Nothing selected starts from the top (`delta >= 0`) or the bottom; otherwise
/// the selection moves by `delta`, clamped to the tier list. No tiers: unchanged.
#[must_use]
pub fn step_selection(current: Option<usize>, delta: i32, tier_count: usize) -> Option<usize> {
    if tier_count == 0 {
        return current;
    }
    let last = tier_count - 1;
    let Some(current) = current else {
        return Some(if delta >= 0 { 0 } else { last });
    };
    let moved = i64::try_from(current.min(last)).unwrap_or(i64::MAX) + i64::from(delta);
    Some(usize::try_from(moved.max(0)).unwrap_or(0).min(last))
}

/// Every facet id whose tier (`facet_tier[id]`, a frame's facet -> tier table) is
/// in `tiers`: the multi-selection highlight ([`super::FacetOverlay::multi_selected`]).
#[must_use]
pub fn facets_of_tiers(facet_tier: &[Option<usize>], tiers: &BTreeSet<usize>) -> Vec<u32> {
    if tiers.is_empty() {
        return Vec::new();
    }
    facet_tier
        .iter()
        .enumerate()
        .filter(|(_, tier)| tier.is_some_and(|tier| tiers.contains(&tier)))
        .map(|(id, _)| id as u32)
        .collect()
}

/// The tiers an edit from `previous` to `current` touched, for a subgraph
/// re-solve against `previous`'s masts: every index whose tier differs.
///
/// `None` when only a full solve is valid -- the tier count changed (an add/remove,
/// which `Design::resolve_dirty` cannot take), or the gear, reference angle,
/// symmetry or mirror changed (the desktop forces a full solve for those).
#[must_use]
pub fn dirty_tiers(previous: &Design, current: &Design) -> Option<BTreeSet<usize>> {
    if previous.tiers.len() != current.tiers.len()
        || previous.meta.gear_teeth != current.meta.gear_teeth
        || previous.meta.gear_reference_angle != current.meta.gear_reference_angle
        || previous.meta.symmetry_order != current.meta.symmetry_order
        || previous.meta.mirror != current.meta.mirror
    {
        return None;
    }
    Some(
        previous
            .tiers
            .iter()
            .zip(&current.tiers)
            .enumerate()
            .filter(|(_, (before, after))| before != after)
            .map(|(index, _)| index)
            .collect(),
    )
}

/// Whether `design` plans with no solver call at all.
///
/// True when every tier is a
/// `MeetConstraint::ScaleReference` (`live_update::plan_preview`'s "Pinned" tier,
/// checked first there too), so its masts are read straight off the tiers and a
/// "full solve" costs nothing whatever the design's size.
#[must_use]
pub fn plans_without_solver(design: &Design) -> bool {
    design.tiers.iter().all(|tier| {
        matches!(
            tier.constraint,
            indicatrix::geometry::meet_solver::MeetConstraint::ScaleReference(_)
        )
    })
}

/// What a replan starts from -- see [`replan_basis`].
#[derive(Debug, Clone)]
pub enum ReplanBasis {
    /// Plan against these masts, re-solving only `dirty`.
    Chain {
        /// Masts aligned with the current design's tiers.
        last_solved: Vec<SolvedTier>,
        /// The tiers to re-solve (empty: the masts are current).
        dirty: BTreeSet<usize>,
    },
    /// No aligned masts: only a full `Design::solve()` can plan this design.
    FullSolve,
}

/// Chooses a replan's starting point for `design`.
///
/// First an authoritative solve of the
/// CURRENT design (`current_solve`) first, nothing dirty; else the masts the
/// previous frame was planned from (`cached`: that design and its masts), with
/// [`dirty_tiers`] as the dirty set; else [`ReplanBasis::FullSolve`].
#[must_use]
pub fn replan_basis(
    design: &Design,
    current_solve: Option<&[SolvedTier]>,
    cached: Option<(&Design, &[SolvedTier])>,
) -> ReplanBasis {
    if let Some(solved) = current_solve.filter(|solved| solved.len() == design.tiers.len()) {
        return ReplanBasis::Chain {
            last_solved: solved.to_vec(),
            dirty: BTreeSet::new(),
        };
    }
    if let Some((previous, masts)) = cached
        && masts.len() == design.tiers.len()
        && let Some(dirty) = dirty_tiers(previous, design)
    {
        return ReplanBasis::Chain {
            last_solved: masts.to_vec(),
            dirty,
        };
    }
    ReplanBasis::FullSolve
}

/// The UI-side inputs of one replan, as a view reads them.
///
/// The design snapshot
/// and generation, the [`ReplanBasis`], the camera and raster size, and the
/// view's own settings in their UI encodings (`-1` for "none").
pub struct ReplanInputs<'a> {
    /// The design to plan (shared with the planned frame, never deep-cloned).
    pub design: Arc<Design>,
    /// The design generation this replan describes.
    pub generation: u64,
    /// What to plan from -- see [`replan_basis`].
    pub basis: ReplanBasis,
    /// The orbit camera.
    pub camera: CameraPose,
    /// The raster size in pixels.
    pub size: (u32, u32),
    /// The tier selected in the tier list.
    pub selected_tier: Option<usize>,
    /// Custom materials the design's material name may refer to.
    pub custom_materials: &'a [GemMaterial],
    /// 0 Solid, 1 Path-traced, 2 Both, 3 Diagram.
    pub view_mode: u8,
    /// The Preform toggle.
    pub show_preform: bool,
    /// The enlarged diagram panel, `-1` for three panels.
    pub enlarged_panel: i32,
    /// The "Cut" slider, `-1` for the whole design.
    pub tier_cutoff: i32,
}

/// Builds the [`PlanJob`] for `inputs` the way the desktop's
/// `submit_preview_replan_for` does.
///
/// `n_d` comes from
/// `Design::effective_refractive_index_with` (custom materials included), the
/// cutoff's `-1` as `None`, and [`ReplanBasis::FullSolve`] as `last_solved: None`
/// with nothing dirty.
#[must_use]
pub fn plan_job(inputs: ReplanInputs<'_>) -> PlanJob {
    let (last_solved, dirty) = match inputs.basis {
        ReplanBasis::Chain { last_solved, dirty } => (Some(last_solved), dirty),
        ReplanBasis::FullSolve => (None, BTreeSet::new()),
    };
    let n_d = inputs
        .design
        .effective_refractive_index_with(inputs.custom_materials);
    PlanJob {
        design: inputs.design,
        dirty,
        last_solved,
        camera: inputs.camera,
        size: inputs.size,
        selected_tier: inputs.selected_tier,
        n_d,
        view_mode: inputs.view_mode,
        generation: inputs.generation,
        show_preform: inputs.show_preform,
        enlarged_panel: inputs.enlarged_panel,
        tier_cutoff: usize::try_from(inputs.tier_cutoff).ok(),
    }
}
