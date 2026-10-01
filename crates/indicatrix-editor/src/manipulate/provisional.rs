//! The Slice tool's PROVISIONAL tier: a line dragged across the stone becomes a facet
//! that is tweaked with the same three handles, then kept (one `Edit::AddTier`) or
//! discarded.
//!
//! [`ProvisionalSlice`] owns a clone of the committed design with the new tier appended
//! at `len()`; it never touches the editor's session or undo history. The UI renders it
//! by planning that clone as if it were the design, and keeps such frames out of every
//! cache that describes the committed design. While it exists the committed design must
//! not change underneath it: [`session_outlives`] compares generations.
//!
//! Everything here is GUI-free. The web app runs it as is; the desktop's own copy in
//! `apps/indicatrix-cut/src/gui/editor/manipulate/slice.rs` follows the same rules.

use super::{
    gesture::{AppliedEdit, GestureInputs, Step, tier_label},
    projection::{ScreenPoint, ScreenSize},
    slice::{SliceSide, SnappedFacet, slice_normal, slice_tier, snap_to_gear, tangency_mast},
    target::{CAMERA_FOV_DEG, HandleTarget, ids_aligned, target_for},
    text,
};
use crate::session::clamp_nudge_to_side;
use glam::Vec3;
use indicatrix::{
    geometry::meet_solver::{MeetConstraint, SolveStrategy, SolvedTier},
    optics::raytracer::Camera,
};
use indicatrix_cut_core::{ConstraintTier, Design, Edit, expected_orbit, rotate_indices};
use indicatrix_solid::{facet_map::FacetMap, preview::FrameGeometry};
use std::{collections::BTreeSet, rc::Rc, sync::Arc};

/// The angle step a sliced facet snaps to, in degrees.
pub const SLICE_ANGLE_STEP_DEG: f64 = 0.1;

/// The committed stone's corner points, shared with the frame they came from
/// (`FrameGeometry::corner_points`).
pub type CornerPoints = Arc<Vec<Vec3>>;

/// The generation a solid-preview frame of the PROVISIONAL slice design is stamped with.
///
/// Above every real `EditorSession` generation (a session would need 2^63 edits to get
/// there) but never `u64::MAX` itself. A frame carrying it describes a design that is not
/// the committed one, so a UI keeps it out of every cache of the committed design; see
/// [`frame_updates_mast_cache`].
pub const PROVISIONAL_GENERATION: u64 = u64::MAX - 1;

/// Whether a frame stamped `generation` describes the committed design, and so may
/// update the solved-mast cache and the tier table. `false` only for
/// [`PROVISIONAL_GENERATION`].
#[must_use]
pub const fn frame_updates_mast_cache(generation: u64) -> bool {
    generation != PROVISIONAL_GENERATION
}

/// Whether a provisional session taken against `base_generation` still describes the
/// design: it does exactly while the editor's generation has not moved.
#[must_use]
pub const fn session_outlives(base_generation: u64, current_generation: u64) -> bool {
    base_generation == current_generation
}

/// The whole-tooth turn `more` amounts to on a wheel of `gear` teeth.
///
/// A turn of the wheel is periodic, so `more` is reduced into `0..gear` (never dropped,
/// however large the `i64` is), which always fits a `u32` and converts to `f64` without
/// a cast. `None` for a wheel with no teeth and for a turn by whole revolutions
/// (nothing moves).
#[must_use]
pub fn wheel_turn(more: i64, gear: u32) -> Option<u32> {
    if gear == 0 {
        return None;
    }
    u32::try_from(more.rem_euclid(i64::from(gear)))
        .ok()
        .filter(|&teeth| teeth != 0)
}

/// How many of `tier_facets` (the provisional tier's facet ids) touch the stone.
///
/// Counts those with a centroid in the frame on screen. `None` while the facet-id space
/// of the map (`map_facets` planes) and of the frame's `centroids` disagree, so nothing
/// is known.
#[must_use]
pub fn surviving_facets(
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
#[must_use]
pub fn cut_hides_tier(cutoff: i32, tier_index: usize) -> bool {
    usize::try_from(cutoff).is_ok_and(|through| through < tier_index)
}

/// Whether Keep may commit a provisional tier with `surviving` facets on the stone: a
/// tier that cuts nothing (or whose picture has not landed yet) is refused.
#[must_use]
pub const fn keep_allowed(surviving: Option<usize>) -> bool {
    matches!(surviving, Some(count) if count > 0)
}

/// Why a provisional slice is being dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscardReason {
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
    #[must_use]
    pub const fn replans(self) -> bool {
        !matches!(self, Self::SelectionChanged)
    }

    /// The toast that says the session ended; `label` is the dropped tier's name.
    #[must_use]
    pub fn toast(self, label: &str) -> String {
        match self {
            Self::User => text::slice_discarded_toast(label),
            Self::DesignChanged => text::SLICE_CHANGED_TOAST.to_string(),
            Self::SelectionChanged => text::SLICE_SELECTION_TOAST.to_string(),
        }
    }
}

/// The dragged line and the camera it was drawn against, so Flip can recompute the
/// plane later even if the camera has moved since.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SliceLine {
    /// Where the drag started (pick frame).
    pub a: ScreenPoint,
    /// Where the drag ended (pick frame).
    pub b: ScreenPoint,
    /// The pick frame's size.
    pub size: ScreenSize,
    /// The camera's `(yaw, pitch, distance)` for that frame.
    pub pose: (f32, f32, f32),
}

impl SliceLine {
    /// The line `a -> b` drawn on the frame `geometry` describes.
    #[must_use]
    pub const fn new(a: ScreenPoint, b: ScreenPoint, geometry: &FrameGeometry) -> Self {
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
    #[must_use]
    pub fn normal(&self, side: SliceSide) -> Option<Vec3> {
        let (yaw, pitch, distance) = self.pose;
        let camera = Camera::new(yaw, pitch, distance, CAMERA_FOV_DEG);
        slice_normal(&camera, self.a, self.b, self.size, side)
    }
}

/// What a slice line turns into before it becomes a session.
#[derive(Debug, Clone)]
pub struct SlicePlan {
    /// The plane snapped to the gear.
    pub snapped: SnappedFacet,
    /// The provisional tier (`ScaleReference(tangency mast)`).
    pub tier: ConstraintTier,
}

/// The snapped facet and provisional tier for `line` on `design`'s stone.
///
/// `corners` are the committed stone's corner points: the plane just touches the stone
/// from outside, so the cut starts at zero depth. `None` for a degenerate line.
#[must_use]
pub fn plan_slice(
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
#[must_use]
pub fn with_provisional_tier(design: &Design, tier: ConstraintTier) -> Option<(Design, usize)> {
    let mut provisional = design.clone();
    let index = provisional.tiers.len();
    provisional.apply_edit(Edit::AddTier { index, tier }).ok()?;
    Some((provisional, index))
}

/// The provisional tier and its snapped facet as they were at a handle press, for
/// Escape.
#[derive(Debug, Clone)]
pub struct Snapshot {
    /// The provisional tier.
    pub tier: ConstraintTier,
    /// Its snapped facet.
    pub snapped: SnappedFacet,
}

/// The provisional slice: a design clone with the new tier last, and everything needed
/// to render, tweak, flip and keep it.
#[derive(Debug)]
pub struct ProvisionalSlice {
    /// The editor's generation when the session began.
    pub base_generation: u64,
    /// The committed design plus the provisional tier at `tier_index`.
    pub design: Arc<Design>,
    /// Where the provisional tier sits: the committed design's tier count.
    pub tier_index: usize,
    /// Which side of the line is cut away.
    pub side: SliceSide,
    /// The snapped facet; `index` follows the index handle so the Symmetric toggle can
    /// rebuild the orbit from it.
    pub snapped: SnappedFacet,
    /// The dragged line, for Flip.
    pub line: SliceLine,
    /// The COMMITTED stone's corner points (a provisional frame's geometry describes
    /// the cut stone).
    pub base_corners: CornerPoints,
    /// The solved masts of the latest provisional frame.
    pub masts: Option<Vec<SolvedTier>>,
    /// The facet map of `design` against `masts`, rebuilt lazily.
    facet_map: Option<Rc<FacetMap>>,
    /// The facet ids currently outlined green.
    outline: Vec<u32>,
    /// Whether `masts` changed since the outline was last computed.
    masts_new: bool,
    /// How many of the tier's facets touch the stone in the latest provisional frame
    /// (`None` until one has landed for this design) -- see [`surviving_facets`].
    surviving: Option<usize>,
}

impl ProvisionalSlice {
    /// The session for `line` cutting away `side` of `design`'s stone (`corners` are
    /// the committed stone's corner points), begun against `base_generation`. `None`
    /// for a degenerate line.
    #[must_use]
    pub fn build(
        line: SliceLine,
        side: SliceSide,
        corners: CornerPoints,
        design: &Design,
        base_generation: u64,
        symmetric: bool,
    ) -> Option<Self> {
        let plan = plan_slice(&line, side, &corners, design, symmetric)?;
        let (provisional, tier_index) = with_provisional_tier(design, plan.tier)?;
        Some(Self {
            base_generation,
            design: Arc::new(provisional),
            tier_index,
            side,
            snapped: plan.snapped,
            line,
            base_corners: corners,
            masts: None,
            facet_map: None,
            outline: Vec::new(),
            masts_new: false,
            surviving: None,
        })
    }

    /// The provisional tier.
    #[must_use]
    pub fn tier(&self) -> Option<&ConstraintTier> {
        self.design.tiers.get(self.tier_index)
    }

    /// The provisional tier's name for hints, buttons and toasts.
    #[must_use]
    pub fn label(&self) -> Option<String> {
        self.tier().map(|tier| tier_label(tier, self.tier_index))
    }

    /// The facet map of the provisional design, built on first use after a change.
    /// `None` until a frame's masts describe the design.
    pub fn facet_map(&mut self) -> Option<Rc<FacetMap>> {
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
    #[must_use]
    pub fn facet_count(&self) -> usize {
        self.surviving
            .unwrap_or_else(|| self.tier().map_or(0, |tier| tier.indices.len()))
    }

    /// Whether Keep may commit the tier now -- see [`keep_allowed`].
    #[must_use]
    pub const fn may_keep(&self) -> bool {
        keep_allowed(self.surviving)
    }

    /// The hint for the provisional tier as it stands.
    #[must_use]
    pub fn hint(&self) -> String {
        let Some(tier) = self.tier() else {
            return String::new();
        };
        if self.surviving == Some(0) {
            return format!(
                "New tier {}: {}. Esc discards it.",
                tier_label(tier, self.tier_index),
                text::SLICE_NO_DEPTH_HINT
            );
        }
        text::slice_provisional_hint(
            &tier_label(tier, self.tier_index),
            self.facet_count(),
            tier.angle_deg,
            self.snapped.index,
        )
    }

    /// The handle target on the provisional tier for the frame `geometry`, or `None`
    /// before a frame's masts describe it.
    pub fn place(&mut self, geometry: &FrameGeometry) -> Option<HandleTarget> {
        let map = self.facet_map()?;
        target_for(geometry, &map, &self.design, self.tier_index, None, true)
    }

    /// The provisional tier and snapped facet now, to restore on Escape.
    #[must_use]
    pub fn snapshot(&self) -> Option<Snapshot> {
        self.tier().map(|tier| Snapshot {
            tier: tier.clone(),
            snapped: self.snapped,
        })
    }

    /// What a handle press on the provisional tier starts from.
    #[must_use]
    pub fn gesture_inputs(&self) -> Option<GestureInputs> {
        let tier = self.tier()?;
        Some(GestureInputs {
            start_angle_deg: tier.angle_deg,
            masts: self.masts.clone(),
            restore: self.snapshot(),
        })
    }

    /// What the next provisional replan chains from, given `committed_masts` (the solved
    /// masts of the committed design, if the UI has them): the masts of the session's
    /// own last frame once they describe every tier, with `dirty = {tier}` -- a subgraph
    /// re-solve that fits the preview budget on a large design. Before any frame has
    /// landed (or after a Flip) the tier is pinned by its scale reference, so the
    /// committed masts plus that pinned mast chain just as well. Otherwise `(None, {})`:
    /// a full solve.
    #[must_use]
    pub fn replan_chain(
        &self,
        committed_masts: Option<&[SolvedTier]>,
    ) -> (Option<Vec<SolvedTier>>, BTreeSet<usize>) {
        let tier_count = self.design.tiers.len();
        if let Some(masts) = self.masts.as_deref().filter(|m| m.len() == tier_count) {
            return (Some(masts.to_vec()), BTreeSet::from([self.tier_index]));
        }
        let pinned = match self.tier().map(|tier| &tier.constraint) {
            Some(MeetConstraint::ScaleReference(mast)) => Some(*mast),
            _ => None,
        };
        match (committed_masts, pinned) {
            (Some(committed), Some(mast)) if committed.len() + 1 == tier_count => {
                let mut masts = committed.to_vec();
                masts.push(SolvedTier {
                    mast,
                    strategy: SolveStrategy::ScaleReference,
                    detail: "given (scale reference)".to_string(),
                });
                (Some(masts), BTreeSet::from([self.tier_index]))
            }
            _ => (None, BTreeSet::new()),
        }
    }

    /// The solved masts of a provisional-generation frame. Kept only when they describe
    /// the session's design one to one.
    pub fn note_masts(&mut self, masts: Vec<SolvedTier>) {
        if masts.len() == self.design.tiers.len() {
            self.masts = Some(masts);
            self.facet_map = None;
            self.masts_new = true;
        }
    }

    /// Turns a landed provisional frame's masts into the green outline: `Some((changed,
    /// ids))` with the tier's facet ids when new masts arrived since the last call
    /// (`changed` says whether the ids differ from the outline already shown), `None`
    /// when nothing is new. Cheap in that case. `geometry` (the frame on screen) also
    /// refreshes how many facets touch the stone.
    pub fn take_outline_update(
        &mut self,
        geometry: Option<&FrameGeometry>,
    ) -> Option<(bool, Vec<u32>)> {
        if !std::mem::take(&mut self.masts_new) {
            return None;
        }
        let map = self.facet_map()?;
        let ids = map.facets_of_tier(self.tier_index).to_vec();
        if let Some(geometry) = geometry {
            self.surviving = surviving_facets(&ids, &geometry.facet_centroids, map.facet_count());
        }
        let changed = ids != self.outline;
        if changed {
            self.outline.clone_from(&ids);
        }
        Some((changed, ids))
    }

    /// Applies one throttled handle step to the provisional tier in place (no history,
    /// no panel refresh). `None` when nothing changed.
    pub fn apply_step(&mut self, step: Step) -> Option<AppliedEdit> {
        let changed = match step {
            Step::Angle(deg) => self.set_angle(deg),
            Step::Mast(mast) => self.set_mast(mast),
            Step::Teeth(more) => self.turn(more),
        };
        if changed {
            self.facet_map = None;
        }
        changed.then_some(AppliedEdit {
            generation: 0,
            replaced_meet: None,
        })
    }

    /// Sets the provisional tier's angle, held to its side of zero like the keyboard
    /// nudge.
    fn set_angle(&mut self, deg: f64) -> bool {
        let index = self.tier_index;
        let Some(tier) = Arc::make_mut(&mut self.design).tiers.get_mut(index) else {
            return false;
        };
        let new = clamp_nudge_to_side(tier.angle_deg, deg);
        if !new.is_finite() || new.to_bits() == tier.angle_deg.to_bits() {
            return false;
        }
        tier.angle_deg = new;
        self.snapped.angle_deg = new;
        true
    }

    /// Pins the provisional tier at `mast`.
    fn set_mast(&mut self, mast: f64) -> bool {
        let index = self.tier_index;
        let Some(tier) = Arc::make_mut(&mut self.design).tiers.get_mut(index) else {
            return false;
        };
        let unchanged = matches!(&tier.constraint, MeetConstraint::ScaleReference(m) if m.to_bits() == mast.to_bits());
        if !mast.is_finite() || unchanged {
            return false;
        }
        tier.constraint = MeetConstraint::ScaleReference(mast);
        true
    }

    /// Turns the provisional tier's index positions (and the snapped anchor) by `more`
    /// whole teeth. A turn by a whole number of wheel revolutions changes nothing
    /// (`false`).
    fn turn(&mut self, more: i64) -> bool {
        let gear = self.design.meta.gear_teeth_abs();
        let Some(teeth) = wheel_turn(more, gear).map(f64::from) else {
            return false;
        };
        let index = self.tier_index;
        let Some(tier) = Arc::make_mut(&mut self.design).tiers.get_mut(index) else {
            return false;
        };
        let mut indices = rotate_indices(&tier.indices, teeth, gear);
        indices.sort_by(f64::total_cmp);
        tier.indices = indices;
        let anchor = rotate_indices(&[self.snapped.index], teeth, gear);
        self.snapped.index = anchor.first().copied().unwrap_or(self.snapped.index);
        true
    }

    /// Puts the provisional tier back the way a handle press found it (Escape
    /// mid-drag). `false` when the tier is gone.
    pub fn restore(&mut self, snapshot: Snapshot) -> bool {
        let index = self.tier_index;
        let Some(slot) = Arc::make_mut(&mut self.design).tiers.get_mut(index) else {
            return false;
        };
        *slot = snapshot.tier;
        self.snapped = snapshot.snapped;
        self.facet_map = None;
        true
    }

    /// Gives the provisional tier the whole symmetric orbit of its anchor index, or
    /// just that one index; angle and mast stay as they are.
    pub fn rebuild_indices(&mut self, symmetric: bool) -> bool {
        let indices = {
            let meta = &self.design.meta;
            if symmetric {
                expected_orbit(
                    self.snapped.index,
                    meta.symmetry_order,
                    meta.mirror,
                    meta.gear_teeth_abs(),
                )
            } else {
                vec![self.snapped.index]
            }
        };
        let index = self.tier_index;
        let Some(tier) = Arc::make_mut(&mut self.design).tiers.get_mut(index) else {
            return false;
        };
        tier.indices = indices;
        tier.detached.clear();
        self.facet_map = None;
        true
    }
}

/// The hint that belongs to no handle: the Cut-slider warning when `cutoff` hides the
/// provisional tier, else the provisional tier's own hint, else Slice mode's, else
/// nothing.
#[must_use]
pub fn resting_hint(
    provisional: Option<&ProvisionalSlice>,
    cutoff: i32,
    slice_mode: bool,
    symmetric: bool,
) -> String {
    provisional
        .filter(|p| cut_hides_tier(cutoff, p.tier_index))
        .map(|_| text::SLICE_CUT_SLIDER_HINT.to_string())
        .or_else(|| provisional.map(ProvisionalSlice::hint))
        .or_else(|| slice_mode.then(|| text::slice_mode_hint(symmetric)))
        .unwrap_or_default()
}
