//! Runs the unit tests of the video harness' pure helpers. `tests/tutorial_videos/main.rs` has
//! `harness = false` (a custom `main`), so `#[test]`s inside it never run; the helpers that need
//! no window live in their own files and are included here, each with its tests in a sibling file
//! (a test module inside the helper file would also be compiled, with unused imports, into the
//! harness).

#[path = "tutorial_videos/pure_tables.rs"]
mod pure_tables;
#[path = "tutorial_videos/pure_tables_tests.rs"]
mod pure_tables_tests;
#[path = "tutorial_videos/pure_targets.rs"]
mod pure_targets;
#[path = "tutorial_videos/pure_targets_tests.rs"]
mod pure_targets_tests;
