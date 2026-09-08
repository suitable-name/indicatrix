//! `filename_template` tests: every recognised variable, sanitising, extension
//! handling, and collision-avoidance. Moved out of `mod.rs` to keep that file from
//! growing further.

use super::*;

/// Every recognised template variable name, alphabetised, so
/// `every_documented_variable_is_recognised` below can walk it. [`substitute`]'s
/// `match` is still the source of truth for behaviour.
const KNOWN_VARIABLES: &[&str] = &[
    "bounces",
    "colorspace",
    "date",
    "design",
    "designer",
    "distance",
    "exposure",
    "height",
    "lighting",
    "material",
    "pitch",
    "preset",
    "ri",
    "shape",
    "spp",
    "time",
    "timestamp",
    "width",
    "yaw",
];

fn ctx() -> TemplateContext {
    TemplateContext {
        design: "Solitaire Round".to_string(),
        designer: "Marcel Tolkowsky".to_string(),
        shape: "Round".to_string(),
        material: "Diamond".to_string(),
        ri: "2.417".to_string(),
        width: 1920,
        height: 1080,
        spp: 256,
        bounces: 12,
        colorspace: "sRGB".to_string(),
        preset: String::new(),
        lighting: "Gem Studio Ring Lights".to_string(),
        yaw_deg: 34.377_2,
        pitch_deg: 25.78,
        distance: 2.4,
        exposure: 1.0,
    }
}

// A fixed instant (2024-03-05 06:07:08 UTC), used by every test below that touches
// `{date}`/`{time}`/`{timestamp}` so they're deterministic regardless of when the
// test suite runs.
fn fixed_now() -> SystemTime {
    SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_709_618_828)
}

// ---- civil_from_unix_seconds -----------------------------------------------------

#[test]
fn civil_from_unix_seconds_pins_the_epoch() {
    assert_eq!(civil_from_unix_seconds(0), (1970, 1, 1, 0, 0, 0));
}

#[test]
fn civil_from_unix_seconds_pins_a_leap_day() {
    // 2024-02-29 12:00:00 UTC.
    assert_eq!(
        civil_from_unix_seconds(1_709_208_000),
        (2024, 2, 29, 12, 0, 0)
    );
}

#[test]
fn civil_from_unix_seconds_pins_the_fixed_test_instant() {
    assert_eq!(
        civil_from_unix_seconds(1_709_618_828),
        (2024, 3, 5, 6, 7, 8)
    );
}

// ---- render: every variable -------------------------------------------------------

#[test]
fn every_documented_variable_is_recognised() {
    for name in KNOWN_VARIABLES {
        let template = format!("{{{name}}}");
        let rendered = render(&template, &ctx(), fixed_now());
        assert!(
            !rendered.contains('{') && !rendered.contains('}'),
            "{{{name}}} must substitute to a real value, got {rendered:?}"
        );
    }
}

#[test]
fn render_substitutes_string_variables_verbatim() {
    let rendered = render("{design}-{designer}-{shape}-{ri}", &ctx(), fixed_now());
    assert_eq!(rendered, "Solitaire Round-Marcel Tolkowsky-Round-2.417");
}

#[test]
fn render_substitutes_numeric_variables() {
    let rendered = render("{width}x{height}_{spp}spp_{bounces}b", &ctx(), fixed_now());
    assert_eq!(rendered, "1920x1080_256spp_12b");
}

#[test]
fn render_formats_angles_and_floats_to_a_fixed_precision() {
    let rendered = render("{yaw}_{pitch}_{distance}_{exposure}", &ctx(), fixed_now());
    assert_eq!(rendered, "34.4_25.8_2.40_1.00");
}

#[test]
fn render_formats_date_time_and_timestamp() {
    let rendered = render("{date}_{time}_{timestamp}", &ctx(), fixed_now());
    assert_eq!(rendered, "2024-03-05_06-07-08_1709618828");
}

#[test]
fn render_substitutes_preset_and_lighting() {
    let mut with_preset = ctx();
    with_preset.preset = "Dramatic Spotlight".to_string();
    let rendered = render("{preset}_{lighting}", &with_preset, fixed_now());
    assert_eq!(rendered, "Dramatic Spotlight_Gem Studio Ring Lights");
}

#[test]
fn render_substitutes_colorspace() {
    assert_eq!(
        render("{colorspace}", &ctx(), fixed_now()),
        "sRGB".to_string()
    );
}

// ---- render: unknown/malformed variables ------------------------------------------

#[test]
fn unknown_variables_pass_through_literally() {
    let rendered = render("{material}_{totallyMadeUp}", &ctx(), fixed_now());
    assert_eq!(rendered, "Diamond_{totallyMadeUp}");
}

#[test]
fn an_unterminated_brace_is_emitted_literally_rather_than_panicking() {
    let rendered = render("{material}_{oops", &ctx(), fixed_now());
    assert_eq!(rendered, "Diamond_{oops");
}

#[test]
fn empty_braces_pass_through_literally() {
    let rendered = render("a{}b", &ctx(), fixed_now());
    assert_eq!(rendered, "a{}b");
}

// ---- resolve_export_path: sanitising ----------------------------------------------

fn scratch_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "indicatrix-cut-filename-template-test-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create scratch export dir");
    dir
}

#[test]
fn forbidden_characters_in_a_design_title_are_sanitised() {
    let dir = scratch_dir("forbidden-chars");
    let mut c = ctx();
    c.design = "Ashoka® / \"Cut\": v2?*<>|".to_string();
    let path = resolve_export_path(&dir, "{design}", &c);
    let name = path.file_name().unwrap().to_string_lossy().into_owned();
    for forbidden in ['/', '\\', ':', '"', '?', '*', '<', '>', '|'] {
        assert!(
            !name.contains(forbidden),
            "{name:?} must not contain {forbidden:?}"
        );
    }
    assert!(path.starts_with(&dir), "must resolve inside the export dir");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn directory_traversal_via_a_scraped_title_cannot_escape_the_export_dir() {
    let dir = scratch_dir("traversal");
    let mut c = ctx();
    c.design = "../../../etc/passwd".to_string();
    let path = resolve_export_path(&dir, "{design}", &c);
    assert!(
        path.starts_with(&dir),
        "path {path:?} escaped the export directory {dir:?}"
    );
    assert_eq!(
        path.parent(),
        Some(dir.as_path()),
        "path must land directly inside the export directory, not a subdirectory"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_reserved_windows_device_name_is_not_used_verbatim() {
    let dir = scratch_dir("reserved-name");
    let mut c = ctx();
    c.design = "CON".to_string();
    let path = resolve_export_path(&dir, "{design}", &c);
    let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
    assert_ne!(stem.to_ascii_uppercase(), "CON");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn trailing_dots_and_spaces_are_trimmed() {
    let dir = scratch_dir("trailing-dots");
    let mut c = ctx();
    c.design = "Trailing Title. ".to_string();
    let path = resolve_export_path(&dir, "{design}", &c);
    let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
    assert_eq!(stem, "Trailing Title");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The DEFAULT template includes `{material}`, and a material name with a space must
/// survive it, not just the exotic scraped-`{design}` case above.
#[test]
fn a_material_name_with_a_space_survives_the_default_template() {
    let dir = scratch_dir("material-space");
    let mut c = ctx();
    c.material = "Cubic Zirconia".to_string();
    let path = resolve_export_path(&dir, DEFAULT_TEMPLATE, &c);
    let name = path.file_name().unwrap().to_string_lossy().into_owned();
    assert!(
        name.contains("Cubic Zirconia"),
        "expected the material name preserved verbatim (spaces are valid filename \
             characters) in {name:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A free-typed custom material name is exactly as untrusted as a scraped design
/// title, so it must go through the same sanitising pass.
#[test]
fn a_custom_material_name_with_forbidden_characters_is_sanitised() {
    let dir = scratch_dir("material-forbidden");
    let mut c = ctx();
    c.material = "My/Weird:Material*Name".to_string();
    let path = resolve_export_path(&dir, DEFAULT_TEMPLATE, &c);
    let name = path.file_name().unwrap().to_string_lossy().into_owned();
    for forbidden in ['/', ':', '*'] {
        assert!(
            !name.contains(forbidden),
            "{name:?} must not contain {forbidden:?}"
        );
    }
    assert!(path.starts_with(&dir));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A rendered result that sanitises down to nothing must still fall back to a valid,
/// non-empty name rather than handing `unique_path` an empty string.
#[test]
fn an_all_dots_template_falls_back_to_a_non_empty_name() {
    let dir = scratch_dir("empty-fallback");
    let mut c = ctx();
    c.design = "...".to_string();
    c.material = String::new();
    let path = resolve_export_path(&dir, "{design}{material}", &c);
    let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
    assert!(!stem.is_empty(), "must never resolve to an empty filename");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_entirely_empty_template_falls_back_to_a_non_empty_name() {
    let dir = scratch_dir("empty-template");
    let mut c = ctx();
    c.design = String::new();
    c.material = String::new();
    let path = resolve_export_path(&dir, "{design}{material}", &c);
    let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
    assert!(!stem.is_empty(), "must never resolve to an empty filename");
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- resolve_export_path: extension -------------------------------------------------

#[test]
fn a_template_missing_an_extension_gets_png_appended() {
    let dir = scratch_dir("missing-ext");
    let path = resolve_export_path(&dir, "{material}", &ctx());
    assert_eq!(path.extension().and_then(|e| e.to_str()), Some("png"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_template_already_ending_in_png_is_not_double_suffixed() {
    let dir = scratch_dir("has-ext");
    let path = resolve_export_path(&dir, "{material}.png", &ctx());
    let name = path.file_name().unwrap().to_string_lossy().into_owned();
    assert!(!name.to_ascii_lowercase().ends_with(".png.png"));
    assert!(name.to_ascii_lowercase().ends_with(".png"));
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- resolve_export_path: collisions -----------------------------------------------

#[test]
fn a_colliding_template_appends_a_numbered_suffix_rather_than_overwriting() {
    let dir = scratch_dir("collisions");
    // A template with no `{timestamp}`, so every call renders the identical base name.
    let template = "{material}_{width}x{height}";

    let first = resolve_export_path(&dir, template, &ctx());
    std::fs::write(&first, b"fake png bytes").expect("write first file");

    let second = resolve_export_path(&dir, template, &ctx());
    assert_ne!(
        first, second,
        "a second export must not collide with the first"
    );
    assert!(
        second
            .file_stem()
            .unwrap()
            .to_string_lossy()
            .ends_with("(2)"),
        "expected a ' (2)' suffix, got {second:?}"
    );
    std::fs::write(&second, b"fake png bytes").expect("write second file");

    let third = resolve_export_path(&dir, template, &ctx());
    assert!(
        third
            .file_stem()
            .unwrap()
            .to_string_lossy()
            .ends_with("(3)"),
        "expected a ' (3)' suffix, got {third:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_default_template_is_unique_enough_that_two_calls_do_not_collide() {
    let dir = scratch_dir("default-template-uniqueness");
    let first = resolve_export_path(&dir, DEFAULT_TEMPLATE, &ctx());
    std::fs::write(&first, b"fake png bytes").expect("write first file");
    let second = resolve_export_path(&dir, DEFAULT_TEMPLATE, &ctx());
    // Not asserting `first != second` (two calls in the same wall-clock second would
    // collide on `{timestamp}` alone) -- the actual guarantee is that the second call
    // still resolves to a path that doesn't already exist.
    assert!(!second.exists());
    let _ = std::fs::remove_dir_all(&dir);
}
