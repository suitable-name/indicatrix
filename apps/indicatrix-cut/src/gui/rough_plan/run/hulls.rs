//! The convex outlines the exact single-stone fit works on: the cached hulls rotated into
//! each design's caliper frame and centred on their bounding boxes.

use indicatrix::geometry::stone_metrics::caliper_frame;
use indicatrix_cut_core::rough_plan::DesignHull;
use indicatrix_vault::model::{
    solid_extents::{SolidExtents, SolidExtentsSource, StoredSolidExtents},
    solid_hull::SolidHull,
};
use std::collections::{BTreeMap, BTreeSet};
use tracing::debug;

/// The relative disagreement allowed between a hull's box and the cached caliper extents.
const EXTENT_TOLERANCE: f64 = 1e-6;

/// The rounding of an `f32` coordinate as a multiple of its magnitude (with margin). A
/// design authored far from the origin loses that much when its hull is stored as `f32`.
const F32_ROUNDING: f64 = 4.0 * 1.192_092_9e-7;

/// The hulls that can be fitted, and what became of the designs that have none.
#[derive(Debug, Default)]
pub(super) struct PreparedHulls {
    /// One hull per distinct usable design, in entry-id order.
    pub hulls: Vec<DesignHull>,
    /// Designs dropped because the hull is corrupt or no longer matches the cached
    /// extents (a stale cache).
    pub stale: usize,
    /// Designs whose hull has no usable outline (fewer than three points, or collinear).
    pub skipped: usize,
    /// Designs whose hull, volume and width equal those of an earlier (lower id) design
    /// bit for bit. Fitting the twin again would only repeat the same layout, so the
    /// first one stands for both.
    pub duplicates: usize,
}

/// The bits of everything the fit reads from a hull, for telling exact twins apart.
fn fit_key(hull: &DesignHull) -> Vec<u64> {
    let mut key = Vec::with_capacity(2 + 3 * hull.vertices.len());
    key.push(hull.volume.to_bits());
    key.push(hull.width.to_bits());
    key.extend(hull.vertices.iter().flatten().map(|c| c.to_bits()));
    key
}

/// A point set rotated into the caliper frame, not yet centred.
pub(in crate::gui::rough_plan) struct CaliperOutline {
    /// The rotated points.
    pub(in crate::gui::rough_plan) vertices: Vec<[f64; 3]>,
    /// The smallest coordinate of each axis.
    min: [f64; 3],
    /// The largest coordinate of each axis.
    max: [f64; 3],
    /// The direction `(x, z)` of the caliper width in the design frame, which the rotation
    /// turns onto x.
    pub(in crate::gui::rough_plan) width_dir: [f64; 2],
}

impl CaliperOutline {
    /// The bounding box's size `[width, height, length]`.
    pub(in crate::gui::rough_plan) fn size(&self) -> [f64; 3] {
        [0, 1, 2].map(|i| self.max[i] - self.min[i])
    }

    /// The centre of the bounding box.
    pub(in crate::gui::rough_plan) fn centre(&self) -> [f64; 3] {
        [0, 1, 2].map(|i| self.min[i].midpoint(self.max[i]))
    }

    /// The largest coordinate magnitude.
    fn magnitude(&self) -> f64 {
        self.vertices
            .iter()
            .flatten()
            .fold(0.0_f64, |largest, c| largest.max(c.abs()))
    }

    /// The vertices with the bounding-box centre moved to the origin.
    fn centred(&self) -> Vec<[f64; 3]> {
        let centre = self.centre();
        self.vertices
            .iter()
            .map(|v| [v[0] - centre[0], v[1] - centre[1], v[2] - centre[2]])
            .collect()
    }
}

/// What became of one cached hull.
enum Prepared {
    Ready(DesignHull),
    /// No usable outline (too few or collinear points): the design is not fitted.
    Skipped,
    /// The hull is corrupt or disagrees with the cached extents.
    Stale,
}

/// Grows the box `min..max` to contain `point`.
fn widen_bounds(min: &mut [f64; 3], max: &mut [f64; 3], point: [f64; 3]) {
    for ((low, high), value) in min.iter_mut().zip(max.iter_mut()).zip(point) {
        *low = low.min(value);
        *high = high.max(value);
    }
}

/// Turns `point` about the y axis so the caliper width direction `(wx, wz)` becomes x and
/// the length z (a proper rotation, so a stone is not mirrored).
#[must_use]
pub(in crate::gui::rough_plan) fn to_caliper_frame(
    point: [f64; 3],
    width_dir: [f64; 2],
) -> [f64; 3] {
    let [wx, wz] = width_dir;
    let [px, py, pz] = point;
    [px.mul_add(wx, pz * wz), py, (-px).mul_add(wz, pz * wx)]
}

/// Rotates `points` (a design's hull or facet corners) into the caliper frame of their
/// x/z outline. `None` for a degenerate outline.
pub(in crate::gui::rough_plan) fn rotate_into_caliper_frame(
    points: &[[f64; 3]],
) -> Option<CaliperOutline> {
    let outline: Vec<(f64, f64)> = points.iter().map(|p| (p[0], p[2])).collect();
    let width_dir = caliper_frame(&outline)?.width_dir;

    let mut vertices = Vec::with_capacity(points.len());
    let mut min = [f64::INFINITY; 3];
    let mut max = [f64::NEG_INFINITY; 3];
    for &point in points {
        let rotated = to_caliper_frame(point, width_dir);
        widen_bounds(&mut min, &mut max, rotated);
        vertices.push(rotated);
    }
    Some(CaliperOutline {
        vertices,
        min,
        max,
        width_dir,
    })
}

/// Whether the hull's box `[width, height, length]` matches the cached extents.
///
/// The tolerance is relative, widened by the `f32` rounding of the stored coordinates. Any
/// non-finite figure fails: an infinite one is refused outright (its tolerance would be
/// infinite too) and the comparison is written so that a NaN is "not close".
fn agrees_with_extents(size: [f64; 3], extents: &SolidExtents, slack: f64) -> bool {
    let expected = [
        extents.width_caliper.min(extents.length_caliper),
        extents.height,
        extents.width_caliper.max(extents.length_caliper),
    ];
    size.iter().zip(expected).all(|(&got, want)| {
        let tolerance = EXTENT_TOLERANCE.mul_add(got.max(want).max(1e-12), slack);
        got.is_finite() && want.is_finite() && (got - want).abs() <= tolerance
    })
}

/// The fit hull of one design, or why there is none.
fn prepare_one(entry_id: i64, extents: &SolidExtents, hull: &SolidHull) -> Prepared {
    if hull.vertices.len() < 3 {
        return Prepared::Skipped;
    }
    if hull.vertices.iter().flatten().any(|c| !c.is_finite()) || !extents.volume.is_finite() {
        debug!("Rough planner: dropping design #{entry_id}: its cached hull is not finite");
        return Prepared::Stale;
    }
    let points: Vec<[f64; 3]> = hull.vertices.iter().map(|v| v.map(f64::from)).collect();
    let Some(outline) = rotate_into_caliper_frame(&points) else {
        return Prepared::Skipped;
    };
    let size = outline.size();
    if !agrees_with_extents(size, extents, F32_ROUNDING * outline.magnitude()) {
        debug!(
            "Rough planner: dropping design #{entry_id}: rotated bbox ({:.4}, {:.4}, {:.4}) \
             disagrees with cached extents ({:.4}, {:.4}, {:.4})",
            size[0],
            size[1],
            size[2],
            extents.width_caliper.min(extents.length_caliper),
            extents.height,
            extents.width_caliper.max(extents.length_caliper)
        );
        return Prepared::Stale;
    }
    Prepared::Ready(DesignHull {
        entry_id,
        vertices: outline.centred(),
        volume: extents.volume,
        width: extents.width_caliper.min(extents.length_caliper),
    })
}

/// Builds the [`DesignHull`]s of the measured designs from the cached extents and hulls.
///
/// Each hull is rotated by its rotating-caliper direction so the width lies along x and
/// the length along z, and centred on its bounding box. A design is dropped when its
/// rotated box disagrees with the cached caliper extents (a stale cache) or the cached
/// data is not finite; a design without a hull is simply not fitted. A design whose fit
/// hull equals an earlier one's bit for bit is left out as a duplicate.
pub(super) fn prepare_design_hulls(
    extents_map: &BTreeMap<i64, StoredSolidExtents>,
    hulls_map: &BTreeMap<i64, SolidHull>,
) -> PreparedHulls {
    let mut prepared = PreparedHulls::default();
    let mut seen: BTreeSet<Vec<u64>> = BTreeSet::new();
    for (&entry_id, stored) in extents_map {
        if stored.source != SolidExtentsSource::DesignFile {
            continue;
        }
        let (Some(extents), Some(hull)) = (&stored.extents, hulls_map.get(&entry_id)) else {
            continue;
        };
        match prepare_one(entry_id, extents, hull) {
            Prepared::Ready(design_hull) => {
                if seen.insert(fit_key(&design_hull)) {
                    prepared.hulls.push(design_hull);
                } else {
                    prepared.duplicates += 1;
                }
            }
            Prepared::Skipped => prepared.skipped += 1,
            Prepared::Stale => prepared.stale += 1,
        }
    }
    prepared
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A box `[w, h, l]` rotated by `degrees` about y, then moved by `offset`, stored as
    /// the `f32` hull the vault keeps.
    fn box_hull(size: [f64; 3], degrees: f64, offset: [f64; 3]) -> SolidHull {
        let (sin, cos) = degrees.to_radians().sin_cos();
        let mut vertices = Vec::new();
        for sx in [-0.5, 0.5] {
            for sy in [-0.5, 0.5] {
                for sz in [-0.5, 0.5] {
                    let (x, y, z) = (sx * size[0], sy * size[1], sz * size[2]);
                    let rotated = [
                        x.mul_add(cos, z * sin) + offset[0],
                        y + offset[1],
                        (-x).mul_add(sin, z * cos) + offset[2],
                    ];
                    vertices.push(rotated.map(|c| c as f32));
                }
            }
        }
        SolidHull { vertices }
    }

    fn extents_of_box(size: [f64; 3]) -> SolidExtents {
        SolidExtents {
            width_caliper: size[0],
            length_caliper: size[2],
            width_axis: size[0],
            length_axis: size[2],
            height: size[1],
            volume: size[0] * size[1] * size[2],
        }
    }

    fn stored(extents: Option<SolidExtents>, source: SolidExtentsSource) -> StoredSolidExtents {
        StoredSolidExtents { extents, source }
    }

    fn one(
        extents: SolidExtents,
        hull: SolidHull,
    ) -> (BTreeMap<i64, StoredSolidExtents>, BTreeMap<i64, SolidHull>) {
        (
            BTreeMap::from([(7, stored(Some(extents), SolidExtentsSource::DesignFile))]),
            BTreeMap::from([(7, hull)]),
        )
    }

    #[test]
    fn a_rotated_box_comes_back_axis_aligned_with_width_on_x_and_centred() {
        let size = [1.0, 0.5, 3.0];
        let (extents, hulls) = one(extents_of_box(size), box_hull(size, 30.0, [0.0; 3]));
        let prepared = prepare_design_hulls(&extents, &hulls);
        assert_eq!(prepared.stale, 0);
        let [hull] = &prepared.hulls[..] else {
            panic!("expected one hull, got {}", prepared.hulls.len());
        };
        assert_eq!(hull.entry_id, 7);
        assert_eq!(hull.vertices.len(), 8);
        assert!((hull.width - 1.0).abs() < 1e-12);
        assert!((hull.volume - 1.5).abs() < 1e-12);

        let mut lo = [f64::INFINITY; 3];
        let mut hi = [f64::NEG_INFINITY; 3];
        for &v in &hull.vertices {
            widen_bounds(&mut lo, &mut hi, v);
        }
        for (axis, want) in size.iter().enumerate() {
            assert!(
                (hi[axis] - lo[axis] - want).abs() < 1e-6,
                "axis {axis}: {} vs {want}",
                hi[axis] - lo[axis]
            );
            assert!((hi[axis] + lo[axis]).abs() < 1e-9, "axis {axis} is centred");
        }
        assert!(
            hi[0] - lo[0] <= hi[2] - lo[2],
            "width is the smaller of x and z"
        );
    }

    #[test]
    fn a_stale_extents_row_drops_the_design_and_is_counted() {
        let size = [1.0, 0.5, 3.0];
        let mut wrong = extents_of_box(size);
        wrong.width_caliper *= 1.001;
        let (extents, hulls) = one(wrong, box_hull(size, 30.0, [0.0; 3]));
        let prepared = prepare_design_hulls(&extents, &hulls);
        assert_eq!(prepared.hulls, Vec::<DesignHull>::new());
        assert_eq!(prepared.stale, 1);

        let mut wrong_height = extents_of_box(size);
        wrong_height.height *= 0.99;
        let (extents, hulls) = one(wrong_height, box_hull(size, 30.0, [0.0; 3]));
        assert_eq!(prepare_design_hulls(&extents, &hulls).stale, 1);
    }

    #[test]
    fn designs_without_a_usable_row_or_hull_are_skipped_and_not_counted_as_stale() {
        let size = [1.0, 0.5, 3.0];
        let hull = box_hull(size, 0.0, [0.0; 3]);
        let extents = BTreeMap::from([
            (
                1,
                stored(Some(extents_of_box(size)), SolidExtentsSource::DesignFile),
            ),
            (
                2,
                stored(Some(extents_of_box(size)), SolidExtentsSource::AngleTable),
            ),
            (3, stored(None, SolidExtentsSource::DesignFile)),
            (4, stored(None, SolidExtentsSource::Unbounded)),
        ]);
        // Id 1 has no hull at all; 2, 3 and 4 have one but are not fittable rows.
        let hulls = BTreeMap::from([(2, hull.clone()), (3, hull.clone()), (4, hull)]);
        let prepared = prepare_design_hulls(&extents, &hulls);
        assert_eq!(prepared.hulls, Vec::<DesignHull>::new());
        assert_eq!(prepared.stale, 0);
    }

    #[test]
    fn f32_rounding_of_an_off_centre_design_does_not_drop_it() {
        let size = [1.0, 0.5, 3.0];
        for offset in [[40.0, 3.0, -25.0], [300.0, -120.0, 250.0]] {
            let (extents, hulls) = one(extents_of_box(size), box_hull(size, 30.0, offset));
            let prepared = prepare_design_hulls(&extents, &hulls);
            assert_eq!(prepared.hulls.len(), 1, "offset {offset:?}");
            assert_eq!(prepared.stale, 0, "offset {offset:?}");
        }
    }

    #[test]
    fn non_finite_hull_or_extents_data_is_rejected_not_passed_on() {
        let size = [1.0, 0.5, 3.0];
        for corrupt in [f32::NAN, f32::INFINITY] {
            let mut hull = box_hull(size, 30.0, [0.0; 3]);
            hull.vertices[3][1] = corrupt;
            let (extents, hulls) = one(extents_of_box(size), hull);
            let prepared = prepare_design_hulls(&extents, &hulls);
            assert_eq!(prepared.hulls, Vec::<DesignHull>::new());
            assert_eq!(prepared.stale, 1);
        }
        let mut nan_height = extents_of_box(size);
        nan_height.height = f64::NAN;
        let (extents, hulls) = one(nan_height, box_hull(size, 30.0, [0.0; 3]));
        let prepared = prepare_design_hulls(&extents, &hulls);
        // A NaN extent must not compare as close.
        assert_eq!(prepared.hulls, Vec::<DesignHull>::new());
        assert_eq!(prepared.stale, 1);

        // Neither must an infinite one, although its tolerance is infinite as well.
        for infinite in [f64::INFINITY, f64::NEG_INFINITY] {
            let mut wide = extents_of_box(size);
            wide.height = infinite;
            let (extents, hulls) = one(wide, box_hull(size, 30.0, [0.0; 3]));
            let prepared = prepare_design_hulls(&extents, &hulls);
            assert_eq!(prepared.hulls, Vec::<DesignHull>::new(), "{infinite}");
            assert_eq!(prepared.stale, 1, "{infinite}");
        }
    }

    #[test]
    fn a_degenerate_outline_is_skipped() {
        let size = [1.0, 0.5, 3.0];
        let two_points = SolidHull {
            vertices: vec![[0.0, 0.0, 0.0], [1.0, 0.5, 3.0]],
        };
        let (extents, hulls) = one(extents_of_box(size), two_points);
        assert_eq!(
            prepare_design_hulls(&extents, &hulls).hulls,
            Vec::<DesignHull>::new()
        );

        let collinear = SolidHull {
            vertices: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 1.0], [2.0, 0.5, 2.0]],
        };
        let (extents, hulls) = one(extents_of_box(size), collinear);
        let prepared = prepare_design_hulls(&extents, &hulls);
        assert_eq!(prepared.hulls, Vec::<DesignHull>::new());
        assert_eq!(prepared.stale, 0);
        assert_eq!(prepared.skipped, 1, "a skipped design is counted as such");
    }

    #[test]
    fn a_hull_equal_to_an_earlier_one_bit_for_bit_is_dropped_as_a_duplicate() {
        let size = [1.0, 0.5, 3.0];
        let longer = [1.0, 0.5, 3.5];
        let twin = box_hull(size, 30.0, [0.0; 3]);
        let design_file = |size| stored(Some(extents_of_box(size)), SolidExtentsSource::DesignFile);
        let extents = BTreeMap::from([
            (4, design_file(size)),
            (7, design_file(size)),
            (9, design_file(longer)),
        ]);
        let hulls = BTreeMap::from([
            (4, twin.clone()),
            (7, twin),
            (9, box_hull(longer, 30.0, [0.0; 3])),
        ]);
        let prepared = prepare_design_hulls(&extents, &hulls);
        let ids: Vec<i64> = prepared.hulls.iter().map(|h| h.entry_id).collect();
        assert_eq!(ids, vec![4, 9], "the lower id of the twins stays");
        assert_eq!(prepared.duplicates, 1);
        assert_eq!(prepared.stale, 0);
    }
}
