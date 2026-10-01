//! Measures what the GPU furnace anchors' budgets have to cover, so that they can be set
//! from measured runs instead of by hand: the method of the CPU anchors' integration
//! test `furnace_noise_floor`, on this adapter.
//!
//! Each anchor compares one CPU and one GPU furnace mean (614,400 samples per side) with
//! the analytic radiance at a relative-error budget. Two things sit inside that budget:
//! the bounce cap, which drops a path still inside the gem with its energy, so an anchor
//! at cap 12 or 48 reads low by a systematic fraction, and the sampling noise of the two
//! means. This run repeats every anchor over [`RUNS`] independent sample ranges at its
//! own cap and at [`HIGH_CAP`], where the truncation loss is negligible, and prints per
//! side the bias (the mean signed relative error over the runs), the run-to-run noise,
//! and the standard error the runs report for their own means; then the truncation loss
//! and the budget the measurements justify (`|bias| + 4 sigma`). It fails on an energy
//! error at the high cap beyond four standard errors on either side, or on a justified
//! budget above the one in force. Without a GPU adapter it prints a note and passes.
//!
//! Ignored by default (minutes on the adapter). Run it with
//!
//! ```text
//! cargo test -p indicatrix --features gpu --lib -- furnace_budgets_from_measured_runs --ignored --nocapture
//! ```

use glam::Vec3;

use super::{
    FurnaceResult, bruted_girdle_finishes,
    furnace::{
        EDGE_ROUNDING_FURNACE_CONVERGENCE_TOLERANCE, FROSTED_FURNACE_CONVERGENCE_TOLERANCE,
        FURNACE_CONVERGENCE_TOLERANCE, FURNACE_DEFAULT_MAX_BOUNCES,
        SCATTERING_FURNACE_CONVERGENCE_TOLERANCE, SCATTERING_FURNACE_MAX_BOUNCES,
        run_furnace_for_run,
    },
    furnace_material, round_brilliant_planes,
};
use crate::{
    optics::{materials::GemMaterial, raytracer::FacetFinish},
    renderer::gpu::GpuContext,
};

/// Independent sample ranges per cap.
const RUNS: u32 = 4;
/// A bounce cap at which the truncation loss is negligible.
const HIGH_CAP: u32 = 256;
/// Standard deviations of headroom a justified budget leaves above the bias.
const SIGMA: f32 = 4.0;

/// One GPU furnace anchor: its scene and the budget it is checked against.
struct Anchor {
    /// Short name for the report.
    name: &'static str,
    /// The budget in force.
    budget: f32,
    /// The anchor's bounce cap.
    max_bounces: u32,
    /// The material.
    material: GemMaterial,
    /// Per-facet finishes, empty for all polished.
    finishes: Vec<FacetFinish>,
    /// Whether the GPU side runs with next-event estimation against the uniform map.
    use_hdr_nee: bool,
}

/// The anchors, with the caps and budgets in force.
fn anchors() -> Vec<Anchor> {
    let frosted = bruted_girdle_finishes(round_brilliant_planes().len());
    let scattering = || furnace_material().with_scattering(1.2, 0.4);
    vec![
        Anchor {
            name: "polished",
            budget: FURNACE_CONVERGENCE_TOLERANCE,
            max_bounces: FURNACE_DEFAULT_MAX_BOUNCES,
            material: furnace_material(),
            finishes: Vec::new(),
            use_hdr_nee: false,
        },
        Anchor {
            name: "frosted girdle",
            budget: FROSTED_FURNACE_CONVERGENCE_TOLERANCE,
            max_bounces: FURNACE_DEFAULT_MAX_BOUNCES,
            material: furnace_material(),
            finishes: frosted.clone(),
            use_hdr_nee: false,
        },
        Anchor {
            name: "lossless scattering",
            budget: SCATTERING_FURNACE_CONVERGENCE_TOLERANCE,
            max_bounces: SCATTERING_FURNACE_MAX_BOUNCES,
            material: scattering(),
            finishes: Vec::new(),
            use_hdr_nee: false,
        },
        Anchor {
            name: "edge rounding",
            budget: EDGE_ROUNDING_FURNACE_CONVERGENCE_TOLERANCE,
            max_bounces: FURNACE_DEFAULT_MAX_BOUNCES,
            material: furnace_material().with_edge_rounding(0.03),
            finishes: Vec::new(),
            use_hdr_nee: false,
        },
        Anchor {
            name: "lossless scattering, HDR NEE",
            budget: SCATTERING_FURNACE_CONVERGENCE_TOLERANCE,
            max_bounces: SCATTERING_FURNACE_MAX_BOUNCES,
            material: scattering(),
            finishes: Vec::new(),
            use_hdr_nee: true,
        },
        Anchor {
            name: "frosted girdle, HDR NEE",
            budget: FROSTED_FURNACE_CONVERGENCE_TOLERANCE,
            max_bounces: FURNACE_DEFAULT_MAX_BOUNCES,
            material: furnace_material(),
            finishes: frosted,
            use_hdr_nee: true,
        },
    ]
}

/// One side's figures over the runs at one cap, per XYZ component: the mean signed
/// relative error, its run-to-run standard deviation, and the mean of the relative
/// standard errors the runs report for their own means.
#[derive(Clone, Copy)]
struct Side {
    /// Mean signed relative error over the runs.
    bias: [f32; 3],
    /// Standard deviation of the signed relative error over the runs.
    noise: [f32; 3],
    /// Mean relative standard error of one run's mean.
    standard_error: [f32; 3],
}

/// Both sides at one cap.
struct Level {
    /// Bounce cap of every run at this level.
    max_bounces: u32,
    /// The CPU reference.
    cpu: Side,
    /// The GPU port.
    gpu: Side,
}

/// `value` relative to `target`, per component.
fn relative(value: Vec3, target: Vec3) -> [f32; 3] {
    let rel = |v: f32, t: f32| v / t.abs().max(1e-6);
    [
        rel(value.x, target.x),
        rel(value.y, target.y),
        rel(value.z, target.z),
    ]
}

/// The summary of one side from the runs' signed relative `errors` and their relative
/// `standard_errors`.
fn side(errors: &[[f32; 3]], standard_errors: &[[f32; 3]]) -> Side {
    let n = errors.len() as f32;
    let mut out = Side {
        bias: [0.0; 3],
        noise: [0.0; 3],
        standard_error: [0.0; 3],
    };
    for c in 0..3 {
        let mean = errors.iter().map(|e| e[c]).sum::<f32>() / n;
        let variance = errors
            .iter()
            .map(|e| e[c] - mean)
            .fold(0.0f32, |acc, d| d.mul_add(d, acc))
            / (n - 1.0);
        out.bias[c] = mean;
        out.noise[c] = variance.sqrt();
        out.standard_error[c] = standard_errors.iter().map(|e| e[c]).sum::<f32>() / n;
    }
    out
}

/// Runs `anchor` at `max_bounces` over the independent sample ranges and summarises
/// both sides.
fn measure(ctx: &GpuContext, anchor: &Anchor, max_bounces: u32) -> Level {
    let results: Vec<FurnaceResult> = (0..RUNS)
        .map(|run| {
            run_furnace_for_run(
                ctx,
                &anchor.material,
                &anchor.finishes,
                anchor.use_hdr_nee,
                max_bounces,
                run,
            )
        })
        .collect();
    let signed = |pick: fn(&FurnaceResult) -> Vec3| -> Vec<[f32; 3]> {
        results
            .iter()
            .map(|r| relative(pick(r) - r.analytic_target, r.analytic_target))
            .collect()
    };
    let spread = |pick: fn(&FurnaceResult) -> Vec3| -> Vec<[f32; 3]> {
        results
            .iter()
            .map(|r| relative(pick(r), r.analytic_target))
            .collect()
    };
    Level {
        max_bounces,
        cpu: side(&signed(|r| r.cpu_mean), &spread(|r| r.cpu_standard_error)),
        gpu: side(&signed(|r| r.gpu_mean), &spread(|r| r.gpu_standard_error)),
    }
}

/// The report line of one side at one cap.
fn describe(label: &str, max_bounces: u32, side: &Side) -> String {
    let [bx, by, bz] = side.bias;
    let [nx, ny, nz] = side.noise;
    let [sx, sy, sz] = side.standard_error;
    format!(
        "{label} cap {max_bounces:>3}: bias ({bx:+.4}, {by:+.4}, {bz:+.4})  noise ({nx:.4}, \
         {ny:.4}, {nz:.4})  standard error ({sx:.4}, {sy:.4}, {sz:.4})"
    )
}

/// The budget the measurements at the anchor's own cap justify: over both sides and
/// every component, `|bias| + SIGMA * max(noise, standard error)`, rounded up to 0.005.
fn justified_budget(own: &Level) -> f32 {
    let needed = [own.cpu, own.gpu]
        .iter()
        .flat_map(|side| {
            (0..3).map(|c| {
                SIGMA.mul_add(
                    side.noise[c].max(side.standard_error[c]),
                    side.bias[c].abs(),
                )
            })
        })
        .fold(0.0, f32::max);
    (needed / 0.005).ceil() * 0.005
}

/// Prints the truncation loss of both sides and the justified budget.
fn report(anchor: &Anchor, own: &Level, high: &Level) {
    for (label, own_side, high_side) in [("cpu", own.cpu, high.cpu), ("gpu", own.gpu, high.gpu)] {
        let [tx, ty, tz] = [0, 1, 2].map(|c| own_side.bias[c] - high_side.bias[c]);
        println!(
            "    truncation loss at cap {} on the {label}: ({tx:+.4}, {ty:+.4}, {tz:+.4})",
            anchor.max_bounces
        );
    }
    println!(
        "    budget {:.3} today; the measurements justify {:.3}",
        anchor.budget,
        justified_budget(own)
    );
}

/// Records what the levels of `anchor` say against its budget: an energy error at the
/// high cap beyond the noise on either side, or a budget below what the measurements
/// need.
fn check(anchor: &Anchor, own: &Level, high: &Level, findings: &mut Vec<String>) {
    for (label, side) in [("cpu", high.cpu), ("gpu", high.gpu)] {
        for c in 0..3 {
            if side.bias[c].abs() > SIGMA * side.standard_error[c] {
                findings.push(format!(
                    "{}: {label} component {c} bias {:+.4} at cap {HIGH_CAP} exceeds {SIGMA} \
                     standard errors ({:.4}): an energy error beyond the truncation loss",
                    anchor.name, side.bias[c], side.standard_error[c]
                ));
            }
        }
    }
    let justified = justified_budget(own);
    if justified > anchor.budget {
        findings.push(format!(
            "{}: the measurements need {justified:.3}, above the budget {:.3} in force",
            anchor.name, anchor.budget
        ));
    }
}

#[test]
#[ignore = "a measurement on the GPU adapter over millions of samples per anchor: run \
            with --features gpu --ignored --nocapture, see the module doc"]
fn furnace_budgets_from_measured_runs() {
    let ctx = match GpuContext::acquire() {
        Ok(ctx) => ctx,
        Err(error) => {
            println!("skipping furnace_budgets_from_measured_runs: no GPU adapter ({error})");
            return;
        }
    };
    let mut findings = Vec::new();
    for anchor in anchors() {
        println!(
            "[{}] budget {:.3} at cap {}",
            anchor.name, anchor.budget, anchor.max_bounces
        );
        let own = measure(&ctx, &anchor, anchor.max_bounces);
        let high = measure(&ctx, &anchor, HIGH_CAP);
        for level in [&own, &high] {
            println!("    {}", describe("cpu", level.max_bounces, &level.cpu));
            println!("    {}", describe("gpu", level.max_bounces, &level.gpu));
        }
        report(&anchor, &own, &high);
        check(&anchor, &own, &high, &mut findings);
    }
    assert!(findings.is_empty(), "{}", findings.join("\n"));
}
