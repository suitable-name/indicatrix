//! Readers and writers for gemstone faceting design file formats.
//!
//! `indicatrix-formats` is an independent, unaffiliated implementation of file formats originating with `GemCAD` (Robert
//! Strickland's faceting-design software) and, in future, Gem Cut Studio. It is not
//! produced or endorsed by either. Each format lives in its own module so that
//! reading (or writing) a design never requires pulling in a particular renderer,
//! database, or GUI toolkit:
//!
//! - [`asc`]: `GemCAD`'s `.asc` cutting-schedule text format. Read and write support,
//!   verified against a real-world corpus of 5,759 files.
//! - [`native`]: `apps/indicatrix-cut`'s own `.indicatrix.toml` sidecar (legacy
//!   `.gemcut.toml` files still load) -- the on-disk schema, TOML encode/decode,
//!   sidecar path rules, and paired-`.asc` fingerprint for a text file that carries
//!   design state `.asc` itself has no field for. Converting to and from the actual
//!   in-memory editor design is `indicatrix-cut-core`'s job, not this module's -- see
//!   its own module doc comment for the split.
//! - `gem` / `gcs` (not yet implemented): `GemCAD`'s native `.gem` format and Gem Cut
//!   Studio's `.gcs` format are natural future additions here, since some real-world
//!   designs exist only as one of those and have no `.asc` counterpart at all.
//!
//! Each format module pulls in only what it needs: [`asc`] has zero runtime
//! dependencies, while [`native`] depends on `serde`/`toml` (its on-disk encoding) and
//! `sha2` (its fingerprint) -- see that module's own doc comment. Anything shared
//! across more than one format's module belongs at this crate root rather than inside
//! a single format module; nothing has met that bar yet.

pub mod asc;
pub mod native;
