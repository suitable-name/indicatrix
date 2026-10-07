//! Ranges, steps and presets for the sliders beside the main number fields.
//!
//! The Tier form's Angle field gets a slider and a "Typical" menu, and the Preform tab's
//! number fields get sliders. Every decision is made here, as plain functions, so the
//! desktop wiring only carries values between this module and the screen:
//!
//! - which side of the stone an angle belongs to, and so which range applies
//!   ([`angle_slider_state`]);
//! - the range, the 0.05 degree step (0.01 with Shift), the 0.5 degree marks and, for a
//!   pavilion, the band "critical angle plus 1 to plus 6 degrees" ([`angle_slider`]);
//! - the presets of the "Typical" menu, each a value with a one-line reason
//!   ([`angle_presets`]);
//! - the text a slider writes into the field ([`format_angle_text`]) and the number it
//!   reads back out of it ([`read_angle_field`]), so the two always agree;
//! - the Preform tab's sliders ([`preform_slider`]).
//!
//! The text fields stay the authority. A slider only shows what the field says and writes
//! plain numbers into it; the field keeps its arithmetic (`41.5+0.3`). Text a slider cannot
//! show (a relation such as `=P1-2`, or a calculation that does not work out) switches it
//! off. Nothing here saves anything: the Tier form is a draft saved by "Save Tier", the
//! Preform tab by "Apply Preform".
//!
//! # Where the numbers come from
//!
//! - The pavilion band is the margin over the critical angle `asin(1 / n)` of the design's
//!   effective refractive index. Two degrees is the app's own "Safe" margin
//!   (`indicatrix_cut_core::windowing_risk`), so a "Critical + 2" preset is always Safe.
//! - "Standard for this material" is the middle of the reference window the proportion
//!   chips already use (`indicatrix_cut_core::proportions_windows`: the published
//!   AGS/GIA round-brilliant ranges for diamond, the lapidary rule of thumb for colored
//!   stones), raised for a pavilion when that would leave less than the Safe margin.
//! - The crown presets Low, Medium and High (25, 32 and 40 degrees) are round figures that
//!   run from a flat to a steep crown; the "Standard" entry carries the published window.
//! - The Preform ranges follow the app's own units: the stone's girdle half-width is 1, so
//!   a rough is never narrower than 1.

use indicatrix_cut_core::{
    MaterialClass, ProportionMetric, ShapeClass, critical_angle_deg,
    design::labelling::name_indicates_pavilion, proportions_windows::window_for,
};

use crate::loading::eval_number;

/// What a plain drag, an arrow key or a click on an angle slider snaps to, in degrees.
pub const ANGLE_STEP_DEG: f64 = 0.05;
/// The step with Shift held, in degrees: the finest the field shows (two decimals).
pub const ANGLE_FINE_STEP_DEG: f64 = 0.01;
/// The distance between the marks under an angle slider, in degrees. The Page Up and Page
/// Down keys move by it, and a drag lands on a mark when it passes close to one.
pub const ANGLE_MARK_DEG: f64 = 0.5;
/// The lowest crown angle the slider shows, in degrees.
pub const CROWN_MIN_DEG: f64 = 5.0;
/// The highest crown angle the slider shows, in degrees.
pub const CROWN_MAX_DEG: f64 = 55.0;
/// The lowest pavilion angle the slider shows, in degrees. A high-index stone's band can
/// start lower; the range then grows to include it.
pub const PAVILION_MIN_DEG: f64 = 30.0;
/// The highest pavilion angle the slider shows, in degrees.
pub const PAVILION_MAX_DEG: f64 = 55.0;
/// Where the marked pavilion band starts: this many degrees over the critical angle.
pub const BAND_NEAR_DEG: f64 = 1.0;
/// Where the marked pavilion band ends: this many degrees over the critical angle.
pub const BAND_FAR_DEG: f64 = 6.0;

/// The margin over the critical angle that `indicatrix_cut_core::Risk::Safe` starts at.
const SAFE_MARGIN_DEG: f64 = 2.0;
/// A girdle facet (no slider): exactly 90 degrees from the girdle plane.
const GIRDLE_DEG: f64 = 90.0;
const GIRDLE_TOLERANCE_DEG: f64 = 1e-6;
/// The steepest angle a widened pavilion range reaches, in degrees.
const SLIDER_CEILING_DEG: f64 = 85.0;
/// The range of refractive indices the critical angle is worked out for.
const USABLE_INDEX: std::ops::RangeInclusive<f64> = 1.05..=6.0;

/// One slider's scale.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SliderRange {
    /// The value at the left end.
    pub min: f64,
    /// The value at the right end.
    pub max: f64,
    /// What a plain drag, an arrow key or a click snaps to.
    pub step: f64,
    /// The step with Shift held.
    pub fine_step: f64,
    /// The distance between the marks under the slider, also the Page Up and Page Down
    /// step; `0.0` for no marks.
    pub mark: f64,
}

/// The side of the stone an angle slider is for. Girdle and flat facets have no slider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AngleSide {
    /// Below the girdle.
    Pavilion,
    /// Above the girdle.
    Crown,
}

impl AngleSide {
    /// The side's name as the UI carries it: `"Pavilion"` or `"Crown"` (the same words as
    /// a tier row's block).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Pavilion => "Pavilion",
            Self::Crown => "Crown",
        }
    }

    /// The side named by [`Self::name`]; `None` for any other text.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "Pavilion" => Some(Self::Pavilion),
            "Crown" => Some(Self::Crown),
            _ => None,
        }
    }
}

/// An angle slider's scale and, for a pavilion, its marked band.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AngleSlider {
    /// The range, steps and marks.
    pub range: SliderRange,
    /// The critical angle at the design's refractive index; `None` when that index is
    /// missing or not usable.
    pub critical_deg: Option<f64>,
    /// The marked band (pavilion only), in degrees from the girdle: the critical angle
    /// plus [`BAND_NEAR_DEG`] up to plus [`BAND_FAR_DEG`].
    pub band: Option<(f64, f64)>,
}

/// One entry of the "Typical" menu.
#[derive(Debug, Clone, PartialEq)]
pub struct AnglePreset {
    /// What the menu shows ("Critical angle + 2 degrees (bright, safe)").
    pub label: String,
    /// The angle, in degrees from the girdle plane (a magnitude).
    pub value_deg: f64,
    /// The angle as the menu shows it: two decimals and a degree sign.
    pub value_text: String,
    /// Why this angle, in one line.
    pub reason: String,
}

/// What the Angle field holds, as far as a slider is concerned.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AngleField {
    /// A number or a calculation that works out, in degrees. Negative for the minus-sign
    /// convention of a pavilion angle.
    Degrees(f64),
    /// Text starting with `=`: the angle follows other tiers, so no slider.
    Relation,
    /// Anything else (a half-typed number, a calculation that does not work out).
    Unreadable,
}

/// Everything one slider needs, as plain numbers and text. The desktop maps it field by
/// field to the UI's `SliderSpec`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SliderSpecData {
    /// Whether to show the slider at all.
    pub visible: bool,
    /// Whether the field holds a number the slider can show and change.
    pub usable: bool,
    /// `"Pavilion"` or `"Crown"` for an angle slider, otherwise empty.
    pub side: String,
    /// The field's value (the magnitude for an angle); `0.0` while not usable.
    pub value: f64,
    /// The scale.
    pub range: SliderRange,
    /// The marked band, if any.
    pub band: Option<(f64, f64)>,
    /// A line of help under the slider; empty for none.
    pub note: String,
    /// The value in words, for screen readers; empty while not usable.
    pub value_text: String,
}

// ---- helpers -------------------------------------------------------------------------

/// Whether `n_d` is a refractive index the critical angle makes sense for.
fn usable_index(n_d: f64) -> bool {
    n_d.is_finite() && USABLE_INDEX.contains(&n_d)
}

/// The refractive index in a UI text such as `"1.7620"`; NaN for anything else.
fn parse_index_text(text: &str) -> f64 {
    text.trim().parse().unwrap_or(f64::NAN)
}

/// `value` rounded to two decimals.
fn round_hundredths(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

/// The smallest multiple of `step` that is at least `value`. Floating-point noise below
/// one part in a million of a step does not count as "over".
fn ceil_to_step(value: f64, step: f64) -> f64 {
    let steps = ((value / step) * 1e6).round() / 1e6;
    round_hundredths(steps.ceil() * step)
}

/// The smallest step-aligned angle at least `margin` degrees over `critical`.
fn angle_over_critical(critical: f64, margin: f64) -> f64 {
    let value = ceil_to_step(critical + margin, ANGLE_STEP_DEG);
    if value - critical < margin {
        round_hundredths(value + ANGLE_STEP_DEG)
    } else {
        value
    }
}

/// The largest multiple of `unit` that is at most `value`.
fn floor_to(value: f64, unit: f64) -> f64 {
    (value / unit).floor() * unit
}

/// The smallest multiple of `unit` that is at least `value`.
fn ceil_to(value: f64, unit: f64) -> f64 {
    (value / unit).ceil() * unit
}

/// A number with two decimals and no negative zero: what every slider writes.
fn two_decimals(value: f64) -> String {
    let rounded = round_hundredths(value);
    if rounded == 0.0 {
        "0.00".to_owned()
    } else {
        format!("{rounded:.2}")
    }
}

// ---- angle sliders -------------------------------------------------------------------

/// The scale of the angle slider for `side` at refractive index `n_d`.
///
/// Pavilion: 30 to 55 degrees, with the band "critical angle plus 1 to plus 6 degrees"
/// marked. A high-index stone's band starts below 30 degrees (diamond: 25.4), so the range
/// is widened downward in steps of 5 degrees to include the whole band. Crown: 5 to 55
/// degrees, no band. Both: 0.05 degree step, 0.01 with Shift, a mark every 0.5 degree.
/// Without a usable `n_d` there is no critical angle and no band.
#[must_use]
pub fn angle_slider(side: AngleSide, n_d: f64) -> AngleSlider {
    let critical_deg = usable_index(n_d).then(|| critical_angle_deg(n_d));
    let (mut min, mut max) = match side {
        AngleSide::Pavilion => (PAVILION_MIN_DEG, PAVILION_MAX_DEG),
        AngleSide::Crown => (CROWN_MIN_DEG, CROWN_MAX_DEG),
    };
    let band = critical_deg
        .filter(|_| side == AngleSide::Pavilion)
        .map(|critical| (critical + BAND_NEAR_DEG, critical + BAND_FAR_DEG));
    if let Some((from, to)) = band {
        min = min.min(floor_to(from, 5.0)).max(5.0);
        max = max.max(ceil_to(to, 5.0)).min(SLIDER_CEILING_DEG);
    }
    AngleSlider {
        range: SliderRange {
            min,
            max,
            step: ANGLE_STEP_DEG,
            fine_step: ANGLE_FINE_STEP_DEG,
            mark: ANGLE_MARK_DEG,
        },
        critical_deg,
        band,
    }
}

/// What the Angle field holds. A blank field reads as 0 degrees, so a new tier's slider
/// can be dragged from the start.
#[must_use]
pub fn read_angle_field(text: &str) -> AngleField {
    let trimmed = text.trim();
    if trimmed.starts_with('=') {
        return AngleField::Relation;
    }
    if trimmed.is_empty() {
        return AngleField::Degrees(0.0);
    }
    match eval_number(trimmed, None) {
        Ok(value) if value.is_finite() => AngleField::Degrees(value),
        _ => AngleField::Unreadable,
    }
}

/// The text a slider writes into the Angle field for `value_deg`.
///
/// Two decimals, with the minus sign kept when the field `like` already uses one (a
/// pavilion angle typed as `-41`). The field then reads back as the same number
/// ([`read_angle_field`]).
#[must_use]
pub fn format_angle_text(value_deg: f64, like: &str) -> String {
    let magnitude = round_hundredths(value_deg.abs());
    if magnitude > 0.0 && like.trim_start().starts_with('-') {
        format!("-{magnitude:.2}")
    } else {
        format!("{magnitude:.2}")
    }
}

/// The Angle field's text after an Up or Down key press moves it by `delta_deg`.
///
/// Only a plain number is stepped, and a blank field steps from 0. Text that holds a
/// calculation (`41.5+0.3`), a relation (`=P1-2`) or something that is no number gives
/// `None`, so the field keeps what was typed. The result has two decimals, locale
/// independent like every number the form writes ([`format_angle_text`]).
///
/// A step that lands on zero from a negative text keeps its minus sign (`-0.00`): the sign
/// is what tells a pavilion facet at 0 degrees from a crown one, so stepping through zero
/// must not flip the side by itself.
#[must_use]
pub fn stepped_angle_text(current: &str, delta_deg: f64) -> Option<String> {
    let trimmed = current.trim();
    let start = if trimmed.is_empty() {
        0.0
    } else {
        trimmed
            .parse::<f64>()
            .ok()
            .filter(|value| value.is_finite())?
    };
    let stepped = round_hundredths(start + delta_deg);
    if !stepped.is_finite() {
        return None;
    }
    Some(if stepped == 0.0 {
        if trimmed.starts_with('-') {
            "-0.00".to_owned()
        } else {
            "0.00".to_owned()
        }
    } else {
        format!("{stepped:.2}")
    })
}

/// The text the live margin bar should read for the Angle field.
///
/// The form shows a pavilion tier's angle as a plain positive number, but the bar tells
/// the two sides apart by the sign, so a positive pavilion angle is handed over with a
/// minus sign. Anything else (a crown, a negative number, a relation, text that is no
/// number) is passed through unchanged.
#[must_use]
pub fn margin_preview_text(side: Option<AngleSide>, text: &str) -> String {
    if side == Some(AngleSide::Pavilion)
        && let AngleField::Degrees(value) = read_angle_field(text)
        && value > 0.0
    {
        format!("-{value}")
    } else {
        text.to_owned()
    }
}

/// The angle slider as the Tier form's current state asks for it, or `None` when there is
/// no slider to show.
#[derive(Debug, Clone, PartialEq)]
pub struct AngleSliderState {
    /// The side whose range applies.
    pub side: AngleSide,
    /// The field's angle in degrees as a magnitude, or `None` while the field holds text the
    /// slider cannot show.
    pub value_deg: Option<f64>,
    /// The scale.
    pub slider: AngleSlider,
}

/// The slider for the Tier form's Angle field.
///
/// `kind` says what the form holds: `""` for a new tier, otherwise the loaded tier's block
/// (`"Pavilion"`, `"Crown"`, `"Girdle"`), `"Horizontal"` for a flat tier (the table, the
/// culet) and `"Driven"` for a tier whose angle follows a relation. Girdle, flat and driven
/// tiers have no slider, nor does a field holding a relation or exactly 90 degrees. A new
/// tier is a pavilion when its angle is typed with a minus sign or its `name` starts with
/// the pavilion letter P (the rule Save Tier applies), otherwise a crown. `n_d` is the
/// design's effective refractive index.
#[must_use]
pub fn angle_slider_state(
    kind: &str,
    text: &str,
    name: &str,
    n_d: f64,
) -> Option<AngleSliderState> {
    let value_deg = match read_angle_field(text) {
        AngleField::Relation => return None,
        AngleField::Degrees(value) => Some(value.abs()),
        AngleField::Unreadable => None,
    };
    let side = match kind {
        "Driven" | "Girdle" | "Horizontal" => return None,
        "Pavilion" => AngleSide::Pavilion,
        "Crown" => AngleSide::Crown,
        _ if text.trim_start().starts_with('-') || name_indicates_pavilion(name) => {
            AngleSide::Pavilion
        }
        _ => AngleSide::Crown,
    };
    if value_deg.is_some_and(|value| (value - GIRDLE_DEG).abs() < GIRDLE_TOLERANCE_DEG) {
        return None;
    }
    Some(AngleSliderState {
        side,
        value_deg,
        slider: angle_slider(side, n_d),
    })
}

/// A stone described for a menu entry: the material's name, or "this stone" when the design
/// names none.
fn material_label(material_name: &str) -> String {
    let name = material_name.trim();
    if name.is_empty() || name == "(none)" {
        "this stone".to_owned()
    } else {
        name.to_owned()
    }
}

/// The kind of stone a reference window is for, in a few words.
const fn class_words(class: MaterialClass) -> &'static str {
    match class {
        MaterialClass::High => "high-index stones such as diamond",
        MaterialClass::Mid | MaterialClass::Low => "colored stones",
    }
}

fn preset(label: String, value_deg: f64, reason: String) -> AnglePreset {
    AnglePreset {
        label,
        value_deg,
        value_text: format!("{value_deg:.2}\u{b0}"),
        reason,
    }
}

fn pavilion_presets(n_d: f64, material_name: &str) -> Vec<AnglePreset> {
    let critical = critical_angle_deg(n_d);
    let class = MaterialClass::from_ri(n_d);
    let mut presets = Vec::with_capacity(3);
    if let Some(window) = window_for(ShapeClass::Round, class, ProportionMetric::PavilionAngle) {
        let middle = ceil_to_step(f64::midpoint(window.min, window.max), ANGLE_STEP_DEG);
        let safe = angle_over_critical(critical, SAFE_MARGIN_DEG);
        let raised = safe > middle;
        let usual = format!(
            "The middle of the usual {:.1}\u{b0} to {:.1}\u{b0} range for {}.",
            window.min,
            window.max,
            class_words(class)
        );
        let reason = if raised {
            format!("{usual} Raised to keep 2\u{b0} over the critical angle ({critical:.1}\u{b0}).")
        } else {
            usual
        };
        presets.push(preset(
            format!("Standard for {}", material_label(material_name)),
            if raised { safe } else { middle },
            reason,
        ));
    }
    presets.push(preset(
        "Critical angle + 2\u{b0} (bright, safe)".to_owned(),
        angle_over_critical(critical, SAFE_MARGIN_DEG),
        format!(
            "The lowest angle the margin bar calls Safe: the critical angle ({critical:.1}\u{b0}) plus 2\u{b0}."
        ),
    ));
    presets.push(preset(
        "Critical angle + 4\u{b0} (extra margin)".to_owned(),
        angle_over_critical(critical, 2.0 * SAFE_MARGIN_DEG),
        "More room for a cutting error or a change of stone.".to_owned(),
    ));
    presets
}

fn crown_presets(n_d: f64, material_name: &str) -> Vec<AnglePreset> {
    let class = if usable_index(n_d) {
        MaterialClass::from_ri(n_d)
    } else {
        MaterialClass::Mid
    };
    let mut presets = Vec::with_capacity(4);
    if let Some(window) = window_for(ShapeClass::Round, class, ProportionMetric::CrownAngle) {
        presets.push(preset(
            format!("Standard for {}", material_label(material_name)),
            ceil_to_step(f64::midpoint(window.min, window.max), ANGLE_STEP_DEG),
            format!(
                "The middle of the usual {:.1}\u{b0} to {:.1}\u{b0} range for {}.",
                window.min,
                window.max,
                class_words(class)
            ),
        ));
    }
    presets.push(preset(
        "Low crown".to_owned(),
        25.0,
        "A flat crown: a large table and little height.".to_owned(),
    ));
    presets.push(preset(
        "Medium crown".to_owned(),
        32.0,
        "A balanced crown.".to_owned(),
    ));
    presets.push(preset(
        "High crown".to_owned(),
        40.0,
        "A steep crown: a small table and more height.".to_owned(),
    ));
    presets
}

/// The "Typical" menu for `side` at refractive index `n_d` and for the material called
/// `material_name` (empty for none), each entry a value with a one-line reason.
///
/// Pavilion: "Standard for the material", "Critical angle + 2 degrees (bright, safe)" and
/// "Critical angle + 4 degrees (extra margin)". Every pavilion preset is a multiple of the
/// 0.05 degree step and keeps at least the margin its label says (the step is rounded up,
/// never down). A pavilion menu needs a usable `n_d` and is empty without one. Crown:
/// "Standard for the material", Low (25), Medium (32) and High (40). An entry that repeats
/// an earlier one's angle is dropped.
#[must_use]
pub fn angle_presets(side: AngleSide, n_d: f64, material_name: &str) -> Vec<AnglePreset> {
    let mut presets = match side {
        AngleSide::Pavilion if usable_index(n_d) => pavilion_presets(n_d, material_name),
        AngleSide::Pavilion => Vec::new(),
        AngleSide::Crown => crown_presets(n_d, material_name),
    };
    let mut seen: Vec<f64> = Vec::with_capacity(presets.len());
    presets.retain(|entry| {
        if seen
            .iter()
            .any(|value| (value - entry.value_deg).abs() < ANGLE_STEP_DEG / 2.0)
        {
            false
        } else {
            seen.push(entry.value_deg);
            true
        }
    });
    presets
}

/// The line of help under an angle slider: the pavilion band in words, or the usual crown
/// range for this kind of stone.
fn angle_note(side: AngleSide, slider: &AngleSlider, n_d: f64) -> String {
    match (side, slider.band, slider.critical_deg) {
        (AngleSide::Pavilion, Some((from, to)), Some(critical)) => format!(
            "Green band: {from:.1}\u{b0} to {to:.1}\u{b0}, which is 1\u{b0} to 6\u{b0} over this stone's critical angle ({critical:.1}\u{b0})."
        ),
        (AngleSide::Crown, ..) => {
            let class = if usable_index(n_d) {
                MaterialClass::from_ri(n_d)
            } else {
                MaterialClass::Mid
            };
            window_for(ShapeClass::Round, class, ProportionMetric::CrownAngle).map_or_else(
                String::new,
                |window| {
                    format!(
                        "The usual crown range for {} is {:.0}\u{b0} to {:.0}\u{b0}.",
                        class_words(class),
                        window.min,
                        window.max
                    )
                },
            )
        }
        _ => String::new(),
    }
}

/// The Tier form's Angle slider as plain data.
///
/// `visible` is false when there is none (see [`angle_slider_state`] for `kind`, `text`
/// and `name`). `ri_text` is the design's effective refractive index as the UI shows it
/// (`"1.7620"`).
#[must_use]
pub fn angle_spec_data(kind: &str, text: &str, name: &str, ri_text: &str) -> SliderSpecData {
    let n_d = parse_index_text(ri_text);
    let Some(AngleSliderState {
        side,
        value_deg,
        slider,
    }) = angle_slider_state(kind, text, name, n_d)
    else {
        return SliderSpecData::default();
    };
    SliderSpecData {
        visible: true,
        usable: value_deg.is_some(),
        side: side.name().to_owned(),
        value: value_deg.unwrap_or(0.0),
        range: slider.range,
        band: slider.band,
        note: angle_note(side, &slider, n_d),
        value_text: value_deg.map_or_else(String::new, |value| format!("{value:.2} degrees")),
    }
}

/// The "Typical" menu for the side named `side` (`"Pavilion"` or `"Crown"`; anything else
/// gives an empty menu), with `ri_text` as in [`angle_spec_data`].
#[must_use]
pub fn angle_presets_for(side: &str, ri_text: &str, material_name: &str) -> Vec<AnglePreset> {
    AngleSide::from_name(side).map_or_else(Vec::new, |side| {
        angle_presets(side, parse_index_text(ri_text), material_name)
    })
}

// ---- preform sliders -----------------------------------------------------------------

/// A number field of the Preform tab that has a slider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreformField {
    /// Half-Width, in model units (the stone's girdle half-width is 1).
    HalfWidth,
    /// Length / Width, a ratio.
    LengthOverWidth,
    /// Depth, in model units.
    Depth,
    /// Girdle Y-Offset, in millimetres.
    YOffsetMm,
}

impl PreformField {
    /// The field named `"half_width"`, `"length_over_width"`, `"depth"` or `"y_offset"`.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "half_width" => Some(Self::HalfWidth),
            "length_over_width" => Some(Self::LengthOverWidth),
            "depth" => Some(Self::Depth),
            "y_offset" => Some(Self::YOffsetMm),
            _ => None,
        }
    }
}

/// A Preform slider as the fields currently ask for it.
#[derive(Debug, Clone, PartialEq)]
pub struct NumberSlider {
    /// The scale.
    pub range: SliderRange,
    /// The field's value, or `None` while it holds text that is no number.
    pub value: Option<f64>,
    /// Whether the slider can be used: the field holds a number and, for the Y-offset, the
    /// girdle diameter that fixes its scale is set.
    pub usable: bool,
    /// Why the slider is off, in a line; empty while it works.
    pub note: String,
}

const HALF_WIDTH_RANGE: SliderRange = SliderRange {
    min: 1.0,
    max: 3.0,
    step: 0.05,
    fine_step: 0.01,
    mark: 0.25,
};
const LENGTH_OVER_WIDTH_RANGE: SliderRange = SliderRange {
    min: 1.0,
    max: 2.5,
    step: 0.05,
    fine_step: 0.01,
    mark: 0.25,
};
const DEPTH_RANGE: SliderRange = SliderRange {
    min: 1.0,
    max: 3.5,
    step: 0.05,
    fine_step: 0.01,
    mark: 0.5,
};
/// The Y-offset's range before a girdle diameter makes one: shown switched off.
const Y_OFFSET_PLACEHOLDER_RANGE: SliderRange = SliderRange {
    min: -1.0,
    max: 1.0,
    step: 0.05,
    fine_step: 0.01,
    mark: 0.5,
};

/// A positive, finite number in a Preform field (arithmetic allowed); `None` otherwise.
fn positive_number(text: &str) -> Option<f64> {
    eval_number(text, None)
        .ok()
        .filter(|value| value.is_finite() && *value > 0.0)
}

/// The Y-offset slider's range: plus and minus half the rough's depth, in millimetres.
///
/// The depth is in model units and the girdle diameter in millimetres; the stone's girdle
/// half-width is 1 unit, so one unit is about half the diameter. The range only limits
/// the slider: the field takes any value, and Apply converts it exactly.
fn y_offset_range(depth_text: &str, girdle_diameter_text: &str) -> Option<SliderRange> {
    let depth = positive_number(depth_text)?;
    let girdle = positive_number(girdle_diameter_text)?;
    let reach = ceil_to(depth * girdle / 4.0, 0.5).max(0.5);
    Some(SliderRange {
        min: -reach,
        max: reach,
        step: 0.05,
        fine_step: 0.01,
        mark: 0.5,
    })
}

/// The slider for one Preform field.
///
/// `text` is the field itself; `depth_text` and `girdle_diameter_text` only matter for
/// the Y-offset, whose range is half the depth in millimetres and so needs both. Ranges:
/// Half-Width 1 to 3, Length / Width 1 to 2.5, Depth 1 to 3.5 (1 is the stone's own
/// half-width: a rough is never narrower), each in steps of 0.05 (0.01 with Shift).
/// Block and Cylinder roughs share them, because both have to enclose the same stone.
#[must_use]
pub fn preform_slider(
    field: PreformField,
    text: &str,
    depth_text: &str,
    girdle_diameter_text: &str,
) -> NumberSlider {
    let value = eval_number(text, None)
        .ok()
        .filter(|value| value.is_finite());
    let (range, scale_known) = match field {
        PreformField::HalfWidth => (HALF_WIDTH_RANGE, true),
        PreformField::LengthOverWidth => (LENGTH_OVER_WIDTH_RANGE, true),
        PreformField::Depth => (DEPTH_RANGE, true),
        PreformField::YOffsetMm => y_offset_range(depth_text, girdle_diameter_text)
            .map_or((Y_OFFSET_PLACEHOLDER_RANGE, false), |range| (range, true)),
    };
    let note = if scale_known {
        String::new()
    } else {
        "Set a Girdle Diameter (Yield section below) to use this slider.".to_owned()
    };
    NumberSlider {
        range,
        value,
        usable: value.is_some() && scale_known,
        note,
    }
}

/// The text a Preform slider writes into its field: two decimals, never `-0.00`.
#[must_use]
pub fn format_slider_number(value: f64) -> String {
    two_decimals(value)
}

/// A Preform slider as plain data (see [`SliderSpecData`]). `field` is the name
/// [`PreformField::from_name`] reads; an unknown name gives an invisible slider.
#[must_use]
pub fn preform_spec_data(
    field: &str,
    text: &str,
    depth_text: &str,
    girdle_diameter_text: &str,
) -> SliderSpecData {
    let Some(field) = PreformField::from_name(field) else {
        return SliderSpecData::default();
    };
    let slider = preform_slider(field, text, depth_text, girdle_diameter_text);
    SliderSpecData {
        visible: true,
        usable: slider.usable,
        side: String::new(),
        value: slider.value.unwrap_or(0.0),
        range: slider.range,
        band: None,
        note: slider.note,
        value_text: slider
            .value
            .map_or_else(String::new, |value| format!("{value:.2}")),
    }
}

#[cfg(test)]
mod tests;
