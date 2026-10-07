//! The Gem Material Editor's Dispersion section: a custom material's refractive-index curve
//! typed as Sellmeier or Cauchy coefficients instead of a refractive index and one
//! `n_F - n_C` number.
//!
//! Everything the section shows is computed here from the typed text ([`Readout::new`]) and
//! handed to Slint through pure callbacks on `DispersionEditorModel`, so the dialog keeps no
//! numeric logic of its own: the model kind and the coefficient fields live in the dialog, the
//! parsing, validation (`DispersionModel::validate`), warnings, the live `n_d` / `n_F` / `n_C` /
//! `n_F - n_C` / Abbe readout and the curve all come back as one [`Readout`]. The pure parts
//! are exercised by the unit tests below; [`setup_dispersion_callbacks`] is the only part that
//! touches Slint.
//!
//! The stored form is the JSON of `DispersionModelDto` (`indicatrix_cut_core::native`), the
//! same table the design file carries.

use crate::{DispersionEditorModel, DispersionView, MainWindow};
use indicatrix::optics::{dispersion::DispersionModel, materials::GemMaterial};
use indicatrix_cut_core::native::{dispersion_model_from_json, dispersion_model_to_json};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

/// The most coefficient fields any model has: a three-term Sellmeier's B1-B3 and C1-C3.
pub const FIELD_COUNT: usize = 6;

/// The first entry of the "Copy from" combo, before any built-in material.
const COPY_PROMPT: &str = "Copy from a built-in\u{2026}";
/// Shown in place of a readout that does not exist yet (an em dash).
const NONE_TEXT: &str = "\u{2014}";
/// The hint shown while no coefficient has been typed.
const EMPTY_HINT: &str =
    "Type the coefficients from a published fit, or copy them from a built-in material to start.";
/// The curve is sampled every 5 nm from 380 to 780 nm: this many steps.
const CURVE_STEPS: u16 = 80;
/// The curve's vertical range never gets narrower than this, so a flat curve is a line in
/// the middle of the plot rather than a division by zero.
const MIN_CURVE_SPAN: f32 = 0.002;

/// The model families the section offers, in the order of the dialog's model combo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelKind {
    /// One Sellmeier term.
    Sellmeier1,
    /// Three Sellmeier terms.
    Sellmeier3,
    /// A Cauchy fit.
    Cauchy,
}

impl ModelKind {
    /// Every kind, in combo order.
    pub const ALL: [Self; 3] = [Self::Sellmeier1, Self::Sellmeier3, Self::Cauchy];

    /// The kind at a model-combo index, or `None` for an index outside the combo.
    #[must_use]
    pub fn from_index(index: i32) -> Option<Self> {
        usize::try_from(index)
            .ok()
            .and_then(|i| Self::ALL.get(i).copied())
    }

    /// The model-combo index of this kind.
    #[must_use]
    pub const fn index(self) -> i32 {
        match self {
            Self::Sellmeier1 => 0,
            Self::Sellmeier3 => 1,
            Self::Cauchy => 2,
        }
    }

    /// The kind of `model`.
    #[must_use]
    pub const fn of(model: &DispersionModel) -> Self {
        match model {
            DispersionModel::Sellmeier1 { .. } => Self::Sellmeier1,
            DispersionModel::Sellmeier3 { .. } => Self::Sellmeier3,
            DispersionModel::Cauchy { .. } => Self::Cauchy,
        }
    }

    /// The label of each coefficient field, with its unit, in field order. The number of
    /// labels is the number of fields this kind needs.
    #[must_use]
    pub const fn labels(self) -> &'static [&'static str] {
        match self {
            Self::Sellmeier1 => &["B1", "C1 (\u{b5}m\u{b2})"],
            Self::Sellmeier3 => &[
                "B1",
                "B2",
                "B3",
                "C1 (\u{b5}m\u{b2})",
                "C2 (\u{b5}m\u{b2})",
                "C3 (\u{b5}m\u{b2})",
            ],
            Self::Cauchy => &["A", "B (\u{b5}m\u{b2})", "C (\u{b5}m^4)"],
        }
    }

    /// An example value for each field, shown greyed while the field is empty. Real fits
    /// (N-BK7 glass for Sellmeier, a typical glass for Cauchy), so the format is clear.
    #[must_use]
    pub const fn placeholders(self) -> &'static [&'static str] {
        match self {
            Self::Sellmeier1 => &["1.03961212", "0.00600069867"],
            Self::Sellmeier3 => &[
                "1.03961212",
                "0.231792344",
                "1.01046945",
                "0.00600069867",
                "0.0200179144",
                "103.560653",
            ],
            Self::Cauchy => &["1.5046", "0.0042", "0"],
        }
    }

    /// How many coefficient fields this kind uses.
    #[must_use]
    pub const fn field_count(self) -> usize {
        self.labels().len()
    }
}

/// The coefficients of `model` in field order ([`ModelKind::labels`]).
#[must_use]
pub fn coefficients(model: &DispersionModel) -> Vec<f32> {
    match *model {
        DispersionModel::Sellmeier1 { b1, c1 } => vec![b1, c1],
        DispersionModel::Sellmeier3 { b, c } => vec![b[0], b[1], b[2], c[0], c[1], c[2]],
        DispersionModel::Cauchy { a, b, c } => vec![a, b, c],
    }
}

/// `value` as the shortest text that reads back as the identical `f32`, so copying a built-in
/// material's coefficients and saving them again changes nothing.
fn format_coefficient(value: f32) -> String {
    value.to_string()
}

/// The coefficients of `model` as field text, in field order.
#[must_use]
pub fn coefficient_texts(model: &DispersionModel) -> Vec<String> {
    coefficients(model)
        .into_iter()
        .map(format_coefficient)
        .collect()
}

/// The model of `kind` with these coefficients (a missing value reads as 0).
fn model_from_values(kind: ModelKind, values: &[f32]) -> DispersionModel {
    let at = |i: usize| values.get(i).copied().unwrap_or(0.0);
    match kind {
        ModelKind::Sellmeier1 => DispersionModel::Sellmeier1 {
            b1: at(0),
            c1: at(1),
        },
        ModelKind::Sellmeier3 => DispersionModel::Sellmeier3 {
            b: [at(0), at(1), at(2)],
            c: [at(3), at(4), at(5)],
        },
        ModelKind::Cauchy => DispersionModel::Cauchy {
            a: at(0),
            b: at(1),
            c: at(2),
        },
    }
}

/// Why one coefficient field could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldProblem {
    /// Nothing typed.
    Blank,
    /// Not a finite number.
    NotANumber,
}

/// Reads one coefficient field. A comma is a decimal comma when there is no point, so
/// `1,5` is 1.5; anything else that is not a finite number is refused.
///
/// # Errors
///
/// [`FieldProblem::Blank`] for empty text, [`FieldProblem::NotANumber`] for anything that is
/// not a finite number (`abc`, `NaN`, `1e999`, `1,000.5`).
pub fn parse_coefficient(text: &str) -> Result<f32, FieldProblem> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(FieldProblem::Blank);
    }
    let normalised = if trimmed.contains(',') && !trimmed.contains('.') {
        trimmed.replace(',', ".")
    } else {
        trimmed.to_string()
    };
    normalised
        .parse::<f32>()
        .ok()
        .filter(|value| value.is_finite())
        .ok_or(FieldProblem::NotANumber)
}

/// The plain-words message for a field that could not be read.
fn problem_message(label: &str, text: &str, problem: FieldProblem) -> String {
    match problem {
        FieldProblem::Blank => {
            let name = label.split(" (").next().unwrap_or(label);
            format!("Enter a number for {name}.")
        }
        FieldProblem::NotANumber => format!(
            "'{}' is not a number. Use digits and a decimal point, for example 0.0042.",
            text.trim()
        ),
    }
}

/// Everything the Dispersion section shows for one model kind and one set of typed
/// coefficients: the field labels and messages, the validation result, the warnings, the live
/// readout and the curve, plus the JSON to store when the model is usable.
#[derive(Debug, Clone, PartialEq)]
pub struct Readout {
    /// The model can be saved: every field is a number and the curve passes
    /// `DispersionModel::validate`.
    pub valid: bool,
    /// One label per coefficient field, with its unit.
    pub labels: Vec<String>,
    /// One example value per coefficient field.
    pub placeholders: Vec<String>,
    /// One message per coefficient field, empty when the field is fine.
    pub field_errors: Vec<String>,
    /// A neutral hint, shown while nothing has been typed.
    pub hint: String,
    /// A problem with the curve as a whole (`DispersionError`), empty when there is none.
    pub model_error: String,
    /// Things worth a second look in a usable model.
    pub warnings: Vec<String>,
    /// `n_d`, the index at the sodium D line, as text.
    pub n_d: String,
    /// `n_F`, the index at the blue F line, as text.
    pub n_f: String,
    /// `n_C`, the index at the red C line, as text.
    pub n_c: String,
    /// `n_F - n_C` as text.
    pub delta: String,
    /// The Abbe number as text.
    pub abbe: String,
    /// `n_d` as a number (0 while the model is not usable).
    pub n_d_value: f32,
    /// `n_F - n_C` as a number (0 while the model is not usable).
    pub delta_value: f32,
    /// The curve as SVG path data in a 0..100 square, highest index at the top; empty while
    /// the model is not usable.
    pub curve: String,
    /// The index at the top edge of the plot, as text.
    pub curve_top: String,
    /// The index at the bottom edge of the plot, as text.
    pub curve_bottom: String,
    /// The model as the JSON the material database stores; empty while it is not usable.
    pub model_json: String,
}

impl Readout {
    /// An empty readout for `kind`: labels and examples filled in, nothing typed.
    fn empty(kind: ModelKind) -> Self {
        let owned = |items: &[&str]| {
            items
                .iter()
                .map(|item| (*item).to_owned())
                .collect::<Vec<_>>()
        };
        Self {
            valid: false,
            labels: owned(kind.labels()),
            placeholders: owned(kind.placeholders()),
            field_errors: vec![String::new(); kind.field_count()],
            hint: String::new(),
            model_error: String::new(),
            warnings: Vec::new(),
            n_d: NONE_TEXT.to_string(),
            n_f: NONE_TEXT.to_string(),
            n_c: NONE_TEXT.to_string(),
            delta: NONE_TEXT.to_string(),
            abbe: NONE_TEXT.to_string(),
            n_d_value: 0.0,
            delta_value: 0.0,
            curve: String::new(),
            curve_top: String::new(),
            curve_bottom: String::new(),
            model_json: String::new(),
        }
    }

    /// Reads `texts` (field order, missing ones blank) as a model of `kind` and works out
    /// everything the section shows for it.
    #[must_use]
    pub fn new(kind: ModelKind, texts: &[String]) -> Self {
        let mut readout = Self::empty(kind);
        let text_at = |i: usize| texts.get(i).map_or("", String::as_str);
        let labels = kind.labels();

        if (0..labels.len()).all(|i| text_at(i).trim().is_empty()) {
            readout.hint = EMPTY_HINT.to_string();
            return readout;
        }

        let mut values = Vec::with_capacity(labels.len());
        let mut complete = true;
        for (i, label) in labels.iter().enumerate() {
            match parse_coefficient(text_at(i)) {
                Ok(value) => values.push(value),
                Err(problem) => {
                    complete = false;
                    readout.field_errors[i] = problem_message(label, text_at(i), problem);
                    values.push(0.0);
                }
            }
        }
        if !complete {
            return readout;
        }

        let model = model_from_values(kind, &values);
        if let Err(error) = model.validate() {
            readout.model_error = error.to_string();
            return readout;
        }
        readout.fill_from(&model);
        readout
    }

    /// Fills the readout, curve, warnings and JSON from a model that passed validation.
    fn fill_from(&mut self, model: &DispersionModel) {
        self.valid = true;
        self.n_d = format!("{:.4}", model.n_d());
        self.n_f = format!("{:.4}", model.n_f());
        self.n_c = format!("{:.4}", model.n_c());
        self.delta = format!("{:.4}", model.delta_f_c());
        self.abbe = model
            .abbe_number()
            .map_or_else(|| NONE_TEXT.to_string(), |abbe| format!("{abbe:.1}"));
        self.n_d_value = model.n_d();
        self.delta_value = model.delta_f_c();
        self.warnings = warning_texts(model);
        let (curve, top, bottom) = curve_path(model);
        self.curve = curve;
        self.curve_top = format!("{top:.4}");
        self.curve_bottom = format!("{bottom:.4}");
        self.model_json = dispersion_model_to_json(model);
    }
}

/// The amber lines for a usable model, in plain words.
fn warning_texts(model: &DispersionModel) -> Vec<String> {
    let warnings = model.warnings();
    let mut texts = Vec::new();
    if warnings.anomalous {
        texts.push(
            "The index rises with wavelength somewhere between 380 and 780 nm. Real gems fall \
             from violet to red, so check the signs of the coefficients."
                .to_string(),
        );
    }
    if warnings.abbe_out_of_range
        && let Some(abbe) = model.abbe_number()
    {
        texts.push(format!(
            "The Abbe number is {abbe:.1}. Gem materials sit between about 5 and 120, so check \
             the coefficients."
        ));
    }
    if warnings.no_dispersion {
        texts.push(
            "The index is the same at the blue (F) and red (C) lines, so the stone will show \
             no fire."
                .to_string(),
        );
    }
    texts
}

/// The curve `n(wavelength)` from 380 to 780 nm as SVG path data in a 0..100 square (the
/// same `M x y L x y` form the tilt charts use), and the index at the top and bottom edges
/// of the plot. The vertical range is the curve's own spread plus a margin, never narrower
/// than [`MIN_CURVE_SPAN`].
fn curve_path(model: &DispersionModel) -> (String, f32, f32) {
    use std::fmt::Write as _;

    let samples: Vec<f32> = (0..=CURVE_STEPS)
        .map(|step| model.evaluate(f32::from(step).mul_add(5.0, DispersionModel::BAND_MIN_NM)))
        .collect();
    let low = samples.iter().copied().fold(f32::INFINITY, f32::min);
    let high = samples.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let centre = f32::midpoint(high, low);
    let half = (high - low).max(MIN_CURVE_SPAN) / 2.0 * 1.1;
    let (bottom, top) = (centre - half, centre + half);
    let range = top - bottom;

    let mut path = String::new();
    for (step, n) in (0..=CURVE_STEPS).zip(&samples) {
        let x = f32::from(step) * (100.0 / f32::from(CURVE_STEPS));
        let fraction = ((n - bottom) / range).clamp(0.0, 1.0);
        let y = (1.0 - fraction) * 100.0;
        if step > 0 {
            path.push(' ');
        }
        let command = if step == 0 { 'M' } else { 'L' };
        let _ = write!(path, "{command} {x:.2} {y:.2}");
    }
    (path, top, bottom)
}

/// The built-in materials whose ordinary-ray curve is a model of `kind` that passes
/// validation: what "Copy from" offers.
fn copy_sources(kind: ModelKind) -> Vec<GemMaterial> {
    GemMaterial::all_materials()
        .into_iter()
        .filter(|material| {
            ModelKind::of(&material.dispersion) == kind && material.dispersion.validate().is_ok()
        })
        .collect()
}

/// The entries of the "Copy from" combo for `kind`: a prompt first, then each built-in
/// material with a model of that kind. Empty when there is none (no built-in uses a one-term
/// Sellmeier), which hides the combo.
#[must_use]
pub fn copy_options(kind: ModelKind) -> Vec<String> {
    let names: Vec<String> = copy_sources(kind)
        .into_iter()
        .map(|material| material.name)
        .collect();
    if names.is_empty() {
        return names;
    }
    std::iter::once(COPY_PROMPT.to_string())
        .chain(names)
        .collect()
}

/// The coefficient text of the built-in material `name`, in field order; empty for the
/// prompt entry or a name that is not an option for `kind`.
#[must_use]
pub fn copied_texts(kind: ModelKind, name: &str) -> Vec<String> {
    copy_sources(kind)
        .into_iter()
        .find(|material| material.name == name)
        .map(|material| coefficient_texts(&material.dispersion))
        .unwrap_or_default()
}

/// The Cauchy coefficients, as field text, that reproduce a refractive index and an
/// `n_F - n_C` figure: the curve the simple mode builds, as a starting point for editing.
#[must_use]
pub fn cauchy_seed_texts(refractive_index: f32, dispersion: f32) -> Vec<String> {
    let seed = GemMaterial::new_custom("seed", refractive_index, dispersion, 0.0, [0.0; 3]);
    coefficient_texts(&seed.dispersion)
}

/// The model the dialog hands the save, from the JSON its Dispersion section produced.
///
/// # Errors
///
/// A message for the toast when the text is not blank but is not a usable model. Blank text
/// is `Ok(None)`: the simple refractive-index path.
pub fn model_from_save_json(json: &str) -> Result<Option<DispersionModel>, String> {
    let trimmed = json.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    dispersion_model_from_json(trimmed).map(Some).ok_or_else(|| {
        "The dispersion coefficients cannot be used. Check the Dispersion section and try again."
            .to_string()
    })
}

/// What the dialog's Dispersion section starts from when it opens on a stored material.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prefill {
    /// 0 for the simple mode, 1 for coefficients.
    pub mode: i32,
    /// The model-combo index.
    pub kind: i32,
    /// [`FIELD_COUNT`] field texts, blank where the model has no such coefficient.
    pub texts: Vec<String>,
}

/// The prefill for a stored row's model (`None` for a row on the plain path).
#[must_use]
pub fn prefill_for(model: Option<&DispersionModel>) -> Prefill {
    model.map_or_else(
        || Prefill {
            mode: 0,
            kind: ModelKind::Cauchy.index(),
            texts: vec![String::new(); FIELD_COUNT],
        },
        |model| {
            let mut texts = coefficient_texts(model);
            texts.resize(FIELD_COUNT, String::new());
            Prefill {
                mode: 1,
                kind: ModelKind::of(model).index(),
                texts,
            }
        },
    )
}

/// A Slint string model from owned strings.
fn string_model(items: Vec<String>) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(
        items
            .into_iter()
            .map(SharedString::from)
            .collect::<Vec<_>>(),
    ))
}

/// The Slint view of a [`Readout`].
fn view_of(readout: Readout) -> DispersionView {
    DispersionView {
        valid: readout.valid,
        labels: string_model(readout.labels),
        placeholders: string_model(readout.placeholders),
        field_errors: string_model(readout.field_errors),
        hint: readout.hint.into(),
        model_error: readout.model_error.into(),
        warnings: string_model(readout.warnings),
        n_d: readout.n_d.into(),
        n_f: readout.n_f.into(),
        n_c: readout.n_c.into(),
        delta: readout.delta.into(),
        abbe: readout.abbe.into(),
        n_d_value: readout.n_d_value,
        delta_value: readout.delta_value,
        curve: readout.curve.into(),
        curve_top: readout.curve_top.into(),
        curve_bottom: readout.curve_bottom.into(),
        model_json: readout.model_json.into(),
    }
}

/// Registers the pure callbacks of `DispersionEditorModel`: the live readout for the typed
/// coefficients, the "Copy from" options and their coefficients, and the Cauchy starting
/// point for the simple mode's numbers.
pub(super) fn setup_dispersion_callbacks(ui: &MainWindow) {
    let model = ui.global::<DispersionEditorModel>();
    model.on_evaluate(|kind, texts| {
        let kind = ModelKind::from_index(kind).unwrap_or(ModelKind::Cauchy);
        let texts: Vec<String> = texts.iter().map(String::from).collect();
        view_of(Readout::new(kind, &texts))
    });
    model.on_copy_options(|kind| {
        string_model(
            ModelKind::from_index(kind)
                .map(copy_options)
                .unwrap_or_default(),
        )
    });
    model.on_coefficient(|kind, name, index| {
        let texts = ModelKind::from_index(kind)
            .map(|kind| copied_texts(kind, &name))
            .unwrap_or_default();
        usize::try_from(index)
            .ok()
            .and_then(|i| texts.get(i).cloned())
            .unwrap_or_default()
            .into()
    });
    model.on_cauchy_seed(|refractive_index, dispersion, index| {
        let texts = cauchy_seed_texts(refractive_index, dispersion);
        usize::try_from(index)
            .ok()
            .and_then(|i| texts.get(i).cloned())
            .unwrap_or_default()
            .into()
    });
}

/// Pushes the Dispersion section's starting state for the material the editor will open on.
pub(super) fn push_prefill(ui: &MainWindow, prefill: &Prefill) {
    let model = ui.global::<DispersionEditorModel>();
    model.set_prefill_mode(prefill.mode);
    model.set_prefill_kind(prefill.kind);
    model.set_prefill_texts(string_model(prefill.texts.clone()));
}

#[cfg(test)]
mod tests;
