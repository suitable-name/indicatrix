//! Yield/weight and proportion view-model text, the proportion verdicts, and the
//! Edit tab's cut-order schedule rows. The logic lives in
//! `indicatrix_editor::view_model::yield_report` (shared with the web app); this
//! file re-exports it and maps the plain cut-order rows to the Slint `AngleItem`.

use crate::AngleItem;
use indicatrix::geometry::meet_solver::SolvedTier;
use indicatrix_cut_core::Design;

pub(in crate::gui::editor) use indicatrix_editor::view_model::yield_report::{
    design_label_text, girdle_and_ratio_texts, preform_mm_texts, preform_y_offset_mm_text,
    proportion_verdicts, proportions_texts, yield_report_texts, yield_report_texts_from_solved,
};

/// [`indicatrix_editor::view_model::yield_report::cutting_instructions_rows`], mapped
/// to the Slint `AngleItem`s the Edit tab's schedule shows.
pub(in crate::gui::editor) fn cutting_instructions_rows(
    design: &Design,
    solved: &[SolvedTier],
) -> Vec<AngleItem> {
    indicatrix_editor::view_model::yield_report::cutting_instructions_rows(design, solved)
        .into_iter()
        .map(|row| AngleItem {
            order_idx: row.order_idx,
            side: row.side,
            facet: row.facet.into(),
            angle: row.angle.into(),
            index_val: row.index_val.into(),
            notes: row.notes.into(),
            // The tool columns joined the way the cutting sheet's text prints them.
            second_line: row
                .second_line
                .map_or_else(String::new, |fields| fields.join("  "))
                .into(),
        })
        .collect()
}
