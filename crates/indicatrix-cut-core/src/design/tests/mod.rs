//! Tests for [`super`], split by topic: shared real-fixture data, basic
//! solve/close sanity, `.asc` import classification, the discard-and-resolve
//! corpus-fidelity gate, the `resolve_dirty`-vs-full-`solve` equivalence gate,
//! `solve_with`/cancellation, large-fixture speed, and
//! `FreshDesignSpec`/effective-refractive-index behavior.

mod fixtures;

mod asc_import;
mod basic;
mod discard_resolve_gate;
mod fresh_and_refractive;
mod large_fixture_speed;
mod resolve_dirty_equivalence;
mod solve_with_cancellation;
