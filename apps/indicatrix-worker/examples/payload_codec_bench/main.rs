//! Codec x data x link benchmark for coordinator <-> worker <-> desktop traffic.
//!
//! Measures the production lossless payload codecs of `indicatrix-net`
//! (`PayloadEncoding::{Raw, ShuffleLz4, ShuffleZstd}` for `FRAME`/`PREVIEW` radiance, and the
//! PNG display codec for 8-bit `DISPLAY_FRAME`s) on deterministic synthetic frames, then
//! applies a link model to say which setting gives the best frame rate. Run with `--help`
//! for the flags; the sweep is synthetic, `--captures <dir>` switches to the older report
//! over frames written by `capture_frame_payloads`.
//!
//! ```text
//! cargo run -p indicatrix-worker --release --example payload_codec_bench -- --quick
//! ```

mod captures;
mod data;
mod link;
mod measure;
mod report;

use data::{DataSet, generate, to_rgba8, zero_fraction};
use link::Model;
use measure::{Candidate, Frame, Record, measure};
use std::{error::Error, path::PathBuf};

/// The `--help` text.
const HELP: &str = "\
payload_codec_bench: lossless payload codec x data set x link model benchmark

USAGE:
    cargo run -p indicatrix-worker --release --example payload_codec_bench -- [FLAGS]

CODECS (production codecs of indicatrix-net, one thread):
    Raw, ShuffleLz4, ShuffleZstd at --levels, and the PNG display codec (plus raw RGBA8
    for scale) on a tone-mapped 8-bit copy of each frame.

DATA SETS (deterministic, fixed seeds, no files):
    sparse     gem-shaped blob with a smooth gradient on a mostly zero background
    dense      path-tracing noise everywhere (log-normal, rare fireflies)
    converged  smooth, high-sample-count, low noise
    delta      per-chunk FRAME delta: the sum of 8 samples per pixel, most missing
    Sizes: 256x256 and 1024x1024; --large adds 3840x2160 (about 100 MB raw per frame).

FLAGS:
    --quick                 smoke run: sizes 256x256 and 512x512, 2 reps (under a minute)
    --large                 also run 3840x2160
    --reps <N>              timed repetitions per cell after one warm-up; the median is
                            reported (default 5; 2 with --quick)
    --levels <a,b,..>       zstd levels to sweep (default 1,2,3,4,5,6,7,8,9,12,19; 1..=22)
    --link-mbps <M[,M..]>   link speed in Mbit/s; repeatable (default 1000,300,100: gigabit
                            LAN, good WLAN, bad WLAN)
    --min-compress-mbps <X> compress throughput floor in MB/s of raw input (default 0 = off).
                            Candidates slower than X are skipped for the 'with compress >='
                            pick and for the recommendation, so a coordinator that is also
                            tracing can bound the CPU the codec may use
    --cpu-share <F>         fraction of one core the codec gets, 0 < F <= 1 (default 1).
                            Measurements run on a dedicated thread; when the codec shares
                            cores with the tracer (cpu threads shared), set F below 1 and
                            every compress/decompress time is divided by F
    --csv <path>            write every measurement to a CSV file (fixed column order)
    --captures <dir>        legacy mode: report on capture_frame_payloads output instead
    -h, --help              this text

LINK MODEL (per frame, per link):
    t_transfer = encoded_bits / link;  pipelined = max(t_compress, t_transfer, t_decompress);
    serial = t_compress + t_transfer + t_decompress;  fps = 1 / time.
    The 'comp ms' column is the CPU cost per frame; compare it with the tracing time per frame.
";

/// Parsed command line.
struct Args {
    quick: bool,
    large: bool,
    reps: Option<usize>,
    levels: Vec<u8>,
    links: Vec<f64>,
    min_compress_mbs: f64,
    cpu_share: f64,
    csv: Option<PathBuf>,
    captures: Option<String>,
}

/// Parses a comma-separated list with `parse`.
fn list<T: std::str::FromStr>(text: &str, what: &str) -> Result<Vec<T>, Box<dyn Error>> {
    text.split(',')
        .map(|p| {
            p.trim()
                .parse()
                .map_err(|_| format!("bad {what}: '{p}'").into())
        })
        .collect()
}

/// Parses the command line; `Ok(None)` after printing help.
fn parse_args() -> Result<Option<Args>, Box<dyn Error>> {
    let mut a = Args {
        quick: false,
        large: false,
        reps: None,
        levels: vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 12, 19],
        links: Vec::new(),
        min_compress_mbs: 0.0,
        cpu_share: 1.0,
        csv: None,
        captures: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        if flag == "-h" || flag == "--help" {
            print!("{HELP}");
            return Ok(None);
        }
        let mut value = |name: &str| it.next().ok_or_else(|| format!("{name} needs a value"));
        match flag.as_str() {
            "--quick" => a.quick = true,
            "--large" => a.large = true,
            "--reps" => a.reps = Some(value("--reps")?.parse()?),
            "--levels" => a.levels = list(&value("--levels")?, "level")?,
            "--link-mbps" => a.links.extend(list::<f64>(&value("--link-mbps")?, "link")?),
            "--min-compress-mbps" => a.min_compress_mbs = value("--min-compress-mbps")?.parse()?,
            "--cpu-share" => a.cpu_share = value("--cpu-share")?.parse()?,
            "--csv" => a.csv = Some(PathBuf::from(value("--csv")?)),
            "--captures" => a.captures = Some(value("--captures")?),
            other => return Err(format!("unknown flag '{other}' (see --help)").into()),
        }
    }
    if a.links.is_empty() {
        a.links = vec![1000.0, 300.0, 100.0];
    }
    validate(&a)?;
    Ok(Some(a))
}

/// Rejects values the model cannot use.
fn validate(a: &Args) -> Result<(), Box<dyn Error>> {
    if a.levels.is_empty() || a.levels.iter().any(|l| !(1..=22).contains(l)) {
        return Err("--levels must be a non-empty list within 1..=22".into());
    }
    if a.links.iter().any(|l| !l.is_finite() || *l <= 0.0) {
        return Err("--link-mbps values must be positive".into());
    }
    if !(a.cpu_share > 0.0 && a.cpu_share <= 1.0) {
        return Err("--cpu-share must satisfy 0 < F <= 1".into());
    }
    if a.reps == Some(0) || !a.min_compress_mbs.is_finite() || a.min_compress_mbs < 0.0 {
        return Err("--reps must be >= 1 and --min-compress-mbps >= 0".into());
    }
    Ok(())
}

/// Measures every candidate on one generated frame and prints its table.
fn run_case(
    set: DataSet,
    (width, height): (usize, usize),
    candidates: &[Candidate],
    reps: usize,
    model: &Model,
) -> Result<Vec<Record>, Box<dyn Error>> {
    eprintln!("measuring {} {width}x{height} ...", set.name());
    let raw = generate(set, width, height);
    let mut records = Vec::new();
    let frame = Frame {
        set: set.name(),
        width,
        height,
        raw: &raw,
    };
    for &c in candidates {
        records.push(measure(c, frame, reps)?);
    }
    let rgba = to_rgba8(&raw);
    let display = Frame {
        raw: &rgba,
        ..frame
    };
    for c in Candidate::DISPLAY {
        records.push(measure(c, display, reps)?);
    }
    report::print_case(
        set.name(),
        width,
        height,
        zero_fraction(&raw),
        &records,
        model,
    );
    Ok(records)
}

/// Entry point.
fn main() -> Result<(), Box<dyn Error>> {
    let Some(args) = parse_args()? else {
        return Ok(());
    };
    if let Some(dir) = &args.captures {
        return captures::run(dir);
    }
    let reps = args.reps.unwrap_or(if args.quick { 2 } else { 5 });
    let mut sizes = if args.quick {
        vec![(256, 256), (512, 512)]
    } else {
        vec![(256, 256), (1024, 1024)]
    };
    if args.large {
        sizes.push((3840, 2160));
    }
    let model = Model {
        links: args.links.clone(),
        min_compress_mbs: args.min_compress_mbs,
        cpu_share: args.cpu_share,
    };
    println!(
        "MB = 10^6 bytes of raw input, one thread, median of {reps} reps after a warm-up; \
         links {:?} Mbit/s; cpu share {}; min compress {} MB/s",
        model.links, model.cpu_share, model.min_compress_mbs
    );
    let candidates = Candidate::payload_sweep(&args.levels);
    let mut all = Vec::new();
    for size in sizes {
        for set in DataSet::ALL {
            all.extend(run_case(set, size, &candidates, reps, &model)?);
        }
    }
    report::print_recommendations(&all, &model);
    if let Some(path) = &args.csv {
        report::write_csv(path, &all, &model)?;
        println!("\nwrote {} measurements to {}", all.len(), path.display());
    }
    println!("all round trips were bit-identical");
    Ok(())
}
