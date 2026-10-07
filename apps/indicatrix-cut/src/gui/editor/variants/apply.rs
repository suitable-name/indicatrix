//! "Open variant": the one edit that turns the open design into a saved variant.
//!
//! The edit is a single undo step. The cutting instructions (tiers with their ids, notes,
//! targets and relations, the gear and the header lines) go in with `Edit::ReplaceSchedule`;
//! the rough, the girdle size, the material and the concave tiers have edits of their own, and
//! each is included only when the variant differs from the open design in that part. So a
//! variant that only moves two angles is one `ReplaceSchedule`, and one that also changes the
//! material is a short batch. Undo takes the whole thing back.

use indicatrix_cut_core::{Design, Edit, ScheduleState};

/// Whether the cutting instructions of `current` and `variant` differ.
fn schedule_differs(current: &Design, variant: &Design) -> bool {
    // Everything that is not part of the schedule is copied over from the open design, so the
    // comparison can only be decided by the schedule.
    let mut probe = variant.clone();
    probe.preform = current.preform;
    probe.preform_y_offset = current.preform_y_offset;
    probe.girdle_diameter_mm = current.girdle_diameter_mm;
    probe.material = current.material.clone();
    probe.concave_tiers.clone_from(&current.concave_tiers);
    probe != *current
}

/// Whether two numbers are the same value, the sign of zero included.
const fn same_bits(a: f64, b: f64) -> bool {
    a.to_bits() == b.to_bits()
}

/// The edits that turn `current` into `variant`, in the order they apply.
fn edits_between(current: &Design, variant: &Design) -> Vec<Edit> {
    let concave_differs = current.concave_tiers != variant.concave_tiers;
    let mut edits = Vec::new();
    // The old concave tiers go first, so none of them can clash with the new flat names.
    if concave_differs {
        edits.extend(
            (0..current.concave_tiers.len())
                .rev()
                .map(|index| Edit::RemoveConcaveTier { index }),
        );
    }
    if schedule_differs(current, variant) {
        edits.push(Edit::ReplaceSchedule(Box::new(ScheduleState::of(variant))));
    }
    if current.preform != variant.preform {
        edits.push(Edit::SetPreform {
            preform: variant.preform,
        });
    }
    if !same_bits(current.preform_y_offset, variant.preform_y_offset) {
        edits.push(Edit::SetPreformYOffset {
            y_offset: variant.preform_y_offset,
        });
    }
    if current.girdle_diameter_mm.map(f64::to_bits) != variant.girdle_diameter_mm.map(f64::to_bits)
    {
        edits.push(Edit::SetGirdleDiameterMm {
            girdle_diameter_mm: variant.girdle_diameter_mm,
        });
    }
    if current.material != variant.material {
        edits.push(Edit::SetMaterial {
            material: variant.material.clone(),
        });
    }
    if concave_differs {
        edits.extend(
            variant
                .concave_tiers
                .iter()
                .enumerate()
                .map(|(index, tier)| Edit::AddConcaveTier {
                    index,
                    tier: tier.clone(),
                }),
        );
    }
    edits
}

/// The words the history shows for opening the variant called `name` (`Open variant "Steeper
/// crown"`), instead of the words of the edit it is made of ("Edit instructions as text").
pub(super) fn open_label(name: &str) -> String {
    format!("Open variant \"{}\"", name.trim())
}

/// The single edit that makes `current` the same design as `variant`, or `None` when they
/// already are.
///
/// The edit is tried on a copy first, so a variant that cannot be put in place is refused
/// here, with the design untouched, instead of failing halfway through the real edit.
///
/// # Errors
///
/// A plain sentence when the edit is refused, or when it would not give exactly the saved
/// design.
pub(super) fn replacement_edit(current: &Design, variant: &Design) -> Result<Option<Edit>, String> {
    if current == variant {
        return Ok(None);
    }
    let mut edits = edits_between(current, variant);
    let edit = if edits.len() == 1 {
        edits.remove(0)
    } else {
        Edit::Batch(edits)
    };
    let mut scratch = current.clone();
    scratch
        .apply_edit(edit.clone())
        .map_err(|error| format!("The design cannot take this variant. {error}"))?;
    if scratch != *variant {
        return Err("The design cannot be put back exactly as this variant was saved.".to_owned());
    }
    Ok(Some(edit))
}
