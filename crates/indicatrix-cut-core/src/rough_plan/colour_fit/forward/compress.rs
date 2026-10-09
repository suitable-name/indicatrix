//! Deterministic compression of the path records of one (pixel, bin) to at most `R` records.
//!
//! Plan section 5.3. The records are points in length space (one coordinate per zone) with a
//! weight. The compression
//!
//! 1. sorts them lexicographically (so the result does not depend on the order the samples were
//!    taken in) and merges exact duplicates, which is all a polished pixel needs;
//! 2. splits the cluster with the most weighted extent at its weighted median along its widest
//!    zone, until every cluster is narrower than the tolerance in every zone or `R` clusters
//!    exist (a polished pixel ends with one or two clusters, a frosted one uses all `R`);
//! 3. refines the cluster centres with two Lloyd (k-means) passes seeded by those clusters, and
//!    keeps the result when it is not wider than the median-cut one;
//! 4. replaces every cluster by one record at its weighted mean length with the summed weight.
//!
//! Total weight and the weighted first moment of every zone length are preserved exactly. The
//! second moment is not stored: a cluster whose zone-length range is `r` has a transmittance
//! error of about `(alpha r / 2)^2 / 2` (relative) for the absorption `alpha`, so the tolerance
//! `r <= 0.1 / alpha_ref` keeps the error under 0.13 % for `alpha <= alpha_ref` (the plan's
//! bound is 0.5 % at `alpha * range <= 0.1`). When `R` clusters are not narrow enough, the
//! widest range reached is returned and ends up in the trace statistics.

use std::cmp::Ordering;

use super::records::{LENGTHS, PathRecord};

#[derive(Clone, Copy)]
struct Point {
    len: [f32; LENGTHS],
    weight: f64,
}

fn lexicographic(a: &[f32; LENGTHS], b: &[f32; LENGTHS]) -> Ordering {
    for i in 0..LENGTHS {
        match a[i].total_cmp(&b[i]) {
            Ordering::Equal => {}
            other => return other,
        }
    }
    Ordering::Equal
}

/// The widest extent of `cluster` over the first `dims` zones: `(extent, zone)`.
fn widest(cluster: &[Point], dims: usize) -> (f32, usize) {
    let mut best = (0.0_f32, 0_usize);
    for d in 0..dims {
        let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
        for p in cluster {
            lo = lo.min(p.len[d]);
            hi = hi.max(p.len[d]);
        }
        let extent = hi - lo;
        if extent > best.0 {
            best = (extent, d);
        }
    }
    best
}

fn weight_of(cluster: &[Point]) -> f64 {
    cluster.iter().map(|p| p.weight).sum()
}

/// Median-cut: splits the most important cluster wider than `tolerance` until none is or
/// `max_records` clusters exist.
fn median_cut(
    points: Vec<Point>,
    max_records: usize,
    tolerance: f32,
    dims: usize,
) -> Vec<Vec<Point>> {
    let mut clusters = vec![points];
    while clusters.len() < max_records {
        let mut pick: Option<(usize, f64)> = None;
        for (i, cluster) in clusters.iter().enumerate() {
            if cluster.len() < 2 {
                continue;
            }
            let (extent, _) = widest(cluster, dims);
            if extent <= tolerance {
                continue;
            }
            let score = f64::from(extent) * weight_of(cluster);
            if pick.is_none_or(|(_, best)| score > best) {
                pick = Some((i, score));
            }
        }
        let Some((index, _)) = pick else { break };
        let (_, dim) = widest(&clusters[index], dims);
        let cluster = &mut clusters[index];
        cluster.sort_by(|a, b| {
            a.len[dim]
                .total_cmp(&b.len[dim])
                .then_with(|| lexicographic(&a.len, &b.len))
        });
        let half = 0.5 * weight_of(cluster);
        let mut cumulative = 0.0;
        let mut split = cluster.len() - 1;
        for (k, p) in cluster.iter().enumerate() {
            cumulative += p.weight;
            if cumulative >= half {
                split = k + 1;
                break;
            }
        }
        let split = split.clamp(1, cluster.len() - 1);
        let tail = cluster.split_off(split);
        clusters.push(tail);
    }
    clusters
}

fn centroid(cluster: &[Point]) -> [f64; LENGTHS] {
    let total = weight_of(cluster);
    let mut out = [0.0_f64; LENGTHS];
    for p in cluster {
        let w = if total > 0.0 {
            p.weight / total
        } else {
            1.0 / cluster.len() as f64
        };
        for (o, &len) in out.iter_mut().zip(&p.len) {
            *o = w.mul_add(f64::from(len), *o);
        }
    }
    out
}

/// Two Lloyd passes from the given clusters; returns the new clusters, empty ones dropped.
fn lloyd(clusters: &[Vec<Point>], dims: usize) -> Vec<Vec<Point>> {
    let mut centres: Vec<[f64; LENGTHS]> = clusters.iter().map(|c| centroid(c)).collect();
    let all: Vec<Point> = clusters.iter().flatten().copied().collect();
    let mut groups: Vec<Vec<Point>> = clusters.to_vec();
    for _ in 0..2 {
        groups = vec![Vec::new(); centres.len()];
        for p in &all {
            let mut best = (f64::INFINITY, 0_usize);
            for (k, c) in centres.iter().enumerate() {
                let mut d2 = 0.0;
                for (&len, &centre) in p.len.iter().zip(c.iter()).take(dims) {
                    let diff = f64::from(len) - centre;
                    d2 = diff.mul_add(diff, d2);
                }
                if d2 < best.0 {
                    best = (d2, k);
                }
            }
            groups[best.1].push(*p);
        }
        groups.retain(|g| !g.is_empty());
        centres = groups.iter().map(|g| centroid(g)).collect();
    }
    groups
}

fn max_extent(clusters: &[Vec<Point>], dims: usize) -> f32 {
    clusters
        .iter()
        .map(|c| widest(c, dims).0)
        .fold(0.0, f32::max)
}

/// Compresses `raw` (any order) into at most `max_records` records using the first `dims` zone
/// lengths for distances, appending them to `out`.
///
/// Returns the widest zone-length range inside
/// one resulting cluster, in mm (0 when everything merged exactly).
///
/// `raw` is sorted in place.
pub fn compress(
    raw: &mut [PathRecord],
    max_records: usize,
    tolerance_mm: f32,
    dims: usize,
    out: &mut Vec<PathRecord>,
) -> f32 {
    if raw.is_empty() {
        return 0.0;
    }
    let dims = dims.clamp(1, LENGTHS);
    raw.sort_by(|a, b| {
        lexicographic(&a.lengths, &b.lengths).then_with(|| a.weight.total_cmp(&b.weight))
    });
    let mut points: Vec<Point> = Vec::new();
    for r in &*raw {
        match points.last_mut() {
            Some(last) if last.len == r.lengths => last.weight += f64::from(r.weight),
            _ => points.push(Point {
                len: r.lengths,
                weight: f64::from(r.weight),
            }),
        }
    }
    let max_records = max_records.max(1);
    let mut clusters = median_cut(points, max_records, tolerance_mm, dims);
    let mut extent = max_extent(&clusters, dims);
    if clusters.len() > 1 && clusters.iter().any(|c| c.len() > 1) {
        let refined = lloyd(&clusters, dims);
        let refined_extent = max_extent(&refined, dims);
        if refined_extent <= extent.max(tolerance_mm) && refined.len() <= max_records {
            clusters = refined;
            extent = refined_extent;
        }
    }
    for cluster in &clusters {
        let centre = centroid(cluster);
        let mut lengths = [0.0_f32; LENGTHS];
        for d in 0..LENGTHS {
            lengths[d] = centre[d] as f32;
        }
        out.push(PathRecord {
            lengths,
            weight: weight_of(cluster) as f32,
        });
    }
    extent
}
