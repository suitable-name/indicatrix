//! The extents and convex hull scan: measures candidate designs whose solid extents or
//! hulls are not in the catalogue database yet and saves each result as it lands.
//!
//! # The measuring rule
//!
//! `gui::editor::resolve_catalogue_planes` gives a design's planes and where they came
//! from:
//!
//! - A design file's planes are the preform's own planes FIRST, then the facet planes.
//!   The default preform is a generous cylinder, so a schedule that does not close on
//!   its own would measure as a fat, inflated blob. So only `planes[preform_plane_count..]`
//!   (the facet planes alone) are measured. If they close, the design is stored as
//!   `DesignFile` along with its convex hull vertices; if not, it relies on its preform
//!   to close and is stored as `Unbounded` with no extents or hull.
//! - The angle-table fallback's planes are synthetic, so they are measured as they are
//!   and stored as `AngleTable` with no hull; the planner excludes them.
//!
//! A design that cannot be loaded or panics while measuring is left unsaved, so the
//! next run tries it again.
//!
//! A measurement is stored only while the design's geometry is the one it was measured
//! from: the invalidation epoch of the database is read in the same lock hold as the
//! record, and the save is refused when the epoch moved (an import or a native save
//! replaced the geometry meanwhile). A measurement whose save FAILED is not lost: it is
//! handed back in [`ScanOutcome::measured`] for this run to plan with.

use super::{Reporter, WORKER_STACK_BYTES, group_thousands, panic_message, stages};
use crate::gui::{batch::batch_queue::local_lane_count, editor::CataloguePlanesSource};
use indicatrix::geometry::{
    GpuFacetPlane,
    stone_metrics::{SolidMetrics, measure_solid_with_vertices},
};
use indicatrix_vault::{
    db::sqlite::Database,
    model::{
        entry::FullDiagramRecord,
        solid_extents::{SolidExtents, SolidExtentsSource, StoredSolidExtents},
        solid_hull::SolidHull,
    },
};
use std::{
    collections::BTreeMap,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Mutex, PoisonError,
        atomic::{AtomicUsize, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};
use tracing::{debug, warn};

/// The share of the progress bar the scan occupies when it has anything to do; the
/// planning stages use the rest.
pub(super) const SCAN_BAND: f32 = 0.35;

/// A design's measured extents, its source classification, and optional convex hull.
type Measured = (Option<SolidExtents>, SolidExtentsSource, Option<SolidHull>);

/// `metrics` as the stored extents, or `None` if any figure is unusable.
fn extents_of(metrics: &SolidMetrics) -> Option<SolidExtents> {
    let extents = SolidExtents {
        width_caliper: metrics.width_caliper,
        length_caliper: metrics.length_caliper,
        width_axis: metrics.width_axis,
        length_axis: metrics.length_axis,
        height: metrics.total_height,
        volume: metrics.volume,
    };
    [
        extents.width_caliper,
        extents.length_caliper,
        extents.width_axis,
        extents.length_axis,
        extents.height,
        extents.volume,
    ]
    .iter()
    .all(|v| v.is_finite() && *v > 0.0)
    .then_some(extents)
}

/// Measures one record under the rule in this module's doc comment.
pub(super) fn measure_record(full: &FullDiagramRecord) -> Measured {
    let resolved = crate::gui::editor::resolve_catalogue_planes(full);
    measure_planes(
        resolved.source,
        resolved.preform_plane_count,
        &resolved.planes,
    )
}

/// Measures `planes` (in the tracer's `n . x + d <= 0` convention) that came from `source`;
/// the first `preform_plane_count` of a design file's planes are its preform and are left
/// out.
fn measure_planes(
    source: CataloguePlanesSource,
    preform_plane_count: usize,
    planes: &[GpuFacetPlane],
) -> Measured {
    let halfspaces: Vec<_> = planes
        .iter()
        .copied()
        .map(GpuFacetPlane::to_halfspace_f64)
        .collect();

    match source {
        CataloguePlanesSource::AngleTable => {
            let measured = measure_solid_with_vertices(&halfspaces);
            let extents = measured.as_ref().and_then(|(m, _)| extents_of(m));
            (extents, SolidExtentsSource::AngleTable, None)
        }
        CataloguePlanesSource::DesignFile => {
            let start = preform_plane_count.min(halfspaces.len());
            let measured = measure_solid_with_vertices(&halfspaces[start..]);
            match measured {
                Some((metrics, verts)) => {
                    let extents = extents_of(&metrics);
                    let vertices = verts
                        .into_iter()
                        .map(|v| [v.x as f32, v.y as f32, v.z as f32])
                        .collect();
                    let hull = SolidHull { vertices };
                    (extents, SolidExtentsSource::DesignFile, Some(hull))
                }
                None => (None, SolidExtentsSource::Unbounded, None),
            }
        }
    }
}

/// What loading and measuring one design came to.
pub(super) enum Loaded {
    /// The design was measured. `epoch` is the database's invalidation count as read in
    /// the same lock hold as the record.
    Ready {
        /// The figures.
        measured: Measured,
        /// The invalidation epoch the record was read under.
        epoch: u64,
    },
    /// The design no longer exists.
    Gone,
    /// The record could not be read.
    Failed,
}

/// Loads `entry_id` (holding the database lock only for the read, which also reads the
/// invalidation epoch) and measures it.
fn measure_entry(db: &Mutex<Database>, entry_id: i64) -> Loaded {
    let (epoch, full) = {
        let guard = db.lock().unwrap_or_else(PoisonError::into_inner);
        (
            guard.solid_extents_epoch(),
            guard.get_diagram_full(entry_id),
        )
    };
    match full {
        Ok(Some(full)) => Loaded::Ready {
            measured: measure_record(&full),
            epoch,
        },
        Ok(None) => {
            debug!("Rough planner: design #{entry_id} no longer exists");
            Loaded::Gone
        }
        Err(e) => {
            warn!("Rough planner: could not load design #{entry_id}: {e}");
            Loaded::Failed
        }
    }
}

/// How saving one measurement ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Saved {
    /// The figures are in the database.
    Stored,
    /// The design's geometry was replaced since it was read, so the figures were dropped.
    Stale,
    /// The write failed.
    Failed,
}

/// Saves one measurement if the design is unchanged since `epoch` was read: the extents
/// and the hull in one transaction (a design measured without a hull drops any older hull
/// row). The database lock is held for the save only.
fn save_measurement(
    db: &Mutex<Database>,
    entry_id: i64,
    epoch: u64,
    (extents, source, hull): &Measured,
) -> Saved {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);

    let guard = db.lock().unwrap_or_else(PoisonError::into_inner);
    match guard.save_solid_extents_and_hull_if_current(
        entry_id,
        epoch,
        *extents,
        *source,
        hull.as_ref(),
        now,
    ) {
        Ok(true) => Saved::Stored,
        Ok(false) => {
            debug!("Rough planner: design #{entry_id} changed while it was measured; not saved");
            Saved::Stale
        }
        Err(e) => {
            warn!("Rough planner: could not save the measurement of design #{entry_id}: {e}");
            Saved::Failed
        }
    }
}

/// Lays the measurements whose save failed over what the database read back, so this run
/// plans with them although the cache could not keep them. A measurement without a hull
/// also removes the hull read back for it.
pub(super) fn merge_measured(
    stored: &mut BTreeMap<i64, StoredSolidExtents>,
    hulls: &mut BTreeMap<i64, SolidHull>,
    measured: BTreeMap<i64, Measured>,
) {
    for (entry_id, (extents, source, hull)) in measured {
        stored.insert(entry_id, StoredSolidExtents { extents, source });
        match hull {
            Some(hull) => {
                hulls.insert(entry_id, hull);
            }
            None => {
                hulls.remove(&entry_id);
            }
        }
    }
}

/// Which of `ids` (sorted) still need measuring, and whether that scan is only for the
/// convex outlines.
///
/// An id needs a scan when it has no extents row, or when it is a design-file row with
/// usable extents but no cached hull. Unbounded and angle-table rows never get a hull,
/// so a missing hull there is not a reason to measure again. The scan is "outlines
/// only" when every id to measure already has its extents (the first run after the
/// hull cache was introduced).
pub(super) fn ids_needing_scan(
    ids: &[i64],
    extents: &BTreeMap<i64, StoredSolidExtents>,
    hulls: &BTreeMap<i64, SolidHull>,
) -> (Vec<i64>, bool) {
    let mut missing = Vec::new();
    let mut every_id_has_extents = true;
    for &id in ids {
        match extents.get(&id) {
            None => {
                missing.push(id);
                every_id_has_extents = false;
            }
            Some(row) => {
                let wants_hull = row.source == SolidExtentsSource::DesignFile
                    && row.extents.is_some()
                    && !hulls.contains_key(&id);
                if wants_hull {
                    missing.push(id);
                }
            }
        }
    }
    let outlines_only = every_id_has_extents && !missing.is_empty();
    (missing, outlines_only)
}

/// What a scan left behind.
#[derive(Default)]
pub(super) struct ScanOutcome {
    /// Whether the user cancelled (what was measured so far stays saved).
    pub cancelled: bool,
    /// The measurements that could not be saved, by entry id: the run plans with them,
    /// but the next run measures those designs again.
    pub measured: BTreeMap<i64, Measured>,
    /// How many measurements could not be saved although the design was unchanged.
    pub save_failures: usize,
    /// How many designs could not be read from the database.
    pub load_failures: usize,
    /// How many designs panicked while being measured.
    pub panics: usize,
}

/// Loads and measures one design (see [`measure_entry`]); a parameter of the scan so a
/// test can stand in for the catalogue's geometry.
type MeasureFn<'a> = &'a (dyn Fn(&Mutex<Database>, i64) -> Loaded + Sync);

/// What the scan lanes share by reference.
struct ScanShared<'a> {
    db: &'a Mutex<Database>,
    reporter: &'a Reporter,
    missing: &'a [i64],
    outlines_only: bool,
    measure: MeasureFn<'a>,
    next: AtomicUsize,
    done: AtomicUsize,
    outcome: Mutex<ScanOutcome>,
}

impl ScanShared<'_> {
    /// Applies `update` to the outcome collected so far.
    fn record(&self, update: impl FnOnce(&mut ScanOutcome)) {
        update(&mut self.outcome.lock().unwrap_or_else(PoisonError::into_inner));
    }

    /// Stores one measurement; a failed write keeps it for this run.
    fn store(&self, entry_id: i64, epoch: u64, measured: Measured) {
        if save_measurement(self.db, entry_id, epoch, &measured) == Saved::Failed {
            self.record(|outcome| {
                outcome.save_failures += 1;
                outcome.measured.insert(entry_id, measured);
            });
        }
    }
}

/// One lane: claims the next unmeasured design until none is left or the user cancels.
fn scan_lane(shared: &ScanShared<'_>) {
    stages::lower_thread_priority();
    let total = shared.missing.len();
    loop {
        if shared.reporter.cancelled() {
            break;
        }
        let index = shared.next.fetch_add(1, Ordering::Relaxed);
        let Some(&entry_id) = shared.missing.get(index) else {
            break;
        };
        let loaded = catch_unwind(AssertUnwindSafe(|| (shared.measure)(shared.db, entry_id)));
        match loaded {
            Ok(Loaded::Ready { measured, epoch }) => shared.store(entry_id, epoch, measured),
            Ok(Loaded::Gone) => {}
            Ok(Loaded::Failed) => shared.record(|outcome| outcome.load_failures += 1),
            Err(payload) => {
                warn!(
                    "Rough planner: measuring design #{entry_id} panicked ({}); skipped",
                    panic_message(&*payload)
                );
                shared.record(|outcome| outcome.panics += 1);
            }
        }
        let finished = shared.done.fetch_add(1, Ordering::Relaxed) + 1;
        let fraction = SCAN_BAND * (finished as f32 / total.max(1) as f32);
        let label = if shared.outlines_only {
            format!(
                "Measuring design outlines {} / {} (one-time)",
                group_thousands(finished),
                group_thousands(total)
            )
        } else {
            format!(
                "Measuring designs {} / {}",
                group_thousands(finished),
                group_thousands(total)
            )
        };
        shared.reporter.report(&label, fraction, finished == total);
    }
}

/// Measures and saves every design in `missing` on `local_lane_count()` scoped threads.
/// What was measured before a cancel stays saved.
pub(super) fn scan_missing(
    db: &Mutex<Database>,
    reporter: &Reporter,
    missing: &[i64],
    outlines_only: bool,
) -> ScanOutcome {
    scan_with(
        db,
        reporter,
        missing,
        outlines_only,
        local_lane_count(),
        &measure_entry,
    )
}

/// [`scan_missing`] on `lanes` threads, with `measure` loading and measuring each design.
fn scan_with(
    db: &Mutex<Database>,
    reporter: &Reporter,
    missing: &[i64],
    outlines_only: bool,
    lanes: usize,
    measure: MeasureFn<'_>,
) -> ScanOutcome {
    if missing.is_empty() {
        return ScanOutcome::default();
    }
    let shared = ScanShared {
        db,
        reporter,
        missing,
        outlines_only,
        measure,
        next: AtomicUsize::new(0),
        done: AtomicUsize::new(0),
        outcome: Mutex::new(ScanOutcome::default()),
    };
    let initial_label = if outlines_only {
        format!(
            "Measuring design outlines 0 / {} (one-time)",
            group_thousands(missing.len())
        )
    } else {
        format!("Measuring designs 0 / {}", group_thousands(missing.len()))
    };
    reporter.report(&initial_label, 0.0, true);

    let lanes = lanes.min(missing.len()).max(1);
    std::thread::scope(|scope| {
        for lane in 0..lanes {
            std::thread::Builder::new()
                .name(format!("rough-plan-scan-{lane}"))
                .stack_size(WORKER_STACK_BYTES)
                .spawn_scoped(scope, || scan_lane(&shared))
                .expect("the operating system could not start a measuring thread");
        }
    });
    let mut outcome = shared
        .outcome
        .into_inner()
        .unwrap_or_else(PoisonError::into_inner);
    outcome.cancelled = reporter.cancelled();
    if outcome.load_failures > 0 || outcome.panics > 0 || outcome.save_failures > 0 {
        warn!(
            "Rough planner: of {} designs, {} could not be read, {} panicked while measured and \
             {} measurements could not be saved",
            missing.len(),
            outcome.load_failures,
            outcome.panics,
            outcome.save_failures
        );
    }
    outcome
}

/// The cached extents and hulls of some designs, by entry id.
type Cached = (BTreeMap<i64, StoredSolidExtents>, BTreeMap<i64, SolidHull>);

/// Reads cached extents and hulls for `ids` from the database.
pub(super) fn load_extents_and_hulls(db: &Mutex<Database>, ids: &[i64]) -> Result<Cached, String> {
    let (extents, hulls) = {
        let guard = db.lock().unwrap_or_else(PoisonError::into_inner);
        (guard.solid_extents_for(ids), guard.solid_hulls_for(ids))
    };
    let extents = extents.map_err(|e| format!("Could not read cached extents: {e}"))?;
    let hulls = hulls.map_err(|e| format!("Could not read cached hulls: {e}"))?;
    Ok((extents, hulls))
}

#[cfg(test)]
mod cache_tests;

#[cfg(test)]
mod tests {
    use super::{
        BTreeMap, CataloguePlanesSource, GpuFacetPlane, SolidExtents, SolidExtentsSource,
        SolidHull, StoredSolidExtents, ids_needing_scan, measure_planes,
    };
    use glam::Vec3;

    /// The six planes of an axis-aligned box centred on the origin with the given half sizes.
    fn box_planes(half: [f32; 3]) -> Vec<GpuFacetPlane> {
        [
            Vec3::X,
            Vec3::NEG_X,
            Vec3::Y,
            Vec3::NEG_Y,
            Vec3::Z,
            Vec3::NEG_Z,
        ]
        .into_iter()
        .map(|normal| {
            let along = normal.abs().dot(Vec3::from(half));
            GpuFacetPlane::new(normal, -along)
        })
        .collect()
    }

    /// The bounding box `(min, max)` of a hull's vertices.
    fn hull_bounds(hull: &SolidHull) -> ([f32; 3], [f32; 3]) {
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        for v in &hull.vertices {
            for axis in 0..3 {
                min[axis] = min[axis].min(v[axis]);
                max[axis] = max[axis].max(v[axis]);
            }
        }
        (min, max)
    }

    #[test]
    fn a_design_file_is_measured_from_its_facet_planes_without_the_preform() {
        // A preform box that is tighter than the facets along z: measuring every plane
        // would give a 1 mm long stone instead of the facets' 3 mm.
        let mut planes = box_planes([0.5, 0.5, 0.5]);
        let preform_plane_count = planes.len();
        planes.extend(box_planes([0.5, 0.25, 1.5]));

        let (extents, source, hull) = measure_planes(
            CataloguePlanesSource::DesignFile,
            preform_plane_count,
            &planes,
        );
        assert_eq!(source, SolidExtentsSource::DesignFile);
        let extents = extents.expect("the facet planes close");
        for (got, want) in [
            (extents.width_caliper, 1.0),
            (extents.length_caliper, 3.0),
            (extents.height, 0.5),
            (extents.volume, 1.5),
        ] {
            assert!((got - want).abs() < 1e-6, "{got} vs {want}");
        }

        let hull = hull.expect("a design file keeps its hull");
        assert_eq!(hull.vertices.len(), 8);
        let (min, max) = hull_bounds(&hull);
        for (axis, half) in [0.5_f32, 0.25, 1.5].into_iter().enumerate() {
            assert!(
                (max[axis] - half).abs() < 1e-6,
                "axis {axis} max {}",
                max[axis]
            );
            assert!(
                (min[axis] + half).abs() < 1e-6,
                "axis {axis} min {}",
                min[axis]
            );
        }
    }

    #[test]
    fn hull_vertices_are_the_solids_vertices_narrowed_to_f32() {
        let half = 0.1_f32;
        let planes = box_planes([half; 3]);
        let (_, _, hull) = measure_planes(CataloguePlanesSource::DesignFile, 0, &planes);
        let hull = hull.expect("a hull");
        assert_eq!(hull.vertices.len(), 8);
        for v in &hull.vertices {
            for c in v {
                assert_eq!(
                    c.abs(),
                    half,
                    "each coordinate is the plane offset, as an f32"
                );
            }
        }
    }

    #[test]
    fn facet_planes_that_do_not_close_leave_the_design_unbounded_even_with_a_closed_preform() {
        let mut planes = box_planes([0.5, 0.5, 0.5]);
        let preform_plane_count = planes.len();
        // Only the x and y faces: open along z.
        planes.extend(box_planes([0.5, 0.25, 1.5]).into_iter().take(4));

        let (extents, source, hull) = measure_planes(
            CataloguePlanesSource::DesignFile,
            preform_plane_count,
            &planes,
        );
        assert_eq!(source, SolidExtentsSource::Unbounded);
        assert!(extents.is_none() && hull.is_none());
    }

    #[test]
    fn a_preform_count_past_the_end_measures_nothing_instead_of_panicking() {
        let planes = box_planes([0.5, 0.5, 0.5]);
        let (extents, source, hull) =
            measure_planes(CataloguePlanesSource::DesignFile, 100, &planes);
        assert_eq!(source, SolidExtentsSource::Unbounded);
        assert!(extents.is_none() && hull.is_none());
    }

    #[test]
    fn angle_table_planes_are_measured_whole_and_never_get_a_hull() {
        // The preform count means nothing for the angle table's synthetic planes.
        let planes = box_planes([0.5, 0.25, 1.5]);
        let (extents, source, hull) = measure_planes(CataloguePlanesSource::AngleTable, 3, &planes);
        assert_eq!(source, SolidExtentsSource::AngleTable);
        assert!(hull.is_none());
        let extents = extents.expect("the box closes");
        assert!((extents.volume - 1.5).abs() < 1e-6);
    }

    fn extents() -> SolidExtents {
        SolidExtents {
            width_caliper: 1.0,
            length_caliper: 1.5,
            width_axis: 1.0,
            length_axis: 1.5,
            height: 0.7,
            volume: 0.9,
        }
    }

    fn row(source: SolidExtentsSource, usable: bool) -> StoredSolidExtents {
        StoredSolidExtents {
            extents: usable.then(extents),
            source,
        }
    }

    fn hull() -> SolidHull {
        SolidHull {
            vertices: vec![[0.0, 0.0, 0.0]; 4],
        }
    }

    #[test]
    fn a_design_without_any_row_needs_a_full_scan() {
        let (missing, outlines_only) =
            ids_needing_scan(&[1, 2], &BTreeMap::new(), &BTreeMap::new());
        assert_eq!(missing, vec![1, 2]);
        assert!(!outlines_only);
    }

    #[test]
    fn a_design_file_row_without_a_hull_needs_an_outline_scan() {
        let extents_map = BTreeMap::from([(7, row(SolidExtentsSource::DesignFile, true))]);
        let (missing, outlines_only) = ids_needing_scan(&[7], &extents_map, &BTreeMap::new());
        assert_eq!(missing, vec![7]);
        assert!(outlines_only);

        let hulls = BTreeMap::from([(7, hull())]);
        let (missing, outlines_only) = ids_needing_scan(&[7], &extents_map, &hulls);
        assert_eq!(missing, Vec::<i64>::new());
        assert!(!outlines_only, "nothing to scan is not an outline scan");
    }

    #[test]
    fn unbounded_and_angle_table_rows_never_get_a_hull_so_are_not_rescanned() {
        let extents_map = BTreeMap::from([
            (1, row(SolidExtentsSource::Unbounded, false)),
            (2, row(SolidExtentsSource::AngleTable, true)),
            (3, row(SolidExtentsSource::DesignFile, false)),
        ]);
        let (missing, outlines_only) = ids_needing_scan(&[1, 2, 3], &extents_map, &BTreeMap::new());
        assert_eq!(missing, Vec::<i64>::new());
        assert!(!outlines_only);
    }

    #[test]
    fn a_mixed_set_is_a_full_scan_of_exactly_the_missing_ids() {
        let extents_map = BTreeMap::from([
            (1, row(SolidExtentsSource::DesignFile, true)),
            (2, row(SolidExtentsSource::DesignFile, true)),
            (3, row(SolidExtentsSource::Unbounded, false)),
        ]);
        let hulls = BTreeMap::from([(1, hull())]);
        let (missing, outlines_only) = ids_needing_scan(&[1, 2, 3, 4], &extents_map, &hulls);
        assert_eq!(missing, vec![2, 4]);
        assert!(!outlines_only, "id 4 has no extents at all");
    }
}
