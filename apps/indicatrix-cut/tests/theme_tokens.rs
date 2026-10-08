//! Guards the colour rules of the Slint UI (`ui/`; see `ui/README.md`, "Colours and high
//! contrast"):
//!
//! - every colour token in `theme.slint` has a normal and a high-contrast value, except the
//!   few data colours that are deliberately the same in both palettes;
//! - the normal value of each token added for the high-contrast pass is the literal the
//!   code used before, so the normal look did not move;
//! - the high-contrast pairs those tokens form (text on its fill, ink on an accent, a series
//!   colour on its pill) meet their WCAG contrast ratios;
//! - the files converted in that pass hold no hex literal, `white`, `black`, `rgb()` or
//!   `hsv()`, so a new one cannot slip back in unseen.
//!
//! It only reads the `.slint` sources as text.

use std::{collections::BTreeMap, fs, path::Path};

/// Colour tokens whose value is deliberately the same in both palettes: white on a dark
/// fill, and the data colours (a chart series, the Compare overlay's red and green).
const CONSTANT_TOKENS: &[&str] = &[
    "text-on-primary",
    "chart-extinction",
    "compare-removed",
    "compare-added",
];

/// Files (relative to `ui/`) that were moved onto tokens and must stay free of colour
/// literals. Files still waiting for their own conversion are not listed.
const CONVERTED_FILES: &[&str] = &[
    "components/export_dialog/compute_and_bounces.slint",
    "components/export_dialog/output_location.slint",
    "components/export_dialog/samples_and_presets.slint",
    "components/performance_graph/tilt_curve_chart.slint",
    "components/performance_graph/tilt_video_export_section.slint",
    "components/performance_graph_dialog.slint",
    "components/gem_viewport.slint",
    "components/import_dialog.slint",
    "components/remote_worker_dialog.slint",
    "components/filter_panel.slint",
    "components/diagram_list.slint",
    "components/detail_header.slint",
    "components/compare_view.slint",
    "components/header.slint",
    // Viewport, toolbars, hint bar, manipulate overlay, render settings dialog and the shared
    // buttons they are built from.
    "components/pill_button.slint",
    "components/toggle_switch.slint",
    "components/icon_button.slint",
    "components/chip_toggle.slint",
    "components/export_chip.slint",
    "components/gem_viewport/live_cut_badge.slint",
    "components/gem_viewport/render_color_picker.slint",
    "components/gem_viewport/toolbar_actions.slint",
    "components/solid_viewport.slint",
    "components/solid_viewport/hint_bar.slint",
    "components/solid_viewport/manipulate_overlay.slint",
    "components/solid_viewport/toolbar.slint",
    "components/settings_dialog.slint",
    "components/settings_dialog/bounces_picker.slint",
    "components/settings_dialog/crystal_axis_section.slint",
    "components/settings_dialog/design_lighting_section.slint",
    "components/settings_dialog/environment_map_section.slint",
    "components/settings_dialog/lighting_presets_section.slint",
    "components/settings_dialog/live_compute_picker.slint",
    "components/settings_dialog/local_compute_picker.slint",
    "components/settings_dialog/motion_preview_picker.slint",
    "components/settings_dialog/performance_curve_panel.slint",
    "components/settings_dialog/resolution_picker.slint",
    "components/settings_dialog/setting_label.slint",
    // Inspector (every tab), tier table and the compact buttons they use.
    "components/compact_controls.slint",
    "components/advanced_note.slint",
    "components/editor_inspector.slint",
    "components/editor_inspector/concave_form_tab.slint",
    "components/editor_inspector/form_row.slint",
    "components/editor_inspector/history_tab.slint",
    "components/editor_inspector/optimize_tab.slint",
    "components/editor_inspector/preform_tab.slint",
    "components/editor_inspector/schedule_tab.slint",
    "components/editor_inspector/tier_form_tab.slint",
    "components/editor_inspector/variants_section.slint",
    "components/editor_tier_table.slint",
    "components/editor_tier_table/tier_angle_cell.slint",
    "components/editor_tier_table/tier_table_header.slint",
    "components/editor_tier_table/tier_table_row.slint",
    "components/editor_tier_table/tier_table_toolbar.slint",
    // Command bar, Design settings panel, status strip with its verdict badge and popover, empty
    // state, the library's cutting table, and the shared buttons they are built from.
    "components/touch_button.slint",
    "components/editor_command_bar.slint",
    "components/editor_design_settings.slint",
    "components/editor_status_strip.slint",
    "components/verdict_badge.slint",
    "components/verdict_popover.slint",
    "components/empty_state.slint",
    "components/app/viewport_overlay.slint",
    "components/export_split_button.slint",
    "components/section_header.slint",
    "components/stale_badge.slint",
    "components/material_guess.slint",
    "components/cutting_table.slint",
    // Every other dialog and window: New Design, Retarget, the Compare window, the material
    // editor, the export, import and remote worker dialogs, Preferences, the command palette, the
    // help windows, the tutorial browser and welcome, Edit as Text, Angle Sweep, cutting mode, the
    // Rough Planner, and the two small shared pieces (`TipBox`, `HelpButton`) they use.
    "components/tip_box.slint",
    "components/help_button.slint",
    "components/confirm_action_dialog.slint",
    "components/metadata_editor_dialog.slint",
    "components/anchor_explainer.slint",
    "components/template_gallery.slint",
    "components/new_design_dialog.slint",
    "components/preview_batch_dialog.slint",
    "components/tilt_batch_dialog.slint",
    "components/export_dialog.slint",
    "components/retarget_dialog.slint",
    "compare_window.slint",
    "components/compare_metrics_strip.slint",
    "components/material_editor_dialog.slint",
    "components/material_editor/optics_sliders.slint",
    "components/material_editor/crystal_classification.slint",
    "components/material_editor/template_selector.slint",
    "components/material_editor/dispersion_coefficients.slint",
    "components/material_editor/chromophore_picker.slint",
    "components/preferences_dialog.slint",
    "components/command_palette.slint",
    "components/shortcut_overlay.slint",
    "components/help_window.slint",
    "components/glossary_dialog.slint",
    "components/welcome_dialog.slint",
    "components/tutorial_browser.slint",
    "components/guide_panel.slint",
    "components/raw_text_dialog.slint",
    "components/toast.slint",
    "components/sweep_dialog.slint",
    "components/sweep_dialog/sweep_button.slint",
    "components/sweep_dialog/sweep_form.slint",
    "components/sweep_dialog/sweep_chart.slint",
    "components/sweep_dialog/sweep_table.slint",
    "components/sweep_dialog/sweep_style.slint",
    "components/cutting_mode/cm_button.slint",
    "components/cutting_mode/cutting_mode_screen.slint",
    "components/cutting_mode/index_chips.slint",
    "components/cutting_mode/index_wheel.slint",
    "rough_planner_window.slint",
    "components/rough_planner/cut_list.slint",
    "components/rough_planner/design_group_row.slint",
    "components/rough_planner/fields.slint",
    "components/rough_planner/fit_view.slint",
    "components/rough_planner/plan_form.slint",
    "components/rough_planner/result_card.slint",
    "components/rough_planner/rough_form.slint",
    "components/rough_planner/save_name_row.slint",
    "components/rough_planner/saved_list.slint",
];

/// Converted files that still hold colours which are DATA, not chrome: the body colours a
/// swatch stands for, the saturation/value square and the hue strip of the colour picker. They
/// look the same in every palette on purpose. Each entry lists the only literals the file may
/// hold; any other one is an offender like in [`CONVERTED_FILES`].
const CONVERTED_WITH_DATA: &[(&str, &[&str])] = &[
    (
        "components/material_editor/color_presets.slint",
        &[
            "#1e40af", "#1d4ed8", "#be123c", "#e11d48", "#047857", "#059669", "#6b21a8", "#7c3aed",
            "#b45309", "#d97706", "#be185d", "#db2777", "#0891b2", "#06b6d4", "#f59e0b",
        ],
    ),
    (
        "components/material_editor/color_picker.slint",
        &[
            "#c0392b",
            "hsv(",
            "#ffffff",
            "#ffffff00",
            "#00000000",
            "#000000",
            "#ff0000",
            "#ffff00",
            "#00ff00",
            "#00ffff",
            "#0000ff",
            "#ff00ff",
        ],
    ),
];

/// The normal-palette value each token added for the high-contrast pass must keep: the
/// literal that stood in the code before.
const NORMAL_PINS: &[(&str, &str)] = &[
    ("surface-chip", "#131b2e"),
    ("surface-chip-off", "#334155"),
    ("tint-cyan", "#0284c733"),
    ("tint-amber", "#d9770633"),
    ("tint-red", "#b91c1c33"),
    ("tint-purple", "#a855f733"),
    ("tint-neutral", "#33415577"),
    ("surface-graph", "#0f1523"),
    ("surface-graph-popup", "#0f1523ee"),
    ("chart-bg", "#050811"),
    ("chart-border", "#1e293b"),
    ("chart-grid", "#1e293b44"),
    ("chart-grid-mid", "#1e293b66"),
    ("chart-guide", "#33415577"),
    ("chart-crosshair", "#94a3b877"),
    ("chart-extinction", "#ef4444"),
    ("notice-surface", "#7c2d1233"),
    ("notice-accent", "#ea580c"),
    ("notice-accent-hover", "#f97316"),
    ("notice-text", "#fdba74"),
    ("badge-amber-bg", "#78350f"),
    ("badge-emerald-bg", "#065f46"),
    ("badge-purple-bg", "#4c1d95"),
    ("badge-orange-bg", "#7c2d12"),
    ("badge-orange-text", "#fed7aa"),
    ("viewport-bg", "#080a0f"),
    ("hud-surface", "#0f172aee"),
    ("hint-surface", "#1e293bdd"),
    ("hint-text", "#cbd5e1"),
    ("scrim-light", "#00000055"),
    ("shadow-heavy", "#000000ee"),
    ("shadow-medium", "#000000aa"),
    ("toggle-knob", "#ffffff"),
    ("brand-mark-bg", "#1e3a8a"),
    ("compare-removed", "#ef4444"),
    ("compare-added", "#22c55e"),
    ("swatch-clear", "#e2e8f0"),
    ("swatch-clear-off", "#94a3b844"),
    ("swatch-keep", "#94a3b822"),
];

/// What a translucent surface is laid over when its contrast is measured.
#[derive(Clone, Copy)]
enum Under {
    Black,
    White,
    Token(&'static str),
}

/// One token of `theme.slint` with both of its values as `[red, green, blue, alpha]`, each
/// channel `0.0..=255.0`.
struct Token {
    high_contrast: [f64; 4],
    normal: [f64; 4],
}

fn read_ui(relative: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("ui")
        .join(relative);
    fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()))
}

/// `#rrggbb` or `#rrggbbaa` as channels; a missing alpha is opaque.
fn parse_hex(literal: &str) -> Option<[f64; 4]> {
    let digits = literal.strip_prefix('#')?;
    if !matches!(digits.len(), 6 | 8) {
        return None;
    }
    let mut channels = [255.0; 4];
    for (slot, index) in channels.iter_mut().zip(0..digits.len() / 2) {
        let pair = digits.get(index * 2..index * 2 + 2)?;
        *slot = f64::from(u8::from_str_radix(pair, 16).ok()?);
    }
    Some(channels)
}

/// Every colour token of `theme.slint`, by name. A token that is not a
/// `root.high-contrast ? <hc> : <normal>` pair must be one of [`CONSTANT_TOKENS`].
fn theme_tokens() -> BTreeMap<String, Token> {
    let source = read_ui("theme.slint");
    let mut tokens = BTreeMap::new();
    for line in source.lines() {
        let Some(rest) = line.trim().strip_prefix("out property <color> ") else {
            continue;
        };
        let (name, value) = rest
            .split_once(':')
            .unwrap_or_else(|| panic!("no value in `{line}`"));
        let (name, value) = (name.trim(), value.trim().trim_end_matches(';').trim());
        let parse = |text: &str| {
            parse_hex(text.trim())
                .unwrap_or_else(|| panic!("`{text}` of token `{name}` is not a hex colour"))
        };
        let token = if value.starts_with("root.high-contrast ?") {
            let pair = value.trim_start_matches("root.high-contrast ?");
            let (high_contrast, normal) = pair
                .split_once(" : ")
                .unwrap_or_else(|| panic!("token `{name}` has no normal value"));
            Token {
                high_contrast: parse(high_contrast),
                normal: parse(normal),
            }
        } else {
            assert!(
                CONSTANT_TOKENS.contains(&name),
                "token `{name}` has no high-contrast value: write it as \
                 `root.high-contrast ? <hc> : <normal>`"
            );
            Token {
                high_contrast: parse(value),
                normal: parse(value),
            }
        };
        tokens.insert(name.to_string(), token);
    }
    tokens
}

/// `top` (with its alpha) laid over the opaque colour `under`.
fn over(top: [f64; 4], under: [f64; 3]) -> [f64; 3] {
    let alpha = top[3] / 255.0;
    [0, 1, 2].map(|index| top[index].mul_add(alpha, under[index] * (1.0 - alpha)))
}

/// WCAG relative luminance of an opaque colour.
fn luminance(colour: [f64; 3]) -> f64 {
    let linear = colour.map(|channel| {
        let level = channel / 255.0;
        if level <= 0.04 {
            level / 12.92
        } else {
            ((level + 0.055) / 1.055).powf(2.4)
        }
    });
    0.0722_f64.mul_add(linear[2], 0.7152_f64.mul_add(linear[1], 0.2126 * linear[0]))
}

/// WCAG contrast ratio of two opaque colours, `1.0..=21.0`.
fn contrast(first: [f64; 3], second: [f64; 3]) -> f64 {
    let (one, other) = (luminance(first), luminance(second));
    (one.max(other) + 0.05) / (one.min(other) + 0.05)
}

/// The hex colour literals (`#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`) in `code`.
fn hex_literals(code: &str) -> Vec<String> {
    let bytes = code.as_bytes();
    let mut found = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] != b'#' {
            at += 1;
            continue;
        }
        let start = at + 1;
        let mut end = start;
        while end < bytes.len() && bytes[end].is_ascii_hexdigit() {
            end += 1;
        }
        let ends_cleanly = bytes
            .get(end)
            .is_none_or(|next| !next.is_ascii_alphanumeric() && *next != b'_' && *next != b'-');
        if matches!(end - start, 3 | 4 | 6 | 8) && ends_cleanly {
            found.push(code[at..end].to_string());
        }
        at = end.max(at + 1);
    }
    found
}

/// `line` without its `//` comment and without the contents of its string literals.
fn code_only(line: &str) -> String {
    let mut code = String::new();
    let mut in_string = false;
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' if in_string => {
                chars.next();
            }
            '"' => in_string = !in_string,
            '/' if !in_string && chars.peek() == Some(&'/') => break,
            _ if in_string => {}
            _ => code.push(ch),
        }
    }
    code
}

/// The colour literals and colour keywords in one line of Slint code (comment and strings
/// already removed).
fn colour_literals(code: &str) -> Vec<String> {
    let mut found = hex_literals(code);
    let words = code.split(|ch: char| !(ch.is_ascii_alphanumeric() || "-_.".contains(ch)));
    found.extend(
        words
            .filter(|word| matches!(*word, "white" | "black" | "Colors.white" | "Colors.black"))
            .map(ToString::to_string),
    );
    for call in ["rgb(", "rgba(", "hsv(", "hsva("] {
        if code.contains(call) {
            found.push(call.to_string());
        }
    }
    found
}

#[test]
fn every_colour_token_has_a_high_contrast_value() {
    let tokens = theme_tokens();
    assert!(
        tokens.len() >= 80,
        "only {} colour tokens were read from theme.slint",
        tokens.len()
    );
    for name in CONSTANT_TOKENS {
        let token = tokens
            .get(*name)
            .unwrap_or_else(|| panic!("constant token `{name}` is gone from theme.slint"));
        assert_eq!(
            token.high_contrast, token.normal,
            "`{name}` is a data colour and must be the same in both palettes"
        );
    }
}

#[test]
fn new_tokens_keep_the_normal_value_the_code_used_before() {
    let tokens = theme_tokens();
    for (name, literal) in NORMAL_PINS {
        let token = tokens
            .get(*name)
            .unwrap_or_else(|| panic!("token `{name}` is missing from theme.slint"));
        assert_eq!(
            Some(token.normal),
            parse_hex(literal),
            "the normal value of `{name}` moved away from {literal}"
        );
    }
}

#[test]
fn high_contrast_pairs_meet_their_ratios() {
    use Under::{Black, Token as OnToken, White};
    // (text or border, surface it sits on, what a translucent surface is laid over, least ratio)
    let pairs = [
        ("notice-text", "notice-surface", Black, 7.0),
        ("text-on-accent", "notice-accent", Black, 7.0),
        ("text-on-accent", "notice-accent-hover", Black, 7.0),
        ("notice-accent", "notice-surface", Black, 3.0),
        ("badge-orange-text", "badge-orange-bg", Black, 7.0),
        ("accent-amber", "badge-amber-bg", Black, 4.5),
        ("accent-emerald", "badge-emerald-bg", Black, 4.5),
        ("accent-purple", "badge-purple-bg", Black, 4.5),
        ("chart-extinction", "surface-chip", Black, 4.5),
        ("chart-extinction", "chart-bg", Black, 4.5),
        (
            "chart-extinction",
            "tint-red",
            OnToken("surface-sunken"),
            4.5,
        ),
        ("accent-cyan", "tint-cyan", OnToken("surface-sunken"), 7.0),
        ("accent-amber", "tint-amber", OnToken("surface-sunken"), 7.0),
        (
            "accent-purple",
            "tint-purple",
            OnToken("surface-sunken"),
            4.5,
        ),
        (
            "text-primary",
            "tint-neutral",
            OnToken("surface-sunken"),
            7.0,
        ),
        ("hint-text", "hint-surface", White, 12.0),
        ("text-muted", "hud-surface", White, 7.0),
        ("toggle-knob", "border-dialog", Black, 7.0),
        ("toggle-knob", "accent-cyan", Black, 7.0),
        ("toggle-knob", "accent-emerald", Black, 7.0),
        ("toggle-knob", "accent-amber", Black, 7.0),
        ("text-muted", "surface-chip-off", Black, 7.0),
        ("text-secondary", "surface-chip", Black, 7.0),
        ("accent-cyan", "brand-mark-bg", Black, 4.5),
        ("chart-border", "chart-bg", Black, 3.0),
        ("chart-crosshair", "chart-bg", Black, 7.0),
        ("chart-guide", "chart-bg", Black, 2.5),
        ("chart-grid-mid", "chart-bg", Black, 2.5),
        ("chart-grid", "chart-bg", Black, 1.5),
        ("text-on-accent", "swatch-clear", Black, 7.0),
        (
            "text-secondary",
            "swatch-clear-off",
            OnToken("surface-dialog"),
            7.0,
        ),
        (
            "text-secondary",
            "swatch-keep",
            OnToken("surface-dialog"),
            7.0,
        ),
    ];
    let tokens = theme_tokens();
    let high_contrast = |name: &str| {
        tokens
            .get(name)
            .unwrap_or_else(|| panic!("token `{name}` is missing from theme.slint"))
            .high_contrast
    };
    let mut failures = Vec::new();
    for (foreground, surface, under, least) in pairs {
        let base = match under {
            Black => [0.0; 3],
            White => [255.0; 3],
            OnToken(name) => {
                let colour = high_contrast(name);
                [colour[0], colour[1], colour[2]]
            }
        };
        let surface_colour = over(high_contrast(surface), base);
        let foreground_colour = over(high_contrast(foreground), surface_colour);
        let ratio = contrast(foreground_colour, surface_colour);
        if ratio < least {
            failures.push(format!(
                "{foreground} on {surface}: {ratio:.2}:1, wanted {least}:1"
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "high-contrast pairs below their ratio:\n{}",
        failures.join("\n")
    );
}

#[test]
fn converted_files_hold_no_colour_literals() {
    let no_data: &[&str] = &[];
    let converted = CONVERTED_FILES.iter().map(|file| (*file, no_data));
    let with_data = CONVERTED_WITH_DATA
        .iter()
        .map(|(file, allowed)| (*file, *allowed));
    let mut offenders = Vec::new();
    for (file, allowed) in converted.chain(with_data) {
        for (index, line) in read_ui(file).lines().enumerate() {
            for literal in colour_literals(&code_only(line)) {
                if !allowed.contains(&literal.as_str()) {
                    offenders.push(format!("{file}:{}: {literal}", index + 1));
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "use a Theme token instead of a colour literal:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn data_colour_allowances_are_all_still_in_use() {
    for (file, allowed) in CONVERTED_WITH_DATA {
        let source = read_ui(file);
        let used: Vec<String> = source
            .lines()
            .flat_map(|line| colour_literals(&code_only(line)))
            .collect();
        for literal in *allowed {
            assert!(
                used.iter().any(|found| found == literal),
                "{file} no longer holds `{literal}`: take it off the data allowance"
            );
        }
    }
}

#[test]
fn hex_literals_are_found_only_when_they_are_colours() {
    assert_eq!(
        hex_literals("background: #1e293b; color: #fff; x: #00000055;"),
        ["#1e293b", "#fff", "#00000055"]
    );
    let none = Vec::<String>::new();
    assert_eq!(hex_literals("let tag = #12345; x: #abcdefg;"), none);
    assert_eq!(hex_literals("color: Theme.text-muted;"), none);
}

#[test]
fn comments_and_strings_are_not_code() {
    assert_eq!(code_only("a: 1; // #ffffff white"), "a: 1; ");
    assert_eq!(
        code_only(r##"text: "#ffffff \" white"; b: 2;"##),
        "text: ; b: 2;"
    );
    assert_eq!(
        colour_literals(&code_only(r#"text: "white on black"; // #123456"#)),
        Vec::<String>::new()
    );
}

#[test]
fn colour_keywords_and_calls_are_found() {
    assert_eq!(colour_literals("background: white;"), ["white"]);
    assert_eq!(
        colour_literals("color: x ? black : Theme.text-muted;"),
        ["black"]
    );
    assert_eq!(colour_literals("c: rgb(1, 2, 3);"), ["rgb("]);
    assert_eq!(
        colour_literals("c: Theme.text-on-accent;"),
        Vec::<String>::new()
    );
}

#[test]
fn hex_parsing_reads_both_forms() {
    assert_eq!(parse_hex("#102030"), Some([16.0, 32.0, 48.0, 255.0]));
    assert_eq!(parse_hex("#10203040"), Some([16.0, 32.0, 48.0, 64.0]));
    assert_eq!(parse_hex("#102"), None);
    assert_eq!(parse_hex("102030"), None);
    assert_eq!(parse_hex("#10203g"), None);
}

#[test]
fn contrast_matches_the_wcag_extremes() {
    assert!((contrast([0.0; 3], [255.0; 3]) - 21.0).abs() < 1e-3);
    assert!((contrast([90.0; 3], [90.0; 3]) - 1.0).abs() < 1e-9);
}

#[test]
fn a_translucent_colour_is_blended_over_what_is_under_it() {
    assert_eq!(over([255.0, 0.0, 0.0, 255.0], [10.0; 3]), [255.0, 0.0, 0.0]);
    let half = over([200.0, 100.0, 0.0, 51.0], [0.0; 3]);
    assert!((half[0] - 40.0).abs() < 1e-9 && (half[1] - 20.0).abs() < 1e-9);
}
