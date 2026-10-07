//! Tests for [`super::diagram`]/[`super::html`]: pure-function coverage with a
//! plain `Design` fixture and no window, per this group's own module doc comment.

use super::{
    diagram::render_cut_diagram,
    html::{base64_encode, cutting_sheet_html, html_escape},
    layout::material_label,
};
use indicatrix::{
    geometry::meet_solver::{Block, classify_blocks},
    optics::materials::GemMaterial,
};
use indicatrix_cut_core::{
    ConstraintTier, Design, MaterialSelection, PreformSpec, ScheduleMeta, design::TierRef,
};

/// The label cell (the second cell) of every row of the sheet's tier tables (the Pavilion
/// section, then the Crown section), in printed order. A concave tier's tool line is a
/// `<tr class="tool">` and is not a row here.
pub(super) fn label_cells(html: &str) -> Vec<String> {
    html.split("<table class=\"tiers\">")
        .skip(1)
        .flat_map(|table| {
            table.split("<tr>").skip(1).filter_map(|row| {
                let mut cells = row.split("<td").skip(1);
                let _sequence = cells.next()?;
                let label = cells.next()?;
                let text = label.strip_prefix('>')?.split("</td>").next()?;
                Some(text.to_string())
            })
        })
        .collect()
}

/// The HTML of the section headed `heading` (`Pavilion` or `Crown`): from its `<section>` tag
/// to the closing `</section>`.
pub(super) fn section_html<'a>(html: &'a str, heading: &str) -> &'a str {
    let opener = format!("<section class=\"cut-section\">\n<h2>{heading}</h2>");
    let start = html
        .find(&opener)
        .unwrap_or_else(|| panic!("no {heading} section in the sheet"));
    let rest = &html[start..];
    &rest[..rest.find("</section>").expect("the section closes")]
}

/// The same round-brilliant fixture `indicatrix_cut_core::cutting_sheet`'s
/// own tests use: 8 tiers, all `ScaleReference`, always solves and closes --
/// see that crate's `design/tier.rs::standard_round_brilliant_template_solves_and_closes`.
pub(super) fn round_brilliant_design() -> Design {
    Design::new(
        PreformSpec::block(2.0, 1.0, 2.0),
        ScheduleMeta::standard_round_brilliant(),
        ConstraintTier::standard_round_brilliant(),
    )
}

fn custom_garnet(n_d: f32) -> GemMaterial {
    let mut gem = GemMaterial::diamond();
    gem.name = "My Garnet".to_string();
    gem.dispersion = indicatrix::optics::dispersion::DispersionModel::Cauchy {
        a: n_d,
        b: 0.0,
        c: 0.0,
    };
    gem
}

/// A design on a CUSTOM catalogue material must print that material's own
/// `n_D` in the printed sheet's "Refractive index" row when `custom` is
/// supplied -- the bug this module fixes. `&[]` (no catalogue) must still fall
/// back to the legacy schedule RI, matching the built-ins-only behaviour every
/// other test in this module already relies on.
#[test]
fn cutting_sheet_html_prints_a_custom_materials_own_refractive_index() {
    let mut design = round_brilliant_design();
    design.material = MaterialSelection {
        name: Some("My Garnet".to_string()),
        specific_gravity_override: None,
        refractive_index_override: None,
        body_color_override: None,
        body_color_bands_override: None,
        absorption_path_scale_override: None,
    };
    let solved = design.solve().expect("every tier is pinned");
    let custom = [custom_garnet(1.9)];

    let with_catalogue = cutting_sheet_html(&design, &solved, None, &custom);
    assert!(
        with_catalogue.contains("<td>1.900 (My Garnet)</td>"),
        "{with_catalogue}"
    );

    let without_catalogue = cutting_sheet_html(&design, &solved, None, &[]);
    assert!(
        without_catalogue.contains(&format!(
            "<td>{:.3} (My Garnet)</td>",
            design.meta.refractive_index
        )),
        "{without_catalogue}"
    );
}

/// An unnamed design (every untouched import) whose effective RI sits at
/// sapphire's own must print the guess "Sapphire?", never the bare name passed
/// off as fact.
#[test]
fn material_label_shows_a_guess_for_an_unnamed_design_near_a_builtin() {
    let mut design = round_brilliant_design();
    design.meta.refractive_index = 1.76;
    design.material = MaterialSelection::none();
    let n_d = design.effective_refractive_index_with(&[]);
    assert_eq!(material_label(&design, n_d).as_deref(), Some("Sapphire?"));
}

/// Once a material name IS set, the line prints it plainly -- no
/// guess wording once something is a stated fact.
#[test]
fn material_label_prints_the_name_plainly_once_set() {
    let mut design = round_brilliant_design();
    design.material = MaterialSelection {
        name: Some("Sapphire".to_string()),
        specific_gravity_override: None,
        refractive_index_override: None,
        body_color_override: None,
        body_color_bands_override: None,
        absorption_path_scale_override: None,
    };
    let n_d = design.effective_refractive_index_with(&[]);
    assert_eq!(material_label(&design, n_d).as_deref(), Some("Sapphire"));
}

/// An unnamed design whose index is close to no built-in prints no material at all: the line
/// is the bare index, not an invented species.
#[test]
fn material_label_is_absent_when_nothing_is_close() {
    let mut design = round_brilliant_design();
    design.meta.refractive_index = 1.90;
    design.material = MaterialSelection::none();
    let n_d = design.effective_refractive_index_with(&[]);
    assert_eq!(material_label(&design, n_d), None);
}

/// The generated sheet must carry one row per tier, in cutting order, with
/// the step number, tier code, tier name (in the instruction text) and formatted mast all
/// present -- a basic "does the known schedule show up" check before the more targeted
/// tests below.
#[test]
fn cutting_sheet_html_contains_expected_rows() {
    let design = round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let html = cutting_sheet_html(&design, &solved, None, &[]);

    // No title on the fixture: the heading is "Cutting instructions".
    assert!(html.contains("<h1>Cutting instructions</h1>"));
    assert!(html.contains("<td>T</td>"), "the table's code is T");
    assert!(html.contains("Table: Set to mast depth 0.3200"));
    assert!(html.contains("Crown Main: Set to mast depth 0.5900"));
    assert!(html.contains("Pavilion Main: Set to mast depth 0.6700"));
    assert!(html.contains("<td>Culet</td>"));
    // One row per tier: count `<tr>` tags inside the `<tbody>...</tbody>` blocks only
    // (the Pavilion and the Crown section), so the data tables' own rows and the section
    // tables' `<thead>` rows (which also open with `<tr>`) are never counted.
    let tr_count: usize = html
        .split("<tbody>")
        .skip(1)
        .map(|body| {
            body.split("</tbody>")
                .next()
                .map_or(0, |rows| rows.matches("<tr>").count())
        })
        .sum();
    assert_eq!(tr_count, design.tiers.len());
}

/// Every row must print a positive angle (the unsigned magnitude of its angle)
/// in accordance with international faceting standards.
#[test]
fn angle_column_prints_the_positive_angle_magnitude() {
    let design = round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let html = cutting_sheet_html(&design, &solved, None, &[]);
    assert!(html.contains("<th>Angle</th>"), "{html}");
    assert!(!html.contains("<th>Elevation</th>"), "{html}");
    // "Pavilion Main" is authored at -41.0 degrees; its angle cell must
    // print the positive magnitude (41.00°), not a negative angle.
    assert!(html.contains("<td>41.00&deg;</td>"), "{html}");
    assert!(!html.contains("<td>-41.00&deg;</td>"), "{html}");
}

/// A carat-weight row must appear only once a girdle diameter anchors a real
/// mm scale -- matching the mm column's own gating.
#[test]
fn carat_weight_row_appears_only_when_girdle_diameter_is_set() {
    let design_no_mm = round_brilliant_design();
    let solved = design_no_mm.solve().expect("every tier is pinned");
    let html_no_mm = cutting_sheet_html(&design_no_mm, &solved, None, &[]);
    assert!(!html_no_mm.contains("Carat weight"));

    let mut design_with_mm = round_brilliant_design();
    design_with_mm.girdle_diameter_mm = Some(6.5);
    // A carat weight needs a specific gravity too, so a material must be
    // named -- the same fixture cut-core's own header test uses.
    design_with_mm.material.name = Some("Diamond".to_string());
    let html_with_mm = cutting_sheet_html(&design_with_mm, &solved, None, &[]);
    assert!(
        html_with_mm.contains("Carat weight (estimate)"),
        "{html_with_mm}"
    );
}

/// The "Mast (mm)" column -- and any mm-formatted cell -- must appear only
/// when `girdle_diameter_mm` is set, never derived from whether the mm
/// conversion happens to resolve.
#[test]
fn mm_column_appears_only_when_girdle_diameter_is_set() {
    let design_no_mm = round_brilliant_design();
    let solved = design_no_mm.solve().expect("every tier is pinned");
    let html_no_mm = cutting_sheet_html(&design_no_mm, &solved, None, &[]);
    assert!(!html_no_mm.contains("Mast (mm)"));

    let mut design_with_mm = round_brilliant_design();
    design_with_mm.girdle_diameter_mm = Some(6.5);
    let solved_mm = design_with_mm.solve().expect("every tier is pinned");
    let html_with_mm = cutting_sheet_html(&design_with_mm, &solved_mm, None, &[]);
    assert!(html_with_mm.contains("Mast (mm)"));
    assert!(html_with_mm.contains("Girdle diameter"));
}

/// The section a row is in must come from the solver's own `classify_blocks`, not the sign
/// of the printed angle -- the mistake this codebase already made once.
/// `standard_round_brilliant`'s own tiers give a known split: Table/Star/Crown Main/Upper
/// Girdle are Crown, the 90-degree `Girdle` tier is Girdle (printed in the Pavilion
/// section), and Pavilion Main/Lower Girdle/Culet (the last at a sign-negative zero angle,
/// which is the pavilion side) are Pavilion.
#[test]
fn sections_match_classify_blocks_not_angle_sign() {
    let design = round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let blocks = classify_blocks(&design.meet_tier_inputs());
    assert_eq!(
        blocks,
        vec![
            Block::Crown,
            Block::Crown,
            Block::Crown,
            Block::Crown,
            Block::Girdle,
            Block::Pavilion,
            Block::Pavilion,
            Block::Pavilion,
        ]
    );

    let html = cutting_sheet_html(&design, &solved, None, &[]);
    // "Culet" sits at angle `-0.0` -- a naive sign-of-the-formatted-string
    // check would call that Crown (`-0.0` formats with no visible sign in
    // some locales, or as positive zero); `classify_blocks` reads the sign of
    // the zero and puts it on the pavilion, and that is the section it must print in.
    let pavilion = section_html(&html, "Pavilion");
    let crown = section_html(&html, "Crown");
    assert!(
        pavilion.contains("<td>Culet</td>"),
        "Culet belongs to the Pavilion section: {pavilion}"
    );
    assert!(!crown.contains("<td>Culet</td>"), "{crown}");
    assert!(
        pavilion.contains("<td>G1</td>"),
        "the girdle is part of the Pavilion section: {pavilion}"
    );
    assert!(!crown.contains("<td>G1</td>"), "{crown}");
}

/// `render_cut_diagram` must produce a non-empty PNG for a design that
/// closes, sized exactly as requested.
#[test]
fn render_cut_diagram_produces_a_sized_png_for_a_closed_design() {
    let design = round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let image = render_cut_diagram(&design, &solved, 200, 100)
        .expect("standard round brilliant closes to a solid");
    assert_eq!(image.width, 200);
    assert_eq!(image.height, 100);
    assert_ne!(image.png_bytes, [] as [u8; 0]);
    // PNG magic bytes.
    assert_eq!(&image.png_bytes[..8], &[137, 80, 78, 71, 13, 10, 26, 10]);
}

/// The embedded `data:` URI must actually appear in the sheet when a
/// diagram is supplied, and the "unavailable" notice must appear instead
/// when it is not.
#[test]
fn diagram_embedding_switches_on_the_option() {
    // The ELEMENT, not the bare class name: `.diagram-missing` is also a rule in
    // the sheet's own stylesheet, so a substring check on the class alone
    // matches even when the diagram is present.
    const NOTICE: &str = "<p class=\"diagram-missing\">";

    let design = round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let image = render_cut_diagram(&design, &solved, 64, 32).expect("closes to a solid");

    let with_image = cutting_sheet_html(&design, &solved, Some(&image), &[]);
    assert!(with_image.contains("data:image/png;base64,"));
    assert!(!with_image.contains(NOTICE));

    let without_image = cutting_sheet_html(&design, &solved, None, &[]);
    assert!(without_image.contains(NOTICE));
    assert!(!without_image.contains("data:image/png;base64,"));
}

/// The hand-rolled base64 encoder must match RFC 4648 on values covering
/// both padding cases (`chunks(3)` remainders of 1 and 2 bytes) and the
/// empty input.
#[test]
fn base64_encode_matches_known_vectors() {
    assert_eq!(base64_encode(b""), "");
    assert_eq!(base64_encode(b"f"), "Zg==");
    assert_eq!(base64_encode(b"fo"), "Zm8=");
    assert_eq!(base64_encode(b"foo"), "Zm9v");
    assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
}

/// HTML-unsafe characters in a tier name must be escaped, not interpolated
/// raw -- a `.asc` file's own free-text header/notes can legally contain
/// `<`/`&`/quotes.
#[test]
fn html_escape_handles_unsafe_characters() {
    assert_eq!(
        html_escape("A & B <tag> \"quoted\" 'apos'"),
        "A &amp; B &lt;tag&gt; &quot;quoted&quot; &#39;apos&#39;"
    );
}

/// Each concave tier's facet line and tool line sit in ONE `<tbody>` (so the zebra band
/// and the page-break rule cover the pair), every tier gets its own `<tbody>`, and the
/// tool line carries the very strings `second_line_fields` gives.
#[test]
fn html_sheet_groups_each_concave_pair_in_one_tbody() {
    let design = Design::concave_fixture();
    let solved = design.solve().expect("the fixture's flat tiers solve");
    let html = cutting_sheet_html(&design, &solved, None, &[]);

    let bodies: Vec<&str> = html.split("<tbody").skip(1).collect();
    let tiers = design.tiers.len() + design.concave_tiers.len();
    assert_eq!(bodies.len(), tiers, "one tbody per tier, flat and concave");
    assert!(bodies[0].starts_with(" class=\"band-a\""));
    assert!(
        bodies[1].starts_with(" class=\"band-b\""),
        "stripes alternate per tier"
    );

    let with_tool: Vec<&&str> = bodies
        .iter()
        .filter(|b| b.contains("class=\"tool\""))
        .collect();
    assert_eq!(with_tool.len(), design.concave_tiers.len());
    for body in with_tool {
        // A chunk runs to the next `<tbody`, so the last tier of a section drags the next
        // section's `<thead><tr>` along: count the rows inside the body only.
        let inside = body.split("</tbody>").next().unwrap_or(body);
        assert_eq!(inside.matches("<tr").count(), 2, "facet line and tool line");
        assert_eq!(inside.matches("class=\"tool\"").count(), 1);
    }
    for tier in &design.concave_tiers {
        for field in tier.second_line_fields() {
            assert!(html.contains(&html_escape(&field)), "{field:?} missing");
        }
    }
    // The fixture sets a girdle diameter, so millimetres appear beside the ratios.
    assert!(html.contains("class=\"mm\""));
}

/// A planar sheet keeps one plain `<tbody>` per section (Pavilion, Crown) and gains no
/// banding rules.
#[test]
fn html_sheet_for_a_planar_design_has_no_banding() {
    let design = round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let html = cutting_sheet_html(&design, &solved, None, &[]);
    assert_eq!(html.matches("<tbody").count(), 2);
    assert!(!html.contains("band-a") && !html.contains("tr.tool"));
}

/// A planar design is no longer printed in its stored order: the fixture is stored top-down
/// (table first), the sheet lists the pavilion and girdle tiers first (the Pavilion
/// section), the crown next and the table last (the Crown section), each row labelled with
/// its code. The step numbers run on across the two sections.
#[test]
fn html_rows_of_a_planar_design_follow_the_cutting_order() {
    let design = round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let html = cutting_sheet_html(&design, &solved, None, &[]);

    assert_eq!(
        label_cells(&html),
        ["G1", "P1", "P2", "Culet", "C1", "C2", "C3", "T"]
    );
    assert_eq!(
        label_cells(section_html(&html, "Pavilion")),
        ["G1", "P1", "P2", "Culet"],
        "the girdle is cut with the pavilion"
    );
    let crown = section_html(&html, "Crown");
    assert_eq!(label_cells(crown), ["C1", "C2", "C3", "T"]);
    assert!(
        section_html(&html, "Pavilion").contains("<td>1</td><td>G1</td>"),
        "step 1 opens the Pavilion section"
    );
    assert!(
        crown.contains("<td>8</td><td>T</td>"),
        "step 8, the table, is a Crown row"
    );
    // The tier's own name opens the instruction text, the code owns the label column.
    assert!(html.contains("Pavilion Main: Set to mast depth 0.6700"));
    assert!(!html.contains("<td>Pavilion Main</td>"));
}

/// The stored order still decides the order inside a section: the fixture stored bottom-up
/// (culet first, table last) prints the pavilion and girdle tiers in that stored order, then
/// the crown tiers in theirs, the table last, and the codes are numbered along the rows.
#[test]
fn html_rows_keep_the_stored_order_inside_a_section() {
    let mut design = round_brilliant_design();
    design.tiers.reverse();
    let solved = design.solve().expect("every tier is pinned");
    let html = cutting_sheet_html(&design, &solved, None, &[]);
    assert_eq!(
        label_cells(&html),
        ["Culet", "P1", "P2", "G1", "C1", "C2", "C3", "T"]
    );
    assert_eq!(
        label_cells(section_html(&html, "Pavilion")),
        ["Culet", "P1", "P2", "G1"]
    );
    assert_eq!(
        label_cells(section_html(&html, "Crown")),
        ["C1", "C2", "C3", "T"]
    );
}

/// A concave design's rows are the cutting order too, labelled with `tier_codes()` (a concave
/// tier continues the P/C counts), and a concave row's block cell is the side its angle is on.
#[test]
fn html_rows_of_a_concave_design_follow_the_cutting_order_with_codes() {
    let design = Design::concave_fixture();
    let solved = design.solve().expect("the fixture's flat tiers solve");
    let html = cutting_sheet_html(&design, &solved, None, &[]);

    let codes = design.tier_codes();
    let order = design.cutting_order();
    let expected_codes: Vec<String> = order
        .iter()
        .map(|tier| match *tier {
            TierRef::Flat(i) => codes.flat[i].code.clone(),
            TierRef::Concave(i) => codes.concave[i].code.clone(),
        })
        .collect();
    assert_eq!(label_cells(&html), expected_codes);

    // A row is in the Crown section when its tier is on the crown side; a girdle (90 degrees)
    // and everything on the pavilion side is in the Pavilion section.
    let is_crown: Vec<bool> = order
        .iter()
        .map(|tier| match *tier {
            TierRef::Flat(i) => {
                let angle = design.tiers[i].angle_deg;
                angle.abs() < 90.0 && angle >= 0.0
            }
            TierRef::Concave(i) => design.concave_tiers[i].is_crown_side(),
        })
        .collect();
    let section_of = |wanted: bool| -> Vec<String> {
        expected_codes
            .iter()
            .zip(&is_crown)
            .filter(|&(_, &crown)| crown == wanted)
            .map(|(code, _)| code.clone())
            .collect()
    };
    assert_eq!(
        label_cells(section_html(&html, "Pavilion")),
        section_of(false)
    );
    assert_eq!(label_cells(section_html(&html, "Crown")), section_of(true));
}

/// A concave tier has no mast: its mast cells read `-` (as the text sheet's do), its tool
/// line keeps the code, theta and displacement under the facet line's columns, and the
/// details cell spans the rest (instruction, mast, mm: three cells here, since the fixture
/// sets a girdle diameter).
#[test]
fn html_concave_rows_print_a_dash_for_the_mast_and_keep_their_tool_line() {
    let design = Design::concave_fixture();
    let solved = design.solve().expect("the fixture's flat tiers solve");
    let html = cutting_sheet_html(&design, &solved, None, &[]);
    let groove_body = html
        .split("<tbody")
        .find(|body| body.contains("Groove:"))
        .expect("the groove's own body");
    assert!(
        groove_body.contains("<td>-</td><td>-</td></tr>"),
        "mast and mast (mm) of a concave facet line: {groove_body}"
    );
    assert!(
        groove_body.contains("<tr class=\"tool\"><td></td>"),
        "{groove_body}"
    );
    assert!(groove_body.contains("colspan=\"3\""), "{groove_body}");
}

/// The index column is the text sheet's: dash-separated, two digits per position, `-` for a
/// tier with no list.
#[test]
fn html_index_cells_are_dash_separated_and_zero_padded() {
    let design = round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let html = cutting_sheet_html(&design, &solved, None, &[]);
    assert!(html.contains("<td>00-12-24-36-48-60-72-84</td>"), "{html}");
    assert!(html.contains("<td>06-18-30-42-54-66-78-90</td>"), "{html}");
    assert!(
        html.contains("<td>95-01-11-13-23-25-35-37-47-49-59-61-71-73-83-85</td>"),
        "{html}"
    );
    assert!(html.contains("<td>-</td>"), "an empty list prints a dash");
    assert!(!html.contains("<td>0, 12"), "no comma lists any more");
}

/// A Meet instruction names its targets by code, and the name in front of it is the tier's own.
#[test]
fn html_meet_cells_use_codes_and_lead_with_the_tier_name() {
    let mut design = round_brilliant_design();
    // The sheet only needs one solved entry per tier, so the pinned fixture's own masts stand
    // in while the girdle (stored 4) is changed to meet the two main tiers by their names.
    let solved = design.solve().expect("every tier is pinned");
    design.tiers[4].constraint =
        indicatrix::geometry::meet_solver::MeetConstraint::MeetNamed(vec![
            "Pavilion Main".to_string(),
            "Crown Main".to_string(),
        ]);
    let html = cutting_sheet_html(&design, &solved, None, &[]);
    let codes = design.tier_codes();
    let pavilion = &codes.flat[5].code;
    let crown = &codes.flat[2].code;
    assert!(
        html.contains(&format!("Girdle: Meet {pavilion}, {crown}")),
        "{html}"
    );
}

/// The millimetre figures beside a concave tool line are the resolver's width-relative
/// ratios times the stone width, which `mm_per_unit` anchors to the girdle diameter. On
/// an elongated stone (width axis well below the length axis) they still equal
/// `ratio * mm_per_unit * width_axis`, i.e. `ratio * girdle_diameter_mm`.
#[test]
fn html_tool_row_millimetres_use_the_stone_width_on_an_elongated_stone() {
    let mut design = Design::concave_fixture();
    design.preform = PreformSpec::block(0.7, 1.8, 2.0);
    design.girdle_diameter_mm = Some(7.0);
    let solved = design.solve().expect("the elongated fixture solves");
    let metrics = design
        .measure_from_solved_geom(&solved)
        .expect("resolves")
        .expect("closed stone");
    assert!(
        metrics.length_axis > 1.2 * metrics.width_axis,
        "the stone must be elongated: {} x {}",
        metrics.width_axis,
        metrics.length_axis
    );
    let width_mm = design.yield_report(&solved).mm_per_unit.expect("scale") * metrics.width_axis;
    let html = cutting_sheet_html(&design, &solved, None, &[]);
    for tier in &design.concave_tiers {
        let expected = format!("(D = {:.2} mm)", tier.diameter_ratio * width_mm);
        assert!(html.contains(&expected), "{expected} missing");
    }
}
