//! Tests for [`super::parse_gcs`], [`super::gcs_to_asc_schedule`] and
//! [`super::to_gcs_string`]: a real trimmed excerpt, a spec-shaped example,
//! hand-built error-path fixtures, and `#[ignore]` corpus checks that read
//! `INDICATRIX_FORMAT_CORPUS_DIR`.

use super::{
    GcsDesign, GcsParseError, GcsWriteError, gcs_to_asc_schedule, parse_gcs, parse_gcs_bytes,
    side_rule_index_angle, to_gcs_string,
};
use crate::asc::{AscSchedule, AscTier};

mod hostile;

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

/// A garbled (present but non-numeric) `<color>` attribute must be reported, not
/// silently treated as `0.0` -- same rule as [`rejects_a_non_numeric_render_attribute`].
#[test]
fn rejects_a_non_numeric_color_attribute() {
    let content = r#"<GemCutStudio version="1000">
    <index gear="64" base="0" symmetry="4" mirror="0"/>
    <render material="Diamond" refractive_index="1.5" dispersion="0.02" clarity="100" density="3.5" lighting_model="Random">
        <color r="not-a-number" g="0" b="0"/>
    </render>
</GemCutStudio>"#;
    let err = parse_gcs(content).expect_err("must reject a garbled color attribute");
    assert!(
        matches!(
            err,
            GcsParseError::ColorAttributeNotNumeric { attr: "r", .. }
        ),
        "{err:?}"
    );
}

/// An ABSENT `<color>` attribute is still not an error -- only a present but garbled
/// one is, same convention as `<render>`.
#[test]
fn a_missing_color_attribute_defaults_to_zero_without_erroring() {
    let content = r#"<GemCutStudio version="1000">
    <index gear="64" base="0" symmetry="4" mirror="0"/>
    <render material="Diamond" lighting_model="Random">
        <color g="0.5"/>
    </render>
</GemCutStudio>"#;
    let design = parse_gcs(content).expect("an absent color attribute must not error");
    let color = design
        .render
        .expect("render element must still be parsed")
        .color;
    assert_eq!(color.r, 0.0);
    assert!((color.g - 0.5).abs() < 1e-9);
    assert_eq!(color.b, 0.0);
}

/// A 180-degree tier (a culet) must convert to `.asc`'s convention as a
/// NEGATIVE-signed zero, not plain (positive) `0.0` -- `.asc` distinguishes a
/// crown-side table (`0.0`) from a pavilion-side culet (`-0.0`) purely by sign bit.
#[test]
fn a_180_degree_tier_converts_to_a_negative_signed_zero() {
    let content = r#"<GemCutStudio version="1000">
    <index gear="64" base="0" symmetry="4" mirror="0"/>
    <tier angle="180" depth="0.5" name="Culet" instructions="" visible="true" guide="false">
        <facet nx="0" ny="0" nz="-1" index_angle="0">
            <vertex x="0" y="0" z="-1"/>
        </facet>
    </tier>
</GemCutStudio>"#;
    let design = parse_gcs(content).expect("must parse");
    let signed = design.tiers[0].to_signed_asc_angle();
    assert_eq!(signed, 0.0, "magnitude must still be zero");
    assert!(
        signed.is_sign_negative(),
        "a 180-degree culet must sign its zero negative, got {signed}"
    );
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

/// A spec-shaped design after the example in the Gem Cut Studio User's Manual
/// v1.1.0 pp. 58-61 (a square step cut; numbers from the example, comments ours),
/// plus an XML declaration, a single-quoted attribute and an entity. Its girdle's
/// `index_angle` values do not follow the side rule, as in the manual.
const GCS_SPEC_EXAMPLE: &str = r#"<?xml version="1.0" encoding="windows-1252"?>
<!-- A comment before the root, as in the manual's example. -->
<GemCutStudio version="1000">
  <!-- index: only "gear" is required; the rest is UI state -->
  <index gear="96" base="0" symmetry="4" mirror="0"/>
  <!-- pavilion tier: angle over 90 -->
  <tier angle="137.00000000000003" depth="0.67470026016235329" name="P1" instructions="Cut to point" visible="true" guide="false" frosting="0">
    <facet nx="-0" ny="-0.68199836006249825" nz="-0.73135370161917068" index_angle="0">
      <!-- vertices are optional -->
      <vertex x="1.1102230246251565e-016" y="1.1102230246251565e-016" z="-0.63505441789868788"/>
      <vertex x="-0.99999999999999989" y="-1.0000000000000002" z="0.29746066823897344"/>
      <vertex x="1" y="-1" z="0.29746066823897299"/>
    </facet>
    <facet nx="-0.68199836006249825" ny="-4.1760355433714127e-017" nz="-0.73135370161917068" index_angle="90"/>
    <facet nx="-8.3520710867428254e-017" ny="0.68199836006249825" nz="-0.73135370161917068" index_angle="180"/>
    <facet nx="0.68199836006249825" ny="1.2528106630114239e-016" nz="-0.73135370161917068" index_angle="270"/>
  </tier>
  <tier angle="90" depth="1" name="G1" instructions="Level girdle" visible="true" guide="false" frosting="0">
    <facet nx="0" ny="1" nz="0" index_angle="0"/>
    <facet nx="1" ny="6.123233995736766e-017" nz="0" index_angle="90"/>
    <facet nx="1.2246467991473532e-016" ny="-1" nz="0" index_angle="180"/>
    <facet nx="-1" ny="-1.8369701987210297e-016" nz="0" index_angle="270"/>
  </tier>
  <tier angle="32" depth="0.57782161235809326" name="C1" instructions="Set girdle width" visible="true" guide="false" frosting="0">
    <facet nx="-0" ny="-0.5299192642332049" nz="0.84804809615642596" index_angle="0"/>
    <facet nx="0.5299192642332049" ny="-3.2448196537485745e-017" nz="0.84804809615642596" index_angle="90"/>
    <facet nx="6.489639307497149e-017" ny="0.5299192642332049" nz="0.84804809615642596" index_angle="180"/>
    <facet nx="-0.5299192642332049" ny="9.7344589612457222e-017" nz="0.84804809615642596" index_angle="270"/>
  </tier>
  <tier angle="7.0167092985348768e-015" depth="0.46025484800338745" name="T" instructions="" visible="true" guide="false" frosting="0">
    <facet nx="-0" ny="-1.2246467991473532e-016" nz="1" index_angle="0"/>
  </tier>
  <!-- rendering parameters -->
  <render refractive_index="1.54" dispersion="0" clarity="100" density="1" lighting_model="Random">
    <color r="1" g="1" b="1"/>
  </render>
  <info title='Example &amp; test' author="R. P." date="Nov 7 2017" header2="Spec example" ri_min="1.4" ri_max="1.8099999" size_min="5" size_max="15" shape="Square" footer1="Only a test." footer2="A second footer."/>
</GemCutStudio>
"#;

#[test]
fn parses_the_spec_shaped_example_with_comments_and_declaration() {
    let design = parse_gcs(GCS_SPEC_EXAMPLE).expect("spec-shaped example must parse");
    assert!(design.warnings.is_empty(), "{:?}", design.warnings);
    assert_eq!(design.index.gear, 96);
    assert_eq!(design.tiers.len(), 4);
    let pavilion = &design.tiers[0];
    assert_eq!(pavilion.frosting, Some(0.0));
    assert!(!pavilion.is_frosted());
    assert_eq!(pavilion.facets.len(), 4);
    assert_eq!(pavilion.facets[0].vertices.len(), 3);
    assert_eq!(pavilion.facets[1].vertices.len(), 0);
    assert_eq!(design.tiers[1].to_signed_asc_angle(), -90.0);
    let info = design.info.expect("info");
    assert_eq!(info.title.as_deref(), Some("Example & test"));
}

#[test]
fn side_rule_gives_the_stored_index_on_real_facets() {
    let design = parse_gcs(GCS_OCTABAR_X_EXCERPT).expect("real excerpt must parse");
    let facets = &design.tiers[0].facets;
    // Pavilion: phi = -90 -> 270 - phi = 360 = 0; phi = -135 -> 405 = 45.
    assert!((side_rule_index_angle(facets[1].normal).expect("not flat") - 45.0).abs() < 1e-9);
    assert_eq!(facets[0].index(64), 64.0);
    assert!((facets[1].index(64) - 8.0).abs() < 1e-9);
    // Crown facets of the spec example wind the other way: phi = 0 -> 90.
    let spec = parse_gcs(GCS_SPEC_EXAMPLE).expect("spec example");
    let crown = &spec.tiers[2].facets;
    assert!((side_rule_index_angle(crown[1].normal).expect("not flat") - 90.0).abs() < 1e-9);
    assert!((crown[1].index(96) - 24.0).abs() < 1e-9);
    assert_eq!(side_rule_index_angle(spec.tiers[3].facets[0].normal), None);
}

#[test]
fn decodes_entities_in_one_pass_and_numeric_references() {
    let content = r#"<GemCutStudio version="1000"><index gear="96"/>
        <info title="a &amp;lt; b &#233;&#xE9; &bogus; &amp;"/></GemCutStudio>"#;
    let design = parse_gcs(content).expect("must parse");
    assert_eq!(
        design.info.and_then(|i| i.title).as_deref(),
        Some("a &lt; b éé &bogus; &")
    );
}

#[test]
fn reads_windows_1252_bytes() {
    let bytes = b"<GemCutStudio version=\"1000\"><index gear=\"96\"/>\
        <tier angle=\"0\" depth=\"1\" instructions=\"CAM 41.13\xBA \"/></GemCutStudio>";
    let design = parse_gcs_bytes(bytes).expect("Windows-1252 file must parse");
    assert_eq!(design.tiers[0].instructions, "CAM 41.13º ");
}

#[test]
fn optional_attributes_are_optional() {
    let content = r#"<GemCutStudio version="1000">
    <index gear="96"></index>
    <tier angle="0" name="T">
        <facet nx="0" ny="0" nz="1" frosting="0.5">
            <vertex x="0.3" y="0" z="0.5"/>
            <vertex x="0" y="0.3" z="0.5"/>
        </facet>
    </tier>
    <tier angle="137" depth="0.7">
        <facet index_angle="90"/>
    </tier>
    <tier angle="32" depth="0.6">
        <facet nx="0.5299192642332049" ny="0" nz="0.84804809615642596"/>
    </tier>
</GemCutStudio>"#;
    let design = parse_gcs(content).expect("spec-optional attributes may be absent");
    assert_eq!(design.index.base, 0.0);
    assert_eq!(design.index.symmetry, 1);
    assert_eq!(design.index.mirror, 0);
    let table = &design.tiers[0];
    assert!((table.depth - 0.5).abs() < 1e-12, "depth from the vertices");
    assert!(table.facets[0].is_frosted());
    let derived = design.tiers[1].facets[0].normal;
    assert!((derived[0] + 137.0_f64.to_radians().sin()).abs() < 1e-12);
    assert!(derived[1].abs() < 1e-12);
    assert!((design.tiers[2].facets[0].index_angle_deg - 90.0).abs() < 1e-9);
}

#[test]
fn rejects_what_cannot_be_derived() {
    let no_depth = r#"<GemCutStudio version="1000"><index gear="96"/>
        <tier angle="0"><facet nx="0" ny="0" nz="1"/></tier></GemCutStudio>"#;
    assert!(matches!(
        parse_gcs(no_depth),
        Err(GcsParseError::TierAttributeMissing { attr: "depth", .. })
    ));
    let no_plane = r#"<GemCutStudio version="1000"><index gear="96"/>
        <tier angle="0" depth="1"><facet/></tier></GemCutStudio>"#;
    assert!(matches!(
        parse_gcs(no_plane),
        Err(GcsParseError::FacetAttributeMissing {
            attr: "index_angle",
            ..
        })
    ));
    let no_gear = r#"<GemCutStudio version="1000"><index base="0"/></GemCutStudio>"#;
    assert_eq!(
        parse_gcs(no_gear),
        Err(GcsParseError::IndexAttributeMissing { attr: "gear" })
    );
}

#[test]
fn unknown_markup_and_newer_versions_are_warnings() {
    let content = r#"<GemCutStudio version="2000"><index gear="96" extra="1"/>
        <pleochroism r="1"/></GemCutStudio>"#;
    let design = parse_gcs(content).expect("unknown markup is tolerated");
    assert_eq!(design.warnings.len(), 3, "{:?}", design.warnings);
}

/// Cutting instructions for a bounded octagon: crown, girdle, pavilion, table and
/// culet, gear 96.
fn octagon_schedule() -> AscSchedule {
    let eight: Vec<f64> = (1..=8).map(|k| f64::from(k) * 12.0).collect();
    let tier = |angle_deg: f64, mast: f64, indices: &[f64], name: &str| AscTier {
        angle_deg,
        mast,
        name: name.to_string(),
        indices: indices.to_vec(),
        notes: format!("{name} notes & more"),
        ..AscTier::default()
    };
    AscSchedule {
        gemcad_version: "5.0".to_string(),
        gear_teeth: 96,
        symmetry_order: 8,
        mirror: true,
        refractive_index: 1.76,
        headers: vec!["Octagon test".to_string(), "by nobody".to_string()],
        footnotes: vec!["Étude".to_string()],
        tiers: vec![
            tier(40.0, 0.9, &eight, "C1"),
            tier(-90.0, 1.0, &eight, "G"),
            tier(-41.0, 0.8, &eight, "P1"),
            tier(0.0, 0.6, &[96.0], "T"),
            tier(-0.0, 1.0, &[], "CU"),
        ],
        ..AscSchedule::default()
    }
}

/// Sorted index set of a `.gcs` tier, rounded as `.asc` conversion rounds it.
fn index_set(design: &GcsDesign, tier: usize) -> Vec<f64> {
    let gear = design.index.gear;
    let mut set: Vec<f64> = design.tiers[tier]
        .facets
        .iter()
        .map(|f| crate::gem::snap_index(f.index(gear), f64::from(gear)))
        .collect();
    set.sort_by(f64::total_cmp);
    set
}

/// The first difference between two designs' tiers (angle, index set and name
/// beyond `1e-6`, depth beyond `depth_tolerance`), and the largest depth difference.
fn tier_mismatch(a: &GcsDesign, b: &GcsDesign, depth_tolerance: f64) -> (Option<String>, f64) {
    if a.tiers.len() != b.tiers.len() {
        return (
            Some(format!("tier count {} vs {}", a.tiers.len(), b.tiers.len())),
            0.0,
        );
    }
    let mut worst_depth = 0.0_f64;
    let mut first = None;
    for (k, (x, y)) in a.tiers.iter().zip(&b.tiers).enumerate() {
        worst_depth = worst_depth.max((x.depth - y.depth).abs());
        let (ix, iy) = (index_set(a, k), index_set(b, k));
        let same_indices =
            ix.len() == iy.len() && ix.iter().zip(&iy).all(|(p, q)| (p - q).abs() < 1e-6);
        let problem = if (x.angle_deg - y.angle_deg).abs() >= 1e-6 {
            Some("angle")
        } else if (x.depth - y.depth).abs() > depth_tolerance {
            Some("depth")
        } else if !same_indices {
            Some("index set")
        } else if x.name != y.name {
            Some("name")
        } else {
            None
        };
        if first.is_none() {
            first = problem.map(|p| format!("tier {k} ({}): {p}", x.name));
        }
    }
    (first, worst_depth)
}

/// Parse -> convert -> write -> parse.
fn rewrite(design: &GcsDesign) -> GcsDesign {
    let text = to_gcs_string(&gcs_to_asc_schedule(design)).expect("writable");
    parse_gcs(&text).expect("the writer's output parses")
}

#[test]
fn writer_output_is_normalised_crlf_ascii() {
    let schedule = octagon_schedule();
    let text = to_gcs_string(&schedule).expect("a bounded octagon is writable");
    assert!(text.starts_with("<GemCutStudio version=\"1000\">\r\n"));
    assert!(text.is_ascii());
    assert_eq!(text.matches('\n').count(), text.matches("\r\n").count());
    let design = parse_gcs(&text).expect("the writer's output parses");
    assert!(design.warnings.is_empty(), "{:?}", design.warnings);
    let angles: Vec<f64> = design.tiers.iter().map(|t| t.angle_deg).collect();
    assert_eq!(angles, [40.0, 90.0, 139.0, 0.0, 180.0]);
    let (mut reach, mut z_min, mut z_max) = (0.0_f64, f64::MAX, f64::MIN);
    for tier in &design.tiers {
        for facet in &tier.facets {
            assert!(facet.vertices.len() >= 3, "{}", tier.name);
            for v in &facet.vertices {
                let n = facet.normal;
                let dot = n[2].mul_add(v.z, n[0].mul_add(v.x, n[1] * v.y));
                assert!(
                    (dot - tier.depth).abs() < 1e-9,
                    "{}: vertex off its plane",
                    tier.name
                );
                reach = reach.max(v.x.abs().max(v.y.abs()));
                z_min = z_min.min(v.z);
                z_max = z_max.max(v.z);
            }
        }
    }
    assert!((reach - 1.0).abs() < 1e-12);
    assert!((z_min + z_max).abs() < 1e-12);
    let crown: Vec<f64> = index_set(&design, 0).iter().map(|i| i.round()).collect();
    assert_eq!(crown, [12.0, 24.0, 36.0, 48.0, 60.0, 72.0, 84.0, 96.0]);
    assert_eq!(index_set(&design, 4), [96.0]);
    assert_eq!(design.tiers[0].instructions, "C1 notes & more");
    let info = design.info.expect("info");
    assert_eq!(info.title.as_deref(), Some("Octagon test"));
    assert_eq!(info.footer1.as_deref(), Some("Étude"));
    assert!((design.render.expect("render").refractive_index - 1.76).abs() < 1e-12);
}

#[test]
fn parse_write_parse_round_trips_tiers() {
    let first = parse_gcs(&to_gcs_string(&octagon_schedule()).expect("writable")).expect("parses");
    let (mismatch, worst) = tier_mismatch(&first, &rewrite(&first), 1e-6);
    assert_eq!(mismatch, None, "worst depth difference {worst}");
    // The spec example is not normalised (its depths do not match its vertices), so
    // its first rewrite rescales depths; after that the round trip is exact.
    let spec = parse_gcs(GCS_SPEC_EXAMPLE).expect("spec example");
    let once = rewrite(&spec);
    for (k, (a, b)) in spec.tiers.iter().zip(&once.tiers).enumerate() {
        assert!((a.angle_deg - b.angle_deg).abs() < 1e-9);
        assert_eq!(a.name, b.name);
        let (before, after) = (index_set(&spec, k), index_set(&once, k));
        assert_eq!(before.len(), after.len());
        assert!(before.iter().zip(&after).all(|(p, q)| (p - q).abs() < 1e-9));
    }
    let (mismatch, worst) = tier_mismatch(&once, &rewrite(&once), 1e-6);
    assert_eq!(mismatch, None, "worst depth difference {worst}");
}

#[test]
fn a_negative_gear_and_offset_fold_into_the_tooth() {
    let mut schedule = octagon_schedule();
    schedule.gear_teeth = -96;
    schedule.gear_reference_angle = 48.0;
    schedule.tiers.push(AscTier {
        angle_deg: 30.0,
        mast: 0.95,
        indices: vec![3.0],
        ..AscTier::default()
    });
    let design = parse_gcs(&to_gcs_string(&schedule).expect("writable")).expect("parses");
    assert_eq!(design.index.gear, 96);
    // t = -(i - 48): 12 -> 36 (index_angle 135), 3 -> 45.
    assert!((design.tiers[0].facets[0].index_angle_deg - 135.0).abs() < 1e-9);
    assert!((design.tiers[5].facets[0].index(96) - 45.0).abs() < 1e-9);
}

#[test]
fn writer_errors() {
    let excerpt = parse_gcs(GCS_OCTABAR_X_EXCERPT).expect("real excerpt must parse");
    assert_eq!(
        to_gcs_string(&gcs_to_asc_schedule(&excerpt)),
        Err(GcsWriteError::Unbounded)
    );
    let mut schedule = octagon_schedule();
    schedule.gear_teeth = 0;
    assert_eq!(to_gcs_string(&schedule), Err(GcsWriteError::ZeroGear));
    let mut schedule = octagon_schedule();
    schedule.tiers.clear();
    assert_eq!(to_gcs_string(&schedule), Err(GcsWriteError::NoTiers));
    let mut schedule = octagon_schedule();
    schedule.tiers[2].mast = f64::NAN;
    assert_eq!(
        to_gcs_string(&schedule),
        Err(GcsWriteError::NonFinite { tier_index: 2 })
    );
}

#[test]
fn converts_to_an_asc_schedule() {
    let schedule = gcs_to_asc_schedule(&parse_gcs(GCS_OCTABAR_X_EXCERPT).expect("parses"));
    assert_eq!(schedule.gear_teeth, 64);
    assert_eq!(schedule.symmetry_order, 4);
    assert!(!schedule.mirror);
    assert!((schedule.refractive_index - 1.76).abs() < 1e-12);
    assert_eq!(schedule.headers[0], "FVS-044 Octabar-X PC 21.086F");
    assert_eq!(
        schedule.headers[2], "Star Cuts 1 1998",
        "CR/LF runs collapse"
    );
    assert_eq!(schedule.footnotes.len(), 2);
    let tier = &schedule.tiers[0];
    assert_eq!(tier.name, "P1");
    assert_eq!(tier.indices, [64.0, 8.0]);
    assert!((tier.angle_deg + 53.610_000_610_351_58).abs() < 1e-9);
    assert!((tier.mast - 0.666_425_032_825_342_6).abs() < 1e-12);
    crate::asc::to_asc_string(&schedule).expect("the converted schedule writes as .asc");
}

/// Every `.gcs` file in `INDICATRIX_FORMAT_CORPUS_DIR/gcs`, sorted by name.
fn corpus_gcs_files() -> Vec<(std::path::PathBuf, Vec<u8>)> {
    let dir = std::path::PathBuf::from(
        std::env::var_os("INDICATRIX_FORMAT_CORPUS_DIR")
            .expect("set INDICATRIX_FORMAT_CORPUS_DIR to the fmt_probe directory"),
    )
    .join("gcs");
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .expect("corpus directory")
        .map(|entry| entry.expect("directory entry").path())
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("gcs")))
        .collect();
    files.sort();
    files
        .into_iter()
        .map(|p| {
            let bytes = std::fs::read(&p).expect("read .gcs");
            (p, bytes)
        })
        .collect()
}

#[test]
#[ignore = "reads the .gcs corpus from INDICATRIX_FORMAT_CORPUS_DIR"]
fn corpus_all_gcs_files_parse() {
    let files = corpus_gcs_files();
    let (mut parsed, mut side_rule_ok, mut non_flat, mut failures) = (0, 0, 0, Vec::new());
    for (path, bytes) in &files {
        match parse_gcs_bytes(bytes) {
            Ok(design) => {
                parsed += 1;
                if !design.warnings.is_empty() {
                    failures.push(format!(
                        "{}: warnings {:?}",
                        path.display(),
                        design.warnings
                    ));
                }
                for facet in design.tiers.iter().flat_map(|t| &t.facets) {
                    if let Some(ia) = side_rule_index_angle(facet.normal) {
                        non_flat += 1;
                        let diff = (ia - facet.index_angle_deg).rem_euclid(360.0);
                        side_rule_ok += usize::from(diff.min(360.0 - diff) < 1e-6);
                    }
                }
            }
            Err(e) => failures.push(format!("{}: {e}", path.display())),
        }
    }
    println!(
        "gcs corpus: {parsed}/{} parsed; side rule holds on {side_rule_ok}/{non_flat} non-flat facets",
        files.len()
    );
    assert!(failures.is_empty(), "{failures:#?}");
    assert_eq!(side_rule_ok, non_flat);
    assert_ne!(files.len(), 0);
}

/// The largest distance of a stored vertex from its facet plane (`|n·v - depth|`).
fn own_vertex_residual(design: &GcsDesign) -> f64 {
    design
        .tiers
        .iter()
        .flat_map(|t| t.facets.iter().map(move |f| (t.depth, f)))
        .flat_map(|(depth, f)| {
            f.vertices.iter().map(move |v| {
                let n = f.normal;
                (n[2].mul_add(v.z, n[0].mul_add(v.x, n[1] * v.y)) - depth).abs()
            })
        })
        .fold(0.0, f64::max)
}

#[test]
#[ignore = "reads the .gcs corpus from INDICATRIX_FORMAT_CORPUS_DIR"]
fn corpus_gcs_writer_round_trip() {
    let files = corpus_gcs_files();
    let (mut passed, mut exact, mut worst, mut failures) = (0, 0, 0.0_f64, Vec::new());
    for (path, bytes) in &files {
        let design = parse_gcs_bytes(bytes).expect("corpus file parses");
        let reparsed = to_gcs_string(&gcs_to_asc_schedule(&design))
            .map_err(|e| e.to_string())
            .and_then(|text| parse_gcs(&text).map_err(|e| e.to_string()));
        match reparsed {
            Ok(again) => {
                // A file's depths cannot come back closer than its own vertices sit
                // to its planes: GCS normalised it by those vertices.
                let residual = own_vertex_residual(&design);
                let (mismatch, depth) = tier_mismatch(&design, &again, residual.max(1e-6));
                worst = worst.max(depth);
                exact += usize::from(depth <= 1e-6);
                match mismatch {
                    None => passed += 1,
                    Some(m) => failures.push(format!("{}: {m} (depth {depth:e})", path.display())),
                }
            }
            Err(e) => failures.push(format!("{}: {e}", path.display())),
        }
    }
    println!(
        "gcs writer round trip: {passed}/{} files equal (angle, index set, name to 1e-6; depth \
         to max(1e-6, the file's own vertex-to-plane residual)); {exact} with every depth \
         within 1e-6; worst depth difference {worst:e}",
        files.len()
    );
    assert!(failures.is_empty(), "{failures:#?}");
}
