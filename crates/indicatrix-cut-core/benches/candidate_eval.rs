//! The pieces of one optimizer candidate evaluation, timed alone (BLD-2; findings
//! PERF-1/PERF-2/PERF-3 in `review_perf_2026-10-12_fable.md`, section 5, table rows 1-3).
//!
//! Fixture: RBC-445 (12 tiers), the small real meet-derived design the `cost_probe` test
//! uses; the 103-tier CrackOtto-Step design appears for `solve` and the Fast objective
//! only (its Full objective and the optimizer's inner loop are minutes, not a bench).
//! Expected before any fix, 16-core AVX-512 host, release: RBC-445 `Full` 1.2-1.4 s,
//! `Fast` 2.0-2.4 ms (PERF-2: 0.4-0.65 s and 0.7-1.1 ms), mesh plus measure plus
//! manufacturability together 1.5-3 ms (PERF-3: under 1 ms), CrackOtto-Step `solve`
//! about 5.9 s.
//!
//! The fixture builder repeats `optimize/tests/fixtures.rs` (private to the crate's test
//! tree) against the public API; the RBC-445 text is copied from there.
//!
//! Run: `cargo bench -p indicatrix-cut-core --bench candidate_eval`.

use divan::black_box;
use glam::{DVec3, Vec3};
use indicatrix::{
    geometry::{
        GpuFacetPlane,
        meet_solver::{
            Block, MeetConstraint, SolvedTier, classify_blocks, meet_tier_inputs_from_asc,
        },
        stone_metrics::{SolidMetrics, SolidStatus, build_solid_mesh, measure_solid},
    },
    optics::materials::GemMaterial,
};
use indicatrix_cut_core::{
    ConstraintTier, DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2, Design, ObjectiveFidelity, PreformSpec,
    ScheduleMeta, check_manufacturability, evaluate_objective,
    manufacturability::check_manufacturability_with_geometry,
};

fn main() {
    divan::main();
}

/// "RBC-445" (PC 13.156), 12 tiers; byte-identical to the cost probe's fixture.
const RBC_445: &str = "GemCad 5.0\ng 96 0.0\ny 3 y\nI 1.54\n\
     H PC 13.156  RBC-445\n\
     H Richard B Conley, Facets, Oct 2013 p10\n\
     a -50.400000 0.61624001 92 n 1 68 60 36 28 4 G TCP\n\
     a -48.900000 0.63552822 88 n 2 72 56 40 24 8 G TCP\n\
     a -90.000000 0.91252100 92 n 3 68 60 36 28 4\n\
     a -90.000000 0.96225045 88 n 4 72 56 40 24 8\n\
     a -47.200000 0.59908304 95 n 5 65 63 33 31 1\n\
     a -43.000000 0.64485570 86 n 6 74 54 42 22 10 G PCP\n\
     a -47.266965 0.63949242 87 n 7 73 55 41 23 9\n\
     a 31.000000 0.62509296 4 n A 28 36 60 68 92\n\
     a 29.000000 0.62477630 8 n B 24 40 56 72 88\n\
     a 28.100000 0.60078994 2 n C 30 34 62 66 94\n\
     a 20.940747 0.56272062 14 n D 18 46 50 78 82\n\
     a 0.000000 0.40674031 96 n E\n";

/// The 103-tier design the cost probe measures its 5.9 s solve against.
const CRACKOTTO_STEP: &str = include_str!("../src/optimize_cost_probe_crackotto_step.asc");

/// A real [`Design`] with genuine meet-derived structure from raw `.asc` text: every tier
/// keeps its file's meet constraint, and each block without a scale anchor gets one
/// (its first tier's recorded mast), otherwise the solve has no scale.
fn design_from_asc(text: &str) -> Design {
    let schedule = indicatrix_formats::asc::parse_asc(text).expect("fixture must parse");
    let mut inputs = meet_tier_inputs_from_asc(&schedule);
    let blocks = classify_blocks(&inputs);
    for block in [Block::Crown, Block::Pavilion, Block::Girdle] {
        let anchored = inputs
            .iter()
            .zip(&blocks)
            .any(|(t, &b)| b == block && matches!(t.constraint, MeetConstraint::ScaleReference(_)));
        if anchored {
            continue;
        }
        if let Some(i) = (0..inputs.len()).find(|&i| blocks[i] == block) {
            inputs[i].constraint = MeetConstraint::ScaleReference(schedule.tiers[i].mast);
        }
    }
    let tiers = inputs
        .into_iter()
        .zip(&schedule.tiers)
        .map(|(input, original)| ConstraintTier {
            angle_deg: input.angle_deg,
            name: original.name.clone(),
            indices: input.indices,
            constraint: input.constraint,
            imported_meet: None,
            original_notes: None,
            detached: Vec::new(),
        })
        .collect();
    Design::new(
        PreformSpec::block(2.0, 1.0, 2.0),
        ScheduleMeta {
            gemcad_version: schedule.gemcad_version.clone(),
            gear_teeth: schedule.gear_teeth,
            gear_reference_angle: schedule.gear_reference_angle,
            symmetry_order: schedule.symmetry_order,
            mirror: schedule.mirror,
            refractive_index: schedule.refractive_index,
            headers: schedule.headers.clone(),
            footnotes: schedule.footnotes,
        },
        tiers,
    )
}

/// The solved design with the plane lists every stage works from.
struct Solved {
    design: Design,
    tiers: Vec<SolvedTier>,
    planes: Vec<(DVec3, f64)>,
    gpu_planes: Vec<GpuFacetPlane>,
}

impl Solved {
    fn new(text: &str) -> Self {
        let design = design_from_asc(text);
        let solved = design.solve().expect("fixture must solve");
        let planes = design.planes_from_solved(&solved);
        // The objective's own conversion (`optimize/objective.rs::to_gpu_planes`).
        #[expect(
            clippy::cast_possible_truncation,
            reason = "f32 is the GPU plane precision; plane normals and offsets are in the normal float range"
        )]
        let gpu_planes = planes
            .iter()
            .map(|&(n, m)| {
                GpuFacetPlane::new(Vec3::new(n.x as f32, n.y as f32, n.z as f32), -m as f32)
            })
            .collect();
        Self {
            design,
            tiers: solved,
            planes,
            gpu_planes,
        }
    }
}

/// `Design::solve`, RBC-445.
#[divan::bench(sample_count = 50)]
fn solve_rbc445(bencher: divan::Bencher) {
    let design = design_from_asc(RBC_445);
    bencher.bench_local(|| black_box(black_box(&design).solve()));
}

/// `Design::solve`, CrackOtto-Step (103 tiers; seconds per call).
#[divan::bench(sample_count = 3)]
fn solve_crackotto(bencher: divan::Bencher) {
    let design = design_from_asc(CRACKOTTO_STEP);
    bencher.bench_local(|| black_box(black_box(&design).solve()));
}

/// `Design::planes_from_solved`, RBC-445.
#[divan::bench(sample_count = 100)]
fn planes_from_solved_rbc445(bencher: divan::Bencher) {
    let fixture = Solved::new(RBC_445);
    bencher.bench_local(|| {
        black_box(black_box(&fixture.design).planes_from_solved(black_box(&fixture.tiers)))
    });
}

/// `build_solid_mesh`, RBC-445.
#[divan::bench(sample_count = 100)]
fn build_solid_mesh_rbc445(bencher: divan::Bencher) {
    let fixture = Solved::new(RBC_445);
    bencher.bench_local(|| black_box(build_solid_mesh(black_box(&fixture.planes))));
}

/// `measure_solid`, RBC-445.
#[divan::bench(sample_count = 100)]
fn measure_solid_rbc445(bencher: divan::Bencher) {
    let fixture = Solved::new(RBC_445);
    bencher.bench_local(|| black_box(measure_solid(black_box(&fixture.planes))));
}

/// `check_manufacturability_with_geometry` given the mesh and metrics, RBC-445.
#[divan::bench(sample_count = 100)]
fn check_manufacturability_with_geometry_rbc445(bencher: divan::Bencher) {
    let fixture = Solved::new(RBC_445);
    let SolidStatus::Closed(mesh) = build_solid_mesh(&fixture.planes) else {
        panic!("RBC-445 must close");
    };
    let metrics: Option<SolidMetrics> = measure_solid(&fixture.planes);
    bencher.bench_local(|| {
        black_box(check_manufacturability_with_geometry(
            black_box(&fixture.design),
            black_box(&fixture.tiers),
            Some(black_box(&mesh)),
            metrics.as_ref(),
            DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2,
        ))
    });
}

/// The public `check_manufacturability` (computes its own geometry), RBC-445, to compare
/// against the shared-geometry variant above.
#[divan::bench(sample_count = 100)]
fn check_manufacturability_rbc445(bencher: divan::Bencher) {
    let fixture = Solved::new(RBC_445);
    bencher.bench_local(|| {
        black_box(check_manufacturability(
            black_box(&fixture.design),
            black_box(&fixture.tiers),
            DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2,
        ))
    });
}

/// `evaluate_objective` at `Fast` (one canonical pose), RBC-445.
#[divan::bench(sample_count = 100)]
fn objective_fast_rbc445(bencher: divan::Bencher) {
    let fixture = Solved::new(RBC_445);
    let material = GemMaterial::diamond();
    bencher.bench_local(|| {
        black_box(evaluate_objective(
            black_box(&fixture.gpu_planes),
            black_box(&material),
            ObjectiveFidelity::Fast,
        ))
    });
}

/// `evaluate_objective` at `Fast`, CrackOtto-Step.
#[divan::bench(sample_count = 30)]
fn objective_fast_crackotto(bencher: divan::Bencher) {
    let fixture = Solved::new(CRACKOTTO_STEP);
    let material = GemMaterial::diamond();
    bencher.bench_local(|| {
        black_box(evaluate_objective(
            black_box(&fixture.gpu_planes),
            black_box(&material),
            ObjectiveFidelity::Fast,
        ))
    });
}

/// `evaluate_objective` at `Full` (the 724-pose tilt sweep), RBC-445.
#[divan::bench(sample_count = 5)]
fn objective_full_rbc445(bencher: divan::Bencher) {
    let fixture = Solved::new(RBC_445);
    let material = GemMaterial::diamond();
    bencher.bench_local(|| {
        black_box(evaluate_objective(
            black_box(&fixture.gpu_planes),
            black_box(&material),
            ObjectiveFidelity::Full,
        ))
    });
}
