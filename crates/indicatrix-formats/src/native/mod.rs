//! The native design file: the self-contained `.isdd` file ([`design`], Indicatrix Suite
//! Design Document) that carries a whole design. Files saved as `.indicatrix` before the
//! rename are read exactly like `.isdd`; nothing writes `.indicatrix` any more.
//!
//! # The gap this closes
//!
//! `.asc`'s own schema is whatever `GemCAD` (and this crate's own [`asc`](crate::asc)
//! module) can express -- angles, mast distances, index-wheel positions, free-text
//! notes -- and nothing more. An editor built on top of `.asc` naturally wants to
//! author several things that have nowhere to live in that format at all:
//!
//! - A rough blank's own shape and size, independent of any cut facet.
//! - A tier's authored meet constraint as INTENT, not just a recorded number:
//!   `.asc`'s `G` field carries only free-text meet notes, and a constraint that pins
//!   a tier's mast to an exact scale reference has no field to round-trip through at
//!   all -- naive `.asc` reimport instead re-derives every tier's constraint fresh
//!   from geometry, silently discarding a deliberate "meet an existing facet" /
//!   "meet a named facet" choice.
//! - Which of a tier's symmetry-generated facets are exempt from group propagation.
//! - A real-world millimeter binding for the design's girdle diameter -- `.asc` is
//!   unitless.
//! - A material preset and specific-gravity/refractive-index override.
//!
//! # Format: TOML
//!
//! TOML over JSON for a file real people open, diff, and hand-edit: it supports
//! comments (`# this crown was copied from RBC-445`), and its `[[tiers]]`
//! array-of-tables keeps a one-tier edit to a one-line diff. Like JSON it is
//! self-describing (field names travel with the value, unlike `postcard`), so an
//! unrecognized key survives inspection rather than being silently misread by
//! position -- see the `unknown_keys_survive_at_every_level` test in [`design`].
//!
//! [`design::DesignFile`], [`PreformTable`], [`MaterialTable`], and [`TierTable`] each
//! carry a `#[serde(flatten)]` `unknown: toml::Table` field: keys a newer build wrote
//! that this build doesn't claim land there and are written back unchanged rather than
//! dropped. `toml::Table` is an ordered map, never a `HashMap`, so re-serializing a
//! value this build only reads reproduces its exact key order -- what keeps a
//! save-without-editing round trip byte-identical (see the determinism test).
//!
//! # The self-contained design file
//!
//! [`design`] holds the single-file `.isdd` format that carries a whole design with no
//! paired `.asc`. [`design::detect_kind`] tells a design file from anything else.
//!
//! # Module layout
//!
//! [`fingerprint`] is the SHA-256 helper; [`schema`] holds the shared tables the design
//! file is made of. Building those tables from -- or applying them back onto -- an
//! actual editor design is `indicatrix-cut-core`'s own `native` module's job, not this
//! crate's: this module only owns the on-disk document shape, never the in-memory editor
//! types it mirrors.

mod custom_material;
pub mod design;
mod fingerprint;
mod schema;
#[cfg(test)]
mod tests;

pub use custom_material::{ColorRecipeDto, CustomMaterialSnapshot, DispersionModelDto};
pub use fingerprint::sha256_hex;
pub use schema::{
    ConcaveTierTable, HistoryTable, MaterialTable, NativeMeetConstraint, NativePreformShape,
    NativeTierTarget, PreformTable, SourceTable, TierTable,
};
