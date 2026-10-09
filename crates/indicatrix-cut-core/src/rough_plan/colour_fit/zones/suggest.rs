//! Zone suggestions from the residual of a fit or from the transmittance images (plan section
//! 7.3, step 3).
//!
//! Assistance only: the result is a list of candidate shapes with scores, never
//! applied to anything.
//!
//! # Pipeline (deterministic)
//!
//! 1. **Segmentation.** The valid pixels of all views are pooled and clustered by k-means:
//!    transmittance images in CIELAB (linear camera RGB read as linear sRGB, D65), residual maps
//!    on the whitened channel vector. The seeds are quantiles of the pixels sorted by `L` (first
//!    channel); Lloyd iterations run in pixel order. With `k` unset, `k = 2..=4` are tried and the
//!    smallest `k` is kept that explains all but 5 % of the variance, or after which the inertia
//!    no longer drops below 70 % of the previous one (an elbow rule). Clusters are numbered by the first channel of their centre, ascending
//!    (label 0 is the darkest in Lab).
//! 2. **Smoothing.** Iterated conditional modes with a Potts prior, a fixed number of raster
//!    sweeps: each pixel takes the label that minimises `|x - mu|^2 / (2 sigma^2) + beta * (number
//!    of 4-neighbours with another label)`.
//! 3. **Back-projection.** Between two 4-neighbouring pixels with different labels lies a
//!    boundary sample (the midpoint of their shared edge). Its camera ray is traced into the
//!    stone with `locate::trace_pixel`; the candidate 3D point is where the ray enters the surface
//!    ([`BackProjection::SurfaceEntry`]) or the middle of its first interior leg
//!    ([`BackProjection::LegMidpoint`]).
//! 4. **Fitting.** The points of each pair of labels are fitted with a half space, a cylinder
//!    and prisms ([`fit_half_space`] and friends), once, then again without the points farther
//!    from the fit than `max(2.5 tol, 2 rms)`. The zone is the side of the boundary where the
//!    label with the smaller number lies (the darker cluster of a Lab segmentation); the
//!    orientation is taken from the traced surface points of that label.
//!
//! # Limits (also in the manual)
//!
//! The surface-entry rule is exact for a boundary seen edge-on (a plane that contains the viewing
//! direction) and biased otherwise: the colour step in the photo is where rays cross the boundary
//! INSIDE the stone, which the entry point does not know. The points of different views then
//! disagree and the fit residual shows it; the suggestion is a starting point for the manual
//! marking, not a result. Candidates with a large residual get a small score.

use std::collections::BTreeMap;

use glam::{DVec2, DVec3};
use indicatrix::{color::body_color::xyz_to_lab, optics::zoning::ZoneShape};

use super::fit::{
    BoundaryPoints, FittedZoneShape, RadialRole, ZoneFitError, fit_cylinder, fit_half_space,
    fit_prism,
};
use crate::rough_plan::{
    colour_fit::{
        forward::ViewPrediction,
        solve::{ObservedView, ResidualMap},
    },
    locate::{Projection, Scene, trace_pixel},
    photometry::WorkingGrid,
};

/// The label of a pixel that is not used.
pub const NO_LABEL: u8 = u8::MAX;

/// A cluster count whose inertia is at most this fraction of the total variance is kept without
/// testing for an elbow.
const GOOD_ENOUGH: f64 = 0.05;

/// What the pixel values of a [`SuggestView`] mean.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuggestKind {
    /// Linear camera RGB relative to the empty rig (clustered in CIELAB).
    Transmittance,
    /// Whitened residuals (clustered as plain 3-vectors).
    Residual,
}

/// One view's image on its working grid.
#[derive(Debug, Clone, PartialEq)]
pub struct SuggestView {
    /// The rig view index.
    pub view: usize,
    /// The working grid.
    pub grid: WorkingGrid,
    /// The values, row-major (`grid.width * grid.height`).
    pub values: Vec<[f32; 3]>,
    /// Whether a pixel is used.
    pub valid: Vec<bool>,
    /// What the values are.
    pub kind: SuggestKind,
}

impl SuggestView {
    /// The predicted transmittance of a forward evaluation (synthetic data or a model image).
    #[must_use]
    pub fn from_prediction(prediction: &ViewPrediction) -> Self {
        Self {
            view: prediction.view,
            grid: prediction.grid,
            values: prediction.rgb.clone(),
            valid: prediction.valid.clone(),
            kind: SuggestKind::Transmittance,
        }
    }

    /// A photo's transmittance image; pixels with a non-finite value or variance are unused.
    #[must_use]
    pub fn from_observed(grid: WorkingGrid, observed: &ObservedView) -> Self {
        let valid = observed
            .values
            .iter()
            .zip(&observed.variance)
            .map(|(v, s)| v.iter().chain(s.iter()).all(|x| x.is_finite()))
            .collect();
        Self {
            view: observed.view,
            grid,
            values: observed.values.clone(),
            valid,
            kind: SuggestKind::Transmittance,
        }
    }

    /// The whitened residual map of a fit.
    #[must_use]
    pub fn from_residual(grid: WorkingGrid, map: &ResidualMap) -> Self {
        Self {
            view: map.view,
            grid,
            values: map.whitened.clone(),
            valid: map.valid.clone(),
            kind: SuggestKind::Residual,
        }
    }
}

/// How a boundary pixel becomes a 3D point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackProjection {
    /// The point where the camera ray meets the stone's surface.
    SurfaceEntry,
    /// The middle of the first stretch of the refracted ray inside the stone.
    LegMidpoint,
}

/// Settings of the suggestion. The defaults are described in the module docs.
#[derive(Debug, Clone, PartialEq)]
pub struct SuggestOptions {
    /// The fewest clusters tried (default 2).
    pub k_min: usize,
    /// The most clusters tried (default 4).
    pub k_max: usize,
    /// A fixed number of clusters (overrides the range).
    pub k: Option<usize>,
    /// The elbow ratio of the cluster-count rule (default 0.7).
    pub elbow: f64,
    /// Lloyd iterations (default 50).
    pub kmeans_iterations: usize,
    /// The weight `beta` of the smoothness prior (default 1.5).
    pub smoothness: f64,
    /// Smoothing sweeps (default 8).
    pub icm_sweeps: usize,
    /// The fewest boundary points a pair of clusters needs (default 12).
    pub min_boundary_points: usize,
    /// The most boundary samples per cluster pair and view that are traced (default 150).
    pub max_points_per_pair_view: usize,
    /// The most pixels per view traced to orient the shapes (default 300).
    pub label_samples_per_view: usize,
    /// How boundary pixels become points (default the surface entry).
    pub back_projection: BackProjection,
    /// The fit tolerance as a fraction of the mesh size (default 0.01); at least half a working
    /// pixel in mm is used.
    pub fit_tolerance_fraction: f64,
    /// The crystal axis, to fix the axis of the cylinders and prisms.
    pub c_axis: Option<DVec3>,
    /// Try cylinders (default true).
    pub try_cylinder: bool,
    /// The side counts of the prisms tried (default 3 and 6).
    pub prism_sides: Vec<u32>,
    /// Suggestions scoring below this are dropped (default 0.15).
    pub min_score: f64,
    /// The most suggestions returned (default 6).
    pub max_suggestions: usize,
}

impl Default for SuggestOptions {
    fn default() -> Self {
        Self {
            k_min: 2,
            k_max: 4,
            k: None,
            elbow: 0.7,
            kmeans_iterations: 50,
            smoothness: 1.5,
            icm_sweeps: 8,
            min_boundary_points: 12,
            max_points_per_pair_view: 150,
            label_samples_per_view: 300,
            back_projection: BackProjection::SurfaceEntry,
            fit_tolerance_fraction: 0.01,
            c_axis: None,
            try_cylinder: true,
            prism_sides: vec![3, 6],
            min_score: 0.15,
            max_suggestions: 6,
        }
    }
}

/// Why no suggestion could be made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SuggestError {
    /// There are not enough valid pixels.
    NoPixels,
    /// The views do not all hold the same kind of image.
    MixedKinds,
    /// The values or the validity of this view do not match its grid.
    BadView(usize),
    /// A setting is out of range.
    BadOptions(&'static str),
}

impl std::fmt::Display for SuggestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoPixels => write!(f, "there are not enough usable pixels to segment"),
            Self::MixedKinds => write!(f, "the views mix transmittance images and residual maps"),
            Self::BadView(v) => write!(f, "view {} does not match its working grid", v + 1),
            Self::BadOptions(what) => write!(f, "invalid setting: {what}"),
        }
    }
}

impl std::error::Error for SuggestError {}

/// The labels of one view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewLabels {
    /// The rig view index.
    pub view: usize,
    /// Working pixels across.
    pub width: usize,
    /// Working pixels down.
    pub height: usize,
    /// The cluster of every pixel, [`NO_LABEL`] for unused pixels.
    pub labels: Vec<u8>,
}

/// The segmentation of the views.
#[derive(Debug, Clone, PartialEq)]
pub struct Segmentation {
    /// The number of clusters kept (non-empty after smoothing).
    pub k: usize,
    /// The centre of every cluster in the clustering space (CIELAB for transmittance images).
    pub centres: Vec<[f64; 3]>,
    /// The inertia of every cluster count tried, as `(k, inertia)`.
    pub inertia: Vec<(usize, f64)>,
    /// The labels of every view, in the order of the input.
    pub views: Vec<ViewLabels>,
}

/// One suggested zone shape. Never applied automatically.
#[derive(Debug, Clone, PartialEq)]
pub struct ZoneSuggestion {
    /// The shape in the mesh frame (use with an identity zone frame).
    pub shape: ZoneShape,
    /// The score in `0..=1`: inlier fraction, times `1 / (1 + (rms / tolerance)^2)`, times a
    /// preference for simpler shapes (half space 1, cylinder 0.9, prism 0.85).
    pub score: f64,
    /// The number of views that contributed at least 3 inlier boundary points.
    pub views_supporting: usize,
    /// The residual RMS of the inliers, mm.
    pub rms_mm: f64,
    /// The inlier boundary points.
    pub boundary_points: usize,
    /// The two clusters whose boundary this is, smaller number first.
    pub labels: [u8; 2],
    /// The cluster the zone is made of (the smaller label).
    pub target_label: u8,
    /// The centre of that cluster in the clustering space.
    pub target_centre: [f64; 3],
}

/// The segmentation and the suggestions.
#[derive(Debug, Clone, PartialEq)]
pub struct SuggestReport {
    /// The segmentation.
    pub segmentation: Segmentation,
    /// The suggestions, best first.
    pub suggestions: Vec<ZoneSuggestion>,
}

fn validate(views: &[SuggestView], options: &SuggestOptions) -> Result<SuggestKind, SuggestError> {
    let bad = SuggestError::BadOptions;
    let (lo, hi) = options.k.map_or((options.k_min, options.k_max), |k| (k, k));
    if lo < 2 || hi < lo || hi > 8 {
        return Err(bad("the cluster count must be between 2 and 8"));
    }
    if !(options.elbow.is_finite() && options.elbow > 0.0 && options.elbow <= 1.0) {
        return Err(bad("the elbow ratio must be in (0, 1]"));
    }
    if !(options.smoothness.is_finite() && options.smoothness >= 0.0) {
        return Err(bad("the smoothness weight must not be negative"));
    }
    if !(options.fit_tolerance_fraction.is_finite() && options.fit_tolerance_fraction > 0.0) {
        return Err(bad("the fit tolerance must be positive"));
    }
    let kind = views
        .first()
        .map(|v| v.kind)
        .ok_or(SuggestError::NoPixels)?;
    for v in views {
        if v.kind != kind {
            return Err(SuggestError::MixedKinds);
        }
        let n = v.grid.width * v.grid.height;
        if v.values.len() != n || v.valid.len() != n || n == 0 {
            return Err(SuggestError::BadView(v.view));
        }
    }
    Ok(kind)
}

/// The clustering feature of a pixel.
fn feature(kind: SuggestKind, rgb: [f32; 3]) -> [f64; 3] {
    let clean = |x: f32| if x.is_finite() { f64::from(x) } else { 0.0 };
    let (r, g, b) = (clean(rgb[0]), clean(rgb[1]), clean(rgb[2]));
    match kind {
        SuggestKind::Residual => [r, g, b],
        SuggestKind::Transmittance => {
            let (r, g, b) = (r.max(0.0), g.max(0.0), b.max(0.0));
            let x = 0.412_456_4 * r + 0.357_576_1 * g + 0.180_437_5 * b;
            let y = 0.212_672_9 * r + 0.715_152_2 * g + 0.072_175_0 * b;
            let z = 0.019_333_9 * r + 0.119_192_0 * g + 0.950_304_1 * b;
            xyz_to_lab([x, y, z], [0.950_47, 1.0, 1.088_83])
        }
    }
}

fn dist2(a: [f64; 3], b: [f64; 3]) -> f64 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)
}

struct KMeans {
    centres: Vec<[f64; 3]>,
    assign: Vec<u8>,
    inertia: f64,
}

fn nearest(centres: &[[f64; 3]], f: [f64; 3]) -> u8 {
    let mut best = 0_usize;
    let mut best_d = f64::INFINITY;
    for (j, c) in centres.iter().enumerate() {
        let d = dist2(f, *c);
        if d < best_d {
            best_d = d;
            best = j;
        }
    }
    best as u8
}

/// Lloyd's algorithm from quantile seeds of the pixels sorted by their features.
fn kmeans(features: &[[f64; 3]], k: usize, iterations: usize) -> KMeans {
    let n = features.len();
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&i, &j| {
        features[i][0]
            .total_cmp(&features[j][0])
            .then(features[i][1].total_cmp(&features[j][1]))
            .then(features[i][2].total_cmp(&features[j][2]))
            .then(i.cmp(&j))
    });
    let mut centres: Vec<[f64; 3]> = (0..k)
        .map(|j| {
            let pos = (((j as f64 + 0.5) / k as f64) * n as f64) as usize;
            features[order[pos.min(n - 1)]]
        })
        .collect();
    let mut assign = vec![NO_LABEL; n];
    for _ in 0..iterations.max(1) {
        let mut changed = false;
        for (i, f) in features.iter().enumerate() {
            let a = nearest(&centres, *f);
            if a != assign[i] {
                assign[i] = a;
                changed = true;
            }
        }
        let mut sums = vec![[0.0_f64; 3]; k];
        let mut counts = vec![0_usize; k];
        for (i, f) in features.iter().enumerate() {
            let a = usize::from(assign[i]);
            for c in 0..3 {
                sums[a][c] += f[c];
            }
            counts[a] += 1;
        }
        for j in 0..k {
            if counts[j] > 0 {
                let m = counts[j] as f64;
                centres[j] = [sums[j][0] / m, sums[j][1] / m, sums[j][2] / m];
            }
        }
        if !changed {
            break;
        }
    }
    let mut inertia = 0.0;
    for (i, f) in features.iter().enumerate() {
        assign[i] = nearest(&centres, *f);
        inertia += dist2(*f, centres[usize::from(assign[i])]);
    }
    KMeans {
        centres,
        assign,
        inertia,
    }
}

/// Segments the views into 2 to 4 colour clusters (module docs, steps 1 and 2).
///
/// # Errors
///
/// [`SuggestError`] for bad settings, views that do not match their grids, mixed image kinds, or
/// fewer valid pixels than clusters.
pub fn segment_views(
    views: &[SuggestView],
    options: &SuggestOptions,
) -> Result<Segmentation, SuggestError> {
    let kind = validate(views, options)?;
    let per_view: Vec<Vec<[f64; 3]>> = views
        .iter()
        .map(|v| v.values.iter().map(|rgb| feature(kind, *rgb)).collect())
        .collect();
    let mut pooled = Vec::new();
    let mut origin: Vec<(usize, usize)> = Vec::new();
    for (slot, v) in views.iter().enumerate() {
        for (p, ok) in v.valid.iter().enumerate() {
            if *ok {
                pooled.push(per_view[slot][p]);
                origin.push((slot, p));
            }
        }
    }
    let ks: Vec<usize> = options
        .k
        .map_or_else(|| (options.k_min..=options.k_max).collect(), |k| vec![k]);
    let top = *ks.last().unwrap_or(&2);
    if pooled.len() < top.max(2) {
        return Err(SuggestError::NoPixels);
    }
    let runs: Vec<KMeans> = ks
        .iter()
        .map(|&k| kmeans(&pooled, k, options.kmeans_iterations))
        .collect();
    let inertia: Vec<(usize, f64)> = ks
        .iter()
        .copied()
        .zip(runs.iter().map(|r| r.inertia))
        .collect();
    // The inertia about the single global mean: a count of clusters that already explains all but
    // `GOOD_ENOUGH` of it is kept without looking at the elbow (splitting noise lowers a tiny
    // inertia by a large ratio).
    let global_mean = {
        let m = pooled.len() as f64;
        let mut sum = [0.0_f64; 3];
        for f in &pooled {
            for c in 0..3 {
                sum[c] += f[c];
            }
        }
        [sum[0] / m, sum[1] / m, sum[2] / m]
    };
    let total: f64 = pooled.iter().map(|f| dist2(*f, global_mean)).sum();
    let mut chosen = runs.len() - 1;
    for i in 0..runs.len().saturating_sub(1) {
        if runs[i].inertia <= GOOD_ENOUGH * total
            || runs[i + 1].inertia > options.elbow * runs[i].inertia
        {
            chosen = i;
            break;
        }
    }
    let run = &runs[chosen];
    let k = ks[chosen];

    // Number the clusters by the first channel of their centre, ascending.
    let mut order: Vec<usize> = (0..k).collect();
    order.sort_by(|&a, &b| {
        run.centres[a][0]
            .total_cmp(&run.centres[b][0])
            .then(a.cmp(&b))
    });
    let mut rank = vec![0_u8; k];
    for (new, &old) in order.iter().enumerate() {
        rank[old] = new as u8;
    }
    let centres: Vec<[f64; 3]> = order.iter().map(|&old| run.centres[old]).collect();
    let mut labels: Vec<Vec<u8>> = views
        .iter()
        .map(|v| vec![NO_LABEL; v.grid.width * v.grid.height])
        .collect();
    for (i, &(slot, p)) in origin.iter().enumerate() {
        labels[slot][p] = rank[usize::from(run.assign[i])];
    }

    // Iterated conditional modes with a Potts prior.
    let sigma2 = (run.inertia / (3.0 * pooled.len() as f64)).max(1e-9);
    for _ in 0..options.icm_sweeps {
        let mut changed = false;
        for (slot, v) in views.iter().enumerate() {
            let (w, h) = (v.grid.width, v.grid.height);
            for y in 0..h {
                for x in 0..w {
                    let p = y * w + x;
                    let current = labels[slot][p];
                    if current == NO_LABEL {
                        continue;
                    }
                    let f = per_view[slot][p];
                    let neighbours = neighbour_labels(&labels[slot], w, h, x, y);
                    let energy = |l: u8| -> f64 {
                        let differing = neighbours
                            .iter()
                            .filter(|n| **n != NO_LABEL && **n != l)
                            .count();
                        dist2(f, centres[usize::from(l)]) / (2.0 * sigma2)
                            + options.smoothness * differing as f64
                    };
                    let mut best = current;
                    let mut best_e = energy(current);
                    for l in 0..k as u8 {
                        let e = energy(l);
                        if e < best_e - 1e-12 {
                            best_e = e;
                            best = l;
                        }
                    }
                    if best != current {
                        labels[slot][p] = best;
                        changed = true;
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }
    let mut used = vec![false; k];
    for l in labels.iter().flatten() {
        if *l != NO_LABEL {
            used[usize::from(*l)] = true;
        }
    }
    Ok(Segmentation {
        k: used.iter().filter(|u| **u).count(),
        centres,
        inertia,
        views: views
            .iter()
            .zip(labels)
            .map(|(v, labels)| ViewLabels {
                view: v.view,
                width: v.grid.width,
                height: v.grid.height,
                labels,
            })
            .collect(),
    })
}

fn neighbour_labels(labels: &[u8], w: usize, h: usize, x: usize, y: usize) -> [u8; 4] {
    let at = |xx: isize, yy: isize| -> u8 {
        if xx < 0 || yy < 0 || xx >= w as isize || yy >= h as isize {
            NO_LABEL
        } else {
            labels[yy as usize * w + xx as usize]
        }
    };
    let (xi, yi) = (x as isize, y as isize);
    [
        at(xi - 1, yi),
        at(xi + 1, yi),
        at(xi, yi - 1),
        at(xi, yi + 1),
    ]
}

/// One boundary sample between two clusters.
struct Sample {
    slot: usize,
    position: DVec2,
}

/// The boundary samples of every pair of clusters, in raster order per view (horizontal pairs of
/// a view first, then vertical ones).
fn boundary_samples(
    views: &[SuggestView],
    segmentation: &Segmentation,
) -> BTreeMap<(u8, u8), Vec<Sample>> {
    let mut out: BTreeMap<(u8, u8), Vec<Sample>> = BTreeMap::new();
    for (slot, (v, vl)) in views.iter().zip(&segmentation.views).enumerate() {
        let (w, h) = (vl.width, vl.height);
        let at = |x: usize, y: usize| vl.labels[y * w + x];
        let place = |gx: f64, gy: f64| {
            DVec2::new(
                v.grid.origin[0] + gx * v.grid.scale,
                v.grid.origin[1] + gy * v.grid.scale,
            )
        };
        for y in 0..h {
            for x in 0..w.saturating_sub(1) {
                let (a, b) = (at(x, y), at(x + 1, y));
                if a != NO_LABEL && b != NO_LABEL && a != b {
                    out.entry((a.min(b), a.max(b))).or_default().push(Sample {
                        slot,
                        position: place(x as f64 + 1.0, y as f64 + 0.5),
                    });
                }
            }
        }
        for y in 0..h.saturating_sub(1) {
            for x in 0..w {
                let (a, b) = (at(x, y), at(x, y + 1));
                if a != NO_LABEL && b != NO_LABEL && a != b {
                    out.entry((a.min(b), a.max(b))).or_default().push(Sample {
                        slot,
                        position: place(x as f64 + 0.5, y as f64 + 1.0),
                    });
                }
            }
        }
    }
    out
}

fn back_project(
    scene: &Scene<'_>,
    view: usize,
    position: DVec2,
    mode: BackProjection,
) -> Option<DVec3> {
    let path = trace_pixel(scene, view, position, 0).ok()?;
    match mode {
        BackProjection::SurfaceEntry => Some(path.entry),
        BackProjection::LegMidpoint => path.legs.first().map(|l| l.from.midpoint(l.to)),
    }
}

/// The mean size of a working pixel at the stone, mm.
fn working_pixel_mm(scene: &Scene<'_>, views: &[SuggestView]) -> f64 {
    let (lo, hi) = scene.mesh.bounds();
    let centre = scene.alignment.to_rig(lo.midpoint(hi));
    let mut sum = 0.0;
    let mut count = 0_usize;
    for v in views {
        let Some(pose) = scene.rig.views.get(v.view) else {
            continue;
        };
        let mm_per_px = match pose.projection {
            Projection::Orthographic { px_per_mm } => 1.0 / px_per_mm,
            Projection::Pinhole { focal_px } => (centre - pose.position_vec()).length() / focal_px,
        };
        sum += v.grid.scale * mm_per_px;
        count += 1;
    }
    if count == 0 { 0.0 } else { sum / count as f64 }
}

/// A fit and which input points it kept.
struct Trimmed {
    fitted: FittedZoneShape,
    kept: Vec<bool>,
}

type FitCall<'a> = dyn Fn(&BoundaryPoints) -> Result<FittedZoneShape, ZoneFitError> + 'a;

/// Fits, drops the points farther than `max(2.5 tol, 2 rms)` and fits again once.
fn fit_trimmed(points: &[(DVec3, usize)], tol: f64, fit: &FitCall<'_>) -> Option<Trimmed> {
    let all = BoundaryPoints::from_points(points.iter().map(|p| p.0).collect());
    let first = fit(&all).ok()?;
    let threshold = (2.5 * tol).max(2.0 * first.rms_mm);
    let kept: Vec<bool> = first
        .per_point_residual
        .iter()
        .map(|r| r.abs() <= threshold)
        .collect();
    let count = kept.iter().filter(|k| **k).count();
    if count == points.len() || count < 6 {
        return Some(Trimmed {
            fitted: first,
            kept,
        });
    }
    let reduced = BoundaryPoints::from_points(
        points
            .iter()
            .zip(&kept)
            .filter(|(_, k)| **k)
            .map(|(p, _)| p.0)
            .collect(),
    );
    let second = fit(&reduced).ok()?;
    Some(Trimmed {
        fitted: second,
        kept,
    })
}

fn mean_point(points: &[DVec3]) -> Option<DVec3> {
    if points.is_empty() {
        return None;
    }
    let mut sum = DVec3::ZERO;
    for p in points {
        sum += *p;
    }
    Some(sum / points.len() as f64)
}

/// The mean distance of `points` from the axis of a cylinder or prism, if `shape` is one.
fn mean_axis_distance(shape: &ZoneShape, points: &[DVec3]) -> Option<(f64, f64)> {
    let (axis_point, axis_dir, radius) = match shape {
        ZoneShape::CoaxialCylinder {
            axis_point,
            axis_dir,
            r_out,
            ..
        }
        | ZoneShape::CoaxialPrism {
            axis_point,
            axis_dir,
            r_out,
            ..
        } => (*axis_point, *axis_dir, *r_out),
        _ => return None,
    };
    if points.is_empty() {
        return None;
    }
    let mean = points
        .iter()
        .map(|p| {
            let q = *p - axis_point;
            (q - axis_dir * q.dot(axis_dir)).length()
        })
        .sum::<f64>()
        / points.len() as f64;
    Some((mean, radius))
}

struct Candidate {
    shape: ZoneShape,
    rms: f64,
    kept: Vec<bool>,
    prior: f64,
}

fn candidates(
    points: &[(DVec3, usize)],
    target_points: &[DVec3],
    mesh_scale: f64,
    tol: f64,
    options: &SuggestOptions,
) -> Vec<Candidate> {
    let mut out = Vec::new();
    let toward = mean_point(target_points);
    if let Some(t) = fit_trimmed(points, tol, &|bp| fit_half_space(bp, toward)) {
        out.push(Candidate {
            shape: t.fitted.shape,
            rms: t.fitted.rms_mm,
            kept: t.kept,
            prior: 1.0,
        });
    }
    let radial = |sides: Option<u32>, prior: f64, out: &mut Vec<Candidate>| {
        let fit_with = |role: RadialRole| {
            move |bp: &BoundaryPoints| {
                sides.map_or_else(
                    || fit_cylinder(bp, options.c_axis, role),
                    |n| fit_prism(bp, n, options.c_axis, role),
                )
            }
        };
        let all = BoundaryPoints::from_points(points.iter().map(|p| p.0).collect());
        let Ok(first) = fit_with(RadialRole::Core)(&all) else {
            return;
        };
        let Some((mean_distance, radius)) = mean_axis_distance(&first.shape, target_points) else {
            return;
        };
        if !(radius.is_finite() && radius < 20.0 * mesh_scale) {
            return;
        }
        let role = if mean_distance <= radius {
            RadialRole::Core
        } else {
            RadialRole::Beyond {
                r_out: radius + 2.0 * mesh_scale,
            }
        };
        let fit = fit_with(role);
        if let Some(t) = fit_trimmed(points, tol, &fit) {
            out.push(Candidate {
                shape: t.fitted.shape,
                rms: t.fitted.rms_mm,
                kept: t.kept,
                prior,
            });
        }
    };
    if options.try_cylinder {
        radial(None, 0.9, &mut out);
    }
    for &n in &options.prism_sides {
        radial(Some(n), 0.85, &mut out);
    }
    out
}

/// Suggests zone shapes from the segmentation's cluster boundaries (module docs, steps 3 and 4).
fn suggestions_from(
    scene: &Scene<'_>,
    views: &[SuggestView],
    segmentation: &Segmentation,
    options: &SuggestOptions,
) -> Vec<ZoneSuggestion> {
    let (lo, hi) = scene.mesh.bounds();
    let mesh_scale = (hi - lo).length();
    let tol = (options.fit_tolerance_fraction * mesh_scale)
        .max(0.5 * working_pixel_mm(scene, views))
        .max(1e-9);

    // Surface points of every cluster, to orient the shapes.
    let mut cluster_points: Vec<Vec<DVec3>> = vec![Vec::new(); segmentation.centres.len()];
    for (v, vl) in views.iter().zip(&segmentation.views) {
        let used: Vec<usize> = (0..vl.labels.len())
            .filter(|&p| vl.labels[p] != NO_LABEL)
            .collect();
        let step = used
            .len()
            .div_ceil(options.label_samples_per_view.max(1))
            .max(1);
        for &p in used.iter().step_by(step) {
            let centre = v.grid.centre(p % vl.width, p / vl.width);
            if let Some(point) = back_project(scene, v.view, centre, options.back_projection) {
                cluster_points[usize::from(vl.labels[p])].push(point);
            }
        }
    }

    let mut out = Vec::new();
    for ((a, b), samples) in boundary_samples(views, segmentation) {
        let mut points: Vec<(DVec3, usize)> = Vec::new();
        for slot in 0..views.len() {
            let own: Vec<&Sample> = samples.iter().filter(|s| s.slot == slot).collect();
            let step = own
                .len()
                .div_ceil(options.max_points_per_pair_view.max(1))
                .max(1);
            for s in own.into_iter().step_by(step) {
                if let Some(p) =
                    back_project(scene, views[slot].view, s.position, options.back_projection)
                {
                    points.push((p, views[slot].view));
                }
            }
        }
        if points.len() < options.min_boundary_points {
            continue;
        }
        let target = a;
        let target_points = &cluster_points[usize::from(target)];
        for candidate in candidates(&points, target_points, mesh_scale, tol, options) {
            let total = points.len();
            let inliers = candidate.kept.iter().filter(|k| **k).count();
            let inlier_fraction = inliers as f64 / total as f64;
            let score = inlier_fraction / (1.0 + (candidate.rms / tol).powi(2)) * candidate.prior;
            if !score.is_finite() || score < options.min_score {
                continue;
            }
            let mut per_view: BTreeMap<usize, usize> = BTreeMap::new();
            for ((_, view), kept) in points.iter().zip(&candidate.kept) {
                if *kept {
                    *per_view.entry(*view).or_default() += 1;
                }
            }
            out.push(ZoneSuggestion {
                shape: candidate.shape,
                score,
                views_supporting: per_view.values().filter(|c| **c >= 3).count(),
                rms_mm: candidate.rms,
                boundary_points: inliers,
                labels: [a, b],
                target_label: target,
                target_centre: segmentation.centres[usize::from(target)],
            });
        }
    }
    out.sort_by(|x, y| y.score.total_cmp(&x.score));
    out.truncate(options.max_suggestions);
    out
}

/// Segments the views and suggests zone shapes from the boundaries between the clusters.
///
/// Returns the segmentation (for an overlay of the regions) and the candidate shapes, best
/// first; the list is empty when the images are uniform or no boundary could be traced. The
/// shapes are in the mesh frame.
///
/// # Errors
///
/// [`SuggestError`] as [`segment_views`].
pub fn suggest_zones(
    scene: &Scene<'_>,
    views: &[SuggestView],
    options: &SuggestOptions,
) -> Result<SuggestReport, SuggestError> {
    let segmentation = segment_views(views, options)?;
    let suggestions = if segmentation.k >= 2 {
        suggestions_from(scene, views, &segmentation, options)
    } else {
        Vec::new()
    };
    Ok(SuggestReport {
        segmentation,
        suggestions,
    })
}
