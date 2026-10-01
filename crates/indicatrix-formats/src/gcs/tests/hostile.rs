//! Hostile `.gcs` input: non-finite numbers, an entity flood, and converted designs
//! whose header values `.asc` would reject, and facets or vertices missing a required
//! attribute.

use super::{GCS_OCTABAR_X_EXCERPT, GCS_SPEC_EXAMPLE};
use crate::{
    asc::{AscSchedule, parse_asc, to_asc_string},
    gcs::{GcsParseError, gcs_to_asc_schedule, parse_gcs},
};
use std::time::{Duration, Instant};

const TIER: &str = r#"angle="30" depth="0.5""#;
const FACET: &str = r#"nx="0" ny="-0.5" nz="0.8" index_angle="0""#;
const VERTEX: &str = r#"x="0.1" y="0.2" z="0.3""#;

/// A one-tier file whose tier, facet and vertex carry the given attribute text.
fn file_with(tier: &str, facet: &str, vertex: &str) -> String {
    format!(
        r#"<GemCutStudio version="1000"><index gear="96"/>
        <tier {tier}><facet {facet}><vertex {vertex}/></facet></tier></GemCutStudio>"#
    )
}

/// The `.asc` text of `schedule` read back through [`parse_asc`].
fn reparse(schedule: &AscSchedule) -> AscSchedule {
    let text = to_asc_string(schedule).expect("a converted schedule is writable");
    parse_asc(&text).unwrap_or_else(|e| panic!("the converted text must parse: {e}\n{text}"))
}

/// `nan` and the infinities parse as `f64` but would poison every angle, depth,
/// normal and vertex they reach: each is reported as a garbled number.
#[test]
fn non_finite_numbers_are_rejected_with_a_typed_error() {
    parse_gcs(&file_with(TIER, FACET, VERTEX)).expect("the baseline file parses");
    for bad in ["nan", "NaN", "inf", "-inf", "infinity"] {
        let tier = format!(r#"angle="{bad}" depth="0.5""#);
        assert!(
            matches!(
                parse_gcs(&file_with(&tier, FACET, VERTEX)),
                Err(GcsParseError::TierAttributeNotNumeric { attr: "angle", .. })
            ),
            "tier angle {bad}"
        );
        let tier = format!(r#"angle="30" depth="{bad}""#);
        assert!(
            matches!(
                parse_gcs(&file_with(&tier, FACET, VERTEX)),
                Err(GcsParseError::TierAttributeNotNumeric { attr: "depth", .. })
            ),
            "tier depth {bad}"
        );
        let facet = format!(r#"nx="{bad}" ny="-0.5" nz="0.8" index_angle="0""#);
        assert!(
            matches!(
                parse_gcs(&file_with(TIER, &facet, VERTEX)),
                Err(GcsParseError::FacetAttributeNotNumeric { attr: "nx", .. })
            ),
            "facet normal {bad}"
        );
        let facet = format!(r#"nx="0" ny="-0.5" nz="0.8" index_angle="{bad}""#);
        assert!(
            matches!(
                parse_gcs(&file_with(TIER, &facet, VERTEX)),
                Err(GcsParseError::FacetAttributeNotNumeric {
                    attr: "index_angle",
                    ..
                })
            ),
            "facet index angle {bad}"
        );
        let vertex = format!(r#"x="0.1" y="{bad}" z="0.3""#);
        assert!(
            matches!(
                parse_gcs(&file_with(TIER, FACET, &vertex)),
                Err(GcsParseError::VertexAttributeNotNumeric { attr: "y", .. })
            ),
            "vertex y {bad}"
        );
    }
}

#[test]
fn non_finite_index_and_render_attributes_are_rejected() {
    let index = r#"<GemCutStudio version="1000"><index gear="nan"/></GemCutStudio>"#;
    assert!(matches!(
        parse_gcs(index),
        Err(GcsParseError::IndexAttributeNotNumeric { attr: "gear", .. })
    ));
    let render = r#"<GemCutStudio version="1000"><index gear="96"/>
        <render refractive_index="inf"/></GemCutStudio>"#;
    assert!(matches!(
        parse_gcs(render),
        Err(GcsParseError::RenderAttributeNotNumeric {
            attr: "refractive_index",
            ..
        })
    ));
}

/// Looking for the `;` after every `&` used to scan to the end of the attribute, so
/// a run of ampersands cost quadratic time. The search is bounded now.
#[test]
fn a_hundred_thousand_ampersands_decode_in_linear_time() {
    let amps = "&".repeat(100_000);
    for tail in ["", ";"] {
        let content = format!(
            r#"<GemCutStudio version="1000"><index gear="96"/><info title="{amps}{tail}"/></GemCutStudio>"#
        );
        let start = Instant::now();
        let design = parse_gcs(&content).expect("an ampersand flood still parses");
        let elapsed = start.elapsed();
        assert!(elapsed < Duration::from_secs(1), "took {elapsed:?}");
        let title = design.info.and_then(|info| info.title).expect("the title");
        assert_eq!(title, format!("{amps}{tail}"));
    }
}

/// Entities still decode up to the longest real one (`&#x10FFFF;`), and a `;` beyond
/// the search window leaves the `&` verbatim.
#[test]
fn entities_decode_within_the_search_window_only() {
    let far = format!("&{};", "x".repeat(40));
    let content = format!(
        r#"<GemCutStudio version="1000"><index gear="96"/>
        <info title="&#x10FFFF;&amp;{far}"/></GemCutStudio>"#
    );
    let design = parse_gcs(&content).expect("must parse");
    let title = design.info.and_then(|info| info.title).expect("the title");
    assert_eq!(title, format!("\u{10FFFF}&{far}"));
}

#[test]
fn converted_fixtures_reparse_as_asc() {
    for (name, text) in [
        ("octabar excerpt", GCS_OCTABAR_X_EXCERPT),
        ("spec example", GCS_SPEC_EXAMPLE),
    ] {
        let schedule = gcs_to_asc_schedule(&parse_gcs(text).expect(name));
        let reparsed = reparse(&schedule);
        assert_eq!(reparsed.tiers.len(), schedule.tiers.len(), "{name}");
        assert_eq!(reparsed.gear_teeth, schedule.gear_teeth, "{name}");
        assert_eq!(reparsed.symmetry_order, schedule.symmetry_order, "{name}");
        assert_eq!(
            reparsed.refractive_index, schedule.refractive_index,
            "{name}"
        );
    }
}

/// A file with no `<render>`, a zero symmetry order, or a gear `.asc` cannot hold
/// converts with defaults and warnings, and its text still parses.
#[test]
fn a_missing_refractive_index_and_a_hostile_index_element_are_defaulted() {
    let tier =
        r#"<tier angle="40" depth="0.9" name="C1"><facet nx="0" ny="-0.64" nz="0.77"/></tier>"#;
    for (index, gear) in [
        (r#"<index gear="96" symmetry="0"/>"#, 96),
        (r#"<index gear="0"/>"#, 96),
        (r#"<index gear="721"/>"#, 96),
        (r#"<index gear="4000000000"/>"#, 96),
        (r#"<index gear="720"/>"#, 720),
    ] {
        let content = format!(r#"<GemCutStudio version="1000">{index}{tier}</GemCutStudio>"#);
        let schedule = gcs_to_asc_schedule(&parse_gcs(&content).expect("the file parses"));
        assert_eq!(schedule.gear_teeth, gear, "{index}");
        assert_eq!(schedule.refractive_index, 1.54, "{index}");
        assert!(schedule.symmetry_order >= 1, "{index}");
        assert!(
            schedule
                .warnings
                .iter()
                .any(|w| w.contains("refractive index")),
            "{index}: {:?}",
            schedule.warnings
        );
        let reparsed = reparse(&schedule);
        assert_eq!(reparsed.gear_teeth, gear, "{index}");
        assert_eq!(reparsed.tiers.len(), 1, "{index}");
    }
}

#[test]
fn a_refractive_index_not_above_one_is_defaulted_with_one_warning() {
    let tier =
        r#"<tier angle="40" depth="0.9" name="C1"><facet nx="0" ny="-0.64" nz="0.77"/></tier>"#;
    for render in [
        "",
        r#"<render refractive_index="0"/>"#,
        r#"<render refractive_index="0.5"/>"#,
        r#"<render refractive_index="1"/>"#,
    ] {
        let content = format!(
            r#"<GemCutStudio version="1000"><index gear="96"/>{tier}{render}</GemCutStudio>"#
        );
        let schedule = gcs_to_asc_schedule(&parse_gcs(&content).expect("the file parses"));
        assert_eq!(schedule.refractive_index, 1.54, "{render}");
        assert_eq!(
            schedule.warnings.len(),
            1,
            "{render}: {:?}",
            schedule.warnings
        );
        assert_eq!(reparse(&schedule).refractive_index, 1.54, "{render}");
    }
}

/// A `<facet>` element missing a required attribute must be rejected, not silently
/// defaulted to `0.0` -- the module doc comment's `# Errors` section promises this
/// for "a tier, facet, or vertex with a missing or non-numeric required attribute".
#[test]
fn rejects_a_facet_missing_a_required_attribute() {
    let content = r#"<GemCutStudio version="1000">
    <index gear="64" base="0" symmetry="4" mirror="0"/>
    <tier angle="90" depth="1.0" name="T" instructions="" visible="true" guide="false">
        <facet ny="0" nz="-1" index_angle="0">
            <vertex x="0" y="0" z="0"/>
        </facet>
    </tier>
</GemCutStudio>"#;
    let err = parse_gcs(content).expect_err("a facet missing 'nx' must be rejected");
    assert!(
        matches!(err, GcsParseError::FacetAttributeMissing { attr: "nx", .. }),
        "{err:?}"
    );
}

/// A `<vertex>` element missing a required attribute must likewise be rejected.
#[test]
fn rejects_a_vertex_missing_a_required_attribute() {
    let content = r#"<GemCutStudio version="1000">
    <index gear="64" base="0" symmetry="4" mirror="0"/>
    <tier angle="90" depth="1.0" name="T" instructions="" visible="true" guide="false">
        <facet nx="0" ny="0" nz="-1" index_angle="0">
            <vertex y="0" z="0"/>
        </facet>
    </tier>
</GemCutStudio>"#;
    let err = parse_gcs(content).expect_err("a vertex missing 'x' must be rejected");
    assert!(
        matches!(err, GcsParseError::VertexAttributeMissing { attr: "x", .. }),
        "{err:?}"
    );
}
