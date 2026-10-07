//! Canonical facet and tier labelling according to international faceting standards.
//!
//! Replaces the legacy 123-ABC labelling system (digits 1, 2, 3... for pavilion,
//! letters A, B, C... for crown) with the global standard PF-P-G-C tier labelling:
//! - `PF`: Pavilion Foundation / Preform (`PF1`, `PF2`, ...); only a tier whose own name
//!   starts with "PF" or says "preform" or "foundation" gets one
//! - `P`: Pavilion (`P1`, `P2`, `P3`, ...)
//! - `G`: Girdle (`G1`, `G2`, ...)
//! - `C`: Crown (`C1`, `C2`, `C3`, ...)
//! - `T`: the flat table facet (shown as "Table" in names)
//! - `Culet`: Flat culet facet (`Culet`)
//!
//! # Numbered in cutting order
//!
//! The codes are numbered per letter in the order the tiers are CUT
//! ([`Design::cutting_order`](super::Design::cutting_order)), the way the fantasy-cut
//! template does: `P` and `G` count independently, even when they interleave (`P1`, `P2`,
//! `G1`, `G2`, `P3`), and a concave pavilion tier continues the `P` count while a concave
//! crown tier continues the `C` count. [`compute_tier_labels`] numbers a plain tier list;
//! [`Design::tier_codes`] also returns the concave tiers' codes. Concave tiers come after
//! every flat tier of their section, so the flat codes do not depend on them.

use super::{
    ConcaveTier, ConstraintTier, Design, TierRef,
    cutting_order::{flat_cutting_order, meet_inputs},
};
use indicatrix::geometry::meet_solver::{Block, classify_blocks};

/// Canonical tier labelling information for display and diagram annotation.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TierLabelInfo {
    /// Canonical short code for diagrams, index-wheel references, and compact displays
    /// (e.g. "PF1", "P1", "G1", "C1", "T", "Culet").
    pub code: String,
    /// Canonical display name for cutting sheets and tables (e.g. "P1", "C1", or
    /// "P1 (Pavilion Main)" / "Table" if the tier has a distinct descriptive name).
    pub display_name: String,
}

/// The codes of every tier of a design, flat and concave, in the tiers' stored order.
///
/// Built by [`Design::tier_codes`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DesignTierCodes {
    /// One entry per [`Design::tiers`] tier, as [`compute_tier_labels`] gives them.
    pub flat: Vec<TierLabelInfo>,
    /// One entry per [`Design::concave_tiers`] tier. A pavilion-side tier continues the
    /// `P` count after the flat pavilion tiers, a crown-side one the `C` count.
    pub concave: Vec<TierLabelInfo>,
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
/// unchanged (or normalized). The table becomes `T`, like the code
/// [`compute_tier_labels`] gives it.
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
        return "T".to_string();
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

/// How many tiers of each letter have been numbered so far.
#[derive(Debug, Clone, Copy, Default)]
struct LetterCounts {
    pf: usize,
    p: usize,
    g: usize,
    c: usize,
}

/// What a tier is shown as next to its `code`: the code alone when the tier is unnamed, has
/// an old-style name or is named like its code, the name when it already starts with the
/// code, else `"<code> (<name>)"`.
fn display_name_for(code: &str, name: &str) -> String {
    if name.is_empty() || is_legacy_123_abc(name) || name.eq_ignore_ascii_case(code) {
        code.to_string()
    } else if name
        .to_ascii_uppercase()
        .starts_with(&code.to_ascii_uppercase())
    {
        name.to_string()
    } else {
        format!("{code} ({name})")
    }
}

/// The label of one flat tier of block `block`, advancing `counts` for its letter.
fn label_of(tier: &ConstraintTier, block: Block, counts: &mut LetterCounts) -> TierLabelInfo {
    let name_trimmed = tier.name.trim();

    let is_table =
        (tier.angle_deg == 0.0 && !tier.angle_deg.is_sign_negative() && block == Block::Crown)
            || name_trimmed.eq_ignore_ascii_case("table")
            || name_trimmed.eq_ignore_ascii_case("t");

    let is_culet = (tier.angle_deg == 0.0 && tier.angle_deg.is_sign_negative())
        || name_trimmed.eq_ignore_ascii_case("culet");

    if is_table {
        return TierLabelInfo {
            code: "T".to_string(),
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
                counts.pf += 1;
                format!("PF{}", counts.pf)
            } else {
                counts.p += 1;
                format!("P{}", counts.p)
            }
        }
        Block::Girdle => {
            counts.g += 1;
            format!("G{}", counts.g)
        }
        Block::Crown => {
            counts.c += 1;
            format!("C{}", counts.c)
        }
    };

    TierLabelInfo {
        display_name: display_name_for(&code, name_trimmed),
        code,
    }
}

/// The labels of `tiers`, numbering them in the order `cut_order` lists their indices, and
/// the letter counts that leaves for a caller that goes on to number concave tiers.
fn label_flat_tiers(
    tiers: &[ConstraintTier],
    cut_order: impl IntoIterator<Item = usize>,
) -> (Vec<TierLabelInfo>, LetterCounts) {
    let blocks = classify_blocks(&meet_inputs(tiers));
    let mut labels = vec![TierLabelInfo::default(); tiers.len()];
    let mut counts = LetterCounts::default();
    for index in cut_order {
        let block = blocks.get(index).copied().unwrap_or(Block::Crown);
        labels[index] = label_of(&tiers[index], block, &mut counts);
    }
    (labels, counts)
}

/// Computes canonical PF-P-G-C labels for all tiers in `tiers`, numbered per letter in the
/// order the tiers are cut ([`flat_cutting_order`]), one entry per tier in stored order.
///
/// A concave tier would continue the count after the flat ones; this function takes a plain
/// tier list and knows no concave tiers, so use [`Design::tier_codes`] when the design
/// has any.
#[must_use]
pub fn compute_tier_labels(tiers: &[ConstraintTier]) -> Vec<TierLabelInfo> {
    label_flat_tiers(tiers, flat_cutting_order(tiers)).0
}

impl Design {
    /// The code of every tier, flat and concave: the flat tiers as [`compute_tier_labels`]
    /// numbers them, then each concave tier continuing its letter (`P` for a pavilion-side
    /// tier, `C` for a crown-side one) in [`Self::cutting_order`].
    ///
    /// A flat code never depends on the concave tiers, because every concave tier is cut
    /// after all flat tiers of its section.
    #[must_use]
    pub fn tier_codes(&self) -> DesignTierCodes {
        let order = self.cutting_order();
        let flat_order = order.iter().filter_map(|tier| match tier {
            TierRef::Flat(index) => Some(*index),
            TierRef::Concave(_) => None,
        });
        let (flat, mut counts) = label_flat_tiers(&self.tiers, flat_order);
        let mut concave = vec![TierLabelInfo::default(); self.concave_tiers.len()];
        for tier in &order {
            let TierRef::Concave(index) = tier else {
                continue;
            };
            let concave_tier: &ConcaveTier = &self.concave_tiers[*index];
            let code = if concave_tier.is_crown_side() {
                counts.c += 1;
                format!("C{}", counts.c)
            } else {
                counts.p += 1;
                format!("P{}", counts.p)
            };
            concave[*index] = TierLabelInfo {
                display_name: display_name_for(&code, concave_tier.name.trim()),
                code,
            };
        }
        DesignTierCodes { flat, concave }
    }
}

#[cfg(test)]
mod tests;
