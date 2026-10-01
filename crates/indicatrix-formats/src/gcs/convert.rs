//! [`gcs_to_asc_schedule`]: a parsed `.gcs` design as `.asc` cutting instructions.

use super::{design::GcsDesign, tier::GcsTier};
use crate::{
    asc::{AscLineEnding, AscParseError, AscSchedule, AscTier},
    gem::snap_index,
};

/// The refractive index (quartz) used when the file states none, or one not above 1,
/// which [`crate::asc::parse_asc`] would reject. Mirrors the `.gem` converter's
/// default.
const DEFAULT_REFRACTIVE_INDEX: f64 = 1.54;

/// The gear used when the file's tooth count is zero or beyond
/// [`AscParseError::MAX_GEAR_TEETH`]. Mirrors the `.gem` converter's default.
const DEFAULT_GEAR: u32 = 96;

/// The symmetry order used when the file's is zero. Mirrors the `.gem` converter's
/// default.
const DEFAULT_SYMMETRY: u32 = 1;

/// Collapses every run of CR/LF (real `<info>` values end in `\r\r\n`) into one
/// space and trims, so the text fits on one `.asc` line.
fn one_line(text: &str) -> String {
    text.split(['\r', '\n'])
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The non-empty one-line forms of `values`, in order.
fn lines<'a>(values: impl IntoIterator<Item = Option<&'a String>>) -> Vec<String> {
    values
        .into_iter()
        .flatten()
        .map(|v| one_line(v.as_str()))
        .filter(|v| !v.is_empty())
        .collect()
}

/// `gear` when [`crate::asc::parse_asc`] accepts it, otherwise [`DEFAULT_GEAR`] with
/// a line pushed onto `warnings`.
fn checked_gear(gear: u32, warnings: &mut Vec<String>) -> u32 {
    if (1..=AscParseError::MAX_GEAR_TEETH).contains(&gear) {
        return gear;
    }
    warnings.push(format!(
        "the .gcs file states a gear of {gear} teeth, outside 1..={}; {DEFAULT_GEAR} teeth assumed",
        AscParseError::MAX_GEAR_TEETH
    ));
    DEFAULT_GEAR
}

/// The `<render>` refractive index when it is above 1, otherwise
/// [`DEFAULT_REFRACTIVE_INDEX`] with a line pushed onto `warnings`.
fn checked_refractive_index(design: &GcsDesign, warnings: &mut Vec<String>) -> f64 {
    match design.render.as_ref().map(|r| r.refractive_index) {
        Some(ri) if ri.is_finite() && ri > 1.0 => ri,
        Some(ri) if ri.is_finite() && ri > 0.0 => {
            warnings.push(format!(
                "the .gcs file states a refractive index of {ri}, not above 1; \
                 {DEFAULT_REFRACTIVE_INDEX} assumed"
            ));
            DEFAULT_REFRACTIVE_INDEX
        }
        _ => {
            warnings.push(format!(
                "the .gcs file states no refractive index; {DEFAULT_REFRACTIVE_INDEX} assumed"
            ));
            DEFAULT_REFRACTIVE_INDEX
        }
    }
}

/// One `.asc` tier from a `.gcs` tier.
fn convert_tier(tier: &GcsTier, gear: u32) -> AscTier {
    let teeth = f64::from(gear);
    AscTier {
        angle_deg: tier.to_signed_asc_angle(),
        mast: tier.depth,
        name: tier.name.clone(),
        indices: tier
            .facets
            .iter()
            .map(|f| snap_index(f.index(gear), teeth))
            .collect(),
        index_names: Vec::new(),
        notes: one_line(&tier.instructions),
    }
}

/// Converts a parsed `.gcs` design into `.asc` cutting instructions.
///
/// One [`AscTier`] per `<tier>`, in file order: `angle_deg` from
/// [`GcsTier::to_signed_asc_angle`] (girdle `-90`, culet `-0`), `mast` = `depth`
/// (still in Gem Cut Studio's normalised frame: `max(|x|,|y|) = 1`, z-range centred;
/// masts are a common scale away from a `GemCAD` export of the same design),
/// `indices` from [`super::GcsFacet::index`] rounded like `.gem` indices, `name`,
/// and `notes` = the instructions on one line.
///
/// `gear_teeth` = the index gear (never negative), `gear_reference_angle` = 0,
/// `symmetry_order`/`mirror` from the `<index>` UI state (symmetry as is, mirror
/// `true` when the mirror offset is non-zero; both are UI state, see
/// [`super::GcsIndex`]), `refractive_index` from `<render>`, `headers` = title,
/// author, date, header2, header3 and `footnotes` = footer1..4 (non-empty, on one
/// line each). A missing refractive index, a hidden tier and a guide tier are
/// noted in `warnings`.
///
/// The schedule always parses back through [`crate::asc::parse_asc`] once written:
/// a gear of zero teeth or beyond [`AscParseError::MAX_GEAR_TEETH`] becomes 96 (the
/// tier indices are computed with it), a symmetry order of zero becomes 1, and a
/// refractive index that is missing or not above 1 becomes 1.54. Each substitution
/// is named in `warnings`.
#[must_use]
pub fn gcs_to_asc_schedule(design: &GcsDesign) -> AscSchedule {
    let mut warnings = design.warnings.clone();
    let gear = checked_gear(design.index.gear, &mut warnings);
    let symmetry_order = if design.index.symmetry == 0 {
        warnings.push(format!(
            "the .gcs file states a symmetry order of 0; {DEFAULT_SYMMETRY} assumed"
        ));
        DEFAULT_SYMMETRY
    } else {
        design.index.symmetry
    };
    let refractive_index = checked_refractive_index(design, &mut warnings);
    for (k, tier) in design.tiers.iter().enumerate() {
        if !tier.visible {
            warnings.push(format!(
                "tier #{k} ({}) is hidden in the .gcs file",
                tier.name
            ));
        }
        if tier.guide {
            warnings.push(format!(
                "tier #{k} ({}) is a guide tier in the .gcs file",
                tier.name
            ));
        }
    }
    let (headers, footnotes) = design.info.as_ref().map_or_else(Default::default, |info| {
        (
            lines([
                info.title.as_ref(),
                info.author.as_ref(),
                info.date.as_ref(),
                info.header2.as_ref(),
                info.header3.as_ref(),
            ]),
            lines([
                info.footer1.as_ref(),
                info.footer2.as_ref(),
                info.footer3.as_ref(),
                info.footer4.as_ref(),
            ]),
        )
    });
    AscSchedule {
        gemcad_version: "5.0".to_string(),
        gear_teeth: i32::try_from(gear).unwrap_or(i32::MAX),
        gear_reference_angle: 0.0,
        symmetry_order,
        mirror: design.index.mirror != 0,
        refractive_index,
        headers,
        footnotes,
        tiers: design.tiers.iter().map(|t| convert_tier(t, gear)).collect(),
        warnings,
        line_ending: AscLineEnding::default(),
    }
}
