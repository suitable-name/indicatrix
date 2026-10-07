//! [`check_manufacturability`], the orchestrator, plus checks 1 and 2 (the
//! two that need a real solved mesh): [`check_vanishing_facets`] and
//! [`check_undersized_facets`], and the plane-boundary/area machinery they
//! share. See the parent module's doc comment for why these run off an
//! already-solved design rather than forcing a second solve.

use super::{
    tool_cull::{Bound, Plane, boxes_separated, needed_planes, outside_stone},
    warning::ManufacturabilityWarning,
};
use crate::design::Design;
use glam::DVec3;
use indicatrix::geometry::{
    meet_solver::{SolveStrategy, SolvedTier},
    stone_metrics::{
        SolidMesh, SolidStatus, TOOL_SEGMENTS, ToolBounds, build_solid_mesh, build_solid_mesh_geom,
        measure_solid, measure_solid_with_vertices, mesh_volume, tessellate_tool, tool_bounds,
    },
    tool::ToolPrimitive,
};
use std::{
    collections::BTreeMap,
    sync::{Mutex, PoisonError},
};

#[cfg(test)]
mod reference_tests;

/// Default minimum facet area for [`check_undersized_facets`].
///
/// Expressed as a fraction of the stone's own measured width squared rather than
/// an absolute number -- masts (and areas) are in an arbitrary per-design "mast
/// unit" scale, so an absolute threshold would mean a different real fraction of
/// the stone on every design.
///
/// **A reasoned default, not a corpus-measured one**: `1e-4` is `(1%)^2`, i.e. it
/// flags a facet whose *linear* extent is below roughly 1% of the stone's own
/// width -- smaller than that and a standard faceting lap has no real margin to
/// polish the facet flat without rounding its edges into its neighbors. Exposed
/// as a parameter so a caller who disagrees can override it.
pub const DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2: f64 = 1e-4;

/// Runs all five checks over `design`'s current authored state, then (only when it
/// has concave tiers) [`check_concave_tools`].
///
/// Uses `solved` (an already-[`Design::solve`]'d, or [`Design::resolve_dirty`]'d,
/// mast list -- see this module's doc comment for why a second solve is not
/// forced here) for the two checks that need real geometry.
///
/// `min_facet_area_fraction_of_w2` is [`check_undersized_facets`]'s threshold,
/// expressed as a fraction of the solid's measured width squared -- pass
/// [`DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2`] for the reasoned default.
///
/// Checks 3/4/5 always run (they need no mast at all). Checks 1/2 run only when
/// `solved` actually closes into a real solid ([`SolidStatus::Closed`]) -- an
/// unbounded or degenerate arrangement has no well-defined facet set to check.
///
/// # Panics
///
/// Same alignment contract as [`Design::planes_from_solved`]: `solved` must have
/// one entry per tier `design` currently has, in the same order.
#[must_use]
pub fn check_manufacturability(
    design: &Design,
    solved: &[SolvedTier],
    min_facet_area_fraction_of_w2: f64,
) -> Vec<ManufacturabilityWarning> {
    let mut warnings = super::authored_checks::check_gear_quantization(design);
    warnings.extend(super::authored_checks::check_cut_order(design));
    warnings.extend(super::authored_checks::check_meet_name_asc_safety(design));

    let planes = design.planes_from_solved(solved);
    if let SolidStatus::Closed(mesh) = build_solid_mesh(&planes) {
        let schedule = design.to_asc_schedule_from_solved(solved);
        let boundaries = facet_plane_boundaries(&schedule);
        let preform_len = design.preform.planes().len();
        let ring_by_index: BTreeMap<usize, &Vec<glam::DVec3>> =
            mesh.rings.iter().map(|(i, ring)| (*i, ring)).collect();

        warnings.extend(check_vanishing_facets(
            design,
            preform_len,
            &boundaries,
            &ring_by_index,
        ));

        if let Some(metrics) = measure_solid(&planes) {
            let threshold = min_facet_area_fraction_of_w2 * metrics.width_axis * metrics.width_axis;
            warnings.extend(check_undersized_facets(
                design,
                preform_len,
                &boundaries,
                &ring_by_index,
                threshold,
                metrics.width_axis,
            ));
        }
    }

    warnings.extend(check_concave_tools(
        design,
        solved,
        min_facet_area_fraction_of_w2,
    ));

    warnings
}

/// Cumulative facet-plane count contributed by `schedule.tiers[0..=i]`, for every
/// `i` -- i.e. `boundaries[i]` is how many facet planes
/// `StandardGemCuts::from_asc_schedule(schedule)` places at or before tier `i`'s
/// own contribution, in [`Design::planes`]'s combined list (offset by the
/// preform's own plane count, which the caller adds).
///
/// # Why this calls the real production function repeatedly instead of re-deriving plane counts
///
/// `StandardGemCuts::from_asc_schedule` deduplicates near-identical planes (two
/// `.asc` tier rows occasionally produce the exact same half-space), so a tier's
/// contribution is not always exactly `indices.len().max(1)`. Dedup only ever
/// *drops* an element based on what came before it, so calling
/// [`StandardGemCuts::from_asc_schedule`] once per prefix and taking the length is
/// byte-for-byte consistent with a single call over the whole schedule, avoiding a
/// second copy of the dedup rule that could drift out of sync. This costs O(n^2)
/// tier-generation passes rather than O(n), but `n` is capped at
/// `meet_solver::MAX_PLANES = 400` and each pass is well under a millisecond,
/// negligible next to the solve this module's caller already paid for.
///
/// `pub`, not re-exported crate-wide, since this `mesh_checks` module is
/// itself private: [`crate::design::Design::tier_for_plane_index`] reaches
/// this via `crate::manufacturability`'s own `pub(crate)` re-export, turning
/// an escaping [`SolidStatus::Unbounded`] plane index into a tier index
/// without re-deriving this same offset itself; nothing outside this crate
/// needs the raw boundaries directly, only that per-index lookup.
///
/// [`StandardGemCuts::from_asc_schedule`]: indicatrix::geometry::cuts::StandardGemCuts::from_asc_schedule
pub fn facet_plane_boundaries(schedule: &indicatrix_formats::asc::AscSchedule) -> Vec<usize> {
    (0..schedule.tiers.len())
        .map(|i| {
            let prefix = indicatrix_formats::asc::AscSchedule {
                gemcad_version: String::new(),
                gear_teeth: schedule.gear_teeth,
                gear_reference_angle: 0.0,
                symmetry_order: 0,
                mirror: false,
                refractive_index: 0.0,
                headers: Vec::new(),
                footnotes: Vec::new(),
                tiers: schedule.tiers[..=i].to_vec(),
                warnings: Vec::new(),
                line_ending: schedule.line_ending,
            };
            indicatrix::geometry::cuts::StandardGemCuts::from_asc_schedule(&prefix).len()
        })
        .collect()
}

/// Check 1: a facet plane this tier describes never reaches the solid's surface --
/// see the module docs.
///
/// `preform_len` and `boundaries` (from [`facet_plane_boundaries`]) locate each
/// tier's own slice of [`Design::planes`]'s combined list; `ring_by_index` (keyed
/// by that same combined-list index) is present for exactly the planes
/// `SolidMesh::rings` kept -- so a plane index in a tier's range absent from
/// `ring_by_index` is exactly a vanished facet.
///
/// A tier whose own facet-plane count falls short of its authored
/// `indices.len().max(1)` had one of its planes deduplicated against an earlier
/// tier's identical plane (see [`facet_plane_boundaries`]) -- a data redundancy,
/// not a vanished facet, and not attributable to one specific index without
/// re-deriving which one collided. `vanished`/`total` are always exact counts, but
/// the exact index value is not necessarily reliable in that rare case.
fn check_vanishing_facets(
    design: &Design,
    preform_len: usize,
    boundaries: &[usize],
    ring_by_index: &BTreeMap<usize, &Vec<glam::DVec3>>,
) -> Vec<ManufacturabilityWarning> {
    let mut warnings = Vec::new();
    let mut prev = 0usize;
    for (tier_index, tier) in design.tiers.iter().enumerate() {
        let end = boundaries[tier_index];
        let total = end - prev;
        let vanished = (preform_len + prev..preform_len + end)
            .filter(|i| !ring_by_index.contains_key(i))
            .count();
        if vanished > 0 {
            warnings.push(ManufacturabilityWarning::VanishingFacet {
                tier_index,
                tier_id: design.tier_id_at_or_synthetic(tier_index),
                tier_name: tier.name.clone(),
                vanished,
                total,
            });
        }
        prev = end;
    }
    warnings
}

/// Check 2: a facet survives but its polygon area is below `threshold` --
/// see the module docs and [`DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2`].
///
/// `width_axis` is passed straight through onto every
/// [`ManufacturabilityWarning::UndersizedFacet`] this raises, unused for the
/// threshold comparison itself (already baked into `threshold` by the caller) --
/// only so that warning's `Display` impl can report a cutter-meaningful
/// percentage of stone width instead of a raw, design-scale area.
fn check_undersized_facets(
    design: &Design,
    preform_len: usize,
    boundaries: &[usize],
    ring_by_index: &BTreeMap<usize, &Vec<glam::DVec3>>,
    threshold: f64,
    width_axis: f64,
) -> Vec<ManufacturabilityWarning> {
    let mut warnings = Vec::new();
    let mut prev = 0usize;
    for (tier_index, tier) in design.tiers.iter().enumerate() {
        let end = boundaries[tier_index];
        for plane_index in preform_len + prev..preform_len + end {
            if let Some(ring) = ring_by_index.get(&plane_index) {
                let area = polygon_area(ring);
                if area < threshold {
                    warnings.push(ManufacturabilityWarning::UndersizedFacet {
                        tier_index,
                        tier_id: design.tier_id_at_or_synthetic(tier_index),
                        tier_name: tier.name.clone(),
                        facet_plane_index: plane_index,
                        area,
                        threshold,
                        width_axis,
                    });
                }
            }
        }
        prev = end;
    }
    warnings
}

/// A tool or overlap that removes less than this fraction of the stone's volume
/// removes nothing a cutter could see: it is a graze, not a cut.
const REMOVED_VOLUME_FRACTION: f64 = 1e-9;

/// Two facet normals at least this far apart (cosine at most `-0.9`, about 154
/// degrees) face opposite ways for the break-through test.
const OPPOSITE_FACET_COS: f64 = -0.9;

/// A reciprocating tool's break-through test ignores facets whose normal is
/// within 60 degrees of its axis (cosine above `0.5`): those are the ends of its
/// stroke, which leaves the stone there by design.
const STROKE_END_FACET_COS: f64 = 0.5;

/// Polygon sides for the overlap test's tool tessellation. Coarse on purpose: the
/// test runs once per pair of nearby tools, and an octagon's equal-area radius
/// keeps its overlap estimate within the noise of a threshold that is already a
/// graze. (A ball is always its 320-face icosphere, whatever this says; the pairs it
/// takes part in are kept cheap by the bounding-box test instead.)
const OVERLAP_SEGMENTS: usize = 8;

/// A tool that touches the stone's surface with less than this fraction of the stone's
/// width squared, in total facet area, is treated as not reaching it: such a patch is
/// rounding, not a cut. Far below the sliver threshold, so a real nick still counts.
const CONTACT_AREA_FRACTION: f64 = 1e-9;

/// Geometric slack, relative to `1 + width`, for "strictly inside" and "on the
/// plane" tests of a vertex.
const VERTEX_EPS: f64 = 1e-9;

/// Total polygon area of each facet of `mesh`, keyed by facet id. A concave mesh
/// may carry several rings for one facet, which add.
fn facet_areas(mesh: &SolidMesh) -> BTreeMap<usize, f64> {
    let mut areas = BTreeMap::new();
    for (id, ring) in &mesh.rings {
        *areas.entry(*id).or_insert(0.0) += polygon_area(ring);
    }
    areas
}

/// Unit axis of `tool` as an `f64` vector.
fn tool_axis_f64(tool: &ToolPrimitive) -> DVec3 {
    DVec3::new(
        f64::from(tool.axis[0]),
        f64::from(tool.axis[1]),
        f64::from(tool.axis[2]),
    )
}

/// The lowest-numbered touching facet that has a touching facet facing the
/// opposite way, or `None`. `stroke_axis` is the tool axis for a reciprocating
/// tool, whose stroke-end facets are skipped ([`STROKE_END_FACET_COS`]).
fn broken_through_facet(
    planes: &[(DVec3, f64)],
    touched: &[usize],
    stroke_axis: Option<DVec3>,
) -> Option<usize> {
    let normal = |i: usize| planes[i].0.normalize_or_zero();
    let radial = |n: DVec3| stroke_axis.is_none_or(|axis| n.dot(axis).abs() < STROKE_END_FACET_COS);
    for (position, &a) in touched.iter().enumerate() {
        let na = normal(a);
        if !radial(na) {
            continue;
        }
        for &b in &touched[position + 1..] {
            let nb = normal(b);
            if radial(nb) && na.dot(nb) <= OPPOSITE_FACET_COS {
                return Some(a);
            }
        }
    }
    None
}

/// Check 6 (concave designs only): the concave-tool warnings of an already-solved design.
///
/// Empty for a design without concave tiers, and also when the tiers cannot be
/// resolved (an invalid tier is reported by validation, not here), so planar
/// manufacturability output never changes. See [`concave_tool_warnings`] for what each warning means.
#[must_use]
pub fn check_concave_tools(
    design: &Design,
    solved: &[SolvedTier],
    min_facet_area_fraction_of_w2: f64,
) -> Vec<ManufacturabilityWarning> {
    if design.concave_tiers.is_empty() {
        return Vec::new();
    }
    let Ok((planes, tools, placements)) = design.geometry_from_solved(solved) else {
        return Vec::new();
    };
    let preform_len = design.preform.planes().len();
    // The tier table asks on every refresh, and the warnings cost many mesh builds, so
    // an unchanged stone (same planes, tools and threshold) is answered from a small
    // cache. It is shared by every thread: a worker that computed the warnings for a
    // frame leaves them for the next caller of the same stone, whichever thread that is.
    // The key covers every input the warnings read, and a hit compares all of them, not just
    // their hash.
    let key = ConcaveWarningsKey::new(
        &planes,
        preform_len,
        &tools,
        &placements,
        min_facet_area_fraction_of_w2,
    );
    let hit = cached_concave_warnings(
        &CONCAVE_WARNINGS_CACHE
            .lock()
            .unwrap_or_else(PoisonError::into_inner),
        &key,
    );
    if let Some(hit) = hit {
        return hit;
    }
    // Computed outside the lock: two threads asking for the same new stone may both
    // compute it, which only costs time -- the answer depends on the inputs alone.
    let warnings = concave_tool_warnings(
        &planes,
        preform_len,
        &tools,
        &placements,
        min_facet_area_fraction_of_w2,
    );
    remember_concave_warnings(key, &warnings);
    warnings
}

/// Stores `warnings` under `key` in [`CONCAVE_WARNINGS_CACHE`], dropping the oldest entry
/// when it is full.
fn remember_concave_warnings(key: ConcaveWarningsKey, warnings: &[ManufacturabilityWarning]) {
    let mut cache = CONCAVE_WARNINGS_CACHE
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if cached_concave_warnings(&cache, &key).is_some() {
        return;
    }
    if cache.len() >= CONCAVE_WARNINGS_CACHE_LEN {
        cache.remove(0);
    }
    cache.push((key, warnings.to_vec()));
}

/// The warnings remembered for exactly `key`, if any. Two stones whose hashes collide have
/// different inputs, so they do not answer each other.
fn cached_concave_warnings(
    cache: &[(ConcaveWarningsKey, Vec<ManufacturabilityWarning>)],
    key: &ConcaveWarningsKey,
) -> Option<Vec<ManufacturabilityWarning>> {
    cache
        .iter()
        .find(|(cached, _)| cached == key)
        .map(|(_, warnings)| warnings.clone())
}

/// How many stones' concave warnings [`check_concave_tools`] remembers (the design being
/// edited plus a few it was just switched from).
const CONCAVE_WARNINGS_CACHE_LEN: usize = 4;

/// Oldest first; see [`check_concave_tools`].
static CONCAVE_WARNINGS_CACHE: Mutex<Vec<(ConcaveWarningsKey, Vec<ManufacturabilityWarning>)>> =
    Mutex::new(Vec::new());

/// Everything [`concave_tool_warnings`] reads, as the exact bits of the planes and of every
/// tool, the placements, the preform's plane count and the area threshold, plus a 64-bit hash
/// of them.
///
/// The hash is the quick test and the inputs the final one: two keys are equal only when
/// every input is, so a hash collision cannot hand one stone another's warnings. The key is
/// a few thousand words for a design of some hundred planes, and a cache of four keeps that
/// small. `DefaultHasher::new()` has fixed keys, so equal inputs always give equal hashes.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ConcaveWarningsKey {
    hash: u64,
    inputs: Vec<u64>,
}

impl ConcaveWarningsKey {
    fn new(
        planes: &[(DVec3, f64)],
        preform_len: usize,
        tools: &[ToolPrimitive],
        placements: &[(usize, usize)],
        min_facet_area_fraction_of_w2: f64,
    ) -> Self {
        let mut inputs = Vec::with_capacity(4 * planes.len() + 18 * tools.len() + 8);
        // The counts come first, so inputs of different shapes cannot run into each other.
        inputs.extend([planes.len() as u64, preform_len as u64, tools.len() as u64]);
        for (normal, offset) in planes {
            inputs.extend([normal.x, normal.y, normal.z, *offset].map(f64::to_bits));
        }
        for tool in tools {
            inputs.extend([u64::from(tool.kind), u64::from(tool.sweep_kind)]);
            inputs.extend(
                tool.origin
                    .iter()
                    .chain(&tool.axis)
                    .chain(&tool.profile)
                    .chain(&tool.sweep_dir)
                    .map(|value| u64::from(value.to_bits())),
            );
        }
        inputs.push(placements.len() as u64);
        for &(tier, placement) in placements {
            inputs.extend([tier as u64, placement as u64]);
        }
        inputs.push(min_facet_area_fraction_of_w2.to_bits());
        Self::from_inputs(inputs)
    }

    /// The key of `inputs`, hashed.
    fn from_inputs(inputs: Vec<u64>) -> Self {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        inputs.hash(&mut hasher);
        Self {
            hash: hasher.finish(),
            inputs,
        }
    }
}

/// The concave-tool warnings for `tools` carved from the flat stone `planes` (plan §9.1).
///
/// The order is fixed: per tool in primitive order (`ToolMissesStone`, or
/// `ToolEnclosed`, or else `ToolBreaksThrough`, `ToolRemovesMeet` per vertex,
/// `ToolRemovesHullVertex`), then every `ToolsOverlap` pair, then every `ConcaveSliver`
/// by facet id.
///
/// - **Misses**: the tool's convex polytope clipped to the stone has (almost) no
///   volume. A missing tool is reported alone, the other per-tool checks being
///   moot.
/// - **Enclosed**: the tool removes volume but touches no flat facet, so it is an
///   internal void the cutter cannot reach. Reported alone, like a miss.
/// - **Breaks through**: the removed region touches two flat facets facing
///   opposite ways. A reciprocating tool's stroke-end facets are ignored.
/// - **Removes a meet**: the tool strictly contains a vertex where at least three
///   schedule facets (not the preform's walls, `preform_len` planes first) meet.
/// - **Removes a hull vertex**: the tool strictly contains a vertex that is
///   extreme in x, y or z, so the stone shrinks.
/// - **Overlap**: two tools remove common volume (a bounding-box test first).
/// - **Sliver**: a facet at least the minimum area on the flat stone is left
///   below it, but not gone, by the tools (`min_facet_area_fraction_of_w2` times
///   the flat width squared, like check 2).
///
/// `placements[k]` is the `(tier, placement)` of `tools[k]`.
///
/// # Cost
///
/// A plane-arrangement mesh is cubic in its plane count, so each mesh the check builds
/// leaves out the planes that cannot matter ([`super::tool_cull`]): the stone planes a tool
/// lies wholly inside, and the tool planes the stone lies wholly inside. A tool wholly beyond
/// a stone plane, and a pair of tools whose boxes are separated, need no mesh at all. None
/// of this changes an answer; it only skips work.
#[must_use]
pub fn concave_tool_warnings(
    planes: &[(DVec3, f64)],
    preform_len: usize,
    tools: &[ToolPrimitive],
    placements: &[(usize, usize)],
    min_facet_area_fraction_of_w2: f64,
) -> Vec<ManufacturabilityWarning> {
    let mut warnings = Vec::new();
    let Some((metrics, vertices)) = measure_solid_with_vertices(planes) else {
        return warnings;
    };
    if tools.is_empty() {
        return warnings;
    }
    let area_threshold = min_facet_area_fraction_of_w2 * metrics.width_axis * metrics.width_axis;
    let min_removed = REMOVED_VOLUME_FRACTION * metrics.volume;
    let eps = VERTEX_EPS * (1.0 + metrics.width_axis);
    let contact_floor = CONTACT_AREA_FRACTION * metrics.width_axis * metrics.width_axis;
    let stone = Stone {
        planes,
        lo: vertices
            .iter()
            .fold(DVec3::splat(f64::INFINITY), |a, &v| a.min(v)),
        hi: vertices
            .iter()
            .fold(DVec3::splat(f64::NEG_INFINITY), |a, &v| a.max(v)),
        slack: eps,
    };
    let (lo, hi) = (stone.lo, stone.hi);

    let polytopes: Vec<Vec<Plane>> = tools
        .iter()
        .map(|tool| tessellate_tool(tool, TOOL_SEGMENTS))
        .collect();
    for (k, polytope) in polytopes.iter().enumerate() {
        let (tier, placement) = placements[k];
        // An invalid tool has no polytope and no box; a tool wholly beyond a stone
        // plane has no volume in the stone, so neither needs a mesh.
        let removed = tool_bounds(&tools[k], TOOL_SEGMENTS)
            .filter(|bounds| {
                !polytope.is_empty() && !outside_stone(&Bound::Obb(bounds), planes, eps)
            })
            .and_then(|bounds| stone.clipped(&[(polytope.as_slice(), &bounds)]))
            .filter(|(mesh, _)| mesh_volume(mesh) > min_removed);
        let Some((removed, kept)) = removed else {
            warnings.push(ManufacturabilityWarning::ToolMissesStone { tier, placement });
            continue;
        };

        // `kept[id]` is the flat plane of mesh facet `id < kept.len()`; the rest are tool facets.
        let areas = facet_areas(&removed);
        if !areas
            .iter()
            .any(|(&id, &area)| id < kept.len() && area > contact_floor)
        {
            warnings.push(ManufacturabilityWarning::ToolEnclosed { tier, placement });
            continue;
        }
        let touched: Vec<usize> = areas
            .into_iter()
            .filter(|&(id, area)| id < kept.len() && area >= area_threshold)
            .map(|(id, _)| kept[id])
            .collect();
        let stroke_axis = (tools[k].sweep_kind != 0).then(|| tool_axis_f64(&tools[k]));
        if let Some(facet) = broken_through_facet(planes, &touched, stroke_axis) {
            warnings.push(ManufacturabilityWarning::ToolBreaksThrough {
                tier,
                placement,
                facet,
            });
        }

        let mut removes_hull_vertex = false;
        for (vertex, &v) in vertices.iter().enumerate() {
            if !polytope.iter().all(|&(n, m)| n.dot(v) - m < -eps) {
                continue;
            }
            let meeting = planes[preform_len.min(planes.len())..]
                .iter()
                .filter(|&&(n, m)| (n.dot(v) - m).abs() <= eps)
                .count();
            if meeting >= 3 {
                warnings.push(ManufacturabilityWarning::ToolRemovesMeet {
                    tier,
                    placement,
                    vertex,
                });
            }
            removes_hull_vertex |= (0..3)
                .any(|axis| (v[axis] - lo[axis]).abs() <= eps || (v[axis] - hi[axis]).abs() <= eps);
        }
        if removes_hull_vertex {
            warnings.push(ManufacturabilityWarning::ToolRemovesHullVertex { tier, placement });
        }
    }

    warnings.extend(overlap_warnings(&stone, tools, min_removed));
    warnings.extend(sliver_warnings(planes, tools, area_threshold));
    warnings
}

/// The flat stone as the concave checks see it: its planes, the axis-aligned box around its
/// vertices, and the slack the bounding tests allow for rounding.
struct Stone<'a> {
    planes: &'a [Plane],
    lo: DVec3,
    hi: DVec3,
    slack: f64,
}

impl Stone<'_> {
    /// The mesh of the stone cut down to where every one of `tools` is (each given as its
    /// polytope's planes and the box around it), if that is a closed solid, and the position
    /// in `self.planes` of each stone plane the mesh was built with: mesh facet `id` below
    /// that list's length is the stone plane `kept[id]`, any later id is a tool facet.
    ///
    /// Leaves out the planes that cannot shape the result ([`needed_planes`]): a stone plane
    /// some tool lies strictly inside, and a tool plane the stone's box, or another tool's
    /// box, lies strictly inside.
    fn clipped(&self, tools: &[(&[Plane], &ToolBounds)]) -> Option<(SolidMesh, Vec<usize>)> {
        let boxes: Vec<Bound<'_>> = tools
            .iter()
            .map(|&(_, bounds)| Bound::Obb(bounds))
            .collect();
        let kept = needed_planes(self.planes, &boxes, self.slack);
        let mut all: Vec<Plane> = kept.iter().map(|&index| self.planes[index]).collect();
        let stone_box = Bound::Aabb {
            lo: self.lo,
            hi: self.hi,
        };
        for (position, &(polytope, _)) in tools.iter().enumerate() {
            let mut others = vec![stone_box];
            others.extend(
                boxes
                    .iter()
                    .enumerate()
                    .filter(|&(other, _)| other != position)
                    .map(|(_, &bound)| bound),
            );
            all.extend(
                needed_planes(polytope, &others, self.slack)
                    .into_iter()
                    .map(|index| polytope[index]),
            );
        }
        match build_solid_mesh(&all) {
            SolidStatus::Closed(mesh) => Some((mesh, kept)),
            _ => None,
        }
    }
}

/// `ToolsOverlap` for every pair of tools that remove common volume from the stone.
///
/// The tools are the coarse ([`OVERLAP_SEGMENTS`]) polytopes. A pair whose boxes are
/// separated, or a tool wholly outside the stone, cannot overlap inside it, so only nearby
/// pairs pay for a mesh.
fn overlap_warnings(
    stone: &Stone<'_>,
    tools: &[ToolPrimitive],
    min_removed: f64,
) -> Vec<ManufacturabilityWarning> {
    let mut warnings = Vec::new();
    let coarse: Vec<Option<(Vec<Plane>, ToolBounds)>> = tools
        .iter()
        .map(|tool| {
            let bounds = tool_bounds(tool, OVERLAP_SEGMENTS)?;
            let polytope = tessellate_tool(tool, OVERLAP_SEGMENTS);
            (!polytope.is_empty()
                && !outside_stone(&Bound::Obb(&bounds), stone.planes, stone.slack))
            .then_some((polytope, bounds))
        })
        .collect();
    for (a, first) in coarse.iter().enumerate() {
        let Some((polytope_a, box_a)) = first else {
            continue;
        };
        for (offset, second) in coarse[a + 1..].iter().enumerate() {
            let Some((polytope_b, box_b)) = second else {
                continue;
            };
            if boxes_separated(box_a, box_b, stone.slack) {
                continue;
            }
            let overlap = stone
                .clipped(&[
                    (polytope_a.as_slice(), box_a),
                    (polytope_b.as_slice(), box_b),
                ])
                .is_some_and(|(mesh, _)| mesh_volume(&mesh) > min_removed);
            if overlap {
                warnings.push(ManufacturabilityWarning::ToolsOverlap {
                    a,
                    b: a + 1 + offset,
                });
            }
        }
    }

    warnings
}

/// `ConcaveSliver` for every facet the tools leave smaller than `area_threshold`.
fn sliver_warnings(
    planes: &[(DVec3, f64)],
    tools: &[ToolPrimitive],
    area_threshold: f64,
) -> Vec<ManufacturabilityWarning> {
    let mut warnings = Vec::new();
    if let (SolidStatus::Closed(flat), SolidStatus::Closed(carved)) = (
        build_solid_mesh(planes),
        build_solid_mesh_geom(planes, tools),
    ) {
        let flat_areas = facet_areas(&flat);
        for (id, area) in facet_areas(&carved) {
            let was_big_enough =
                id >= planes.len() || flat_areas.get(&id).is_some_and(|&a| a >= area_threshold);
            if was_big_enough && area > 0.0 && area < area_threshold {
                warnings.push(ManufacturabilityWarning::ConcaveSliver { facet: id });
            }
        }
    }
    warnings
}

/// Tier indices whose solved mast came from neither a real authored anchor
/// nor real vertex-derived structure.
///
/// Indices are into `solved`, which lines up 1:1 with `Design::tiers` -- see
/// [`crate::design::Design::planes_from_solved`]'s alignment contract.
/// "Neither" means neither [`SolveStrategy::ScaleReference`] nor real
/// vertex-derived structure ([`SolveStrategy::DependencyOrder`] or
/// [`SolveStrategy::JointGroup`]) -- i.e. [`SolveStrategy::LeastSquaresFallback`]
/// or [`SolveStrategy::Failed`].
///
/// These are the suspects a [`SolidStatus::Degenerate`] verdict's caller
/// should point the user at: every other tier's mast came from a real
/// authored dimension or from where the design's own facets actually meet,
/// so a tier that could not be placed from any of that real structure is the
/// more likely cause of a degenerate arrangement.
#[must_use]
pub fn degenerate_suspects(solved: &[SolvedTier]) -> Vec<usize> {
    solved
        .iter()
        .enumerate()
        .filter(|(_, tier)| {
            matches!(
                tier.strategy,
                SolveStrategy::LeastSquaresFallback | SolveStrategy::Failed
            )
        })
        .map(|(index, _)| index)
        .collect()
}

/// Area of a planar polygon given as an ordered ring of 3D vertices: the standard
/// vector-area shoelace formula, generalized off the ring's own centroid so it
/// needs no assumption about which 2D plane the polygon lies in (`0.5 * |sum of
/// (v_i - c) x (v_{i+1} - c)|`, exact for a planar, non-self-intersecting polygon
/// regardless of its normal's orientation).
fn polygon_area(ring: &[glam::DVec3]) -> f64 {
    if ring.len() < 3 {
        return 0.0;
    }
    let centroid = ring.iter().copied().sum::<glam::DVec3>() / ring.len() as f64;
    let mut cross_sum = glam::DVec3::ZERO;
    for i in 0..ring.len() {
        let a = ring[i] - centroid;
        let b = ring[(i + 1) % ring.len()] - centroid;
        cross_sum += a.cross(b);
    }
    0.5 * cross_sum.length()
}
