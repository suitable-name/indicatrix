//! The small helpers behind one row of an angle sweep: writing the swept angle (with its
//! relations), counting the facets that vanish or come out very small, and noting a lost
//! girdle band.

use super::SweepRow;
use indicatrix_cut_core::{Design, ManufacturabilityWarning};

/// Sets the swept tier's angle on the clone and lets every tier whose angle follows a
/// relation follow, as a real edit does. The error is the relation's plain-English
/// message (a result outside 0 to 90 degrees, a loop).
pub(super) fn set_swept_angle(
    scratch: &mut Design,
    tier: usize,
    angle_deg: f64,
) -> Result<(), String> {
    if let Some(swept) = scratch.tiers.get_mut(tier) {
        swept.angle_deg = angle_deg;
    }
    let updates = scratch
        .evaluate_relations()
        .map_err(|error| error.to_string())?;
    for (position, deg) in updates {
        if let Some(driven) = scratch.tiers.get_mut(position) {
            driven.angle_deg = deg;
        }
    }
    Ok(())
}

/// Counts the vanishing and undersized facets of a row and says so in its notes.
pub(super) fn count_warnings(
    warnings: &[ManufacturabilityWarning],
    notes: &mut Vec<String>,
) -> usize {
    let vanishing = warnings
        .iter()
        .filter(|warning| matches!(warning, ManufacturabilityWarning::VanishingFacet { .. }))
        .count();
    let undersized = warnings
        .iter()
        .filter(|warning| matches!(warning, ManufacturabilityWarning::UndersizedFacet { .. }))
        .count();
    if vanishing > 0 {
        notes.push(format!(
            "{vanishing} facet{} vanish{}.",
            if vanishing == 1 { "" } else { "s" },
            if vanishing == 1 { "es" } else { "" }
        ));
    }
    if undersized > 0 {
        notes.push(format!(
            "{undersized} facet{} very small.",
            if undersized == 1 { " is" } else { "s are" }
        ));
    }
    vanishing + undersized
}

/// Says "the girdle band is gone" on every valid row that lost the band the current
/// row has.
pub(super) fn note_lost_girdles(rows: &mut [SweepRow]) {
    let had_girdle = rows
        .iter()
        .find(|row| row.is_current)
        .and_then(|row| row.metrics)
        .is_some_and(|metrics| metrics.has_girdle);
    if !had_girdle {
        return;
    }
    for row in rows.iter_mut().filter(|row| !row.is_current) {
        if row.metrics.is_some_and(|metrics| !metrics.has_girdle) {
            row.notes
                .push("The girdle band is gone at this angle.".to_owned());
        }
    }
}
