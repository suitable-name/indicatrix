//! The provisional slice session's data and the pure decisions around it: the dragged
//! line, the snapped plan, the session struct, the in-place edits a handle drag makes to
//! its tier, and the small predicates the orchestration in [`super`] branches on. None of
//! it touches the Slint window or the thread-local session.

use super::super::{
    frame_updates_mast_cache,
    handles::{CAMERA_FOV_DEG, ids_aligned},
};
use crate::gui::solid_preview::{facet_map::FacetMap, preview_state::FrameGeometry};
use glam::Vec3;
use indicatrix::{
    geometry::meet_solver::{MeetConstraint, SolvedTier},
    optics::raytracer::Camera,
};
use indicatrix_cut_core::{ConstraintTier, Design, Edit, expected_orbit, rotate_indices};
use indicatrix_editor::{
    manipulate::{
        ScreenPoint, ScreenSize, SliceSide, SnappedFacet, slice_normal, slice_tier, snap_to_gear,
        tangency_mast,
    },
    session::clamp_nudge_to_side,
};
use std::{collections::BTreeSet, rc::Rc, sync::Arc};

/// The angle step a sliced facet snaps to, in degrees.
const SLICE_ANGLE_STEP_DEG: f64 = 0.1;

/// Whether a provisional session taken against `base_generation` still describes the
/// design: it does exactly while the editor's generation has not moved.
pub(in super::super) const fn session_outlives(
    base_generation: u64,
    current_generation: u64,
) -> bool {
    base_generation == current_generation
}

/// What a landed solid-preview frame means for a live provisional session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in super::super) enum LandedAction {
    /// Nothing to do: no session, or the frame is the provisional picture itself (or a
    /// provisional replan is already on its way).
    Ignore,
    /// A frame of the committed design replaced the provisional picture: render the
    /// provisional design again.
    Resubmit,
    /// The committed design moved on since the session began: the session is stale.
    Discard,
}

/// The decision behind [`super::on_landed`], on plain numbers.
///
/// `base_generation` is the session's (`None`: no session), `awaiting` whether a
/// provisional replan was submitted and is still queued or running (no provisional-
/// generation frame has landed since, and no committed replan has taken the plan gate's
/// single slot over it -- see `clear_awaiting`), `current_generation` the editor's now,
/// `frame_generation` the landed frame's
/// own. A frame stamped `PROVISIONAL_GENERATION` is the provisional picture
/// (`Ignore`), so a provisional replan can never cause another one: the only frames that
/// `Resubmit` are committed ones -- an idle replan, a solve landing, a selection
/// replan -- and each of those needs at most one.
pub(in super::super) const fn landed_action(
    base_generation: Option<u64>,
    awaiting: bool,
    current_generation: u64,
    frame_generation: u64,
) -> LandedAction {
    match base_generation {
        None => LandedAction::Ignore,
        Some(base) if !session_outlives(base, current_generation) => LandedAction::Discard,
        Some(_) if awaiting || !frame_updates_mast_cache(frame_generation) => LandedAction::Ignore,
        Some(_) => LandedAction::Resubmit,
    }
}

/// What a provisional replan chains from: the session's own masts of its last frame
/// (`masts`) with `dirty = {tier_index}` once they describe every tier of the design
/// (`tier_count`), else nothing (a full solve). The shared `solid_last_solved` cache is
/// the committed design's and is never used for a provisional replan.
pub(in super::super) fn replan_chain(
    masts: Option<&[SolvedTier]>,
    tier_count: usize,
    tier_index: usize,
) -> (Option<Vec<SolvedTier>>, BTreeSet<usize>) {
    match masts {
        Some(masts) if masts.len() == tier_count => {
            (Some(masts.to_vec()), BTreeSet::from([tier_index]))
        }
        _ => (None, BTreeSet::new()),
    }
}

/// How many of `tier_facets` (the provisional tier's facet ids) have a centroid in the
/// frame on screen, i.e. actually touch the stone. `None` while the facet-id space of
/// the map (`map_facets` planes) and of the frame's `centroids` disagree, so nothing is
/// known.
pub(in super::super) fn surviving_facets(
    tier_facets: &[u32],
    centroids: &[Option<Vec3>],
    map_facets: usize,
) -> Option<usize> {
    ids_aligned(map_facets, centroids.len()).then(|| {
        tier_facets
            .iter()
            .filter(|&&id| centroids.get(id as usize).is_some_and(Option::is_some))
            .count()
    })
}

/// Whether the Cut slider (`cutoff`, `-1` for the whole design) shows fewer tiers than
/// reach `tier_index`, so the mesh on screen does not contain that tier at all.
pub(in super::super) fn cut_hides_tier(cutoff: i32, tier_index: usize) -> bool {
    usize::try_from(cutoff).is_ok_and(|through| through < tier_index)
}

/// Whether Keep may commit a provisional tier with `surviving` facets on the stone: a
/// tier that cuts nothing (or whose picture has not landed yet) is refused.
pub(in super::super) fn keep_allowed(surviving: Option<usize>) -> bool {
    surviving.is_some_and(|count| count > 0)
}

/// Why a provisional slice is being dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in super::super) enum DiscardReason {
    /// The Discard button, Escape, or leaving Slice mode.
    User,
    /// The committed design was edited underneath the session.
    DesignChanged,
    /// A tier-list selection change is about to replan the committed design itself.
    SelectionChanged,
}

impl DiscardReason {
    /// Whether the committed design must be re-rendered to make the provisional
    /// outline disappear (a selection change replans on its own).
    pub(super) const fn replans(self) -> bool {
        !matches!(self, Self::SelectionChanged)
    }
}

/// The dragged line and the camera it was drawn against, so Flip can recompute the
/// plane later even if the camera has moved since.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(in super::super) struct SliceLine {
    /// Where the drag started (pick frame).
    pub(in super::super) a: ScreenPoint,
    /// Where the drag ended (pick frame).
    pub(in super::super) b: ScreenPoint,
    /// The pick frame's size.
    pub(in super::super) size: ScreenSize,
    /// The camera's `(yaw, pitch, distance)` for that frame.
    pub(in super::super) pose: (f32, f32, f32),
}

impl SliceLine {
    /// The line `a -> b` drawn on the frame `geometry` describes.
    pub(super) const fn new(a: ScreenPoint, b: ScreenPoint, geometry: &FrameGeometry) -> Self {
        Self {
            a,
            b,
            size: ScreenSize::new(geometry.size.0 as f32, geometry.size.1 as f32),
            pose: (
                geometry.camera.yaw,
                geometry.camera.pitch,
                geometry.camera.distance,
            ),
        }
    }

    /// The cutting-plane normal for the cut `side`; `None` for a line too short to
    /// define one.
    pub(in super::super) fn normal(&self, side: SliceSide) -> Option<Vec3> {
        let (yaw, pitch, distance) = self.pose;
        let camera = Camera::new(yaw, pitch, distance, CAMERA_FOV_DEG);
        slice_normal(&camera, self.a, self.b, self.size, side)
    }
}

/// What a slice line turns into before it becomes a session.
pub(in super::super) struct SlicePlan {
    /// The plane snapped to the gear.
    pub(in super::super) snapped: SnappedFacet,
    /// The provisional tier (`ScaleReference(tangency mast)`).
    pub(in super::super) tier: ConstraintTier,
}

/// The snapped facet and provisional tier for `line` on `design`'s stone (`corners`
/// are the committed stone's corner points): the plane just touching the stone from
/// outside, so the cut starts at zero depth. `None` for a degenerate line.
pub(in super::super) fn plan_slice(
    line: &SliceLine,
    side: SliceSide,
    corners: &[Vec3],
    design: &Design,
    symmetric: bool,
) -> Option<SlicePlan> {
    let normal = line.normal(side)?;
    let snapped = snap_to_gear(normal, design.meta.gear_teeth_abs(), SLICE_ANGLE_STEP_DEG);
    let mast = tangency_mast(snapped.normal, corners);
    let names: Vec<String> = design
        .tiers
        .iter()
        .flat_map(|tier| tier.names().into_iter().map(str::to_string))
        .collect();
    let tier = slice_tier(&snapped, mast, &design.meta, symmetric, &names);
    Some(SlicePlan { snapped, tier })
}

/// A clone of `design` with `tier` appended, and the index it landed at -- always the
/// committed design's tier count, so every committed tier keeps its index.
pub(in super::super) fn with_provisional_tier(
    design: &Design,
    tier: ConstraintTier,
) -> Option<(Design, usize)> {
    let mut provisional = design.clone();
    let index = provisional.tiers.len();
    provisional.apply_edit(Edit::AddTier { index, tier }).ok()?;
    Some((provisional, index))
}

/// The provisional tier and its snapped facet as they were at a handle press, for
/// Escape.
#[derive(Debug, Clone)]
pub(in super::super) struct Snapshot {
    pub(super) tier: ConstraintTier,
    pub(super) snapped: SnappedFacet,
}

/// The provisional slice: a design clone with the new tier last, and everything needed
/// to render, tweak, flip and keep it.
pub(super) struct ProvisionalSlice {
    /// `EditorState`'s generation when the session began.
    pub(super) base_generation: u64,
    /// The committed design plus the provisional tier at `tier_index`.
    pub(super) design: Arc<Design>,
    /// Where the provisional tier sits: the committed design's tier count.
    pub(super) tier_index: usize,
    /// Which side of the line is cut away.
    pub(super) side: SliceSide,
    /// The snapped facet; `index` follows the index handle so the Symmetric toggle can
    /// rebuild the orbit from it.
    pub(super) snapped: SnappedFacet,
    /// The dragged line, for Flip.
    pub(super) line: SliceLine,
    /// The COMMITTED stone's corner points (a provisional frame's geometry describes
    /// the cut stone).
    pub(super) base_corners: Arc<Vec<Vec3>>,
    /// The solved masts of the latest provisional frame.
    pub(super) masts: Option<Vec<SolvedTier>>,
    /// The facet map of `design` against `masts`, rebuilt lazily.
    pub(super) facet_map: Option<Rc<FacetMap>>,
    /// The facet ids currently outlined green.
    pub(super) outline: Vec<u32>,
    /// Whether `masts` changed since the outline was last computed.
    pub(super) masts_new: bool,
    /// Whether a provisional replan was submitted and no provisional-generation frame
    /// has landed since -- see [`landed_action`].
    pub(super) awaiting: bool,
    /// How many of the tier's facets touch the stone in the latest provisional frame
    /// (`None` until one has landed for this design) -- see [`surviving_facets`].
    pub(super) surviving: Option<usize>,
}

impl ProvisionalSlice {
    /// A fresh session for `design` (the committed design plus the provisional tier at
    /// `tier_index`), rendered by nothing yet.
    pub(super) fn new(
        base_generation: u64,
        design: Design,
        tier_index: usize,
        side: SliceSide,
        snapped: SnappedFacet,
        line: SliceLine,
        base_corners: Arc<Vec<Vec3>>,
    ) -> Self {
        Self {
            base_generation,
            design: Arc::new(design),
            tier_index,
            side,
            snapped,
            line,
            base_corners,
            masts: None,
            facet_map: None,
            outline: Vec::new(),
            masts_new: false,
            awaiting: false,
            surviving: None,
        }
    }

    /// The provisional tier.
    pub(super) fn tier(&self) -> Option<&ConstraintTier> {
        self.design.tiers.get(self.tier_index)
    }

    /// The facet map of the provisional design, built on first use after a change.
    /// `None` until a frame's masts describe the design.
    pub(super) fn facet_map(&mut self) -> Option<Rc<FacetMap>> {
        if self.facet_map.is_none() {
            let masts = self
                .masts
                .as_ref()
                .filter(|masts| masts.len() == self.design.tiers.len())?;
            self.facet_map = Some(Rc::new(FacetMap::from_design(&self.design, masts)));
        }
        self.facet_map.clone()
    }

    /// How many facets the tier has: the ones that touch the stone once a frame landed,
    /// else the number of index positions.
    pub(super) fn facet_count(&self) -> usize {
        self.surviving
            .unwrap_or_else(|| self.tier().map_or(0, |tier| tier.indices.len()))
    }
}

/// The Slice tool's part of the manipulation session.
pub(in super::super) struct SliceState {
    /// The line gesture in progress: where the press started (logical).
    pub(super) gesture: Option<(f32, f32)>,
    /// The provisional tier, if there is one.
    pub(super) provisional: Option<ProvisionalSlice>,
}

impl SliceState {
    /// No gesture, no session.
    pub(in super::super) const fn new() -> Self {
        Self {
            gesture: None,
            provisional: None,
        }
    }
}

/// Sets the provisional tier's angle, held to its side of zero like the keyboard nudge.
pub(super) fn set_angle(p: &mut ProvisionalSlice, deg: f64) -> bool {
    let index = p.tier_index;
    let Some(tier) = Arc::make_mut(&mut p.design).tiers.get_mut(index) else {
        return false;
    };
    let new = clamp_nudge_to_side(tier.angle_deg, deg);
    if !new.is_finite() || new.to_bits() == tier.angle_deg.to_bits() {
        return false;
    }
    tier.angle_deg = new;
    p.snapped.angle_deg = new;
    true
}

/// Pins the provisional tier at `mast`.
pub(super) fn set_mast(p: &mut ProvisionalSlice, mast: f64) -> bool {
    let index = p.tier_index;
    let Some(tier) = Arc::make_mut(&mut p.design).tiers.get_mut(index) else {
        return false;
    };
    let unchanged = matches!(&tier.constraint, MeetConstraint::ScaleReference(m) if m.to_bits() == mast.to_bits());
    if !mast.is_finite() || unchanged {
        return false;
    }
    tier.constraint = MeetConstraint::ScaleReference(mast);
    true
}

/// The whole-tooth turn `more` amounts to on a wheel of `gear` teeth: a turn of the
/// wheel is periodic, so `more` is reduced into `0..gear` (never dropped, however large
/// the `i64` is), which always fits a `u32` and converts to `f64` without a cast.
/// `None` for a wheel with no teeth and for a turn by whole revolutions (nothing moves).
pub(in super::super) fn wheel_turn(more: i64, gear: u32) -> Option<u32> {
    if gear == 0 {
        return None;
    }
    u32::try_from(more.rem_euclid(i64::from(gear)))
        .ok()
        .filter(|&teeth| teeth != 0)
}

/// Turns the provisional tier's index positions (and the snapped anchor) by `more` whole
/// teeth. A turn by a whole number of wheel revolutions changes nothing (`false`).
pub(super) fn turn(p: &mut ProvisionalSlice, more: i64) -> bool {
    let gear = p.design.meta.gear_teeth_abs();
    let Some(teeth) = wheel_turn(more, gear).map(f64::from) else {
        return false;
    };
    let index = p.tier_index;
    let Some(tier) = Arc::make_mut(&mut p.design).tiers.get_mut(index) else {
        return false;
    };
    let mut indices = rotate_indices(&tier.indices, teeth, gear);
    indices.sort_by(f64::total_cmp);
    tier.indices = indices;
    let anchor = rotate_indices(&[p.snapped.index], teeth, gear);
    p.snapped.index = anchor.first().copied().unwrap_or(p.snapped.index);
    true
}

/// Gives the provisional tier the whole symmetric orbit of its anchor index, or just
/// that one index; angle and mast stay as they are.
pub(super) fn rebuild_indices(p: &mut ProvisionalSlice, symmetric: bool) -> bool {
    let indices = {
        let meta = &p.design.meta;
        if symmetric {
            expected_orbit(
                p.snapped.index,
                meta.symmetry_order,
                meta.mirror,
                meta.gear_teeth_abs(),
            )
        } else {
            vec![p.snapped.index]
        }
    };
    let index = p.tier_index;
    let Some(tier) = Arc::make_mut(&mut p.design).tiers.get_mut(index) else {
        return false;
    };
    tier.indices = indices;
    tier.detached.clear();
    p.facet_map = None;
    true
}
