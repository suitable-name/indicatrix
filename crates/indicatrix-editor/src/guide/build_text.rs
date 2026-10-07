//! The words and numbers of a "Build this design" lesson: what a tier is for, tier names a
//! learner can type, and the angle, index and depth texts the tier form takes.
//!
//! Everything here is a pure function of the target design, so the generator stays a
//! sequence of decisions and the tests can check each text against the real form parsers.

use super::{rebuilt::blank_equals_zero, same_index_set};
use crate::loading::parse_index_list;
use indicatrix::geometry::meet_solver::Block;
use indicatrix_cut_core::ConstraintTier;
use std::collections::{BTreeMap, BTreeSet};

/// What a tier is for, inferred from its block, its angle, its name and how many index
/// positions it has next to its neighbours. The lesson words are cautious ("usually")
/// wherever the inference is a guess.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Role {
    Girdle,
    PavilionMain,
    PavilionBreak,
    Pavilion,
    Culet,
    CrownMain,
    Star,
    UpperBreak,
    Crown,
    Table,
}

impl Role {
    /// The role's name in a step title and in the grouping of a large design.
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Girdle => "girdle",
            Self::PavilionMain => "pavilion main",
            Self::PavilionBreak => "pavilion break",
            Self::Pavilion => "pavilion facet",
            Self::Culet => "culet",
            Self::CrownMain => "crown main",
            Self::Star => "star",
            Self::UpperBreak => "upper break",
            Self::Crown => "crown facet",
            Self::Table => "table",
        }
    }

    /// The plural a grouped step uses.
    pub(super) const fn plural(self) -> &'static str {
        match self {
            Self::Girdle => "girdle facets",
            Self::PavilionMain => "pavilion mains",
            Self::PavilionBreak => "pavilion breaks",
            Self::Pavilion => "pavilion facets",
            Self::Culet => "culet facets",
            Self::CrownMain => "crown mains",
            Self::Star => "star facets",
            Self::UpperBreak => "upper breaks",
            Self::Crown => "crown facets",
            Self::Table => "table facets",
        }
    }

    /// One sentence on what the tier is.
    pub(super) const fn blurb(self) -> &'static str {
        match self {
            Self::Girdle => "The girdle is the stone's widest band.",
            Self::PavilionMain => "The pavilion mains are the big facets below the girdle.",
            Self::PavilionBreak => "The pavilion breaks sit between the pavilion mains.",
            Self::Pavilion => "A pavilion facet, cut below the girdle.",
            Self::Culet => "The culet is the tip at the bottom of the pavilion.",
            Self::CrownMain => "The crown mains are the large facets above the girdle.",
            Self::Star => "The star facets ring the table.",
            Self::UpperBreak => {
                "The upper breaks sit between the crown mains, just above the girdle."
            }
            Self::Crown => "A crown facet, cut above the girdle.",
            Self::Table => "The table is the large flat facet on top.",
        }
    }

    /// The explanation under the step.
    pub(super) const fn why(self) -> &'static str {
        match self {
            Self::Girdle => {
                "The girdle is cut at exactly 90 degrees and goes first: the pavilion and the \
                 crown close against it, and its size sets the scale of the whole stone."
            }
            Self::PavilionMain => {
                "The pavilion mains send light back up through the crown, so they matter most for \
                 brilliance. Their steep angle has to stay beyond the material's critical angle, \
                 or light leaks out of the bottom."
            }
            Self::PavilionBreak => {
                "Break facets fill the gaps between the mains so the pavilion reads as one smooth \
                 bowl. They sit at the in-between index positions and usually have twice as many \
                 repeats as the mains."
            }
            Self::Pavilion => {
                "Pavilion facets are measured down from the girdle plane. Type a pavilion angle \
                 with a minus sign so the editor puts the tier below the girdle; the tier table \
                 then shows the plain number under a P label. Each one trims the rough that is \
                 still left below the girdle."
            }
            Self::Culet => {
                "The culet has an angle of minus zero: the minus sign tells the editor it is the \
                 bottom point and not the table. It takes off the sharp tip so the stone does not \
                 chip."
            }
            Self::CrownMain => {
                "The crown mains set the crown's height and most of the stone's fire. Like the \
                 pavilion mains they close against the girdle."
            }
            Self::Star => {
                "The stars are the shallowest crown facets. They fill the gaps between the table \
                 and the crown mains."
            }
            Self::UpperBreak => {
                "The upper breaks fill the gaps between the crown mains and finish the edge of \
                 the crown, so the girdle outline reads clean."
            }
            Self::Crown => {
                "Crown facets are measured up from the girdle plane, so type their angle without a sign."
            }
            Self::Table => {
                "The table has an angle of exactly 0 and is not repeated around the gear, so \
                 Indices stay blank. It is cut last, once the crown facets around it are in."
            }
        }
    }
}

/// The block's name in a sentence.
pub(super) const fn block_label(block: Block) -> &'static str {
    match block {
        Block::Crown => "crown",
        Block::Pavilion => "pavilion",
        Block::Girdle => "girdle",
    }
}

/// How many index positions the multi-position tiers of one block have.
#[derive(Clone, Debug, Default)]
struct BlockCounts {
    /// The fewest positions of any multi-position tier, if the block has one.
    base: Option<usize>,
    /// Whether the block has tiers with different position counts.
    varied: bool,
    /// The smallest angle among the tiers with `base` positions.
    lowest_base_angle: f64,
    /// How many tiers have `base` positions.
    base_tiers: usize,
}

impl BlockCounts {
    fn of(tiers: &[ConstraintTier], blocks: &[Block], block: Block) -> Self {
        let members: Vec<&ConstraintTier> = tiers
            .iter()
            .zip(blocks)
            .filter(|(tier, member_block)| **member_block == block && tier.indices.len() > 1)
            .map(|(tier, _)| tier)
            .collect();
        let Some(base) = members.iter().map(|tier| tier.indices.len()).min() else {
            return Self::default();
        };
        let at_base: Vec<&ConstraintTier> = members
            .iter()
            .copied()
            .filter(|tier| tier.indices.len() == base)
            .collect();
        Self {
            base: Some(base),
            varied: members.iter().any(|tier| tier.indices.len() != base),
            lowest_base_angle: at_base
                .iter()
                .map(|tier| tier.angle_deg.abs())
                .fold(f64::INFINITY, f64::min),
            base_tiers: at_base.len(),
        }
    }
}

/// The role of every tier of `tiers` (parallel to `names` and `blocks`).
pub(super) fn infer_roles(
    tiers: &[ConstraintTier],
    names: &[String],
    blocks: &[Block],
) -> Vec<Role> {
    let crown = BlockCounts::of(tiers, blocks, Block::Crown);
    let pavilion = BlockCounts::of(tiers, blocks, Block::Pavilion);
    tiers
        .iter()
        .zip(names)
        .zip(blocks)
        .map(|((tier, name), &block)| {
            let lower = name.to_ascii_lowercase();
            match block {
                Block::Girdle => Role::Girdle,
                Block::Crown => crown_role(tier, &lower, &crown),
                Block::Pavilion => pavilion_role(tier, &lower, &pavilion),
            }
        })
        .collect()
}

fn crown_role(tier: &ConstraintTier, lower: &str, counts: &BlockCounts) -> Role {
    if tier.is_table() || lower == "table" {
        return Role::Table;
    }
    if lower.contains("star") {
        return Role::Star;
    }
    if lower.contains("break") || lower.contains("upper girdle") || lower.contains("upper-girdle") {
        return Role::UpperBreak;
    }
    if lower.contains("main") {
        return Role::CrownMain;
    }
    let count = tier.indices.len();
    match counts.base {
        Some(base) if counts.varied && count == base * 2 => Role::UpperBreak,
        Some(base) if counts.varied && count == base => {
            let lowest = counts.base_tiers > 1
                && (tier.angle_deg.abs() - counts.lowest_base_angle).abs() < 1e-9;
            if lowest { Role::Star } else { Role::CrownMain }
        }
        _ => Role::Crown,
    }
}

fn pavilion_role(tier: &ConstraintTier, lower: &str, counts: &BlockCounts) -> Role {
    if tier.angle_deg == 0.0 || lower.contains("culet") {
        return Role::Culet;
    }
    if lower.contains("break") || lower.contains("lower girdle") || lower.contains("lower-girdle") {
        return Role::PavilionBreak;
    }
    if lower.contains("main") {
        return Role::PavilionMain;
    }
    let count = tier.indices.len();
    match counts.base {
        Some(base) if counts.varied && count == base * 2 => Role::PavilionBreak,
        Some(base) if counts.varied && count == base => Role::PavilionMain,
        _ => Role::Pavilion,
    }
}

/// Whether `name` can be typed into the tier form and named in another tier's "Meets" list:
/// not blank, and without the two characters the form reads as separators.
fn usable_name(name: &str) -> bool {
    !name.is_empty() && !name.contains([',', '/'])
}

/// The prefix of a name made up for an unnamed tier.
fn generated_prefix(tier: &ConstraintTier, block: Block) -> &'static str {
    if tier.is_table() {
        "T"
    } else {
        match block {
            Block::Pavilion if tier.angle_deg == 0.0 => "U",
            Block::Pavilion => "P",
            Block::Crown => "C",
            Block::Girdle => "G",
        }
    }
}

/// A name for every tier that the tier form accepts and no other tier shares (case does not
/// matter): the tier's own name where it is usable and the first of its kind, otherwise a
/// made-up one such as `P3`.
pub(super) fn teaching_names(tiers: &[ConstraintTier], blocks: &[Block]) -> Vec<String> {
    let mut taken: BTreeSet<String> = BTreeSet::new();
    let kept: Vec<Option<String>> = tiers
        .iter()
        .map(|tier| {
            let name = tier.name.trim();
            (usable_name(name) && taken.insert(name.to_ascii_lowercase())).then(|| name.to_owned())
        })
        .collect();
    let mut next_number: BTreeMap<&'static str, usize> = BTreeMap::new();
    kept.into_iter()
        .zip(tiers.iter().zip(blocks))
        .map(|(kept, (tier, &block))| {
            kept.unwrap_or_else(|| {
                let prefix = generated_prefix(tier, block);
                let counter = next_number.entry(prefix).or_insert(0);
                loop {
                    *counter += 1;
                    let candidate = format!("{prefix}{counter}");
                    if taken.insert(candidate.to_ascii_lowercase()) {
                        return candidate;
                    }
                }
            })
        })
        .collect()
}

/// `value` with at most `max_decimals` decimals, trailing zeros dropped but one decimal
/// kept (`90.0`, `34.5`).
pub(super) fn number_text(value: f64, max_decimals: usize) -> String {
    let fixed = format!("{value:.max_decimals$}");
    let Some((whole, fraction)) = fixed.split_once('.') else {
        return fixed;
    };
    let fraction = fraction.trim_end_matches('0');
    if fraction.is_empty() {
        let whole = if whole == "-0" { "0" } else { whole };
        format!("{whole}.0")
    } else {
        format!("{whole}.{fraction}")
    }
}

/// `value` as a whole number when it is one, otherwise with at most three decimals (an
/// index position: `12`, `12.5`).
pub(super) fn plain_number_text(value: f64) -> String {
    let fixed = format!("{value:.3}");
    let trimmed = fixed.trim_end_matches('0').trim_end_matches('.');
    if trimmed == "-0" {
        "0".to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// The text to type into the Angle field: the angle in degrees, with the culet's minus
/// zero kept.
pub(super) fn angle_text(angle_deg: f64) -> String {
    if angle_deg == 0.0 {
        return if angle_deg.is_sign_negative() {
            "-0".to_owned()
        } else {
            "0.0".to_owned()
        };
    }
    let text = number_text(angle_deg, 4);
    // An angle too small for four decimals keeps its own digits rather than becoming zero.
    if text.parse::<f64>().is_ok_and(|rounded| rounded == 0.0) {
        angle_deg.to_string()
    } else {
        text
    }
}

/// The text to type into the Meets field for an exact scale value: four decimals, so the
/// typed depth is within 0.00005 of the target's.
pub(super) fn mast_text(mast: f64) -> String {
    number_text(mast, 4)
}

/// What to type into the Indices field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct IndicesText {
    /// The text; empty when the field stays blank.
    pub(super) typed: String,
    /// Whether the field stays blank.
    pub(super) blank: bool,
}

/// Whether the tier is typed with a blank Indices field: it has no positions, or it is a
/// table, culet or girdle with the lone position `0` (which a blank field means).
fn is_blank(indices: &[f64], gear: f64, angle_deg: f64) -> bool {
    indices.is_empty()
        || (indices.len() == 1
            && blank_equals_zero(angle_deg)
            && same_index_set(indices, &[0.0], gear))
}

/// The orbit shorthand `B xN` for `indices` (the base position and the number of equal
/// steps around the gear), if it is shorter than the plain list.
fn orbit_shorthand(indices: &[f64], gear: f64) -> Option<String> {
    let count = indices.len();
    if count < 4 || gear <= 0.0 {
        return None;
    }
    let base = indices
        .iter()
        .map(|index| index.rem_euclid(gear))
        .fold(f64::INFINITY, f64::min);
    Some(format!("{} x{count}", plain_number_text(base)))
}

/// The arithmetic shorthand `start:step:stop` for `indices`, if they step evenly.
fn colon_shorthand(indices: &[f64]) -> Option<String> {
    let [first, second, .., last] = indices else {
        return None;
    };
    if indices.len() < 4 {
        return None;
    }
    let step = second - first;
    if step.abs() < 1e-9
        || !indices
            .windows(2)
            .all(|pair| (pair[1] - pair[0] - step).abs() < 1e-9)
    {
        return None;
    }
    Some(format!(
        "{}:{}:{}",
        plain_number_text(*first),
        plain_number_text(step),
        plain_number_text(last + step)
    ))
}

/// Whether the tier form reads `text` as exactly the positions `indices`.
fn reads_back(text: &str, indices: &[f64], gear_teeth: u32) -> bool {
    parse_index_list(text, gear_teeth)
        .is_ok_and(|parsed| same_index_set(&parsed, indices, f64::from(gear_teeth)))
}

/// What to type into the Indices field for a tier at `angle_deg` with these positions: the
/// shortest of the plain list, the orbit shorthand and the arithmetic shorthand that the
/// tier form reads back as the same positions.
pub(super) fn indices_text(indices: &[f64], gear_teeth: u32, angle_deg: f64) -> IndicesText {
    let gear = f64::from(gear_teeth);
    if is_blank(indices, gear, angle_deg) {
        return IndicesText {
            typed: String::new(),
            blank: true,
        };
    }
    let list = indices
        .iter()
        .map(|&index| plain_number_text(index))
        .collect::<Vec<_>>()
        .join(", ");
    let shortest = [orbit_shorthand(indices, gear), colon_shorthand(indices)]
        .into_iter()
        .flatten()
        .filter(|candidate| {
            candidate.len() < list.len() && reads_back(candidate, indices, gear_teeth)
        })
        .min_by_key(String::len);
    IndicesText {
        typed: shortest.unwrap_or(list),
        blank: false,
    }
}

/// The plain list of positions behind a shorthand, for the line under it, or a short
/// description when the list is long. `None` when the typed text is already the plain list.
pub(super) fn indices_expansion(typed: &str, indices: &[f64], gear_teeth: u32) -> Option<String> {
    if !typed.contains(['x', ':']) {
        return None;
    }
    let list: Vec<String> = parse_index_list(typed, gear_teeth)
        .ok()?
        .into_iter()
        .map(plain_number_text)
        .collect();
    let joined = list.join(", ");
    if joined.len() <= 72 {
        Some(joined)
    } else {
        Some(format!("{} positions", indices.len()))
    }
}

/// A few words on the positions a goal waits for: `no indices`, `8 indices`, `1 index`.
pub(super) fn indices_phrase(text: &IndicesText, indices: &[f64]) -> String {
    match (text.blank, indices.len()) {
        (true, _) | (_, 0) => "no indices".to_owned(),
        (false, 1) => "1 index".to_owned(),
        (false, count) => format!("{count} indices"),
    }
}
