//! Rig calibration from a beam-splitter cube of known size.
//!
//! The reference object is a beam-splitter cube (default: edge 25.4 mm, tolerance 0.1 mm, glass
//! N-BK7 with `n_d` 1.5168). It stands at the rig origin with its faces along the rig axes; the
//! corner with the orientation mark (a paint dot or tape) is the corner at `+X +Y +Z`.
//!
//! # Pass 1: outer edges
//!
//! The cube's outer edges, not a filled silhouette: a clear cube on a background gives weak
//! silhouettes, so backlighting or a dark field is recommended to make the edges sharp lines. In
//! every photo the user (or an edge detector) supplies the visible outer edges as line segments
//! ([`EdgeLine`]). Each line is matched to the nearest cube edge under the current pose estimate
//! (only edges that lie on a face turned toward the camera are candidates, since the others are
//! seen through the glass), and then the poses, the focal lengths and the cube's scale are fitted
//! together by Levenberg-Marquardt on the distance of the projected edge ends from the observed
//! lines.
//!
//! Camera positions, focal lengths and the cube's scale cannot all be recovered from images alone
//! (the image scale is `focal * edge / distance`), so the fit carries priors: each camera position
//! is held to the profile's value within [`CalibrationOptions::position_sigma_mm`], each focal
//! length within [`CalibrationOptions::focal_sigma`], and the cube's edge to its datasheet value
//! within its tolerance. The fitted scale is reported against the datasheet; it is as good as
//! those priors, so a tight focal prior makes it a real check of the cube's size.
//!
//! The orientation mark is a consistency check: the corner it names must land near the click in
//! every view, or the cube was turned.
//!
//! # Pass 2: the coated diagonal, seen through the faces
//!
//! The diagonal plane of the cube is a known internal plane. Its edges lie on the cube's surface,
//! but a camera on the far side of the glass sees them THROUGH the faces, refracted. Each such
//! edge is photographed with a few clicks along it ([`DiagonalLine`]); the refractive rays of all
//! its clicks, from every view that sees it through the glass, are fitted by one 3D line, which is
//! compared with the true edge and the true plane. The residual is the rig's measured accuracy:
//! it contains the pose errors, the focal lengths, the glass index and the refraction model all
//! at once.

use std::{collections::BTreeSet, fmt};

use glam::{DQuat, DVec2, DVec3};
use serde::{Deserialize, Serialize};

use super::{
    rig::{Projection, RigError, RigProfile, Rigid, ViewPose},
    shapes::box_mesh,
    solve::levenberg_marquardt,
    trace::{Scene, trace_pixel},
    triangulate::Line,
};
use crate::rough_plan::shape::{MeshError, RoughMesh};

/// Parameters per view: position (3), rotation vector (3), relative focal change (1).
const PER_VIEW: usize = 7;

/// The corner of the cube that carries the orientation mark: `+X +Y +Z`.
const MARK_CORNER: u8 = 7;

/// The residual of an edge end that cannot be projected, in pixels.
const BEHIND_PX: f64 = 1.0e4;

/// The 12 edges of the cube as pairs of corner indices (bit 0 = x, bit 1 = y, bit 2 = z, set
/// meaning the positive side): four along x, four along y, four along z.
pub(super) const CUBE_EDGES: [(u8, u8); 12] = [
    (0, 1),
    (2, 3),
    (4, 5),
    (6, 7),
    (0, 2),
    (1, 3),
    (4, 6),
    (5, 7),
    (0, 4),
    (1, 5),
    (2, 6),
    (3, 7),
];

/// The reference cube.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CubeSpec {
    /// The datasheet edge length in mm.
    pub edge_mm: f64,
    /// The datasheet tolerance of the edge length in mm.
    pub tolerance_mm: f64,
    /// The refractive index `n_d` of the glass (N-BK7: 1.5168).
    pub n_d: f64,
    /// The rig axis (0 = x, 1 = y, 2 = z) the coated diagonal plane is parallel to (default 2).
    pub diagonal_axis: u8,
    /// Whether the diagonal runs the other way, `c = -b` instead of `c = b`, where `b` and `c`
    /// are the two other axes in cyclic order (default `false`).
    pub mirrored: bool,
}

impl Default for CubeSpec {
    fn default() -> Self {
        Self {
            edge_mm: 25.4,
            tolerance_mm: 0.1,
            n_d: 1.5168,
            diagonal_axis: 2,
            mirrored: false,
        }
    }
}

const fn axis_vec(axis: u8) -> DVec3 {
    match axis {
        0 => DVec3::X,
        1 => DVec3::Y,
        _ => DVec3::Z,
    }
}

/// The corner `index` of a cube of edge `edge_mm` centred at the origin.
pub(super) fn cube_corner(index: u8, edge_mm: f64) -> DVec3 {
    let half = edge_mm * 0.5;
    let side = |bit: u8| {
        if ((index >> bit) & 1) == 1 {
            half
        } else {
            -half
        }
    };
    DVec3::new(side(0), side(1), side(2))
}

/// The outward normals of the two faces that meet at `edge`.
fn edge_face_normals(edge: (u8, u8)) -> [DVec3; 2] {
    let differing = edge.0 ^ edge.1;
    let mut normals = [DVec3::ZERO; 2];
    let mut slot = 0;
    for axis in 0..3_u8 {
        if (differing & (1 << axis)) == 0 && slot < 2 {
            let sign = if ((edge.0 >> axis) & 1) == 1 {
                1.0
            } else {
                -1.0
            };
            normals[slot] = axis_vec(axis) * sign;
            slot += 1;
        }
    }
    normals
}

impl CubeSpec {
    /// The four corners of the coated diagonal (a rectangle), for a cube of edge `edge_mm`.
    ///
    /// Corner 0 and 1 on one cube edge (parallel to the diagonal axis), 2 and 3 on the opposite
    /// one. The diagonal's four edges run 0 to 1, 1 to 2, 2 to 3 and 3 to 0.
    #[must_use]
    pub fn diagonal_corners(&self, edge_mm: f64) -> [DVec3; 4] {
        let half = edge_mm * 0.5;
        let along = self.diagonal_axis % 3;
        let (first, second) = ((along + 1) % 3, (along + 2) % 3);
        let twist = if self.mirrored { -1.0 } else { 1.0 };
        let at = |along_value: f64, first_value: f64| {
            let mut coords = [0.0; 3];
            coords[usize::from(along)] = along_value;
            coords[usize::from(first)] = first_value;
            coords[usize::from(second)] = first_value * twist;
            DVec3::from_array(coords)
        };
        [
            at(half, half),
            at(-half, half),
            at(-half, -half),
            at(half, -half),
        ]
    }

    /// The unit normal of the coated diagonal plane.
    #[must_use]
    pub fn diagonal_normal(&self) -> DVec3 {
        let along = self.diagonal_axis % 3;
        let twist = if self.mirrored { -1.0 } else { 1.0 };
        (axis_vec((along + 1) % 3) - axis_vec((along + 2) % 3) * twist).normalize()
    }

    /// A cube as a mesh, centred at the origin, with edge `edge_mm`.
    ///
    /// # Errors
    ///
    /// [`MeshError`] for an edge that is not positive and finite.
    pub fn mesh(edge_mm: f64) -> Result<RoughMesh, MeshError> {
        box_mesh(DVec3::splat(edge_mm * 0.5))
    }

    const fn check(&self) -> Result<(), CalibrationError> {
        let usable = self.edge_mm.is_finite()
            && self.edge_mm > 0.0
            && self.tolerance_mm.is_finite()
            && self.tolerance_mm >= 0.0
            && self.n_d.is_finite()
            && self.n_d >= 1.0;
        if usable {
            Ok(())
        } else {
            Err(CalibrationError::BadCube)
        }
    }
}

/// One visible outer edge of the cube in a photo, as two points of the line the edge makes.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct EdgeLine {
    /// One end, in pixels.
    pub a: [f64; 2],
    /// The other end, in pixels.
    pub b: [f64; 2],
}

/// What the user marked in one photo of the cube.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ViewObservations {
    /// The view the photo belongs to.
    pub view: usize,
    /// The visible outer edges of the cube.
    pub lines: Vec<EdgeLine>,
    /// The click on the orientation mark, if any.
    pub mark: Option<[f64; 2]>,
}

/// Clicks along one edge of the coated diagonal as seen in one photo.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiagonalLine {
    /// Which diagonal edge: 0 to 3, see [`CubeSpec::diagonal_corners`].
    pub edge: u8,
    /// The view the photo belongs to.
    pub view: usize,
    /// Points clicked along the edge (they need not match between views).
    pub pixels: Vec<[f64; 2]>,
}

/// Tuning of the calibration.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CalibrationOptions {
    /// The expected noise of an edge line, in pixels (default 0.5).
    pub edge_sigma_px: f64,
    /// How well the camera positions of the profile are known, in mm (default 0.5).
    pub position_sigma_mm: f64,
    /// How well the focal lengths of the profile are known, as a fraction (default 0.1).
    ///
    /// The image scale is `focal * edge / distance`, so the cube's size and the focal lengths are
    /// confounded: the fitted scale is only as good as this prior and the position prior. Set it
    /// small when the lens's focal length is known.
    pub focal_sigma: f64,
    /// A line farther than this from every candidate edge is rejected, in pixels (default 80).
    pub gate_px: f64,
    /// How often the line-to-edge matching is redone with the improved poses (default 3).
    pub rounds: usize,
    /// The most Levenberg-Marquardt iterations per round (default 50).
    pub max_iterations: usize,
    /// The largest distance, in pixels, of the orientation mark from its predicted place before
    /// a warning is given (default 30).
    pub mark_gate_px: f64,
}

impl Default for CalibrationOptions {
    fn default() -> Self {
        Self {
            edge_sigma_px: 0.5,
            position_sigma_mm: 0.5,
            focal_sigma: 0.1,
            gate_px: 80.0,
            rounds: 3,
            max_iterations: 50,
            mark_gate_px: 30.0,
        }
    }
}

/// What the calibration measured; it is stored with the rig profile.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CalibrationResult {
    /// The datasheet edge length in mm.
    pub datasheet_edge_mm: f64,
    /// The datasheet tolerance in mm.
    pub tolerance_mm: f64,
    /// The glass index the diagonal check used.
    pub cube_n_d: f64,
    /// The fitted edge length in mm.
    pub fitted_edge_mm: f64,
    /// The fitted edge divided by the datasheet edge (1.004 means 0.4 % larger).
    pub scale_ratio: f64,
    /// Whether the fitted edge is within the datasheet tolerance.
    pub scale_within_tolerance: bool,
    /// The RMS distance of the observed edge lines from the fitted cube, in pixels.
    pub edge_rms_px: f64,
    /// How many edge lines were matched.
    pub lines_used: usize,
    /// The RMS distance of the triangulated diagonal edges from the true ones, in mm: the rig's
    /// measured accuracy. `None` when pass 2 was not run.
    pub diagonal_rms_mm: Option<f64>,
    /// The RMS distance of the triangulated diagonal edges from the true diagonal plane, in mm.
    pub diagonal_plane_rms_mm: Option<f64>,
}

/// Why a calibration could not be run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalibrationError {
    /// The rig profile is unusable.
    Rig(RigError),
    /// The cube's size or index is not usable.
    BadCube,
    /// An observation names a view the rig lacks.
    NoSuchView(usize),
    /// No edge line could be matched to a cube edge.
    NoObservations,
    /// The cube mesh could not be built.
    Mesh(MeshError),
    /// No diagonal edge was seen through the glass by two views.
    NoDiagonalEdges,
}

impl fmt::Display for CalibrationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Rig(error) => write!(f, "{error}"),
            Self::BadCube => write!(f, "the cube's edge, tolerance or glass index is not usable"),
            Self::NoSuchView(view) => write!(f, "view {} does not exist", view + 1),
            Self::NoObservations => write!(f, "no edge line could be matched to a cube edge"),
            Self::Mesh(error) => write!(f, "{error}"),
            Self::NoDiagonalEdges => write!(
                f,
                "no edge of the coated diagonal is seen through the glass by two views"
            ),
        }
    }
}

impl std::error::Error for CalibrationError {}

/// The deviation of one triangulated diagonal edge from the true one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EdgeDeviation {
    /// Which diagonal edge (0 to 3).
    pub edge: u8,
    /// How many views saw it through the glass.
    pub views: usize,
    /// How many rays were used.
    pub rays: usize,
    /// The distance of each true end from the triangulated line, in mm.
    pub end_deviation_mm: [f64; 2],
    /// The distance of the triangulated line's points nearest the true ends from the true
    /// diagonal plane, in mm.
    pub end_plane_mm: [f64; 2],
}

/// The result of pass 2.
#[derive(Debug, Clone, PartialEq)]
pub struct DiagonalCheck {
    /// The edges that were triangulated.
    pub edges: Vec<EdgeDeviation>,
    /// The RMS of all the end deviations, in mm.
    pub deviation_rms_mm: f64,
    /// The RMS of all the distances from the true plane, in mm.
    pub plane_rms_mm: f64,
    /// Why some edges were left out.
    pub skipped: Vec<String>,
}

/// The outcome of a calibration.
#[derive(Debug, Clone, PartialEq)]
pub struct Calibrated {
    /// The rig with refined poses and focal lengths; its `calibration` holds the result.
    pub rig: RigProfile,
    /// What was measured (also in `rig.calibration`).
    pub result: CalibrationResult,
    /// The RMS edge-line misfit of every view, in pixels (`None` for a view without lines).
    pub view_rms_px: Vec<Option<f64>>,
    /// How far each view's orientation mark is from the corner it should name, in pixels.
    pub mark_error_px: Vec<Option<f64>>,
    /// Pass 2, when it was run.
    pub diagonal: Option<DiagonalCheck>,
    /// Half the weighted sum of squared residuals at the optimum of pass 1.
    pub fit_cost: f64,
    /// How many Levenberg-Marquardt iterations pass 1 took.
    pub iterations: usize,
    /// Things the user should look at: rejected lines, a mark that does not fit.
    pub warnings: Vec<String>,
}

/// Whether a face with outward `normal` turns toward the camera of `pose`; `half` is half the
/// cube's edge.
fn faces_camera(pose: &ViewPose, normal: DVec3, half: f64) -> bool {
    match pose.projection {
        Projection::Pinhole { .. } => normal.dot(pose.position_vec()) > half,
        Projection::Orthographic { .. } => normal.dot(pose.basis().forward) < 0.0,
    }
}

/// The cube edges that lie on a face turned toward the camera: those are seen directly.
pub(super) fn visible_edges(pose: &ViewPose, edge_mm: f64) -> Vec<usize> {
    let half = edge_mm * 0.5;
    (0..CUBE_EDGES.len())
        .filter(|&edge| {
            edge_face_normals(CUBE_EDGES[edge])
                .iter()
                .any(|&normal| faces_camera(pose, normal, half))
        })
        .collect()
}

/// Matches every observed line to the visible cube edge nearest to it; returns the matches and
/// how many lines were rejected.
fn associate(
    pose: &ViewPose,
    edge_mm: f64,
    lines: &[EdgeLine],
    gate_px: f64,
) -> (Vec<(usize, EdgeLine)>, usize) {
    let visible = visible_edges(pose, edge_mm);
    let mut matches = Vec::new();
    let mut rejected = 0;
    for line in lines {
        let (end_a, end_b) = (DVec2::from_array(line.a), DVec2::from_array(line.b));
        let mut best: Option<(f64, usize)> = None;
        for &edge in &visible {
            let (corner_0, corner_1) = CUBE_EDGES[edge];
            let (Some(from), Some(to)) = (
                pose.project(cube_corner(corner_0, edge_mm)),
                pose.project(cube_corner(corner_1, edge_mm)),
            ) else {
                continue;
            };
            let direction = (to - from).normalize_or_zero();
            if direction == DVec2::ZERO {
                continue;
            }
            let normal = direction.perp();
            let cost = normal.dot(end_a - from).abs() + normal.dot(end_b - from).abs();
            if best.is_none_or(|(least, _)| cost < least) {
                best = Some((cost, edge));
            }
        }
        match best {
            Some((cost, edge)) if cost <= 2.0 * gate_px => matches.push((edge, *line)),
            _ => rejected += 1,
        }
    }
    (matches, rejected)
}

/// The pose of `base` moved by the seven parameters `block` (position, rotation vector, relative
/// change of the focal length).
fn adjusted_pose(base: &ViewPose, block: &[f64]) -> ViewPose {
    let rotation = DQuat::from_scaled_axis(DVec3::new(block[3], block[4], block[5]));
    let mut pose = base.clone();
    pose.position =
        (DVec3::from_array(base.position) + DVec3::new(block[0], block[1], block[2])).to_array();
    pose.forward = (rotation * DVec3::from_array(base.forward)).to_array();
    pose.up = (rotation * DVec3::from_array(base.up)).to_array();
    pose.projection = base
        .projection
        .with_scale(base.projection.scale_px() * (1.0 + block[6]));
    pose
}

/// Appends the signed distances of the projected ends of every matched edge from its observed
/// line, each multiplied by `weight`.
fn append_edge_residuals(
    pose: &ViewPose,
    edge_mm: f64,
    matches: &[(usize, EdgeLine)],
    weight: f64,
    out: &mut Vec<f64>,
) {
    for &(edge, line) in matches {
        let anchor = DVec2::from_array(line.a);
        let normal = (DVec2::from_array(line.b) - anchor)
            .normalize_or_zero()
            .perp();
        let (corner_0, corner_1) = CUBE_EDGES[edge];
        for corner in <[u8; 2]>::from((corner_0, corner_1)) {
            let value = pose
                .project(cube_corner(corner, edge_mm))
                .map_or(BEHIND_PX, |pixel| normal.dot(pixel - anchor));
            out.push(value * weight);
        }
    }
}

/// What the pass-1 residuals need besides the parameters.
struct FitContext<'a> {
    views: &'a [ViewPose],
    matches: &'a [Vec<(usize, EdgeLine)>],
    cube: &'a CubeSpec,
    options: &'a CalibrationOptions,
}

/// The weighted residuals of pass 1: matched edge ends, then the priors.
fn pass1_residuals(params: &[f64], ctx: &FitContext<'_>) -> Vec<f64> {
    let scale_index = ctx.views.len() * PER_VIEW;
    let edge_mm = ctx.cube.edge_mm * (1.0 + params[scale_index]);
    let weight = 1.0 / ctx.options.edge_sigma_px;
    let mut out = Vec::new();
    for (index, base) in ctx.views.iter().enumerate() {
        let block = &params[index * PER_VIEW..(index + 1) * PER_VIEW];
        let pose = adjusted_pose(base, block);
        append_edge_residuals(&pose, edge_mm, &ctx.matches[index], weight, &mut out);
        out.extend(
            block[..3]
                .iter()
                .map(|shift| shift / ctx.options.position_sigma_mm),
        );
        out.push(block[6] / ctx.options.focal_sigma);
    }
    out.push(params[scale_index] * ctx.cube.edge_mm / ctx.cube.tolerance_mm.max(1e-3));
    out
}

/// The finite-difference step of parameter `index`.
const fn step_of(index: usize, scale_index: usize) -> f64 {
    if index >= scale_index {
        return 1e-6;
    }
    if index % PER_VIEW < 3 { 1e-4 } else { 1e-6 }
}

/// The poses of every view for the parameter vector.
fn poses_of(views: &[ViewPose], params: &[f64]) -> Vec<ViewPose> {
    views
        .iter()
        .enumerate()
        .map(|(index, base)| adjusted_pose(base, &params[index * PER_VIEW..(index + 1) * PER_VIEW]))
        .collect()
}

/// Matches the lines of every view under the poses `poses` and the edge `edge_mm`.
fn associate_all(
    poses: &[ViewPose],
    edge_mm: f64,
    lines: &[Vec<EdgeLine>],
    gate_px: f64,
) -> (Vec<Vec<(usize, EdgeLine)>>, usize) {
    let mut matches = Vec::with_capacity(poses.len());
    let mut rejected = 0;
    for (pose, own) in poses.iter().zip(lines) {
        let (found, dropped) = associate(pose, edge_mm, own, gate_px);
        matches.push(found);
        rejected += dropped;
    }
    (matches, rejected)
}

/// Pass 1: fits the poses, the focal lengths and the cube's scale from the outer edges.
///
/// The cube stands at the rig origin with its faces along the rig axes. The returned rig has
/// the refined poses and focal lengths and carries the [`CalibrationResult`] (without pass 2).
///
/// # Errors
///
/// [`CalibrationError`] for an unusable rig or cube, an observation of a view the rig lacks, or
/// when no line could be matched to an edge.
pub fn calibrate_edges(
    rig: &RigProfile,
    cube: &CubeSpec,
    observations: &[ViewObservations],
    options: &CalibrationOptions,
) -> Result<Calibrated, CalibrationError> {
    rig.validate().map_err(CalibrationError::Rig)?;
    cube.check()?;
    let view_count = rig.views.len();
    let mut lines: Vec<Vec<EdgeLine>> = vec![Vec::new(); view_count];
    let mut marks: Vec<Option<[f64; 2]>> = vec![None; view_count];
    for item in observations {
        if item.view >= view_count {
            return Err(CalibrationError::NoSuchView(item.view));
        }
        lines[item.view].extend(item.lines.iter().copied());
        if item.mark.is_some() {
            marks[item.view] = item.mark;
        }
    }
    let scale_index = view_count * PER_VIEW;
    let mut params = vec![0.0; scale_index + 1];
    let steps: Vec<f64> = (0..params.len())
        .map(|index| step_of(index, scale_index))
        .collect();
    let (mut iterations, mut fit_cost) = (0, 0.0);
    for _ in 0..options.rounds.max(1) {
        let poses = poses_of(&rig.views, &params);
        let edge_mm = cube.edge_mm * (1.0 + params[scale_index]);
        let (matches, _) = associate_all(&poses, edge_mm, &lines, options.gate_px);
        if matches.iter().all(Vec::is_empty) {
            return Err(CalibrationError::NoObservations);
        }
        let ctx = FitContext {
            views: &rig.views,
            matches: &matches,
            cube,
            options,
        };
        let fit = levenberg_marquardt(&params, &steps, options.max_iterations, &|p| {
            pass1_residuals(p, &ctx)
        });
        params = fit.params;
        iterations += fit.iterations;
        fit_cost = fit.cost;
    }
    let state = Pass1State {
        rig,
        cube,
        options,
        lines: &lines,
        marks: &marks,
    };
    Ok(finish_pass1(&state, &params, iterations, fit_cost))
}

/// The inputs of one pass-1 calibration, for building its outcome.
struct Pass1State<'a> {
    rig: &'a RigProfile,
    cube: &'a CubeSpec,
    options: &'a CalibrationOptions,
    lines: &'a [Vec<EdgeLine>],
    marks: &'a [Option<[f64; 2]>],
}

/// Builds the pass-1 outcome from the fitted parameters.
fn finish_pass1(
    state: &Pass1State<'_>,
    params: &[f64],
    iterations: usize,
    fit_cost: f64,
) -> Calibrated {
    let (rig, cube, options) = (state.rig, state.cube, state.options);
    let scale_index = rig.views.len() * PER_VIEW;
    let poses = poses_of(&rig.views, params);
    let fitted_edge_mm = cube.edge_mm * (1.0 + params[scale_index]);
    let (matches, rejected) = associate_all(&poses, fitted_edge_mm, state.lines, options.gate_px);
    let mut warnings = Vec::new();
    if rejected > 0 {
        warnings.push(format!(
            "{rejected} edge line(s) were too far from every cube edge and were left out"
        ));
    }
    let mut view_rms_px = Vec::with_capacity(poses.len());
    let (mut sum_sq, mut count) = (0.0, 0_usize);
    for (pose, own) in poses.iter().zip(&matches) {
        let mut values = Vec::new();
        append_edge_residuals(pose, fitted_edge_mm, own, 1.0, &mut values);
        let squares: f64 = values.iter().map(|v| v * v).sum();
        sum_sq += squares;
        count += values.len();
        view_rms_px.push((!values.is_empty()).then(|| (squares / values.len() as f64).sqrt()));
    }
    let mut mark_error_px = Vec::with_capacity(poses.len());
    for (index, (pose, mark)) in poses.iter().zip(state.marks).enumerate() {
        let error = mark.and_then(|click| {
            let corner = cube_corner(MARK_CORNER, fitted_edge_mm);
            pose.project(corner)
                .map(|pixel| (pixel - DVec2::from_array(click)).length())
        });
        if error.is_some_and(|px| px > options.mark_gate_px) {
            warnings.push(format!(
                "the orientation mark in view {} is far from the corner it should name: the cube may be turned",
                index + 1
            ));
        }
        mark_error_px.push(error);
    }
    let result = CalibrationResult {
        datasheet_edge_mm: cube.edge_mm,
        tolerance_mm: cube.tolerance_mm,
        cube_n_d: cube.n_d,
        fitted_edge_mm,
        scale_ratio: fitted_edge_mm / cube.edge_mm,
        scale_within_tolerance: (fitted_edge_mm - cube.edge_mm).abs() <= cube.tolerance_mm,
        edge_rms_px: if count > 0 {
            (sum_sq / count as f64).sqrt()
        } else {
            0.0
        },
        lines_used: matches.iter().map(Vec::len).sum(),
        diagonal_rms_mm: None,
        diagonal_plane_rms_mm: None,
    };
    let mut refined = rig.clone();
    refined.views = poses;
    refined.calibration = Some(result.clone());
    Calibrated {
        rig: refined,
        result,
        view_rms_px,
        mark_error_px,
        diagonal: None,
        fit_cost,
        iterations,
        warnings,
    }
}

/// Whether `point` (mesh frame) is seen through the glass from `view`: the straight line of sight
/// to it meets the surface before it reaches it.
pub(super) fn seen_through(scene: &Scene<'_>, view: usize, point: DVec3) -> bool {
    let Some(pose) = scene.rig.views.get(view) else {
        return false;
    };
    let Some(pixel) = pose.project(scene.alignment.to_rig(point)) else {
        return false;
    };
    let Some((origin, dir)) = scene.camera_ray(view, pixel) else {
        return false;
    };
    let along = (point - origin).dot(dir);
    let slack = 1e-6 * scene.mesh_scale();
    scene
        .mesh
        .first_hit(origin, dir, 0.0)
        .is_some_and(|hit| hit.t < along - slack)
}

/// The signed gap between a ray and the line through `point` along `along`.
fn line_gap(ray: &Line, point: DVec3, along: DVec3) -> f64 {
    let cross = ray.dir.cross(along);
    let length = cross.length();
    let rel = ray.origin - point;
    if length < 1e-9 {
        (rel - along * rel.dot(along)).length()
    } else {
        rel.dot(cross) / length
    }
}

/// How far the line fit starts from the nominal edge, in mm.
///
/// The two ends are moved in opposite directions, so the start is off in position and in direction, and the result shows what the
/// rays say, not where the fit began.
const START_OFFSET_MM: DVec3 = DVec3::new(0.6, -0.5, 0.4);

/// Fits the line that every ray meets as closely as possible.
///
/// It starts from the nominal edge `start_a` to `start_b` moved by [`START_OFFSET_MM`]. Returns two points of the fitted line.
fn fit_line(rays: &[Line], start_a: DVec3, start_b: DVec3) -> (DVec3, DVec3) {
    let (start_a, start_b) = (start_a + START_OFFSET_MM, start_b - START_OFFSET_MM);
    let begin = [
        start_a.x, start_a.y, start_a.z, start_b.x, start_b.y, start_b.z,
    ];
    let fit = levenberg_marquardt(&begin, &[1e-4; 6], 80, &|p| {
        let first = DVec3::new(p[0], p[1], p[2]);
        let second = DVec3::new(p[3], p[4], p[5]);
        let along = (second - first).normalize_or_zero();
        rays.iter().map(|ray| line_gap(ray, first, along)).collect()
    });
    (
        DVec3::new(fit.params[0], fit.params[1], fit.params[2]),
        DVec3::new(fit.params[3], fit.params[4], fit.params[5]),
    )
}

/// The rays of the clicks along diagonal edge `edge` from every view that sees its middle `middle`
/// through the glass, and the views that contributed. Views that do not are noted in `skipped`.
fn gather_edge_rays(
    scene: &Scene<'_>,
    lines: &[DiagonalLine],
    edge: u8,
    middle: DVec3,
    skipped: &mut Vec<String>,
) -> (Vec<Line>, BTreeSet<usize>) {
    let mut rays = Vec::new();
    let mut views = BTreeSet::new();
    for item in lines.iter().filter(|item| item.edge == edge) {
        if !seen_through(scene, item.view, middle) {
            skipped.push(format!(
                "diagonal edge {} in view {} is not seen through the glass",
                edge + 1,
                item.view + 1
            ));
            continue;
        }
        let before = rays.len();
        rays.extend(item.pixels.iter().filter_map(|&pixel| {
            let path = trace_pixel(scene, item.view, DVec2::from_array(pixel), 0).ok()?;
            let leg = path.legs.first().copied()?;
            Some(Line {
                origin: leg.from,
                dir: leg.dir(),
            })
        }));
        if rays.len() > before {
            views.insert(item.view);
        }
    }
    (rays, views)
}

/// Pass 2: triangulates the edges of the coated diagonal through the glass with the refractive
/// solver and compares them with the true edges and plane.
///
/// `rig` is the refined rig of pass 1 and `edge_mm` the fitted edge. An edge is triangulated
/// from the clicks of the views that see it through the glass (the straight line of sight to its
/// middle meets a face first) and needs at least two such views.
///
/// # Errors
///
/// [`CalibrationError`] for an unusable rig or cube, or when no diagonal edge was seen through
/// the glass by two views.
pub fn check_diagonal(
    rig: &RigProfile,
    cube: &CubeSpec,
    edge_mm: f64,
    lines: &[DiagonalLine],
) -> Result<DiagonalCheck, CalibrationError> {
    rig.validate().map_err(CalibrationError::Rig)?;
    cube.check()?;
    let mesh = CubeSpec::mesh(edge_mm).map_err(CalibrationError::Mesh)?;
    let mut glass = rig.clone();
    glass.stone_n = cube.n_d;
    let scene = Scene::new(&mesh, &glass, Rigid::IDENTITY);
    let corners = cube.diagonal_corners(edge_mm);
    let (plane_point, plane_normal) = (corners[0], cube.diagonal_normal());
    let mut edges = Vec::new();
    let mut skipped = Vec::new();
    for edge in 0..4_u8 {
        let (end_a, end_b) = (
            corners[usize::from(edge)],
            corners[(usize::from(edge) + 1) % 4],
        );
        let middle = end_a.midpoint(end_b);
        let (rays, views) = gather_edge_rays(&scene, lines, edge, middle, &mut skipped);
        if views.len() < 2 || rays.len() < 4 {
            skipped.push(format!(
                "diagonal edge {} has fewer than two views through the glass",
                edge + 1
            ));
            continue;
        }
        let (fit_a, fit_b) = fit_line(&rays, end_a, end_b);
        let along = (fit_b - fit_a).normalize_or_zero();
        let foot = |truth: DVec3| fit_a + along * (truth - fit_a).dot(along);
        let ends = [end_a, end_b].map(foot);
        edges.push(EdgeDeviation {
            edge,
            views: views.len(),
            rays: rays.len(),
            end_deviation_mm: [(ends[0] - end_a).length(), (ends[1] - end_b).length()],
            end_plane_mm: ends.map(|end| plane_normal.dot(end - plane_point).abs()),
        });
    }
    if edges.is_empty() {
        return Err(CalibrationError::NoDiagonalEdges);
    }
    let rms =
        |values: Vec<f64>| (values.iter().map(|v| v * v).sum::<f64>() / values.len() as f64).sqrt();
    Ok(DiagonalCheck {
        deviation_rms_mm: rms(edges.iter().flat_map(|e| e.end_deviation_mm).collect()),
        plane_rms_mm: rms(edges.iter().flat_map(|e| e.end_plane_mm).collect()),
        edges,
        skipped,
    })
}

/// The whole calibration: pass 1 on the outer edges, then pass 2 on the coated diagonal.
///
/// Pass 2 runs when `diagonal` has clicks. The measured accuracy is stored in the returned rig's `calibration`.
///
/// # Errors
///
/// As [`calibrate_edges`] and [`check_diagonal`].
pub fn calibrate_rig(
    rig: &RigProfile,
    cube: &CubeSpec,
    observations: &[ViewObservations],
    diagonal: &[DiagonalLine],
    options: &CalibrationOptions,
) -> Result<Calibrated, CalibrationError> {
    let mut done = calibrate_edges(rig, cube, observations, options)?;
    if !diagonal.is_empty() {
        let check = check_diagonal(&done.rig, cube, done.result.fitted_edge_mm, diagonal)?;
        done.result.diagonal_rms_mm = Some(check.deviation_rms_mm);
        done.result.diagonal_plane_rms_mm = Some(check.plane_rms_mm);
        done.rig.calibration = Some(done.result.clone());
        done.diagonal = Some(check);
    }
    Ok(done)
}
