//! Shape classification from the girdle outline.

use super::super::measure::classify_shape;
use indicatrix::geometry::cuts::StandardGemCuts;

/// The strongest available anchor: the built-in standard round brilliant has 16
/// girdle facets, so the outline-based rule must call it Round.
#[test]
fn classify_shape_calls_the_standard_round_brilliant_round() {
    let planes = StandardGemCuts::standard_round_brilliant();
    assert_eq!(
        classify_shape(&planes, 1.0).as_deref(),
        Some("Round"),
        "16 girdle facets at lw == 1.0 must classify as Round"
    );
}

/// The regression this rule exists for. A fold-count rule called "Round
/// Trichecker-12" a Hexagon, because its schedule declares 6-fold symmetry while
/// the cut is round. Classification keys on the girdle OUTLINE instead, so a
/// round outline stays Round no matter what fold count the schedule declares --
/// `classify_shape` no longer receives `symmetry_order` at all, which is what
/// makes that misreading unrepresentable rather than merely unlikely.
#[test]
fn classify_shape_ignores_fold_count_entirely() {
    let planes = StandardGemCuts::standard_round_brilliant();
    // Same planes, and no symmetry_order is threaded in from anywhere: the only
    // inputs are the outline and the measured ratio.
    assert_eq!(classify_shape(&planes, 1.0).as_deref(), Some("Round"));
}

/// An elongated stone is never guessed at, however round-looking its outline:
/// Oval/Marquise/Pear cannot be told apart by side count alone, so the honest
/// answer is no shape rather than a confident wrong one.
#[test]
fn classify_shape_refuses_to_guess_for_an_elongated_outline() {
    let planes = StandardGemCuts::standard_round_brilliant();
    assert_eq!(
        classify_shape(&planes, 1.6),
        None,
        "a 1.6 length/width ratio must not be classified Round"
    );
}

/// No girdle facets at all (or too few to be confident) yields no shape rather
/// than a panic or a default.
#[test]
fn classify_shape_returns_none_without_a_usable_girdle() {
    assert_eq!(classify_shape(&[], 1.0), None);
}

/// Saving a design with concave tiers records them on the catalogue row: the tier
/// count, the placement count, the tool rows (which the `has_concave` search filter and
/// the library's second line read), and the flat rows stay the only measured ones.
#[test]
fn saving_the_concave_fixture_records_its_concave_tiers_and_tool_lines() {
    use super::super::measure::apply_measured_metadata;
    use indicatrix_cut_core::{
        Design,
        native::{DesignExtras, design_to_string},
    };
    use indicatrix_vault::{db::sqlite::Database, model::filter::RangeFilter};

    let design = Design::concave_fixture();
    let text = design_to_string(&design, None, &DesignExtras::default()).expect("serializes");
    let mut imported =
        indicatrix_vault::local::import_native_design("fixture.indicatrix", text.as_bytes())
            .expect("imports");
    let flat_rows = imported.detail.angle_settings_table.len();
    apply_measured_metadata(&mut imported.detail);
    let detail = &imported.detail;
    assert_eq!(detail.concave_tiers, 2);
    assert_eq!(detail.concave_facets, 8 + 4);
    assert_eq!(detail.angle_settings_table.len(), flat_rows + 2);
    let tools: Vec<_> = detail
        .angle_settings_table
        .iter()
        .filter(|r| r.tool.is_some())
        .collect();
    assert_eq!(tools.len(), 2);
    assert_eq!(tools[0].facet, "Groove");
    assert_eq!(tools[0].tool.as_deref(), Some("CYL"));
    assert_eq!(
        tools[0].tool_line.as_deref(),
        Some(
            design.concave_tiers[0]
                .second_line_fields()
                .join("  ")
                .as_str()
        )
    );
    // Running it again must not duplicate the rows.
    let mut again = imported.detail.clone();
    apply_measured_metadata(&mut again);
    assert_eq!(again.angle_settings_table.len(), flat_rows + 2);

    let db = Database::new(Some(":memory:")).expect("db");
    let id = db
        .save_design(&imported.entry, &imported.detail, "local-import")
        .expect("saves");
    let range = RangeFilter {
        has_concave: Some(true),
        ..Default::default()
    };
    let hits = db
        .search_diagrams("", "All", "All", &range)
        .expect("searches");
    assert!(hits.iter().any(|h| h.id == id), "has_concave must match");
    assert_eq!(db.entry_concave_tiers(id).expect("count"), 2);
}
