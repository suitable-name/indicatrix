//! The link model: per-frame transfer/pipeline times, best-codec selection and the
//! server-preference recommendation.
//!
//! For one frame over a link of `L` Mbit/s: `t_transfer = wire_bits / L`; a pipelined sender
//! (compress frame N+1 while frame N is on the wire and frame N-1 is decoded) is limited by
//! `max(t_compress, t_transfer, t_decompress)`; a strictly serial one pays the sum.

use crate::measure::{Candidate, Kind, Record};
use indicatrix_net::messages::{DEFAULT_SERVER_PREFERENCE, PayloadEncoding};
use std::collections::BTreeMap;

/// Link and CPU assumptions shared by every report line.
pub struct Model {
    /// Link speeds in Mbit/s (10^6 bits per second).
    pub links: Vec<f64>,
    /// Compress throughput floor in MB/s of raw input; slower candidates are excluded from
    /// the constrained pick and from the recommendation.
    pub min_compress_mbs: f64,
    /// Fraction of one core the codec gets (1.0 = a dedicated core).
    pub cpu_share: f64,
}

/// Per-frame times of one record on one link, in seconds.
#[derive(Clone, Copy, Debug)]
pub struct Timing {
    /// Encoded bits over the link speed.
    pub transfer: f64,
    /// Compress seconds, scaled by the CPU share.
    pub compress: f64,
    /// Decompress seconds, scaled by the CPU share.
    pub decompress: f64,
}

impl Timing {
    /// `max` of the three stages.
    pub const fn pipelined_s(self) -> f64 {
        self.transfer.max(self.compress).max(self.decompress)
    }

    /// Sum of the three stages.
    pub fn serial_s(self) -> f64 {
        self.transfer + self.compress + self.decompress
    }

    /// The frame time under `rank`.
    pub fn seconds(self, rank: Rank) -> f64 {
        match rank {
            Rank::Pipelined => self.pipelined_s(),
            Rank::Serial => self.serial_s(),
        }
    }
}

impl Model {
    /// The timing of `r` on a link of `mbps` Mbit/s.
    pub fn timing(&self, r: &Record, mbps: f64) -> Timing {
        Timing {
            transfer: r.wire_bytes as f64 * 8.0 / (mbps * 1e6),
            compress: r.compress_s / self.cpu_share,
            decompress: r.decompress_s / self.cpu_share,
        }
    }

    /// Whether `r` compresses fast enough for `--min-compress-mbps`.
    pub fn meets_floor(&self, r: &Record) -> bool {
        r.compress_mbs() * self.cpu_share >= self.min_compress_mbs
    }
}

/// How to rank candidates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rank {
    /// Smallest pipelined frame time.
    Pipelined,
    /// Smallest serial frame time.
    Serial,
}

/// The record with the smallest time under `rank` on `mbps`, restricted by `floor`.
pub fn best<'a>(
    records: &[&'a Record],
    model: &Model,
    mbps: f64,
    rank: Rank,
    floor: bool,
) -> Option<&'a Record> {
    let key = |r: &Record| {
        let t = model.timing(r, mbps);
        match rank {
            Rank::Pipelined => t.pipelined_s(),
            Rank::Serial => t.serial_s(),
        }
    };
    records
        .iter()
        .copied()
        .filter(|r| !floor || model.meets_floor(r))
        .min_by(|a, b| key(a).total_cmp(&key(b)))
}

/// One family's best geometric-mean slowdown against the per-case winner.
struct FamilyScore {
    family: &'static str,
    level: u32,
    score: f64,
}

/// Per-candidate accumulation: slowdown ratios per case, and whether it met the floor everywhere.
type Acc = BTreeMap<(&'static str, u32), (Vec<f64>, bool)>;

/// Geometric mean of positive values.
pub fn geomean(v: &[f64]) -> f64 {
    (v.iter().map(|x| x.ln()).sum::<f64>() / v.len() as f64).exp()
}

/// Accumulates every payload candidate's pipelined slowdown over all (set, size) cases.
fn accumulate(records: &[Record], model: &Model, mbps: f64) -> Acc {
    let mut cases: BTreeMap<(&str, usize, usize), Vec<&Record>> = BTreeMap::new();
    for r in records.iter().filter(|r| r.kind == Kind::Payload) {
        cases.entry((r.set, r.width, r.height)).or_default().push(r);
    }
    let mut acc = Acc::new();
    for group in cases.values() {
        let pipe = |r: &Record| model.timing(r, mbps).pipelined_s();
        let winner = group.iter().map(|r| pipe(r)).fold(f64::MAX, f64::min);
        for r in group {
            let e = acc
                .entry((r.family, r.level))
                .or_insert_with(|| (Vec::new(), true));
            e.0.push(pipe(r) / winner);
            e.1 &= model.meets_floor(r);
        }
    }
    acc
}

/// Best candidate per family, ordered best first.
fn family_scores(acc: &Acc) -> Vec<FamilyScore> {
    let mut best: BTreeMap<&'static str, FamilyScore> = BTreeMap::new();
    for (&(family, level), (ratios, ok)) in acc {
        if !ok {
            continue;
        }
        let score = geomean(ratios);
        let slot = best.entry(family).or_insert(FamilyScore {
            family,
            level,
            score,
        });
        if score < slot.score {
            *slot = FamilyScore {
                family,
                level,
                score,
            };
        }
    }
    let mut out: Vec<FamilyScore> = best.into_values().collect();
    out.sort_by(|a, b| a.score.total_cmp(&b.score));
    out
}

/// Renders a family/level as the Rust expression for the preference list.
fn expr(family: &str, level: u32) -> String {
    match family {
        "ShuffleZstd" => format!("PayloadEncoding::ShuffleZstd {{ level: {level} }}"),
        other => format!("PayloadEncoding::{other}"),
    }
}

/// The default preference's family order.
fn default_families() -> Vec<&'static str> {
    DEFAULT_SERVER_PREFERENCE
        .iter()
        .map(|e| Candidate::Payload(*e).family())
        .collect()
}

/// The one-line recommendation for `mbps`, plus a plain statement about the default.
pub fn recommend(records: &[Record], model: &Model, mbps: f64) -> (String, String) {
    let acc = accumulate(records, model, mbps);
    let scores = family_scores(&acc);
    if scores.is_empty() {
        return (
            "no candidate meets --min-compress-mbps in every case".to_string(),
            "cannot compare with DEFAULT_SERVER_PREFERENCE".to_string(),
        );
    }
    let list: Vec<String> = scores.iter().map(|s| expr(s.family, s.level)).collect();
    let line = format!("[{}]", list.join(", "));
    let order_matches = scores
        .iter()
        .map(|s| s.family)
        .eq(default_families().iter().copied());
    let default_level = match DEFAULT_SERVER_PREFERENCE[0] {
        PayloadEncoding::ShuffleZstd { level } => u32::from(level),
        _ => 0,
    };
    let verdict = judge(&acc, &scores, order_matches, default_level);
    (line, verdict)
}

/// Compares the recommendation with the default and says plainly whether it still wins.
fn judge(acc: &Acc, scores: &[FamilyScore], order_matches: bool, default_level: u32) -> String {
    let Some(zstd) = scores.iter().find(|s| s.family == "ShuffleZstd") else {
        return "DEFAULT differs: no ShuffleZstd candidate qualifies".to_string();
    };
    let Some((ratios, _)) = acc.get(&("ShuffleZstd", default_level)) else {
        return format!(
            "cannot compare: the default's zstd level {default_level} was not in the sweep"
        );
    };
    let slowdown = geomean(ratios) / zstd.score;
    let pct = (slowdown - 1.0) * 100.0;
    if order_matches && slowdown <= 1.02 {
        format!(
            "DEFAULT is still optimal (zstd level {default_level} is within {pct:.1}% of the best level {})",
            zstd.level
        )
    } else if order_matches {
        format!(
            "DEFAULT order holds but zstd level {} beats level {default_level} by {pct:.1}% (geometric mean frame time)",
            zstd.level
        )
    } else {
        format!(
            "DEFAULT differs: use the list above (default zstd level {default_level} is {pct:.1}% slower than level {})",
            zstd.level
        )
    }
}
