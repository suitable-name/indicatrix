use super::*;

/// The N-BK7 glass curve: a three-term Sellmeier no refractive index and dispersion pair
/// can describe.
fn bk7() -> DispersionModel {
    GemMaterial::by_name("Glass (N-BK7)")
        .expect("N-BK7 is a built-in")
        .dispersion
}

fn texts(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| (*item).to_owned()).collect()
}

// --- the kinds ---

#[test]
fn every_kind_has_matching_labels_examples_and_an_index() {
    for (position, kind) in ModelKind::ALL.into_iter().enumerate() {
        assert_eq!(kind.labels().len(), kind.placeholders().len(), "{kind:?}");
        assert_eq!(kind.field_count(), kind.labels().len());
        assert!(kind.field_count() <= FIELD_COUNT);
        assert_eq!(kind.index(), i32::try_from(position).unwrap());
        assert_eq!(ModelKind::from_index(kind.index()), Some(kind));
    }
    assert_eq!(ModelKind::Sellmeier3.field_count(), FIELD_COUNT);
    assert_eq!(ModelKind::from_index(-1), None);
    assert_eq!(ModelKind::from_index(3), None);
}

#[test]
fn the_examples_are_themselves_a_usable_model() {
    for kind in ModelKind::ALL {
        let example = texts(kind.placeholders());
        assert!(Readout::new(kind, &example).valid, "{kind:?}");
    }
}

#[test]
fn units_are_written_with_plain_latin_one_characters() {
    for kind in ModelKind::ALL {
        for label in kind.labels() {
            assert!(
                label
                    .chars()
                    .all(|c| c.is_ascii() || c == '\u{b5}' || c == '\u{b2}'),
                "{label}"
            );
        }
    }
    assert_eq!(ModelKind::Sellmeier1.labels()[1], "C1 (\u{b5}m\u{b2})");
}

// --- reading a field ---

#[test]
fn a_field_reads_numbers_with_a_point_or_a_decimal_comma() {
    assert_eq!(parse_coefficient("1.5"), Ok(1.5));
    assert_eq!(parse_coefficient("  2 "), Ok(2.0));
    assert_eq!(parse_coefficient("1,5"), Ok(1.5));
    assert_eq!(parse_coefficient("-0.0042"), Ok(-0.0042));
    assert_eq!(parse_coefficient("1e-3"), Ok(0.001));
}

#[test]
fn a_field_refuses_what_is_not_a_finite_number() {
    assert_eq!(parse_coefficient(""), Err(FieldProblem::Blank));
    assert_eq!(parse_coefficient("   "), Err(FieldProblem::Blank));
    for bad in [
        "abc",
        "NaN",
        "inf",
        "-infinity",
        "1e999",
        "1,000.5",
        "1.2.3",
        "--1",
    ] {
        assert_eq!(
            parse_coefficient(bad),
            Err(FieldProblem::NotANumber),
            "{bad}"
        );
    }
}

// --- the readout ---

#[test]
fn nothing_typed_gives_a_hint_and_no_errors() {
    let readout = Readout::new(ModelKind::Sellmeier3, &[]);
    assert!(!readout.valid);
    assert_eq!(readout.labels.len(), 6);
    assert_eq!(readout.placeholders.len(), 6);
    assert!(readout.field_errors.iter().all(String::is_empty));
    assert!(readout.hint.contains("copy them from a built-in"));
    assert!(readout.model_error.is_empty() && readout.model_json.is_empty());
    let blanks = vec![String::new(); FIELD_COUNT];
    assert_eq!(Readout::new(ModelKind::Sellmeier3, &blanks), readout);
}

#[test]
fn a_half_filled_form_names_each_missing_or_bad_field() {
    let readout = Readout::new(ModelKind::Sellmeier1, &texts(&["abc", ""]));
    assert!(!readout.valid);
    assert_eq!(readout.hint, "");
    assert!(readout.field_errors[0].contains("'abc' is not a number"));
    assert_eq!(readout.field_errors[1], "Enter a number for C1.");
    assert_eq!(readout.model_json, "");

    let readout = Readout::new(ModelKind::Cauchy, &texts(&["1.7", "0.006", ""]));
    assert!(!readout.valid);
    assert!(readout.field_errors[0].is_empty() && readout.field_errors[1].is_empty());
    assert_eq!(readout.field_errors[2], "Enter a number for C.");
}

#[test]
fn a_usable_cauchy_fit_fills_the_readout_curve_and_json() {
    let readout = Readout::new(ModelKind::Cauchy, &texts(&["1.7", "0.006", "0"]));
    let model = DispersionModel::Cauchy {
        a: 1.7,
        b: 0.006,
        c: 0.0,
    };
    assert!(readout.valid);
    assert!(readout.field_errors.iter().all(String::is_empty));
    assert_eq!(readout.model_error, "");
    assert_eq!(readout.n_d, format!("{:.4}", model.n_d()));
    assert_eq!(readout.n_f, format!("{:.4}", model.n_f()));
    assert_eq!(readout.n_c, format!("{:.4}", model.n_c()));
    assert_eq!(readout.delta, format!("{:.4}", model.delta_f_c()));
    assert_eq!(readout.n_d_value.to_bits(), model.n_d().to_bits());
    assert_eq!(readout.delta_value.to_bits(), model.delta_f_c().to_bits());
    assert!(readout.abbe.parse::<f32>().is_ok(), "{}", readout.abbe);
    assert_eq!(readout.model_json, dispersion_model_to_json(&model));
    assert_eq!(dispersion_model_from_json(&readout.model_json), Some(model));
}

#[test]
fn the_curve_has_one_point_per_five_nanometers_inside_the_plot() {
    let readout = Readout::new(ModelKind::Cauchy, &texts(&["1.7", "0.006", "0"]));
    let tokens: Vec<&str> = readout.curve.split_whitespace().collect();
    assert_eq!(tokens.len(), 81 * 3);
    assert_eq!(&tokens[..3], ["M", "0.00", tokens[2]]);
    assert_eq!(tokens[tokens.len() - 2], "100.00");
    assert_eq!(tokens.iter().filter(|t| **t == "L").count(), 80);
    // Index falls with wavelength, so the line runs from the top-left down to the right.
    let first: f32 = tokens[2].parse().unwrap();
    let last: f32 = tokens[tokens.len() - 1].parse().unwrap();
    assert!(first < last, "{first} {last}");
    for y in tokens
        .chunks(3)
        .map(|point| point[2].parse::<f32>().unwrap())
    {
        assert!((0.0..=100.0).contains(&y), "{y}");
    }
    // The axis labels bracket the readout's own indices.
    let top: f32 = readout.curve_top.parse().unwrap();
    let bottom: f32 = readout.curve_bottom.parse().unwrap();
    assert!(top > readout.n_f.parse::<f32>().unwrap());
    assert!(bottom < readout.n_c.parse::<f32>().unwrap());
}

#[test]
fn a_flat_curve_is_a_line_not_a_division_by_zero() {
    let readout = Readout::new(ModelKind::Cauchy, &texts(&["1.7", "0", "0"]));
    assert!(readout.valid);
    assert_eq!(readout.abbe, NONE_TEXT);
    assert_eq!(readout.warnings.len(), 1);
    assert!(readout.warnings[0].contains("no fire"));
    for y in readout
        .curve
        .split_whitespace()
        .skip(2)
        .step_by(3)
        .map(|y| y.parse::<f32>().unwrap())
    {
        assert!((y - 50.0).abs() < 0.1, "{y}");
    }
}

#[test]
fn a_visible_band_resonance_is_a_curve_error_not_a_field_error() {
    let readout = Readout::new(ModelKind::Sellmeier1, &texts(&["1.0", "0.36"]));
    assert!(!readout.valid);
    assert!(readout.field_errors.iter().all(String::is_empty));
    assert!(
        readout.model_error.contains("600 nm"),
        "{}",
        readout.model_error
    );
    assert!(readout.model_json.is_empty() && readout.curve.is_empty());
    assert_eq!(readout.n_d, NONE_TEXT);
}

#[test]
fn an_index_below_one_is_refused() {
    let readout = Readout::new(ModelKind::Cauchy, &texts(&["0.9", "0", "0"]));
    assert!(!readout.valid);
    assert!(
        readout.model_error.contains("above 1"),
        "{}",
        readout.model_error
    );
}

#[test]
fn warnings_are_listed_for_a_usable_but_odd_curve() {
    // The index rises with wavelength.
    let anomalous = Readout::new(ModelKind::Cauchy, &texts(&["1.5", "-0.002", "0"]));
    assert!(anomalous.valid);
    assert_eq!(anomalous.warnings.len(), 1);
    assert!(anomalous.warnings[0].contains("rises with wavelength"));
    // Barely dispersive: Abbe number far above 120.
    let weak = Readout::new(ModelKind::Cauchy, &texts(&["1.45", "0.0005", "0"]));
    assert!(weak.valid);
    assert_eq!(weak.warnings.len(), 1);
    assert!(weak.warnings[0].contains("Abbe number"));
    // A real glass raises none.
    let glass = Readout::new(ModelKind::Sellmeier3, &coefficient_texts(&bk7()));
    assert!(glass.valid);
    assert_eq!(glass.warnings, Vec::<String>::new());
}

#[test]
fn a_published_glass_reads_back_its_published_figures() {
    let glass = Readout::new(ModelKind::Sellmeier3, &coefficient_texts(&bk7()));
    assert!(glass.valid);
    let n_d: f32 = glass.n_d.parse().unwrap();
    let abbe: f32 = glass.abbe.parse().unwrap();
    assert!((n_d - 1.5168).abs() < 0.002, "{n_d}");
    // N-BK7's Abbe number is 64.17.
    assert!((abbe - 64.17).abs() < 1.5, "{abbe}");
}

// --- copy from / seeds / prefill ---

#[test]
fn copy_options_offer_a_prompt_then_the_built_ins_with_that_model() {
    let options = copy_options(ModelKind::Sellmeier3);
    assert_eq!(options[0], COPY_PROMPT);
    assert!(options.iter().any(|name| name == "Glass (N-BK7)"));
    assert!(options.iter().any(|name| name == "Diamond"));
    assert!(
        copy_options(ModelKind::Sellmeier1).is_empty(),
        "no built-in is a one-term Sellmeier, so the combo is hidden"
    );
    assert!(copy_options(ModelKind::Cauchy).len() > 1);
}

#[test]
fn every_copy_option_fills_every_field_with_a_usable_model() {
    for kind in ModelKind::ALL {
        for name in copy_options(kind).into_iter().skip(1) {
            let copied = copied_texts(kind, &name);
            assert_eq!(copied.len(), kind.field_count(), "{name}");
            let readout = Readout::new(kind, &copied);
            assert!(readout.valid, "{name}: {}", readout.model_error);
        }
        // The prompt and a stranger copy nothing.
        assert_eq!(copied_texts(kind, COPY_PROMPT), Vec::<String>::new());
        assert_eq!(copied_texts(kind, "No Such Stone"), Vec::<String>::new());
    }
}

#[test]
fn copying_a_built_in_reproduces_its_curve_bit_for_bit() {
    let copied = copied_texts(ModelKind::Sellmeier3, "Glass (N-BK7)");
    let readout = Readout::new(ModelKind::Sellmeier3, &copied);
    assert_eq!(dispersion_model_from_json(&readout.model_json), Some(bk7()));
}

#[test]
fn the_cauchy_seed_reproduces_the_simple_modes_numbers() {
    let seed = cauchy_seed_texts(2.417, 0.0256);
    assert_eq!(seed.len(), 3);
    let readout = Readout::new(ModelKind::Cauchy, &seed);
    assert!(readout.valid);
    assert!((readout.n_d_value - 2.417).abs() < 1e-4);
    assert!((readout.delta_value - 0.0256).abs() < 1e-4);
}

#[test]
fn a_stored_model_prefills_the_section_and_reads_back_identically() {
    let model = bk7();
    let prefill = prefill_for(Some(&model));
    assert_eq!(prefill.mode, 1);
    assert_eq!(prefill.kind, ModelKind::Sellmeier3.index());
    assert_eq!(prefill.texts.len(), FIELD_COUNT);
    let kind = ModelKind::from_index(prefill.kind).unwrap();
    let readout = Readout::new(kind, &prefill.texts);
    assert_eq!(readout.model_json, dispersion_model_to_json(&model));

    // A short model pads its unused fields with blanks.
    let one = DispersionModel::Sellmeier1 { b1: 1.0, c1: 0.01 };
    let padded = prefill_for(Some(&one));
    assert_eq!(padded.kind, ModelKind::Sellmeier1.index());
    assert_eq!(padded.texts.len(), FIELD_COUNT);
    assert!(padded.texts[2..].iter().all(String::is_empty));

    let plain = prefill_for(None);
    assert_eq!(plain.mode, 0);
    assert!(plain.texts.iter().all(String::is_empty));
}

// --- the save hand-off ---

#[test]
fn the_save_json_is_blank_for_the_simple_path_and_a_model_otherwise() {
    assert_eq!(model_from_save_json(""), Ok(None));
    assert_eq!(model_from_save_json("   "), Ok(None));
    let model = bk7();
    let json = dispersion_model_to_json(&model);
    assert_eq!(model_from_save_json(&json), Ok(Some(model)));
    let refusal = model_from_save_json("not a model").expect_err("garbage is refused");
    assert!(refusal.contains("cannot be used"));
    // A model that would not render is refused here too, whatever sent it.
    let resonant = r#"{"kind":"sellmeier1","b1":1.0,"c1":0.36}"#;
    assert!(model_from_save_json(resonant).is_err());
}
