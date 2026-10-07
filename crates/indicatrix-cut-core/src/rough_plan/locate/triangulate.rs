//! Refractive triangulation: the point closest to the refracted rays of several photos.
//!
//! Each mark becomes a camera ray, which is refracted at the surface and followed inside the
//! stone to its first exit (see [`trace_pixel`]). The inclusion is the point that is closest, in
//! the least-squares sense, to all those interior rays: with `d` the unit direction and `p` a
//! point of each, it solves the closed-form system `sum (I - d d^T) X = sum (I - d d^T) p`.
//!
//! Afterwards the point is kept inside the mesh, the RMS distance of the rays from it is the
//! uncertainty, and a leave-one-out pass names the views that disagree with all the others (a
//! ghost image or a wrong click).

use std::fmt;

use glam::{DMat3, DVec2, DVec3};
use serde::{Deserialize, Serialize};

use super::trace::{Leg, Scene, TraceError, trace_pixel};

/// A mark the user placed on a photo.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Mark {
    /// The view (index into the rig's views) the photo belongs to.
    pub view: usize,
    /// The pixel that was clicked, `u` then `v`.
    pub pixel: [f64; 2],
}

/// The marks of one polyline or polygon in one view. Vertex `k` of every view's polyline is the
/// same physical vertex.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ViewPolyline {
    /// The view the photo belongs to.
    pub view: usize,
    /// The clicked pixels, vertex by vertex.
    pub pixels: Vec<[f64; 2]>,
}

/// A line in space: a point and a unit direction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Line {
    /// A point on the line.
    pub origin: DVec3,
    /// The unit direction.
    pub dir: DVec3,
}

/// Tuning of the triangulation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LocateOptions {
    /// A view is flagged when its distance from the solution of all the OTHER views exceeds
    /// this multiple of those views' own RMS distance (default 4).
    pub outlier_ratio: f64,
    /// The smallest RMS, in mm, the outlier test trusts, so a noise-free fit does not flag
    /// everything (default 0.01).
    pub noise_floor_mm: f64,
    /// The fewest usable views for which the outlier test runs (default 4): with three, any
    /// one view can be explained away.
    pub min_views_for_outliers: usize,
}

impl Default for LocateOptions {
    fn default() -> Self {
        Self {
            outlier_ratio: 4.0,
            noise_floor_mm: 0.01,
            min_views_for_outliers: 4,
        }
    }
}

/// What happened to one view's mark.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ViewStatus {
    /// The mark was traced and used.
    Used,
    /// The ray misses the stone.
    NoSurfaceHit,
    /// The ray starts inside the stone or meets its surface from behind.
    FromInside,
    /// Beyond the critical angle at entry: the mark is invalid for this view.
    TotalInternalReflection,
    /// The ray never leaves the stone again (a damaged mesh).
    NoExit,
    /// The view does not exist in the rig.
    NoSuchView,
}

impl ViewStatus {
    const fn of(error: TraceError) -> Self {
        match error {
            TraceError::NoSuchView => Self::NoSuchView,
            TraceError::Miss => Self::NoSurfaceHit,
            TraceError::FromInside => Self::FromInside,
            TraceError::TotalInternalReflection => Self::TotalInternalReflection,
            TraceError::NoExit | TraceError::NotReflected(_) => Self::NoExit,
        }
    }
}

/// What the solution says about one view.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ViewReport {
    /// The view.
    pub view: usize,
    /// Whether the mark could be used.
    pub status: ViewStatus,
    /// The distance of the solution from this view's interior ray, in mm.
    pub distance_mm: Option<f64>,
    /// The distance of this view's ray from the solution of all the other views, in mm.
    pub leave_one_out_mm: Option<f64>,
    /// The RMS distance of the other views' rays from their own solution, in mm.
    pub rms_without_mm: Option<f64>,
    /// This view disagrees with the others by far more than they disagree among themselves:
    /// a likely ghost image or a wrong click.
    pub likely_outlier: bool,
    /// The solution lies beyond the stretch of the ray inside the stone.
    pub beyond_segment: bool,
}

/// Where the solution ended up relative to the mesh.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InsideState {
    /// The free solution was inside the mesh.
    Inside,
    /// It was outside and was moved inward along the line to the middle of the rays.
    MovedInward,
    /// It is outside and could not be moved: the rays do not meet inside the stone.
    Outside,
}

/// A located point.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Located {
    /// The point, in the mesh frame in mm.
    pub point: [f64; 3],
    /// The RMS distance of the interior rays from the point, in mm: the uncertainty.
    pub rms_mm: f64,
    /// The same corrected for the degrees of freedom (`2n - 3` for `n` views), the estimate of
    /// the per-ray error in mm.
    pub sigma_mm: f64,
    /// How many views were used.
    pub used_views: usize,
    /// Whether the point had to be moved into the mesh.
    pub inside: InsideState,
    /// One report per mark, in the order the marks were given.
    pub views: Vec<ViewReport>,
}

impl Located {
    /// The point as a vector.
    #[must_use]
    pub const fn point_vec(&self) -> DVec3 {
        DVec3::from_array(self.point)
    }

    /// The views flagged as likely outliers.
    #[must_use]
    pub fn outlier_views(&self) -> Vec<usize> {
        self.views
            .iter()
            .filter(|report| report.likely_outlier)
            .map(|report| report.view)
            .collect()
    }
}

/// Why a point could not be located.
#[derive(Debug, Clone, PartialEq)]
pub enum LocateError {
    /// Fewer than two marks could be traced into the stone.
    TooFewViews {
        /// How many could.
        usable: usize,
        /// What happened to every mark.
        views: Vec<ViewReport>,
    },
    /// The rays are all parallel (or nearly): the point is undetermined.
    Degenerate,
    /// The views of a polyline do not all have the same number of vertices.
    MismatchedVertexCount,
    /// A vertex of a polyline could not be located.
    AtVertex {
        /// The vertex (0-based).
        index: usize,
        /// Why.
        reason: Box<Self>,
    },
}

impl fmt::Display for LocateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooFewViews { usable, .. } => write!(
                f,
                "only {usable} of the marks reach the stone's interior; at least 2 are needed"
            ),
            Self::Degenerate => write!(f, "the rays are parallel, so the position is undetermined"),
            Self::MismatchedVertexCount => {
                write!(f, "the views do not have the same number of vertices")
            }
            Self::AtVertex { index, reason } => {
                write!(f, "vertex {}: {reason}", index + 1)
            }
        }
    }
}

impl std::error::Error for LocateError {}

/// A polyline or polygon located vertex by vertex.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LocatedPolyline {
    /// The located vertices, in order.
    pub vertices: Vec<Located>,
    /// Whether the last vertex joins the first (a polygon).
    pub closed: bool,
}

impl LocatedPolyline {
    /// The largest uncertainty of any vertex, in mm.
    #[must_use]
    pub fn worst_rms_mm(&self) -> f64 {
        self.vertices
            .iter()
            .map(|vertex| vertex.rms_mm)
            .fold(0.0, f64::max)
    }
}

/// The point closest, in the least-squares sense, to all the lines: `None` when they are all
/// parallel (or there are none).
#[must_use]
pub fn closest_point_to_lines(lines: &[Line]) -> Option<DVec3> {
    let mut normal = DMat3::ZERO;
    let mut rhs = DVec3::ZERO;
    for line in lines {
        let dir = line.dir;
        let across = DMat3::from_cols(
            DVec3::X - dir * dir.x,
            DVec3::Y - dir * dir.y,
            DVec3::Z - dir * dir.z,
        );
        normal += across;
        rhs += across * line.origin;
    }
    let count = lines.len() as f64;
    let det = normal.determinant();
    if !det.is_finite() || det.abs() < 1e-9 * count * count * count {
        return None;
    }
    Some(normal.inverse() * rhs)
}

/// One mark that was traced into the stone.
struct Usable {
    /// Index into the reports.
    report: usize,
    line: Line,
    leg: Leg,
}

/// Traces every mark; returns one report per mark and the usable ones.
fn trace_marks(scene: &Scene<'_>, marks: &[Mark]) -> (Vec<ViewReport>, Vec<Usable>) {
    let mut reports = Vec::with_capacity(marks.len());
    let mut usable = Vec::new();
    for (index, mark) in marks.iter().enumerate() {
        let mut report = ViewReport {
            view: mark.view,
            status: ViewStatus::Used,
            distance_mm: None,
            leave_one_out_mm: None,
            rms_without_mm: None,
            likely_outlier: false,
            beyond_segment: false,
        };
        match trace_pixel(scene, mark.view, DVec2::from_array(mark.pixel), 0) {
            Ok(path) => {
                // A path always has its first stretch (one per bounce, and no bounce here).
                let leg = path.legs[0];
                usable.push(Usable {
                    report: index,
                    line: Line {
                        origin: leg.from,
                        dir: leg.dir(),
                    },
                    leg,
                });
            }
            Err(error) => report.status = ViewStatus::of(error),
        }
        reports.push(report);
    }
    (reports, usable)
}

/// The least-squares point of the usable views except `skip`.
fn solve(usable: &[Usable], skip: Option<usize>) -> Option<DVec3> {
    let lines: Vec<Line> = usable
        .iter()
        .enumerate()
        .filter(|&(k, _)| Some(k) != skip)
        .map(|(_, item)| item.line)
        .collect();
    closest_point_to_lines(&lines)
}

/// The distance of `point` from the stretch of every usable view except `skip`.
fn distances(usable: &[Usable], skip: Option<usize>, point: DVec3) -> Vec<f64> {
    usable
        .iter()
        .enumerate()
        .filter(|&(k, _)| Some(k) != skip)
        .map(|(_, item)| item.leg.distance_to(point))
        .collect()
}

fn rms_of(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    (values.iter().map(|v| v * v).sum::<f64>() / values.len() as f64).sqrt()
}

/// Keeps `point` inside the mesh: when it is outside, moves it toward the middle of the first
/// usable stretch whose midpoint is inside, to the last point of that line that is.
fn keep_inside(scene: &Scene<'_>, point: DVec3, usable: &[Usable]) -> (DVec3, InsideState) {
    if scene.mesh.contains_point(point) {
        return (point, InsideState::Inside);
    }
    let pivot = usable
        .iter()
        .map(|item| item.leg.from.midpoint(item.leg.to))
        .find(|&middle| scene.mesh.contains_point(middle));
    let Some(pivot) = pivot else {
        return (point, InsideState::Outside);
    };
    let (mut inside_end, mut outside_end) = (0.0, 1.0);
    for _ in 0..48 {
        let middle = f64::midpoint(inside_end, outside_end);
        if scene.mesh.contains_point(pivot + (point - pivot) * middle) {
            inside_end = middle;
        } else {
            outside_end = middle;
        }
    }
    (
        pivot + (point - pivot) * inside_end,
        InsideState::MovedInward,
    )
}

/// Fills the leave-one-out fields of the reports.
fn leave_one_out(usable: &[Usable], options: &LocateOptions, reports: &mut [ViewReport]) {
    if usable.len() < options.min_views_for_outliers.max(3) {
        return;
    }
    for (k, item) in usable.iter().enumerate() {
        let Some(without) = solve(usable, Some(k)) else {
            continue;
        };
        let rms_without = rms_of(&distances(usable, Some(k), without));
        let own = item.leg.distance_to(without);
        let report = &mut reports[item.report];
        report.leave_one_out_mm = Some(own);
        report.rms_without_mm = Some(rms_without);
        report.likely_outlier =
            own > options.outlier_ratio * rms_without.max(options.noise_floor_mm);
    }
}

/// Locates the point seen at `marks`: refracts every mark's ray into the stone and finds the
/// point closest to all of them.
///
/// Marks that cannot be traced (a miss, total internal reflection at entry, an unknown view)
/// are reported and left out. The point is kept inside the mesh. With enough views a leave-one-out
/// pass flags the views that disagree with the rest.
///
/// # Errors
///
/// [`LocateError::TooFewViews`] when fewer than two marks could be traced, and
/// [`LocateError::Degenerate`] when their rays are all parallel.
pub fn locate_point(
    scene: &Scene<'_>,
    marks: &[Mark],
    options: &LocateOptions,
) -> Result<Located, LocateError> {
    let (mut reports, usable) = trace_marks(scene, marks);
    if usable.len() < 2 {
        return Err(LocateError::TooFewViews {
            usable: usable.len(),
            views: reports,
        });
    }
    let free = solve(&usable, None).ok_or(LocateError::Degenerate)?;
    let (point, inside) = keep_inside(scene, free, &usable);
    let tolerance = 1e-6 * scene.mesh_scale();
    for item in &usable {
        let report = &mut reports[item.report];
        report.distance_mm = Some(item.leg.distance_to(point));
        let along = item.leg.param_of(point);
        report.beyond_segment = along < -tolerance || along > item.leg.length() + tolerance;
    }
    leave_one_out(&usable, options, &mut reports);
    let all = distances(&usable, None, point);
    let degrees_of_freedom = (2 * usable.len()).saturating_sub(3).max(1) as f64;
    let sigma_mm = (all.iter().map(|v| v * v).sum::<f64>() / degrees_of_freedom).sqrt();
    Ok(Located {
        point: point.to_array(),
        rms_mm: rms_of(&all),
        sigma_mm,
        used_views: usable.len(),
        inside,
        views: reports,
    })
}

/// Locates a polyline or polygon vertex by vertex. Vertex `k` of every view's polyline is the
/// same physical vertex, so the views must all have the same number of vertices.
///
/// # Errors
///
/// [`LocateError::MismatchedVertexCount`] when the views disagree on the vertex count,
/// [`LocateError::AtVertex`] naming the first vertex that cannot be located, and
/// [`LocateError::TooFewViews`] when no view was given.
pub fn locate_polyline(
    scene: &Scene<'_>,
    polylines: &[ViewPolyline],
    closed: bool,
    options: &LocateOptions,
) -> Result<LocatedPolyline, LocateError> {
    let Some(first) = polylines.first() else {
        return Err(LocateError::TooFewViews {
            usable: 0,
            views: Vec::new(),
        });
    };
    let count = first.pixels.len();
    if polylines.iter().any(|line| line.pixels.len() != count) {
        return Err(LocateError::MismatchedVertexCount);
    }
    let mut vertices = Vec::with_capacity(count);
    for index in 0..count {
        let marks: Vec<Mark> = polylines
            .iter()
            .map(|line| Mark {
                view: line.view,
                pixel: line.pixels[index],
            })
            .collect();
        match locate_point(scene, &marks, options) {
            Ok(located) => vertices.push(located),
            Err(reason) => {
                return Err(LocateError::AtVertex {
                    index,
                    reason: Box::new(reason),
                });
            }
        }
    }
    Ok(LocatedPolyline { vertices, closed })
}
