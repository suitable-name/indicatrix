//! Stdout tables, best-pick lines, the recommendation block and the CSV writer.

use crate::{
    link::{Model, Rank, best, recommend},
    measure::{Kind, Record},
};
use std::{
    error::Error,
    fmt::Write as _,
    fs::File,
    io::{BufWriter, Write as _},
    path::Path,
};

/// The label with a marker when the encoder sent the payload raw instead.
fn label_with_fallback(r: &Record) -> String {
    let fell_back = r.sent_as == "Raw" && r.family != "Raw";
    if fell_back {
        format!("{} (sent Raw)", r.label())
    } else {
        r.label()
    }
}

/// Prints the table and best picks for one (data set, size) case.
pub fn print_case(
    set: &str,
    width: usize,
    height: usize,
    zero_fraction: f64,
    records: &[Record],
    model: &Model,
) {
    println!(
        "\n=== {set} {width}x{height} (zero f32 values: {:.1}%, raw frame {:.1} MB) ===",
        100.0 * zero_fraction,
        (width * height * 12) as f64 / 1e6
    );
    let mut head = format!(
        "{:<26} {:>11} {:>8} {:>10} {:>10} {:>9} {:>9}",
        "codec", "bytes", "ratio", "comp ms", "decomp ms", "comp MB/s", "dec MB/s"
    );
    for l in &model.links {
        let _ = write!(
            head,
            " | {:>9} {:>9}",
            format!("pipe@{l}"),
            format!("ser@{l}")
        );
    }
    println!("{head}");
    for r in records {
        println!("{}", row(r, model));
    }
    print_best(records, model);
}

/// One aligned table row.
fn row(r: &Record, model: &Model) -> String {
    let mut s = format!(
        "{:<26} {:>11} {:>8.3} {:>10.2} {:>10.2} {:>9.0} {:>9.0}",
        label_with_fallback(r),
        r.wire_bytes,
        r.ratio(),
        r.compress_s * 1e3,
        r.decompress_s * 1e3,
        r.compress_mbs(),
        r.decompress_mbs()
    );
    for &l in &model.links {
        let t = model.timing(r, l);
        let _ = write!(
            s,
            " | {:>9.1} {:>9.1}",
            1.0 / t.pipelined_s(),
            1.0 / t.serial_s()
        );
    }
    s
}

/// Formats a pick as `label (fps, cpu cost)`.
fn describe(r: Option<&Record>, model: &Model, mbps: f64, rank: Rank) -> String {
    r.map_or_else(
        || "none".to_string(),
        |r| {
            let t = model.timing(r, mbps);
            let secs = match rank {
                Rank::Pipelined => t.pipelined_s(),
                Rank::Serial => t.serial_s(),
            };
            format!(
                "{} ({:.1} fps, compress {:.1} ms/frame)",
                label_with_fallback(r),
                1.0 / secs,
                t.compress * 1e3
            )
        },
    )
}

/// Prints the best pick per link and kind for one case.
fn print_best(records: &[Record], model: &Model) {
    for kind in [Kind::Payload, Kind::Display] {
        let group: Vec<&Record> = records.iter().filter(|r| r.kind == kind).collect();
        if group.is_empty() {
            continue;
        }
        for &l in &model.links {
            let pipe = best(&group, model, l, Rank::Pipelined, false);
            let ser = best(&group, model, l, Rank::Serial, false);
            let mut line = format!(
                "  best [{}] @{l} Mbit/s: pipelined {} | serial {}",
                kind.name(),
                describe(pipe, model, l, Rank::Pipelined),
                describe(ser, model, l, Rank::Serial)
            );
            if model.min_compress_mbs > 0.0 {
                let floor = best(&group, model, l, Rank::Pipelined, true);
                let _ = write!(
                    line,
                    " | with compress >= {} MB/s: {}",
                    model.min_compress_mbs,
                    describe(floor, model, l, Rank::Pipelined)
                );
            }
            println!("{line}");
        }
    }
}

/// Prints the per-link recommendation block over every frame-kind case.
pub fn print_recommendations(records: &[Record], model: &Model) {
    println!(
        "\n=== recommendation (frame payloads; geometric mean of pipelined frame time over all cases) ==="
    );
    println!(
        "compare with DEFAULT_SERVER_PREFERENCE = [ShuffleZstd {{ level: 1 }}, ShuffleLz4, Raw] \
         in crates/indicatrix-net/src/messages/encoding.rs"
    );
    for &l in &model.links {
        let (list, verdict) = recommend(records, model, l);
        println!("link {l} Mbit/s: {list}");
        println!("    {verdict}");
    }
}

/// Writes every measurement as CSV, columns in a fixed order, links in command-line order.
///
/// # Errors
///
/// Any I/O error creating or writing `path`.
pub fn write_csv(path: &Path, records: &[Record], model: &Model) -> Result<(), Box<dyn Error>> {
    let mut out = BufWriter::new(File::create(path)?);
    let mut head = String::from(
        "dataset,width,height,kind,family,level,sent_as,raw_bytes,wire_bytes,ratio,\
         compress_s,decompress_s,compress_mbs,decompress_mbs,reps,cpu_share",
    );
    for l in &model.links {
        let _ = write!(
            head,
            ",t_transfer_s@{l},pipelined_s@{l},serial_s@{l},fps_pipelined@{l},fps_serial@{l}"
        );
    }
    writeln!(out, "{head}")?;
    for r in records {
        let mut line = format!(
            "{},{},{},{},{},{},{},{},{},{:.6},{:.9},{:.9},{:.3},{:.3},{},{}",
            r.set,
            r.width,
            r.height,
            r.kind.name(),
            r.family,
            r.level,
            r.sent_as.replace(',', ";"),
            r.raw_bytes,
            r.wire_bytes,
            r.ratio(),
            r.compress_s,
            r.decompress_s,
            r.compress_mbs(),
            r.decompress_mbs(),
            r.reps,
            model.cpu_share
        );
        for &l in &model.links {
            let t = model.timing(r, l);
            let _ = write!(
                line,
                ",{:.9},{:.9},{:.9},{:.4},{:.4}",
                t.transfer,
                t.pipelined_s(),
                t.serial_s(),
                1.0 / t.pipelined_s(),
                1.0 / t.serial_s()
            );
        }
        writeln!(out, "{line}")?;
    }
    out.flush()?;
    Ok(())
}
