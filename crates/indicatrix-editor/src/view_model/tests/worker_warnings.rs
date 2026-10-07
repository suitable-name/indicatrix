//! The manufacturability findings a worker computes and a UI thread only displays:
//! [`solved_manufacturability_warnings`], [`tier_items_from_solved_with_warnings`] and
//! [`manufacturability_warnings_tagged_with`] must show what the inline pass shows.

use crate::view_model::{TierRowKind, rows::*};
use indicatrix_cut_core::{Design, ManufacturabilityWarning};

#[test]
fn precomputed_findings_give_the_rows_and_banner_the_inline_pass_gives() {
    for design in [
        Design::concave_fixture(),
        crate::EditorSession::fresh().design,
    ] {
        let solved = design.solve().expect("the design solves");
        let n_d = design.effective_refractive_index();
        let warnings = solved_manufacturability_warnings(&design, &solved);
        assert_eq!(
            tier_items_from_solved_with_warnings(&design, &solved, n_d, Some(&warnings)),
            tier_items_from_solved(&design, &solved, n_d)
        );
        assert_eq!(
            manufacturability_warnings_tagged_with(&design, &solved, Some(&warnings)),
            manufacturability_warnings_tagged(&design, Some(&solved))
        );
    }
}

/// Without a worker pass a design WITHOUT concave tools gets the inline pass it always had.
#[test]
fn without_a_worker_pass_a_planar_design_keeps_the_inline_findings() {
    let mut design = Design::concave_fixture();
    design.concave_tiers.clear();
    design.concave_tier_ids.clear();
    let solved = design.solve().expect("the design solves");
    let n_d = design.effective_refractive_index();
    assert_eq!(
        tier_items_from_solved_with_warnings(&design, &solved, n_d, None),
        tier_items_from_solved(&design, &solved, n_d)
    );
    assert_eq!(
        manufacturability_warnings_tagged_with(&design, &solved, None),
        manufacturability_warnings_tagged(&design, Some(&solved))
    );
}

/// Without a worker pass the rows of a design WITH concave tools show only the mast-free
/// findings, tagged as pre-solve, and never build a mesh: identical to the stale path's own
/// findings.
#[test]
fn without_a_worker_pass_only_the_mast_free_findings_show_tagged() {
    let mut design = Design::concave_fixture();
    // A fractional index is a mast-free finding.
    design.tiers[1].indices = vec![0.5, 4.0, 8.0, 12.0];
    let solved = design.solve().expect("the design solves");
    let n_d = design.effective_refractive_index();
    let tagged = manufacturability_warnings_tagged_with(&design, &solved, None);
    assert_eq!(tagged, manufacturability_warnings_tagged(&design, None));
    assert!(
        tagged
            .iter()
            .all(|(_, text)| text.starts_with("(pre-solve) ")),
        "{tagged:?}"
    );
    let rows = tier_items_from_solved_with_warnings(&design, &solved, n_d, None);
    assert_eq!(rows.len(), design.tiers.len() + design.concave_tiers.len());
    assert!(
        rows[1].warning_text.starts_with("(pre-solve) "),
        "{:?}",
        rows[1].warning_text
    );
}

/// A worker's concave finding lands on its own concave row, not on a flat row, and the flat
/// rows keep theirs.
#[test]
fn a_worker_finding_about_a_concave_tool_badges_its_concave_row() {
    let design = Design::concave_fixture();
    let solved = design.solve().expect("the design solves");
    let n_d = design.effective_refractive_index();
    let findings = [
        ManufacturabilityWarning::ToolEnclosed {
            tier: 1,
            placement: 0,
        },
        ManufacturabilityWarning::ToolsOverlap { a: 0, b: 1 },
    ];
    let rows = tier_items_from_solved_with_warnings(&design, &solved, n_d, Some(&findings));
    let (flat, concave) = rows.split_at(design.tiers.len());
    assert!(
        concave[1].warning_text.contains("inside the stone"),
        "{concave:?}"
    );
    assert_eq!(concave[0].warning_text, "");
    assert!(flat.iter().all(|row| row.warning_text.is_empty()));
    assert!(concave.iter().all(|row| row.kind == TierRowKind::Concave));
    // The overlap names no tier, so it is the banner's, not a row's.
    assert_eq!(
        manufacturability_warnings_tagged_with(&design, &solved, Some(&findings)),
        [] as [(usize, String); 0]
    );
}
