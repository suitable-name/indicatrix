//! [`to_asc_string`]: serializes an [`AscSchedule`] back to `.asc` text, and
//! [`mark_reconstructed`]: marks a schedule as derived rather than authored.

use super::schedule::AscSchedule;
use std::fmt;

/// Serializes an [`AscSchedule`] back to `.asc` text.
///
/// Not byte-identical to hand-authored `GemCAD` output -- whitespace, the exact
/// token order within a tier line, and how repeated `n <name>` groups collapse back
/// down are all normalized -- but it round-trips semantically:
/// `parse_asc(&to_asc_string(s))` reproduces a schedule equal to `s`. Every numeric
/// field is written with `f64`'s/`i32`'s default `Display` formatting, which Rust
/// guarantees produces the shortest decimal string that reads back to the exact same
/// value, so no precision is lost across the round trip.
#[must_use]
pub fn to_asc_string(schedule: &AscSchedule) -> String {
    use fmt::Write as _;

    let mut out = String::new();

    let _ = writeln!(out, "GemCad {}", schedule.gemcad_version);
    let _ = writeln!(
        out,
        "g {} {}",
        schedule.gear_teeth, schedule.gear_reference_angle
    );
    let _ = writeln!(
        out,
        "y {} {}",
        schedule.symmetry_order,
        if schedule.mirror { "y" } else { "n" }
    );
    let _ = writeln!(out, "I {}", schedule.refractive_index);
    for header in &schedule.headers {
        let _ = writeln!(out, "H {header}");
    }

    for tier in &schedule.tiers {
        let _ = write!(out, "a {} {}", tier.angle_deg, tier.mast);
        for idx in &tier.indices {
            let _ = write!(out, " {idx}");
        }
        if !tier.name.is_empty() {
            let _ = write!(out, " n {}", tier.name);
        }
        if !tier.notes.is_empty() {
            let _ = write!(out, " G {}", tier.notes);
        }
        out.push('\n');
    }

    for footnote in &schedule.footnotes {
        let _ = writeln!(out, "F {footnote}");
    }

    out
}

/// Prepends a clear "this is derived, not authored" marker to `schedule`'s headers.
///
/// A reconstructed `.asc` (mast distances solved from angles and meet constraints,
/// not the original design's own measured masts) must never be mistaken for a
/// hand-authored, verified cutting schedule -- a user must be able to tell the two
/// apart before cutting a stone from either. Idempotent: calling it again on a
/// schedule that already carries a `RECONSTRUCTED` marker as its first header does
/// not add a second one.
///
/// `note` should say how the schedule was derived (e.g. which solver, and any caveat
/// about accuracy); it is appended after the marker on the same header line.
pub fn mark_reconstructed(schedule: &mut AscSchedule, note: &str) {
    let already_marked = schedule
        .headers
        .first()
        .is_some_and(|h| h.starts_with("RECONSTRUCTED"));
    if already_marked {
        return;
    }
    schedule.headers.insert(
        0,
        format!("RECONSTRUCTED -- mast distances are solved, not original -- {note}"),
    );
}
