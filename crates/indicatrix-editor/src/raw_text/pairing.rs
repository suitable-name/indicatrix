//! Pairing the tiers of an edited text with the tiers of the text the design produced, and
//! refusing what the design cannot hold.

use super::{ParsedText, TextProblem, list, same, same_all};
use indicatrix_cut_core::Design;
use indicatrix_formats::asc::AscTier;

/// The first index not yet taken for which `matches` holds.
fn first_free(taken: &[bool], matches: impl Fn(usize) -> bool) -> Option<usize> {
    (0..taken.len()).find(|&index| !taken[index] && matches(index))
}

/// For each tier of the edited text, the position of the tier it continues in the text the
/// design produced, or `None` for a tier the text adds.
///
/// Three passes, each only over what the earlier ones left: the first unmatched tier of the
/// same name; a renamed line with the same angle and indices; and, when the tier count did
/// not change, the tier in the same position (a line edited in more than one place).
pub(super) fn pair_tiers(base: &[AscTier], edited: &[AscTier]) -> Vec<Option<usize>> {
    let mut taken = vec![false; base.len()];
    let mut pairs: Vec<Option<usize>> = vec![None; edited.len()];
    for (pair, tier) in pairs.iter_mut().zip(edited) {
        *pair = first_free(&taken, |i| base[i].name == tier.name);
        if let Some(index) = *pair {
            taken[index] = true;
        }
    }
    for (pair, tier) in pairs.iter_mut().zip(edited) {
        if pair.is_some() {
            continue;
        }
        *pair = first_free(&taken, |i| {
            same(base[i].angle_deg, tier.angle_deg) && same_all(&base[i].indices, &tier.indices)
        });
        if let Some(index) = *pair {
            taken[index] = true;
        }
    }
    if base.len() == edited.len() {
        for (position, pair) in pairs.iter_mut().enumerate() {
            if pair.is_none() && !taken[position] {
                taken[position] = true;
                *pair = Some(position);
            }
        }
    }
    pairs
}

// --- checking the edited text ------------------------------------------------------------

/// How a refusal names the concave tiers of `design`: "the concave tier Groove", or "the
/// concave tiers Groove, Dimple" for several. A tier without a name is "number N" (its place
/// among the concave tiers).
fn concave_tiers_phrase(design: &Design) -> String {
    let names: Vec<String> = design
        .concave_tiers
        .iter()
        .enumerate()
        .map(|(position, concave)| {
            if concave.name.is_empty() {
                format!("number {}", position + 1)
            } else {
                concave.name.clone()
            }
        })
        .collect();
    let noun = if names.len() == 1 { "tier" } else { "tiers" };
    format!("the concave {noun} {}", list(&names))
}

/// Refuses any change of the gear while the design has concave tiers, at the gear line.
///
/// The concave tiers are not in the text (they are footnotes), so nothing the text says can
/// move their tooth numbers, and a numbered tooth is a different angle on a different gear
/// (index 24 is 90 degrees on 96 teeth and 45 on 192). A smaller gear can push them off the
/// wheel, a bigger one leaves them turned against the flat facets with nothing to say so.
/// The design settings remap the concave tiers with the flat ones, so the refusal points
/// there. A design that has no concave tier takes the new gear as written, and the same
/// gear never refuses (even when a design is already off its wheel).
fn check_concave_gear(
    design: &Design,
    base: &ParsedText,
    edited: &ParsedText,
) -> Result<(), TextProblem> {
    let gear = edited.schedule.gear_teeth_abs();
    let was = base.schedule.gear_teeth_abs();
    if gear == was || design.concave_tiers.is_empty() {
        return Ok(());
    }
    Err(TextProblem::at(
        edited.gear_line,
        format!(
            "The gear cannot change in the text while the design has {}. Concave tooth numbers \
             are not in the text, so they cannot follow a new gear: on the {gear}-tooth gear \
             they would keep the same numbers and sit at a different angle. Change the gear in \
             the design settings instead (that moves them too), or keep {was} teeth.",
            concave_tiers_phrase(design)
        ),
    ))
}

/// Refuses what the parser accepts but the design cannot hold, with the tier's line: an
/// angle outside -90 to 90 degrees, an index off the gear, a name a concave tier already
/// has, and (at the gear line) any change of the gear while the design has concave tiers.
/// A value the text did not change is not checked (a catalogue design may carry values
/// outside these ranges), except that every index is checked again when the gear changed.
pub(super) fn check_edited(
    design: &Design,
    base: &ParsedText,
    edited: &ParsedText,
    pairs: &[Option<usize>],
) -> Result<(), TextProblem> {
    check_concave_gear(design, base, edited)?;
    let gear = edited.schedule.gear_teeth_abs();
    let gear_changed = gear != base.schedule.gear_teeth_abs();
    for (position, (tier, pair)) in edited.schedule.tiers.iter().zip(pairs).enumerate() {
        let line = edited.tier_line(position);
        let before = pair.map(|index| &base.schedule.tiers[index]);
        let angle_is_new = before.is_none_or(|b| !same(b.angle_deg, tier.angle_deg));
        if angle_is_new && !(-90.0..=90.0).contains(&tier.angle_deg) {
            return Err(TextProblem::at(
                line,
                format!(
                    "The angle {} is outside the range -90 to 90 degrees.",
                    tier.angle_deg
                ),
            ));
        }
        let indices_are_new =
            gear_changed || before.is_none_or(|b| !same_all(&b.indices, &tier.indices));
        if indices_are_new
            && let Some(bad) = tier
                .indices
                .iter()
                .find(|index| !(0.0..=f64::from(gear)).contains(*index))
        {
            return Err(TextProblem::at(
                line,
                format!(
                    "The index {bad} is not on the {gear}-tooth gear. Indices run from 0 to {gear}."
                ),
            ));
        }
        if let Some(clash) = design.concave_tiers.iter().find(|concave| {
            !concave.name.is_empty() && tier.names().contains(&concave.name.as_str())
        }) {
            return Err(TextProblem::at(
                line,
                format!(
                    "The name {} belongs to a concave tier. Pick another name.",
                    clash.name
                ),
            ));
        }
    }
    Ok(())
}
