use super::*;

fn kinds(chapter: &Chapter) -> Vec<BlockKind> {
    chapter.blocks.iter().map(|block| block.kind).collect()
}

#[test]
fn slugs_follow_the_github_rule() {
    assert_eq!(slug("What you will do"), "what-you-will-do");
    assert_eq!(slug("4. Editing Tiers"), "4-editing-tiers");
    assert_eq!(
        slug("Targets: cut to depth, girdle thickness, table width"),
        "targets-cut-to-depth-girdle-thickness-table-width"
    );
    assert_eq!(
        slug("The inspector's Schedule tab"),
        "the-inspectors-schedule-tab"
    );
    assert_eq!(
        slug("Visual before/after comparison"),
        "visual-beforeafter-comparison"
    );
    // Marks are removed first; an em dash leaves its two spaces behind.
    assert_eq!(
        slug("`.asc` \u{2014} the plain file"),
        "asc--the-plain-file"
    );
    assert_eq!(slug("**Bold** and [a link](x.md)"), "bold-and-a-link");
    assert_eq!(slug("snake_case Words"), "snake_case-words");
}

#[test]
fn plain_text_drops_the_marks_and_keeps_the_words() {
    assert_eq!(
        plain_text("**Anchor** \u{2014} a *tier* with `code` and [a link](a.md#b)"),
        "Anchor \u{2014} a tier with code and a link"
    );
    assert_eq!(plain_text("![the figure](x.png) after"), "the figure after");
    assert_eq!(plain_text("1 \\* 2 and `a*b`"), "1 * 2 and a*b");
    assert_eq!(
        plain_text("[not a link] and (plain)"),
        "[not a link] and (plain)"
    );
    assert_eq!(plain_text("~~gone~~"), "gone");
}

#[test]
fn a_chapter_splits_into_sections_by_heading() {
    let chapter = parse_chapter(
        "# Title\n\nIntro.\n\n## One\n\nText one.\n\n### Deeper\n\nDeep text.\n\n## Two\n\nText two.\n",
    );
    let titles: Vec<&str> = chapter.sections.iter().map(|s| s.title.as_str()).collect();
    assert_eq!(titles, ["Title", "One", "Deeper", "Two"]);
    assert_eq!(chapter.title, "Title");
    assert_eq!(chapter.sections[2].level, 3);
    assert_eq!(chapter.section_by_slug("deeper"), Some(2));
    assert_eq!(chapter.section_by_slug("missing"), None);
    // A section's body is its own text only.
    assert_eq!(chapter.section_plain_text(1), "Text one.");
    assert_eq!(chapter.section_plain_text(2), "Deep text.");
    // Every block knows its section; the ranges tile the block list.
    assert_eq!(chapter.blocks[0].section, 0);
    assert_eq!(chapter.blocks.last().map(|b| b.section), Some(3));
    assert_eq!(chapter.sections[0].range, 0..2);
    assert_eq!(chapter.sections[3].range.end, chapter.blocks.len());
}

#[test]
fn repeated_headings_get_numbered_slugs() {
    let chapter =
        parse_chapter("# A\n\n## Next steps\n\nx\n\n## Next steps\n\ny\n\n## Next steps\n");
    let slugs: Vec<&str> = chapter.sections.iter().map(|s| s.slug.as_str()).collect();
    assert_eq!(slugs, ["a", "next-steps", "next-steps-1", "next-steps-2"]);
}

#[test]
fn wrapped_lines_join_into_one_paragraph() {
    let chapter = parse_chapter("# T\n\nfirst line\nsecond line\n  third line\n\nnext paragraph\n");
    assert_eq!(chapter.blocks[1].text, "first line second line third line");
    assert_eq!(chapter.blocks[2].text, "next paragraph");
}

#[test]
fn nested_lists_keep_depth_markers_and_continuations() {
    let chapter = parse_chapter(
        "# T\n\n- **Meets** \u{2014} a drop-down\n  that wraps\n  - **Exact** \u{2014} a number\n  - **Named** \u{2014} names\n- **Name** \u{2014} the name\n\n  A second paragraph of the name.\n\n1. First\n2. Second\n",
    );
    let items: Vec<(&BlockKind, u8, &str, &str)> = chapter
        .blocks
        .iter()
        .skip(1)
        .map(|b| (&b.kind, b.indent, b.marker.as_str(), b.text.as_str()))
        .collect();
    assert_eq!(
        items[0],
        (
            &BlockKind::ListItem,
            0,
            "\u{2022}",
            "**Meets** \u{2014} a drop-down that wraps"
        )
    );
    assert_eq!(
        items[1],
        (
            &BlockKind::ListItem,
            1,
            "\u{2013}",
            "**Exact** \u{2014} a number"
        )
    );
    assert_eq!(items[2].1, 1);
    assert_eq!(
        items[3],
        (
            &BlockKind::ListItem,
            0,
            "\u{2022}",
            "**Name** \u{2014} the name"
        )
    );
    // The paragraph under an item sits one level in.
    assert_eq!(
        items[4],
        (
            &BlockKind::Paragraph,
            1,
            "",
            "A second paragraph of the name."
        )
    );
    assert_eq!(items[5], (&BlockKind::ListItem, 0, "1.", "First"));
    assert_eq!(items[6], (&BlockKind::ListItem, 0, "2.", "Second"));
}

#[test]
fn only_a_list_starting_at_one_interrupts_a_paragraph() {
    let chapter =
        parse_chapter("# T\n\nintro line\n1. First\n2. Second\n\ntext\n7. wrapped number\n");
    assert_eq!(chapter.blocks[1].kind, BlockKind::Paragraph);
    assert_eq!(chapter.blocks[1].text, "intro line");
    assert_eq!(chapter.blocks[2].kind, BlockKind::ListItem);
    assert_eq!(chapter.blocks[3].text, "Second");
    // "7." in the middle of a paragraph is just a wrapped number.
    assert_eq!(chapter.blocks[4].text, "text 7. wrapped number");
}

#[test]
fn tables_become_rows_with_column_weights() {
    let chapter = parse_chapter(
        "# T\n\n| Key | Action |\n| --- | --- |\n| Ctrl+S | Save the design to the file you chose, asking where first |\n| F5 | Solve |\n\nafter\n",
    );
    let rows: Vec<&Block> = chapter
        .blocks
        .iter()
        .filter(|b| b.kind == BlockKind::TableRow)
        .collect();
    assert_eq!(rows.len(), 3);
    assert!(rows[0].header && !rows[1].header);
    assert_eq!(rows[0].cells, ["Key", "Action"]);
    assert_eq!(rows[1].cells[0], "Ctrl+S");
    assert_eq!(rows[0].weights, rows[2].weights);
    assert!(rows[0].weights[1] > rows[0].weights[0]);
    assert_eq!(
        chapter.blocks.last().map(|b| b.kind),
        Some(BlockKind::Paragraph)
    );
}

#[test]
fn a_table_row_keeps_an_escaped_bar_in_its_cell() {
    let chapter = parse_chapter("# T\n\n| a \\| b | c |\n| --- | --- |\n");
    assert_eq!(chapter.blocks[1].cells, ["a \\| b", "c"]);
    assert_eq!(plain_text(&chapter.blocks[1].cells[0]), "a | b");
}

#[test]
fn short_rows_are_padded_to_the_header_width() {
    let chapter = parse_chapter("# T\n\n| a | b | c |\n|---|---|---|\n| 1 |\n");
    assert_eq!(chapter.blocks[2].cells, ["1", "", ""]);
}

#[test]
fn code_quotes_figures_rules_and_comments() {
    let chapter = parse_chapter(
        "# T\n\n```\nline one\n  indented\n```\n\n<!-- hidden\nstill hidden -->\n> quoted\n> text\n>\n> second\n\n![A diagram](d.png)\n\n---\n\n   ```text\n   in a list\n   ```\n",
    );
    let k = kinds(&chapter);
    assert_eq!(
        k,
        [
            BlockKind::Heading,
            BlockKind::Code,
            BlockKind::Quote,
            BlockKind::Quote,
            BlockKind::Figure,
            BlockKind::Rule,
            BlockKind::Code,
        ]
    );
    assert_eq!(chapter.blocks[1].text, "line one\n  indented");
    assert_eq!(chapter.blocks[2].text, "quoted text");
    assert_eq!(chapter.blocks[3].text, "second");
    assert_eq!(chapter.blocks[4].text, "A diagram");
    assert_eq!(chapter.blocks[4].marker, "d.png");
    assert_eq!(chapter.blocks[6].text, "in a list");
}

#[test]
fn text_before_the_first_heading_gets_an_implicit_section() {
    let chapter = parse_chapter("Just text.\n\n## Then a heading\n");
    assert_eq!(chapter.sections.len(), 2);
    assert_eq!(chapter.sections[0].title, "");
    assert_eq!(chapter.blocks[0].section, 0);
    assert_eq!(chapter.blocks[1].section, 1);
    assert_eq!(chapter.title, "Then a heading");
    assert_eq!(chapter.section_plain_text(0), "Just text.");
}

#[test]
fn an_empty_source_has_no_blocks() {
    let chapter = parse_chapter("");
    assert!(chapter.blocks.is_empty() && chapter.sections.is_empty());
    assert_eq!(chapter.section_plain_text(0), "");
}

#[test]
fn a_bold_start_is_not_a_list_and_stars_alone_are_a_rule() {
    let chapter = parse_chapter("# T\n\n**Anchor** \u{2014} a tier.\n\n* * *\n\n- real item\n");
    assert_eq!(chapter.blocks[1].kind, BlockKind::Paragraph);
    assert_eq!(chapter.blocks[2].kind, BlockKind::Rule);
    assert_eq!(chapter.blocks[3].kind, BlockKind::ListItem);
}

#[test]
fn crlf_sources_parse_like_lf_sources() {
    let lf = parse_chapter("# T\n\na\nb\n\n- x\n");
    let crlf = parse_chapter("# T\r\n\r\na\r\nb\r\n\r\n- x\r\n");
    assert_eq!(lf, crlf);
}
