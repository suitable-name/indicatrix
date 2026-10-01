//! The driver against the core's sequential `plan()`: same layouts, bit for bit, for any
//! lane count, on a plain block, a cut block, a cylinder and a cut pebble; the parallel
//! stages on their own; and the cancel and panic paths.

use super::{
    drive::drive,
    stages::{FitJob, fit_single_stones_parallel, refine_parallel, run_orders},
    tracker::{Note, Progress},
};
use indicatrix_cut_core::rough_plan::{
    Axis, BoxFace, CandidateDesign, CutOrder, CutPlan, DesignHull, PlacedStone, PlanInput,
    PlanProgress, PlanSettings, RoughBase, RoughCut, RoughLayout, RoughModel, StonePose,
    fit_single_stones, plan,
};
use std::{
    collections::BTreeSet,
    panic::{AssertUnwindSafe, catch_unwind, resume_unwind},
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
};

/// The lane counts every equality below is checked for: one lane, fewer and more lanes
/// than the six cut orders, and counts that divide nothing evenly.
const LANES: [usize; 5] = [1, 2, 3, 5, 7];

/// A sink that never reports and can be told to cancel after some events. Once
/// [`Progress::abort`] was called every further event asks to stop.
#[derive(Default)]
struct Silent {
    events: AtomicUsize,
    stop_after: Option<usize>,
    aborted: AtomicBool,
}

impl Progress for Silent {
    fn event(&self, _event: PlanProgress) -> bool {
        let seen = self.events.fetch_add(1, Ordering::Relaxed) + 1;
        !self.aborted.load(Ordering::Relaxed) && self.stop_after.is_none_or(|limit| seen <= limit)
    }

    fn note(&self, _note: Note) {}

    fn abort(&self) {
        self.aborted.store(true, Ordering::Relaxed);
    }
}

impl Silent {
    fn stopping_after(events: usize) -> Self {
        Self {
            stop_after: Some(events),
            ..Self::default()
        }
    }

    fn events(&self) -> usize {
        self.events.load(Ordering::Relaxed)
    }
}

/// Appends the bits of `values` to `bits`.
fn push_floats(bits: &mut Vec<u64>, values: &[f64]) {
    bits.extend(values.iter().map(|value| value.to_bits()));
}

/// Every number of `layout` as its bits, every discrete choice as an integer, in a fixed
/// order: two layouts are the same exactly when their flattenings are equal. Unlike `==`
/// it tells `0.0` from `-0.0` and never calls a NaN different from itself.
pub(super) fn layout_bits(layout: &RoughLayout) -> Vec<u64> {
    let mut bits = vec![
        CutOrder::ALL
            .iter()
            .position(|&order| order == layout.cut_order)
            .map_or(u64::MAX, |index| index as u64),
        u64::from(layout.exact_fit),
        layout.stones.len() as u64,
    ];
    push_floats(
        &mut bits,
        &[
            layout.total_carat,
            layout.total_volume_mm3,
            layout.yield_fraction,
        ],
    );
    for stone in &layout.stones {
        bits.push(stone.entry_id as u64);
        bits.push(match stone.table_axis {
            Axis::X => 0,
            Axis::Y => 1,
            Axis::Z => 2,
        });
        push_floats(&mut bits, &stone.piece_origin_mm);
        push_floats(&mut bits, &stone.piece_size_mm);
        push_floats(&mut bits, &stone.stone_size_mm);
        push_floats(&mut bits, &[stone.carat, stone.volume_mm3]);
        push_floats(&mut bits, &stone.pose.center_mm);
        for axis in &stone.pose.axes {
            push_floats(&mut bits, axis);
        }
        push_floats(&mut bits, &[stone.pose.mm_per_unit]);
    }
    bits.push(layout.cut_plan.slabs.len() as u64);
    for slab in &layout.cut_plan.slabs {
        push_floats(&mut bits, &[slab.thickness_mm]);
        bits.push(slab.bars.len() as u64);
        for bar in &slab.bars {
            push_floats(&mut bits, &[bar.width_mm]);
            bits.push(bar.pieces_mm.len() as u64);
            push_floats(&mut bits, &bar.pieces_mm);
        }
    }
    bits
}

/// [`layout_bits`] of every layout of a list.
fn list_bits(layouts: &[RoughLayout]) -> Vec<Vec<u64>> {
    layouts.iter().map(layout_bits).collect()
}

/// A box design of `[width, height, length]` and its hull in the caliper frame.
fn box_design(entry_id: i64, [w, h, l]: [f64; 3]) -> (CandidateDesign, DesignHull) {
    let mut vertices = Vec::new();
    for sx in [-0.5, 0.5] {
        for sy in [-0.5, 0.5] {
            for sz in [-0.5, 0.5] {
                vertices.push([sx * w, sy * h, sz * l]);
            }
        }
    }
    let volume = w * h * l;
    (
        CandidateDesign {
            entry_id,
            width: w,
            length: l,
            height: h,
            volume,
        },
        DesignHull {
            entry_id,
            vertices,
            volume,
            width: w,
        },
    )
}

/// An octahedron with half extents `[a, b, c]` along width, height and length.
fn octahedron(entry_id: i64, [a, b, c]: [f64; 3]) -> (CandidateDesign, DesignHull) {
    let vertices = vec![
        [a, 0.0, 0.0],
        [-a, 0.0, 0.0],
        [0.0, b, 0.0],
        [0.0, -b, 0.0],
        [0.0, 0.0, c],
        [0.0, 0.0, -c],
    ];
    let volume = 4.0 / 3.0 * a * b * c;
    (
        CandidateDesign {
            entry_id,
            width: 2.0 * a,
            length: 2.0 * c,
            height: 2.0 * b,
            volume,
        },
        DesignHull {
            entry_id,
            vertices,
            volume,
            width: 2.0 * a,
        },
    )
}

/// Everything one plan needs, owned.
struct Fixture {
    model: RoughModel,
    settings: PlanSettings,
    designs: Vec<CandidateDesign>,
    hulls: Vec<DesignHull>,
}

impl Fixture {
    fn new(model: RoughModel, count: u8, catalogue: Vec<(CandidateDesign, DesignHull)>) -> Self {
        let (designs, hulls) = catalogue.into_iter().unzip();
        Self {
            model,
            settings: PlanSettings {
                count,
                ..PlanSettings::default()
            },
            designs,
            hulls,
        }
    }

    fn input(&self) -> PlanInput<'_> {
        PlanInput {
            model: &self.model,
            settings: &self.settings,
            designs: &self.designs,
            hulls: &self.hulls,
        }
    }
}

fn block(x_mm: f64, y_mm: f64, z_mm: f64) -> RoughBase {
    RoughBase::Block { x_mm, y_mm, z_mm }
}

/// Three designs to choose from, for the fixtures that plan several stones.
fn catalogue() -> Vec<(CandidateDesign, DesignHull)> {
    vec![
        box_design(1, [1.0, 0.5, 1.5]),
        box_design(2, [1.0, 0.7, 1.0]),
        octahedron(3, [0.5, 0.4, 0.6]),
    ]
}

fn plain_block() -> Fixture {
    Fixture::new(
        RoughModel::new(block(9.0, 7.0, 5.0), Vec::new()),
        3,
        catalogue(),
    )
}

fn cut_edge() -> RoughCut {
    RoughCut::Edge {
        faces: [BoxFace::Top, BoxFace::Front],
        setbacks_mm: [2.0, 2.0],
    }
}

fn cut_block() -> Fixture {
    Fixture::new(
        RoughModel::new(block(8.0, 6.0, 5.0), vec![cut_edge()]),
        1,
        vec![
            box_design(1, [1.0, 0.5, 1.5]),
            octahedron(3, [0.5, 0.4, 0.6]),
        ],
    )
}

/// The cut block planning up to six stones of three designs.
fn cut_block_of_six() -> Fixture {
    Fixture::new(
        RoughModel::new(block(8.0, 6.0, 5.0), vec![cut_edge()]),
        6,
        catalogue(),
    )
}

fn cylinder_base() -> RoughBase {
    RoughBase::Cylinder {
        diameter_mm: 6.0,
        length_mm: 8.0,
        axis: Axis::Y,
    }
}

fn cylinder() -> Fixture {
    Fixture::new(
        RoughModel::new(cylinder_base(), Vec::new()),
        1,
        vec![
            box_design(2, [1.0, 0.7, 1.0]),
            octahedron(3, [0.5, 0.4, 0.6]),
        ],
    )
}

/// The cylinder planning up to six stones of three designs.
fn cylinder_of_six() -> Fixture {
    Fixture::new(RoughModel::new(cylinder_base(), Vec::new()), 6, catalogue())
}

/// A pebble with its top sawn flat, planning up to six stones of three designs.
fn cut_pebble_of_six() -> Fixture {
    let pebble = RoughBase::Pebble {
        x_mm: 7.0,
        y_mm: 5.0,
        z_mm: 6.0,
    };
    let saw = RoughCut::Face {
        normal: [0.0, 1.0, 0.0],
        depth_mm: 1.0,
    };
    Fixture::new(RoughModel::new(pebble, vec![saw]), 6, catalogue())
}

/// The result of a scoped plan thread; a panic in it is re-raised with its own message.
fn joined<T>(handle: thread::ScopedJoinHandle<'_, T>) -> T {
    handle
        .join()
        .unwrap_or_else(|payload| resume_unwind(payload))
}

/// Runs the core's own sequential plan and the driver on every lane count of [`LANES`],
/// all at once on scoped threads, and checks that every driver result equals the
/// sequential one bit for bit. The plans are independent and deterministic, so running
/// them side by side changes only the wall time. The drivers are spawned through the
/// eager array `map`, so every thread is running before the first is joined; a lazy
/// iterator would run them one after another.
fn assert_lanes_match_the_core(fixture: &Fixture) {
    let input = &fixture.input();
    let (expected, driven) = thread::scope(|scope| {
        let sequential = scope.spawn(move || plan(input, &mut |_| true));
        let drivers =
            LANES.map(|lanes| scope.spawn(move || drive(input, lanes, &Silent::default())));
        (joined(sequential), drivers.map(joined))
    });
    let expected = expected.expect("the sequential plan finishes");
    assert!(!expected.is_empty(), "the fixture must be plannable");
    let expected = list_bits(&expected);
    for (lanes, got) in LANES.into_iter().zip(driven) {
        let got = got.expect("the driver finishes");
        assert_eq!(list_bits(&got), expected, "lanes = {lanes}");
    }
}

#[test]
fn a_plain_block_plans_like_the_core_for_every_lane_count() {
    assert_lanes_match_the_core(&plain_block());
}

#[test]
fn a_cut_block_plans_like_the_core_for_every_lane_count() {
    assert_lanes_match_the_core(&cut_block());
}

#[test]
fn a_cut_block_of_six_stones_plans_like_the_core_for_every_lane_count() {
    assert_lanes_match_the_core(&cut_block_of_six());
}

#[test]
fn a_cylinder_plans_like_the_core_for_every_lane_count() {
    assert_lanes_match_the_core(&cylinder());
}

#[test]
fn a_cylinder_of_six_stones_plans_like_the_core_for_every_lane_count() {
    assert_lanes_match_the_core(&cylinder_of_six());
}

#[test]
fn a_pebble_with_a_sawn_face_plans_like_the_core_for_every_lane_count() {
    assert_lanes_match_the_core(&cut_pebble_of_six());
}

#[test]
fn the_layout_flattening_tells_what_equality_would_not() {
    let fixture = plain_block();
    let layouts = drive(&fixture.input(), 1, &Silent::default()).expect("finishes");
    let first = layouts.first().expect("a layout");
    assert_eq!(layout_bits(first), layout_bits(&first.clone()));

    // A difference in the last bit of one figure is a difference.
    let mut nudged = first.clone();
    nudged.total_volume_mm3 = f64::from_bits(nudged.total_volume_mm3.to_bits() + 1);
    assert_ne!(layout_bits(first), layout_bits(&nudged));

    // `0.0` and `-0.0` compare equal as numbers but are different bits.
    let mut positive_zero = first.clone();
    positive_zero.yield_fraction = 0.0;
    let mut negative_zero = first.clone();
    negative_zero.yield_fraction = -0.0;
    assert_eq!(positive_zero.yield_fraction, negative_zero.yield_fraction);
    assert_ne!(layout_bits(&positive_zero), layout_bits(&negative_zero));
}

#[test]
fn the_chunked_single_stone_fit_equals_the_core_fit_for_any_lane_count() {
    let fixture = plain_block();
    let settings = &fixture.settings;
    let inset = settings.skin_mm + settings.allowance_mm;
    let region = fixture.model.usable_halfspaces(inset).expect("valid model");
    let coarse = fixture
        .model
        .coarse_usable_halfspaces(inset)
        .expect("valid model");
    for keep in [1, 2, 10] {
        let expected = fit_single_stones(
            &region,
            &coarse,
            &fixture.hulls,
            settings,
            keep,
            &mut |_| true,
        )
        .expect("the core fit finishes");
        assert_eq!(expected.len(), keep.min(fixture.hulls.len()));
        assert!(expected[0].volume_mm3 > 0.0);
        let job = FitJob {
            region: &region,
            coarse_region: &coarse,
            hulls: &fixture.hulls,
            settings,
            keep,
        };
        for lanes in LANES {
            let got = fit_single_stones_parallel(&job, lanes, &Silent::default())
                .expect("the chunked fit finishes");
            assert_eq!(got, expected, "keep = {keep}, lanes = {lanes}");
        }
    }
}

#[test]
fn a_fit_without_designs_or_a_keep_is_empty_and_reports_nothing() {
    let fixture = plain_block();
    let region = fixture.model.usable_halfspaces(0.2).expect("valid model");
    let progress = Silent::default();
    let none = FitJob {
        region: &region,
        coarse_region: &region,
        hulls: &[],
        settings: &fixture.settings,
        keep: 10,
    };
    assert_eq!(
        fit_single_stones_parallel(&none, 3, &progress),
        Some(Vec::new())
    );
    let keep_nothing = FitJob {
        hulls: &fixture.hulls,
        keep: 0,
        ..none
    };
    assert_eq!(
        fit_single_stones_parallel(&keep_nothing, 3, &progress),
        Some(Vec::new())
    );
    assert_eq!(progress.events(), 0);
}

/// A one-stone layout that carries `slot` as the stone's entry id, to see which slot a
/// layout came from.
fn marked_layout(slot: usize) -> RoughLayout {
    RoughLayout {
        cut_order: CutOrder::Xyz,
        stones: vec![PlacedStone {
            entry_id: slot as i64,
            piece_origin_mm: [0.0; 3],
            piece_size_mm: [1.0; 3],
            stone_size_mm: [1.0; 3],
            table_axis: Axis::Y,
            carat: 0.1,
            volume_mm3: 1.0,
            pose: StonePose {
                center_mm: [0.5; 3],
                axes: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
                mm_per_unit: 1.0,
            },
        }],
        cut_plan: CutPlan { slabs: Vec::new() },
        total_carat: 0.1,
        total_volume_mm3: 1.0,
        yield_fraction: 0.5,
        exact_fit: false,
    }
}

#[test]
fn the_refinement_returns_the_slots_in_order_for_every_lane_count() {
    for lanes in 1..=8 {
        let progress = Silent::default();
        // The early slots take longest, so they finish last.
        let got = refine_parallel(11, lanes, &progress, |slot| {
            std::thread::sleep(std::time::Duration::from_millis((11 - slot as u64) * 2));
            marked_layout(slot)
        })
        .expect("the refinement finishes");
        let slots: Vec<i64> = got.iter().map(|layout| layout.stones[0].entry_id).collect();
        assert_eq!(slots, (0..11).collect::<Vec<i64>>(), "lanes = {lanes}");
        assert_eq!(progress.events(), 11, "one Refine event per slot");
    }
}

#[test]
fn the_refinement_of_nothing_is_empty_and_a_cancel_returns_none() {
    let progress = Silent::default();
    assert_eq!(
        refine_parallel(0, 4, &progress, marked_layout),
        Some(Vec::new())
    );

    for lanes in [1, 4, 8] {
        let progress = Silent::stopping_after(3);
        let worked = AtomicUsize::new(0);
        let got = refine_parallel(20, lanes, &progress, |slot| {
            worked.fetch_add(1, Ordering::Relaxed);
            marked_layout(slot)
        });
        assert!(got.is_none(), "lanes = {lanes}");
        // Three events say go on and the fourth says stop, so exactly three slots ran.
        assert_eq!(worked.load(Ordering::Relaxed), 3, "lanes = {lanes}");
    }
}

#[test]
fn the_cut_orders_run_on_no_more_threads_than_there_are_lanes_or_orders() {
    for (lanes, most) in [(1, 1), (2, 2), (20, 6)] {
        let names = Mutex::new(BTreeSet::new());
        let layouts = run_orders(lanes, &Silent::default(), |_order, _on| {
            names.lock().expect("lock").insert(
                std::thread::current()
                    .name()
                    .unwrap_or_default()
                    .to_string(),
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
            Some(Vec::new())
        });
        assert_eq!(layouts, Some(Vec::new()), "lanes = {lanes}");
        let threads = names.lock().expect("lock").len();
        assert!(
            (1..=most).contains(&threads),
            "{threads} threads for {lanes} lanes"
        );
    }
}

#[test]
fn the_cut_orders_come_back_in_the_fixed_order_whatever_finishes_first() {
    let layouts = run_orders(6, &Silent::default(), |order, _on| {
        let index = CutOrder::ALL
            .iter()
            .position(|&candidate| candidate == order)
            .expect("a known order");
        // The early orders take longest.
        std::thread::sleep(std::time::Duration::from_millis((6 - index as u64) * 4));
        Some(vec![marked_layout(index)])
    })
    .expect("every order finishes");
    let slots: Vec<i64> = layouts.iter().map(|l| l.stones[0].entry_id).collect();
    assert_eq!(slots, vec![0, 1, 2, 3, 4, 5]);
}

#[test]
fn a_cancel_ends_every_path_after_a_few_events() {
    for fixture in [plain_block(), cut_block(), cylinder()] {
        let input = fixture.input();
        for lanes in [1, 3] {
            let progress = Silent::stopping_after(2);
            assert!(drive(&input, lanes, &progress).is_none());
            assert!(
                progress.events() < 100,
                "{} events after a cancel on the second",
                progress.events()
            );
        }
    }
}

#[test]
fn a_cancel_inside_the_fit_stops_all_lanes() {
    let fixture = plain_block();
    let region = fixture.model.usable_halfspaces(0.2).expect("valid model");
    let job = FitJob {
        region: &region,
        coarse_region: &region,
        hulls: &fixture.hulls,
        settings: &fixture.settings,
        keep: 10,
    };
    for stop_after in [1, 3, 5] {
        let progress = Silent::stopping_after(stop_after);
        assert!(fit_single_stones_parallel(&job, 3, &progress).is_none());
    }
}

/// A sink whose cut-order progress panics, as a bug inside one lane would.
#[derive(Default)]
struct PanicsInTheDp {
    aborted: AtomicBool,
}

impl Progress for PanicsInTheDp {
    fn event(&self, event: PlanProgress) -> bool {
        assert!(
            !matches!(event, PlanProgress::Dp { .. }),
            "a lane went wrong"
        );
        true
    }

    fn note(&self, _note: Note) {}

    fn abort(&self) {
        self.aborted.store(true, Ordering::Relaxed);
    }
}

#[test]
fn a_panic_in_a_lane_reaches_the_caller_and_stops_the_other_lanes() {
    let fixture = plain_block();
    let input = fixture.input();
    let progress = PanicsInTheDp::default();
    // The worker thread catches exactly this and reports an internal error.
    let outcome = catch_unwind(AssertUnwindSafe(|| drive(&input, 3, &progress)));
    assert!(outcome.is_err(), "the panic is not swallowed");
    assert!(
        progress.aborted.load(Ordering::Relaxed),
        "the abort hook ran, so sibling lanes stop at their next event"
    );
}
