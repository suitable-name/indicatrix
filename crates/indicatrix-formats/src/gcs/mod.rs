//! Reader and (experimental) writer for Gem Cut Studio's `.gcs` design format.
//!
//! Gem Cut Studio ("GCS") is a Windows faceting-design application distinct from
//! `GemCAD`; this module is not produced, endorsed, or affiliated with Gem Cut
//! Studio or its author (see the crate-level docs for the full affiliation note).
//! Some real-world designs are only ever published as a `.gcs` file, and reading one
//! shouldn't require Gem Cut Studio itself.
//!
//! # Format
//!
//! GCS publishes the format: Gem Cut Studio User's Manual v1.1.0, pp. 58-61, "The
//! .GCS file format", an annotated example that marks required versus optional
//! fields. Page numbers below refer to that manual. A `.gcs` file is plain XML with
//! one root wrapping a flat element tree:
//!
//! ```text
//! <GemCutStudio version="1000">
//!     <index gear="64" base="0" symmetry="4" mirror="0"/>
//!     <tier angle="126.39" depth="0.6664" name="P1" instructions="" visible="true" guide="false">
//!         <facet nx="-0" ny="-0.805" nz="-0.593" index_angle="0">
//!             <vertex x="-0.0827" y="-0.8186" z="-0.0126"/>
//!             ...
//!         </facet>
//!         ...
//!     </tier>
//!     ...
//!     <render material="176 Corundum" refractive_index="1.76" dispersion="0.018" clarity="100" density="1.4" lighting_model="Random">
//!         <color r="0.71" g="0.73" b="0.95"/>
//!     </render>
//!     <info title="..." author="..." date="..." ri_min="1.7" ri_max="2.15" shape="Octagon" footer1="..." footer2="..."/>
//! </GemCutStudio>
//! ```
//!
//! Required (p.58-59): the root (its `version` "is checked to be less than or equal
//! to app version"), `index gear`, `tier angle`, and per facet enough to recover
//! the plane. Everything else is optional, and [`parse_gcs`] treats it so: the
//! `<index>` UI state defaults, a missing facet normal is derived from the tier
//! angle and `index_angle`, a missing `index_angle` from the normal, and a missing
//! tier `depth` from the vertices. The tokenizer accepts XML comments (the manual's
//! own example is full of them), an `<?xml ...?>` declaration, single-quoted
//! attributes and numeric character references. Unknown elements and attributes are
//! kept out of the model and listed in [`GcsDesign::warnings`].
//!
//! Encoding: undeclared; GCS 1.1 writes Windows-1252 (corpus file id 2213 holds a
//! raw `0xBA`). Read files with [`parse_gcs_bytes`].
//!
//! # Conventions, measured on a 59-file corpus (56 with an `.asc` sibling)
//!
//! - **Angle** ([`GcsTier::angle_deg`]): a polar angle, `0` table, `90` girdle
//!   ("always included in pav", p.58), above 90 pavilion, `180` flat culet.
//!   [`GcsTier::to_signed_asc_angle`] maps it to `.asc`'s signed angle (girdle
//!   `-90`, culet `-0`). Values carry `f32` residue (`126.38999938964842`).
//! - **Frame**: GCS rescales every design so `max(|x|,|y|) = 1` and centres its
//!   z-range on 0 (59/59 files, exactly), "re-scaled after each operation to fill
//!   the workspace" (p.10). The manual's frame comment ("negative X is front (index
//!   0)", p.59) is contradicted by its own example, which puts index 0 at `-y`.
//! - **Depth** ([`GcsTier::depth`]): the `.asc` mast in that normalised frame,
//!   `depth = n·v` for the tier's vertices (≤ 3e-5 over 17,828 vertices). Against a
//!   `GemCAD` `.asc` sibling, `depth = s·|mast| - s·z0·nz` with `s` ≈ 0.90 and `z0`
//!   the `.asc` frame's z-centre: residual ≤ 2e-3 in 34 of 56 pairs, ≤ 9e-3 in 53. The
//!   one `.gem`/`.gcs` pair (Orb) gives `depth = 0.9·d_gem` exactly. The manual's own
//!   example is inconsistent here: its depths do not match its vertices.
//! - **Index** ([`GcsFacet::index_angle_deg`]): the tooth in absolute degrees,
//!   `tooth / gear · 360`. The winding is side-dependent ("pav vs crown has opposite
//!   index ordering", p.59): with `phi = atan2(ny, nx)`, crown facets have
//!   `index_angle = 90° + phi` and pavilion and girdle facets `270° - phi` (4,836 of
//!   4,836 non-flat facets). [`side_rule_index_angle`] and [`GcsFacet::index`]
//!   implement it. A flat facet (table, culet) carries an arbitrary value. The
//!   corpus has no chiral design, so the rule is verified on labels and normals only.
//! - **`<index base/symmetry/mirror>`** are UI state (p.58: "NOT the values for
//!   symmetry/mirror as would be printed in a faceting diagram"): base index,
//!   the symmetry of the tier being cut, and the ± mirror offset in index steps
//!   (pp.36-44). They do not describe the design.
//! - **`frosting`** (tier, p.9 and the p.58 example): `0` clear, `0.5` frosted as GCS
//!   1.1 writes it; kept in [`GcsTier::frosting`] and, per third-party writers, per
//!   facet in [`GcsFacet::frosting`].
//!
//! # Conversion and writing
//!
//! [`gcs_to_asc_schedule`] turns a design into `.asc` cutting instructions (masts
//! stay in the normalised frame). [`to_gcs_string`] writes cutting instructions as
//! a `.gcs` file (experimental): it solves the facet polygons as the faces of the
//! convex polytope of the schedule's planes, applies GCS's normalisation and the
//! side-dependent `index_angle`, and writes CRLF ASCII with `version="1000"`.
//! Parsing its output and converting back reproduces every tier's angle, depth,
//! index set and name.

mod convert;
mod design;
mod error;
mod metadata;
mod parse;
mod polytope;
mod tier;
mod tokenize;
mod write;

#[cfg(test)]
mod tests;

pub use convert::gcs_to_asc_schedule;
pub use design::GcsDesign;
pub use error::GcsParseError;
pub use metadata::{GcsColor, GcsIndex, GcsInfo, GcsRender};
pub use parse::{parse_gcs, parse_gcs_bytes};
pub use tier::{GcsFacet, GcsTier, GcsVertex, normal_from_index_angle, side_rule_index_angle};
pub use write::{GcsWriteError, to_gcs_string};
