//! View-model unit tests, split by topic: [`inline`] (the small formatting/helper
//! tests), [`material`] (material/RI and gear presets), [`rows`] (tier-row
//! builders) and [`status`] (the validation-banner text), plus the tierless
//! dash-out checks for the `_from_solved` proportion mirrors below.

mod inline;
mod material;
mod rows;
mod status;

use super::yield_report::{girdle_and_ratio_texts_from_solved, proportions_texts_from_solved};
use crate::EditorSession;

// The `_from_solved` mirrors must dash out a tierless design too, not just their
// plain (internally-solving) counterparts: a tierless design still SOLVES (an empty
// mast list is a valid, closed, zero-plane solve), so a UI's hot path can hand them
// exactly that solved-empty list.

#[test]
fn proportions_texts_from_solved_dashes_out_a_design_with_no_tiers() {
    let design = EditorSession::fresh().design;
    let solved = design.solve().expect("a tierless design still solves");
    assert_eq!(
        proportions_texts_from_solved(&design, &solved),
        (
            "-".to_string(),
            "-".to_string(),
            "-".to_string(),
            "-".to_string(),
            "-".to_string(),
        ),
        "a tierless design's proportions must never show the bare preform \
         block's own numbers (table 100%, total depth = preform depth) as \
         if they were the stone's"
    );
}

#[test]
fn girdle_and_ratio_texts_from_solved_dashes_out_a_design_with_no_tiers() {
    let design = EditorSession::fresh().design;
    let solved = design.solve().expect("a tierless design still solves");
    assert_eq!(
        girdle_and_ratio_texts_from_solved(&design, &solved),
        (
            "-".to_string(),
            "-".to_string(),
            "-".to_string(),
            "-".to_string(),
        )
    );
}
