use super::*;

/// A minimal, valid native document used by several tests below -- built directly via
/// the `new` constructors since this crate has no editor `Design` type to build one
/// through (that higher-level assembly is `indicatrix-cut-core`'s `native::
/// to_native_file`, which this crate deliberately doesn't own -- see the module doc
/// comment).
fn sample_file() -> NativeDesignFile {
    NativeDesignFile::new(
        "design.asc",
        sha256_hex(b"asc bytes"),
        PreformTable::new(NativePreformShape::Block, 1.0, 1.0, 2.0),
        Some(6.5),
        MaterialTable::new(Some("Diamond".to_string()), None, Some(1.62)),
        vec![TierTable::new(
            "T",
            NativeMeetConstraint::MeetExisting,
            vec![0.0, 2.0],
        )],
    )
}

/// The core promise: serialize a document to TOML text, parse it back, and get an
/// equal struct -- the basic round trip every other test here builds on.
#[test]
fn native_file_round_trips_through_toml_text() {
    let native = sample_file();
    let text = to_toml_string(&native).expect("must serialize");
    let parsed = from_toml_str(&text).expect("must parse its own output");
    assert_eq!(native, parsed);
}

/// Determinism: serializing the same document twice must produce byte-identical text
/// (the module doc comment's "Format: TOML" section).
#[test]
fn serializing_the_same_document_twice_is_byte_identical() {
    let a = to_toml_string(&sample_file()).expect("must serialize");
    let b = to_toml_string(&sample_file()).expect("must serialize");
    assert_eq!(a, b);
}

/// A field this build's [`NativeDesignFile`] does not know about must survive a
/// parse-then-reserialize round trip untouched, at every nesting level this module
/// defines (top level, `preform`, `material`, a `[[tiers]]` entry).
#[test]
fn unknown_fields_survive_a_round_trip() {
    let text = r#"
format_version = 1
asc_filename = "design.asc"
asc_sha256 = "deadbeef"
future_top_level_field = "kept"

[preform]
shape = { kind = "block" }
half_width = 1.0
length_over_width = 1.0
depth = 2.0
future_preform_field = 42

[material]
name = "Diamond"
future_material_field = true

[[tiers]]
name = "T"
constraint = { kind = "meet_existing" }
future_tier_field = "kept too"
"#;
    let parsed = from_toml_str(text).expect("must parse a file with unknown fields");
    assert_eq!(
        parsed
            .unknown
            .get("future_top_level_field")
            .and_then(|v| v.as_str()),
        Some("kept")
    );
    assert_eq!(
        parsed
            .preform
            .unknown
            .get("future_preform_field")
            .and_then(toml::Value::as_integer),
        Some(42)
    );
    assert_eq!(
        parsed
            .material
            .unknown
            .get("future_material_field")
            .and_then(toml::Value::as_bool),
        Some(true)
    );
    assert_eq!(
        parsed.tiers[0]
            .unknown
            .get("future_tier_field")
            .and_then(|v| v.as_str()),
        Some("kept too")
    );

    // Re-serializing must still carry every key forward.
    let round_tripped = to_toml_string(&parsed).expect("must reserialize");
    let reparsed = from_toml_str(&round_tripped).expect("must reparse");
    assert_eq!(parsed, reparsed);
    assert!(round_tripped.contains("future_top_level_field"));
    assert!(round_tripped.contains("future_preform_field"));
    assert!(round_tripped.contains("future_material_field"));
    assert!(round_tripped.contains("future_tier_field"));
}

/// A native file with no override set must not even write the
/// `refractive_index_override` key (`#[serde(skip_serializing_if =
/// "Option::is_none")]`, same rule as `specific_gravity_override`), and that text
/// must still parse with the field loading back as `None`.
#[test]
fn a_material_table_with_no_ri_override_omits_the_key_and_still_loads_as_none() {
    let mut native = sample_file();
    native.material.refractive_index_override = None; // the case under test
    let text = to_toml_string(&native).expect("must serialize");
    assert!(
        !text.contains("refractive_index_override"),
        "an unset override must not appear in the written TOML at all:\n{text}"
    );

    let parsed = from_toml_str(&text).expect("must still parse without the key");
    assert_eq!(parsed.material.refractive_index_override, None);
}

/// [`native_path_for_asc`]/[`asc_path_for_native`] must be inverses of each other
/// for a well-formed pair, and the latter must recognize a path that doesn't end
/// in either the current or legacy suffix as not one of this format's files.
#[test]
fn native_and_asc_paths_convert_both_ways() {
    let asc = std::path::Path::new("C:/designs/RBC-445.asc");
    let native = native_path_for_asc(asc);
    assert_eq!(
        native,
        std::path::Path::new("C:/designs/RBC-445.indicatrix.toml")
    );
    assert_eq!(asc_path_for_native(&native).as_deref(), Some(asc));

    assert_eq!(
        asc_path_for_native(std::path::Path::new("not_native.txt")),
        None
    );
}

/// A legacy `.gemcut.toml` sidecar (this format's former name) must still resolve
/// back to its paired `.asc`.
#[test]
fn legacy_gemcut_suffix_still_resolves() {
    let legacy = std::path::Path::new("C:/designs/RBC-445.gemcut.toml");
    assert_eq!(
        asc_path_for_native(legacy).as_deref(),
        Some(std::path::Path::new("C:/designs/RBC-445.asc"))
    );
}

/// [`check_fingerprint`] must report a match against the exact bytes a document's
/// `asc_sha256` was computed from, and a mismatch (carrying both hashes) against any
/// other bytes.
#[test]
fn check_fingerprint_matches_identical_bytes_and_flags_a_change() {
    let native = sample_file();
    assert_eq!(
        check_fingerprint(&native, b"asc bytes"),
        FingerprintCheck::Match
    );
    match check_fingerprint(&native, b"different bytes") {
        FingerprintCheck::Mismatch {
            expected_sha256,
            found_sha256,
        } => {
            assert_eq!(expected_sha256, native.asc_sha256);
            assert_eq!(found_sha256, sha256_hex(b"different bytes"));
        }
        FingerprintCheck::Match => panic!("different bytes must not match"),
    }
}
