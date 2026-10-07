//! Tests of the slider ranges, steps and presets.

use super::*;
use indicatrix_cut_core::{Risk, tier_margin_deg, windowing_risk};

const QUARTZ: f64 = 1.544;
const SAPPHIRE: f64 = 1.762;
const DIAMOND: f64 = 2.417;

/// An empty "Typical" menu, to compare against.
fn no_presets() -> Vec<AnglePreset> {
    Vec::new()
}

fn on_step(value: f64) -> bool {
    let steps = value / ANGLE_STEP_DEG;
    (steps - steps.round()).abs() < 1e-6
}

#[test]
fn a_pavilion_slider_spans_30_to_55_with_the_band_over_the_critical_angle() {
    let slider = angle_slider(AngleSide::Pavilion, QUARTZ);
    let critical = critical_angle_deg(QUARTZ);
    assert_eq!(slider.critical_deg, Some(critical));
    let (from, to) = slider.band.expect("a band");
    assert!((from - (critical + 1.0)).abs() < 1e-12);
    assert!((to - (critical + 6.0)).abs() < 1e-12);
    assert_eq!((slider.range.min, slider.range.max), (30.0, 55.0));
    assert_eq!(slider.range.step, 0.05);
    assert_eq!(slider.range.fine_step, 0.01);
    assert_eq!(slider.range.mark, 0.5);
}

#[test]
fn the_band_edges_are_one_and_six_degrees_of_margin() {
    for n_d in [QUARTZ, SAPPHIRE, DIAMOND] {
        let slider = angle_slider(AngleSide::Pavilion, n_d);
        let (from, to) = slider.band.expect("a band");
        assert!((tier_margin_deg(-from, n_d) - 1.0).abs() < 1e-9, "n={n_d}");
        assert!((tier_margin_deg(-to, n_d) - 6.0).abs() < 1e-9, "n={n_d}");
        assert_eq!(windowing_risk(-from, n_d), Risk::Marginal);
        assert_eq!(windowing_risk(-to, n_d), Risk::Safe);
    }
}

#[test]
fn a_high_index_band_that_starts_below_30_widens_the_range() {
    // Diamond: critical 24.4, band 25.4 to 30.4.
    let diamond = angle_slider(AngleSide::Pavilion, DIAMOND);
    assert_eq!(diamond.range.min, 25.0);
    assert_eq!(diamond.range.max, 55.0);
    // Moissanite-like: critical about 22.2, band from 23.2.
    let high = angle_slider(AngleSide::Pavilion, 2.65);
    assert_eq!(high.range.min, 20.0);
    let (from, to) = high.band.expect("a band");
    assert!(high.range.min <= from && to <= high.range.max);
}

#[test]
fn every_band_sits_inside_its_range_for_any_usable_index() {
    for step in 0..=70 {
        let n_d = f64::from(step).mul_add(0.07, 1.05);
        let slider = angle_slider(AngleSide::Pavilion, n_d);
        let (from, to) = slider.band.expect("a band");
        assert!(
            slider.range.min <= from && to <= slider.range.max,
            "n={n_d}: band {from}..{to} outside {}..{}",
            slider.range.min,
            slider.range.max
        );
        assert!(
            slider.range.min >= 5.0 && slider.range.max <= 85.0,
            "n={n_d}"
        );
    }
}

#[test]
fn a_crown_slider_spans_15_to_55_and_has_no_band() {
    let slider = angle_slider(AngleSide::Crown, QUARTZ);
    assert_eq!((slider.range.min, slider.range.max), (5.0, 55.0));
    assert_eq!(slider.band, None);
    assert_eq!(slider.range.step, 0.05);
    assert_eq!(slider.range.mark, 0.5);
}

#[test]
fn an_unusable_index_gives_no_critical_angle_and_no_band() {
    for n_d in [f64::NAN, f64::INFINITY, 1.0, 0.5, -2.0, 9.0] {
        let slider = angle_slider(AngleSide::Pavilion, n_d);
        assert_eq!(slider.critical_deg, None, "n={n_d}");
        assert_eq!(slider.band, None, "n={n_d}");
        assert_eq!((slider.range.min, slider.range.max), (30.0, 55.0));
    }
}

#[test]
fn the_plus_two_preset_is_always_safe_and_never_loses_margin_to_rounding() {
    for step in 0..150 {
        let n_d = f64::from(step).mul_add(0.01, 1.40);
        let presets = angle_presets(AngleSide::Pavilion, n_d, "Test");
        let plus_two = presets
            .iter()
            .find(|entry| entry.label.starts_with("Critical angle + 2"))
            .or_else(|| presets.first())
            .expect("a preset");
        let critical = critical_angle_deg(n_d);
        assert!(
            plus_two.value_deg - critical >= 2.0,
            "n={n_d}: {} over {critical}",
            plus_two.value_deg
        );
        assert_eq!(
            windowing_risk(-plus_two.value_deg, n_d),
            Risk::Safe,
            "n={n_d}"
        );
    }
}

#[test]
fn pavilion_presets_are_step_aligned_inside_the_range_and_in_a_plain_order() {
    for n_d in [QUARTZ, SAPPHIRE, DIAMOND, 2.65, 1.434] {
        let slider = angle_slider(AngleSide::Pavilion, n_d);
        let presets = angle_presets(AngleSide::Pavilion, n_d, "Stone");
        assert!(presets.len() >= 2, "n={n_d}");
        for entry in &presets {
            assert!(on_step(entry.value_deg), "n={n_d}: {}", entry.value_deg);
            assert!(
                entry.value_deg >= slider.range.min && entry.value_deg <= slider.range.max,
                "n={n_d}: {} outside the range",
                entry.value_deg
            );
            assert!(!entry.label.is_empty() && !entry.reason.is_empty());
            assert!(entry.value_text.ends_with('\u{b0}'));
        }
        assert!(presets[0].label.starts_with("Standard for Stone"));
    }
}

#[test]
fn standard_for_diamond_is_the_middle_of_the_published_window() {
    let presets = angle_presets(AngleSide::Pavilion, DIAMOND, "Diamond");
    let standard = &presets[0];
    assert_eq!(standard.label, "Standard for Diamond");
    // AGS/GIA pavilion window 40.6 to 41.8.
    assert!(
        (standard.value_deg - 41.2).abs() < 1e-9,
        "{}",
        standard.value_deg
    );
    assert!(!standard.reason.contains("Raised"));
    // 41.2 is far over the plus-2 and plus-4 margins of diamond, so all three stay.
    assert_eq!(presets.len(), 3);
}

#[test]
fn standard_for_a_low_index_stone_is_raised_to_the_safe_margin() {
    // Quartz: critical 40.35, rule-of-thumb window 40 to 43, middle 41.5 is only +1.1.
    let presets = angle_presets(AngleSide::Pavilion, QUARTZ, "Quartz");
    let standard = &presets[0];
    let safe = angle_over_critical(critical_angle_deg(QUARTZ), 2.0);
    assert!((standard.value_deg - safe).abs() < 1e-9);
    assert!(standard.reason.contains("Raised to keep 2"));
    assert_eq!(windowing_risk(-standard.value_deg, QUARTZ), Risk::Safe);
    // The plus-2 entry repeats that angle and is dropped; plus-4 stays.
    assert_eq!(presets.len(), 2);
    assert!(presets[1].label.starts_with("Critical angle + 4"));
}

#[test]
fn the_plus_four_preset_keeps_four_degrees_of_margin() {
    let presets = angle_presets(AngleSide::Pavilion, DIAMOND, "Diamond");
    let critical = critical_angle_deg(DIAMOND);
    let plus_four = presets
        .iter()
        .find(|entry| entry.label.starts_with("Critical angle + 4"))
        .expect("plus four");
    assert!(plus_four.value_deg - critical >= 4.0);
    assert!(plus_four.value_deg - critical < 4.0 + ANGLE_STEP_DEG + 1e-9);
}

#[test]
fn a_pavilion_menu_without_a_usable_index_is_empty() {
    assert_eq!(
        angle_presets(AngleSide::Pavilion, f64::NAN, "Stone"),
        no_presets()
    );
    assert_eq!(
        angle_presets(AngleSide::Pavilion, 1.0, "Stone"),
        no_presets()
    );
}

#[test]
fn crown_presets_are_standard_then_low_medium_high() {
    let colored = angle_presets(AngleSide::Crown, SAPPHIRE, "Sapphire");
    let values: Vec<f64> = colored.iter().map(|entry| entry.value_deg).collect();
    assert_eq!(values, [35.0, 25.0, 32.0, 40.0]);
    assert_eq!(colored[0].label, "Standard for Sapphire");
    assert_eq!(colored[1].label, "Low crown");
    assert_eq!(colored[2].label, "Medium crown");
    assert_eq!(colored[3].label, "High crown");
    // Diamond's AGS/GIA crown window is 34 to 35.
    let diamond = angle_presets(AngleSide::Crown, DIAMOND, "Diamond");
    assert!((diamond[0].value_deg - 34.5).abs() < 1e-9);
    // Without an index the crown menu still works, on the colored-stone window.
    let no_index = angle_presets(AngleSide::Crown, f64::NAN, "");
    assert_eq!(no_index[0].label, "Standard for this stone");
    assert_eq!(no_index[0].value_deg, 35.0);
    let slider = angle_slider(AngleSide::Crown, SAPPHIRE);
    assert!(colored.iter().all(
            |entry| entry.value_deg >= slider.range.min && entry.value_deg <= slider.range.max
        ));
}

#[test]
fn a_material_without_a_name_reads_as_this_stone() {
    for name in ["", "  ", "(none)"] {
        let presets = angle_presets(AngleSide::Pavilion, DIAMOND, name);
        assert_eq!(presets[0].label, "Standard for this stone");
    }
    let named = angle_presets(AngleSide::Pavilion, DIAMOND, " Sapphire ");
    assert_eq!(named[0].label, "Standard for Sapphire");
}

#[test]
fn menu_text_by_side_name_is_empty_for_an_unknown_side() {
    assert_eq!(
        angle_presets_for("Girdle", "1.5440", "Quartz"),
        no_presets()
    );
    assert_eq!(angle_presets_for("", "1.5440", "Quartz"), no_presets());
    assert_eq!(
        angle_presets_for("Pavilion", "1.5440", "Quartz"),
        angle_presets(AngleSide::Pavilion, 1.544, "Quartz")
    );
    assert_eq!(angle_presets_for("Pavilion", "", "Quartz"), no_presets());
}

// ---- value and text -------------------------------------------------------------

#[test]
fn a_value_written_by_the_slider_reads_back_as_the_same_number() {
    let mut hundredths = 0;
    while hundredths <= 9000 {
        let value = f64::from(hundredths) / 100.0;
        let text = format_angle_text(value, "");
        let AngleField::Degrees(read) = read_angle_field(&text) else {
            panic!("{text:?} should read as degrees");
        };
        assert!((read - value).abs() < 1e-9, "{value} -> {text:?} -> {read}");
        hundredths += 1;
    }
}

#[test]
fn the_minus_sign_convention_of_the_field_is_kept() {
    assert_eq!(format_angle_text(41.2, "-40"), "-41.20");
    assert_eq!(format_angle_text(41.2, "  -40.5 "), "-41.20");
    assert_eq!(format_angle_text(41.2, "40"), "41.20");
    assert_eq!(format_angle_text(41.2, ""), "41.20");
    // A value that rounds to zero never gets a minus sign.
    assert_eq!(format_angle_text(0.004, "-40"), "0.00");
    // The slider hands over a magnitude; a negative value is written as one.
    assert_eq!(format_angle_text(-41.256, "-40"), "-41.26");
    assert_eq!(
        read_angle_field("-41.20"),
        AngleField::Degrees(-41.2),
        "the written text is a pavilion angle again"
    );
}

#[test]
fn up_and_down_step_a_plain_number_by_the_given_amount() {
    assert_eq!(stepped_angle_text("41.2", 0.1).as_deref(), Some("41.30"));
    assert_eq!(stepped_angle_text("41.2", -0.1).as_deref(), Some("41.10"));
    assert_eq!(stepped_angle_text(" 41.2 ", 1.0).as_deref(), Some("42.20"));
    assert_eq!(stepped_angle_text("41.2", 0.01).as_deref(), Some("41.21"));
    assert_eq!(stepped_angle_text("-41.2", -0.1).as_deref(), Some("-41.30"));
    // A blank field steps from 0, so a new tier can be stepped from the start.
    assert_eq!(stepped_angle_text("", 0.1).as_deref(), Some("0.10"));
    assert_eq!(stepped_angle_text("  ", -0.1).as_deref(), Some("-0.10"));
    // The result is the plain number the field reads back.
    let text = stepped_angle_text("41.07", 0.1).expect("a plain number steps");
    assert_eq!(read_angle_field(&text), AngleField::Degrees(41.17));
}

#[test]
fn stepping_does_not_drift_over_many_presses() {
    let mut text = "40.00".to_owned();
    for _ in 0..100 {
        text = stepped_angle_text(&text, 0.1).expect("a plain number steps");
    }
    assert_eq!(text, "50.00");
    for _ in 0..1000 {
        text = stepped_angle_text(&text, -0.01).expect("a plain number steps");
    }
    assert_eq!(text, "40.00");
}

#[test]
fn stepping_through_zero_keeps_the_side_of_a_negative_text() {
    assert_eq!(stepped_angle_text("-0.10", 0.1).as_deref(), Some("-0.00"));
    assert_eq!(stepped_angle_text("0.10", -0.1).as_deref(), Some("0.00"));
    assert_eq!(stepped_angle_text("0", 0.004).as_deref(), Some("0.00"));
    assert_eq!(stepped_angle_text("-0.004", 0.0).as_deref(), Some("-0.00"));
}

#[test]
fn calculations_relations_and_junk_are_left_as_typed() {
    for text in ["41.5+0.3", "=P1-2", "abc", "41.5+", "NaN", "inf", "4 1"] {
        assert_eq!(stepped_angle_text(text, 0.1), None, "{text:?}");
    }
    assert_eq!(stepped_angle_text("1e308", 1e308), None, "overflow");
}

#[test]
fn the_field_reads_numbers_calculations_relations_and_junk() {
    assert_eq!(read_angle_field("41.5"), AngleField::Degrees(41.5));
    assert_eq!(read_angle_field(" -41.5 "), AngleField::Degrees(-41.5));
    assert_eq!(read_angle_field(""), AngleField::Degrees(0.0));
    assert_eq!(read_angle_field("   "), AngleField::Degrees(0.0));
    let AngleField::Degrees(sum) = read_angle_field("41.5+0.3") else {
        panic!("a calculation");
    };
    assert!((sum - 41.8).abs() < 1e-9);
    assert_eq!(read_angle_field("=P1-2"), AngleField::Relation);
    assert_eq!(read_angle_field("  = C1 + 1"), AngleField::Relation);
    for junk in ["abc", "-", "41.5+", "NaN", "inf", "1/0", "4 1"] {
        assert_eq!(read_angle_field(junk), AngleField::Unreadable, "{junk:?}");
    }
}

#[test]
fn the_margin_bar_is_told_a_plain_pavilion_angle_is_a_pavilion() {
    let pavilion = Some(AngleSide::Pavilion);
    assert_eq!(margin_preview_text(pavilion, "41.2"), "-41.2");
    assert_eq!(margin_preview_text(pavilion, " 41.2 "), "-41.2");
    assert_eq!(margin_preview_text(pavilion, "-41.2"), "-41.2");
    // A calculation is worked out first, never prefixed as text.
    assert_eq!(margin_preview_text(pavilion, "41.5+0.3"), "-41.8");
    // Zero, junk and relations are passed through.
    assert_eq!(margin_preview_text(pavilion, "0"), "0");
    assert_eq!(margin_preview_text(pavilion, "abc"), "abc");
    assert_eq!(margin_preview_text(pavilion, "=P1-2"), "=P1-2");
    // Other sides are left alone.
    assert_eq!(margin_preview_text(Some(AngleSide::Crown), "34.5"), "34.5");
    assert_eq!(margin_preview_text(None, "41.2"), "41.2");
}

// ---- which tiers get a slider ----------------------------------------------------

#[test]
fn loaded_pavilion_and_crown_tiers_get_their_own_side() {
    let pavilion = angle_slider_state("Pavilion", "41", "P1", QUARTZ).expect("a slider");
    assert_eq!(pavilion.side, AngleSide::Pavilion);
    assert_eq!(pavilion.value_deg, Some(41.0));
    assert_eq!(pavilion.slider.range.min, 30.0);
    let crown = angle_slider_state("Crown", "34.5", "C1", QUARTZ).expect("a slider");
    assert_eq!(crown.side, AngleSide::Crown);
    assert_eq!(crown.slider.range.min, 5.0);
    // The side of a loaded tier is the tier's, whatever its name or sign.
    let odd = angle_slider_state("Crown", "-34.5", "P9", QUARTZ).expect("a slider");
    assert_eq!(odd.side, AngleSide::Crown);
    assert_eq!(odd.value_deg, Some(34.5));
}

#[test]
fn girdle_flat_and_driven_tiers_have_no_slider() {
    for kind in ["Girdle", "Horizontal", "Driven"] {
        assert_eq!(angle_slider_state(kind, "30", "X", QUARTZ), None, "{kind}");
    }
}

#[test]
fn a_relation_or_exactly_90_degrees_has_no_slider() {
    assert_eq!(angle_slider_state("", "=P1-2", "", QUARTZ), None);
    assert_eq!(angle_slider_state("Crown", "=P1-2", "", QUARTZ), None);
    assert_eq!(angle_slider_state("", "90", "", QUARTZ), None);
    assert_eq!(angle_slider_state("", "-90", "", QUARTZ), None);
    assert_eq!(angle_slider_state("", "45+45", "", QUARTZ), None);
    assert!(angle_slider_state("", "89.9", "", QUARTZ).is_some());
}

#[test]
fn a_new_tier_is_a_pavilion_by_its_sign_or_its_name_otherwise_a_crown() {
    let side = |text: &str, name: &str| {
        angle_slider_state("", text, name, QUARTZ)
            .expect("a slider")
            .side
    };
    assert_eq!(side("0.0", ""), AngleSide::Crown);
    assert_eq!(side("34", "C1"), AngleSide::Crown);
    assert_eq!(side("41", "P1"), AngleSide::Pavilion);
    assert_eq!(side("41", "PF2"), AngleSide::Pavilion);
    assert_eq!(side("41", "Pavilion Main"), AngleSide::Pavilion);
    assert_eq!(side("-41", ""), AngleSide::Pavilion);
    assert_eq!(side("-", ""), AngleSide::Pavilion);
    assert_eq!(side("34", "Star"), AngleSide::Crown);
}

#[test]
fn unreadable_text_keeps_the_slider_but_switches_it_off() {
    let state = angle_slider_state("Pavilion", "41.5+", "P1", QUARTZ).expect("a slider");
    assert_eq!(state.value_deg, None);
    let data = angle_spec_data("Pavilion", "41.5+", "P1", "1.5440");
    assert!(data.visible && !data.usable);
    assert_eq!(data.value_text, "");
}

#[test]
fn a_calculation_is_shown_at_its_result() {
    let state = angle_slider_state("Pavilion", "41.5+0.3", "P1", QUARTZ).expect("a slider");
    assert!((state.value_deg.expect("a value") - 41.8).abs() < 1e-9);
}

// ---- the data the screen gets -------------------------------------------------------

#[test]
fn the_pavilion_spec_carries_the_range_the_band_and_the_words() {
    let data = angle_spec_data("Pavilion", "41.25", "P1", "1.5440");
    assert!(data.visible && data.usable);
    assert_eq!(data.side, "Pavilion");
    assert_eq!(data.value, 41.25);
    assert_eq!(data.value_text, "41.25 degrees");
    assert_eq!((data.range.min, data.range.max), (30.0, 55.0));
    let (from, to) = data.band.expect("a band");
    assert!(from < to);
    assert!(data.note.starts_with("Green band: "), "{}", data.note);
    assert!(data.note.contains("critical angle"), "{}", data.note);
}

#[test]
fn the_crown_spec_names_the_usual_range_instead_of_a_band() {
    let colored = angle_spec_data("Crown", "34.5", "C1", "1.7620");
    assert_eq!(colored.side, "Crown");
    assert_eq!(colored.band, None);
    assert_eq!(
        colored.note,
        "The usual crown range for colored stones is 30\u{b0} to 40\u{b0}."
    );
    let diamond = angle_spec_data("Crown", "34.5", "C1", "2.4170");
    assert_eq!(
        diamond.note,
        "The usual crown range for high-index stones such as diamond is 34\u{b0} to 35\u{b0}."
    );
}

#[test]
fn a_hidden_slider_is_the_default_spec() {
    assert_eq!(
        angle_spec_data("Girdle", "30", "", "1.5440"),
        SliderSpecData::default()
    );
    assert!(!angle_spec_data("", "=P1", "", "1.5440").visible);
}

#[test]
fn a_missing_index_text_still_gives_a_working_slider_without_a_band() {
    let data = angle_spec_data("Pavilion", "41", "P1", "");
    assert!(data.visible && data.usable);
    assert_eq!(data.band, None);
    assert_eq!(data.note, "");
}

// ---- preform ----------------------------------------------------------------------------

#[test]
fn preform_ranges_follow_the_stone_units() {
    let half_width = preform_slider(PreformField::HalfWidth, "1.20", "", "");
    assert_eq!((half_width.range.min, half_width.range.max), (1.0, 3.0));
    assert_eq!(half_width.value, Some(1.2));
    assert!(half_width.usable && half_width.note.is_empty());
    let ratio = preform_slider(PreformField::LengthOverWidth, "1.00", "", "");
    assert_eq!((ratio.range.min, ratio.range.max), (1.0, 2.5));
    let depth = preform_slider(PreformField::Depth, "1.5", "", "");
    assert_eq!((depth.range.min, depth.range.max), (1.0, 3.5));
    for slider in [&half_width, &ratio, &depth] {
        assert_eq!(slider.range.step, 0.05);
        assert_eq!(slider.range.fine_step, 0.01);
    }
}

#[test]
fn preform_fields_take_arithmetic_and_switch_off_on_junk() {
    let sum = preform_slider(PreformField::Depth, "1.2+0.3", "", "");
    assert!((sum.value.expect("a value") - 1.5).abs() < 1e-9);
    assert!(sum.usable);
    let junk = preform_slider(PreformField::Depth, "deep", "", "");
    assert_eq!(junk.value, None);
    assert!(!junk.usable);
    let blank = preform_slider(PreformField::HalfWidth, "", "", "");
    assert_eq!(blank.value, None);
    assert!(!blank.usable);
    let infinite = preform_slider(PreformField::HalfWidth, "inf", "", "");
    assert_eq!(infinite.value, None);
}

#[test]
fn the_y_offset_range_is_half_the_depth_in_millimetres() {
    // Depth 1.5 units on a 6.5 mm girdle: 1.5 * 6.5 / 4 = 2.4375, rounded up to 2.5.
    let slider = preform_slider(PreformField::YOffsetMm, "0.00", "1.5", "6.5");
    assert_eq!((slider.range.min, slider.range.max), (-2.5, 2.5));
    assert!(slider.usable && slider.note.is_empty());
    assert_eq!(slider.range.mark, 0.5);
    // A negative offset is a position inside the range.
    let below = preform_slider(PreformField::YOffsetMm, "-1.25", "1.5", "6.5");
    assert_eq!(below.value, Some(-1.25));
    assert!(below.usable);
    // The range grows with the depth and the girdle diameter.
    let bigger = preform_slider(PreformField::YOffsetMm, "0", "3.0", "12");
    assert_eq!(bigger.range.max, 9.0);
}

#[test]
fn the_y_offset_slider_is_off_until_a_girdle_diameter_is_set() {
    for girdle in ["", "unset", "0", "-4"] {
        let slider = preform_slider(PreformField::YOffsetMm, "0.00", "1.5", girdle);
        assert!(!slider.usable, "{girdle:?}");
        assert_eq!(
            slider.note,
            "Set a Girdle Diameter (Yield section below) to use this slider."
        );
        assert_eq!(slider.value, Some(0.0));
    }
    let no_depth = preform_slider(PreformField::YOffsetMm, "0.00", "deep", "6.5");
    assert!(!no_depth.usable);
}

#[test]
fn preform_spec_data_maps_the_field_names() {
    let data = preform_spec_data("half_width", "1.50", "1.5", "");
    assert!(data.visible && data.usable);
    assert_eq!(data.value, 1.5);
    assert_eq!(data.value_text, "1.50");
    assert_eq!(data.side, "");
    assert_eq!(data.band, None);
    assert_eq!((data.range.min, data.range.max), (1.0, 3.0));
    assert!(preform_spec_data("depth", "1.5", "", "").visible);
    assert!(preform_spec_data("length_over_width", "1", "", "").visible);
    assert!(preform_spec_data("y_offset", "0", "1.5", "6.5").usable);
    assert_eq!(
        preform_spec_data("colour", "1", "", ""),
        SliderSpecData::default()
    );
}

#[test]
fn preform_numbers_are_written_with_two_decimals() {
    assert_eq!(format_slider_number(1.2), "1.20");
    assert_eq!(format_slider_number(1.456), "1.46");
    assert_eq!(format_slider_number(-1.25), "-1.25");
    assert_eq!(format_slider_number(-0.001), "0.00");
    assert_eq!(format_slider_number(0.0), "0.00");
    // And it reads back.
    for hundredths in -500..=500 {
        let value = f64::from(hundredths) / 100.0;
        let text = format_slider_number(value);
        let read = eval_number(&text, None).expect("a number");
        assert!((read - value).abs() < 1e-9, "{value} -> {text:?}");
    }
}

#[test]
fn the_side_names_round_trip() {
    for side in [AngleSide::Pavilion, AngleSide::Crown] {
        assert_eq!(AngleSide::from_name(side.name()), Some(side));
    }
    assert_eq!(AngleSide::from_name("Girdle"), None);
    assert_eq!(AngleSide::from_name(""), None);
}
