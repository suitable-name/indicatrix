//! `--simulate`: replays a fluctuating bandwidth trace over a frame sequence and compares
//! the total time of fixed and adaptive encoding choices.
//!
//! No wall clock is read: every time is computed from the measured per-codec compress and
//! decompress seconds and encoded sizes of the benchmark records, plus `wire_bits / link`
//! at the trace's bandwidth for the frame. The adaptive policy under test is the crate's
//! own `LinkAdapter` (estimator + tier selector) and `AdaptiveEncoderPolicy` (matrix), fed
//! with the simulated write time of each frame, exactly as a sender would feed it.

use crate::{
    link::{Model, Rank},
    measure::{Kind, Record},
};
use indicatrix_net::messages::{
    PayloadEncoding,
    adaptive::{AdaptiveEncoderPolicy, BandwidthTier, LinkAdapter, SizeClass},
};
use std::{collections::BTreeMap, error::Error, time::Duration};

/// A bandwidth in Mbit/s for every simulated frame.
pub struct Trace {
    /// Name used in the report.
    pub name: String,
    /// The true link speed of each frame.
    pub mbps: Vec<f64>,
}

/// Frames per step of the staircase and dip traces.
const STEP_FRAMES: usize = 20;

/// Frames of the wobble trace.
const WOBBLE_FRAMES: usize = 60;

/// A step trace from `(mbps, frames)` segments.
fn steps(name: &str, segments: &[(f64, usize)]) -> Trace {
    Trace {
        name: name.to_string(),
        mbps: segments
            .iter()
            .flat_map(|&(mbps, n)| std::iter::repeat_n(mbps, n))
            .collect(),
    }
}

/// splitmix64 step (fixed seed, platform independent).
const fn splitmix(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// 300 Mbit/s +-40%, uniformly and deterministically pseudo-random per frame.
fn wobble() -> Trace {
    let mut state = 0x5eed_0b1e_0000_0001_u64;
    let mbps = (0..WOBBLE_FRAMES)
        .map(|_| {
            let unit = (splitmix(&mut state) >> 11) as f64 / (1u64 << 53) as f64;
            300.0 * 0.4_f64.mul_add(2.0_f64.mul_add(unit, -1.0), 1.0)
        })
        .collect();
    Trace {
        name: "wlan-wobble".to_string(),
        mbps,
    }
}

/// The built-in traces: `lan-dip`, `wlan-wobble`, `staircase`.
pub fn builtin() -> Vec<Trace> {
    vec![
        steps(
            "lan-dip",
            &[
                (1000.0, STEP_FRAMES),
                (100.0, STEP_FRAMES),
                (1000.0, STEP_FRAMES),
            ],
        ),
        wobble(),
        steps(
            "staircase",
            &[
                (100.0, STEP_FRAMES),
                (300.0, STEP_FRAMES),
                (1000.0, STEP_FRAMES),
                (300.0, STEP_FRAMES),
                (100.0, STEP_FRAMES),
            ],
        ),
    ]
}

/// Parses `"1000:20,100:20,300:20"` (bandwidth in Mbit/s : frames) into a trace.
///
/// # Errors
///
/// Anything that is not a list of `mbps:frames` pairs with positive values.
pub fn parse(spec: &str) -> Result<Trace, Box<dyn Error>> {
    let mut segments = Vec::new();
    for part in spec.split(',') {
        let (mbps, frames) = part
            .trim()
            .split_once(':')
            .ok_or_else(|| format!("bad trace segment '{part}' (want mbps:frames)"))?;
        let mbps: f64 = mbps.trim().parse()?;
        let frames: usize = frames.trim().parse()?;
        if !mbps.is_finite() || mbps <= 0.0 || frames == 0 {
            return Err(format!("trace segment '{part}' needs positive values").into());
        }
        segments.push((mbps, frames));
    }
    Ok(steps(&format!("custom({spec})"), &segments))
}

/// One frame of the rotation: every codec's measurement of one data case.
struct FrameCase<'a> {
    raw_bytes: usize,
    records: Vec<&'a Record>,
}

/// The size to simulate: the largest measured one of the medium class, else the largest.
fn pick_size(records: &[Record]) -> Option<(usize, usize)> {
    let mut sizes: Vec<(usize, usize)> = records
        .iter()
        .filter(|r| r.kind == Kind::Payload)
        .map(|r| (r.width, r.height))
        .collect();
    sizes.sort_unstable();
    sizes.dedup();
    sizes
        .iter()
        .copied()
        .rfind(|&(w, h)| SizeClass::of_pixels(w * h) == SizeClass::Medium)
        .or_else(|| sizes.last().copied())
}

/// The data cases of `size`, in data-set name order.
fn frame_cases(records: &[Record], size: (usize, usize)) -> Vec<FrameCase<'_>> {
    let mut by_set: BTreeMap<&str, Vec<&Record>> = BTreeMap::new();
    for r in records
        .iter()
        .filter(|r| r.kind == Kind::Payload && (r.width, r.height) == size)
    {
        by_set.entry(r.set).or_default().push(r);
    }
    by_set
        .into_values()
        .map(|records| FrameCase {
            raw_bytes: records[0].raw_bytes,
            records,
        })
        .collect()
}

/// The measurement standing for `encoding` on `case` (nearest measured zstd level; the
/// Raw record when the family was not swept).
fn lookup<'a>(case: &FrameCase<'a>, encoding: PayloadEncoding) -> &'a Record {
    let (family, level) = match encoding {
        PayloadEncoding::Raw => ("Raw", 0),
        PayloadEncoding::ShuffleLz4 => ("ShuffleLz4", 0),
        PayloadEncoding::ShuffleZstd { level } => ("ShuffleZstd", u32::from(level)),
    };
    case.records
        .iter()
        .copied()
        .filter(|r| r.family == family)
        .min_by_key(|r| r.level.abs_diff(level))
        .or_else(|| case.records.iter().copied().find(|r| r.family == "Raw"))
        .unwrap_or(case.records[0])
}

/// What one policy cost over a trace.
struct Outcome {
    name: &'static str,
    total_s: f64,
    switches: Option<u32>,
}

/// Replays `trace`; `pick` names the encoding for each frame given the frame's case and the
/// true bandwidth, and may observe the simulated write.
fn replay(
    trace: &Trace,
    cases: &[FrameCase<'_>],
    (model, rank): (&Model, Rank),
    mut pick: impl for<'a, 'b> FnMut(&'a FrameCase<'b>, f64) -> &'b Record,
) -> f64 {
    let mut total = 0.0;
    for (i, &mbps) in trace.mbps.iter().enumerate() {
        let case = &cases[i % cases.len()];
        let record = pick(case, mbps);
        total += model.timing(record, mbps).seconds(rank);
    }
    total
}

/// A fixed choice for every frame.
fn run_fixed(
    name: &'static str,
    trace: &Trace,
    cases: &[FrameCase<'_>],
    ctx: (&Model, Rank),
    choose: impl Fn(&FrameCase<'_>) -> PayloadEncoding,
) -> Outcome {
    let total_s = replay(trace, cases, ctx, |case, _| lookup(case, choose(case)));
    Outcome {
        name,
        total_s,
        switches: None,
    }
}

/// The adaptive policy: the crate's `LinkAdapter` and `AdaptiveEncoderPolicy`, fed with the
/// simulated blocking-write time (the transfer time at the true bandwidth) of each frame.
fn run_adaptive(trace: &Trace, cases: &[FrameCase<'_>], ctx: (&Model, Rank)) -> Outcome {
    let policy = AdaptiveEncoderPolicy::adaptive(&PayloadEncoding::default_accept_list(), false);
    let mut link = LinkAdapter::default();
    let model = ctx.0;
    let total_s = replay(trace, cases, ctx, |case, mbps| {
        let record = lookup(case, policy.choose_payload(case.raw_bytes, link.tier()));
        let write = Duration::from_secs_f64(model.timing(record, mbps).transfer);
        link.observe(record.wire_bytes, write);
        record
    });
    Outcome {
        name: "adaptive (estimator + tiers + matrix)",
        total_s,
        switches: Some(link.tier_switches()),
    }
}

/// The oracle: the best measured candidate for every frame at its true bandwidth.
fn run_oracle(trace: &Trace, cases: &[FrameCase<'_>], ctx: (&Model, Rank)) -> Outcome {
    let (model, rank) = ctx;
    let total_s = replay(trace, cases, ctx, |case, mbps| {
        case.records
            .iter()
            .copied()
            .min_by(|a, b| {
                let t = |r: &Record| model.timing(r, mbps).seconds(rank);
                t(a).total_cmp(&t(b))
            })
            .unwrap_or(case.records[0])
    });
    Outcome {
        name: "oracle (best measured codec per frame)",
        total_s,
        switches: None,
    }
}

/// Prints one trace's comparison.
fn print_trace(trace: &Trace, size: (usize, usize), outcomes: &[Outcome]) {
    let frames = trace.mbps.len();
    println!(
        "\ntrace {}: {frames} frames of {}x{} (the measured data kinds in rotation)",
        trace.name, size.0, size.1
    );
    println!(
        "  {:<42} {:>10} {:>10} {:>14}",
        "policy", "total s", "ms/frame", "loss vs oracle"
    );
    let oracle = outcomes.last().map_or(1.0, |o| o.total_s);
    for o in outcomes {
        let loss = 100.0 * (o.total_s / oracle - 1.0);
        let extra = o
            .switches
            .map_or_else(String::new, |n| format!("  ({n} tier switches)"));
        println!(
            "  {:<42} {:>10.3} {:>10.1} {:>13.1}%{extra}",
            o.name,
            o.total_s,
            1e3 * o.total_s / frames as f64,
            loss
        );
    }
}

/// Runs every trace (`custom` ones first when given, else the built-ins) over the measured
/// records and prints the comparison.
///
/// # Errors
///
/// A bad `--trace` spec, or no measurements to simulate with.
pub fn run(
    records: &[Record],
    model: &Model,
    rank: Rank,
    custom: &[String],
) -> Result<(), Box<dyn Error>> {
    let size = pick_size(records).ok_or("no payload measurements to simulate with")?;
    let cases = frame_cases(records, size);
    let traces = if custom.is_empty() {
        builtin()
    } else {
        custom.iter().map(|s| parse(s)).collect::<Result<_, _>>()?
    };
    println!(
        "\n=== simulation (model {rank:?}, cpu share {}; simulated time only) ===",
        model.cpu_share
    );
    let ctx = (model, rank);
    let gigabit = BandwidthTier::for_estimate(1000.0);
    let fixed_best =
        AdaptiveEncoderPolicy::adaptive(&PayloadEncoding::default_accept_list(), false);
    for trace in &traces {
        let outcomes = [
            run_fixed("fixed zstd 1 (today's default)", trace, &cases, ctx, |_| {
                PayloadEncoding::DEFAULT_ZSTD
            }),
            run_fixed(
                "fixed best-for-1000 (matrix cell)",
                trace,
                &cases,
                ctx,
                |c| fixed_best.choose_payload(c.raw_bytes, gigabit),
            ),
            run_adaptive(trace, &cases, ctx),
            run_oracle(trace, &cases, ctx),
        ];
        print_trace(trace, size, &outcomes);
    }
    Ok(())
}
