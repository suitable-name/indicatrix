//! [`gem_to_asc_schedule`]: a decoded `.gem` design as `.asc` cutting instructions.

use super::model::{GemDesign, GemFacet};
use crate::asc::{AscLineEnding, AscParseError, AscSchedule, AscTier};

/// The refractive index (quartz) used when the file's is missing, not finite or not
/// above 1, which [`crate::asc::parse_asc`] would reject. Mirrors the `.gcs`
/// converter's default.
const DEFAULT_REFRACTIVE_INDEX: f64 = 1.54;

/// The gear used when the file's tooth count is zero or beyond
/// [`AscParseError::MAX_GEAR_TEETH`]. Mirrors the `.gcs` converter's default.
const DEFAULT_GEAR: i32 = 96;

/// The symmetry order used when the file's is zero or negative. Mirrors the `.gcs`
/// converter's default.
const DEFAULT_SYMMETRY: u32 = 1;

/// A tooth position within this distance of a whole tooth is written as that
/// whole tooth; anything further off is a fractional ("cheater") position.
const WHOLE_TOOTH_EPS: f64 = 1e-3;

/// Two facets of one tier number whose angles (degrees) or distances differ by
/// more than this are split into separate tiers.
const SAME_TIER_EPS: f64 = 1e-9;

/// Rounds a raw tooth position for an `.asc` index list: to the nearest whole tooth
/// when within `1e-3` of one, otherwise to 2 decimals. A result of 0 reads as
/// `teeth`, as `.asc` files write tooth 0.
pub fn snap_index(raw: f64, teeth: f64) -> f64 {
    let whole = raw.round();
    let snapped = if (raw - whole).abs() <= WHOLE_TOOTH_EPS {
        whole
    } else {
        (raw * 100.0).round() / 100.0
    };
    if snapped == 0.0 && teeth > 0.0 {
        teeth
    } else {
        snapped
    }
}

/// Converts a decoded `.gem` design into `.asc` cutting instructions.
///
/// Facets are grouped by tier number in order of first appearance, one
/// [`AscTier`] per tier. A tier number whose facets disagree on angle or distance
/// (3 of 3,494 corpus tiers) is split into one [`AscTier`] per distinct plane
/// setting, in file order, so no facet's geometry is lost. Per tier:
///
/// - `angle_deg` from [`GemFacet::angle_deg`] (culet: sign-negative zero);
/// - `mast` = [`GemFacet::distance`];
/// - `indices` from [`GemFacet::index`], rounded as [`snap_index`] does;
/// - `name` from the facet(s) carrying a name (several distinct names join with
///   `/`, consecutive repeats collapsed), `index_names` = each named facet's
///   position within the tier;
/// - `notes` = the first non-empty instructions of the tier.
///
/// The signed gear goes to `gear_teeth`, the gear offset to
/// `gear_reference_angle`, non-empty headings to `headers` and non-empty footnotes
/// to `footnotes`; `gemcad_version` is `"5.0"`. A preform section is not
/// converted; a warning records that it was present.
///
/// The schedule always parses back through [`crate::asc::parse_asc`] once written:
/// a gear of zero teeth or beyond [`AscParseError::MAX_GEAR_TEETH`] becomes 96, a
/// symmetry order of zero or less becomes 1, a refractive index that is missing, not
/// finite or not above 1 becomes 1.54, and a gear offset that is not finite becomes
/// 0. Each substitution is named in `warnings`, and the tier indices are computed
/// with the substituted gear.
#[must_use]
pub fn gem_to_asc_schedule(design: &GemDesign) -> AscSchedule {
    let mut warnings = Vec::new();
    let header = checked_header(design, &mut warnings);
    let tiers = group_tiers(&design.facets)
        .iter()
        .map(|group| build_tier(&design.facets, &header, group))
        .collect();
    if let Some(preform) = &design.preform {
        warnings.push(format!(
            "the .gem file embeds a preform design with {} facet(s); it is not part of these \
             cutting instructions",
            preform.facets.len()
        ));
    }
    AscSchedule {
        gemcad_version: "5.0".to_string(),
        gear_teeth: header.gear,
        gear_reference_angle: header.offset,
        symmetry_order: header.symmetry,
        mirror: design.mirror,
        refractive_index: header.refractive_index,
        headers: non_empty(&design.headings),
        footnotes: non_empty(&design.footnotes),
        tiers,
        warnings,
        line_ending: AscLineEnding::default(),
    }
}

/// The trailer values of a design after the substitutions
/// [`gem_to_asc_schedule`] makes so that `.asc` text parses.
struct CheckedHeader {
    gear: i32,
    offset: f64,
    symmetry: u32,
    refractive_index: f64,
}

/// `design`'s trailer values, each replaced by its default (and a line pushed onto
/// `warnings`) when [`crate::asc::parse_asc`] would reject it.
fn checked_header(design: &GemDesign, warnings: &mut Vec<String>) -> CheckedHeader {
    let gear = if design.gear != 0 && design.gear.unsigned_abs() <= AscParseError::MAX_GEAR_TEETH {
        design.gear
    } else {
        warnings.push(format!(
            "the .gem file states a gear of {} teeth, outside 1..={}; {DEFAULT_GEAR} teeth assumed",
            design.gear,
            AscParseError::MAX_GEAR_TEETH
        ));
        DEFAULT_GEAR
    };
    let offset = if design.gear_offset.is_finite() {
        design.gear_offset
    } else {
        warnings.push("the .gem file's gear offset is not finite; 0 assumed".to_string());
        0.0
    };
    let symmetry = match u32::try_from(design.symmetry) {
        Ok(order) if order > 0 => order,
        _ => {
            warnings.push(format!(
                "the .gem file states a symmetry order of {}; {DEFAULT_SYMMETRY} assumed",
                design.symmetry
            ));
            DEFAULT_SYMMETRY
        }
    };
    let ri = design.refractive_index;
    let refractive_index = if ri.is_finite() && ri > 1.0 {
        ri
    } else {
        warnings.push(format!(
            "the .gem file states a refractive index of {ri}, not above 1; \
             {DEFAULT_REFRACTIVE_INDEX} assumed"
        ));
        DEFAULT_REFRACTIVE_INDEX
    };
    CheckedHeader {
        gear,
        offset,
        symmetry,
        refractive_index,
    }
}

/// The non-empty strings of `slots`, in order.
fn non_empty(slots: &[String]) -> Vec<String> {
    slots.iter().filter(|s| !s.is_empty()).cloned().collect()
}

/// `true` when `a` and `b` describe the same plane setting (angle and distance).
fn same_setting(a: &GemFacet, b: &GemFacet) -> bool {
    let (angle_a, angle_b) = (a.angle_deg(), b.angle_deg());
    (angle_a - angle_b).abs() <= SAME_TIER_EPS
        && angle_a.is_sign_negative() == angle_b.is_sign_negative()
        && (a.distance() - b.distance()).abs() <= SAME_TIER_EPS
}

/// Facet positions grouped by tier number (and plane setting), in order of first
/// appearance. A linear search keeps the order deterministic.
fn group_tiers(facets: &[GemFacet]) -> Vec<Vec<usize>> {
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for (k, facet) in facets.iter().enumerate() {
        let existing = groups.iter_mut().find(|group| {
            let first = &facets[group[0]];
            first.tier == facet.tier && same_setting(first, facet)
        });
        match existing {
            Some(group) => group.push(k),
            None => groups.push(vec![k]),
        }
    }
    groups
}

/// Builds one [`AscTier`] from the facets at `group` (positions into `facets`,
/// never empty), indexing them on `header`'s gear and offset.
fn build_tier(facets: &[GemFacet], header: &CheckedHeader, group: &[usize]) -> AscTier {
    let first = &facets[group[0]];
    let teeth = f64::from(header.gear.unsigned_abs());
    let mut name_parts: Vec<&str> = Vec::new();
    let mut index_names = Vec::new();
    let mut notes = String::new();
    let mut indices = Vec::with_capacity(group.len());
    for (position, &k) in group.iter().enumerate() {
        let facet = &facets[k];
        indices.push(snap_index(facet.index(header.gear, header.offset), teeth));
        if let Some(name) = &facet.name {
            index_names.push((position, name.clone()));
            if name_parts.last() != Some(&name.as_str()) {
                name_parts.push(name);
            }
        }
        if notes.is_empty() && !facet.instructions.is_empty() {
            notes.clone_from(&facet.instructions);
        }
    }
    AscTier {
        angle_deg: first.angle_deg(),
        mast: first.distance(),
        name: name_parts.join("/"),
        indices,
        index_names,
        notes,
    }
}
