//! Round trips of the shared tables ([`PreformTable`], [`MaterialTable`], [`TierTable`],
//! [`HistoryTable`], [`CustomMaterialSnapshot`]) through the text of a design file. The
//! document itself ([`DesignFile`]) is tested in `design/tests.rs`; this crate has no
//! editor `Design` type, so documents are built directly via the `new` constructors.

use super::{
    design::{DesignFile, ScheduleTable, parse, to_string},
    *,
};

fn schedule() -> ScheduleTable {
    ScheduleTable {
        gemcad_version: String::new(),
        gear_teeth: 96,
        gear_reference_angle: 0.0,
        symmetry_order: 4,
        mirror: true,
        refractive_index: 1.62,
        headers: Vec::new(),
        footnotes: Vec::new(),
        unknown: toml::Table::new(),
    }
}

/// A tier with the geometry a design file requires of every tier.
fn tier(name: &str, constraint: NativeMeetConstraint) -> TierTable {
    TierTable::new(name, constraint, Vec::new())
        .with_angle_deg(Some(40.0))
        .with_indices(Some(vec![0.0, 24.0]))
}

/// A minimal, valid design document used by several tests below.
fn sample_file() -> DesignFile {
    DesignFile::new(
        PreformTable::new(NativePreformShape::Block, 1.0, 1.0, 2.0),
        MaterialTable::new(Some("Diamond".to_string()), None, Some(1.62)),
        schedule(),
        Some(6.5),
        vec![tier("T", NativeMeetConstraint::MeetExisting)],
    )
}

fn write(file: &DesignFile) -> String {
    to_string(file).expect("must serialize")
}

fn read(text: &str) -> DesignFile {
    parse(text).expect("must parse")
}

/// A hand-written design file of header `version` whose `[preform]` body is
/// `preform_body`, with one tier carrying `tier_extra` extra lines.
fn hand_written(version: u32, preform_body: &str, tier_extra: &str) -> String {
    format!(
        "format = \"indicatrix-design\"\nversion = {version}\n\n[preform]\n{preform_body}\n\n\
         [material]\nname = \"Diamond\"\n\n[schedule]\ngear_teeth = 96\n\
         gear_reference_angle = 0.0\nsymmetry_order = 4\nmirror = true\n\
         refractive_index = 1.62\n\n[[tiers]]\nname = \"T\"\n\
         constraint = {{ kind = \"meet_existing\" }}\nangle_deg = 40.0\n\
         indices = [0.0, 24.0]\n{tier_extra}"
    )
}

const BLOCK_PREFORM: &str =
    "shape = { kind = \"block\" }\nhalf_width = 1.0\nlength_over_width = 1.0\ndepth = 2.0";

/// A tier without a relation writes no `angle_relation` key (so a relation-less file's
/// bytes are what they always were), and one with a relation round-trips it.
#[test]
fn a_tier_relation_is_written_only_when_present_and_round_trips() {
    let plain = write(&sample_file());
    assert!(!plain.contains("angle_relation"));
    assert_eq!(read(&plain).tiers[0].angle_relation, None);

    let mut native = sample_file();
    native.tiers[0] = native.tiers[0]
        .clone()
        .with_angle_relation(Some("@4 - 2".to_string()));
    native.version = native.needed_version();
    let text = write(&native);
    assert_eq!(text.matches("angle_relation = \"@4 - 2\"").count(), 1);
    let parsed = read(&text);
    assert_eq!(parsed, native);
    assert_eq!(parsed.tiers[0].angle_relation.as_deref(), Some("@4 - 2"));
    assert_eq!(write(&parsed), text);
}

/// A material table with no override must not even write the
/// `refractive_index_override` key, and that text must still parse with the field
/// loading back as `None`.
#[test]
fn a_material_table_with_no_ri_override_omits_the_key_and_still_loads_as_none() {
    let mut native = sample_file();
    native.material.refractive_index_override = None; // the case under test
    let text = write(&native);
    assert!(
        !text.contains("refractive_index_override"),
        "an unset override must not appear in the written TOML at all:\n{text}"
    );
    assert_eq!(read(&text).material.refractive_index_override, None);
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

/// A non-zero `y_offset` must survive a serialize/parse round trip.
#[test]
fn preform_table_y_offset_round_trips_through_toml_text() {
    let mut native = sample_file();
    native.preform = native.preform.with_y_offset(0.35);
    let parsed = read(&write(&native));
    assert_eq!(parsed.preform.y_offset, 0.35);
}

/// A file written before `y_offset` existed (no such key in `[preform]` at all) must
/// still parse, with `y_offset` defaulting to `0.0`.
#[test]
fn a_preform_table_with_no_y_offset_key_parses_and_defaults_to_zero() {
    let parsed = read(&hand_written(1, BLOCK_PREFORM, ""));
    assert_eq!(parsed.preform.y_offset, 0.0);
}

// --- `NativePreformShape::Planes` round-trips and keeps the Block form unchanged ---

/// The six `[nx, ny, nz, m]` rows of a box, used by the plane-set tests below.
fn box_rows() -> Vec<[f64; 4]> {
    vec![
        [1.0, 0.0, 0.0, 1.2],
        [-1.0, 0.0, 0.0, 1.2],
        [0.0, 1.0, 0.0, 0.9],
        [0.0, -1.0, 0.0, 0.9],
        [0.0, 0.0, 1.0, 1.5],
        [0.0, 0.0, -1.0, 1.5],
    ]
}

/// A plane-set preform must survive a serialize/parse round trip through the document
/// text, written with the `kind = "planes"`, `planes` and `source` keys, and the
/// size fields must come back untouched.
#[test]
fn a_plane_set_preform_round_trips_through_toml_text() {
    let mut native = sample_file();
    native.preform = PreformTable::new(
        NativePreformShape::Planes {
            planes: box_rows(),
            source: "plan 'Aqua'".into(),
        },
        1.2,
        1.25,
        1.8,
    );
    native.version = native.needed_version();
    let text = write(&native);
    assert!(text.contains("kind = \"planes\""), "{text}");
    assert!(text.contains("planes = "), "{text}");
    assert!(text.contains("source = "), "{text}");
    let parsed = read(&text);
    assert_eq!(parsed, native);
    match &parsed.preform.shape {
        NativePreformShape::Planes { planes, source } => {
            assert_eq!(planes, &box_rows());
            assert_eq!(source, "plan 'Aqua'");
        }
        other => panic!("expected a plane set, got {other:?}"),
    }
    assert_eq!(parsed.preform.half_width, 1.2);
    assert_eq!(parsed.preform.length_over_width, 1.25);
    assert_eq!(parsed.preform.depth, 1.8);
    assert_eq!(write(&parsed), text);
}

/// A plane set whose table has no `source` key loads with an empty source.
#[test]
fn a_plane_set_preform_without_a_source_key_loads_with_an_empty_source() {
    let preform = "shape = { kind = \"planes\", planes = [[1.0, 0.0, 0.0, 1.2], \
                   [-1.0, 0.0, 0.0, 1.2], [0.0, 1.0, 0.0, 0.9], [0.0, -1.0, 0.0, 0.9], \
                   [0.0, 0.0, 1.0, 1.5], [0.0, 0.0, -1.0, 1.5]] }\n\
                   half_width = 1.2\nlength_over_width = 1.25\ndepth = 1.8";
    let parsed = read(&hand_written(4, preform, ""));
    match &parsed.preform.shape {
        NativePreformShape::Planes { planes, source } => {
            assert_eq!(planes, &box_rows());
            assert_eq!(source, "");
        }
        other => panic!("expected a plane set, got {other:?}"),
    }
}

/// The Block form is unchanged by the new kind: same key, no `planes` or `source`.
#[test]
fn the_block_preform_text_is_unchanged_by_the_plane_set_kind() {
    let text = write(&sample_file());
    assert!(text.contains("kind = \"block\""), "{text}");
    assert!(!text.contains("planes"), "{text}");
    assert_eq!(read(&text).preform.shape, NativePreformShape::Block);
}

// --- `MaterialTable::custom` round-trips through TOML text ---

/// The custom snapshot used by most of the tests below.
fn trigonal_snapshot() -> CustomMaterialSnapshot {
    CustomMaterialSnapshot::new(
        1.62,
        0.017,
        -0.021,
        Some(3.06),
        "Trigonal",
        "UniaxialNegative",
    )
}

/// A [`CustomMaterialSnapshot`] attached via [`MaterialTable::with_custom`] must
/// survive a serialize/parse round trip untouched.
#[test]
fn material_table_custom_snapshot_round_trips_through_toml_text() {
    let mut native = sample_file();
    let snapshot = trigonal_snapshot();
    native.material = native.material.with_custom(Some(snapshot.clone()));
    let parsed = read(&write(&native));
    assert_eq!(parsed.material.custom, Some(snapshot));
}

/// A body color attached via [`CustomMaterialSnapshot::with_body_color`] must
/// survive a TOML text round trip with the identical `f32` bits, written as the
/// shortest decimal of each component (`0.2`, not `0.20000000298023224`).
#[test]
fn custom_snapshot_body_color_round_trips_through_toml_text() {
    let mut native = sample_file();
    let color = [0.2_f32, 0.6, 1.8];
    let snapshot = trigonal_snapshot().with_body_color(Some(color));
    native.material = native.material.with_custom(Some(snapshot.clone()));
    let text = write(&native);
    let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    assert!(compact.contains("absorption_rgb=[0.2,0.6,1.8"), "{text}");
    let custom = read(&text)
        .material
        .custom
        .expect("custom snapshot loads back");
    assert_eq!(custom, snapshot);
    let got = custom.body_color().expect("body color loads back");
    assert_eq!(got.map(f32::to_bits), color.map(f32::to_bits));
}

/// A custom snapshot with no body color must not write an `absorption_rgb` key,
/// and must load back as colorless.
#[test]
fn custom_snapshot_without_body_color_writes_no_absorption_rgb_key() {
    let mut native = sample_file();
    native.material = native.material.with_custom(Some(trigonal_snapshot()));
    let text = write(&native);
    assert!(!text.contains("absorption_rgb"), "{text}");
    let custom = read(&text)
        .material
        .custom
        .expect("custom snapshot loads back");
    assert_eq!(custom.body_color(), None);
}

/// Each dispersion model kind survives a TOML text round trip exactly, written as a
/// `[material.custom.dispersion_model]` table with a `kind` key.
#[test]
fn custom_snapshot_dispersion_model_round_trips_through_toml_text() {
    let models = [
        (
            "sellmeier1",
            DispersionModelDto::Sellmeier1 {
                b1: 1.039_612_2,
                c1: 0.006_000_699,
            },
        ),
        (
            "sellmeier3",
            DispersionModelDto::Sellmeier3 {
                b: [1.039_612_2, 0.231_792_35, 1.010_469_4],
                c: [0.006_000_699, 0.020_017_914, 103.56065],
            },
        ),
        (
            "cauchy",
            DispersionModelDto::Cauchy {
                a: 1.7,
                b: 0.006,
                c: -0.00001,
            },
        ),
    ];
    for (kind, model) in models {
        let mut native = sample_file();
        let snapshot = CustomMaterialSnapshot::new(1.7, 0.02, 0.0, None, "Cubic", "Isotropic")
            .with_dispersion_model(Some(model.clone()));
        native.material = native.material.with_custom(Some(snapshot.clone()));
        let text = write(&native);
        assert!(
            text.contains("[material.custom.dispersion_model]"),
            "{text}"
        );
        assert!(text.contains(&format!("kind = \"{kind}\"")), "{text}");
        let custom = read(&text)
            .material
            .custom
            .expect("custom snapshot loads back");
        assert_eq!(custom.dispersion_model, Some(model), "{kind}");
        assert_eq!(custom, snapshot, "{kind}");
        assert!(
            custom.unknown.is_empty(),
            "the table is not left in unknown"
        );
    }
}

/// A snapshot without a model writes no `dispersion_model` key at all (so every file made
/// before the field existed keeps its bytes) and a file without one loads with `None`.
#[test]
fn custom_snapshot_without_a_dispersion_model_writes_no_key() {
    let mut native = sample_file();
    let snapshot =
        CustomMaterialSnapshot::new(1.62, 0.017, -0.021, None, "Trigonal", "UniaxialNegative");
    native.material = native.material.with_custom(Some(snapshot));
    let text = write(&native);
    assert!(!text.contains("dispersion_model"), "{text}");
    let custom = read(&text)
        .material
        .custom
        .expect("custom snapshot loads back");
    assert_eq!(custom.dispersion_model, None);
}

/// A hand-written table with whole numbers loads; one this build cannot read (a model kind
/// from a newer build, or a missing coefficient) loads as `None` without failing the file.
#[test]
fn an_unreadable_dispersion_model_loads_as_none_and_a_hand_written_one_loads() {
    let mut native = sample_file();
    native.material = native
        .material
        .with_custom(Some(CustomMaterialSnapshot::new(
            1.62,
            0.017,
            0.0,
            None,
            "Cubic",
            "Isotropic",
        )));
    let base = write(&native);

    let by_hand = format!(
        "{base}\n[material.custom.dispersion_model]\nkind = \"sellmeier1\"\nb1 = 1\nc1 = 0.01\n"
    );
    let custom = read(&by_hand)
        .material
        .custom
        .expect("custom snapshot loads back");
    assert_eq!(
        custom.dispersion_model,
        Some(DispersionModelDto::Sellmeier1 { b1: 1.0, c1: 0.01 })
    );

    for bad in [
        "kind = \"tabulated\"\nsamples = [1.5, 1.6]\n",
        "kind = \"cauchy\"\na = 1.7\n",
        "kind = \"sellmeier3\"\nb = [1.0, 2.0]\nc = [0.1, 0.2, 0.3]\n",
    ] {
        let text = format!("{base}\n[material.custom.dispersion_model]\n{bad}");
        let custom = read(&text)
            .material
            .custom
            .expect("custom snapshot loads back");
        assert_eq!(custom.dispersion_model, None, "{bad}");
        assert_eq!(custom.mean_ri, 1.62, "the plain numbers are still there");
    }
}

/// A material with no custom snapshot must not even write a `[material.custom]`
/// table, and must still load back as `None`.
#[test]
fn material_table_with_no_custom_snapshot_omits_the_table() {
    let text = write(&sample_file());
    assert!(!text.contains("[material.custom]"));
    assert_eq!(read(&text).material.custom, None);
}

// --- `MaterialTable::body_color_override` round-trips through TOML text ---

/// A body-color override attached via [`MaterialTable::with_body_color_override`]
/// must survive a serialize/parse round trip bit for bit, written as a plain
/// `body_color_override = [r, g, b]` array under `[material]`.
#[test]
fn material_table_body_color_override_round_trips_through_toml_text() {
    let mut native = sample_file();
    // `f64` on the wire (TOML's only float); `indicatrix-cut-core` converts the
    // editor's `f32` triple to these shortest-decimal values -- see its
    // `native::convert::body_color_to_table`.
    let yellow = [0.2f64, 0.4, 2.8];
    native.material = native.material.with_body_color_override(Some(yellow));
    let text = write(&native);
    // The TOML writer spreads the array over several lines with a trailing comma;
    // compare with all whitespace removed so only the digits matter.
    let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    assert!(
        compact.contains("body_color_override=[0.2,0.4,2.8"),
        "{text}"
    );
    let parsed = read(&text);
    let got = parsed.material.body_color_override.expect("must load back");
    assert_eq!(got.map(f64::to_bits), yellow.map(f64::to_bits));
    assert_eq!(parsed, native);
}

/// No override: the key is not written at all, and a file without it (every file
/// written before the field existed) loads back as `None`.
#[test]
fn material_table_without_a_body_color_override_omits_the_key_and_loads_as_none() {
    let text = write(&sample_file());
    assert!(!text.contains("body_color_override"));
    assert_eq!(read(&text).material.body_color_override, None);
}

/// The N-band color of a design (bands + path scale) is written next to the triple, round
/// trips bit for bit, and a file without the keys loads as `None` with no key written.
#[test]
fn material_table_body_color_bands_round_trip_next_to_the_triple() {
    let mut native = sample_file();
    let rows = vec![[460.0f64, 45.0, 0.25], [620.0, 45.0, 0.5]];
    native.material = native
        .material
        .with_body_color_override(Some([0.2, 0.4, 2.8]))
        .with_body_color_bands_override(Some(rows.clone()), Some(24.5));
    let text = write(&native);
    let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    assert!(
        compact.contains("body_color_override=[0.2,0.4,2.8"),
        "{text}"
    );
    assert!(
        compact.contains("absorption_path_scale_override=24.5"),
        "{text}"
    );
    let parsed = read(&text);
    assert_eq!(parsed.material.body_color_bands_override, Some(rows));
    assert_eq!(parsed.material.absorption_path_scale_override, Some(24.5));
    assert_eq!(parsed, native);

    let plain = write(&sample_file());
    assert!(!plain.contains("body_color_bands_override"), "{plain}");
    assert!(!plain.contains("absorption_path_scale_override"), "{plain}");
    let old = read(&plain);
    assert_eq!(old.material.body_color_bands_override, None);
    assert_eq!(old.material.absorption_path_scale_override, None);
}

/// A custom snapshot's `absorption_bands_per_mm` sits next to `absorption_rgb`, round trips
/// with the identical `f32` bits, and an old snapshot (no key) loads with `None`.
#[test]
fn custom_snapshot_absorption_bands_round_trip_next_to_the_triple() {
    let mut native = sample_file();
    let rows = [[460.0_f32, 45.0, 0.2], [620.0, 45.0, 0.6]];
    let snapshot = trigonal_snapshot()
        .with_body_color(Some([0.2, 0.6, 1.8]))
        .with_absorption_bands(&rows);
    native.material = native.material.with_custom(Some(snapshot.clone()));
    let text = write(&native);
    assert!(text.contains("absorption_bands_per_mm"), "{text}");
    assert!(text.contains("absorption_rgb"), "{text}");
    let custom = read(&text)
        .material
        .custom
        .expect("custom snapshot loads back");
    assert_eq!(custom, snapshot);
    let got = custom.absorption_bands().expect("bands load back");
    assert_eq!(got.len(), 2);
    for (g, r) in got.iter().zip(rows) {
        assert_eq!(g.map(f32::to_bits), r.map(f32::to_bits));
    }

    // An old snapshot (triple only) loads with no bands and writes no key.
    let old =
        CustomMaterialSnapshot::new(1.62, 0.017, -0.021, None, "Trigonal", "UniaxialNegative")
            .with_body_color(Some([0.2, 0.6, 1.8]));
    native.material = native.material.with_custom(Some(old));
    let text = write(&native);
    assert!(!text.contains("absorption_bands_per_mm"), "{text}");
    let custom = read(&text)
        .material
        .custom
        .expect("custom snapshot loads back");
    assert_eq!(custom.absorption_bands(), None);
    assert!(custom.body_color().is_some());
}

/// A biaxial material's `biaxial_delta_beta_alpha` is written, round trips through the
/// document, and a snapshot built without it (every older file) writes no such key.
#[test]
fn custom_snapshot_biaxial_delta_round_trips_and_is_omitted_when_unset() {
    let mut native = sample_file();
    let snapshot = CustomMaterialSnapshot::new(
        1.70,
        0.02,
        0.009,
        Some(3.35),
        "Orthorhombic",
        "BiaxialPositive",
    )
    .with_biaxial_delta(Some(0.004));
    native.material = native.material.with_custom(Some(snapshot.clone()));
    let text = write(&native);
    let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    assert!(compact.contains("biaxial_delta_beta_alpha=0.004"), "{text}");
    let custom = read(&text)
        .material
        .custom
        .expect("custom snapshot loads back");
    assert_eq!(custom, snapshot);
    assert_eq!(custom.biaxial_delta_beta_alpha, Some(0.004));

    // Without the delta no key is written, and the file loads back with `None`.
    let old =
        CustomMaterialSnapshot::new(1.62, 0.017, -0.021, None, "Trigonal", "UniaxialNegative");
    native.material = native.material.with_custom(Some(old));
    let text = write(&native);
    assert!(!text.contains("biaxial_delta_beta_alpha"), "{text}");
    let custom = read(&text)
        .material
        .custom
        .expect("custom snapshot loads back");
    assert_eq!(custom.biaxial_delta_beta_alpha, None);
}

/// `sample_file()`'s text with `line` added as the first line of its `[material]` table.
fn sample_text_with_material_line(line: &str) -> String {
    let text = write(&sample_file());
    let marker = "[material]\n";
    assert!(text.contains(marker), "{text}");
    text.replacen(marker, &format!("{marker}{line}\n"), 1)
}

/// Builds from 2026-09-28 wrote the key as `body_colour_override`. Such a file must load
/// the override, keep nothing of the old key in the carry-over table, and write
/// the current spelling exactly once when saved again.
#[test]
fn the_old_british_body_colour_key_loads_and_is_saved_under_the_new_spelling() {
    let old = sample_text_with_material_line("body_colour_override = [0.2, 0.4, 2.8]");
    let parsed = read(&old);
    let got = parsed.material.body_color_override.expect("override loads");
    assert_eq!(got.map(f64::to_bits), [0.2f64, 0.4, 2.8].map(f64::to_bits));
    assert!(
        parsed.material.unknown.is_empty(),
        "the old key must not linger in the carry-over table: {:?}",
        parsed.material.unknown
    );

    let saved = write(&parsed);
    assert_eq!(saved.matches("body_color_override").count(), 1, "{saved}");
    assert!(!saved.contains("body_colour"), "{saved}");
    // The re-saved text is exactly what a build writes for the same value.
    let mut fresh = sample_file();
    fresh.material = fresh
        .material
        .with_body_color_override(Some([0.2, 0.4, 2.8]));
    assert_eq!(saved, write(&fresh));
}

/// A file a campaign build re-saved after the respelling can hold both keys. It must
/// still open (a plain serde alias would refuse it), the new key wins, and the old one is
/// gone after the next save.
#[test]
fn a_file_with_both_body_colour_spellings_loads_with_the_new_one_winning() {
    let both = sample_text_with_material_line(
        "body_colour_override = [9.0, 9.0, 9.0]\nbody_color_override = [0.2, 0.4, 2.8]",
    );
    let parsed = read(&both);
    assert_eq!(parsed.material.body_color_override, Some([0.2, 0.4, 2.8]));
    assert!(parsed.material.unknown.is_empty());
    let saved = write(&parsed);
    assert_eq!(saved.matches("body_color_override").count(), 1, "{saved}");
    assert!(!saved.contains("body_colour"), "{saved}");
}

/// An old-spelling key that is not a color triple is not applied and not lost: it is
/// carried over untouched like any other key this build cannot read.
#[test]
fn a_malformed_old_body_colour_key_is_carried_over_untouched() {
    let odd = sample_text_with_material_line("body_colour_override = \"amber\"");
    let parsed = read(&odd);
    assert_eq!(parsed.material.body_color_override, None);
    assert_eq!(
        parsed
            .material
            .unknown
            .get("body_colour_override")
            .and_then(toml::Value::as_str),
        Some("amber")
    );
    let saved = write(&parsed);
    assert!(
        saved.contains("body_colour_override = \"amber\""),
        "{saved}"
    );
}

// --- `DesignFile::history` round-trips through TOML text ---

/// A [`HistoryTable`] attached via [`DesignFile::with_history`] must survive a
/// serialize/parse round trip untouched.
#[test]
fn history_table_round_trips_through_toml_text() {
    let native = sample_file().with_history(HistoryTable::new(vec![
        "Set material to Diamond".to_string(),
        "Remove tier C1".to_string(),
    ]));
    let parsed = read(&write(&native));
    assert_eq!(
        parsed.history.map(|h| h.entries),
        Some(vec![
            "Set material to Diamond".to_string(),
            "Remove tier C1".to_string(),
        ])
    );
}

/// An empty history must not grow a `[history]` table at all.
#[test]
fn an_empty_history_table_is_not_attached_at_all() {
    let native = sample_file().with_history(HistoryTable::new(Vec::new()));
    assert!(native.history.is_none());
}

// --- Every `NativeMeetConstraint` variant round-trips through TOML text with
// exact `f64` equality ---

/// Every [`NativeMeetConstraint`] variant must survive a serialize/parse round trip
/// as an EXACT `f64`, not just approximately equal.
///
/// In particular [`NativeMeetConstraint::ScaleReference`]'s `mast`, TOML's only
/// lossy-prone field here, must come back bit-for-bit identical, and
/// [`NativeMeetConstraint::MeetNamed`]'s name list must come back in the same order
/// with no name altered.
#[test]
fn every_native_meet_constraint_variant_round_trips_through_toml_text_exactly() {
    let mast = 0.123_456_789_012_345;
    let native = DesignFile::new(
        PreformTable::new(NativePreformShape::Block, 1.0, 1.0, 2.0),
        MaterialTable::new(Some("Diamond".to_string()), None, Some(1.62)),
        schedule(),
        Some(6.5),
        vec![
            tier("Existing", NativeMeetConstraint::MeetExisting),
            tier(
                "Named",
                NativeMeetConstraint::MeetNamed {
                    names: vec!["Crown Main".to_string(), "Star".to_string()],
                },
            ),
            tier("Scaled", NativeMeetConstraint::ScaleReference { mast }),
        ],
    );
    let parsed = read(&write(&native));

    assert_eq!(
        parsed.tiers[0].constraint,
        NativeMeetConstraint::MeetExisting
    );
    assert_eq!(
        parsed.tiers[1].constraint,
        NativeMeetConstraint::MeetNamed {
            names: vec!["Crown Main".to_string(), "Star".to_string()],
        }
    );
    match &parsed.tiers[2].constraint {
        NativeMeetConstraint::ScaleReference { mast: parsed_mast } => {
            assert_eq!(
                parsed_mast.to_bits(),
                mast.to_bits(),
                "mast must survive the round trip bit-for-bit, got {parsed_mast} vs {mast}"
            );
        }
        other => panic!("expected ScaleReference, got {other:?}"),
    }
    // The whole struct, not just the constraint field -- confirms nothing else on
    // the tier was disturbed by exercising every variant in one document.
    assert_eq!(native, parsed);
}

// --- Stable tier ids and authoring-level tier targets ---

/// A [`TierTable::tier_id`] attached via [`TierTable::with_tier_id`] must survive
/// a serialize/parse round trip untouched.
#[test]
fn tier_id_round_trips_through_toml_text() {
    let mut native = sample_file();
    native.tiers = vec![
        tier("A", NativeMeetConstraint::MeetExisting).with_tier_id(Some(0)),
        tier("B", NativeMeetConstraint::MeetExisting).with_tier_id(Some(1)),
    ];
    let parsed = read(&write(&native));
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
    let extra = "\n[[tiers]]\nname = \"T2\"\n\
                 constraint = { kind = \"scale_reference\", mast = 0.5 }\n\
                 angle_deg = 0.0\nindices = []\n";
    let parsed = read(&hand_written(1, BLOCK_PREFORM, extra));
    assert_eq!(parsed.tiers.len(), 2);
    assert_eq!(parsed.tiers[0].tier_id, None);
    assert_eq!(parsed.tiers[1].tier_id, None);
    // The round trip must not silently invent one either.
    let reparsed = read(&write(&parsed));
    assert_eq!(reparsed.tiers[0].tier_id, None);
}

/// A [`NativeTierTarget`] attached via [`TierTable::with_target`] must survive a
/// serialize/parse round trip untouched, for all three variants.
#[test]
fn tier_target_round_trips_through_toml_text() {
    let mut native = sample_file();
    native.tiers = vec![
        tier("Pavilion Main", NativeMeetConstraint::MeetExisting)
            .with_target(Some(NativeTierTarget::DepthMm { mm: 3.20 })),
        tier("Girdle", NativeMeetConstraint::MeetExisting)
            .with_target(Some(NativeTierTarget::GirdleThicknessMm { mm: 0.25 })),
        tier("Table", NativeMeetConstraint::MeetExisting)
            .with_target(Some(NativeTierTarget::TableWidthMm { mm: 4.10 })),
    ];
    let parsed = read(&write(&native));
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

/// The SHA-256 helper the attachments use gives the known digest of the empty input
/// and of `abc` (lowercase hex).
#[test]
fn sha256_hex_matches_the_known_digests() {
    assert_eq!(
        sha256_hex(b""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(
        sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}
