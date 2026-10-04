//! Canonical facet and tier labelling according to international faceting standards.
//!
//! Replaces the legacy 123-ABC labelling system (digits 1, 2, 3... for pavilion,
//! letters A, B, C... for crown) with the global standard PF-P-G-C tier labelling:
//! - `PF`: Pavilion Foundation / Preform (`PF1`, `PF2`, ...)
//! - `P`: Pavilion (`P1`, `P2`, `P3`, ...)
//! - `G`: Girdle (`G1`, `G2`, ...)
//! - `C`: Crown (`C1`, `C2`, `C3`, ...)
//! - `Table`: Flat table facet (`Table` or `T`)
//! - `Culet`: Flat culet facet (`Culet`)

use super::ConstraintTier;
use indicatrix::geometry::meet_solver::{Block, MeetTierInput, classify_blocks};

/// Canonical tier labelling information for display and diagram annotation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TierLabelInfo {
    /// Canonical short code for diagrams, index-wheel references, and compact displays
    /// (e.g. "PF1", "P1", "G1", "C1", "Table", "Culet").
    pub code: String,
    /// Canonical display name for cutting sheets and tables (e.g. "P1", "C1", or
    /// "P1 (Pavilion Main)" / "Table" if the tier has a distinct descriptive name).
    pub display_name: String,
}

/// Whether `name` represents a legacy 123-ABC facet label (e.g. "1", "2", "A", "B", "G", "T").
#[must_use]
pub fn is_legacy_123_abc(name: &str) -> bool {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return false;
    }
    // Pure digits e.g. "1", "2", "12"
    if trimmed.chars().all(|c| c.is_ascii_digit()) {
        return true;
    }
    // Digits with single trailing letter e.g. "1A", "2B"
    if let Some(last) = trimmed.chars().next_back()
        && last.is_ascii_alphabetic()
    {
        let prefix = &trimmed[..trimmed.len() - last.len_utf8()];
        if !prefix.is_empty() && prefix.chars().all(|c| c.is_ascii_digit()) {
            return true;
        }
    }
    // Single letter e.g. "A", "B", "G", "T"
    trimmed.len() == 1
        && trimmed
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic())
}

/// Converts a single legacy facet or tier name to its canonical PF-P-G-C label.
///
/// If `name` already follows modern conventions (or is descriptive), it is returned
/// unchanged (or normalized).
#[must_use]
pub fn convert_legacy_facet_name(name: &str, block: Block, ordinal_in_block: usize) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return match block {
            Block::Crown => format!("C{ordinal_in_block}"),
            Block::Pavilion => format!("P{ordinal_in_block}"),
            Block::Girdle => format!("G{ordinal_in_block}"),
        };
    }
    if trimmed.eq_ignore_ascii_case("t") || trimmed.eq_ignore_ascii_case("table") {
        return "Table".to_string();
    }
    if trimmed.eq_ignore_ascii_case("culet") {
        return "Culet".to_string();
    }
    if trimmed.eq_ignore_ascii_case("g") || trimmed.eq_ignore_ascii_case("girdle") {
        return format!("G{ordinal_in_block}");
    }
    // Digits on pavilion side: "1" -> "P1"
    if block == Block::Pavilion && trimmed.chars().all(|c| c.is_ascii_digit()) {
        return format!("P{trimmed}");
    }
    // Single letter on crown side: "A" -> "C1", "B" -> "C2"
    if block == Block::Crown
        && trimmed.len() == 1
        && let Some(first) = trimmed.chars().next()
        && first.is_ascii_alphabetic()
    {
        let letter_idx = (first.to_ascii_uppercase() as u8).saturating_sub(b'A') + 1;
        return format!("C{letter_idx}");
    }
    trimmed.to_string()
}

/// Whether `name` indicates a pavilion-side facet (e.g. starts with "PF", "P", or "pavilion").
#[must_use]
pub fn name_indicates_pavilion(name: &str) -> bool {
    let lower = name.trim().to_ascii_lowercase();
    if lower.starts_with("pf") || lower.starts_with("pavilion") {
        return true;
    }
    if let Some(rest) = lower.strip_prefix('p') {
        return rest.is_empty()
            || rest.starts_with(|c: char| c.is_ascii_digit() || c == '_' || c == '-');
    }
    false
}

/// Computes canonical PF-P-G-C labels for all tiers in `tiers`.
#[must_use]
pub fn compute_tier_labels(tiers: &[ConstraintTier]) -> Vec<TierLabelInfo> {
    let inputs: Vec<MeetTierInput> = tiers
        .iter()
        .map(|tier| MeetTierInput {
            angle_deg: tier.angle_deg,
            indices: tier.indices.clone(),
            constraint: tier.constraint.clone(),
            names: tier.names().into_iter().map(str::to_string).collect(),
        })
        .collect();
    let blocks = classify_blocks(&inputs);

    let mut pf_counter = 0usize;
    let mut p_counter = 0usize;
    let mut g_counter = 0usize;
    let mut c_counter = 0usize;

    tiers
        .iter()
        .enumerate()
        .map(|(i, tier)| {
            let block = blocks.get(i).copied().unwrap_or(Block::Crown);
            let name_trimmed = tier.name.trim();

            let is_table = (tier.angle_deg == 0.0
                && !tier.angle_deg.is_sign_negative()
                && block == Block::Crown)
                || name_trimmed.eq_ignore_ascii_case("table")
                || name_trimmed.eq_ignore_ascii_case("t");

            let is_culet = (tier.angle_deg == 0.0 && tier.angle_deg.is_sign_negative())
                || name_trimmed.eq_ignore_ascii_case("culet");

            if is_table {
                return TierLabelInfo {
                    code: "Table".to_string(),
                    display_name: "Table".to_string(),
                };
            }

            if is_culet {
                return TierLabelInfo {
                    code: "Culet".to_string(),
                    display_name: "Culet".to_string(),
                };
            }

            let code = match block {
                Block::Pavilion => {
                    let upper = name_trimmed.to_ascii_uppercase();
                    let lower = name_trimmed.to_ascii_lowercase();
                    let is_pf = upper.starts_with("PF")
                        || lower.contains("preform")
                        || lower.contains("foundation");
                    if is_pf {
                        pf_counter += 1;
                        format!("PF{pf_counter}")
                    } else {
                        p_counter += 1;
                        format!("P{p_counter}")
                    }
                }
                Block::Girdle => {
                    g_counter += 1;
                    format!("G{g_counter}")
                }
                Block::Crown => {
                    c_counter += 1;
                    format!("C{c_counter}")
                }
            };

            let display_name = if name_trimmed.is_empty()
                || is_legacy_123_abc(name_trimmed)
                || name_trimmed.eq_ignore_ascii_case(&code)
            {
                code.clone()
            } else if name_trimmed
                .to_ascii_uppercase()
                .starts_with(&code.to_ascii_uppercase())
            {
                name_trimmed.to_string()
            } else {
                format!("{code} ({name_trimmed})")
            };

            TierLabelInfo { code, display_name }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_legacy_123_abc() {
        assert!(is_legacy_123_abc("1"));
        assert!(is_legacy_123_abc("2"));
        assert!(is_legacy_123_abc("12"));
        assert!(is_legacy_123_abc("1A"));
        assert!(is_legacy_123_abc("A"));
        assert!(is_legacy_123_abc("B"));
        assert!(is_legacy_123_abc("G"));
        assert!(is_legacy_123_abc("T"));

        assert!(!is_legacy_123_abc("P1"));
        assert!(!is_legacy_123_abc("PF1"));
        assert!(!is_legacy_123_abc("C1"));
        assert!(!is_legacy_123_abc("G1"));
        assert!(!is_legacy_123_abc("Table"));
        assert!(!is_legacy_123_abc("Pavilion Main"));
    }

    #[test]
    fn test_convert_legacy_facet_name() {
        assert_eq!(convert_legacy_facet_name("1", Block::Pavilion, 1), "P1");
        assert_eq!(convert_legacy_facet_name("2", Block::Pavilion, 2), "P2");
        assert_eq!(convert_legacy_facet_name("A", Block::Crown, 1), "C1");
        assert_eq!(convert_legacy_facet_name("B", Block::Crown, 2), "C2");
        assert_eq!(convert_legacy_facet_name("G", Block::Girdle, 1), "G1");
        assert_eq!(convert_legacy_facet_name("T", Block::Crown, 1), "Table");
    }

    #[test]
    fn test_standard_round_brilliant_canonical_labels() {
        let tiers = ConstraintTier::standard_round_brilliant();
        let labels = compute_tier_labels(&tiers);

        assert_eq!(labels.len(), 8);
        assert_eq!(labels[0].code, "Table");
        assert_eq!(labels[0].display_name, "Table");

        assert_eq!(labels[1].code, "C1");
        assert_eq!(labels[1].display_name, "C1 (Star)");

        assert_eq!(labels[2].code, "C2");
        assert_eq!(labels[2].display_name, "C2 (Crown Main)");

        assert_eq!(labels[3].code, "C3");
        assert_eq!(labels[3].display_name, "C3 (Upper Girdle)");

        assert_eq!(labels[4].code, "G1");
        assert_eq!(labels[4].display_name, "G1 (Girdle)");

        assert_eq!(labels[5].code, "P1");
        assert_eq!(labels[5].display_name, "P1 (Pavilion Main)");

        assert_eq!(labels[6].code, "P2");
        assert_eq!(labels[6].display_name, "P2 (Lower Girdle)");

        assert_eq!(labels[7].code, "Culet");
        assert_eq!(labels[7].display_name, "Culet");
    }
}
