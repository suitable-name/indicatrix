//! Resolving concave tiers to [`ToolPrimitive`]s (plan §4.4).
//!
//! The kernel never sees a tier or a notation, only convex tool volumes, so this
//! is the one place that turns the authored two-line tier into geometry. Every
//! convention-dependent step is delegated to [`super::concave_frame`]; this file
//! only orders, counts and validates.

use core::fmt;

use glam::DVec3;
use indicatrix::geometry::{
    meet_solver::SolvedTier,
    stone_metrics::measure_solid_with_vertices,
    tool::{MAX_TOOL_PRIMITIVES, ToolPrimitive},
};

use super::concave_frame::{
    dop_frame, primitive_for, stone_width, support_vertex, tool_axis, tool_centre,
};
use crate::design::{ConcaveTier, ConcaveTierError, Design, TierRef};

/// Why a design's concave tiers could not be resolved to tools.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum ConcaveResolveError {
    /// Concave tier `tier` (index into [`Design::concave_tiers`]) fails
    /// [`crate::design::ConcaveTier::validate`] or clashes with a flat name.
    Invalid {
        /// Position of the bad tier.
        tier: usize,
        /// What is wrong with it.
        error: ConcaveTierError,
    },
    /// The flat stone does not close, so there is no width `W` and no contact
    /// point to measure the tools from.
    NoFlatSolid,
    /// More placements than the kernel's [`MAX_TOOL_PRIMITIVES`].
    TooManyPlacements {
        /// Placements in the design.
        count: usize,
        /// The ceiling.
        max: usize,
    },
}

impl fmt::Display for ConcaveResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid { tier, error } => {
                write!(f, "concave tier {}: {error}", tier + 1)
            }
            Self::NoFlatSolid => f.write_str(
                "the flat facets do not close into a stone, so the concave tools have no \
                 width to be measured against",
            ),
            Self::TooManyPlacements { count, max } => write!(
                f,
                "{count} concave placements exceed the limit of {max} per stone"
            ),
        }
    }
}

impl std::error::Error for ConcaveResolveError {}

/// `(tier index, placement index)` for each primitive, in primitive order.
///
/// The tier index is into [`Design::concave_tiers`], the placement index into
/// that tier's `indices`. Kept beside the primitives rather than in them so the
/// GPU struct carries no bookkeeping.
pub type ToolPlacements = Vec<(usize, usize)>;

/// Flat facet planes and tool primitives, without the placement list: what
/// [`Design::facet_geometry_from_solved`] returns.
pub type FlatAndTools2 = (Vec<(DVec3, f64)>, Vec<ToolPrimitive>);

/// What [`Design::geometry_from_solved`] returns: the flat planes, the tools and
/// their placements. A named tuple so the signature stays readable.
pub type FlatAndTools = (Vec<(DVec3, f64)>, Vec<ToolPrimitive>, ToolPlacements);

impl Design {
    /// Resolves every concave tier to tool primitives under the `v0` frame
    /// ([`super::concave_frame`]), one per placement, in [`Self::cutting_order`]
    /// order of the concave tiers and then index order.
    ///
    /// The stone width `W` is the **finished flat** stone's
    /// ([`super::concave_frame::stone_width`]): one `W` for every tier, so concave
    /// edits, the order of the tiers and the step a tier is cut at never move it. A
    /// design with no concave tiers returns empty vectors without solving or
    /// measuring anything, which keeps every planar caller byte-identical.
    ///
    /// # Errors
    ///
    /// [`ConcaveResolveError`] when a tier is invalid, the placements exceed
    /// [`MAX_TOOL_PRIMITIVES`], or the flat stone does not close.
    ///
    /// # Panics
    ///
    /// Same alignment contract as [`Self::planes_from_solved`] when there are
    /// concave tiers: `solved` must have one entry per flat tier.
    pub fn concave_tools_from_solved(
        &self,
        solved: &[SolvedTier],
    ) -> Result<(Vec<ToolPrimitive>, ToolPlacements), ConcaveResolveError> {
        if self.concave_tiers.is_empty() {
            return Ok((Vec::new(), Vec::new()));
        }
        self.check_concave_resolvable()?;
        let planes = self.planes_from_solved(solved);
        self.resolve_concave(&planes, &self.concave_cutting_order())
    }

    /// [`Self::concave_tools_from_solved`] for a caller that already holds the flat
    /// stone: `planes` must be this design's own [`Self::planes_from_solved`]. The
    /// tools are the very ones `concave_tools_from_solved` returns for that solve (one
    /// resolver, one placement rule), without building the arrangement a second time.
    ///
    /// The scoring paths (the optimizer, Retarget, the angle sweep) use this so they
    /// measure the stone the Compare window draws, tools included.
    ///
    /// # Errors
    ///
    /// As [`Self::concave_tools_from_solved`].
    pub fn concave_tools_from_planes(
        &self,
        planes: &[(DVec3, f64)],
    ) -> Result<(Vec<ToolPrimitive>, ToolPlacements), ConcaveResolveError> {
        if self.concave_tiers.is_empty() {
            return Ok((Vec::new(), Vec::new()));
        }
        self.check_concave_resolvable()?;
        self.resolve_concave(planes, &self.concave_cutting_order())
    }

    /// The tools to measure or draw with `planes` ([`Self::planes_from_solved`]):
    /// [`Self::concave_tools_from_planes`] with a failure read as "no tools".
    ///
    /// A design whose concave tiers do not resolve (an invalid tier, too many
    /// placements, a flat stone that does not close) is scored on its flat stone, as
    /// every display path does (`indicatrix_solid::live_update::display_tools` drops
    /// unresolved tools the same way): the flat stone is truthful, a half-resolved set
    /// of tools is not. Empty, without solving or measuring anything, for a design with no concave
    /// tiers, so a flat design's numbers are exactly what they were.
    #[must_use]
    pub fn tools_for_scoring(&self, planes: &[(DVec3, f64)]) -> Vec<ToolPrimitive> {
        self.concave_tools_from_planes(planes)
            .map(|(tools, _)| tools)
            .unwrap_or_default()
    }

    /// Tools of the concave tiers that precede `through` in
    /// [`Self::cutting_order`] (plan §4.4): the concave half of "the stone after
    /// step k" for the tier-cutoff slider. `through` itself is not included, and a
    /// `through` that is not in the order (a stale index) means "every tier",
    /// like [`Self::planes_through_tier`] past the end.
    ///
    /// The tools are the finished stone's tools for those tiers, bit for bit: a
    /// partial stone is not re-measured, because `W` is the finished width by
    /// definition (see `stone_width`).
    ///
    /// # Errors
    ///
    /// As [`Self::concave_tools_from_solved`].
    ///
    /// # Panics
    ///
    /// As [`Self::concave_tools_from_solved`].
    pub fn concave_tools_through_tier(
        &self,
        solved: &[SolvedTier],
        through: TierRef,
    ) -> Result<(Vec<ToolPrimitive>, ToolPlacements), ConcaveResolveError> {
        if self.concave_tiers.is_empty() {
            return Ok((Vec::new(), Vec::new()));
        }
        self.check_concave_resolvable()?;
        let order: Vec<usize> = self
            .cutting_order()
            .into_iter()
            .take_while(|&tier| tier != through)
            .filter_map(|tier| match tier {
                TierRef::Concave(i) => Some(i),
                TierRef::Flat(_) => None,
            })
            .collect();
        let planes = self.planes_from_solved(solved);
        self.resolve_concave(&planes, &order)
    }

    /// Planes and tools in one call, so the flat arrangement is built once.
    /// `tools` is empty for a design without concave tiers, and `planes` is then
    /// exactly [`Self::planes_from_solved`].
    ///
    /// # Errors
    ///
    /// As [`Self::concave_tools_from_solved`].
    ///
    /// # Panics
    ///
    /// Same alignment contract as [`Self::planes_from_solved`].
    pub fn geometry_from_solved(
        &self,
        solved: &[SolvedTier],
    ) -> Result<FlatAndTools, ConcaveResolveError> {
        let planes = self.planes_from_solved(solved);
        if self.concave_tiers.is_empty() {
            return Ok((planes, Vec::new(), Vec::new()));
        }
        self.check_concave_resolvable()?;
        let (tools, placements) = self.resolve_concave(&planes, &self.concave_cutting_order())?;
        Ok((planes, tools, placements))
    }

    /// [`Self::geometry_from_solved`] for a consumer that treats the FACET planes
    /// alone as the stone, as the rough planner does (the preform is a generous
    /// default cylinder, so a schedule that closes by itself is measured without
    /// it). Returns those facet planes (the arrangement without the leading
    /// preform planes) and the tools resolved against that same stone.
    ///
    /// The tools are placed on the stone the caller measures, not on the editor's
    /// preform-trimmed one: a tool's size is a fraction of the stone width `W` and
    /// its centre sits on a vertex of the stone, so resolving against another
    /// stone would carve the planner's stone with tools scaled to a different one.
    /// When the preform does not trim the facet stone (the usual case) the two
    /// agree exactly.
    ///
    /// An open facet stone (the facets alone do not close) has nothing to carve
    /// and no `W`: it returns no tools instead of
    /// [`ConcaveResolveError::NoFlatSolid`], because the caller classifies such a
    /// design as unbounded from its planes. An invalid tier or too many
    /// placements still fail first.
    ///
    /// # Errors
    ///
    /// [`ConcaveResolveError::Invalid`] or
    /// [`ConcaveResolveError::TooManyPlacements`].
    ///
    /// # Panics
    ///
    /// Same alignment contract as [`Self::planes_from_solved`].
    pub fn facet_geometry_from_solved(
        &self,
        solved: &[SolvedTier],
    ) -> Result<FlatAndTools2, ConcaveResolveError> {
        let mut planes = self.planes_from_solved(solved);
        planes.drain(..self.preform.planes().len().min(planes.len()));
        if self.concave_tiers.is_empty() {
            return Ok((planes, Vec::new()));
        }
        self.check_concave_resolvable()?;
        match self.resolve_concave(&planes, &self.concave_cutting_order()) {
            Ok((tools, _)) => Ok((planes, tools)),
            Err(ConcaveResolveError::NoFlatSolid) => Ok((planes, Vec::new())),
            Err(e) => Err(e),
        }
    }

    /// Concave tier positions in cutting order.
    fn concave_cutting_order(&self) -> Vec<usize> {
        self.cutting_order()
            .into_iter()
            .filter_map(|tier| match tier {
                TierRef::Concave(i) => Some(i),
                TierRef::Flat(_) => None,
            })
            .collect()
    }

    /// The checks that need no geometry: tier validity and the placement ceiling.
    fn check_concave_resolvable(&self) -> Result<(), ConcaveResolveError> {
        self.validate_concave_tiers()
            .map_err(|(tier, error)| ConcaveResolveError::Invalid { tier, error })?;
        let count = self.concave_placement_count();
        if count > MAX_TOOL_PRIMITIVES {
            return Err(ConcaveResolveError::TooManyPlacements {
                count,
                max: MAX_TOOL_PRIMITIVES,
            });
        }
        Ok(())
    }

    /// Emits one primitive per placement of `tiers` (concave positions, in the
    /// order to emit them) against the flat stone `planes`.
    fn resolve_concave(
        &self,
        planes: &[(DVec3, f64)],
        tiers: &[usize],
    ) -> Result<(Vec<ToolPrimitive>, ToolPlacements), ConcaveResolveError> {
        resolve_ordered(
            planes,
            &self.concave_tiers,
            tiers,
            (self.meta.gear_reference_angle, self.meta.gear_teeth),
        )
    }
}

/// Emits one primitive per placement of `tiers[order[k]]`, in `order`, against the flat
/// stone `planes`; `gear` is the index gear's `(reference angle, tooth count)`.
///
/// The one placement rule: [`Design`]'s resolvers and [`resolve_tiers_on_planes`] both
/// end here, so a tool is the same primitive whichever of them asked.
///
/// `W` is `stone_width` of `planes`, which must therefore be the FINISHED flat stone (or
/// the facet stone, for the planner), never a prefix.
fn resolve_ordered(
    planes: &[(DVec3, f64)],
    tiers: &[ConcaveTier],
    order: &[usize],
    gear: (f64, i32),
) -> Result<(Vec<ToolPrimitive>, ToolPlacements), ConcaveResolveError> {
    if order.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    let (gear_reference_angle, gear_teeth) = gear;
    let (metrics, vertices) =
        measure_solid_with_vertices(planes).ok_or(ConcaveResolveError::NoFlatSolid)?;
    let width = stone_width(&metrics);

    let mut tools = Vec::new();
    let mut placements = Vec::new();
    for &tier_index in order {
        let tier = &tiers[tier_index];
        for (placement, &index) in tier.indices.iter().enumerate() {
            // The reference angle is part of the wheel position, as it is for
            // the flat facets (`StandardGemCuts::index_to_azimuth`).
            let frame = dop_frame(tier.angle_deg, index, gear_reference_angle, gear_teeth);
            let c0 = support_vertex(&vertices, frame.2, width)
                .ok_or(ConcaveResolveError::NoFlatSolid)?;
            let centre = tool_centre(c0, width, frame, tier.displacement);
            let axis = tool_axis(tier, frame);
            let tool = primitive_for(tier, centre, axis, width);
            // The geometry narrows to `f32` here, so a finite tier can still
            // overflow or collapse; the kernel never sees such a primitive.
            tool.validate().map_err(|e| ConcaveResolveError::Invalid {
                tier: tier_index,
                error: ConcaveTierError::ToolGeometry {
                    reason: e.to_string(),
                },
            })?;
            tools.push(tool);
            placements.push((tier_index, placement));
        }
    }
    Ok((tools, placements))
}

/// Resolves loose concave `tiers` (in the order given) against the flat stone `planes`.
///
/// The planes are `n . x <= m`. This is for a stone that has no [`Design`] to hold the
/// tiers: a library record that keeps its concave tiers only as rows of its angle table.
///
/// Each tier is validated against `gear_teeth` (the same rules a design applies, bar the
/// name clash with a flat tier, which needs the design) and the placements are counted
/// against [`MAX_TOOL_PRIMITIVES`]; the primitives are exactly the ones
/// [`Design::concave_tools_from_planes`] makes for a design holding the same tiers.
///
/// # Errors
///
/// [`ConcaveResolveError`]: an invalid tier, too many placements, or a flat stone that
/// does not close.
pub fn resolve_tiers_on_planes(
    planes: &[(DVec3, f64)],
    tiers: &[ConcaveTier],
    gear_teeth: i32,
    gear_reference_angle: f64,
) -> Result<(Vec<ToolPrimitive>, ToolPlacements), ConcaveResolveError> {
    for (tier, candidate) in tiers.iter().enumerate() {
        candidate
            .validate(gear_teeth)
            .map_err(|error| ConcaveResolveError::Invalid { tier, error })?;
    }
    let count: usize = tiers.iter().map(|tier| tier.indices.len()).sum();
    if count > MAX_TOOL_PRIMITIVES {
        return Err(ConcaveResolveError::TooManyPlacements {
            count,
            max: MAX_TOOL_PRIMITIVES,
        });
    }
    let order: Vec<usize> = (0..tiers.len()).collect();
    resolve_ordered(planes, tiers, &order, (gear_reference_angle, gear_teeth))
}
