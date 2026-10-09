//! Synthetic validation of the rough-colour-from-photos feature (plan 2026-10-09, section 10.1;
//! lane V1). Built and run only with the `zoning` feature:
//!
//! ```text
//! cargo test -p indicatrix-cut-core --features zoning --test zoning_validation
//! ```
//!
//! Every test runs by default (the full matrix included; there are no `#[ignore]`d tests).

#![cfg(feature = "zoning")]
#![allow(
    clippy::needless_range_loop,
    clippy::many_single_char_names,
    clippy::too_many_lines,
    reason = "test code"
)]

mod boundary;
mod cross_check;
mod determinism;
mod recovery;
mod synth;
