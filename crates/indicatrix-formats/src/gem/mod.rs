//! Reader for `GemCAD`'s native `.gem` binary save format.
//!
//! `GemCAD` is Robert Strickland's faceting-design software (see [`crate::asc`]'s
//! module docs for the affiliation note, which applies equally here). `.gem` is its
//! own binary save format, distinct from the `.asc` cutting instructions `GemCAD`
//! also exports. `GemCAD`'s documentation calls it proprietary and publishes no
//! layout. The layout below was decoded from a real corpus of 254 `.gem` files
//! (`attached_files`) and agrees
//! with the record order two third-party readers use (mbparker's
//! `gemcad-file-reader`, `OpenGemCutting`). All 254 files frame byte-exactly from
//! offset 0 to end of file.
//!
//! # Layout (all little-endian, no header or magic number)
//!
//! ```text
//! facet record, repeated:
//!   f64 px, py, pz        plane vector p with p·x = 1 for points x on the facet
//!   i32 tier              groups the facets of one tier
//!   u8 len, len bytes     label "name\tinstructions", Windows-1252
//!   { i32 1, f64 x, y, z } ...   the facet's polygon
//!   i32 0                 end of vertices
//! f64 -99999.0            end of facets (in place of the next px)
//! trailer:
//!   i32 symmetry, i32 mirror (0/1), i32 gear (signed), f64 refractive index,
//!   u32 unknown (0x7FFF in every corpus file), f64 gear offset,
//!   8 x (u8 len, bytes)   headings H1..H4 then footnotes F1..F4
//! optional: u8 7, "preform", then a complete facet list + sentinel + trailer
//! ```
//!
//! The file stores no angles, indices or depths: every one of them derives from the
//! plane vector `p`. The unit normal is `p/|p|`, the plane's distance from the
//! centre is `1/|p|` (the `.asc` mast), and the facet angle is `acos(|p̂z|)`, signed
//! by `pz` (crown `+`, pavilion `-`). A 90° facet stores a `pz` of `±6e-17` or `±0.0`
//! whose sign carries the girdle side, `.asc`'s `+90`/`-90`. See
//! [`GemFacet::angle_deg`] and [`GemFacet::index`].
//!
//! # Verification
//!
//! - Three designs have an exact `.asc` export elsewhere in the corpus (6095 ↔ 994,
//!   6367 ↔ 2488, and the chiral Wittelsbach replica 6508 ↔ 158 with gear `-96`,
//!   offset `48`). [`gem_to_asc_schedule`] reproduces every tier's angle, mast and
//!   index set to 6 decimals. The chiral pair only matches with the signed gear and
//!   the offset applied: `phi = 90° - 360°·(i - offset)/gear`.
//! - In 224 of 254 files every stored vertex satisfies `p·v = 1` to `1e-6`.
//!
//! # What is still open
//!
//! - The trailer's `u32` is `0x7FFF` in all 254 files. Its meaning is unknown; it is
//!   kept raw in [`GemDesign::unknown_7fff`].
//! - Tier numbers are 0-based, 1-based or gappy, and one file lists them out of
//!   order. They are treated as grouping keys only.
//! - No corpus string is longer than 120 bytes, so a 1-byte length cannot be told
//!   apart from a .NET 7-bit varint. [`parse_gem`] reads a single byte and falls
//!   back to the varint reading only when that fails and a length byte had bit 7
//!   set, accepting the varint reading only if it frames the file to end of file.
//! - **0.81 files (hypothesis).** In 30 files (29 from gemologyproject.com) every
//!   vertex lies at `p·v = 0.81` instead of `1`: vertices and planes are scaled by
//!   different factors. Orb.gem is one of them and its `.gcs` sibling has depth
//!   exactly `0.9 · (1/|p|)`, which suggests these files were exported by Gem Cut
//!   Studio. That is a hypothesis; no GCS export was available to confirm it. The
//!   planes are authoritative. [`GemDesign::vertex_scale`] records the median
//!   `p·v`, and [`GemFacet::vertices_on_plane`] rescales the stored vertices.
//! - Positive-gear chirality is inferred from the signed formula; no chiral
//!   positive-gear `.asc` pair exists in the corpus.

mod convert;
mod error;
mod model;
mod parse;

#[cfg(test)]
mod tests;

pub use convert::gem_to_asc_schedule;
pub(crate) use convert::snap_index;
pub use error::GemParseError;
pub use model::{GemDesign, GemFacet, GemNote};
pub use parse::parse_gem;
