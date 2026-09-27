//! Reader and writer for `GemCAD`-style `.asc` cutting-schedule files.
//!
//! `GemCAD` is Robert Strickland's faceting-design software; this module is not
//! produced, endorsed, or affiliated with `GemCAD` or its author (see the crate-level
//! docs for the full affiliation note). It exists because the `.asc` text format
//! `GemCAD` popularized is a de facto standard across the faceting community --
//! shared, archived, and re-published by many independent designers and sites -- and
//! reading (or writing) a cutting schedule shouldn't require pulling in any
//! particular renderer, database, or GUI toolkit.
//!
//! An `.asc` file's `a` records are the only place a schedule's "mast" distance --
//! how far a facet plane is cut from the stone's center -- actually lives; a
//! design's angle/index metadata is sometimes available elsewhere (e.g. scraped from
//! a catalog site) without the depth, which is exactly the gap this format fills.
//!
//! # Format (as verified against a real-world corpus of 5,759 `.asc` files spanning
//! 2,881 distinct designs, not just the format sketch)
//!
//! ```text
//! GemCad 5.0
//! g 96 0.0                                       <- gear teeth, reference angle
//! y 6 y                                           <- symmetry order, mirror flag (y/n)
//! I 1.72                                          <- refractive index
//! H PC 45.149  Round Trichecker-12                <- header/title lines (repeatable)
//! H by Fred W. Van Sant, X 51, Extra Designs 2000
//! a -41.000000 0.64991234 92 n 1 84 76 68 60 ...  <- tier: angle, mast, indices/name
//! F "For small stones"                            <- footnote (repeatable)
//! ```
//!
//! Each `a` record is: a signed `angle` (degrees from the girdle plane; `GemCAD`'s
//! convention is negative = pavilion, non-negative = crown), the `mast` distance (the
//! field this crate exists to extract reliably), then a mix of index-wheel positions
//! (usually integers, occasionally fractional) and `n <name>` markers, and finally an
//! optional `G <notes...>` free-text tail. A record can list its facet's indices in
//! more than one `n <name>` group (e.g. `92 n c ... 94 n d ...` when a compact,
//! single-tier encoding would otherwise need two rows at an identical angle/mast);
//! all of them are folded into one [`AscTier`]'s `indices`, since every one of them
//! wants a plane at the same angle and depth -- only the azimuth (index) differs.
//!
//! Long index lists wrap onto continuation lines that do not start with `a` (verified
//! against real files: continuation lines starting with a bare number, with `n`, or
//! with `G`). [`parse_asc`] treats any line that doesn't start with one of the known
//! record keywords (`GemCad`, `g`, `y`, `I`, `H`, `F`, `a`) as a continuation of
//! whatever `a` record is currently open.
//!
//! Beyond the read path, [`parse_asc`]'s lenient handling absorbs several corpus
//! realities a naive reading of the format sketch would miss: continuation lines with
//! no `a` prefix, facet names that are themselves plain numbers (name-vs-index told
//! apart only by position right after an `n` marker), fractional index positions,
//! negative gear-teeth counts (an internal handedness convention), one real file
//! missing its `g` keyword entirely, and a rare negative mast value.
//!
//! # Writing schedules
//!
//! [`to_asc_string`] serializes an [`AscSchedule`] back to `.asc` text. It is not
//! byte-identical to hand-authored `GemCAD` output (whitespace, field order within a
//! tier, and how repeated `n <name>` groups get folded back down are all
//! normalized), but it round-trips semantically: parsing its output reproduces an
//! equal [`AscSchedule`]. See the `round_trip` tests in this module's own `tests`
//! submodule, which exercise that property against real schedules pulled from the
//! corpus.

mod error;
mod meet_instruction;
mod parse;
mod schedule;
mod tier;
mod write;

#[cfg(test)]
mod tests;

pub use error::AscParseError;
pub use meet_instruction::MeetInstruction;
pub use parse::parse_asc;
pub use schedule::AscSchedule;
pub use tier::AscTier;
pub use write::{mark_reconstructed, to_asc_string};
