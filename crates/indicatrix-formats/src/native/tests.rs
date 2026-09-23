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

// --- `PreformTable::y_offset` round-trips and defaults to `0.0` ---

/// `PreformTable::new` must default `y_offset` to `0.0`, and
/// [`PreformTable::with_y_offset`] must set exactly that field.
#[test]
fn preform_table_y_offset_defaults_to_zero_and_with_y_offset_sets_it() {
    let table = PreformTable::new(NativePreformShape::Block, 1.0, 1.0, 2.0);
    assert_eq!(table.y_offset, 0.0);
    let offset_table = table.with_y_offset(0.35);
    assert_eq!(offset_table.y_offset, 0.35);
}

/// A non-zero `y_offset` must survive a TOML serialize/parse round trip.
#[test]
fn preform_table_y_offset_round_trips_through_toml_text() {
    let mut native = sample_file();
    native.preform = native.preform.with_y_offset(0.35);
    let text = to_toml_string(&native).expect("must serialize");
    let parsed = from_toml_str(&text).expect("must parse its own output");
    assert_eq!(parsed.preform.y_offset, 0.35);
}

/// A sidecar written before `y_offset` existed (no such key in `[preform]` at all)
/// must still parse, with `y_offset` defaulting to `0.0`.
#[test]
fn a_preform_table_with_no_y_offset_key_parses_and_defaults_to_zero() {
    let text = r#"
format_version = 1
asc_filename = "design.asc"
asc_sha256 = "deadbeef"

[preform]
shape = { kind = "block" }
half_width = 1.0
length_over_width = 1.0
depth = 2.0

[material]
name = "Diamond"

[[tiers]]
name = "T"
constraint = { kind = "meet_existing" }
"#;
    let parsed = from_toml_str(text).expect("must parse a pre-item-208 sidecar");
    assert_eq!(parsed.preform.y_offset, 0.0);
}

// --- `MaterialTable::custom` round-trips through TOML text ---

/// A [`CustomMaterialSnapshot`] attached via [`MaterialTable::with_custom`] must
/// survive a serialize/parse round trip untouched.
#[test]
fn material_table_custom_snapshot_round_trips_through_toml_text() {
    let mut native = sample_file();
    let snapshot = CustomMaterialSnapshot::new(
        1.62,
        0.017,
        -0.021,
        Some(3.06),
        "Trigonal",
        "UniaxialNegative",
    );
    native.material = native.material.with_custom(Some(snapshot.clone()));
    let text = to_toml_string(&native).expect("must serialize");
    let parsed = from_toml_str(&text).expect("must parse its own output");
    assert_eq!(parsed.material.custom, Some(snapshot));
}

/// A material with no custom snapshot must not even write a `[material.custom]`
/// table, and must still load back as `None`.
#[test]
fn material_table_with_no_custom_snapshot_omits_the_table() {
    let native = sample_file();
    let text = to_toml_string(&native).expect("must serialize");
    assert!(!text.contains("[material.custom]"));
    let parsed = from_toml_str(&text).expect("must parse");
    assert_eq!(parsed.material.custom, None);
}

// --- `NativeDesignFile::history` round-trips through TOML text ---

/// A [`HistoryTable`] attached via [`NativeDesignFile::with_history`] must survive a
/// serialize/parse round trip untouched.
#[test]
fn history_table_round_trips_through_toml_text() {
    let native = sample_file().with_history(HistoryTable::new(vec![
        "Set material to Diamond".to_string(),
        "Remove tier C1".to_string(),
    ]));
    let text = to_toml_string(&native).expect("must serialize");
    let parsed = from_toml_str(&text).expect("must parse its own output");
    assert_eq!(
        parsed.history.map(|h| h.entries),
        Some(vec![
            "Set material to Diamond".to_string(),
            "Remove tier C1".to_string(),
        ])
    );
}

/// An empty history must not grow a `[history]` table at all -- see
/// [`NativeDesignFile::with_history`]'s own doc comment.
#[test]
fn an_empty_history_table_is_not_attached_at_all() {
    let native = sample_file().with_history(HistoryTable::new(Vec::new()));
    assert!(native.history.is_none());
}

// --- Stable tier ids, authoring-level tier targets, and the authored
// refractive index ---

/// A [`TierTable::tier_id`] attached via [`TierTable::with_tier_id`] must survive
/// a serialize/parse round trip untouched.
#[test]
fn tier_id_round_trips_through_toml_text() {
    let native = NativeDesignFile::new(
        "design.asc",
        sha256_hex(b"asc bytes"),
        PreformTable::new(NativePreformShape::Block, 1.0, 1.0, 2.0),
        Some(6.5),
        MaterialTable::new(Some("Diamond".to_string()), None, Some(1.62)),
        vec![
            TierTable::new("A", NativeMeetConstraint::MeetExisting, vec![]).with_tier_id(Some(0)),
            TierTable::new("B", NativeMeetConstraint::MeetExisting, vec![]).with_tier_id(Some(1)),
        ],
    );
    let text = to_toml_string(&native).expect("must serialize");
    let parsed = from_toml_str(&text).expect("must parse its own output");
    assert_eq!(parsed.tiers[0].tier_id, Some(0));
    assert_eq!(parsed.tiers[1].tier_id, Some(1));
}

/// An OLD file (written before `TierTable::tier_id` existed) must still parse,
/// with every tier's `tier_id` reading back as `None` -- the "ids assigned on
/// load" half of stable tier ids is the loader's job
/// (`indicatrix-cut-core::native`, outside this crate), but this crate's own
/// half of the contract is that such a file is not rejected and carries no
/// fabricated id.
#[test]
fn a_fixture_without_tier_ids_still_parses_with_no_ids() {
    let text = r#"
format_version = 1
asc_filename = "design.asc"
asc_sha256 = "deadbeef"

[preform]
shape = { kind = "block" }
half_width = 1.0
length_over_width = 1.0
depth = 2.0

[material]
name = "Diamond"

[[tiers]]
name = "T1"
constraint = { kind = "meet_existing" }

[[tiers]]
name = "T2"
constraint = { kind = "scale_reference", mast = 0.5 }
"#;
    let parsed = from_toml_str(text).expect("an old fixture with no tier ids must still parse");
    assert_eq!(parsed.tiers.len(), 2);
    assert_eq!(parsed.tiers[0].tier_id, None);
    assert_eq!(parsed.tiers[1].tier_id, None);
    // The round trip must not silently invent one either.
    let reserialized = to_toml_string(&parsed).expect("must reserialize");
    let reparsed = from_toml_str(&reserialized).expect("must reparse");
    assert_eq!(reparsed.tiers[0].tier_id, None);
}

/// A [`NativeTierTarget`] attached via [`TierTable::with_target`] must survive a
/// serialize/parse round trip untouched, for all three variants.
#[test]
fn tier_target_round_trips_through_toml_text() {
    let native = NativeDesignFile::new(
        "design.asc",
        sha256_hex(b"asc bytes"),
        PreformTable::new(NativePreformShape::Block, 1.0, 1.0, 2.0),
        Some(6.5),
        MaterialTable::new(Some("Diamond".to_string()), None, Some(1.62)),
        vec![
            TierTable::new("Pavilion Main", NativeMeetConstraint::MeetExisting, vec![])
                .with_target(Some(NativeTierTarget::DepthMm { mm: 3.20 })),
            TierTable::new("Girdle", NativeMeetConstraint::MeetExisting, vec![])
                .with_target(Some(NativeTierTarget::GirdleThicknessMm { mm: 0.25 })),
            TierTable::new("Table", NativeMeetConstraint::MeetExisting, vec![])
                .with_target(Some(NativeTierTarget::TableWidthMm { mm: 4.10 })),
        ],
    );
    let text = to_toml_string(&native).expect("must serialize");
    let parsed = from_toml_str(&text).expect("must parse its own output");
    assert_eq!(
        parsed.tiers[0].target,
        Some(NativeTierTarget::DepthMm { mm: 3.20 })
    );
    assert_eq!(
        parsed.tiers[1].target,
        Some(NativeTierTarget::GirdleThicknessMm { mm: 0.25 })
    );
    assert_eq!(
        parsed.tiers[2].target,
        Some(NativeTierTarget::TableWidthMm { mm: 4.10 })
    );
}

/// [`NativeDesignFile::authored_refractive_index`], attached via
/// [`NativeDesignFile::with_authored_refractive_index`], must survive a
/// serialize/parse round trip untouched and independently of
/// [`MaterialTable::refractive_index_override`] -- the whole point being that
/// the two can differ (an authored legacy RI the design's material selection
/// currently overrides for scoring, but which should come back unchanged if
/// the material selection is later cleared).
#[test]
fn authored_refractive_index_round_trips_independently_of_the_material_override() {
    let native = sample_file().with_authored_refractive_index(Some(1.54));
    let text = to_toml_string(&native).expect("must serialize");
    let parsed = from_toml_str(&text).expect("must parse its own output");
    assert_eq!(parsed.authored_refractive_index, Some(1.54));
    // `sample_file` itself sets a DIFFERENT effective override (1.62) -- the two
    // fields are independent, not aliases of each other.
    assert_eq!(parsed.material.refractive_index_override, Some(1.62));
}

/// An old file with no `authored_refractive_index` at all must still parse, as
/// `None` -- a loader then falls back to the paired `.asc`'s own `I` line,
/// exactly today's behavior for such a file.
#[test]
fn a_fixture_without_an_authored_refractive_index_still_parses() {
    let text = to_toml_string(&sample_file()).expect("must serialize");
    assert!(!text.contains("authored_refractive_index"));
    let parsed = from_toml_str(&text).expect("must parse");
    assert_eq!(parsed.authored_refractive_index, None);
}
