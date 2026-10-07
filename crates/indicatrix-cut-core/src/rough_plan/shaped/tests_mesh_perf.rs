//! Owner timing run for the rough mesh at scale: how the cost of a scan grows with its
//! triangle count, to set `MAX_MESH_TRIANGLES`. Run it on a release build, never in the
//! normal test run:
//!
//! ```text
//! cargo test -p indicatrix-cut-core --release -- --ignored mesh_scaling_timings --nocapture
//! ```
//!
//! Per fixture the run prints, in this order:
//!
//! - `triangles`, `vertices`: the input as given.
//! - The import phases. `RoughMesh::new` split into the weld and closed check, the
//!   self-intersection check and the orientation plus BVH; the exact hull of the mesh's
//!   welded vertices (`hull_triangles`, the step the quick hull speeds up above 4,096
//!   points); the simplification to the outline (`simplified_outer_planes`, at
//!   `OUTLINE_PLANES` = 64 planes); then `import_mesh` as the total, with the outline's
//!   plane count and its note.
//! - `ShapedCtx::new`, and the classification sweep over the default grid.
//! - The small grid (3 x 3 x 3) for K = 1 and K = 10: the serial clipped table, the same
//!   table from `build_clipped_table_jobs` on all available lanes (asserted equal to the
//!   serial one bit for bit), the share of PARTIAL entries, the dynamic programs over all
//!   cut orders and the layout count. Every row that imported must plan at least one
//!   layout: "0 layouts" is a product bug, and the test fails on it.
//! - The default-grid projection for K = 10 (the grid the app really uses, about two
//!   million range entries): a deterministic sample of a-range jobs is built, its PARTIAL
//!   entries are counted, and `entries x partial share x mean partial cost` is printed as
//!   serial seconds and as seconds over the lanes. The projection ignores the lane
//!   scheduling tail and the DP, so read it as a floor.
//!
//! Reading the verdicts. `>> ... small-grid plan` is the decision-D5 line: import plus
//! context plus the serial K = 10 small-grid table plus the DP, against
//! [`TARGET_SECONDS`]. `>> default grid projected` is the app's reality: how long one plan
//! of this scan takes on the default grid. "The new cap" is the largest triangle count
//! whose rows are all UNDER on the small-grid line and whose projected default-grid time on
//! the owner's lane count is acceptable; the lead sets `MAX_MESH_TRIANGLES` from those two.
//!
//! The mesh builder refuses more than `MAX_MESH_TRIANGLES` triangles, so a fixture above the
//! cap prints "over the cap" and its row stops. To measure above it, raise the constant in
//! `shape/mesh.rs` for the run (for example to `500_000`) and put it back afterwards. The
//! largest fixtures are 327,680 (pebble, level 7) and 458,752 (noisy C, level 7) triangles;
//! with the quick hull their imports take seconds.

use std::time::{Duration, Instant};

use glam::DVec3;

use super::{
    clip::{
        BuildClipParams, CLASS_EXTERIOR, CLASS_INTERIOR, CLASS_PARTIAL, ClippedTable, a_range_jobs,
        build_clipped_a_range, build_clipped_table, build_clipped_table_jobs, classify_box_in,
    },
    ctx::ShapedCtx,
    dp::plan_shaped_for_order,
    grid::{ShapedGrid, piece_count},
    tests_dp::grid_with_cells,
};
use crate::rough_plan::{
    CandidateDesign, CutOrder, PlanSettings, RoughModel, import_mesh, pareto_front,
    shape::{
        MeshError, RoughBase, RoughMesh,
        hull::{OUTLINE_PLANES, hull_triangles, outline_note, simplified_outer_planes},
        mesh_fixture::{noisy_c_scan, pebble_scan},
    },
    tests::{box_design, settings_with},
};

/// The budget for import plus a K = 10 plan on the small grid, in seconds.
const TARGET_SECONDS: f64 = 10.0;
/// The most boxes the classification sweep visits (evenly spread over the default grid).
const SWEEP_BOXES: usize = 50_000;
/// The most a-range jobs the default-grid projection builds.
const SAMPLE_JOBS: usize = 12;
/// The projection stops starting new sample jobs once the sample has taken this long.
const SAMPLE_BUDGET: Duration = Duration::from_secs(20);

/// The result of `work` and how long it took.
fn timed<T>(work: impl FnOnce() -> T) -> (T, Duration) {
    let start = Instant::now();
    let value = work();
    (value, start.elapsed())
}

fn designs() -> Vec<CandidateDesign> {
    vec![
        box_design(1),
        CandidateDesign {
            entry_id: 2,
            width: 1.0,
            length: 1.6,
            height: 0.6,
            volume: 0.5,
        },
    ]
}

/// The lanes the run uses: every available core.
fn lane_count() -> usize {
    std::thread::available_parallelism().map_or(1, usize::from)
}

/// The size table of `grid` for `front`.
fn size_table_of(
    grid: &ShapedGrid,
    front: &[CandidateDesign],
    settings: &PlanSettings,
) -> crate::rough_plan::PieceTable {
    grid.size_table(settings, front, &mut |_| true)
        .expect("size table")
}

/// How many entries of `table` have each class: `(interior, exterior, partial)`.
fn class_counts(classes: &[u8]) -> (usize, usize, usize) {
    let mut counts = (0, 0, 0);
    for &class in classes {
        match class {
            CLASS_INTERIOR => counts.0 += 1,
            CLASS_EXTERIOR => counts.1 += 1,
            CLASS_PARTIAL => counts.2 += 1,
            _ => {}
        }
    }
    counts
}

/// One `classify_box_in` over a spread of the boxes of `grid`'s cells; the number of boxes
/// visited and the time.
fn classify_sweep(ctx: &ShapedCtx, grid: &ShapedGrid) -> (usize, Duration) {
    let total = grid.cells.iter().product::<usize>();
    let step = (total / SWEEP_BOXES).max(1);
    let mut violated = Vec::new();
    let mut visited = 0;
    let ((), elapsed) = timed(|| {
        for index in (0..total).step_by(step) {
            let at = [
                index % grid.cells[0],
                index / grid.cells[0] % grid.cells[1],
                index / (grid.cells[0] * grid.cells[1]),
            ];
            let min: [f64; 3] =
                std::array::from_fn(|i| (at[i] as f64).mul_add(grid.unit[i], grid.origin_mm[i]));
            let max: [f64; 3] = std::array::from_fn(|i| min[i] + grid.unit[i]);
            classify_box_in(min, max, &ctx.non_box, ctx.fit_mesh(), &mut violated);
            visited += 1;
        }
    });
    (visited, elapsed)
}

/// What one small-grid plan took and found.
struct SmallGrid {
    ctx: Duration,
    serial: Duration,
    jobs: Duration,
    dp: Duration,
    layouts: usize,
    /// `(interior, exterior, partial)` entries of the table.
    classes: (usize, usize, usize),
}

/// Plans `count` stones of `model` on a 3 x 3 x 3 grid over every cut order: the context,
/// the serial clipped table, the same table in a-range jobs on `lanes` (asserted bitwise
/// equal), and the dynamic programs.
fn small_grid_plan(model: &RoughModel, count: u8, lanes: usize) -> SmallGrid {
    let settings = settings_with(count, 0.3, 0.2, 1.0);
    let (ctx, t_ctx) = timed(|| ShapedCtx::new(model, &settings).expect("ctx"));
    let grid = grid_with_cells(&ctx, &settings, [3, 3, 3]);
    let front = pareto_front(&designs());
    let size_table = size_table_of(&grid, &front, &settings);
    let params = BuildClipParams {
        grid: &grid,
        front: &front,
        non_box_planes: &ctx.non_box,
        mesh: ctx.mesh.as_deref(),
        size_table: &size_table,
        settings: &settings,
        slice: 0..grid.cells[0],
        cached_classes: None,
    };
    let (serial, t_serial) = timed(|| build_clipped_table(&params, &mut |_| true));
    let serial: ClippedTable = serial.expect("serial clipped table");
    let (jobs, t_jobs) = timed(|| build_clipped_table_jobs(&params, lanes, &mut |_| true));
    let jobs = jobs.expect("job clipped table");
    assert_eq!(
        jobs, serial,
        "the job build on {lanes} lanes must be the serial table, bit for bit"
    );
    let (layouts, t_dp) = timed(|| {
        CutOrder::ALL
            .into_iter()
            .map(|order| {
                plan_shaped_for_order(&grid, &serial, &ctx, &front, order, &settings, &mut |_| {
                    true
                })
                .expect("not cancelled")
                .len()
            })
            .sum::<usize>()
    });
    SmallGrid {
        ctx: t_ctx,
        serial: t_serial,
        jobs: t_jobs,
        dp: t_dp,
        layouts,
        classes: class_counts(&serial.classes),
    }
}

/// Prints the projected cost of the default K = 10 grid from a strided sample of its a-range
/// jobs, and returns `(serial seconds, parallel seconds)`; `None` when the sample found no
/// PARTIAL entry to price.
fn project_default_grid(model: &RoughModel, lanes: usize) -> Option<(f64, f64)> {
    let settings = settings_with(10, 0.3, 0.2, 1.0);
    let ctx = ShapedCtx::new(model, &settings).expect("ctx");
    let grid = ctx.choose_grid(&settings);
    let front = pareto_front(&designs());
    let entries = piece_count(grid.cells);
    let (size_table, t_size) = timed(|| size_table_of(&grid, &front, &settings));
    let jobs = a_range_jobs(&grid);
    let per_a = grid.range_count(1) * grid.range_count(2);
    let step = (jobs.len() / SAMPLE_JOBS).max(1);
    println!(
        "   default K = 10 grid {:?}: {entries} range entries in {} a-range jobs of {per_a} \
         entries (size table {t_size:?})",
        grid.cells,
        jobs.len()
    );

    let started = Instant::now();
    let (mut sampled, mut taken) = (0_usize, 0_usize);
    let (mut interior, mut exterior, mut partial) = (0_usize, 0_usize, 0_usize);
    let mut elapsed = Duration::ZERO;
    for &(a0, a1) in jobs.iter().step_by(step).take(SAMPLE_JOBS) {
        if started.elapsed() > SAMPLE_BUDGET {
            println!(
                "   (sample stopped after {taken} jobs: it passed {} s)",
                SAMPLE_BUDGET.as_secs()
            );
            break;
        }
        let params = BuildClipParams {
            grid: &grid,
            front: &front,
            non_box_planes: &ctx.non_box,
            mesh: ctx.mesh.as_deref(),
            size_table: &size_table,
            settings: &settings,
            slice: a0..(a0 + 1),
            cached_classes: None,
        };
        let (part, t_job) = timed(|| build_clipped_a_range(&params, (a0, a1), &mut |_| true));
        let part = part.expect("a-range block");
        let (i, e, p) = class_counts(&part.classes);
        interior += i;
        exterior += e;
        partial += p;
        sampled += part.classes.len();
        elapsed += t_job;
        taken += 1;
    }
    println!(
        "   sampled {taken} jobs / {sampled} entries in {elapsed:?}: {partial} partial, \
         {interior} interior, {exterior} exterior"
    );
    if partial == 0 || sampled == 0 {
        println!("   no PARTIAL entry in the sample; nothing to project");
        return None;
    }
    let share = partial as f64 / sampled as f64;
    let cost = elapsed.as_secs_f64() / partial as f64;
    let serial = entries as f64 * share * cost;
    let parallel = serial / lanes as f64;
    println!(
        "   entries {entries}, partial share {:.1} %, mean partial cost {:.3} ms, projected \
         serial {serial:.1} s, projected over {lanes} lanes {parallel:.1} s",
        share * 100.0,
        cost * 1000.0
    );
    Some((serial, parallel))
}

fn under_over(seconds: f64) -> &'static str {
    if seconds < TARGET_SECONDS {
        "UNDER"
    } else {
        "OVER"
    }
}

/// Times and prints the phases of the mesh build, the hull and the simplification of the
/// welded vertices; `false` when the mesh builder refuses the fixture.
fn print_import_phases(points: &[DVec3], tris: &[[u32; 3]]) -> bool {
    let (built, phases) = RoughMesh::new_timed(points, tris);
    let t_new: Duration = phases.iter().map(|&(_, t)| t).sum();
    let mesh = match built {
        Ok(mesh) => mesh,
        Err(MeshError::TooManyTriangles(n)) => {
            println!("   over the cap: {n} triangles are refused (see the module docs)");
            return false;
        }
        Err(err) => {
            println!("   the mesh builder refuses it: {err}");
            return false;
        }
    };
    let split: Vec<String> = phases
        .iter()
        .map(|(phase, t)| format!("{phase} {t:?}"))
        .collect();
    println!(
        "   RoughMesh::new {t_new:?} ({}); {} vertices after the weld",
        split.join(", "),
        mesh.vertices().len()
    );
    let (hull, t_hull) = timed(|| hull_triangles(mesh.vertices()));
    match hull {
        Ok((faces, eps)) => {
            println!(
                "   hull of the welded vertices: {t_hull:?} ({} faces)",
                faces.len()
            );
            let (planes, t_simplify) =
                timed(|| simplified_outer_planes(mesh.vertices(), &faces, OUTLINE_PLANES, eps));
            println!(
                "   simplified_outer_planes to at most {OUTLINE_PLANES}: {t_simplify:?} \
                 ({} planes)",
                planes.len()
            );
        }
        Err(err) => println!("   hull of the welded vertices: {t_hull:?}, refused: {err}"),
    }
    true
}

/// Prints the rows of one fixture, and stops early (saying why) when the mesh builder or the
/// import refuses it.
fn measure(name: &str, points: &[DVec3], tris: &[[u32; 3]], lanes: usize) {
    println!(
        "== {name}: {} triangles, {} vertices",
        tris.len(),
        points.len()
    );
    if !print_import_phases(points, tris) {
        return;
    }
    let (imported, t_import) = timed(|| import_mesh(points, tris));
    let (base, _) = match imported {
        Ok(done) => done,
        Err(err) => {
            println!("   import_mesh refuses it: {err}");
            return;
        }
    };
    let outline = base.to_halfspaces(true).map_or(0, |planes| planes.len());
    let note = match &base {
        RoughBase::Hull { id, .. } => outline_note(*id),
        _ => None,
    };
    println!("   import_mesh total: {t_import:?}; outline {outline} planes, note: {note:?}");
    let model = RoughModel::new(base, Vec::new());
    if model.mesh().is_none() {
        println!("   the rough kept no mesh; nothing more to time");
        return;
    }

    let sweep_settings = settings_with(10, 0.3, 0.2, 1.0);
    let (ctx, t_ctx) = timed(|| ShapedCtx::new(&model, &sweep_settings).expect("ctx"));
    println!("   ShapedCtx::new: {t_ctx:?}");
    let default_grid = ctx.choose_grid(&sweep_settings);
    let (boxes, t_sweep) = classify_sweep(&ctx, &default_grid);
    println!(
        "   classify_box_in over the default K = 10 grid {:?}: {boxes} boxes in {t_sweep:?} \
         ({:?} per box)",
        default_grid.cells,
        t_sweep / u32::try_from(boxes.max(1)).unwrap_or(u32::MAX)
    );

    let mut small_total = None;
    for count in [1_u8, 10] {
        let run = small_grid_plan(&model, count, lanes);
        let (interior, exterior, partial) = run.classes;
        let entries = interior + exterior + partial;
        println!(
            "   K = {count} on a 3x3x3 grid ({entries} entries, {:.0} % partial): ctx {:?}, \
             clipped table serial {:?}, in jobs on {lanes} lanes {:?} (equal to the serial \
             table), DP over all orders {:?}, {} layouts",
            100.0 * partial as f64 / entries.max(1) as f64,
            run.ctx,
            run.serial,
            run.jobs,
            run.dp,
            run.layouts
        );
        assert!(
            run.layouts > 0,
            "{name}: K = {count} planned 0 layouts on the small grid"
        );
        if count == 10 {
            small_total = Some(t_import + run.ctx + run.serial + run.dp);
        }
    }
    if let Some(total) = small_total {
        println!(
            "   >> {} triangles: import + ctx + serial K = 10 small-grid table + DP = {total:?}, \
             {} the {TARGET_SECONDS} s target",
            tris.len(),
            under_over(total.as_secs_f64())
        );
    }
    match project_default_grid(&model, lanes) {
        Some((serial, parallel)) => println!(
            "   >> default grid projected: {serial:.1} s serial, {parallel:.1} s on {lanes} lanes"
        ),
        None => println!("   >> default grid projected: no estimate"),
    }
}

#[test]
#[ignore = "owner timing run on a release build"]
fn mesh_scaling_timings() {
    let lanes = lane_count();
    println!(
        "MAX_MESH_TRIANGLES = {}, OUTLINE_PLANES = {OUTLINE_PLANES}, lanes = {lanes}",
        crate::rough_plan::MAX_MESH_TRIANGLES
    );
    // The noisy C-shape: a notch thicket on a cube, 28 * 4^levels triangles (7,168 at 4,
    // 28,672 at 5, 114,688 at 6, 458,752 at 7), jittered by 0.3 of the vertex spacing so
    // the walls do not fold (see `noisy_c_scan`).
    for levels in 4..=7 {
        let (points, tris) = noisy_c_scan(levels);
        measure(
            &format!("noisy C-scan, level {levels}"),
            &points,
            &tris,
            lanes,
        );
    }
    // The smooth pebble scan, a convex-ish ellipsoid whose outline is simplified:
    // 20 * 4^levels triangles (5,120 at 4, 20,480 at 5, 81,920 at 6, 327,680 at 7). Level 7
    // is seconds with the quick hull; above the mesh cap it needs the raised constant.
    for levels in 4..=7 {
        let (points, tris) = pebble_scan(levels, 1);
        measure(
            &format!("pebble scan, level {levels}"),
            &points,
            &tris,
            lanes,
        );
    }
}
