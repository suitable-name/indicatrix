//! Canonical incidence topology shared by capture and candidate validation.
//!
//! Junctions come from the f64 plane arrangement itself, clustered at the
//! preservation tolerance, never from the welded display mesh: the mesh welds
//! vertices at `VERTEX_DEDUP` in units normalised by the *preform's* largest offset,
//! so for a rough much larger than the stone it can merge the triple points of a
//! four-plane meet that the (stone-sized) tolerance still tells apart, and a plane
//! of that meet then silently drops out of the junction's support. Faces are
//! re-derived from the junction list for the same reason.
//!
//! Two radii are in play. Meets are *identified* at `identify` (`5e-5` of the stone,
//! `network.rs`): plane-triple points within it are one junction, and a plane within it
//! of the junction is in its support, with the largest such miss kept as the junction's
//! `slack`. The *reproduction* bar stays the preservation tolerance (`2e-6` of the
//! stone): a candidate must meet every original junction to the larger of the
//! tolerance and that slack. There is no band of refused near-misses between the two:
//! a plane beyond `identify` is no member of the meet and is protected by its own
//! junction, and the input's own quantisation (5-decimal `GemCad` 4.x masts) and the
//! `f32` production planes were measured at no more than `2e-5` of the stone.
#[cfg(test)]
use super::vertices;
use super::{Junction, Plane, PreservationViolation, polygon_area};
use glam::DVec3;
#[cfg(test)]
use indicatrix::geometry::stone_metrics::SolidMesh;
use indicatrix::geometry::{meet_solver::SolveControl, stone_metrics::feasible_plane_triples};

/// Cancellation is polled once per this many arrangement triples.
const CANCEL_STRIDE: usize = 256;

/// Whose topology is being resolved; decides who an error blames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Role {
    /// The operation's original design.
    Baseline,
    /// A proposed design measured against the captured network.
    Candidate,
}

fn fail(role: Role, reason: &str) -> PreservationViolation {
    match role {
        Role::Baseline => PreservationViolation::InvalidBaseline(reason.into()),
        Role::Candidate => PreservationViolation::CandidateTopology(reason.into()),
    }
}

/// The junctions of the arrangement `surfaces`.
///
/// Every feasible plane-triple point is clustered within `identify` (the first point
/// of a cluster is its position), with the support set `{planes of every triple in the
/// cluster} + {planes within identify of the position}`. `extent` is the stone's size
/// (see [`feasible_plane_triples`]). Each junction records its `slack`, the largest
/// distance of a support plane from its position: zero up to rounding for an exact
/// meet, the input's own quantisation for an imported one.
///
/// Junctions whose support another junction carries in full are dropped last (see
/// `drop_covered`), for both roles, so the list is the same canonical set that
/// `MeetPreservationBaseline::validate` maps back onto: no original junction has a
/// second candidate point carrying its whole support.
pub(super) fn arrangement_junctions(
    surfaces: &[Plane],
    extent: f64,
    identify: f64,
    role: Role,
    control: &SolveControl<'_>,
) -> Result<Vec<Junction>, PreservationViolation> {
    if control.is_cancelled() {
        return Err(PreservationViolation::Cancelled);
    }
    let triples = feasible_plane_triples(surfaces, extent)
        .ok_or_else(|| fail(role, "the planes do not enclose a finite stone"))?;
    let mut clusters: Vec<(DVec3, Vec<usize>)> = Vec::new();
    for (count, (triple, point)) in triples.into_iter().enumerate() {
        if count % CANCEL_STRIDE == 0 && control.is_cancelled() {
            return Err(PreservationViolation::Cancelled);
        }
        // The walk admits a wider slack than the preservation tolerance. A point
        // demonstrably outside the actual stone is not a protected meet.
        if !inside(point, surfaces, identify) {
            continue;
        }
        match clusters
            .iter()
            .position(|(position, _)| position.distance(point) <= identify)
        {
            Some(at) => clusters[at].1.extend(triple),
            None => clusters.push((point, triple.to_vec())),
        }
    }
    let mut junctions: Vec<Junction> = Vec::with_capacity(clusters.len());
    for (point, mut support) in clusters {
        support.extend(
            surfaces
                .iter()
                .enumerate()
                .filter(|(_, (normal, offset))| (normal.dot(point) - offset).abs() <= identify)
                .map(|(index, _)| index),
        );
        support.sort_unstable();
        support.dedup();
        if support.len() < 3 {
            return Err(fail(role, "unresolved boundary support"));
        }
        let basis =
            best_basis(surfaces, &support).ok_or_else(|| fail(role, "rank-deficient junction"))?;
        let slack = support
            .iter()
            .map(|&s| (surfaces[s].0.dot(point) - surfaces[s].1).abs())
            .fold(0.0, f64::max);
        junctions.push(Junction {
            point,
            support,
            basis,
            slack,
        });
    }
    Ok(drop_covered(junctions))
}

/// Whether every index of the sorted, deduplicated `small` is also in `big`.
fn is_subset(small: &[usize], big: &[usize]) -> bool {
    let mut rest = big.iter();
    small.iter().all(|wanted| rest.any(|have| have == wanted))
}

/// The junctions that no other junction covers.
///
/// A junction whose support is a subset of another's is that meet seen from an
/// ill-conditioned plane triple (two nearly parallel normals land the point along an
/// edge of the stone), or the same meet clustered a second time because the triple
/// points of a many-plane meet scatter over more than one radius. Every plane of the
/// covered junction passes through the covering junction's point to `identify`, so the
/// covering junction already states that meet in full; keeping both makes the
/// correspondence in `MeetPreservationBaseline::validate` ambiguous (an original
/// junction would find two candidate points carrying its whole support),
/// and an unchanged design would be refused as detached. Equal supports keep the
/// first junction in the arrangement's deterministic order.
///
/// Capture and candidate validation both call this through [`arrangement_junctions`],
/// so they cannot disagree about it. One consequence: a candidate in which an original
/// many-plane meet reappears as two full-support clusters more than `identify` apart
/// keeps only the first cluster, so the meet maps to it instead of being refused as
/// ambiguous; the reported residuals still measure that point against every plane.
fn drop_covered(junctions: Vec<Junction>) -> Vec<Junction> {
    let keep: Vec<bool> = junctions
        .iter()
        .enumerate()
        .map(|(at, junction)| {
            !junctions.iter().enumerate().any(|(other_at, other)| {
                other_at != at
                    && is_subset(&junction.support, &other.support)
                    && (junction.support.len() < other.support.len() || other_at < at)
            })
        })
        .collect();
    junctions
        .into_iter()
        .zip(keep)
        .filter_map(|(junction, keep)| keep.then_some(junction))
        .collect()
}

/// The best-conditioned plane triple of a support set.
pub(super) fn best_basis(surfaces: &[Plane], support: &[usize]) -> Option<[usize; 3]> {
    let mut basis = None;
    let mut best = 1e-8;
    for (i, &a) in support.iter().enumerate() {
        for (k, &b) in support.iter().enumerate().skip(i + 1) {
            for &c in support.iter().skip(k + 1) {
                let det = surfaces[a].0.dot(surfaces[b].0.cross(surfaces[c].0)).abs();
                if det > best {
                    best = det;
                    basis = Some([a, b, c]);
                }
            }
        }
    }
    basis
}

/// A junction's point as its basis triple places it in `surfaces`, and its incidence rows.
///
/// A row is `(n . p - d) / scale` for every support plane outside the basis, in support order. The
/// three basis equations are identities, not independent constraints; keeping their
/// floating-point noise would create spurious derivative directions. `None` when the
/// basis is singular or the point not finite.
pub(super) fn support_rows(
    surfaces: &[Plane],
    junction: &Junction,
    scale: f64,
) -> Option<(DVec3, Vec<f64>)> {
    let [a, b, c] = junction.basis;
    let det = surfaces[a].0.dot(surfaces[b].0.cross(surfaces[c].0));
    if det.abs() <= 1e-8 {
        return None;
    }
    let p = (surfaces[b].0.cross(surfaces[c].0) * surfaces[a].1
        + surfaces[c].0.cross(surfaces[a].0) * surfaces[b].1
        + surfaces[a].0.cross(surfaces[b].0) * surfaces[c].1)
        / det;
    if !p.is_finite() {
        return None;
    }
    let rows = junction
        .support
        .iter()
        .filter(|s| !junction.basis.contains(s))
        .map(|&s| (surfaces[s].0.dot(p) - surfaces[s].1) / scale)
        .collect();
    Some((p, rows))
}

/// Whether the points span an area, i.e. are not all on one line (to `tolerance`).
fn non_collinear(points: &[DVec3], tolerance: f64) -> bool {
    let Some(&first) = points.first() else {
        return false;
    };
    let far = points.iter().copied().fold(first, |best, point| {
        if point.distance(first) > best.distance(first) {
            point
        } else {
            best
        }
    });
    let axis = far - first;
    let length = axis.length();
    length > tolerance
        && points
            .iter()
            .any(|point| axis.cross(*point - first).length() / length > tolerance)
}

/// Each live surface's oriented boundary cycle, as junction indices, at the
/// identification radius. A face is the set of junctions whose support contains its
/// surface, ordered by angle about their centroid in the face plane.
pub(super) fn face_cycles(
    surfaces: &[Plane],
    junctions: &[Junction],
    scale: f64,
    identify: f64,
    role: Role,
) -> Result<Vec<(usize, Vec<usize>)>, PreservationViolation> {
    let mut result = Vec::new();
    for (surface, (normal, _)) in surfaces.iter().enumerate() {
        let members: Vec<usize> = (0..junctions.len())
            .filter(|&index| junctions[index].support.contains(&surface))
            .collect();
        if members.len() < 3 {
            continue;
        }
        let cycle = ordered_by_angle(*normal, &members, junctions);
        let points: Vec<DVec3> = cycle.iter().map(|&index| junctions[index].point).collect();
        // A line contact has no positive-area boundary at our accuracy: it was never a
        // live facet. A formerly live candidate face is still caught by the live-face gate.
        if !non_collinear(&points, identify) {
            continue;
        }
        if polygon_area(&points) <= scale * scale * 1e-12 {
            return Err(fail(role, "degenerate facet"));
        }
        result.push((surface, cycle));
    }
    if result.len() < 4 {
        return Err(fail(role, "insufficient supported boundary faces"));
    }
    Ok(result)
}

/// `members` sorted by angle about their centroid, counter-clockwise about `normal`,
/// in a deterministic in-plane basis (the world axis least aligned with the normal).
fn ordered_by_angle(normal: DVec3, members: &[usize], junctions: &[Junction]) -> Vec<usize> {
    let seed = if normal.x.abs() <= normal.y.abs() && normal.x.abs() <= normal.z.abs() {
        DVec3::X
    } else if normal.y.abs() <= normal.z.abs() {
        DVec3::Y
    } else {
        DVec3::Z
    };
    let basis_u = (seed - normal * normal.dot(seed)).normalize();
    let basis_v = normal.cross(basis_u);
    let centroid = members
        .iter()
        .map(|&index| junctions[index].point)
        .sum::<DVec3>()
        / members.len() as f64;
    let mut keyed: Vec<(f64, usize)> = members
        .iter()
        .map(|&index| {
            let offset = junctions[index].point - centroid;
            (basis_v.dot(offset).atan2(basis_u.dot(offset)), index)
        })
        .collect();
    keyed.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    keyed.into_iter().map(|(_, index)| index).collect()
}

fn inside(point: DVec3, surfaces: &[Plane], tolerance: f64) -> bool {
    point.is_finite() && surfaces.iter().all(|(n, d)| n.dot(point) <= d + tolerance)
}

pub(super) fn supports_face(junctions: &[Junction], cycle: &[usize], surface: usize) -> bool {
    cycle
        .iter()
        .all(|&j| junctions[j].support.contains(&surface))
}

// ---------------------------------------------------------------------------
// Mesh-sample reference implementation. It has no production caller since the
// junctions moved to the arrangement (see the module docs); its rules for repeated,
// infeasible and nearby samples stay pinned by `network_topology_tests.rs`.
// ---------------------------------------------------------------------------

#[cfg(test)]
fn junctions(
    mesh: &SolidMesh,
    surfaces: &[Plane],
    tolerance: f64,
    control: &SolveControl<'_>,
) -> Result<Vec<Junction>, PreservationViolation> {
    let mut result: Vec<Junction> = Vec::new();
    for point in vertices(mesh, 0.0) {
        if control.is_cancelled() {
            return Err(PreservationViolation::Cancelled);
        }
        // The measurement mesh permits a wider feasibility slack. A point
        // demonstrably outside the actual stone is not a protected meet.
        if !inside(point, surfaces, tolerance) {
            continue;
        }
        let support: Vec<_> = surfaces
            .iter()
            .enumerate()
            .filter(|(_, (n, d))| (n.dot(point) - d).abs() <= tolerance)
            .map(|(i, _)| i)
            .collect();
        if support.len() < 3 {
            return Err(invalid("unresolved boundary support"));
        }
        let nearby: Vec<_> = result
            .iter()
            .filter(|j| j.point.distance(point) <= tolerance)
            .collect();
        if !nearby.is_empty() {
            if nearby.len() != 1 || nearby[0].support != support {
                return Err(invalid("ambiguous nearby junctions"));
            }
            continue;
        }
        let basis =
            best_basis(surfaces, &support).ok_or_else(|| invalid("rank-deficient junction"))?;
        let slack = support
            .iter()
            .map(|&s| (surfaces[s].0.dot(point) - surfaces[s].1).abs())
            .fold(0.0, f64::max);
        result.push(Junction {
            point,
            support,
            basis,
            slack,
        });
    }
    Ok(result)
}

/// Retain each actual face's oriented boundary from mesh rings at preservation
/// accuracy. Resolve its owner from the production plane index, not a looser ring fit.
#[cfg(test)]
fn cycles(
    mesh: &SolidMesh,
    planes: &[Plane],
    surfaces: &[Plane],
    junctions: &[Junction],
    scale: f64,
    tolerance: f64,
) -> Result<Vec<(usize, Vec<usize>)>, PreservationViolation> {
    let mut result = Vec::new();
    for (plane, ring) in &mesh.rings {
        let owner = planes
            .get(*plane)
            .ok_or_else(|| invalid("unmapped boundary plane"))?;
        let surface = surfaces
            .iter()
            .position(|p| p == owner)
            .ok_or_else(|| invalid("unmapped boundary surface"))?;
        let mut cycle = Vec::new();
        let mut supported_samples = Vec::new();
        for point in ring {
            if !inside(*point, surfaces, tolerance) {
                continue;
            }
            let index = junctions
                .iter()
                .position(|j| j.point.distance(*point) <= tolerance)
                .ok_or_else(|| invalid("unmapped cycle vertex"))?;
            // EPS_FACE admits points merely near this face. They belong to other
            // faces; their junctions remain protected in the global network.
            if junctions[index].support.contains(&surface) {
                if !supported_samples.contains(point) {
                    supported_samples.push(*point);
                }
                if cycle.last() != Some(&index) {
                    cycle.push(index);
                }
            }
        }
        // EPS_FACE can draw a ring on an inactive/tangent plane. It has no
        // positive-area boundary at our accuracy; it was never a live facet.
        // A formerly live candidate face is still caught by the live-face gate.
        if supported_samples.len() < 3 {
            continue;
        }
        if cycle.len() > 1 && cycle.first() == cycle.last() {
            cycle.pop();
        }
        if cycle.len() < 3
            || cycle
                .iter()
                .enumerate()
                .any(|(i, j)| cycle[..i].contains(j))
        {
            return Err(invalid("degenerate or repeated boundary cycle"));
        }
        let points: Vec<DVec3> = cycle.iter().map(|&i| junctions[i].point).collect();
        if polygon_area(&points) <= scale * scale * 1e-12 {
            return Err(invalid("degenerate facet"));
        }
        result.push((surface, cycle));
    }
    if result.len() < 4 {
        return Err(invalid("insufficient supported boundary faces"));
    }
    Ok(result)
}

#[cfg(test)]
fn invalid(reason: &str) -> PreservationViolation {
    PreservationViolation::InvalidBaseline(reason.into())
}

#[cfg(test)]
#[path = "network_topology_tests.rs"]
mod tests;
