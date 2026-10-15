//! The list models of the setup panel: materials, model sizes and the design picker.

use indicatrix_ai::model::ModelTier;

use crate::{
    DesignItem, MaterialItem, TierItem,
    dataset::MINERAL_SPECTRUM,
    ui_state::{DesignInfo, DesignSplit},
};

/// Every model size, smallest first.
pub const TIERS: [ModelTier; 5] = [
    ModelTier::Small,
    ModelTier::Medium,
    ModelTier::Large,
    ModelTier::ExtraLarge,
    ModelTier::ExtraExtraLarge,
];

/// The refractive-index spectrum as the target-material list.
pub fn materials() -> Vec<MaterialItem> {
    MINERAL_SPECTRUM
        .iter()
        .map(|entry| MaterialItem {
            name: entry.name.into(),
            n_d: entry.refractive_index,
            critical_deg: entry.critical_angle_deg,
            has_preset: entry.preset.is_some(),
        })
        .collect()
}

/// The model sizes with their parameter counts and shapes.
pub fn tiers() -> Vec<TierItem> {
    TIERS
        .iter()
        .map(|&tier| TierItem {
            code: tier.code_str().into(),
            params: format_params(tier.param_count_approx()).into(),
            shape: tier_shape(tier).into(),
        })
        .collect()
}

/// `~103k parameters`, `~3.2M parameters`.
pub fn format_params(count: usize) -> String {
    if count >= 1_000_000 {
        format!("~{:.1}M parameters", count as f64 / 1_000_000.0)
    } else if count >= 1_000 {
        // Integer round-half-up to thousands; equals rounding count / 1000 for every count here.
        format!("~{}k parameters", (count + 500) / 1_000)
    } else {
        format!("~{count} parameters")
    }
}

/// `width 64 · 4 heads · 3 layers`.
pub fn tier_shape(tier: ModelTier) -> String {
    let (width, heads, layers) = tier.config();
    format!("width {width} · {heads} heads · {layers} layers")
}

/// Whether `code` names a model size ("S", "M", "L", "XL", "XXL").
pub fn is_known_tier(code: &str) -> bool {
    TIERS.iter().any(|tier| tier.code_str() == code)
}

/// The split name the picker shows.
pub const fn split_name(split: DesignSplit) -> &'static str {
    match split {
        DesignSplit::Train => "Train",
        DesignSplit::Validation => "Validation",
        DesignSplit::Test => "Test",
        DesignSplit::Skipped => "Skipped",
    }
}

/// Case-insensitive substring match on the title or the file name; an empty filter matches all.
pub fn matches_filter(design: &DesignInfo, filter: &str) -> bool {
    let needle = filter.trim().to_lowercase();
    needle.is_empty()
        || design.title.to_lowercase().contains(&needle)
        || design.filename.to_lowercase().contains(&needle)
}

/// The picker rows for `filter`.
pub fn filter_designs(designs: &[DesignInfo], filter: &str) -> Vec<DesignItem> {
    designs
        .iter()
        .filter(|design| matches_filter(design, filter))
        .map(|design| DesignItem {
            index: i32::try_from(design.index).unwrap_or(i32::MAX),
            title: design.title.as_str().into(),
            filename: design.filename.as_str().into(),
            split: split_name(design.split).into(),
        })
        .collect()
}

/// The design after (`forward`) or before the one with index `current`, wrapping around and
/// skipping designs the trainer cannot use. `None` when no usable design exists.
pub fn step_design(designs: &[DesignInfo], current: i32, forward: bool) -> Option<usize> {
    let count = designs.len();
    if count == 0 {
        return None;
    }
    let start = designs
        .iter()
        .position(|design| i32::try_from(design.index).ok() == Some(current));
    (1..=count)
        .map(|step| match (start, forward) {
            (Some(at), true) => (at + step) % count,
            (Some(at), false) => (at + count - step % count) % count,
            (None, true) => step - 1,
            (None, false) => count - step,
        })
        .map(|position| &designs[position])
        .find(|design| !matches!(design.split, DesignSplit::Skipped))
        .map(|design| design.index)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn design(index: usize, title: &str, filename: &str, split: DesignSplit) -> DesignInfo {
        DesignInfo {
            index,
            title: title.to_owned(),
            filename: filename.to_owned(),
            split,
        }
    }

    fn sample() -> Vec<DesignInfo> {
        vec![
            design(0, "Round Brilliant", "rb.asc", DesignSplit::Train),
            design(1, "Emerald Cut", "em.asc", DesignSplit::Skipped),
            design(2, "Oval", "OVAL-2.asc", DesignSplit::Validation),
            design(3, "Cushion", "cu.asc", DesignSplit::Test),
        ]
    }

    #[test]
    fn params_read_in_thousands_and_millions() {
        assert_eq!(format_params(39_950), "~40k parameters");
        assert_eq!(format_params(103_110), "~103k parameters");
        assert_eq!(format_params(3_173_390), "~3.2M parameters");
        assert_eq!(format_params(9_487_110), "~9.5M parameters");
        assert_eq!(format_params(512), "~512 parameters");
    }

    #[test]
    fn tier_list_has_five_sizes_with_shapes() {
        let list = tiers();
        assert_eq!(list.len(), 5);
        assert_eq!(list[1].code.as_str(), "M");
        assert_eq!(list[1].shape.as_str(), "width 64 · 4 heads · 3 layers");
    }

    #[test]
    fn material_list_follows_the_spectrum() {
        let list = materials();
        assert_eq!(list.len(), MINERAL_SPECTRUM.len());
        let fluorite = &list[0];
        assert_eq!(fluorite.name.as_str(), "Fluorite");
        assert!(!fluorite.has_preset);
        assert!(list.iter().any(|m| m.has_preset));
    }

    #[test]
    fn known_tiers_are_the_five_codes() {
        for code in ["S", "M", "L", "XL", "XXL"] {
            assert!(is_known_tier(code));
        }
        assert!(!is_known_tier("XS"));
        assert!(!is_known_tier("m"));
    }

    #[test]
    fn filter_is_case_insensitive_on_title_and_file_name() {
        let designs = sample();
        assert_eq!(filter_designs(&designs, "").len(), 4);
        assert_eq!(filter_designs(&designs, "  ").len(), 4);
        assert_eq!(filter_designs(&designs, "round").len(), 1);
        let by_file = filter_designs(&designs, "oval-2");
        assert_eq!(by_file.len(), 1);
        assert_eq!(by_file[0].index, 2);
        assert_eq!(by_file[0].split.as_str(), "Validation");
        assert_eq!(filter_designs(&designs, "zzz").len(), 0);
    }

    #[test]
    fn stepping_wraps_and_skips_unusable_designs() {
        let designs = sample();
        assert_eq!(step_design(&designs, 0, true), Some(2));
        assert_eq!(step_design(&designs, 3, true), Some(0));
        assert_eq!(step_design(&designs, 0, false), Some(3));
        assert_eq!(step_design(&designs, 2, false), Some(0));
    }

    #[test]
    fn stepping_without_a_current_design_starts_at_the_ends() {
        let designs = sample();
        assert_eq!(step_design(&designs, -1, true), Some(0));
        assert_eq!(step_design(&designs, -1, false), Some(3));
        assert_eq!(step_design(&[], 0, true), None);
        let only_skipped = vec![design(0, "A", "a.asc", DesignSplit::Skipped)];
        assert_eq!(step_design(&only_skipped, 0, true), None);
    }
}
