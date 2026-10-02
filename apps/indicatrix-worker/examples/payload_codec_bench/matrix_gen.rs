//! `--emit-matrix`: turns the live measurements into the Rust source of
//! `crates/indicatrix-net/src/messages/encoding_matrix.rs`.
//!
//! For every size class (by RAW payload bytes) and bandwidth tier the best payload encoding
//! is the one with the smallest geometric-mean frame time, relative to each case's own
//! winner, over the data kinds measured in that class. The frame time is the serial model
//! (compress + transfer + decompress) by default, because a coordinator that also traces
//! cannot overlap compression for free. A class nothing was measured for copies the next
//! smaller class and says so in the file.

use crate::{
    link::{Model, Rank, geomean},
    measure::{Kind, Record},
};
use indicatrix_net::messages::{DisplayEncoding, PayloadEncoding, adaptive::SizeClass};
use std::{collections::BTreeMap, error::Error, fmt::Write as _, fs, path::Path};

/// What the generator was asked for.
pub struct Spec<'a> {
    /// The tier ladder in Mbit/s, ascending.
    pub tiers: &'a [f64],
    /// Serial or pipelined ranking.
    pub rank: Rank,
    /// Fraction of one core the codec gets.
    pub cpu_share: f64,
    /// The command line that reproduces the file (for the header).
    pub command: String,
}

/// One data case: (data set, width, height).
type Case = (&'static str, usize, usize);

/// Records of one kind grouped by size class, then by case.
type Grouped<'a> = BTreeMap<SizeClass, BTreeMap<Case, Vec<&'a Record>>>;

/// A codec candidate with its geometric-mean slowdown against the per-case winners.
struct Scored {
    family: &'static str,
    level: u32,
    score: f64,
}

/// Where a class's cells came from.
#[derive(Clone, Copy)]
enum Source {
    Measured,
    CopiedFrom(SizeClass),
}

/// The generated cells of one size class.
struct ClassCells {
    payload: Vec<Vec<PayloadEncoding>>,
    display: Vec<DisplayEncoding>,
    source: Source,
}

/// Groups `records` of `kind` by size class of their raw bytes and by case.
fn group(records: &[Record], kind: Kind) -> Grouped<'_> {
    let mut out = Grouped::new();
    for r in records.iter().filter(|r| r.kind == kind) {
        // A display case is classified by the radiance payload of the same view.
        let pixels = r.width * r.height;
        out.entry(SizeClass::of_pixels(pixels))
            .or_default()
            .entry((r.set, r.width, r.height))
            .or_default()
            .push(r);
    }
    out
}

/// Candidates of one class on `mbps`, best first.
fn ranked(
    cases: &BTreeMap<Case, Vec<&Record>>,
    model: &Model,
    mbps: f64,
    rank: Rank,
) -> Vec<Scored> {
    let mut acc: BTreeMap<(&'static str, u32), Vec<f64>> = BTreeMap::new();
    for records in cases.values() {
        let time = |r: &Record| model.timing(r, mbps).seconds(rank);
        let winner = records.iter().map(|r| time(r)).fold(f64::MAX, f64::min);
        for r in records {
            acc.entry((r.family, r.level))
                .or_default()
                .push(time(r) / winner);
        }
    }
    let mut out: Vec<Scored> = acc
        .into_iter()
        .map(|((family, level), v)| Scored {
            family,
            level,
            score: geomean(&v),
        })
        .collect();
    out.sort_by(|a, b| a.score.total_cmp(&b.score));
    out
}

/// The encoding a payload candidate stands for.
fn to_encoding(family: &str, level: u32) -> PayloadEncoding {
    match family {
        "ShuffleZstd" => PayloadEncoding::ShuffleZstd {
            level: level.min(22) as u8,
        },
        "ShuffleLz4" => PayloadEncoding::ShuffleLz4,
        _ => PayloadEncoding::Raw,
    }
}

/// The ordered preference list for a ranked candidate list: the winner, then the best
/// candidate of every other compressed family, then `Raw`. A `Raw` winner is the list
/// `[Raw]`.
fn preference(scored: &[Scored]) -> Vec<PayloadEncoding> {
    let mut list: Vec<PayloadEncoding> = Vec::new();
    for s in scored {
        let e = to_encoding(s.family, s.level);
        if e == PayloadEncoding::Raw {
            if list.is_empty() {
                break;
            }
            continue;
        }
        if !list.iter().any(|p| p.same_family(e)) {
            list.push(e);
        }
    }
    list.push(PayloadEncoding::Raw);
    list
}

/// The display encoding with the best score.
fn display_best(scored: &[Scored]) -> DisplayEncoding {
    match scored.first().map(|s| s.family) {
        Some("Png") => DisplayEncoding::Png,
        _ => DisplayEncoding::Rgba8,
    }
}

/// Cells for every tier of one measured class.
fn measured_cells(
    payload: &BTreeMap<Case, Vec<&Record>>,
    display: Option<&BTreeMap<Case, Vec<&Record>>>,
    model: &Model,
    spec: &Spec<'_>,
) -> ClassCells {
    let mut cells = ClassCells {
        payload: Vec::new(),
        display: Vec::new(),
        source: Source::Measured,
    };
    for &mbps in spec.tiers {
        cells
            .payload
            .push(preference(&ranked(payload, model, mbps, spec.rank)));
        let shown = display.map_or(DisplayEncoding::Rgba8, |d| {
            display_best(&ranked(d, model, mbps, spec.rank))
        });
        cells.display.push(shown);
    }
    cells
}

/// The cells of all three classes, copying unmeasured ones from the next smaller (or, for
/// a missing small class, the next larger) measured class.
fn build(
    records: &[Record],
    spec: &Spec<'_>,
) -> Result<BTreeMap<SizeClass, ClassCells>, Box<dyn Error>> {
    let model = Model {
        links: spec.tiers.to_vec(),
        min_compress_mbs: 0.0,
        cpu_share: spec.cpu_share,
    };
    let payload = group(records, Kind::Payload);
    let display = group(records, Kind::Display);
    let mut out: BTreeMap<SizeClass, ClassCells> = BTreeMap::new();
    for (class, cases) in &payload {
        out.insert(
            *class,
            measured_cells(cases, display.get(class), &model, spec),
        );
    }
    let measured: Vec<SizeClass> = out.keys().copied().collect();
    for class in SizeClass::ALL {
        if measured.contains(&class) {
            continue;
        }
        let donor = SizeClass::ALL
            .into_iter()
            .rev()
            .filter(|c| *c < class)
            .chain(SizeClass::ALL.into_iter().filter(|c| *c > class))
            .find(|c| measured.contains(c));
        let Some(donor) = donor else {
            return Err("no payload measurements to build a matrix from".into());
        };
        let (p, d) = (out[&donor].payload.clone(), out[&donor].display.clone());
        out.insert(
            class,
            ClassCells {
                payload: p,
                display: d,
                source: Source::CopiedFrom(donor),
            },
        );
    }
    Ok(out)
}

/// `PayloadEncoding::...` as Rust source.
fn encoding_source(e: PayloadEncoding) -> String {
    match e {
        PayloadEncoding::Raw => "PayloadEncoding::Raw".to_string(),
        PayloadEncoding::ShuffleLz4 => "PayloadEncoding::ShuffleLz4".to_string(),
        PayloadEncoding::ShuffleZstd { level } => {
            format!("PayloadEncoding::ShuffleZstd {{ level: {level} }}")
        }
    }
}

/// One payload cell as Rust source, indented for the matrix body.
fn cell_source(list: &[PayloadEncoding]) -> String {
    if let [only] = list {
        return format!("        &[{}],\n", encoding_source(*only));
    }
    let mut s = String::from("        &[\n");
    for e in list {
        let _ = writeln!(s, "            {},", encoding_source(*e));
    }
    s.push_str("        ],\n");
    s
}

/// A comment saying where a class's row came from.
fn source_note(class: SizeClass, source: Source) -> String {
    match source {
        Source::Measured => format!("    // {}", class.name()),
        Source::CopiedFrom(from) => format!(
            "    // {} (extrapolated: copied from {}, nothing of this size was measured)",
            class.name(),
            from.name()
        ),
    }
}

/// The file header comment.
fn header(spec: &Spec<'_>, classes: &BTreeMap<SizeClass, ClassCells>) -> String {
    let model = match spec.rank {
        Rank::Serial => "serial (compress + transfer + decompress, summed)",
        Rank::Pipelined => "pipelined (max of compress, transfer, decompress)",
    };
    let ladder: Vec<String> = spec.tiers.iter().map(|t| format!("{t}")).collect();
    let mut s = String::new();
    s.push_str(
        "//! GENERATED FILE: the adaptive payload-encoding matrix. Do not edit by hand.\n//!\n",
    );
    s.push_str(
        "//! Regenerate from live measurements (PowerShell, repo root) and review the diff:\n//!\n",
    );
    let _ = writeln!(s, "//! ```text\n//! {}\n//! ```\n//!", spec.command);
    s.push_str("//! then format it with `rustfmt --edition 2024 <this file>`.\n//!\n");
    let _ = writeln!(
        s,
        "//! Generator settings of this revision: model {model}, cpu-share {}, tier ladder {} Mbit/s.",
        spec.cpu_share,
        ladder.join("/")
    );
    s.push_str("//! Rows are size classes by RAW payload bytes (small < 1 MiB, medium < 16 MiB, large >= 16 MiB),\n");
    s.push_str("//! columns are the bandwidth tiers; every payload cell is an ordered preference list ending in `Raw`.\n");
    for (class, cells) in classes {
        if let Source::CopiedFrom(from) = cells.source {
            let _ = writeln!(
                s,
                "//! EXTRAPOLATED: the {} row is copied from the {} row (no measurements of that size).",
                class.name(),
                from.name()
            );
        }
    }
    s.push('\n');
    s
}

/// The complete Rust source of the matrix file.
fn render(spec: &Spec<'_>, classes: &BTreeMap<SizeClass, ClassCells>) -> String {
    let tiers = spec.tiers.len();
    let mut s = header(spec, classes);
    s.push_str("use super::{DisplayEncoding, PayloadEncoding};\n\n");
    s.push_str("/// The tested bandwidth ladder in Mbit/s, ascending.\n");
    let ladder: Vec<String> = spec.tiers.iter().map(|t| format!("{t:?}")).collect();
    let _ = writeln!(
        s,
        "pub const TIERS_MBPS: [f64; {tiers}] = [{}];\n",
        ladder.join(", ")
    );
    s.push_str("/// Payload encoding preference per size class (small, medium, large) and bandwidth tier.\n");
    let _ = writeln!(
        s,
        "pub const PAYLOAD_MATRIX: [[&[PayloadEncoding]; {tiers}]; 3] = ["
    );
    for (class, cells) in classes {
        let _ = writeln!(s, "{}\n    [", source_note(*class, cells.source));
        for cell in &cells.payload {
            s.push_str(&cell_source(cell));
        }
        s.push_str("    ],\n");
    }
    s.push_str("];\n\n");
    s.push_str(
        "/// Display-frame encoding per size class (small, medium, large) and bandwidth tier.\n",
    );
    let _ = writeln!(
        s,
        "pub const DISPLAY_MATRIX: [[DisplayEncoding; {tiers}]; 3] = ["
    );
    for (class, cells) in classes {
        let _ = writeln!(s, "{}\n    [", source_note(*class, cells.source));
        for d in &cells.display {
            let _ = writeln!(s, "        DisplayEncoding::{d:?},");
        }
        s.push_str("    ],\n");
    }
    s.push_str("];\n");
    s
}

/// Builds the matrix source, prints it and (with `out`) writes it.
///
/// # Errors
///
/// No payload measurements, or an I/O error writing `out`.
pub fn emit(records: &[Record], spec: &Spec<'_>, out: Option<&Path>) -> Result<(), Box<dyn Error>> {
    let classes = build(records, spec)?;
    let source = render(spec, &classes);
    println!("\n=== generated encoding matrix ===\n{source}");
    if let Some(path) = out {
        fs::write(path, &source)?;
        println!("wrote the matrix to {}", path.display());
    }
    Ok(())
}
