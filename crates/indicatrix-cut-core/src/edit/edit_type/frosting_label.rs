//! The cutter-facing sentence of an [`super::Edit::SetTierFrosting`], for the undo and redo
//! hints and the history list.

use crate::design::TierFrosting;

/// "Frost tier P1" for a whole tier, "Frost 3 facets of P1" for single facets, and "Polish
/// tier P1" for a clear -- `label` is the tier as the table shows it.
pub(super) fn describe_frosting(label: &str, frosting: Option<&TierFrosting>) -> String {
    match frosting {
        None => format!("Polish tier {label}"),
        Some(TierFrosting::Tier) => format!("Frost tier {label}"),
        Some(TierFrosting::Facets(positions)) => format!(
            "Frost {} {} of {label}",
            positions.len(),
            if positions.len() == 1 {
                "facet"
            } else {
                "facets"
            }
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sentence_names_the_tier_and_what_changes() {
        assert_eq!(describe_frosting("P1", None), "Polish tier P1");
        assert_eq!(
            describe_frosting("P1", Some(&TierFrosting::Tier)),
            "Frost tier P1"
        );
        assert_eq!(
            describe_frosting("C2", Some(&TierFrosting::Facets(vec![12.0]))),
            "Frost 1 facet of C2"
        );
        assert_eq!(
            describe_frosting("C2", Some(&TierFrosting::Facets(vec![12.0, 36.0, 60.0]))),
            "Frost 3 facets of C2"
        );
    }
}
