//! Benchmark of the rough-colour fit against the performance budget of plan 2026-10-09,
//! section 10.3 (lane V1). Built only with the `zoning` feature (`required-features` in
//! `Cargo.toml`), never part of a default build or of a build script.
//!
//! ```text
//! cargo run --release -p indicatrix-cut-core --features zoning --example zoning_bench
//! cargo run --release -p indicatrix-cut-core --features zoning --example zoning_bench -- \
//!     --views 8 --px 384 --samples 256 --configs frosted,polished,immersion
//! ```
//!
//! For each configuration it builds a 30 mm irregular scanned-like rough (a noise-displaced
//! icosphere), photographs it with 8 orthographic cameras at 24 px/mm, traces the rig at the
//! working resolution (384 px across, about 60 000 stone pixels a view), makes noisy photos of a
//! four-zone stone from the records, and runs the whole solver (multi-start, model comparison,
//! leave-one-view-out). It prints the trace time, the solve time, the size of the stored records
//! and the colour error, next to the budget of section 10.3.
//!
//! Options (all optional): `--views N` (8), `--samples N` (256), `--px N` (384), `--threads N`
//! (0 = all cores), `--stone-mm D` (30), `--configs a,b,c` (frosted, polished, immersion),
//! `--skip-solve`.
//!
//! Peak memory: the records are measured exactly (`ForwardRecords::memory_bytes`); the process
//! peak is not (no OS calls here). Watch the process in Task Manager or run it under a tool that
//! reports the peak working set; the budget is 600 MB peak.

#![allow(
    clippy::too_many_lines,
    clippy::print_stdout,
    clippy::print_stderr,
    clippy::use_debug,
    reason = "a command-line benchmark"
)]

#[path = "../tests/zoning_validation/synth.rs"]
mod synth;

use std::{sync::atomic::AtomicBool, time::Instant};

use indicatrix_cut_core::rough_plan::colour_fit::solve::{FitConfig, fit_records};

use synth::{ColourCase, NoiseSpec, RoughKind, Setup, SetupSpec, face_up_errors, photos_forward};

/// One benchmark configuration.
#[derive(Clone, Copy)]
struct Config {
    name: &'static str,
    rough: RoughKind,
    dispersion: bool,
    /// The budget of section 10.3 for the trace, seconds (low, high).
    trace_budget_s: (f64, f64),
}

const FROSTED: Config = Config {
    name: "frosted, dispersion",
    rough: RoughKind::Frosted,
    dispersion: true,
    trace_budget_s: (60.0, 120.0),
};
const POLISHED: Config = Config {
    name: "polished",
    rough: RoughKind::Irregular,
    dispersion: false,
    trace_budget_s: (10.0, 20.0),
};
const IMMERSION: Config = Config {
    name: "immersion",
    rough: RoughKind::Immersion,
    dispersion: false,
    trace_budget_s: (10.0, 20.0),
};

struct Args {
    views: usize,
    samples: usize,
    grid_px: usize,
    threads: usize,
    stone_mm: f64,
    configs: Vec<Config>,
    skip_solve: bool,
}

impl Args {
    fn parse() -> Self {
        let mut args = Self {
            views: 8,
            samples: 256,
            grid_px: 384,
            threads: 0,
            stone_mm: 30.0,
            configs: vec![FROSTED, POLISHED, IMMERSION],
            skip_solve: false,
        };
        let mut it = std::env::args().skip(1);
        while let Some(flag) = it.next() {
            let mut value =
                |name: &str| it.next().unwrap_or_else(|| panic!("{name} needs a value"));
            match flag.as_str() {
                "--views" => args.views = value("--views").parse().expect("a number of views"),
                "--samples" => {
                    args.samples = value("--samples").parse().expect("a sample count");
                }
                "--px" => args.grid_px = value("--px").parse().expect("a pixel count"),
                "--threads" => args.threads = value("--threads").parse().expect("a thread count"),
                "--stone-mm" => args.stone_mm = value("--stone-mm").parse().expect("a size in mm"),
                "--skip-solve" => args.skip_solve = true,
                "--configs" => {
                    args.configs = value("--configs")
                        .split(',')
                        .map(|name| match name.trim() {
                            "frosted" => FROSTED,
                            "polished" => POLISHED,
                            "immersion" => IMMERSION,
                            other => panic!("unknown configuration '{other}'"),
                        })
                        .collect();
                }
                other => panic!("unknown option '{other}'"),
            }
        }
        args
    }
}

struct Row {
    config: Config,
    valid_pixels: usize,
    pixels_per_view: f64,
    trace_s: f64,
    records_mb: f64,
    samples: u64,
    lost_depth: f64,
    max_range_mm: f32,
    solve_s: Option<f64>,
    worst_delta_e: Option<f64>,
    lovo_median: Option<f64>,
}

fn run(config: Config, args: &Args) -> Row {
    let diameter_mm = args.stone_mm;
    let spec = SetupSpec {
        rough: config.rough,
        colour: ColourCase::Banded,
        views: args.views,
        grid_px: args.grid_px,
        samples: args.samples,
        threads: args.threads,
        // The irregular stone has a mean radius of 7 mm at scale 1.
        scale: diameter_mm / 2.0 / 7.0,
        px_per_mm: 24.0,
        image_px: 1200,
        window: [100, 100, 1100, 1100],
        dispersion: config.dispersion,
        ..SetupSpec::default()
    };
    let setup = Setup::new(&spec);
    eprintln!("[{}] tracing {} views ...", config.name, setup.views.len());
    let started = Instant::now();
    let records = setup.trace();
    let trace_s = started.elapsed().as_secs_f64();
    let valid_pixels = records.valid_pixels();
    let row_base = Row {
        config,
        valid_pixels,
        pixels_per_view: valid_pixels as f64 / args.views as f64,
        trace_s,
        records_mb: records.memory_bytes() as f64 / (1024.0 * 1024.0),
        samples: records.stats.samples,
        lost_depth: records.lost_depth_fraction(),
        max_range_mm: records.stats.max_cluster_range_mm,
        solve_s: None,
        worst_delta_e: None,
        lovo_median: None,
    };
    eprintln!(
        "[{}] traced in {trace_s:.1} s: {valid_pixels} usable pixels, {:.0} MB of records",
        config.name, row_base.records_mb
    );
    if args.skip_solve {
        return row_base;
    }

    let photos = photos_forward(&records, &setup.truth, &NoiseSpec::RAW, 1);
    let fit_config = FitConfig {
        threads: args.threads,
        ..FitConfig::default()
    };
    eprintln!("[{}] solving ...", config.name);
    let started = Instant::now();
    let fit = fit_records(
        &records,
        &photos,
        &fit_config,
        &AtomicBool::new(false),
        &mut |_| {},
    )
    .unwrap_or_else(|e| panic!("[{}] the fit failed: {e}", config.name));
    let solve_s = started.elapsed().as_secs_f64();
    let worst = face_up_errors(&fit, &setup.truth)
        .iter()
        .map(|e| e.delta_e)
        .fold(0.0_f64, f64::max);
    eprintln!("[{}] solved in {solve_s:.1} s", config.name);
    Row {
        solve_s: Some(solve_s),
        worst_delta_e: Some(worst),
        lovo_median: fit.lovo.as_ref().map(|l| l.median_delta_e),
        ..row_base
    }
}

fn main() {
    let args = Args::parse();
    let cores = std::thread::available_parallelism().map_or(1, usize::from);
    println!(
        "zoning_bench: {} views, {} samples/px, {} px working grid, {} mm stone, {} threads ({} cores)",
        args.views,
        args.samples,
        args.grid_px,
        args.stone_mm,
        if args.threads == 0 {
            "all".to_owned()
        } else {
            args.threads.to_string()
        },
        cores
    );
    let rows: Vec<Row> = args.configs.iter().map(|c| run(*c, &args)).collect();

    println!();
    println!(
        "{:<22} {:>9} {:>9} {:>9} {:>11} {:>9} {:>9} {:>8} {:>9}",
        "configuration",
        "px/view",
        "trace s",
        "budget s",
        "records MB",
        "solve s",
        "worst dE",
        "LOVO",
        "lost dep."
    );
    for row in &rows {
        println!(
            "{:<22} {:>9.0} {:>9.1} {:>4.0}-{:<4.0} {:>11.0} {:>9} {:>9} {:>8} {:>8.3}%",
            row.config.name,
            row.pixels_per_view,
            row.trace_s,
            row.config.trace_budget_s.0,
            row.config.trace_budget_s.1,
            row.records_mb,
            row.solve_s
                .map_or_else(|| "-".to_owned(), |s| format!("{s:.1}")),
            row.worst_delta_e
                .map_or_else(|| "-".to_owned(), |d| format!("{d:.2}")),
            row.lovo_median
                .map_or_else(|| "-".to_owned(), |d| format!("{d:.2}")),
            row.lost_depth * 100.0,
        );
    }
    println!();
    println!("budget (plan 10.3): solve about 10 s, records + working set under 600 MB peak");
    for row in &rows {
        println!(
            "  {}: {} usable pixels in all views, {} camera-ray samples traced, widest cluster {:.3} mm",
            row.config.name, row.valid_pixels, row.samples, row.max_range_mm
        );
    }
}
