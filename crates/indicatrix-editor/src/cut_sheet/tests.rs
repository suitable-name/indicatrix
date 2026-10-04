//! Tests for [`super::diagram`]/[`super::html`]: pure-function coverage with a
//! plain `Design` fixture and no window, per this group's own module doc comment.

use super::{
    diagram::render_cut_diagram,
    html::{base64_encode, cutting_sheet_html, html_escape, material_header_text},
};
use indicatrix::{
    geometry::meet_solver::{Block, classify_blocks},
    optics::materials::GemMaterial,
};
use indicatrix_cut_core::{ConstraintTier, Design, MaterialSelection, PreformSpec, ScheduleMeta};

/// The same round-brilliant fixture `indicatrix_cut_core::cutting_sheet`'s
/// own tests use: 8 tiers, all `ScaleReference`, always solves and closes --
/// see that crate's `design/tier.rs::standard_round_brilliant_template_solves_and_closes`.
fn round_brilliant_design() -> Design {
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
    };
    let solved = design.solve().expect("every tier is pinned");
    let custom = [custom_garnet(1.9)];

    let with_catalogue = cutting_sheet_html(&design, &solved, None, &custom);
    assert!(
        with_catalogue.contains("<td>1.9000</td>"),
        "{with_catalogue}"
    );

    let without_catalogue = cutting_sheet_html(&design, &solved, None, &[]);
    assert!(
        without_catalogue.contains(&format!("<td>{:.4}</td>", design.meta.refractive_index)),
        "{without_catalogue}"
    );
}

/// An unnamed design (every untouched import) whose effective RI sits at
/// sapphire's own must print the guess label "Sapphire? (from RI 1.76)",
/// never a bare number passed off as fact.
#[test]
fn material_header_text_shows_a_guess_for_an_unnamed_design_near_a_builtin() {
    let mut design = round_brilliant_design();
    design.meta.refractive_index = 1.76;
    design.material = MaterialSelection::none();
    let n_d = design.effective_refractive_index_with(&[]);
    assert_eq!(
        material_header_text(&design, n_d),
        "Sapphire? (from RI 1.76)"
    );
}

/// Once a material name IS set, the header must print it plainly -- no
/// guess wording once something is a stated fact.
#[test]
fn material_header_text_prints_the_name_plainly_once_set() {
    let mut design = round_brilliant_design();
    design.material = MaterialSelection {
        name: Some("Sapphire".to_string()),
        specific_gravity_override: None,
        refractive_index_override: None,
        body_color_override: None,
    };
    let n_d = design.effective_refractive_index_with(&[]);
    assert_eq!(material_header_text(&design, n_d), "Sapphire");
}

/// The generated sheet must carry one row per tier, in cutting order, with
/// the step number, tier name and formatted mast all present -- a basic
/// "does the known schedule show up" check before the more targeted tests
/// below.
#[test]
fn cutting_sheet_html_contains_expected_rows() {
    let design = round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let html = cutting_sheet_html(&design, &solved, None, &[]);

    assert!(html.contains("<h1>Cutting Sheet</h1>"));
    assert!(html.contains("Table"));
    assert!(html.contains("Crown Main"));
    assert!(html.contains("Pavilion Main"));
    assert!(html.contains("Culet"));
    // One row per tier: count `<tr>` tags inside `<tbody>...</tbody>` only,
    // so the meta table's own rows and the tier table's `<thead>` row
    // (which also opens with `<tr>`) are never counted.
    let tbody_start = html.find("<tbody>").expect("tbody present") + "<tbody>".len();
    let tbody_end = html.find("</tbody>").expect("tbody closes");
    let tr_count = html[tbody_start..tbody_end].matches("<tr>").count();
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

/// Crown/Pavilion/Girdle labels must come from the solver's own
/// `classify_blocks`, not the sign of the printed angle -- the mistake this
/// codebase already made once. `standard_round_brilliant`'s own tiers give
/// a known split: Table/Star/Crown Main/Upper Girdle are Crown, the 90-degree
/// `Girdle` tier is Girdle, and Pavilion Main/Lower Girdle/Culet (the last at
/// a sign-negative zero angle, which is the pavilion side) are
/// Pavilion.
#[test]
fn block_labels_match_classify_blocks_not_angle_sign() {
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
    // the zero and puts it on the pavilion, and that is what the printed label
    // must say.
    let culet_row_start = html.find("Culet").expect("Culet row present");
    let row_html = &html[culet_row_start..culet_row_start + 400.min(html.len() - culet_row_start)];
    assert!(
        row_html.contains("block-pavilion"),
        "Culet's row must be labelled Pavilion, got: {row_html}"
    );
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
        assert_eq!(body.matches("<tr").count(), 2, "facet line and tool line");
        assert_eq!(body.matches("class=\"tool\"").count(), 1);
    }
    for tier in &design.concave_tiers {
        for field in tier.second_line_fields() {
            assert!(html.contains(&html_escape(&field)), "{field:?} missing");
        }
    }
    // The fixture sets a girdle diameter, so millimetres appear beside the ratios.
    assert!(html.contains("class=\"mm\""));
}

/// A planar sheet keeps its single `<tbody>` and gains no banding rules.
#[test]
fn html_sheet_for_a_planar_design_has_no_banding() {
    let design = round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let html = cutting_sheet_html(&design, &solved, None, &[]);
    assert_eq!(html.matches("<tbody").count(), 1);
    assert!(!html.contains("band-a") && !html.contains("tr.tool"));
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
