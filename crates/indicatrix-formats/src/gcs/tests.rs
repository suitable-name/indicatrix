//! Tests for [`super::parse_gcs`], against a real trimmed excerpt and a
//! handful of hand-built error-path fixtures.

use super::{GcsParseError, parse_gcs};

/// A real excerpt of `attached_files` id 1124 (detail 553), `"L1000-h0724-
/// f085-i064-b83-ri170-215-Octagon-pc21086F-FVS-044-Octabar-X-by-Van-Sant-
/// Fred-W.gcs"` -- "FVS-044 Octabar-X PC 21.086F" by Fred W. Van Sant. The
/// root, `<index>`, and `<render>`/`<info>` lines are copied verbatim; the
/// first `<tier>` is trimmed from its real 8 facets down to the first 2 (the
/// bytes of each kept facet, including every vertex, are untouched) purely to
/// keep this fixture short -- see `crates/indicatrix-formats/src/gcs/mod.rs`'s
/// module docs for the full-file cross-check results this trimmed excerpt
/// can't exercise on its own (tier-to-tier angle/index matching against the
/// sibling `.asc`).
const GCS_OCTABAR_X_EXCERPT: &str = r#"<GemCutStudio version="1000">
<index gear="64" base="0" symmetry="4" mirror="0"/>
<tier angle="126.38999938964842" depth="0.6664250328253426" name="P1" instructions="" visible="true" guide="false">
    <facet nx="-0" ny="-0.8049973625502862" nz="-0.59327838852184989" index_angle="0">
        <vertex x="-0.082702055286700479" y="-0.81860733222343418" z="-0.012554459365542562"/>
        <vertex x="-0.24803731741144758" y="-0.87723469910015384" z="0.066994832538740542"/>
        <vertex x="-0.41421356237309492" y="-0.99999999999999944" z="0.23357049979554395"/>
        <vertex x="0.41421356237309509" y="-1" z="0.23357049979554453"/>
        <vertex x="0.24803731773169496" y="-0.87723469921371255" z="0.066994832692824094"/>
        <vertex x="0.082702055286700812" y="-0.81860733222343418" z="-0.012554459365542562"/>
    </facet>
    <facet nx="-0.56921909389659309" ny="-0.56921909389659309" nz="-0.59327838852184989" index_angle="45">
        <vertex x="-0.99999999999999944" y="-0.41421356237309503" z="0.23357049979554412"/>
        <vertex x="-0.41421356237309492" y="-0.99999999999999944" z="0.23357049979554395"/>
        <vertex x="-0.57116930351496131" y="-0.5711693029784638" z="-0.027279076108571262"/>
    </facet>
</tier>
<render material="176 Corundum" refractive_index="1.76" dispersion="0.017999999" clarity="100" density="1.4" lighting_model="Random">
    <color r="0.70980394" g="0.73333335" b="0.94901967"/>
</render>
<info title="FVS-044 Octabar-X PC 21.086F" author="Van Sant, Fred W" date="Star Cuts 1 1998

" header2="This design released into the public domain

" header3="by Keith Wyman in memory of Charles L. Moon

" ri_min="1.7" ri_max="2.1500001" shape="Octagon" footer1="Entered into GCS, filename, shapename, cut sequence and meetpoints revised by Kevin Kane kane2002@telus.net

" footer2="L1000 h0724 f085 i064 b83 ri170-215 Octagon pc21086F FVS-044 Octabar-X by Van Sant, Fred W"/>
</GemCutStudio>
"#;

#[test]
fn parses_real_index_element() {
    let design = parse_gcs(GCS_OCTABAR_X_EXCERPT).expect("real excerpt must parse");
    assert_eq!(design.version, "1000");
    assert_eq!(design.index.gear, 64);
    assert_eq!(design.index.symmetry, 4);
    assert_eq!(design.index.mirror, 0);
}

#[test]
fn parses_real_tier_and_facets() {
    let design = parse_gcs(GCS_OCTABAR_X_EXCERPT).expect("real excerpt must parse");
    assert_eq!(design.tiers.len(), 1);
    let tier = &design.tiers[0];
    assert_eq!(tier.name, "P1");
    assert!((tier.angle_deg - 126.389_999_389_648_42).abs() < 1e-9);
    assert!((tier.depth - 0.666_425_032_825_342_6).abs() < 1e-9);
    assert!(tier.visible);
    assert!(!tier.guide);
    assert_eq!(tier.facets.len(), 2);
    assert_eq!(tier.facets[0].vertices.len(), 6);
    assert_eq!(tier.facets[1].vertices.len(), 3);
    assert!((tier.facets[1].index_angle_deg - 45.0).abs() < 1e-9);
}

#[test]
fn converts_pavilion_angle_to_signed_asc_convention() {
    // Verified against the real sibling `.asc` (attached_files id 1118,
    // detail 553): tier "2" there is angle -53.61435, and 180 + (-53.61435) =
    // 126.38565 -- matching this tier's 126.39 (`.gcs` stores trig fields at
    // float32 precision, hence the small residual).
    let design = parse_gcs(GCS_OCTABAR_X_EXCERPT).expect("real excerpt must parse");
    let signed = design.tiers[0].to_signed_asc_angle();
    assert!(
        (signed - (-53.610_001)).abs() < 1e-3,
        "expected roughly -53.61, got {signed}"
    );
}

#[test]
fn parses_real_render_and_info() {
    let design = parse_gcs(GCS_OCTABAR_X_EXCERPT).expect("real excerpt must parse");
    let render = design.render.expect("render element must be present");
    assert_eq!(render.material, "176 Corundum");
    assert!((render.refractive_index - 1.76).abs() < 1e-9);

    let info = design.info.expect("info element must be present");
    assert_eq!(info.title.as_deref(), Some("FVS-044 Octabar-X PC 21.086F"));
    assert_eq!(info.author.as_deref(), Some("Van Sant, Fred W"));
    assert_eq!(info.shape.as_deref(), Some("Octagon"));
    // The real file embeds a trailing blank line inside this attribute value;
    // the tokenizer must not treat the embedded newline as ending the tag.
    assert!(
        info.date
            .as_deref()
            .unwrap_or("")
            .starts_with("Star Cuts 1 1998")
    );
}

#[test]
fn facet_plane_count_sums_across_tiers() {
    let design = parse_gcs(GCS_OCTABAR_X_EXCERPT).expect("real excerpt must parse");
    assert_eq!(design.facet_plane_count(), 2);
}

#[test]
fn rejects_empty_input() {
    assert!(parse_gcs("").is_err());
    assert!(parse_gcs("   \n  ").is_err());
}

#[test]
fn rejects_missing_root_element() {
    let err = parse_gcs("<index gear=\"64\" base=\"0\" symmetry=\"4\" mirror=\"0\"/>")
        .expect_err("must reject a file with no GemCutStudio root");
    assert_eq!(err, GcsParseError::MissingRootElement);
}

#[test]
fn rejects_missing_index_element() {
    let err = parse_gcs("<GemCutStudio version=\"1000\"></GemCutStudio>")
        .expect_err("must reject a file with no index element");
    assert_eq!(err, GcsParseError::MissingIndexElement);
}

#[test]
fn rejects_unterminated_tag() {
    let err = parse_gcs("<GemCutStudio version=\"1000\"\n<index gear=\"64\"")
        .expect_err("must reject an unterminated tag");
    assert!(matches!(err, GcsParseError::UnterminatedTag { .. }));
}

#[test]
fn rejects_non_numeric_tier_angle() {
    let content = r#"<GemCutStudio version="1000">
    <index gear="64" base="0" symmetry="4" mirror="0"/>
    <tier angle="not-a-number" depth="1.0" name="T" instructions="" visible="true" guide="false">
    </tier>
</GemCutStudio>"#;
    let err = parse_gcs(content).expect_err("must reject a non-numeric tier angle");
    assert!(matches!(
        err,
        GcsParseError::TierAttributeNotNumeric { attr: "angle", .. }
    ));
}

/// `parse_index_u32` must reject a value bigger than `u32::MAX` rather
/// than silently saturating to it via `as u32`.
#[test]
fn rejects_an_index_attribute_larger_than_u32_max() {
    let content = r#"<GemCutStudio version="1000">
    <index gear="1e20" base="0" symmetry="4" mirror="0"/>
</GemCutStudio>"#;
    let err = parse_gcs(content).expect_err("must reject a gear value beyond u32::MAX");
    assert!(
        matches!(
            err,
            GcsParseError::IndexAttributeNotNumeric { attr: "gear", .. }
        ),
        "{err:?}"
    );
}

/// A garbled (present but non-numeric) `<render>` attribute must be
/// reported, not silently treated as `0.0`.
#[test]
fn rejects_a_non_numeric_render_attribute() {
    let content = r#"<GemCutStudio version="1000">
    <index gear="64" base="0" symmetry="4" mirror="0"/>
    <render material="Diamond" refractive_index="not-a-number" dispersion="0.02" clarity="100" density="3.5" lighting_model="Random">
    </render>
</GemCutStudio>"#;
    let err = parse_gcs(content).expect_err("must reject a garbled refractive_index");
    assert!(
        matches!(
            err,
            GcsParseError::RenderAttributeNotNumeric {
                attr: "refractive_index",
                ..
            }
        ),
        "{err:?}"
    );
}

/// An ABSENT `<render>` numeric attribute is still not an error -- only a present
/// but garbled one is (see `render_f64`'s doc comment).
#[test]
fn a_missing_render_attribute_defaults_to_zero_without_erroring() {
    let content = r#"<GemCutStudio version="1000">
    <index gear="64" base="0" symmetry="4" mirror="0"/>
    <render material="Diamond" lighting_model="Random">
    </render>
</GemCutStudio>"#;
    let design = parse_gcs(content).expect("an absent render attribute must not error");
    let render = design.render.expect("render element must still be parsed");
    assert_eq!(render.refractive_index, 0.0);
    assert_eq!(render.dispersion, 0.0);
    assert_eq!(render.clarity, 0.0);
    assert_eq!(render.density, 0.0);
}

#[test]
fn does_not_panic_on_arbitrary_garbage() {
    let samples = [
        "\u{0}\u{1}\u{2}garbage\u{ff}",
        "<<<>>>",
        "<GemCutStudio",
        "<GemCutStudio version=\"1000\">",
    ];
    for s in samples {
        let _ = parse_gcs(s);
    }
}
