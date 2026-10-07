//! Tests for the printed layout: the header block, Facet Data, Size Data, Design Data, the four
//! views, comments, the Pavilion/Crown split, [`SheetDetails`] and the text sheet.
//!
//! Every expected number is worked out by hand from the fixtures' own tier tables.

use super::{
    CutViews, SheetDetails, SheetLayout, cutting_sheet_document, cutting_sheet_document_with,
    cutting_sheet_html, cutting_sheet_html_with, cutting_sheet_text_with, month_year_text,
    render_cut_views,
    tests::{label_cells, round_brilliant_design, section_html},
};
use indicatrix_cut_core::{
    Design, MaterialSelection, PreformSpec, ScheduleMeta, native::DesignMetadata,
};

/// Byte positions of `needles` in `haystack`, each searched after the previous one, so the
/// result is ascending exactly when the needles appear in that order.
fn positions_in_order(haystack: &str, needles: &[&str]) -> Vec<usize> {
    let mut from = 0;
    needles
        .iter()
        .map(|needle| {
            let at = from
                + haystack[from..]
                    .find(needle)
                    .unwrap_or_else(|| panic!("{needle:?} missing or out of order"));
            from = at + needle.len();
            at
        })
        .collect()
}

// ---- Facet Data ----------------------------------------------------------------------

/// The Standard Round Brilliant of the fixture, counted by hand. Pavilion: Pavilion Main 8 +
/// Lower Girdle 16 + Culet 1 (an empty index list is one facet) = 25 facets in 3 tiers.
/// Girdle: 16 facets in 1 tier. Crown: Star 8 + Crown Main 8 + Upper Girdle 16 = 32 facets in
/// 3 tiers, plus the table (1 facet, 1 tier). Totals: 3 + 1 + 3 + 1 = 8 tiers and
/// 25 + 16 + 32 + 1 = 74 facets.
#[test]
fn facet_data_of_the_standard_round_brilliant_is_counted_by_hand() {
    let design = round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let layout = SheetLayout::build(&design, &solved, &[], &SheetDetails::default());
    let facets = layout.facets;
    assert_eq!((facets.pavilion.tiers, facets.pavilion.facets), (3, 25));
    assert_eq!((facets.girdle.tiers, facets.girdle.facets), (1, 16));
    assert_eq!((facets.crown.tiers, facets.crown.facets), (3, 32));
    assert_eq!((facets.table.tiers, facets.table.facets), (1, 1));
    assert_eq!(facets.total_tiers(), 8);
    assert_eq!(facets.total_facets(), 74);
}

/// The crown reads `N+1` when the design has a table: its facets plus the table.
#[test]
fn the_crown_reads_n_plus_one_when_there_is_a_table() {
    let design = round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let layout = SheetLayout::build(&design, &solved, &[], &SheetDetails::default());
    assert_eq!(layout.facets.crown_facets_text(), "32+1");
    assert_eq!(layout.facets.crown_tiers_text(), "3+1");

    let html = cutting_sheet_html(&design, &solved, None, &[]);
    assert!(
        html.contains("<tr><td class=\"label\">Crown</td><td>3+1</td><td>32+1</td></tr>"),
        "{html}"
    );
    assert!(
        html.contains("<tr><td class=\"label\">Pavilion</td><td>3</td><td>25</td></tr>"),
        "{html}"
    );
    assert!(
        html.contains("<tr><td class=\"label\">Girdle</td><td>1</td><td>16</td></tr>"),
        "{html}"
    );
    assert!(
        html.contains("<tr><td class=\"label\">Total</td><td>8</td><td>74</td></tr>"),
        "{html}"
    );
}

/// The concave fixture has no table, so the crown prints a plain count. Pavilion: flat -42
/// (4) + flat -38 (4) + the groove's 8 placements = 16 facets in 3 tiers. Girdle: 4 facets in
/// 1 tier. Crown: flat 32 (4) + flat 40 (4) + the dimple's 4 placements = 12 facets in 3
/// tiers. Totals: 7 tiers and 16 + 4 + 12 = 32 facets.
#[test]
fn facet_data_of_the_concave_fixture_counts_each_tool_by_its_placements() {
    let design = Design::concave_fixture();
    let solved = design.solve().expect("the fixture's flat tiers solve");
    let layout = SheetLayout::build(&design, &solved, &[], &SheetDetails::default());
    let facets = layout.facets;
    assert_eq!((facets.pavilion.tiers, facets.pavilion.facets), (3, 16));
    assert_eq!((facets.girdle.tiers, facets.girdle.facets), (1, 4));
    assert_eq!((facets.crown.tiers, facets.crown.facets), (3, 12));
    assert_eq!((facets.table.tiers, facets.table.facets), (0, 0));
    assert_eq!(facets.total_tiers(), 7);
    assert_eq!(facets.total_facets(), 32);
    assert_eq!(facets.crown_facets_text(), "12", "no table, no +1");
    assert_eq!(facets.crown_tiers_text(), "3");
}

// ---- Size Data -----------------------------------------------------------------------

/// The six ratios are the editor's own: length, crown and pavilion from
/// `Design::stone_proportions`, volume and height from `Design::measure`, and P/C is
/// `(P/W) / (C/W)`.
#[test]
fn size_data_matches_the_existing_proportion_functions() {
    let design = round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let sizes = SheetLayout::build(&design, &solved, &[], &SheetDetails::default()).sizes;
    let proportions = design
        .stone_proportions(&solved)
        .expect("the round brilliant closes");
    let metrics = design
        .measure()
        .expect("solves")
        .expect("the round brilliant measures");
    let width = metrics.width_axis;
    let close = |a: Option<f64>, b: f64| {
        let a = a.expect("measured");
        assert!((a - b).abs() < 1e-12, "{a} vs {b}");
    };
    close(sizes.length_to_width, proportions.length_to_width.unwrap());
    close(sizes.height_to_width, metrics.total_height / width);
    close(
        sizes.volume_to_width_cubed,
        metrics.volume / (width * width * width),
    );
    close(
        sizes.pavilion_to_width,
        proportions.pavilion_to_width_percent.unwrap() / 100.0,
    );
    close(
        sizes.crown_to_width,
        proportions.crown_to_width_percent.unwrap() / 100.0,
    );
    close(
        sizes.pavilion_to_crown,
        proportions.pavilion_depth.unwrap() / proportions.crown_height.unwrap(),
    );
    // A round stone: length over width is 1.000 (the cut-core proportions test pins 1e-6).
    assert_eq!(sizes.cells()[0], ("L/W", "1.000".to_string()));
}

/// The cells are in print order, three decimals, and the HTML prints them under their labels.
#[test]
fn size_data_prints_six_cells_with_three_decimals() {
    let design = round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let layout = SheetLayout::build(&design, &solved, &[], &SheetDetails::default());
    let cells = layout.sizes.cells();
    let labels: Vec<&str> = cells.iter().map(|(label, _)| *label).collect();
    assert_eq!(labels, ["L/W", "H/W", "V/W\u{b3}", "P/W", "C/W", "P/C"]);
    for (label, value) in &cells {
        let decimals = value
            .split('.')
            .nth(1)
            .unwrap_or_else(|| panic!("{label}: {value}"));
        assert_eq!(decimals.len(), 3, "{label} {value}");
    }
    let html = cutting_sheet_html(&design, &solved, None, &[]);
    assert!(html.contains("<th>V/W\u{b3}</th>"), "{html}");
    assert!(html.contains("<th>P/C</th>"), "{html}");
}

/// A design that does not measure shows a dash in every cell.
#[test]
fn size_data_is_dashes_when_the_design_does_not_measure() {
    let design = Design::new(
        PreformSpec::block(1.0, 1.0, 1.0),
        ScheduleMeta::default(),
        Vec::new(),
    );
    let solved = design.solve().expect("no tiers to solve");
    let layout = SheetLayout::build(&design, &solved, &[], &SheetDetails::default());
    assert!(layout.sizes.cells().iter().all(|(_, value)| value == "-"));
}

// ---- Design Data ---------------------------------------------------------------------

/// Without the optional fields: the design's own RI with its material, the symmetry and the
/// index gear -- no range, no size range, no shape.
#[test]
fn design_data_without_the_optional_fields() {
    let mut design = round_brilliant_design();
    design.material = MaterialSelection {
        name: Some("Sapphire".to_string()),
        specific_gravity_override: None,
        refractive_index_override: Some(1.76),
        body_color_override: None,
        body_color_bands_override: None,
        absorption_path_scale_override: None,
    };
    let solved = design.solve().expect("every tier is pinned");
    let layout = SheetLayout::build(&design, &solved, &[], &SheetDetails::default());
    let rows: Vec<(&str, &str)> = layout
        .design_data
        .iter()
        .map(|(label, value)| (label.as_str(), value.as_str()))
        .collect();
    assert_eq!(
        rows,
        [
            ("RI", "1.760 (Sapphire)"),
            ("Symmetry", "8-fold, mirror"),
            ("Index gear", "96 index"),
        ]
    );
    let html = cutting_sheet_html(&design, &solved, None, &[]);
    assert!(!html.contains("RI range") && !html.contains("Size range"));
    assert!(!html.contains(">Shape<"));
}

/// With every optional field: the RI range replaces the single RI, the size range follows, the
/// shape comes last, and a design without mirror symmetry reads "4-fold".
#[test]
fn design_data_with_the_optional_fields() {
    let mut design = round_brilliant_design();
    design.meta.symmetry_order = 4;
    design.meta.mirror = false;
    let solved = design.solve().expect("every tier is pinned");
    let details = SheetDetails {
        shape: "Round".to_string(),
        ri_range: Some((1.54, 1.76)),
        size_range_mm: Some((4.0, 12.5)),
        ..SheetDetails::default()
    };
    let layout = SheetLayout::build(&design, &solved, &[], &details);
    let rows: Vec<(&str, &str)> = layout
        .design_data
        .iter()
        .map(|(label, value)| (label.as_str(), value.as_str()))
        .collect();
    assert_eq!(
        rows,
        [
            ("RI range", "1.54 - 1.76"),
            ("Size range", "4 - 12.5 mm"),
            ("Symmetry", "4-fold"),
            ("Index gear", "96 index"),
            ("Shape", "Round"),
        ]
    );
    let html = cutting_sheet_html_with(&design, &solved, None, &[], &details);
    assert!(
        html.contains("<td class=\"label\">RI range</td><td>1.54 - 1.76</td>"),
        "{html}"
    );
    assert!(
        html.contains("<td class=\"label\">Size range</td><td>4 - 12.5 mm</td>"),
        "{html}"
    );
    assert!(
        html.contains("<td class=\"label\">Shape</td><td>Round</td>"),
        "{html}"
    );
}

/// The girdle diameter and the carat estimate follow Design Data.
#[test]
fn the_girdle_diameter_and_carat_lines_follow_design_data() {
    let mut design = round_brilliant_design();
    design.girdle_diameter_mm = Some(6.5);
    design.material.name = Some("Diamond".to_string());
    let solved = design.solve().expect("every tier is pinned");
    let html = cutting_sheet_html(&design, &solved, None, &[]);
    let [design_data, girdle, carat] =
        positions_in_order(&html, &["Design Data", "Girdle diameter", "Carat weight"])[..]
    else {
        panic!("three positions");
    };
    assert!(design_data < girdle && girdle < carat);
    assert!(html.contains("<td>6.500 mm</td>"), "{html}");
}

// ---- Header, comments, order ---------------------------------------------------------

/// Title as the heading, then subtitle, "by" author and date, each only when present.
#[test]
fn the_header_prints_title_subtitle_author_and_date_in_order() {
    let design = round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let details = SheetDetails {
        title: "Starfire & Co".to_string(),
        subtitle: "Suite No. 3".to_string(),
        author: "A. Cutter".to_string(),
        date_text: "October 2026".to_string(),
        ..SheetDetails::default()
    };
    let html = cutting_sheet_html_with(&design, &solved, None, &[], &details);
    positions_in_order(
        &html,
        &[
            "<h1>Starfire &amp; Co</h1>",
            "<p class=\"subtitle\">Suite No. 3</p>",
            "<p class=\"byline\">by A. Cutter</p>",
            "<p class=\"date\">October 2026</p>",
            "<h2>Facet Data</h2>",
        ],
    );
    assert!(html.contains("<title>Starfire &amp; Co</title>"));
}

/// No title: the heading is "Cutting instructions"; empty subtitle, author and date print no
/// line at all.
#[test]
fn an_empty_header_keeps_the_default_heading_and_prints_no_empty_lines() {
    let design = round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let html = cutting_sheet_html_with(&design, &solved, None, &[], &SheetDetails::default());
    assert!(html.contains("<h1>Cutting instructions</h1>"));
    assert!(html.contains("<title>Cutting instructions</title>"));
    for class in ["subtitle", "byline", "date"] {
        assert!(!html.contains(&format!("class=\"{class}\"")), "{class}");
    }
    assert!(!html.contains("<h2>Comments</h2>"), "no comments, no block");
}

/// The blocks follow the template: header, Facet Data, Size Data, Design Data, Views,
/// Comments, Pavilion, Crown -- in the HTML and, without the views, in the text sheet.
#[test]
fn the_blocks_follow_the_template_order() {
    let design = round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let details = SheetDetails {
        title: "Order".to_string(),
        comments: vec!["Polish on a tin lap.".to_string()],
        ..SheetDetails::default()
    };
    let html = cutting_sheet_html_with(&design, &solved, None, &[], &details);
    positions_in_order(
        &html,
        &[
            "<h1>Order</h1>",
            "<h2>Facet Data</h2>",
            "<h2>Size Data</h2>",
            "<h2>Design Data</h2>",
            "<h2>Views</h2>",
            "<h2>Comments</h2>",
            "<h2>Pavilion</h2>",
            "<h2>Crown</h2>",
        ],
    );

    let text = cutting_sheet_text_with(&design, &solved, &[], &details);
    positions_in_order(
        &text,
        &[
            "Order\n",
            "\nFacet Data\n",
            "\nSize Data\n",
            "\nDesign Data\n",
            "\nComments\n",
            "\nPavilion\n",
            "\nCrown\n",
        ],
    );
}

/// Every comment line is printed, escaped, as a list item.
#[test]
fn every_comment_line_is_printed() {
    let design = round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let details = SheetDetails {
        comments: vec!["First <b>".to_string(), "Second".to_string()],
        ..SheetDetails::default()
    };
    let html = cutting_sheet_html_with(&design, &solved, None, &[], &details);
    assert!(html.contains("<li>First &lt;b&gt;</li>"), "{html}");
    assert!(html.contains("<li>Second</li>"), "{html}");
    let text = cutting_sheet_text_with(&design, &solved, &[], &details);
    assert!(text.contains("  First <b>\n  Second\n"), "{text}");
}

// ---- Sections ------------------------------------------------------------------------

/// The girdle rows are in the Pavilion section, the table is the last row of the Crown
/// section, and the step numbers run on across both.
#[test]
fn the_sections_split_by_block_with_the_girdle_in_the_pavilion_and_the_table_last() {
    let design = round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let layout = SheetLayout::build(&design, &solved, &[], &SheetDetails::default());
    let pavilion: Vec<&str> = layout
        .pavilion_rows()
        .iter()
        .map(|(row, _)| row.label())
        .collect();
    let crown: Vec<&str> = layout
        .crown_rows()
        .iter()
        .map(|(row, _)| row.label())
        .collect();
    assert_eq!(pavilion, ["G1", "P1", "P2", "Culet"]);
    assert_eq!(crown, ["C1", "C2", "C3", "T"]);
    let steps: Vec<usize> = layout
        .pavilion_rows()
        .iter()
        .chain(&layout.crown_rows())
        .map(|(row, _)| row.sequence)
        .collect();
    assert_eq!(steps, [1, 2, 3, 4, 5, 6, 7, 8]);

    let text = layout.to_text();
    let girdle_at = text.find(" G1 ").expect("G1 row in the text sheet");
    let crown_at = text.find("\nCrown\n").expect("Crown heading");
    assert!(
        girdle_at < crown_at,
        "G1 is printed before the Crown heading"
    );
}

/// The section tables lead with the template's columns -- label, angle, indices, instruction --
/// after the step number, and carry mast after them; the Block column is gone.
#[test]
fn the_section_tables_lead_with_the_template_columns() {
    let design = round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let html = cutting_sheet_html(&design, &solved, None, &[]);
    assert!(
        html.contains(
            "<th>#</th><th>Label</th><th>Angle</th><th>Indices</th><th>Instruction</th><th>Mast</th></tr>"
        ),
        "{html}"
    );
    assert!(!html.contains("<th>Block</th>"));
    assert_eq!(html.matches("<th>#</th>").count(), 2, "one per section");
}

/// The print rules keep the page size and keep sections and views whole.
#[test]
fn the_print_rules_keep_sections_and_views_together() {
    let design = round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let html = cutting_sheet_html(&design, &solved, None, &[]);
    assert!(html.contains("@page { size: A4; margin: 14mm; }"));
    assert!(
        html.contains("section.cut-section { break-inside: avoid; page-break-inside: avoid; }")
    );
    assert!(html.contains(".views figure { margin: 0; text-align: center; break-inside: avoid;"));
}

// ---- Views ---------------------------------------------------------------------------

/// A closed design prints four views, each with its label, in a 2 x 2 grid.
#[test]
fn four_views_are_printed_with_their_labels() {
    let design = round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let views: CutViews = render_cut_views(&design, &solved, 120, 100).expect("the stone closes");
    for image in [&views.top, &views.side, &views.end, &views.bottom] {
        assert_eq!((image.width, image.height), (120, 100));
        assert_eq!(&image.png_bytes[..8], &[137, 80, 78, 71, 13, 10, 26, 10]);
    }
    let html = cutting_sheet_html_with(
        &design,
        &solved,
        Some(&views),
        &[],
        &SheetDetails::default(),
    );
    assert_eq!(html.matches("<figure>").count(), 4);
    assert_eq!(html.matches("data:image/png;base64,").count(), 4);
    positions_in_order(
        &html,
        &[
            "<figcaption>down +Z</figcaption>",
            "<figcaption>down +X</figcaption>",
            "<figcaption>down -Y</figcaption>",
            "<figcaption>down -Z</figcaption>",
        ],
    );
    assert!(!html.contains("<p class=\"diagram-missing\">"));
}

/// A design that does not close prints a notice instead of the views.
#[test]
fn the_views_give_way_to_a_notice_for_a_design_that_does_not_close() {
    let design = round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let html = cutting_sheet_html_with(&design, &solved, None, &[], &SheetDetails::default());
    assert!(html.contains("<p class=\"diagram-missing\">"));
    assert_eq!(html.matches("<figure>").count(), 0);
    assert!(!html.contains("data:image/png;base64,"));
}

/// The document builder embeds the four views of a closed design.
#[test]
fn the_document_embeds_the_four_views() {
    let design = round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let html = cutting_sheet_document(&design, &solved, &[]);
    assert_eq!(html.matches("<figure>").count(), 4);
}

// ---- SheetDetails --------------------------------------------------------------------

/// `cutting_sheet_document` is `cutting_sheet_document_with` the design's own details, so the
/// web gets the new layout with the fallback header.
#[test]
fn the_plain_document_equals_the_document_with_the_fallback_details() {
    let mut design = round_brilliant_design();
    design.meta.headers = vec!["Fallback Stone".to_string(), "by Someone".to_string()];
    let solved = design.solve().expect("every tier is pinned");
    assert_eq!(
        cutting_sheet_document(&design, &solved, &[]),
        cutting_sheet_document_with(&design, &solved, &[], &SheetDetails::from_design(&design))
    );
}

/// The fallback reads the `.asc` header and footnote lines and invents nothing.
#[test]
fn from_design_reads_the_asc_header_and_footnotes() {
    let mut design = round_brilliant_design();
    design.meta.headers = vec![
        "  ".to_string(),
        "Star Burst".to_string(),
        "BY Jane Doe".to_string(),
        "A third line".to_string(),
    ];
    design.meta.footnotes = vec![
        String::new(),
        "  Cut dry. ".to_string(),
        "Mind the table.".to_string(),
    ];
    let details = SheetDetails::from_design(&design);
    assert_eq!(details.title, "Star Burst");
    assert_eq!(details.author, "Jane Doe");
    assert_eq!(details.comments, ["Cut dry.", "Mind the table."]);
    assert_eq!(details.subtitle, "");
    assert_eq!(details.date_text, "");
    assert_eq!(details.shape, "");
    assert_eq!(details.ri_range, None);
    assert_eq!(details.size_range_mm, None);
}

/// A design with no header lines gets an entirely empty fallback; a header that is only a
/// "by" line gives an author and no title.
#[test]
fn from_design_invents_nothing() {
    let design = round_brilliant_design();
    assert_eq!(SheetDetails::from_design(&design), SheetDetails::default());

    let mut by_only = round_brilliant_design();
    by_only.meta.headers = vec!["by Jane".to_string()];
    let details = SheetDetails::from_design(&by_only);
    assert_eq!(details.title, "");
    assert_eq!(details.author, "Jane");
}

/// The native file's title, designer and shape replace the fallback when not empty, its notes
/// are appended to the comments one line each, and the date is the caller's.
#[test]
fn from_metadata_layers_the_file_details_over_the_fallback() {
    let mut design = round_brilliant_design();
    design.meta.headers = vec!["Header Title".to_string(), "by Header Author".to_string()];
    design.meta.footnotes = vec!["From the footnote".to_string()];
    let metadata = DesignMetadata {
        title: " File Title ".to_string(),
        designer: "Capps, Jerry".to_string(),
        shape: "Round".to_string(),
        notes: "First note\n\n  Second note  \n".to_string(),
        ..DesignMetadata::default()
    };
    let details = SheetDetails::from_metadata(&design, &metadata, "October 2026");
    assert_eq!(details.title, "File Title");
    assert_eq!(details.author, "Capps, Jerry");
    assert_eq!(details.shape, "Round");
    assert_eq!(
        details.comments,
        ["From the footnote", "First note", "Second note"]
    );
    assert_eq!(details.date_text, "October 2026");

    // Empty file fields fall back to the header lines.
    let bare = SheetDetails::from_metadata(&design, &DesignMetadata::default(), "");
    assert_eq!(bare.title, "Header Title");
    assert_eq!(bare.author, "Header Author");
    assert_eq!(bare.shape, "");
    assert_eq!(bare.comments, ["From the footnote"]);
    assert_eq!(bare.date_text, "");
}

/// The date text is built from numbers the caller reads off its own clock.
#[test]
fn month_year_text_is_english() {
    assert_eq!(month_year_text(2026, 10), "October 2026");
    assert_eq!(month_year_text(1999, 1), "January 1999");
    assert_eq!(month_year_text(2026, 0), "");
    assert_eq!(month_year_text(2026, 13), "");
}

// ---- Text sheet ----------------------------------------------------------------------

/// The text sheet carries the same counts and rows as the HTML.
#[test]
fn the_text_sheet_mirrors_the_html_blocks() {
    let design = round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let details = SheetDetails {
        title: "Plain".to_string(),
        author: "A. Cutter".to_string(),
        date_text: "October 2026".to_string(),
        ..SheetDetails::default()
    };
    let text = cutting_sheet_text_with(&design, &solved, &[], &details);
    let mut lines = text.lines();
    assert_eq!(lines.next(), Some("Plain"));
    assert_eq!(lines.next(), Some("by A. Cutter"));
    assert_eq!(lines.next(), Some("October 2026"));
    assert!(text.contains("Crown     tiers 3+1   facets 32+1"), "{text}");
    assert!(text.contains("Total     tiers 8     facets 74"), "{text}");
    assert!(text.contains("L/W 1.000"), "{text}");
    assert!(text.contains("  RI: 1.540"), "{text}");
    assert!(text.contains("  Symmetry: 8-fold, mirror\n"), "{text}");
    assert!(text.contains("  Index gear: 96 index\n"), "{text}");
    // One row line per tier, in the two sections.
    assert_eq!(text.matches(" angle ").count(), design.tiers.len());
    // The rows are the labels of the HTML, in the same order.
    let html = cutting_sheet_html_with(&design, &solved, None, &[], &details);
    let mut from = 0;
    for label in label_cells(&html) {
        let at = text[from..]
            .find(&format!(". {label} "))
            .unwrap_or_else(|| panic!("{label} out of order in the text sheet"));
        from += at + 1;
    }
    assert_eq!(section_html(&html, "Pavilion").matches("<tr>").count(), 5);
}
