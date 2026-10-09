//! Primitive fits: 3D boundary points (from `locate_polyline`, in the mesh frame) to a
//! [`ZoneShape`], with the fit residual and a leave-one-view-out check.
//!
//! # Method (all deterministic)
//!
//! * **Plane**: PCA; the normal is the eigenvector of the smallest eigenvalue.
//! * **Slab**: the common normal is the smallest eigenvector of the summed scatter of the two
//!   point sets, each about its own centroid; the offsets are the mean projections.
//! * **Cylinder**: damped Gauss-Newton ([`least_squares`]) on the axis (a tilt `(a, b)` of a start
//!   direction plus a perpendicular shift `(s, t)` of the axis point) and the radius, with central
//!   differences. The start axis is each PCA direction in turn (largest variance first); the best
//!   of the three wins. With a fixed axis only the shift and the radius are free.
//! * **Coaxial prism**: the same unknowns plus the phase of the polygon. The residual is the
//!   polygon gauge `max_k w . n_k - apothem` (`w` the point's offset from the axis, perpendicular
//!   to it), which is zero on the whole polygon boundary. The start phase is a 36-point grid
//!   search over one period.
//! * **Sector**: an axis (fitted or fixed) and the angles of the two edge half-planes, from two
//!   point sets, each on one edge. A hint point inside the wedge picks which of the two
//!   complementary wedges is meant, otherwise the smaller one (at most half a turn).
//!
//! Each Gauss-Newton run has an iteration cap of 60 and stops when a step is below 1e-4 of the
//! difference step or the relative gain is below 1e-13. There is no RANSAC and no random start.
//!
//! # Leave-one-view-out
//!
//! The points of a [`BoundaryPoints`] are located from all the marked views. Optionally it also
//! carries, for each view, the points located WITHOUT that view's marks
//! ([`BoundaryPoints::locate`] builds them). The shape is refitted without each view in turn and
//! the residual of the full point set against that shape is measured:
//! [`FittedZoneShape::leave_one_view_out_rms`] is the root mean square over the views of those
//! residual RMS values (mm). A large value against `rms_mm` means one view drives the fit.

use std::{f64::consts::TAU, fmt};

use glam::{DQuat, DVec3};
use indicatrix::optics::zoning::{MAX_PRISM_SIDES, MIN_PRISM_SIDES, ZoneShape};

use super::geometry::{
    angle_in, axis_reference, canonical_dir, centroid, extent, gauge, least_squares, pca,
    polygon_normals, rms, scatter, sym_eigen3,
};
use crate::rough_plan::locate::{
    LocateError, LocateOptions, LocatedPolyline, Scene, ViewPolyline, locate_polyline,
};

/// Why a primitive could not be fitted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ZoneFitError {
    /// Too few points for the primitive.
    TooFewPoints {
        /// The fewest points needed.
        needed: usize,
        /// The points given.
        found: usize,
    },
    /// A point is not finite.
    NotFinite,
    /// The points do not determine the primitive (collinear, coincident planes, no radius, ...).
    Degenerate(&'static str),
    /// The fixed axis is zero or not finite.
    BadAxis,
    /// The side count of a prism is outside 3 to 12.
    BadSideCount(u32),
}

impl fmt::Display for ZoneFitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooFewPoints { needed, found } => {
                write!(
                    f,
                    "{found} points are not enough; this shape needs {needed}"
                )
            }
            Self::NotFinite => write!(f, "a boundary point is not a finite position"),
            Self::Degenerate(why) => write!(f, "the points do not determine the shape: {why}"),
            Self::BadAxis => write!(f, "the fixed axis is zero or not finite"),
            Self::BadSideCount(n) => write!(
                f,
                "a prism has {MIN_PRISM_SIDES} to {MAX_PRISM_SIDES} sides, not {n}"
            ),
        }
    }
}

impl std::error::Error for ZoneFitError {}

/// A fitted primitive with its quality.
#[derive(Debug, Clone, PartialEq)]
pub struct FittedZoneShape {
    /// The shape, in the mesh frame (use it with an identity zone frame).
    pub shape: ZoneShape,
    /// Root mean square of the residuals, mm.
    pub rms_mm: f64,
    /// The signed residual of every input point, mm: the distance from the fitted surface (plane
    /// distance, radial distance, polygon gauge) in the order the points were given; for the two
    /// point sets of a slab or sector, the first set then the second.
    pub per_point_residual: Vec<f64>,
    /// Root mean square over the views of the residual RMS of the full point set against the shape
    /// refitted without that view (module docs); `None` without left-out point sets.
    pub leave_one_view_out_rms: Option<f64>,
}

/// The points that the same boundary gives when one view's marks are left out.
#[derive(Debug, Clone, PartialEq)]
pub struct LeftOutView {
    /// The rig view that was left out.
    pub view: usize,
    /// The located points without it (the same vertex order as the full set).
    pub points: Vec<DVec3>,
}

/// The 3D points of a boundary, in the mesh frame, mm.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BoundaryPoints {
    /// The points located from all views.
    pub points: Vec<DVec3>,
    /// The points located without one view, per view (empty when not available).
    pub left_out: Vec<LeftOutView>,
}

impl BoundaryPoints {
    /// Points without left-out sets.
    #[must_use]
    pub const fn from_points(points: Vec<DVec3>) -> Self {
        Self {
            points,
            left_out: Vec::new(),
        }
    }

    /// This set with the points located without `view`.
    #[must_use]
    pub fn with_left_out(mut self, view: usize, points: Vec<DVec3>) -> Self {
        self.left_out.push(LeftOutView { view, points });
        self
    }

    /// The vertices of a located polyline.
    #[must_use]
    pub fn from_located(polyline: &LocatedPolyline) -> Self {
        Self::from_points(
            polyline
                .vertices
                .iter()
                .map(crate::rough_plan::locate::triangulate::Located::point_vec)
                .collect(),
        )
    }

    /// Locates the polylines marked in the photos (all views), and again once per view with that
    /// view's marks left out when at least three views are marked. A left-out location that
    /// fails is skipped.
    ///
    /// # Errors
    ///
    /// The [`LocateError`] of the full location.
    pub fn locate(
        scene: &Scene<'_>,
        polylines: &[ViewPolyline],
        closed: bool,
        options: &LocateOptions,
    ) -> Result<Self, LocateError> {
        let full = locate_polyline(scene, polylines, closed, options)?;
        let mut out = Self::from_located(&full);
        if polylines.len() >= 3 {
            for skip in 0..polylines.len() {
                let rest: Vec<ViewPolyline> = polylines
                    .iter()
                    .enumerate()
                    .filter(|&(i, _)| i != skip)
                    .map(|(_, p)| p.clone())
                    .collect();
                if let Ok(without) = locate_polyline(scene, &rest, closed, options) {
                    let set = Self::from_located(&without);
                    out.left_out.push(LeftOutView {
                        view: polylines[skip].view,
                        points: set.points,
                    });
                }
            }
        }
        Ok(out)
    }
}

// ---------------------------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------------------------

fn check_points(points: &[DVec3], needed: usize) -> Result<(), ZoneFitError> {
    if points.len() < needed {
        return Err(ZoneFitError::TooFewPoints {
            needed,
            found: points.len(),
        });
    }
    if points.iter().any(|p| !p.is_finite()) {
        return Err(ZoneFitError::NotFinite);
    }
    Ok(())
}

fn unit_axis(axis: Option<DVec3>) -> Result<Option<DVec3>, ZoneFitError> {
    axis.map_or(Ok(None), |a| {
        a.try_normalize().map(Some).ok_or(ZoneFitError::BadAxis)
    })
}

fn finish(shape: ZoneShape, residuals: Vec<f64>, lovo: Option<f64>) -> FittedZoneShape {
    FittedZoneShape {
        shape,
        rms_mm: rms(&residuals),
        per_point_residual: residuals,
        leave_one_view_out_rms: lovo,
    }
}

type FitFn<'a> = dyn Fn(&[Vec<DVec3>]) -> Option<ZoneShape> + 'a;
type RmsFn<'a> = dyn Fn(&ZoneShape, &[Vec<DVec3>]) -> f64 + 'a;

/// The leave-one-view-out statistic (module docs). `fit` refits from point sets, `rms_of`
/// measures the residual RMS of point sets against a shape.
fn lovo_rms(full: &[&BoundaryPoints], fit: &FitFn<'_>, rms_of: &RmsFn<'_>) -> Option<f64> {
    let mut views: Vec<usize> = full
        .iter()
        .flat_map(|b| b.left_out.iter().map(|l| l.view))
        .collect();
    views.sort_unstable();
    views.dedup();
    let whole: Vec<Vec<DVec3>> = full.iter().map(|b| b.points.clone()).collect();
    let mut sum = 0.0;
    let mut count = 0_usize;
    for view in views {
        let sets: Vec<Vec<DVec3>> = full
            .iter()
            .map(|b| {
                b.left_out
                    .iter()
                    .find(|l| l.view == view)
                    .map_or_else(|| b.points.clone(), |l| l.points.clone())
            })
            .collect();
        let Some(shape) = fit(&sets) else {
            continue;
        };
        let r = rms_of(&shape, &whole);
        if r.is_finite() {
            sum += r * r;
            count += 1;
        }
    }
    if count == 0 {
        None
    } else {
        Some((sum / count as f64).sqrt())
    }
}

// ---------------------------------------------------------------------------------------------
// Plane and half space
// ---------------------------------------------------------------------------------------------

enum Orient {
    Canonical,
    Toward(DVec3),
    Along(DVec3),
}

struct PlaneFit {
    normal: DVec3,
    offset: f64,
}

fn plane_fit(points: &[DVec3], orient: &Orient) -> Result<PlaneFit, ZoneFitError> {
    check_points(points, 3)?;
    let p = pca(points);
    let [_, mid, hi] = p.values;
    if mid <= 1e-12 * hi || hi <= 0.0 {
        return Err(ZoneFitError::Degenerate("the points are collinear"));
    }
    let mut normal = canonical_dir(p.vectors[0]);
    match orient {
        Orient::Canonical => {}
        Orient::Toward(t) => {
            if (*t - p.centroid).dot(normal) < 0.0 {
                normal = -normal;
            }
        }
        Orient::Along(n) => {
            if n.dot(normal) < 0.0 {
                normal = -normal;
            }
        }
    }
    Ok(PlaneFit {
        normal,
        offset: normal.dot(p.centroid),
    })
}

/// Fits the half space `normal . p >= offset` whose plane contains the points.
///
/// The normal points toward `toward` when given (the zone lies on that side), otherwise it is
/// oriented by the largest-component-positive rule.
///
/// # Errors
///
/// [`ZoneFitError`] for fewer than 3 points, a non-finite point or collinear points.
pub fn fit_half_space(
    points: &BoundaryPoints,
    toward: Option<DVec3>,
) -> Result<FittedZoneShape, ZoneFitError> {
    let orient = toward.map_or(Orient::Canonical, Orient::Toward);
    let main = plane_fit(&points.points, &orient)?;
    let residuals: Vec<f64> = points
        .points
        .iter()
        .map(|p| main.normal.dot(*p) - main.offset)
        .collect();
    let along = main.normal;
    let fit = move |sets: &[Vec<DVec3>]| -> Option<ZoneShape> {
        plane_fit(&sets[0], &Orient::Along(along))
            .ok()
            .map(|f| ZoneShape::HalfSpace {
                normal: f.normal,
                offset: f.offset,
            })
    };
    let measure = |shape: &ZoneShape, sets: &[Vec<DVec3>]| -> f64 {
        match shape {
            ZoneShape::HalfSpace { normal, offset } => {
                let r: Vec<f64> = sets[0].iter().map(|p| normal.dot(*p) - offset).collect();
                rms(&r)
            }
            _ => f64::NAN,
        }
    };
    let lovo = lovo_rms(&[points], &fit, &measure);
    Ok(finish(
        ZoneShape::HalfSpace {
            normal: main.normal,
            offset: main.offset,
        },
        residuals,
        lovo,
    ))
}

// ---------------------------------------------------------------------------------------------
// Slab
// ---------------------------------------------------------------------------------------------

struct SlabFit {
    normal: DVec3,
    offset_a: f64,
    offset_b: f64,
}

fn slab_core(a: &[DVec3], b: &[DVec3]) -> Result<SlabFit, ZoneFitError> {
    check_points(a, 2)?;
    check_points(b, 2)?;
    let (ca, cb) = (centroid(a), centroid(b));
    let sa = scatter(a, ca);
    let sb = scatter(b, cb);
    let mut m = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            m[i][j] = sa[i][j] + sb[i][j];
        }
    }
    let (values, vectors) = sym_eigen3(m);
    if values[1] <= 1e-12 * values[2] || values[2] <= 0.0 {
        return Err(ZoneFitError::Degenerate(
            "the points of the two planes are collinear",
        ));
    }
    let normal = canonical_dir(vectors[0]);
    let offset_a = normal.dot(ca);
    let offset_b = normal.dot(cb);
    if (offset_a - offset_b).abs() < 1e-9 {
        return Err(ZoneFitError::Degenerate("the two planes coincide"));
    }
    Ok(SlabFit {
        normal,
        offset_a,
        offset_b,
    })
}

fn slab_distance(normal: DVec3, lo: f64, hi: f64, p: DVec3) -> f64 {
    let d1 = normal.dot(p) - lo;
    let d2 = normal.dot(p) - hi;
    if d1.abs() <= d2.abs() { d1 } else { d2 }
}

/// Fits two parallel planes from two point sets (one polyline on each) as a slab.
///
/// The residuals are the distances of the first set's points to its own plane, then the second
/// set's.
///
/// # Errors
///
/// [`ZoneFitError`] for fewer than 2 points in a set, collinear points, or coincident planes.
pub fn fit_slab(
    first: &BoundaryPoints,
    second: &BoundaryPoints,
) -> Result<FittedZoneShape, ZoneFitError> {
    let main = slab_core(&first.points, &second.points)?;
    let mut residuals: Vec<f64> = first
        .points
        .iter()
        .map(|p| main.normal.dot(*p) - main.offset_a)
        .collect();
    residuals.extend(
        second
            .points
            .iter()
            .map(|p| main.normal.dot(*p) - main.offset_b),
    );
    let shape_of = |f: &SlabFit| ZoneShape::Slab {
        normal: f.normal,
        offset_min: f.offset_a.min(f.offset_b),
        offset_max: f.offset_a.max(f.offset_b),
    };
    let fit = |sets: &[Vec<DVec3>]| slab_core(&sets[0], &sets[1]).ok().map(|f| shape_of(&f));
    let measure = |shape: &ZoneShape, sets: &[Vec<DVec3>]| -> f64 {
        match shape {
            ZoneShape::Slab {
                normal,
                offset_min,
                offset_max,
            } => {
                let r: Vec<f64> = sets
                    .iter()
                    .flatten()
                    .map(|p| slab_distance(*normal, *offset_min, *offset_max, *p))
                    .collect();
                rms(&r)
            }
            _ => f64::NAN,
        }
    };
    let lovo = lovo_rms(&[first, second], &fit, &measure);
    Ok(finish(shape_of(&main), residuals, lovo))
}

// ---------------------------------------------------------------------------------------------
// Cylinder and prism
// ---------------------------------------------------------------------------------------------

/// Which side of the fitted boundary the zone lies on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RadialRole {
    /// The zone is inside the boundary: a rod (`r_in = 0`, `r_out` = the fitted radius).
    Core,
    /// The zone is outside the boundary: `r_in` = the fitted radius, `r_out` as given (it must
    /// exceed the fitted radius).
    Beyond {
        /// The outer radius, mm.
        r_out: f64,
    },
}

#[derive(Clone, Copy)]
enum Radial {
    Cylinder,
    Prism { sides: u32 },
}

struct RadialFit {
    axis_point: DVec3,
    axis_dir: DVec3,
    /// The radius of a cylinder or the apothem of a prism.
    radius: f64,
    /// The prism phase in the reference frame of `axis_dir` (0 for a cylinder).
    phase: f64,
    residuals: Vec<f64>,
}

/// The phase (within one period) whose polygon gauge is the most constant over the points, and
/// the mean gauge there (a start apothem).
fn best_phase(
    points: &[DVec3],
    centre: DVec3,
    axis: DVec3,
    e1: DVec3,
    e2: DVec3,
    sides: u32,
) -> (f64, f64) {
    const GRID: usize = 36;
    let period = TAU / f64::from(sides);
    let mut best = (f64::INFINITY, 0.0, 0.0);
    for m in 0..GRID {
        let psi = period * m as f64 / GRID as f64;
        let normals = polygon_normals(sides, psi, e1, e2);
        let gauges: Vec<f64> = points
            .iter()
            .map(|p| {
                let q = *p - centre;
                gauge(&normals, q - axis * q.dot(axis))
            })
            .collect();
        let mean = gauges.iter().sum::<f64>() / gauges.len() as f64;
        let var: f64 = gauges.iter().map(|g| (g - mean) * (g - mean)).sum();
        if var < best.0 {
            best = (var, psi, mean);
        }
    }
    (best.1, best.2)
}

fn radial_from_start(
    points: &[DVec3],
    kind: Radial,
    d0: DVec3,
    axis_fixed: bool,
    c0: DVec3,
    scale: f64,
) -> Option<RadialFit> {
    let (e1, e2) = axis_reference(d0);
    let (psi0, radius0) = match kind {
        Radial::Cylinder => {
            let mut sum = 0.0;
            for p in points {
                let q = *p - c0;
                sum += (q - d0 * q.dot(d0)).length();
            }
            (0.0, sum / points.len() as f64)
        }
        Radial::Prism { sides } => best_phase(points, c0, d0, e1, e2, sides),
    };
    if !(radius0.is_finite() && radius0 > 1e-9 * scale) {
        return None;
    }
    let prism = matches!(kind, Radial::Prism { .. });
    let mut free: Vec<usize> = vec![0, 1];
    if !axis_fixed {
        free.push(2);
        free.push(3);
    }
    free.push(4);
    if prism {
        free.push(5);
    }
    // Parameters: shift s, t of the axis point along e1, e2; tilt a, b of the axis; radius; phase.
    let base = [0.0, 0.0, 0.0, 0.0, radius0, psi0];
    let x0: Vec<f64> = free.iter().map(|&i| base[i]).collect();
    let steps: Vec<f64> = free
        .iter()
        .map(|&i| {
            if matches!(i, 0 | 1 | 4) {
                1e-6 * scale
            } else {
                1e-6
            }
        })
        .collect();
    let expand = |x: &[f64]| -> [f64; 6] {
        let mut full = base;
        for (k, &i) in free.iter().enumerate() {
            full[i] = x[k];
        }
        full
    };
    let geometry = |full: &[f64; 6]| -> (DVec3, DVec3) {
        let d = (d0 + e1 * full[2] + e2 * full[3]).normalize();
        let c = c0 + e1 * full[0] + e2 * full[1];
        (d, c)
    };
    let residuals = |x: &[f64], out: &mut Vec<f64>| {
        out.clear();
        let full = expand(x);
        let (d, c) = geometry(&full);
        match kind {
            Radial::Cylinder => {
                for p in points {
                    let q = *p - c;
                    out.push((q - d * q.dot(d)).length() - full[4]);
                }
            }
            Radial::Prism { sides } => {
                let rot = DQuat::from_rotation_arc(d0, d);
                let normals = polygon_normals(sides, full[5], rot * e1, rot * e2);
                for p in points {
                    let q = *p - c;
                    out.push(gauge(&normals, q - d * q.dot(d)) - full[4]);
                }
            }
        }
    };
    let sol = least_squares(&x0, points.len(), &steps, 60, &residuals);
    let full = expand(&sol.x);
    let (d, c) = geometry(&full);
    let radius = full[4];
    if !(radius.is_finite() && radius > 1e-9 * scale && d.is_finite() && c.is_finite()) {
        return None;
    }
    let mut res = Vec::with_capacity(points.len());
    residuals(&sol.x, &mut res);
    let axis_dir = if axis_fixed { d } else { canonical_dir(d) };
    let axis_point = c + d * (c0 - c).dot(d);
    let phase = match kind {
        Radial::Cylinder => 0.0,
        Radial::Prism { sides } => {
            let rot = DQuat::from_rotation_arc(d0, d);
            let n0 = polygon_normals(sides, full[5], rot * e1, rot * e2)[0];
            let (u, v) = axis_reference(axis_dir);
            angle_in(n0, u, v).rem_euclid(TAU / f64::from(sides))
        }
    };
    Some(RadialFit {
        axis_point,
        axis_dir,
        radius,
        phase,
        residuals: res,
    })
}

fn radial_fit(
    points: &[DVec3],
    kind: Radial,
    fixed_axis: Option<DVec3>,
) -> Result<RadialFit, ZoneFitError> {
    let fixed = unit_axis(fixed_axis)?;
    let free_axis_extra: usize = if fixed.is_some() { 4 } else { 6 };
    let needed = free_axis_extra + usize::from(matches!(kind, Radial::Prism { .. }));
    check_points(points, needed)?;
    let p = pca(points);
    let scale = extent(points).max(1e-9);
    let starts: Vec<DVec3> = fixed.map_or_else(
        || vec![p.vectors[2], p.vectors[1], p.vectors[0]],
        |a| vec![a],
    );
    let mut best: Option<(f64, RadialFit)> = None;
    for d0 in starts {
        if let Some(fit) = radial_from_start(points, kind, d0, fixed.is_some(), p.centroid, scale) {
            let r = rms(&fit.residuals);
            if best.as_ref().is_none_or(|(b, _)| r < *b) {
                best = Some((r, fit));
            }
        }
    }
    best.map(|(_, f)| f).ok_or(ZoneFitError::Degenerate(
        "the fit did not reach a usable radius",
    ))
}

fn cylinder_shape(fit: &RadialFit, role: RadialRole) -> Result<ZoneShape, ZoneFitError> {
    let (r_in, r_out) = match role {
        RadialRole::Core => (0.0, fit.radius),
        RadialRole::Beyond { r_out } => {
            if !(r_out.is_finite() && r_out > fit.radius) {
                return Err(ZoneFitError::Degenerate(
                    "the outer radius must exceed the fitted radius",
                ));
            }
            (fit.radius, r_out)
        }
    };
    Ok(ZoneShape::CoaxialCylinder {
        axis_point: fit.axis_point,
        axis_dir: fit.axis_dir,
        r_in,
        r_out,
    })
}

fn prism_shape(fit: &RadialFit, sides: u32, role: RadialRole) -> Result<ZoneShape, ZoneFitError> {
    let (r_in, r_out) = match role {
        RadialRole::Core => (0.0, fit.radius),
        RadialRole::Beyond { r_out } => {
            if !(r_out.is_finite() && r_out > fit.radius) {
                return Err(ZoneFitError::Degenerate(
                    "the outer apothem must exceed the fitted apothem",
                ));
            }
            (fit.radius, r_out)
        }
    };
    Ok(ZoneShape::CoaxialPrism {
        axis_point: fit.axis_point,
        axis_dir: fit.axis_dir,
        n_sides: sides,
        r_in,
        r_out,
        phase: fit.phase,
    })
}

/// The residual of a point against the boundary a radial shape was fitted to (`r_out` for a
/// core, `r_in` for a zone beyond).
fn radial_residual(shape: &ZoneShape, core: bool, p: DVec3) -> f64 {
    match shape {
        ZoneShape::CoaxialCylinder {
            axis_point,
            axis_dir,
            r_in,
            r_out,
        } => {
            let q = p - *axis_point;
            (q - *axis_dir * q.dot(*axis_dir)).length() - if core { *r_out } else { *r_in }
        }
        ZoneShape::CoaxialPrism {
            axis_point,
            axis_dir,
            n_sides,
            r_in,
            r_out,
            phase,
        } => {
            let (u, v) = axis_reference(*axis_dir);
            let normals = polygon_normals(*n_sides, *phase, u, v);
            let q = p - *axis_point;
            gauge(&normals, q - *axis_dir * q.dot(*axis_dir)) - if core { *r_out } else { *r_in }
        }
        _ => f64::NAN,
    }
}

/// Fits a cylinder (round watermelon, rod or tube wall) to points on one of its surfaces.
///
/// With `fixed_axis` (for instance the crystal's c axis) only the axis position and the radius
/// are fitted. `role` says whether the zone is inside or outside the boundary the points mark.
///
/// # Errors
///
/// [`ZoneFitError`] for too few points (6, or 4 with a fixed axis), a bad axis, or a fit that does
/// not reach a positive radius.
pub fn fit_cylinder(
    points: &BoundaryPoints,
    fixed_axis: Option<DVec3>,
    role: RadialRole,
) -> Result<FittedZoneShape, ZoneFitError> {
    let main = radial_fit(&points.points, Radial::Cylinder, fixed_axis)?;
    let shape = cylinder_shape(&main, role)?;
    let core = matches!(role, RadialRole::Core);
    let fit = move |sets: &[Vec<DVec3>]| -> Option<ZoneShape> {
        radial_fit(&sets[0], Radial::Cylinder, fixed_axis)
            .ok()
            .and_then(|f| cylinder_shape(&f, role).ok())
    };
    let measure = move |shape: &ZoneShape, sets: &[Vec<DVec3>]| -> f64 {
        let r: Vec<f64> = sets[0]
            .iter()
            .map(|p| radial_residual(shape, core, *p))
            .collect();
        rms(&r)
    };
    let lovo = lovo_rms(&[points], &fit, &measure);
    Ok(finish(shape, main.residuals, lovo))
}

/// Fits a regular prism around an axis (trigonal or hexagonal watermelon) to points on one of its
/// side faces: the apothem and the phase, and the axis unless it is fixed.
///
/// `sides` is 3 to 12 (3 or 6 by crystal habit). `phase` of the result is measured from the
/// axis reference direction of the kernels, reduced to one period.
///
/// # Errors
///
/// [`ZoneFitError`] for a bad side count, too few points (7, or 5 with a fixed axis), a bad axis,
/// or a fit that does not reach a positive apothem.
pub fn fit_prism(
    points: &BoundaryPoints,
    sides: u32,
    fixed_axis: Option<DVec3>,
    role: RadialRole,
) -> Result<FittedZoneShape, ZoneFitError> {
    if !(MIN_PRISM_SIDES..=MAX_PRISM_SIDES).contains(&sides) {
        return Err(ZoneFitError::BadSideCount(sides));
    }
    let kind = Radial::Prism { sides };
    let main = radial_fit(&points.points, kind, fixed_axis)?;
    let shape = prism_shape(&main, sides, role)?;
    let core = matches!(role, RadialRole::Core);
    let fit = move |sets: &[Vec<DVec3>]| -> Option<ZoneShape> {
        radial_fit(&sets[0], kind, fixed_axis)
            .ok()
            .and_then(|f| prism_shape(&f, sides, role).ok())
    };
    let measure = move |shape: &ZoneShape, sets: &[Vec<DVec3>]| -> f64 {
        let r: Vec<f64> = sets[0]
            .iter()
            .map(|p| radial_residual(shape, core, *p))
            .collect();
        rms(&r)
    };
    let lovo = lovo_rms(&[points], &fit, &measure);
    Ok(finish(shape, main.residuals, lovo))
}

// ---------------------------------------------------------------------------------------------
// Sector
// ---------------------------------------------------------------------------------------------

struct SectorFit {
    axis_point: DVec3,
    axis_dir: DVec3,
    angle_from: f64,
    angle_to: f64,
    residuals: Vec<f64>,
}

fn outer(u: DVec3, v: DVec3) -> glam::DMat3 {
    glam::DMat3::from_cols(u * v.x, u * v.y, u * v.z)
}

fn sector_core(
    a: &[DVec3],
    b: &[DVec3],
    fixed_axis: Option<DVec3>,
    hint: Option<DVec3>,
) -> Result<SectorFit, ZoneFitError> {
    check_points(a, 3)?;
    check_points(b, 3)?;
    let fixed = unit_axis(fixed_axis)?;
    let pa = plane_fit(a, &Orient::Canonical)?;
    let pb = plane_fit(b, &Orient::Canonical)?;
    let d0 = if let Some(d) = fixed {
        d
    } else {
        let cross = pa.normal.cross(pb.normal);
        if cross.length() < 1e-3 {
            return Err(ZoneFitError::Degenerate(
                "the two edge planes are (nearly) parallel; give the axis",
            ));
        }
        canonical_dir(cross.normalize())
    };
    let all: Vec<DVec3> = a.iter().chain(b.iter()).copied().collect();
    let g = centroid(&all);
    // A start point on (near) the axis: least squares to both planes and to the plane through
    // the overall centroid perpendicular to the axis.
    let m = outer(pa.normal, pa.normal) + outer(pb.normal, pb.normal) + outer(d0, d0);
    let rhs = pa.normal * pa.offset + pb.normal * pb.offset + d0 * d0.dot(g);
    let det = m.determinant();
    let c0 = if det.is_finite() && det.abs() > 1e-12 {
        m.inverse() * rhs
    } else {
        g
    };
    let (e1, e2) = axis_reference(d0);
    let angle_of = |set: &[DVec3]| {
        let rel = centroid(set) - c0;
        rel.dot(e2).atan2(rel.dot(e1))
    };
    let (th_a0, th_b0) = (angle_of(a), angle_of(b));
    let scale = extent(&all).max(1e-9);
    let mut free: Vec<usize> = vec![0, 1];
    if fixed.is_none() {
        free.push(2);
        free.push(3);
    }
    free.push(4);
    free.push(5);
    let base = [0.0, 0.0, 0.0, 0.0, th_a0, th_b0];
    let x0: Vec<f64> = free.iter().map(|&i| base[i]).collect();
    let steps: Vec<f64> = free
        .iter()
        .map(|&i| {
            if matches!(i, 0 | 1) {
                1e-6 * scale
            } else {
                1e-6
            }
        })
        .collect();
    let expand = |x: &[f64]| -> [f64; 6] {
        let mut full = base;
        for (k, &i) in free.iter().enumerate() {
            full[i] = x[k];
        }
        full
    };
    let frame = |full: &[f64; 6]| -> (DVec3, DVec3, DVec3, DVec3) {
        let d = (d0 + e1 * full[2] + e2 * full[3]).normalize();
        let c = c0 + e1 * full[0] + e2 * full[1];
        let rot = DQuat::from_rotation_arc(d0, d);
        (d, c, rot * e1, rot * e2)
    };
    let residuals = |x: &[f64], out: &mut Vec<f64>| {
        out.clear();
        let full = expand(x);
        let (_, c, f1, f2) = frame(&full);
        let na = f2 * full[4].cos() - f1 * full[4].sin();
        let nb = f2 * full[5].cos() - f1 * full[5].sin();
        for p in a {
            out.push(na.dot(*p - c));
        }
        for p in b {
            out.push(nb.dot(*p - c));
        }
    };
    let sol = least_squares(&x0, all.len(), &steps, 60, &residuals);
    let full = expand(&sol.x);
    let (d, c, f1, f2) = frame(&full);
    if !(d.is_finite() && c.is_finite()) {
        return Err(ZoneFitError::Degenerate("the sector fit diverged"));
    }
    let mut res = Vec::with_capacity(all.len());
    residuals(&sol.x, &mut res);
    let axis_dir = if fixed.is_some() { d } else { canonical_dir(d) };
    let axis_point = c + d * (g - c).dot(d);
    // The directions of the two edges (towards the side where the points lie), as angles in the
    // reference frame of the final axis.
    let (u, v) = axis_reference(axis_dir);
    let edge_angle = |theta: f64, set: &[DVec3]| -> f64 {
        let mut along = f1 * theta.cos() + f2 * theta.sin();
        let mean: f64 = set.iter().map(|p| along.dot(*p - c)).sum::<f64>() / set.len() as f64;
        if mean < 0.0 {
            along = -along;
        }
        angle_in(along, u, v)
    };
    let (theta_a, theta_b) = (edge_angle(full[4], a), edge_angle(full[5], b));
    let span_ab = (theta_b - theta_a).rem_euclid(TAU);
    if !(1e-6..=TAU - 1e-6).contains(&span_ab) {
        return Err(ZoneFitError::Degenerate("the two edges coincide"));
    }
    let (from, span) = hint.map_or_else(
        || {
            if span_ab <= std::f64::consts::PI {
                (theta_a, span_ab)
            } else {
                (theta_b, TAU - span_ab)
            }
        },
        |h| {
            let rel = h - axis_point;
            let theta_h = angle_in(rel, u, v);
            if (theta_h - theta_a).rem_euclid(TAU) <= span_ab {
                (theta_a, span_ab)
            } else {
                (theta_b, TAU - span_ab)
            }
        },
    );
    Ok(SectorFit {
        axis_point,
        axis_dir,
        angle_from: from,
        angle_to: from + span,
        residuals: res,
    })
}

fn sector_distance(shape: &ZoneShape, p: DVec3) -> f64 {
    match shape {
        ZoneShape::Sector {
            axis_point,
            axis_dir,
            angle_from,
            angle_to,
        } => {
            let (u, v) = axis_reference(*axis_dir);
            let q = p - *axis_point;
            let plane = |theta: f64| (v * theta.cos() - u * theta.sin()).dot(q);
            let (d1, d2) = (plane(*angle_from), plane(*angle_to));
            if d1.abs() <= d2.abs() { d1 } else { d2 }
        }
        _ => f64::NAN,
    }
}

/// Fits a wedge (ametrine, trapiche sector): an axis and two edge half-planes, from one point set
/// on each edge.
///
/// `fixed_axis` pins the axis direction (needed when the two edges are nearly coplanar).
/// `inside_hint`, a point inside the wedge, picks the wedge when the edges bound two (otherwise
/// the one spanning at most half a turn). The residuals are the plane distances of the first
/// set's points, then the second's.
///
/// # Errors
///
/// [`ZoneFitError`] for fewer than 3 points in a set, parallel edge planes without a fixed axis,
/// coincident edges, or a diverged fit.
pub fn fit_sector(
    edge_a: &BoundaryPoints,
    edge_b: &BoundaryPoints,
    fixed_axis: Option<DVec3>,
    inside_hint: Option<DVec3>,
) -> Result<FittedZoneShape, ZoneFitError> {
    let main = sector_core(&edge_a.points, &edge_b.points, fixed_axis, inside_hint)?;
    let shape_of = |f: &SectorFit| ZoneShape::Sector {
        axis_point: f.axis_point,
        axis_dir: f.axis_dir,
        angle_from: f.angle_from,
        angle_to: f.angle_to,
    };
    let fit = move |sets: &[Vec<DVec3>]| -> Option<ZoneShape> {
        sector_core(&sets[0], &sets[1], fixed_axis, inside_hint)
            .ok()
            .map(|f| shape_of(&f))
    };
    let measure = |shape: &ZoneShape, sets: &[Vec<DVec3>]| -> f64 {
        let r: Vec<f64> = sets
            .iter()
            .flatten()
            .map(|p| sector_distance(shape, *p))
            .collect();
        rms(&r)
    };
    let lovo = lovo_rms(&[edge_a, edge_b], &fit, &measure);
    let shape = shape_of(&main);
    Ok(finish(shape, main.residuals, lovo))
}
