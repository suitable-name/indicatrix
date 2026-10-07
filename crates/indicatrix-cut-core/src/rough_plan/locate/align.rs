//! Mesh to rig alignment: one rigid transform for all the views, from the stone's outline in
//! each photo.
//!
//! The scan's frame is not the rig's. The user gives a coarse start (see
//! [`Rigid::from_axes`]) and an outline per photo (a thresholded or hand-drawn closed polygon, in
//! pixels). The transform is refined by Levenberg-Marquardt on two kinds of residual per view,
//! both in pixels and both independent of refraction (a silhouette does not depend on it):
//!
//! - every sampled mesh vertex that projects OUTSIDE the outline contributes its distance to it;
//! - every point sampled along the outline contributes its signed distance to the outline of the
//!   projected mesh, positive outside.
//!
//! The projected outline is the convex hull of the projected vertices: exact for a convex rough
//! and an outer bound for a concave one, whose bays then show up as a misfit. The remaining misfit
//! per view is reported, and the alignment is refused (not accepted) above a threshold.

use glam::{DVec2, DVec3};

use super::{
    rig::{RigProfile, Rigid, ViewPose},
    solve::levenberg_marquardt,
};
use crate::rough_plan::shape::RoughMesh;

/// The residual of a vertex that cannot be projected (behind the camera) or of a degenerate
/// projection, in pixels.
const PENALTY_PX: f64 = 1.0e3;

/// The stone's outline in one photo.
#[derive(Debug, Clone, PartialEq)]
pub struct OutlineView {
    /// The view (index into the rig's views).
    pub view: usize,
    /// The closed outline as a polygon, in pixels (the last point joins the first).
    pub outline: Vec<[f64; 2]>,
}

/// Tuning of the alignment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AlignOptions {
    /// The largest per-view RMS misfit, in pixels, that is accepted (default 4).
    pub max_misfit_px: f64,
    /// The most Levenberg-Marquardt iterations (default 80).
    pub max_iterations: usize,
    /// The most mesh vertices sampled for the silhouette (default 600).
    pub max_vertices: usize,
    /// About how many points are sampled along each outline (default 64).
    pub outline_samples: usize,
}

impl Default for AlignOptions {
    fn default() -> Self {
        Self {
            max_misfit_px: 4.0,
            max_iterations: 80,
            max_vertices: 600,
            outline_samples: 64,
        }
    }
}

/// The remaining misfit of one view.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewMisfit {
    /// The view.
    pub view: usize,
    /// The RMS distance, in pixels, between the outline and the projected mesh outline.
    pub rms_px: f64,
    /// The largest distance, in pixels, of an outline point or a vertex.
    pub max_px: f64,
}

/// The result of [`align_mesh_to_rig`].
#[derive(Debug, Clone, PartialEq)]
pub struct AlignResult {
    /// The fitted transform from the mesh frame to the rig frame.
    pub transform: Rigid,
    /// The misfit of every view, in the order the outlines were given.
    pub misfit: Vec<ViewMisfit>,
    /// The largest per-view RMS misfit, in pixels.
    pub worst_rms_px: f64,
    /// Whether the worst misfit is within the limit; when `false` the alignment must not be used.
    pub accepted: bool,
    /// What to tell the user when the alignment is not accepted.
    pub note: Option<String>,
    /// How many iterations were run.
    pub iterations: usize,
}

/// Why an alignment could not be attempted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlignError {
    /// No outlines were given.
    NoOutlines,
    /// The outline refers to a view that does not exist.
    NoSuchView(usize),
    /// This outline (0-based) has fewer than three points or a point that is not finite.
    BadOutline(usize),
}

impl std::fmt::Display for AlignError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::NoOutlines => write!(f, "no outline was given"),
            Self::NoSuchView(view) => write!(f, "view {} does not exist", view + 1),
            Self::BadOutline(i) => write!(f, "outline {} is not a usable polygon", i + 1),
        }
    }
}

impl std::error::Error for AlignError {}

/// One view prepared for the fit.
struct Prepared<'a> {
    view: usize,
    pose: &'a ViewPose,
    polygon: Vec<DVec2>,
    samples: Vec<DVec2>,
}

/// The distance from `point` to the segment `from`..`to`.
fn segment_distance(from: DVec2, to: DVec2, point: DVec2) -> f64 {
    let edge = to - from;
    let len2 = edge.length_squared();
    let t = if len2 > 0.0 {
        ((point - from).dot(edge) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (point - (from + edge * t)).length()
}

/// Whether `point` is inside the polygon (even-odd rule).
fn inside_polygon(polygon: &[DVec2], point: DVec2) -> bool {
    let Some(&last) = polygon.last() else {
        return false;
    };
    let mut inside = false;
    let mut previous = last;
    for &current in polygon {
        if (current.y > point.y) != (previous.y > point.y) {
            let slope = (previous.x - current.x) * (point.y - current.y) / (previous.y - current.y);
            if point.x < slope + current.x {
                inside = !inside;
            }
        }
        previous = current;
    }
    inside
}

/// The distance from `point` to the boundary of the closed polygon.
fn boundary_distance(polygon: &[DVec2], point: DVec2) -> f64 {
    let Some(&last) = polygon.last() else {
        return PENALTY_PX;
    };
    let mut previous = last;
    let mut best = f64::INFINITY;
    for &current in polygon {
        best = best.min(segment_distance(previous, current, point));
        previous = current;
    }
    best
}

/// The convex hull of the points, counter-clockwise in a y-up frame (clockwise on screen).
pub(super) fn convex_hull(points: &[DVec2]) -> Vec<DVec2> {
    let mut sorted = points.to_vec();
    sorted.sort_by(|left, right| left.x.total_cmp(&right.x).then(left.y.total_cmp(&right.y)));
    sorted.dedup();
    if sorted.len() < 3 {
        return sorted;
    }
    let turn =
        |origin: DVec2, first: DVec2, second: DVec2| (first - origin).perp_dot(second - origin);
    let mut hull: Vec<DVec2> = Vec::with_capacity(sorted.len() * 2);
    for &point in &sorted {
        while hull.len() >= 2 && turn(hull[hull.len() - 2], hull[hull.len() - 1], point) <= 0.0 {
            hull.pop();
        }
        hull.push(point);
    }
    let lower = hull.len() + 1;
    for &point in sorted.iter().rev().skip(1) {
        while hull.len() >= lower && turn(hull[hull.len() - 2], hull[hull.len() - 1], point) <= 0.0
        {
            hull.pop();
        }
        hull.push(point);
    }
    hull.pop();
    hull
}

/// The signed distance from `point` to the boundary of the convex polygon `hull` (as built by
/// [`convex_hull`]): positive outside, negative inside.
fn hull_signed_distance(hull: &[DVec2], point: DVec2) -> f64 {
    let Some(&last) = hull.last() else {
        return PENALTY_PX;
    };
    let mut previous = last;
    let mut best = f64::INFINITY;
    let mut inside = true;
    for &current in hull {
        best = best.min(segment_distance(previous, current, point));
        if (current - previous).perp_dot(point - previous) < 0.0 {
            inside = false;
        }
        previous = current;
    }
    if inside { -best } else { best }
}

/// Points along the closed outline about `target` in number, including every outline vertex.
fn resample_outline(outline: &[DVec2], target: usize) -> Vec<DVec2> {
    let count = outline.len();
    let perimeter: f64 = (0..count)
        .map(|i| (outline[(i + 1) % count] - outline[i]).length())
        .sum();
    let spacing = (perimeter / target.max(3) as f64).max(1e-9);
    let mut samples = Vec::new();
    for i in 0..count {
        let (from, to) = (outline[i], outline[(i + 1) % count]);
        let pieces = ((to - from).length() / spacing).ceil().max(1.0) as usize;
        for piece in 0..pieces {
            samples.push(from + (to - from) * (piece as f64 / pieces as f64));
        }
    }
    samples
}

/// The mesh vertices the silhouette is fitted to: an even stride through them plus the most
/// extreme vertex along thirteen fixed directions, sorted and without duplicates.
fn sample_vertices(mesh: &RoughMesh, max_vertices: usize) -> Vec<DVec3> {
    let verts = mesh.vertices();
    let stride = verts.len().div_ceil(max_vertices.max(1)).max(1);
    let mut picks: Vec<usize> = (0..verts.len()).step_by(stride).collect();
    let mut directions = vec![DVec3::X, DVec3::Y, DVec3::Z];
    for sign_y in [-1.0, 1.0] {
        directions.push(DVec3::new(1.0, sign_y, 0.0));
        directions.push(DVec3::new(1.0, 0.0, sign_y));
        directions.push(DVec3::new(0.0, 1.0, sign_y));
        for sign_z in [-1.0, 1.0] {
            directions.push(DVec3::new(1.0, sign_y, sign_z));
        }
    }
    for direction in directions {
        let by =
            |left: &&DVec3, right: &&DVec3| left.dot(direction).total_cmp(&right.dot(direction));
        let high = verts.iter().enumerate().max_by(|a, b| by(&a.1, &b.1));
        let low = verts.iter().enumerate().min_by(|a, b| by(&a.1, &b.1));
        picks.extend(high.map(|pair| pair.0));
        picks.extend(low.map(|pair| pair.0));
    }
    picks.sort_unstable();
    picks.dedup();
    picks.into_iter().map(|i| verts[i]).collect()
}

/// Appends the residuals of one view: one per mesh vertex (outside distance), then one per
/// outline sample (signed distance to the projected hull).
fn append_residuals(view: &Prepared<'_>, rig_points: &[DVec3], out: &mut Vec<f64>) {
    let projected: Vec<Option<DVec2>> = rig_points.iter().map(|&p| view.pose.project(p)).collect();
    for pixel in &projected {
        out.push(pixel.map_or(PENALTY_PX, |p| {
            if inside_polygon(&view.polygon, p) {
                0.0
            } else {
                boundary_distance(&view.polygon, p)
            }
        }));
    }
    let seen: Vec<DVec2> = projected.iter().flatten().copied().collect();
    let hull = convex_hull(&seen);
    if hull.len() < 3 {
        out.extend(std::iter::repeat_n(PENALTY_PX, view.samples.len()));
    } else {
        out.extend(view.samples.iter().map(|&s| hull_signed_distance(&hull, s)));
    }
}

/// The residuals of every view for the parameters `[rotation x3, translation x3]`.
fn residuals(params: &[f64], points: &[DVec3], views: &[Prepared<'_>]) -> Vec<f64> {
    let rigid = Rigid {
        rotation: [params[0], params[1], params[2]],
        translation: [params[3], params[4], params[5]],
    };
    let rig_points: Vec<DVec3> = points.iter().map(|&p| rigid.to_rig(p)).collect();
    let mut out = Vec::new();
    for view in views {
        append_residuals(view, &rig_points, &mut out);
    }
    out
}

fn prepare<'a>(
    rig: &'a RigProfile,
    outlines: &[OutlineView],
    samples: usize,
) -> Result<Vec<Prepared<'a>>, AlignError> {
    let mut views = Vec::with_capacity(outlines.len());
    for (index, item) in outlines.iter().enumerate() {
        let pose = rig
            .views
            .get(item.view)
            .ok_or(AlignError::NoSuchView(item.view))?;
        let polygon: Vec<DVec2> = item.outline.iter().map(|&p| DVec2::from_array(p)).collect();
        if polygon.len() < 3 || polygon.iter().any(|p| !p.is_finite()) {
            return Err(AlignError::BadOutline(index));
        }
        views.push(Prepared {
            view: item.view,
            pose,
            samples: resample_outline(&polygon, samples),
            polygon,
        });
    }
    Ok(views)
}

/// Aligns `mesh` to the rig from the stone's outline in each photo.
///
/// `start` is the user's coarse transform (see [`Rigid::from_axes`]); the fit refines all six
/// parameters together over every view. The result carries the misfit per view in pixels and
/// `accepted == false` when the worst RMS misfit exceeds [`AlignOptions::max_misfit_px`].
///
/// # Errors
///
/// [`AlignError`] when there are no outlines, an outline names a view the rig lacks, or an
/// outline is not a polygon.
pub fn align_mesh_to_rig(
    mesh: &RoughMesh,
    rig: &RigProfile,
    outlines: &[OutlineView],
    start: Rigid,
    options: &AlignOptions,
) -> Result<AlignResult, AlignError> {
    if outlines.is_empty() {
        return Err(AlignError::NoOutlines);
    }
    let views = prepare(rig, outlines, options.outline_samples)?;
    let points = sample_vertices(mesh, options.max_vertices);
    let begin = [
        start.rotation[0],
        start.rotation[1],
        start.rotation[2],
        start.translation[0],
        start.translation[1],
        start.translation[2],
    ];
    let steps = [1e-5, 1e-5, 1e-5, 1e-3, 1e-3, 1e-3];
    let fit = levenberg_marquardt(&begin, &steps, options.max_iterations, &|params| {
        residuals(params, &points, &views)
    });
    let transform = Rigid {
        rotation: [fit.params[0], fit.params[1], fit.params[2]],
        translation: [fit.params[3], fit.params[4], fit.params[5]],
    };
    let all = residuals(&fit.params, &points, &views);
    let mut misfit = Vec::with_capacity(views.len());
    let mut offset = 0;
    for view in &views {
        let vertex_part = &all[offset..offset + points.len()];
        let outline_part = &all[offset + points.len()..offset + points.len() + view.samples.len()];
        offset += points.len() + view.samples.len();
        let rms_px =
            (outline_part.iter().map(|r| r * r).sum::<f64>() / outline_part.len() as f64).sqrt();
        let max_px = outline_part
            .iter()
            .chain(vertex_part)
            .fold(0.0, |worst, r| f64::max(worst, r.abs()));
        misfit.push(ViewMisfit {
            view: view.view,
            rms_px,
            max_px,
        });
    }
    let worst = misfit
        .iter()
        .max_by(|left, right| left.rms_px.total_cmp(&right.rms_px))
        .copied();
    let worst_rms_px = worst.map_or(0.0, |entry| entry.rms_px);
    let accepted = worst_rms_px <= options.max_misfit_px;
    let note = (!accepted).then(|| {
        let view = worst.map_or(0, |entry| entry.view) + 1;
        format!(
            "the stone's outline misses the mesh by {worst_rms_px:.1} px in view {view} (limit {:.1} px): \
             check the start orientation, the outlines, and that the scan has the stone's true size",
            options.max_misfit_px
        )
    });
    Ok(AlignResult {
        transform,
        misfit,
        worst_rms_px,
        accepted,
        note,
        iterations: fit.iterations,
    })
}
