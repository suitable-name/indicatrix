//! Tests for the `.gem` decoder: synthetic files from a test-only encoder, and
//! `#[ignore]` corpus checks that read `INDICATRIX_FORMAT_CORPUS_DIR`.

use super::{GemDesign, GemFacet, GemParseError, gem_to_asc_schedule, parse_gem};
use crate::{
    asc::{AscSchedule, parse_asc, to_asc_string},
    encoding::windows_1252_byte,
};

const SENTINEL: f64 = -99_999.0;

/// Appends one length-prefixed Windows-1252 string. With `varint`, a string of 128
/// bytes or more gets a .NET 7-bit varint length.
fn push_string(out: &mut Vec<u8>, text: &str, varint: bool) {
    let bytes: Vec<u8> = text
        .chars()
        .map(|c| windows_1252_byte(c).expect("test strings are Windows-1252"))
        .collect();
    let mut len = bytes.len();
    if varint && len >= 0x80 {
        while len >= 0x80 {
            out.push(u8::try_from(len & 0x7F).expect("7 bits") | 0x80);
            len >>= 7;
        }
        out.push(u8::try_from(len).expect("last varint byte"));
    } else {
        out.push(u8::try_from(len).expect("labels under 256 bytes"));
    }
    out.extend_from_slice(&bytes);
}

/// Appends one design (facets, sentinel, trailer, optional preform).
fn encode_design(design: &GemDesign, out: &mut Vec<u8>, varint: bool) {
    for facet in &design.facets {
        for c in facet.plane {
            out.extend_from_slice(&c.to_le_bytes());
        }
        out.extend_from_slice(&facet.tier.to_le_bytes());
        let label = format!(
            "{}\t{}",
            facet.name.as_deref().unwrap_or(""),
            facet.instructions
        );
        push_string(out, &label, varint);
        for v in &facet.vertices {
            out.extend_from_slice(&1i32.to_le_bytes());
            for c in v {
                out.extend_from_slice(&c.to_le_bytes());
            }
        }
        out.extend_from_slice(&0i32.to_le_bytes());
    }
    out.extend_from_slice(&SENTINEL.to_le_bytes());
    out.extend_from_slice(&design.symmetry.to_le_bytes());
    out.extend_from_slice(&i32::from(design.mirror).to_le_bytes());
    out.extend_from_slice(&design.gear.to_le_bytes());
    out.extend_from_slice(&design.refractive_index.to_le_bytes());
    out.extend_from_slice(&design.unknown_7fff.to_le_bytes());
    out.extend_from_slice(&design.gear_offset.to_le_bytes());
    for s in design.headings.iter().chain(&design.footnotes) {
        push_string(out, s, varint);
    }
    if let Some(preform) = &design.preform {
        out.extend_from_slice(b"\x07preform");
        encode_design(preform, out, varint);
    }
}

/// Test-only `.gem` encoder: the exact inverse of [`parse_gem`].
fn encode_gem(design: &GemDesign) -> Vec<u8> {
    let mut out = Vec::new();
    encode_design(design, &mut out, false);
    out
}

/// A facet at `.asc` `angle` (signed; `-0.0` = culet, `±90` = girdle side),
/// `distance` and tooth `index` for gear `gear`/`offset`, with a triangle of
/// vertices on its plane scaled by `vertex_scale`.
fn facet(angle: f64, distance: f64, index: f64, gear: i32, offset: f64) -> GemFacet {
    let phi = (90.0 - 360.0 * (index - offset) / f64::from(gear)).to_radians();
    let theta = angle.abs().to_radians();
    let nz = if angle.abs() == 90.0 {
        if angle.is_sign_negative() { -0.0 } else { 0.0 }
    } else if angle.is_sign_negative() {
        -theta.cos()
    } else {
        theta.cos()
    };
    let n = [theta.sin() * phi.cos(), theta.sin() * phi.sin(), nz];
    let plane = n.map(|c| c / distance);
    let foot = n.map(|c| c * distance);
    GemFacet {
        plane,
        tier: 0,
        name: None,
        instructions: String::new(),
        vertices: vec![foot, foot, foot],
    }
}

/// Builds a tier's facets at the given teeth, naming the first one.
fn tier(
    number: i32,
    angle: f64,
    distance: f64,
    teeth: &[f64],
    name: &str,
    instructions: &str,
) -> Vec<GemFacet> {
    teeth
        .iter()
        .enumerate()
        .map(|(k, &i)| GemFacet {
            tier: number,
            name: (k == 0).then(|| name.to_string()),
            instructions: instructions.to_string(),
            ..facet(angle, distance, i, -96, 48.0)
        })
        .collect()
}

/// Two crown tiers, a girdle, a pavilion tier, table and culet; gear -96 with
/// offset 48; a tab-separated label on every facet; a Windows-1252 `É` heading;
/// and a preform section.
fn sample_design() -> GemDesign {
    let mut facets = Vec::new();
    facets.extend(tier(1, 0.0, 0.45, &[96.0], "T", ""));
    facets.extend(tier(
        2,
        42.0,
        0.8,
        &[12.0, 36.0, 60.0, 84.0],
        "C1",
        "meet 3",
    ));
    facets.extend(tier(3, 27.5, 0.7, &[24.0, 48.0, 72.0, 96.0], "C2", ""));
    facets.extend(tier(
        4,
        -90.0,
        1.0,
        &[6.0, 30.0, 54.0, 78.0],
        "G",
        "level girdle",
    ));
    facets.extend(tier(
        5,
        -41.0,
        0.75,
        &[3.5, 27.5, 51.5, 75.5],
        "P1",
        "cut to centerpoint",
    ));
    facets.extend(tier(6, -0.0, 0.3, &[96.0], "CU", ""));
    let mut preform = GemDesign {
        facets: tier(1, -45.0, 0.9, &[12.0, 60.0], "PF1", ""),
        gear: -96,
        gear_offset: 48.0,
        unknown_7fff: 0x7FFF,
        ..GemDesign::default()
    };
    preform.vertex_scale = GemDesign::median_vertex_scale(&preform.facets);
    let mut design = GemDesign {
        facets,
        symmetry: 4,
        mirror: true,
        gear: -96,
        refractive_index: 1.54,
        gear_offset: 48.0,
        unknown_7fff: 0x7FFF,
        headings: [
            "Étoile de France".to_string(),
            "by A. Designer".to_string(),
            String::new(),
            String::new(),
        ],
        footnotes: [
            "For quartz".to_string(),
            String::new(),
            String::new(),
            String::new(),
        ],
        preform: Some(Box::new(preform)),
        vertex_scale: 1.0,
    };
    design.vertex_scale = GemDesign::median_vertex_scale(&design.facets);
    design
}

#[test]
fn round_trips_a_synthetic_design() {
    let design = sample_design();
    let parsed = parse_gem(&encode_gem(&design)).expect("synthetic file must parse");
    assert_eq!(parsed, design);
    assert_eq!(parsed.title(), Some("Étoile de France"));
    assert!((parsed.vertex_scale - 1.0).abs() < 1e-12);
    let preform = parsed.preform.as_ref().expect("preform section");
    assert_eq!(preform.facets.len(), 2);
    assert_eq!(preform.facets[0].name.as_deref(), Some("PF1"));
}

#[test]
fn heading_byte_0xc9_decodes_as_windows_1252() {
    let bytes = encode_gem(&sample_design());
    let title = b"\xC9toile de France";
    assert!(bytes.windows(title.len()).any(|w| w == title));
}

#[test]
fn facet_helpers_follow_the_asc_conventions() {
    let design = sample_design();
    let f = &design.facets;
    assert_eq!(f[0].angle_deg(), 0.0);
    assert!(!f[0].angle_deg().is_sign_negative(), "table is +0");
    assert!(f[0].is_flat());
    assert!((f[1].angle_deg() - 42.0).abs() < 1e-9);
    assert!((f[1].distance() - 0.8).abs() < 1e-12);
    assert_eq!(f[9].angle_deg(), -90.0, "girdle side from the sign of pz");
    assert!((f[13].angle_deg() + 41.0).abs() < 1e-9);
    let culet = &f[17];
    assert_eq!(culet.angle_deg(), 0.0);
    assert!(culet.angle_deg().is_sign_negative(), "culet is -0");
    let n = f[1].normal();
    assert!((n[0].mul_add(n[0], n[1].mul_add(n[1], n[2] * n[2])) - 1.0).abs() < 1e-12);
    // Signed gear -96, offset 48: tooth 12 round-trips, tooth 96 reads 96 (not 0),
    // and a fractional cheater tooth survives.
    assert!((design.facet_index(&f[1]) - 12.0).abs() < 1e-9);
    assert!((design.facet_index(&f[8]) - 96.0).abs() < 1e-9);
    assert!((design.facet_index(&f[13]) - 3.5).abs() < 1e-9);
    assert_eq!(design.facet_index(&f[0]), 96.0);
    assert_eq!(f[1].index(0, 0.0), 0.0);
}

#[test]
fn converts_to_an_asc_schedule() {
    let schedule = gem_to_asc_schedule(&sample_design());
    assert_eq!(schedule.gear_teeth, -96);
    assert_eq!(schedule.gear_reference_angle, 48.0);
    assert_eq!(schedule.symmetry_order, 4);
    assert!(schedule.mirror);
    assert_eq!(schedule.headers, ["Étoile de France", "by A. Designer"]);
    assert_eq!(schedule.footnotes, ["For quartz"]);
    assert_eq!(schedule.gemcad_version, "5.0");
    assert_eq!(schedule.warnings.len(), 1, "preform notice");
    assert_eq!(schedule.tiers.len(), 6);
    let crown = &schedule.tiers[1];
    assert_eq!(crown.name, "C1");
    assert_eq!(crown.index_names, [(0, "C1".to_string())]);
    assert_eq!(crown.indices, [12.0, 36.0, 60.0, 84.0]);
    assert_eq!(crown.notes, "meet 3");
    assert!((crown.mast - 0.8).abs() < 1e-12);
    assert_eq!(schedule.tiers[3].angle_deg, -90.0);
    assert_eq!(schedule.tiers[4].indices, [3.5, 27.5, 51.5, 75.5]);
    assert!(schedule.tiers[5].is_culet());
    assert_eq!(schedule.tiers[5].indices, [96.0]);
    assert_eq!(schedule.tiers[0].indices, [96.0]);
}

#[test]
fn an_empty_facet_list_is_just_the_sentinel() {
    let design = GemDesign {
        gear: 96,
        unknown_7fff: 0x7FFF,
        ..GemDesign::default()
    };
    let bytes = encode_gem(&design);
    assert_eq!(&bytes[..8], &[0, 0, 0, 0, 0xF0, 0x69, 0xF8, 0xC0]);
    assert_eq!(parse_gem(&bytes).expect("sentinel-only file"), design);
}

#[test]
fn reads_varint_string_lengths_when_byte_lengths_fail() {
    let mut design = sample_design();
    design.footnotes[1] = "x".repeat(200);
    let mut varint_bytes = Vec::new();
    encode_design(&design, &mut varint_bytes, true);
    assert_eq!(parse_gem(&varint_bytes).expect("varint file"), design);
    // The same design with a plain 200-byte length prefix also parses.
    assert_eq!(parse_gem(&encode_gem(&design)).expect("byte file"), design);
}

#[test]
fn detects_the_0_81_vertex_scale() {
    let mut design = sample_design();
    for f in &mut design.facets {
        for v in &mut f.vertices {
            *v = v.map(|c| c * 0.81);
        }
    }
    let parsed = parse_gem(&encode_gem(&design)).expect("scaled file");
    assert!((parsed.vertex_scale - 0.81).abs() < 1e-12);
    let rescaled = parsed.facets[1].vertices_on_plane(parsed.vertex_scale);
    let p = parsed.facets[1].plane;
    let dot = p[0].mul_add(
        rescaled[0][0],
        p[1].mul_add(rescaled[0][1], p[2] * rescaled[0][2]),
    );
    assert!((dot - 1.0).abs() < 1e-12);
}

/// Every framing error, each at the byte where the layout breaks.
#[test]
fn reports_every_framing_error() {
    assert_eq!(parse_gem(&[]), Err(GemParseError::EmptyInput));
    let good = encode_gem(&sample_design());
    assert!(matches!(
        parse_gem(&good[..good.len() - 3]),
        Err(GemParseError::UnexpectedEof { .. })
    ));
    assert!(matches!(
        parse_gem(&good[..5]),
        Err(GemParseError::UnexpectedEof { offset: 0, .. })
    ));

    let one = GemDesign {
        facets: vec![facet(30.0, 0.8, 12.0, 96, 0.0)],
        gear: 96,
        ..GemDesign::default()
    };
    let bytes = encode_gem(&one);
    // Plane (24) + tier (4) + label length byte + "\t" (2) = first vertex flag at 30.
    let mut bad_flag = bytes.clone();
    bad_flag[30] = 2;
    assert_eq!(
        parse_gem(&bad_flag),
        Err(GemParseError::BadVertexFlag {
            offset: 30,
            found: 2
        })
    );
    let mut zero_plane = bytes.clone();
    zero_plane[..24].fill(0);
    assert_eq!(
        parse_gem(&zero_plane),
        Err(GemParseError::InvalidPlane { offset: 0 })
    );
    let mut nan_plane = bytes.clone();
    nan_plane[..8].copy_from_slice(&f64::NAN.to_le_bytes());
    assert_eq!(
        parse_gem(&nan_plane),
        Err(GemParseError::InvalidPlane { offset: 0 })
    );
    let mirror_at = bytes.len() - 8 - 32 + 4;
    let mut bad_mirror = bytes.clone();
    bad_mirror[mirror_at] = 5;
    assert_eq!(
        parse_gem(&bad_mirror),
        Err(GemParseError::BadMirrorFlag {
            offset: mirror_at,
            found: 5
        })
    );
    let mut trailing = bytes.clone();
    trailing.extend_from_slice(b"junk");
    assert_eq!(
        parse_gem(&trailing),
        Err(GemParseError::TrailingData {
            offset: bytes.len(),
            remaining: 4
        })
    );
}

#[test]
fn a_failed_varint_retry_reports_the_byte_framing_error() {
    // One facet whose label length is a varint of six continuation bytes: byte
    // framing fails (the 255-byte label runs past end of file), so the varint retry
    // runs, fails too (`InvalidStringLength`), and the first error is returned.
    let mut bytes = Vec::new();
    for c in [0.0f64, 0.0, 1.0] {
        bytes.extend_from_slice(&c.to_le_bytes());
    }
    bytes.extend_from_slice(&0i32.to_le_bytes());
    bytes.extend_from_slice(&[0xFF; 6]);
    assert!(matches!(
        parse_gem(&bytes),
        Err(GemParseError::UnexpectedEof { offset: 29, .. })
    ));
}

#[test]
fn does_not_panic_on_arbitrary_garbage() {
    let samples: &[&[u8]] = &[
        &[0xff; 32],
        &[0x00; 32],
        b"no structure here at all, just prose",
        &[0x05, b'h', b'i'],
    ];
    for s in samples {
        assert!(parse_gem(s).is_err());
    }
}

/// A file of 10,000 `preform` tags, each followed by a whole design, must fail at
/// the second tag: real files hold one preform, and recursing once per tag would
/// overflow the stack (an abort, not a catchable panic).
#[test]
fn nested_preform_tags_are_rejected_without_recursing() {
    let tag = b"\x07preform";
    let block = encode_gem(&GemDesign {
        gear: 96,
        unknown_7fff: 0x7FFF,
        ..GemDesign::default()
    });
    let mut bytes = block.clone();
    for _ in 0..10_000 {
        bytes.extend_from_slice(tag);
        bytes.extend_from_slice(&block);
    }
    assert_eq!(
        parse_gem(&bytes),
        Err(GemParseError::NestedPreform {
            offset: block.len() + tag.len() + block.len()
        })
    );
}

/// The `.asc` text of `schedule` read back through [`parse_asc`].
fn reparse(schedule: &AscSchedule) -> AscSchedule {
    let text = to_asc_string(schedule).expect("a converted schedule is writable");
    parse_asc(&text).unwrap_or_else(|e| panic!("the converted text must parse: {e}\n{text}"))
}

/// A one-tier design with the given trailer values, for the substitution tests.
fn design_with_trailer(
    gear: i32,
    symmetry: i32,
    refractive_index: f64,
    gear_offset: f64,
) -> GemDesign {
    GemDesign {
        facets: tier(2, 42.0, 0.8, &[12.0, 36.0, 60.0], "C1", ""),
        symmetry,
        gear,
        refractive_index,
        gear_offset,
        unknown_7fff: 0x7FFF,
        ..GemDesign::default()
    }
}

#[test]
fn the_converted_sample_design_reparses_as_asc() {
    let schedule = gem_to_asc_schedule(&sample_design());
    let reparsed = reparse(&schedule);
    assert_eq!(reparsed.tiers.len(), schedule.tiers.len());
    assert_eq!(reparsed.gear_teeth, schedule.gear_teeth);
    assert_eq!(reparsed.symmetry_order, schedule.symmetry_order);
    assert_eq!(reparsed.refractive_index, schedule.refractive_index);
}

#[test]
fn a_missing_refractive_index_and_zero_symmetry_are_defaulted_with_warnings() {
    for symmetry in [0, -3] {
        let schedule = gem_to_asc_schedule(&design_with_trailer(96, symmetry, 0.0, 0.0));
        assert_eq!(schedule.symmetry_order, 1, "symmetry {symmetry}");
        assert_eq!(schedule.refractive_index, 1.54);
        assert_eq!(schedule.gear_teeth, 96);
        assert_eq!(schedule.warnings.len(), 2, "{:?}", schedule.warnings);
        let reparsed = reparse(&schedule);
        assert_eq!(reparsed.symmetry_order, 1);
        assert_eq!(reparsed.refractive_index, 1.54);
    }
    for refractive_index in [1.0, -2.0, f64::NAN, f64::INFINITY] {
        let schedule = gem_to_asc_schedule(&design_with_trailer(96, 4, refractive_index, 0.0));
        assert_eq!(schedule.refractive_index, 1.54, "index {refractive_index}");
        assert_eq!(schedule.warnings.len(), 1, "{:?}", schedule.warnings);
        assert_eq!(reparse(&schedule).refractive_index, 1.54);
    }
}

#[test]
fn a_zero_or_absurd_gear_and_a_non_finite_offset_are_defaulted_with_warnings() {
    for gear in [0, 721, -721, i32::MAX, i32::MIN] {
        let schedule = gem_to_asc_schedule(&design_with_trailer(gear, 4, 1.54, f64::NAN));
        assert_eq!(schedule.gear_teeth, 96, "gear {gear}");
        assert_eq!(schedule.gear_reference_angle, 0.0);
        assert_eq!(schedule.warnings.len(), 2, "{:?}", schedule.warnings);
        let reparsed = reparse(&schedule);
        assert_eq!(reparsed.gear_teeth, 96);
        assert!(reparsed.tiers[0].indices.iter().all(|i| i.is_finite()));
    }
    // The limits themselves, in either handedness, pass through untouched.
    for gear in [720, -720, 1, -1] {
        let schedule = gem_to_asc_schedule(&design_with_trailer(gear, 4, 1.54, 0.0));
        assert_eq!(schedule.gear_teeth, gear);
        assert!(schedule.warnings.is_empty(), "{:?}", schedule.warnings);
        assert_eq!(reparse(&schedule).gear_teeth, gear);
    }
}

/// The corpus directory from `INDICATRIX_FORMAT_CORPUS_DIR`.
fn corpus_dir() -> std::path::PathBuf {
    std::env::var_os("INDICATRIX_FORMAT_CORPUS_DIR")
        .expect("set INDICATRIX_FORMAT_CORPUS_DIR to the fmt_probe directory")
        .into()
}

/// Every file in `dir` with extension `ext`, sorted by name.
fn corpus_files(dir: &std::path::Path, ext: &str) -> Vec<std::path::PathBuf> {
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .expect("corpus directory")
        .map(|entry| entry.expect("directory entry").path())
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case(ext)))
        .collect();
    files.sort();
    files
}

#[test]
#[ignore = "reads the .gem corpus from INDICATRIX_FORMAT_CORPUS_DIR"]
fn corpus_all_gem_files_parse() {
    let files = corpus_files(&corpus_dir().join("gem"), "gem");
    let (mut parsed, mut scaled, mut preforms, mut failures) = (0, 0, 0, Vec::new());
    for path in &files {
        match parse_gem(&std::fs::read(path).expect("read .gem")) {
            Ok(design) => {
                parsed += 1;
                scaled += usize::from((design.vertex_scale - 1.0).abs() > 1e-6);
                preforms += usize::from(design.preform.is_some());
            }
            Err(e) => failures.push(format!("{}: {e}", path.display())),
        }
    }
    println!(
        "gem corpus: {parsed}/{} parsed, {scaled} with vertex_scale != 1, {preforms} with a preform",
        files.len()
    );
    assert!(failures.is_empty(), "{failures:#?}");
    assert_ne!(files.len(), 0);
}

/// Compares a `.gem` against its exact `.asc` export tier by tier.
fn assert_matches_asc(gem_id: u32, asc_id: u32) {
    let dir = corpus_dir();
    let gem_bytes = std::fs::read(dir.join(format!("gem/{gem_id}.gem"))).expect("read .gem");
    let asc_bytes = std::fs::read(dir.join(format!("asc/{asc_id}.asc"))).expect("read .asc");
    let from_gem = gem_to_asc_schedule(&parse_gem(&gem_bytes).expect("parse .gem"));
    let asc = crate::asc::parse_asc_bytes(&asc_bytes).expect("parse .asc");
    assert_eq!(from_gem.gear_teeth, asc.gear_teeth, "{gem_id}: gear");
    assert_eq!(
        from_gem.tiers.len(),
        asc.tiers.len(),
        "{gem_id}: tier count"
    );
    let teeth = f64::from(asc.gear_teeth_abs());
    let norm = |i: f64| {
        let r = i.rem_euclid(teeth);
        if r < 1e-9 { teeth } else { r }
    };
    for (k, (g, a)) in from_gem.tiers.iter().zip(&asc.tiers).enumerate() {
        assert!(
            (g.angle_deg - a.angle_deg).abs() < 1e-6,
            "{gem_id} tier {k}: angle"
        );
        assert_eq!(
            g.angle_deg.is_sign_negative(),
            a.angle_deg.is_sign_negative(),
            "{gem_id} tier {k}: side"
        );
        assert!(
            (g.mast - a.mast.abs()).abs() < 1e-6,
            "{gem_id} tier {k}: mast"
        );
        let mut gi: Vec<f64> = g.indices.iter().map(|&i| norm(i)).collect();
        let mut ai: Vec<f64> = a.indices.iter().map(|&i| norm(i)).collect();
        gi.sort_by(f64::total_cmp);
        ai.sort_by(f64::total_cmp);
        assert_eq!(gi.len(), ai.len(), "{gem_id} tier {k}: index count");
        for (x, y) in gi.iter().zip(&ai) {
            assert!(
                (x - y).abs() < 1e-6,
                "{gem_id} tier {k}: indices {gi:?} vs {ai:?}"
            );
        }
    }
    println!(
        "gem {gem_id} vs asc {asc_id}: {} tiers match",
        asc.tiers.len()
    );
}

#[test]
#[ignore = "reads the .gem/.asc corpus from INDICATRIX_FORMAT_CORPUS_DIR"]
fn corpus_gem_matches_exact_asc_exports() {
    assert_matches_asc(6095, 994);
    assert_matches_asc(6367, 2488);
    assert_matches_asc(6508, 158);
}
